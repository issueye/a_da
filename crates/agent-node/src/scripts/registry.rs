//! 已装载脚本的**注册表**（P4 · SP-70 抽出）：两种宿主实现共用同一份。
//!
//! # 为什么要抽出来
//!
//! 到 SP-71 为止只有一种宿主实现（进程内 Boa）。子进程档（SP-70）是**第二个**
//! `ScriptHost` 实现，而"句柄 → 元信息"这张表、以及"句柄失效时如实报错"的口径
//! 与沙箱档**完全一样**。各写一份的话，两份迟早分叉——分叉的表现会是
//! "换个档位，某个错误码就变了"，那种问题查起来最费劲。
//!
//! 抽出来之后，各档只负责"怎么执行"，不负责"句柄怎么记"。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use agent_base::ports::{
    ModuleHandle, ScriptError, ScriptHookDeclaration, ScriptNeeds, ScriptPromptFragment,
    ScriptSkillDeclaration, ScriptToolDeclaration,
};

/// 已装载脚本的元信息（句柄的 `id` 指向它）。
#[derive(Clone, Debug)]
pub struct LoadedScript {
    pub path: PathBuf,
    pub workspace: PathBuf,
    /// 装载时探明的可用入口（工具名）。
    ///
    /// 为什么要**在装载期**探明：`invoke` 才能把"这个入口不存在"与"脚本自己抛错"分开
    /// （端口的 [`ScriptError::UnknownExport`] 需要真实产生方）。靠调用失败后去匹配
    /// 沙箱的中文文案是最脆弱的那种判法——文案一改就静默失效。
    pub exports: Vec<String>,
    pub tools: Vec<ScriptToolDeclaration>,
    /// 装载期一并探明的**提示词片段声明**（P2·SP-44；没有就是空）
    pub prompts: Vec<ScriptPromptFragment>,
    /// 装载期一并探明的**技能声明**（P2·SP-45；没有就是空）
    pub skills: Vec<ScriptSkillDeclaration>,
    /// 装载期一并探明的**钩子注册**（P3a·SP-50；没有就是空）
    pub hooks: Vec<ScriptHookDeclaration>,
    /// 这个插件的**能力**（P4·SP-71）：调用期用它建沙箱，而不是一个全局默认档。
    ///
    /// 与装载期同源（都来自源码首部的指令）：装载与调用必须同一套能力，
    /// 否则"装载期跑得过、调用期跑不过"（或反过来）会变成最难查的那类问题。
    pub needs: ScriptNeeds,
}

/// 句柄注册表。
#[derive(Default)]
pub struct ScriptRegistry {
    map: Mutex<HashMap<String, LoadedScript>>,
    next_id: AtomicU64,
}

impl ScriptRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一份元信息，返回它的句柄。
    pub fn insert(&self, script: LoadedScript) -> Result<ModuleHandle, ScriptError> {
        let id = format!("script-{}", self.next_id.fetch_add(1, Ordering::SeqCst) + 1);
        self.map
            .lock()
            .map_err(|_| ScriptError::Unavailable {
                message: "脚本注册表锁中毒".to_string(),
            })?
            .insert(id.clone(), script);
        Ok(ModuleHandle { id })
    }

    /// 取元信息；句柄无效（未装载 / 已释放）时**如实报错**，不返回默认值。
    pub fn get(&self, handle: &ModuleHandle) -> Result<LoadedScript, ScriptError> {
        self.map
            .lock()
            .map_err(|_| ScriptError::Unavailable {
                message: "脚本注册表锁中毒".to_string(),
            })?
            .get(&handle.id)
            .cloned()
            .ok_or_else(|| ScriptError::Unavailable {
                message: format!("脚本句柄 `{}` 未装载或已释放", handle.id),
            })
    }

    /// 释放（**幂等**：不在表里也算成功）。
    pub fn remove(&self, handle: &ModuleHandle) {
        if let Ok(mut map) = self.map.lock() {
            map.remove(&handle.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script() -> LoadedScript {
        LoadedScript {
            path: PathBuf::from("/p.ts"),
            workspace: PathBuf::from("/ws"),
            exports: vec!["read_file".to_string()],
            tools: vec![ScriptToolDeclaration {
                name: "read_file".to_string(),
                label: None,
                description: "read file".to_string(),
                parameters: serde_json::json!({"type":"object","properties":{}}),
                access: agent_base::ports::ScriptToolAccess::External,
                long_running: false,
            }],
            prompts: Vec::new(),
            skills: Vec::new(),
            hooks: Vec::new(),
            needs: ScriptNeeds::plugin_floor(),
        }
    }

    /// 句柄是**唯一的**：两次装载不能互相顶掉。
    #[test]
    fn test_handles_are_unique_and_resolvable() {
        let reg = ScriptRegistry::new();
        let a = reg.insert(script()).expect("登记");
        let b = reg.insert(script()).expect("登记");
        assert_ne!(a.id, b.id);
        assert_eq!(reg.get(&a).expect("a 可解析").exports, vec!["read_file".to_string()]);
        assert!(reg.get(&b).is_ok());
    }

    /// 释放是幂等的（调用方不必为"放两次"写判断），释放后句柄**如实失效**。
    #[test]
    fn test_dispose_is_idempotent_and_invalidates_the_handle() {
        let reg = ScriptRegistry::new();
        let h = reg.insert(script()).expect("登记");
        reg.remove(&h);
        reg.remove(&h); // 幂等
        let err = reg.get(&h).expect_err("释放后必须失效");
        assert!(
            matches!(err, ScriptError::Unavailable { .. }),
            "失效要报 Unavailable（调用方能据此区分'没装过'与'脚本自己炸'）：{err:?}"
        );
    }

    /// 没登记过的句柄同样如实报错（而不是返回一个空元信息）。
    #[test]
    fn test_unknown_handle_is_reported() {
        let reg = ScriptRegistry::new();
        let err = reg
            .get(&ModuleHandle {
                id: "script-999".to_string(),
            })
            .expect_err("未知句柄必须报错");
        assert!(format!("{err:?}").contains("script-999"), "{err:?}");
    }
}
