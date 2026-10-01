use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use super::types::{
    CheckpointEntry, CheckpointFile, CheckpointRecord, RevertOutcome, RevertRecord,
};
use crate::session::get_app_home;

const MAX_SNAPSHOT_BYTES: usize = 5 * 1024 * 1024;
const MAX_CACHE_SIZE: usize = 2;

#[derive(Debug)]
pub struct CheckpointManager {
    base_dir: Option<PathBuf>,
    cache: Mutex<HashMap<String, Vec<CheckpointEntry>>>,
    lru_order: Mutex<Vec<String>>,
}

impl CheckpointManager {
    pub fn new(base_dir: Option<PathBuf>) -> Self {
        Self {
            base_dir,
            cache: Mutex::new(HashMap::new()),
            lru_order: Mutex::new(Vec::new()),
        }
    }

    fn checkpoints_dir(&self) -> PathBuf {
        match &self.base_dir {
            Some(d) => d.clone(),
            None => get_app_home().join("checkpoints"),
        }
    }

    fn file_for(&self, thread_id: &str) -> PathBuf {
        self.checkpoints_dir().join(format!("{}.jsonl", thread_id))
    }

    fn touch_lru(&self, thread_id: &str) {
        let mut order = self.lru_order.lock().unwrap();
        order.retain(|id| id != thread_id);
        order.push(thread_id.to_string());

        if order.len() > MAX_CACHE_SIZE {
            let oldest = order.remove(0);
            let mut cache = self.cache.lock().unwrap();
            cache.remove(&oldest);
        }
    }

    /// 清空内存缓存
    pub fn clear_memory_cache(&self, thread_id: Option<&str>) {
        let mut cache = self.cache.lock().unwrap();
        let mut order = self.lru_order.lock().unwrap();
        if let Some(id) = thread_id {
            cache.remove(id);
            order.retain(|item| item != id);
        } else {
            cache.clear();
            order.clear();
        }
    }

    /// 加载检查点记录流
    pub fn load(&self, thread_id: &str, force: bool) -> Result<Option<Vec<CheckpointEntry>>> {
        if !force {
            let cached = {
                let cache = self.cache.lock().unwrap();
                cache.get(thread_id).cloned()
            };
            if let Some(entries) = cached {
                self.touch_lru(thread_id);
                return Ok(Some(entries));
            }
        }


        let file_path = self.file_for(thread_id);
        if !file_path.exists() {
            return Ok(None);
        }

        let file = File::open(&file_path)?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for line_res in reader.lines() {
            let line = match line_res {
                Ok(l) => l,
                Err(_) => continue,
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Ok(entry) = serde_json::from_str::<CheckpointEntry>(trimmed) {
                entries.push(entry);
            }
        }

        {
            let mut cache = self.cache.lock().unwrap();
            cache.insert(thread_id.to_string(), entries.clone());
        }
        self.touch_lru(thread_id);

        Ok(Some(entries))
    }

    fn append_entry(&self, thread_id: &str, entry: CheckpointEntry) -> Result<()> {
        let dir = self.checkpoints_dir();
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }
        let file_path = self.file_for(thread_id);
        let line = serde_json::to_string(&entry)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)?;
        writeln!(file, "{}", line)?;

