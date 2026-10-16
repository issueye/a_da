//! 插件自检（P1 · SP-18）：**只做只读检查，不执行工具**。
//!
//! # 为什么要它
//!
//! 插件作者今天只能"装上去看看"：写错了名字、转译过不去、工具名没登记，
//! 表现都是"它没生效"——而**为什么**没生效，界面上一句话都没有。
//! 这个自检把四件事一次性说清楚：**转译过不过 / 装载起不起得来 / 每个工具名在不在注册表 /
//! 当前权限与预算是什么**。
//!
//! # 与装配共用同一批判据
//!
//! 这里用的都是装配期那几步（`transpile` → `ScriptHost::load` → 查注册表），
//! 不另写一套"看起来像"的检查——两套判据迟早会说不一样的话。

use std::path::Path;
use std::sync::Arc;

use agent_base::ports::{LoadRequest, ScriptBudget, ScriptCtx, ScriptHost, ScriptSource};
use agent_base::testing::NeverCancel;
use serde::Serialize;

/// 单个导出的检查结果。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportCheck {
    pub name: String,
    /// 是否在**内置注册表**里（决策 1 的口径：只认注册表才装得进 catalog）
    pub registered: bool,
}

/// 自检报告（直接作为 RPC 返回值）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ValidateReport {
    pub plugin_id: String,
    pub path: String,
    /// 转译（oxc 类型擦除）过不过
    pub transpiled: bool,
    /// 装载（真的执行一遍插件装载流程）起不起得来
    pub loaded: bool,
    /// 插件声明的工具与它们在注册表里的状态
    pub exports: Vec<ExportCheck>,
    /// 当前生效的权限清单（人话）
    pub needs: String,
    /// 当前生效的预算（人话）
    pub limits: String,
    /// 人话的问题清单：**空数组 = 可以放心启用**
    pub problems: Vec<String>,
}

impl ValidateReport {
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// 自检一个插件（只读；`host` 由调用方给，通常现造一个就够）。
pub async fn validate_plugin(
    host: &Arc<dyn ScriptHost>,
    plugin_id: &str,
    path: &Path,
    workspace: &Path,
    budget: ScriptBudget,
) -> ValidateReport {
    let mut problems = Vec::new();

    // 1) 转译：与装配期同一步骤（读不到 / 语法错都在这里暴露）
    let transpiled = match tokio::fs::read_to_string(path).await {
        Ok(source) => match ada_ts_engine::compiler::transpile_ts_module_cached(&source, path.to_str())
        {
            Ok(_) => true,
            Err(e) => {
                problems.push(format!("转译失败：{e}"));
                false
            }
        },
        Err(e) => {
            problems.push(format!("读不到插件文件（{}）：{e}", path.display()));
            false
        }
    };

    // 2) 装载 + 导出清单（装载会真的执行一遍插件装载流程，所以这一步也是"能不能装起来"的判据）
    let mut exports = Vec::new();
    let mut loaded = false;
    if transpiled {
        let ctx = ScriptCtx {
            budget,
            cancel: &NeverCancel,
            thread_id: "",
            origin_id: Some(plugin_id),
        };
        match host
            .load(
                &LoadRequest {
                    source: ScriptSource::Path(path.to_path_buf()),
                    workspace: Some(workspace.to_path_buf()),
                },
                &ctx,
            )
            .await
        {
            Ok(handle) => {
                loaded = true;
                for name in host.exports(&handle).unwrap_or_default() {
                    let registered = agent_toolkit::find_tool_descriptor(&name).is_some();
                    if !registered {
                        problems.push(format!(
                            "工具 `{name}` 不在内置注册表里——当前口径（决策 1）只认注册表，\
                             它不会被装进工具目录"
                        ));
                    }
                    exports.push(ExportCheck { name, registered });
                }
                if exports.is_empty() {
                    problems.push("插件没有导出任何工具（检查 export default 的形状）".to_string());
                }
                host.dispose(handle).await;
            }
            Err(e) => {
                problems.push(format!("装载失败：{e:?}"));
            }
        }
    }

    // 3) 权限与预算：把**这个插件按自己声明拿到的**档位原样说出来
    //    （P4·SP-71：声明面是源码首部的 `@ada-needs`，作者据此判断"我能不能联网"）
    let source = std::fs::read_to_string(path).unwrap_or_default();
    let declared = crate::scripts::needs::parse_needs(&source);
    let limits = ada_ts_engine::Limits::default();
    let mut needs_text = format!(
        "fs: {} / net: {}",
        if declared.needs.fs_read || declared.needs.fs_write { "开" } else { "关" },
        if declared.needs.net {
            "开".to_string()
        } else {
            "关（未声明即不注入）".to_string()
        }
    );
    if !declared.declared {
        needs_text.push_str("（源码里没有 @ada-needs 指令，按宿主地板）");
    }
    if !declared.unknown.is_empty() {
        // 认不出的声明**不授予**，但一定要说出来——否则作者会以为写上了就生效
        needs_text.push_str(&format!(
            "；认不出的声明（未授予）：{}",
            declared.unknown.join("、")
        ));
    }
    ValidateReport {
        plugin_id: plugin_id.to_string(),
        path: path.to_string_lossy().to_string(),
        transpiled,
        loaded,
        exports,
        needs: needs_text,
        limits: format!(
            "墙钟 {}s / 循环 {} 次 / 递归 {} 层",
            budget.timeout_ms / 1000,
            limits.loop_iterations,
            limits.recursion_depth
        ),
        problems,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripts::TsScriptHost;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("a_da_val_{tag}_{}_{}", std::process::id(), nanos));
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        dir
    }

