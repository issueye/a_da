use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};


use super::slug::{safe_id, workspace_slug};
use super::types::{
    AgentMessage, CURRENT_SESSION_VERSION, SessionCompactEntry, SessionEntry, SessionHeader,
    SessionMessageEntry, SessionSummary,
};

/// 获取默认的 a-da 配置和数据主目录
pub fn get_app_home() -> PathBuf {
    if let Ok(dir) = std::env::var("A_DA_HOME") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir.trim());
        }
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        return PathBuf::from(userprofile).join(".a-da");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".a-da");
    }
    PathBuf::from(".a-da")
}

/// 会话管理器
#[derive(Debug, Clone)]
pub struct SessionManager {
    explicit_dir: Option<PathBuf>,
}

impl SessionManager {
    pub fn new(explicit_dir: Option<PathBuf>) -> Self {
        Self { explicit_dir }
    }

    /// 获取会话持久化根目录（`~/.a-da/sessions` 或指定目录）
    pub fn sessions_dir(&self) -> PathBuf {
        match &self.explicit_dir {
            Some(dir) => dir.clone(),
            None => get_app_home().join("sessions"),
        }
    }

    /// 确保会话持久化根目录存在
    fn ensure_sessions_dir(&self) -> Result<PathBuf> {
        let dir = self.sessions_dir();
        if !dir.exists() {
            fs::create_dir_all(&dir)
                .with_context(|| format!("创建会话目录失败: {}", dir.display()))?;
        }
        Ok(dir)
    }

    /// 获取某个工作区的专属散列目录
    pub fn workspace_dir(&self, workspace: &str) -> PathBuf {
        let slug = workspace_slug(workspace);
        self.sessions_dir().join(slug)
    }

    /// 工作区目录下的边车文件路径（`workspace.json`）
    fn workspace_pointer(&self, workspace: &str) -> PathBuf {
        self.workspace_dir(workspace).join("workspace.json")
    }