        let mut cache = self.cache.lock().unwrap();
        if let Some(cached) = cache.get_mut(thread_id) {
            cached.push(entry);
        }
        Ok(())
    }

    /// 拍摄改动前的文件快照
    pub fn capture(
        &self,
        thread_id: &str,
        tool_call_id: &str,
        files: &[(&str, &Path)],
    ) -> Result<CheckpointRecord> {
        let mut snapshot_files = Vec::new();

        for &(rel_path, abs_path) in files {
            let mut entry = CheckpointFile {
                path: rel_path.to_string(),
                absolute: abs_path.to_string_lossy().to_string(),
                existed: false,
                content_base64: None,
                snapshot_incomplete: None,
            };

            if abs_path.exists() {
                entry.existed = true;
                if let Ok(bytes) = fs::read(abs_path) {
                    if bytes.len() <= MAX_SNAPSHOT_BYTES {
                        entry.content_base64 = Some(BASE64.encode(&bytes));
                    } else {
                        entry.snapshot_incomplete = Some(true);
                    }
                }
            }

            snapshot_files.push(entry);
        }

        let now = now_ms();
        let record = CheckpointRecord {
            id: format!("ckpt_{}_{}", now, &uuid::Uuid::new_v4().to_string()[..6]),
            thread_id: thread_id.to_string(),
            tool_call_id: tool_call_id.to_string(),
            at: now,
            files: snapshot_files,
        };


        self.append_entry(thread_id, CheckpointEntry::Checkpoint(record.clone()))?;
        Ok(record)
    }

    fn is_invalidated(&self, entries: &[CheckpointEntry], checkpoint_id: &str) -> bool {
        entries.iter().any(|entry| match entry {
            CheckpointEntry::Revert(r) => r.checkpoint_ids.contains(&checkpoint_id.to_string()),
            _ => false,
        })
    }

    fn apply_restore(&self, files: &[CheckpointFile]) -> (Vec<String>, Vec<String>, Vec<String>) {
        let mut restored = Vec::new();
        let mut deleted = Vec::new();
        let mut skipped = Vec::new();

        // 逆序恢复，贴近原本修改时序
        for file in files.iter().rev() {
            let target_path = Path::new(&file.absolute);
            if !file.existed {
                if target_path.exists() {
                    let _ = fs::remove_file(target_path);
                }
                deleted.push(file.path.clone());
            } else if let Some(b64) = &file.content_base64 {
                if let Ok(bytes) = BASE64.decode(b64) {
                    if let Some(parent) = target_path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    if fs::write(target_path, bytes).is_ok() {
                        restored.push(file.path.clone());
                    } else {
                        skipped.push(file.path.clone());
                    }
                } else {
                    skipped.push(file.path.clone());
                }
            } else {
                skipped.push(file.path.clone());
            }
        }

        (restored, deleted, skipped)
    }

    /// 撤销单次检查点
    pub fn revert_checkpoint(
        &self,
        thread_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<RevertOutcome>> {
        let entries = match self.load(thread_id, false)? {
            Some(e) => e,
            None => return Ok(None),
        };

        let target_record = entries.iter().find_map(|e| match e {
            CheckpointEntry::Checkpoint(r) if r.id == checkpoint_id => Some(r.clone()),
            _ => None,
        });

        let record = match target_record {
            Some(r) => r,
            None => return Ok(None),
        };

        if self.is_invalidated(&entries, checkpoint_id) {
            return Ok(None);
        }

        let (restored, deleted, skipped) = self.apply_restore(&record.files);
        let now = now_ms();
        let revert = RevertRecord {
            id: format!("rvrt_{}_{}", now, &uuid::Uuid::new_v4().to_string()[..6]),
            at: now,
            checkpoint_ids: vec![checkpoint_id.to_string()],
            scope: "single".to_string(),
            detail: None,
        };


        self.append_entry(thread_id, CheckpointEntry::Revert(revert))?;

        Ok(Some(RevertOutcome {
            restored,
            deleted,
            skipped,
            invalidated: vec![checkpoint_id.to_string()],
        }))
    }

    /// 把单个文件恢复到修改前最初形态
    pub fn revert_file(
        &self,
        thread_id: &str,
        absolute_path: &Path,
    ) -> Result<Option<RevertOutcome>> {
        let entries = match self.load(thread_id, false)? {
            Some(e) => e,
            None => return Ok(None),
        };

        let abs_str = absolute_path.to_string_lossy().to_lowercase();
        let active_records: Vec<&CheckpointRecord> = entries
            .iter()
            .filter_map(|e| match e {
                CheckpointEntry::Checkpoint(r) if !self.is_invalidated(&entries, &r.id) => Some(r),
                _ => None,
            })
            .filter(|r| {
                r.files
                    .iter()
                    .any(|f| Path::new(&f.absolute).to_string_lossy().to_lowercase() == abs_str)
            })
            .collect();

        if active_records.is_empty() {
            return Ok(None);
        }

        // 取最早的那张检查点里的目标文件快照
        let earliest = active_records[0];
        let target_file = match earliest
            .files
            .iter()
            .find(|f| Path::new(&f.absolute).to_string_lossy().to_lowercase() == abs_str)
        {
            Some(f) => f,
            None => return Ok(None),
        };

        let (restored, deleted, skipped) = self.apply_restore(&[target_file.clone()]);
        let invalidated: Vec<String> = active_records.iter().map(|r| r.id.clone()).collect();

        let now = now_ms();
        let revert = RevertRecord {
            id: format!("rvrt_{}_{}", now, &uuid::Uuid::new_v4().to_string()[..6]),
            at: now,
            checkpoint_ids: invalidated.clone(),
            scope: "file".to_string(),
            detail: Some(target_file.path.clone()),
        };

        self.append_entry(thread_id, CheckpointEntry::Revert(revert))?;

        Ok(Some(RevertOutcome {
            restored,
            deleted,
            skipped,
            invalidated,
        }))
    }

    /// 一键回滚所有改动
    pub fn revert_all(&self, thread_id: &str) -> Result<Option<RevertOutcome>> {
        let entries = match self.load(thread_id, false)? {
            Some(e) => e,
            None => return Ok(None),
        };

        let active_records: Vec<&CheckpointRecord> = entries
            .iter()
            .filter_map(|e| match e {
                CheckpointEntry::Checkpoint(r) if !self.is_invalidated(&entries, &r.id) => Some(r),
                _ => None,
            })
            .collect();

        if active_records.is_empty() {
            return Ok(None);
        }

        let mut earliest_map: HashMap<String, CheckpointFile> = HashMap::new();
        let mut invalidated = Vec::new();

        for r in &active_records {
            invalidated.push(r.id.clone());
            for f in &r.files {
                let key = Path::new(&f.absolute).to_string_lossy().to_lowercase();
                if !earliest_map.contains_key(&key) {
                    earliest_map.insert(key, f.clone());
                }
            }
        }

        let files: Vec<CheckpointFile> = earliest_map.into_values().collect();
        let (restored, deleted, skipped) = self.apply_restore(&files);

        let now = now_ms();
        let revert = RevertRecord {
            id: format!("rvrt_{}_{}", now, &uuid::Uuid::new_v4().to_string()[..6]),
            at: now,
            checkpoint_ids: invalidated.clone(),
            scope: "all".to_string(),
            detail: None,
        };


        self.append_entry(thread_id, CheckpointEntry::Revert(revert))?;

        Ok(Some(RevertOutcome {
            restored,
            deleted,
            skipped,
            invalidated,
        }))
    }

    /// 删除会话时清空该会话的检查点文件
    pub fn discard(&self, thread_id: &str) -> Result<()> {
        self.clear_memory_cache(Some(thread_id));
        let path = self.file_for(thread_id);
        if path.exists() {
            let _ = fs::remove_file(path);
        }
        Ok(())
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_lifecycle() -> Result<()> {
        let temp_dir = std::env::temp_dir().join(format!("a_da_ckpt_test_{}", uuid::Uuid::new_v4()));
        let manager = CheckpointManager::new(Some(temp_dir.clone()));

        let tid = "thread_ckpt_001";
        let test_file = temp_dir.join("code.txt");

        // 写入初始文件
        fs::create_dir_all(&temp_dir)?;
        fs::write(&test_file, "version 1")?;

        // 拍摄快照
        let record = manager.capture(tid, "call_001", &[("code.txt", &test_file)])?;
        assert_eq!(record.files.len(), 1);
        assert_eq!(record.files[0].existed, true);

        // 修改文件
        fs::write(&test_file, "version 2 modified")?;
        assert_eq!(fs::read_to_string(&test_file)?, "version 2 modified");

        // 撤销该检查点
        let outcome = manager.revert_checkpoint(tid, &record.id)?.expect("应成功回滚");
        assert_eq!(outcome.restored, vec!["code.txt"]);
        assert_eq!(fs::read_to_string(&test_file)?, "version 1");

        // 再次撤销应返回 None（已被作废）
        let second_try = manager.revert_checkpoint(tid, &record.id)?;
        assert!(second_try.is_none());

        // 清理
        let _ = fs::remove_dir_all(temp_dir);
        Ok(())
    }
}