    /// 好插件：**可以放心启用**（问题清单为空），且每个导出都标了"在注册表里"。
    #[tokio::test]
    async fn test_valid_plugin_reports_no_problems() {
        let dir = temp_dir("ok");
        let path = dir.join("ok.ts");
        std::fs::write(
            &path,
            "export default { tools: [ { name: 'read_file', description: 'x', \
             execute: async () => ({ ok: true, output: 'ok' }) } ] }\n",
        )
        .expect("写插件失败");

        let host: Arc<dyn ScriptHost> = Arc::new(TsScriptHost::new());
        let report = validate_plugin(&host, "workspace:ok", &path, &dir, ScriptBudget::default()).await;
        assert!(report.transpiled && report.loaded, "{report:?}");
        assert!(report.is_ok(), "好插件不该有问题清单：{:?}", report.problems);
        assert_eq!(report.exports.len(), 1);
        assert!(report.exports[0].registered);
        // 权限与预算要**如实说出来**（插件作者据此判断能不能联网）
        assert!(report.needs.contains("net: 关"), "{}", report.needs);
        assert!(report.limits.contains("循环"), "{}", report.limits);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 坏插件：转译/未注册工具名/空导出各自给出**人话**的问题，而不是只说"没生效"。
    #[tokio::test]
    async fn test_problems_are_actionable() {
        let host: Arc<dyn ScriptHost> = Arc::new(TsScriptHost::new());

        // (a) 语法错 → 转译失败
        let dir = temp_dir("bad");
        let bad = dir.join("bad.ts");
        std::fs::write(&bad, "export default { tools: [ \n").expect("写失败");
        let rep = validate_plugin(&host, "p", &bad, &dir, ScriptBudget::default()).await;
        assert!(!rep.transpiled);
        assert!(rep.problems.iter().any(|p| p.contains("转译失败")), "{:?}", rep.problems);

        // (b) 未注册的工具名 → 明确说"不会被装进工具目录"
        let unreg = dir.join("unreg.ts");
        std::fs::write(
            &unreg,
            "export default { tools: [ { name: 'my_custom_tool', description: 'x', \
             execute: async () => ({ ok: true, output: 'ok' }) } ] }\n",
        )
        .expect("写失败");
        let rep2 = validate_plugin(&host, "p", &unreg, &dir, ScriptBudget::default()).await;
        assert!(!rep2.is_ok());
        assert!(
            rep2.problems.iter().any(|p| p.contains("不在内置注册表里")),
            "{:?}",
            rep2.problems
        );
        assert_eq!(rep2.exports.len(), 1);
        assert!(!rep2.exports[0].registered);

        // (c) 读不到文件
        let rep3 = validate_plugin(&host, "p", &dir.join("nope.ts"), &dir, ScriptBudget::default()).await;
        assert!(!rep3.transpiled);
        assert!(rep3.problems.iter().any(|p| p.contains("读不到插件文件")), "{:?}", rep3.problems);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