    /// 在工作区目录下记录边车文件 `workspace.json`，方便反向定位真实工作区路径
    pub fn remember_workspace(&self, workspace: &str) -> Result<()> {
        let dir = self.workspace_dir(workspace);
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }
        let pointer_path = self.workspace_pointer(workspace);
        let content = serde_json::json!({
            "workspace": workspace
        });
        let mut file = File::create(&pointer_path)?;
        serde_json::to_writer_pretty(&mut file, &content)?;
        file.write_all(b"\n")?;
        Ok(())
    }

    /// 获取指定会话的完整文件路径
    pub fn get_session_path(&self, workspace: &str, session_id: &str) -> PathBuf {
        let safe = safe_id(session_id);
        self.workspace_dir(workspace).join(format!("{}.jsonl", safe))
    }

    /// 创建新的持久化会话文件（幂等）
    pub fn create_session(
        &self,
        id: &str,
        workspace: &str,
        title: Option<&str>,
        parent_id: Option<&str>,
        subagent_id: Option<&str>,
    ) -> Result<SessionHeader> {
        self.ensure_sessions_dir()?;
        let _ = self.remember_workspace(workspace);

        let file_path = self.get_session_path(workspace, id);
        if file_path.exists() {
            if let Ok(file) = File::open(&file_path) {
                let reader = BufReader::new(file);
                if let Some(Ok(first_line)) = reader.lines().next() {
                    if let Ok(header) = serde_json::from_str::<SessionHeader>(&first_line) {
                        if header.entry_type == "session" {
                            return Ok(header);
                        }
                    }
                }
            }
        }

        let now = now_ms();
        let header = SessionHeader {
            entry_type: "session".to_string(),
            version: CURRENT_SESSION_VERSION,
            id: id.to_string(),
            title: Some(title.unwrap_or("新会话").to_string()),
            workspace: workspace.to_string(),
            created_at: now,
            updated_at: now,
            parent_id: parent_id.map(|s| s.to_string()),
            subagent_id: subagent_id.map(|s| s.to_string()),
            plugin_data: None,
        };

        let json_line = serde_json::to_string(&header)?;
        let mut file = File::create(&file_path)?;
        writeln!(file, "{}", json_line)?;

        Ok(header)
    }

    /// 寻找某个会话当前所在的实际文件路径（未指明工作区时跨目录扫描）
    pub fn find_session_path(&self, session_id: &str) -> Option<PathBuf> {
        let safe = safe_id(session_id);
        let target_filename = format!("{}.jsonl", safe);
        let root = self.sessions_dir();
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        let candidate = entry.path().join(&target_filename);
                        if candidate.exists() {
                            return Some(candidate);
                        }
                    }
                }
            }
        }
        None
    }

    /// 向会话以 Append-only 方式追加一条消息
    pub fn append_message(
        &self,
        session_id: &str,
        message: AgentMessage,
        workspace: Option<&str>,
    ) -> Result<()> {
        let file_path = if let Some(ws) = workspace {
            let dir = self.workspace_dir(ws);
            if !dir.exists() {
                fs::create_dir_all(&dir)?;
            }
            self.get_session_path(ws, session_id)
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(());
        };

        let now = now_ms();
        let random_suffix = uuid::Uuid::new_v4().to_string();
        let entry = SessionMessageEntry {
            entry_type: "message".to_string(),
            id: format!("msg_{}_{}", now, &random_suffix[..6]),
            timestamp: now,
            message,
        };

        let line = serde_json::to_string(&entry)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)?;
        writeln!(file, "{}", line)?;
        Ok(())
    }

    /// 向会话追加一条上下文压缩摘要记录
    pub fn append_compact_entry(
        &self,
        session_id: &str,
        mut compact_entry: SessionCompactEntry,
        workspace: Option<&str>,
    ) -> Result<()> {
        let file_path = if let Some(ws) = workspace {
            let dir = self.workspace_dir(ws);
            if !dir.exists() {
                fs::create_dir_all(&dir)?;
            }
            self.get_session_path(ws, session_id)
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(());
        };

        compact_entry.entry_type = "compact".to_string();
        let line = serde_json::to_string(&compact_entry)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)?;
        writeln!(file, "{}", line)?;
        Ok(())
    }

    /// 更新会话标题
    pub fn update_session_title(
        &self,
        session_id: &str,
        title: &str,
        workspace: Option<&str>,
    ) -> Result<()> {
        let file_path = if let Some(ws) = workspace {
            let path = self.get_session_path(ws, session_id);
            if !path.exists() {
                self.create_session(session_id, ws, Some(title), None, None)?;
            }
            path
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(());
        };

        self.update_header(&file_path, |header| {
            header.title = Some(title.to_string());
            header.updated_at = now_ms();
        })
    }

    /// 更新会话的元数据（父子关系、插件数据）
    pub fn update_session_meta(
        &self,
        session_id: &str,
        parent_id: Option<Option<String>>,
        subagent_id: Option<Option<String>>,
        plugin_data: Option<HashMap<String, serde_json::Value>>,
        workspace: Option<&str>,
    ) -> Result<()> {
        let file_path = if let Some(ws) = workspace {
            let path = self.get_session_path(ws, session_id);
            if !path.exists() {
                self.create_session(session_id, ws, None, None, None)?;
            }
            path
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(());
        };

        self.update_header(&file_path, |header| {
            if let Some(pid) = parent_id {
                header.parent_id = pid;
            }
            if let Some(sid) = subagent_id {
                header.subagent_id = sid;
            }
            if let Some(data) = plugin_data {
                let current = header.plugin_data.get_or_insert_with(HashMap::new);
                for (k, v) in data {
                    current.insert(k, v);
                }
            }
            header.updated_at = now_ms();
        })
    }

    /// 辅助方法：原子更新第一行 Header 并重写保留后续内容
    fn update_header<F>(&self, file_path: &Path, update_fn: F) -> Result<()>
    where
        F: FnOnce(&mut SessionHeader),
    {
        if !file_path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(file_path)?;
        let lines: Vec<&str> = content.split('\n').collect();
        if lines.is_empty() || lines[0].trim().is_empty() {

            return Ok(());
        }

        let mut header: SessionHeader = serde_json::from_str(lines[0])?;
        if header.entry_type != "session" {
            return Ok(());
        }

        update_fn(&mut header);
        let new_first_line = serde_json::to_string(&header)?;

        let mut output = String::with_capacity(content.len() + 64);
        output.push_str(&new_first_line);
        output.push('\n');

        for line in &lines[1..] {
            if !line.trim().is_empty() {
                output.push_str(line);
                output.push('\n');
            }
        }

        fs::write(file_path, output)?;
        Ok(())
    }

    /// 读取并重构会话的历史消息
    pub fn load_session(
        &self,
        session_id: &str,
        workspace: Option<&str>,
    ) -> Result<Option<(SessionHeader, Vec<AgentMessage>)>> {
        let file_path = if let Some(ws) = workspace {
            self.get_session_path(ws, session_id)
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(None);
        };

        if !file_path.exists() {
            return Ok(None);
        }

        let file = File::open(&file_path)?;
        let reader = BufReader::new(file);

        let mut header: Option<SessionHeader> = None;
        let mut messages: Vec<AgentMessage> = Vec::new();

        for line_res in reader.lines() {
            let line = match line_res {
                Ok(l) => l,
                Err(_) => continue,
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Ok(entry) = serde_json::from_str::<SessionEntry>(trimmed) {
                match entry {
                    SessionEntry::Header(h) => {
                        header = Some(h);
                    }
                    SessionEntry::Message(m) => {
                        messages.push(m.message);
                    }
                    SessionEntry::Notice(_) => {
                        // Notice 不计入 messages 模型上下文
                    }
                    SessionEntry::Compact(c) => {
                        // 遇到 compact 记录，折叠前面的上下文，生成连续总结消息
                        let continuation_msg = AgentMessage::User {
                            content: format!("[系统自动压缩摘要]\n{}", c.summary),
                            images: None,
                            timestamp: Some(c.timestamp),
                        };
                        messages.clear();
                        messages.push(continuation_msg);
                    }
                }
            }
        }

        match header {
            Some(h) => Ok(Some((h, messages))),
            None => Ok(None),
        }
    }

    /// 读一个工作区目录的 `workspace.json`
    fn read_workspace_pointer(&self, dir: &Path) -> Option<String> {
        let pointer_path = dir.join("workspace.json");
        if !pointer_path.exists() {
            return None;
        }
        let content = fs::read_to_string(pointer_path).ok()?;
        let val: serde_json::Value = serde_json::from_str(&content).ok()?;
        val.get("workspace")
            .and_then(|w| w.as_str())
            .map(|s| s.to_string())
    }

    /// 列举指定工作区的所有历史会话摘要（按 updatedAt 倒序）
    pub fn list_sessions_for_workspace(&self, workspace: &str) -> Result<Vec<SessionSummary>> {
        let dir = self.workspace_dir(workspace);
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut summaries = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return Ok(Vec::new()),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }

            if let Ok(file) = File::open(&path) {
                let mut reader = BufReader::new(file);
                let mut first_line = String::new();
                if reader.read_line(&mut first_line).is_ok() && !first_line.trim().is_empty() {
                    if let Ok(header) = serde_json::from_str::<SessionHeader>(first_line.trim()) {
                        if header.entry_type == "session" {
                            let metadata = entry.metadata().ok();
                            let mtime = metadata
                                .and_then(|m| m.modified().ok())
                                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                                .map(|d| d.as_millis() as i64)
                                .unwrap_or(header.updated_at);

                            summaries.push(SessionSummary {
                                id: header.id,
                                title: header.title.unwrap_or_else(|| "新会话".to_string()),
                                workspace: header.workspace,
                                created_at: header.created_at,
                                updated_at: mtime,
                                file_path: path.to_string_lossy().to_string(),
                                parent_id: header.parent_id,
                                subagent_id: header.subagent_id,
                            });
                        }
                    }
                }
            }
        }

        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(summaries)
    }

    /// 列举所有工作区的全部会话摘要
    pub fn list_all_sessions(&self) -> Result<Vec<SessionSummary>> {
        let root = self.sessions_dir();
        if !root.exists() {
            return Ok(Vec::new());
        }

        let mut summaries = Vec::new();
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(workspace) = self.read_workspace_pointer(&path) {
                        if let Ok(list) = self.list_sessions_for_workspace(&workspace) {
                            summaries.extend(list);
                        }
                    }
                }
            }
        }

        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(summaries)
    }

    /// 删除指定会话并级联删除名下的子会话
    pub fn delete_session(&self, session_id: &str, workspace: Option<&str>) -> Result<()> {
        let file_path = if let Some(ws) = workspace {
            self.get_session_path(ws, session_id)
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(());
        };

        if file_path.exists() {
            let _ = fs::remove_file(&file_path);
        }

        // 级联删除名下的所有子智能体会话
        if let Some(parent_dir) = file_path.parent() {
            if let Some(ws) = self.read_workspace_pointer(parent_dir) {
                if let Ok(sub_list) = self.list_sessions_for_workspace(&ws) {
                    for item in sub_list {
                        if item.parent_id.as_deref() == Some(session_id) {
                            let _ = fs::remove_file(&item.file_path);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// 归档一份会话副本（`<id>.jsonl.archived`）
    pub fn archive_session(&self, session_id: &str, workspace: Option<&str>) -> Result<bool> {
        let file_path = if let Some(ws) = workspace {
            self.get_session_path(ws, session_id)
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(false);
        };

        if !file_path.exists() {
            return Ok(false);
        }

        let archived_path = PathBuf::from(format!("{}.archived", file_path.display()));
        fs::copy(&file_path, &archived_path)?;
        Ok(true)
    }

    /// 删除整个工作区目录
    pub fn delete_workspace(&self, workspace: &str) -> Result<()> {
        let dir = self.workspace_dir(workspace);
        if dir.exists() {
            fs::remove_dir_all(dir)?;
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
    fn test_session_lifecycle() -> Result<()> {
        let temp_dir = std::env::temp_dir().join(format!("a_da_test_{}", uuid::Uuid::new_v4()));
        let manager = SessionManager::new(Some(temp_dir.clone()));

        let ws = "E:/test_project";
        let sid = "session_001";

        // 1. 创建会话
        let header = manager.create_session(sid, ws, Some("测试初始会话"), None, None)?;
        assert_eq!(header.id, sid);
        assert_eq!(header.title, Some("测试初始会话".to_string()));

        // 2. 追加消息
        manager.append_message(
            sid,
            AgentMessage::User {
                content: "你好 Rust 核心".to_string(),
                images: None,
                timestamp: Some(1000),
            },
            Some(ws),
        )?;

        manager.append_message(
            sid,
            AgentMessage::Assistant {
                content: "你好，已成功收到消息。".to_string(),
                thinking: None,
                tool_calls: None,
                stop_reason: Some("stop".to_string()),
                error_message: None,
                timestamp: Some(1001),
                usage: None,
                duration_ms: None,
                turn_duration_ms: None,
            },
            Some(ws),
        )?;

        // 3. 读取会话
        let loaded = manager.load_session(sid, Some(ws))?.expect("会话应存在");
        assert_eq!(loaded.0.id, sid);
        assert_eq!(loaded.1.len(), 2);

        // 4. 改名
        manager.update_session_title(sid, "更新后的标题", Some(ws))?;
        let loaded2 = manager.load_session(sid, Some(ws))?.expect("会话应存在");
        assert_eq!(loaded2.0.title, Some("更新后的标题".to_string()));
        assert_eq!(loaded2.1.len(), 2);

        // 5. 列举工作区会话
        let list = manager.list_sessions_for_workspace(ws)?;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, sid);
        assert_eq!(list[0].title, "更新后的标题");

        // 6. 列举全部会话
        let all = manager.list_all_sessions()?;
        assert_eq!(all.len(), 1);

        // 7. 归档
        assert!(manager.archive_session(sid, Some(ws))?);

        // 8. 清理临时目录
        let _ = fs::remove_dir_all(temp_dir);
        Ok(())
    }
}
