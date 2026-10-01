use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::RwLock;

use anyhow::Result;
use tracing::info;

use super::sandbox::PluginSandbox;
use super::types::{LoadedPlugin, PluginManifest, PluginScope};

use crate::session::get_app_home;
use crate::tools::ToolResult;

pub struct PluginManager {
    plugins: RwLock<HashMap<String, LoadedPlugin>>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
        }
    }

    /// 扫描并加载工作区与全局插件
    pub async fn scan_and_load(&self, workspace: &Path) -> Result<()> {
        let mut loaded = HashMap::new();

        // 1. 扫描工作区插件 (.a-da/plugins/)
        let ws_plugins_dir = workspace.join(".a-da").join("plugins");
        if ws_plugins_dir.exists() {
            if let Ok(entries) = fs::read_dir(&ws_plugins_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if is_plugin_file(&path) {
                        let id = format!("workspace:{}", path.file_stem().unwrap().to_string_lossy());
                        if let Ok(tools) = PluginSandbox::inspect_plugin(&path, workspace).await {
                            loaded.insert(id.clone(), LoadedPlugin {
                                manifest: PluginManifest {
                                    id: id.clone(),
                                    name: id.clone(),
                                    description: "工作区本地插件".to_string(),
                                    version: Some("1.0.0".to_string()),
                                    author: None,
                                    scope: Some(PluginScope::Workspace),
                                },
                                tools,
                                enabled: true,
                                entry_path: Some(path.to_string_lossy().to_string()),
                                config: None,
                            });
                        }
                    }
                }
            }
        }

        // 2. 扫描全局插件 (~/.a-da/plugins/)
        let global_plugins_dir = get_app_home().join("plugins");
        if global_plugins_dir.exists() {
            if let Ok(entries) = fs::read_dir(&global_plugins_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if is_plugin_file(&path) {
                        let id = format!("global:{}", path.file_stem().unwrap().to_string_lossy());
                        if let Ok(tools) = PluginSandbox::inspect_plugin(&path, workspace).await {
                            loaded.insert(id.clone(), LoadedPlugin {
                                manifest: PluginManifest {
                                    id: id.clone(),
                                    name: id.clone(),
                                    description: "全局安装插件".to_string(),
                                    version: Some("1.0.0".to_string()),
                                    author: None,
                                    scope: Some(PluginScope::Global),
                                },
                                tools,
                                enabled: true,
                                entry_path: Some(path.to_string_lossy().to_string()),
                                config: None,
                            });
                        }
                    }
                }
            }
        }

        let count = loaded.len();
        let mut store = self.plugins.write().unwrap();
        *store = loaded;
        info!("插件管理器已就绪，成功加载 {} 个插件", count);

        Ok(())
    }

    /// 列出所有已加载插件
    pub fn list_plugins(&self) -> Vec<LoadedPlugin> {
        let store = self.plugins.read().unwrap();
        store.values().cloned().collect()
    }

    /// 执行插件工具
    pub async fn execute_plugin_tool(
        &self,
        tool_name: &str,
        args: serde_json::Value,
        workspace: &Path,
    ) -> Option<ToolResult> {
        let (entry_path, found) = {
            let store = self.plugins.read().unwrap();
            let mut res = None;
            for p in store.values() {
                if !p.enabled {
                    continue;
                }
                if p.tools.iter().any(|t| t.name == tool_name) {
                    if let Some(entry) = &p.entry_path {
                        res = Some((entry.clone(), true));
                        break;
                    }
                }
            }
            res
        }?;

        if found {
            PluginSandbox::call_tool(Path::new(&entry_path), tool_name, args, workspace, 60)
                .await
                .ok()
        } else {
            None
        }
    }
}

fn is_plugin_file(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        ext == "ts" || ext == "js" || ext == "mjs"
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_plugin_file() {
        assert!(is_plugin_file(Path::new("my-plugin.ts")));
        assert!(is_plugin_file(Path::new("my-plugin.js")));
        assert!(!is_plugin_file(Path::new("README.md")));
    }
}
