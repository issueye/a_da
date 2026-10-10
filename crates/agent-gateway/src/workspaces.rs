//! **网关工作区管理服务**：持久化维护常用与最近访问的工作区清单。
//!
//! 供前端通过 `gateway.workspaces.*` RPC 快速查询历史工作区、切换工作区与移除工作区。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::UNIX_EPOCH;

const MAX_WORKSPACES: usize = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEntry {
    pub workspace: String,
    pub name: String,
    pub last_accessed_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_product: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacesData {
    pub workspaces: Vec<WorkspaceEntry>,
    pub active_workspace: Option<String>,
}

/// 默认获取网关工作区存储文件路径 (~/.a-da/gateway_workspaces.json)
pub fn default_storage_path() -> PathBuf {
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        PathBuf::from(home).join(".a-da").join("gateway_workspaces.json")
    } else {
        PathBuf::from("gateway_workspaces.json")
    }
}

pub struct WorkspacesStore {
    storage_path: PathBuf,
    data: RwLock<WorkspacesData>,
}

impl WorkspacesStore {
    pub fn new(storage_path: Option<PathBuf>) -> Self {
        let p = storage_path.unwrap_or_else(default_storage_path);
        let initial_data = Self::load_from_disk(&p).unwrap_or_default();
        Self {
            storage_path: p,
            data: RwLock::new(initial_data),
        }
    }

    fn load_from_disk(path: &Path) -> Option<WorkspacesData> {
        let content = fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    fn persist(&self) {
        if let Some(parent) = self.storage_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(guard) = self.data.read() {
            if let Ok(json_str) = serde_json::to_string_pretty(&*guard) {
                let _ = fs::write(&self.storage_path, json_str);
            }
        }
    }

    /// 获取工作区列表与当前活跃工作区
    pub fn get_data(&self) -> WorkspacesData {
        self.data.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// 记录/刷新一个访问过的工作区
    pub fn record(&self, workspace: &str, product: Option<&str>) {
        let norm_ws = workspace.trim().replace('\\', "/");
        if norm_ws.is_empty() {
            return;
        }

        let name = Path::new(&norm_ws)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| norm_ws.clone());

        let now = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let mut changed = false;
        if let Ok(mut guard) = self.data.write() {
            guard.active_workspace = Some(norm_ws.clone());

            // 移除已有的相同工作区
            guard.workspaces.retain(|w| w.workspace.to_lowercase() != norm_ws.to_lowercase());

            // 头部插入
            guard.workspaces.insert(
                0,
                WorkspaceEntry {
                    workspace: norm_ws,
                    name,
                    last_accessed_at: now,
                    active_product: product.map(str::to_string),
                },
            );

            // 截断超限
            if guard.workspaces.len() > MAX_WORKSPACES {
                guard.workspaces.truncate(MAX_WORKSPACES);
            }
            changed = true;
        }

        if changed {
            self.persist();
        }
    }

    /// 移除指定工作区
    pub fn remove(&self, workspace: &str) -> bool {
        let norm_ws = workspace.trim().replace('\\', "/").to_lowercase();
        let mut removed = false;

        if let Ok(mut guard) = self.data.write() {
            let before = guard.workspaces.len();
            guard.workspaces.retain(|w| w.workspace.to_lowercase() != norm_ws);
            removed = guard.workspaces.len() < before;

            if guard.active_workspace.as_deref().map(|s| s.to_lowercase()) == Some(norm_ws.clone()) {
                guard.active_workspace = guard.workspaces.first().map(|w| w.workspace.clone());
            }
        }

        if removed {
            self.persist();
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspaces_record_and_remove() {
        let temp_dir = std::env::temp_dir();
        let storage_path = temp_dir.join(format!("test_ws_{}.json", uuid::Uuid::new_v4().simple()));

        let store = WorkspacesStore::new(Some(storage_path.clone()));
        store.record("E:/test/workspace_a", Some("ada-coding"));
        store.record("E:/test/workspace_b", Some("ada-pm"));

        let data = store.get_data();
        assert_eq!(data.workspaces.len(), 2);
        assert_eq!(data.active_workspace.as_deref(), Some("E:/test/workspace_b"));

        let removed = store.remove("E:/test/workspace_b");
        assert!(removed);

        let data_after = store.get_data();
        assert_eq!(data_after.workspaces.len(), 1);
        assert_eq!(data_after.workspaces[0].workspace, "E:/test/workspace_a");

        let _ = fs::remove_file(storage_path);
    }
}
