//! 按**权限档位**裁决路径边界的 `Scope`（P1b · SP-33）。
//!
//! # 两层档位里的第二层
//!
//! 第一层（问不问）在 `approval::policy`：`Full` 与 `Auto` 一样不询问。
//! 第二层（能碰什么）在这里：默认档位下就是工作区沙箱，`Full` 下放开 `..` 与绝对路径。
//!
//! # 保留目录：凭据关着，插件/技能开着
//!
//! `~/.a-da` 放的是应用自己的配置与 API 密钥（`config.json`、网关发现文件…）。
//! 把它整棵交给模型不属于"访问用户文件"——那是把凭据交出去。所以保留目录
//! **任何档位都不可越**，且这条不给开关：给一个"再确认一次就放行"的按钮，迟早会被点掉。
//!
//! 但保留目录里有**不是凭据、且模型本来就该读到**的东西：装好的插件
//! （`<home>/agent-coding/extensions/`）与技能（`<home>/agent-coding/skills/`）。
//! 模型读不到它们，就会出现"插件报错却看不到自己的源码 / 技能说不清用法"。
//! 于是加了**白名单子树**（[`ApprovalAwareScope::with_allowed_subtrees`]）：
//! 命中的路径在完全访问下放行，**其余仍按保留目录拒绝**——`config.json` 没有开。
//!
//! # 判定的两条硬要求
//!
//! 1. **先规范化再比前缀**：`../../home/.a-da/x`、经符号链接指向保留目录的路径，
//!    都必须在规范化之后才判——否则红线形同虚设；
//! 2. **`Scope::id()` 与默认档位逐字一致**：它用于落盘分组与事件归属
//!    （`WorkspaceScope` 的文档写明），换档位不能让同一个会话换一个 id。

use std::path::PathBuf;
use std::sync::Arc;

use agent_base::domain::DenialKind;
use agent_base::ports::Scope;
use agent_proto::ApprovalMode;

use super::workspace::WorkspaceScope;

/// 组合根提供"当前权限档位"的口子。
///
/// 为什么不让本层直接读 store：`AgentStore` 属于 UI 投影层（`agent-rpc`），
/// 适配器依赖它会破坏依赖方向（`agent-adapter → agent-base` 单向）。
/// 这里只要一个"问得出档位"的最小接口，实现由组合根给。
pub trait ApprovalModeSource: Send + Sync {
    fn approval_mode(&self) -> ApprovalMode;
}

/// 在保留目录外放开边界的 `Scope`（默认档位下与 [`WorkspaceScope`] 行为一致）。
pub struct ApprovalAwareScope {
    inner: WorkspaceScope,
    mode: Arc<dyn ApprovalModeSource>,
    /// 任何档位都不可越的保留目录（构造时即规范化，比较时才不会有差异写法漏网）。
    deny_prefixes: Vec<PathBuf>,
    /// 位于保留目录**之内**、完全访问下仍然放行的子树（插件 / 技能）。
    ///
    /// 判定顺序是"先看白名单、再看黑名单"：`extensions/x` 放行，
    /// 而指向 `config.json` 的符号链接**规范化之后**落在黑名单，照样拒。
    allow_prefixes: Vec<PathBuf>,
    /// 安全下线（`agent_toolkit::security::deferred()`）：视作完全访问。
    ///
    /// 默认 `false`（测试与恢复安全后的行为不变）；组合根按总开关显式打开。
    /// **保留目录黑名单仍然生效**——那是数据完整性，不是审批闸门。
    defer_security: bool,
}

impl ApprovalAwareScope {
    /// `deny_prefixes` 由组合根给（通常是应用目录 `~/.a-da`）。
    pub fn new(
        inner: WorkspaceScope,
        mode: Arc<dyn ApprovalModeSource>,
        deny_prefixes: Vec<PathBuf>,
    ) -> Self {
        Self {
            inner,
            mode,
            deny_prefixes: deny_prefixes
                .into_iter()
                .map(|p| agent_toolkit::resolve_real_path(&p))
                .collect(),
            allow_prefixes: Vec::new(),
            defer_security: false,
        }
    }

    /// 安全下线：把"能碰什么"直接当作完全访问（保留目录黑名单仍生效）。
    ///
    /// 只由组合根按 `agent_toolkit::security::deferred()` 调用；默认关闭，
    /// 于是单元测试与恢复安全后的行为逐字节不变。
    pub fn with_security_deferred(mut self, defer: bool) -> Self {
        self.defer_security = defer;
        self
    }

    /// 在保留目录里开几个**非凭据**的子树（插件安装目录、技能目录）。
    pub fn with_allowed_subtrees(mut self, allow: Vec<PathBuf>) -> Self {
        self.allow_prefixes = allow
            .into_iter()
            .map(|p| agent_toolkit::resolve_real_path(&p))
            .collect();
        self
    }

    /// 是否处于完全访问（拿不到档位时**按否处理**——失败方向朝严的一侧）。
    fn is_full_access(&self) -> bool {
        self.defer_security || matches!(self.mode.approval_mode(), ApprovalMode::Full)
    }
}

impl Scope for ApprovalAwareScope {
    fn id(&self) -> &str {
        // 换档位不该换身份：落盘分组与事件归属都按它
        self.inner.id()
    }

    fn resolve_path(&self, raw: &str) -> Result<PathBuf, DenialKind> {
        if !self.is_full_access() {
            // 非完全访问：**逐字节沿用**工作区沙箱（P1b 之前的行为）
            return self.inner.resolve_path(raw);
        }

        let candidate = {
            let p = std::path::Path::new(raw);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                // 相对路径仍相对工作区根（否则完全访问下 `src/lib.rs` 会找不到北）
                self.inner.root().join(p)
            }
        };
        let resolved = agent_toolkit::resolve_real_path(&candidate);

        // 🔴 白名单先于黑名单：插件/技能目录在完全访问下可读写。
        // 判定用的是**规范化之后**的路径，所以从白名单目录里指向 `config.json` 的
        // 符号链接会在下一步命中黑名单，不会被放进来。
        if self
            .allow_prefixes
            .iter()
            .any(|a| resolved.starts_with(a) && resolved != *a)
        {
            return Ok(resolved);
        }

        // 🔴 红线：**先规范化再比**——`a/../../.a-da`、符号链接指向保留目录都必须被拦
        if let Some(hit) = self.deny_prefixes.iter().find(|d| resolved.starts_with(d)) {
            tracing::warn!(
                "[scope] 完全访问下仍拒绝访问保留目录：{}（命中 {}）",
                resolved.display(),
                hit.display()
            );
            return Err(DenialKind::Sandbox {
                path: raw.to_string(),
            });
        }

        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedMode(ApprovalMode);
    impl ApprovalModeSource for FixedMode {
        fn approval_mode(&self) -> ApprovalMode {
            self.0
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("a_da_scope_{tag}_{}_{}", std::process::id(), nanos));
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        dir
    }

    fn scope_with(mode: ApprovalMode, ws: &std::path::Path, deny: Vec<PathBuf>) -> ApprovalAwareScope {
        ApprovalAwareScope::new(
            WorkspaceScope::new(ws.to_path_buf()),
            Arc::new(FixedMode(mode)),
            deny,
        )
    }

    /// 核心增量：**同一个越界路径**，默认档拒绝、完全访问放行。
    /// 两个方向都断言，才能证明"增量真的挂在档位上"，而不是"一直都能写"。
    #[test]
    fn test_full_access_opens_the_workspace_boundary_but_default_does_not() {
        let ws = temp_dir("ws");
        let outside = temp_dir("outside");
        let escape = format!("{}/../{}/file.txt", ws.display(), outside.file_name().unwrap().to_string_lossy());

        let strict = scope_with(ApprovalMode::Auto, &ws, vec![]);
        assert!(
            matches!(strict.resolve_path(&escape), Err(DenialKind::Sandbox { .. })),
            "`auto` 档必须在工作区边界内（它不是完全访问）"
        );

        let full = scope_with(ApprovalMode::Full, &ws, vec![]);
        assert!(
            full.resolve_path(&escape).is_ok(),
            "完全访问档必须放开工作区边界"
        );

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// 红线：**任何档位**都不可越应用自己的目录（那是凭据）。
    #[test]
    fn test_reserved_prefix_is_denied_even_under_full_access() {
        let ws = temp_dir("ws");
        let app_home = temp_dir("app_home");
        let target = app_home.join("config.json");
        std::fs::write(&target, "{}").expect("写临时文件失败");

        for mode in [ApprovalMode::Auto, ApprovalMode::Full, ApprovalMode::Readonly] {
            let scope = scope_with(mode, &ws, vec![app_home.clone()]);
            let direct = scope.resolve_path(&target.to_string_lossy());
            assert!(
                matches!(direct, Err(DenialKind::Sandbox { .. })),
                "{mode:?} 档下访问保留目录必须被拒"
            );
            // `..` 造出来的同一条路径也必须被拦（红线判定在规范化之后）
            let sneaky = scope.resolve_path(&format!(
                "{}/../{}/config.json",
                app_home.display(),
                app_home.file_name().unwrap().to_string_lossy()
            ));
            assert!(
                matches!(sneaky, Err(DenialKind::Sandbox { .. })),
                "{mode:?} 档下用 `..` 拼出的保留目录路径必须被拦"
            );
        }

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&app_home);
    }

    /// 换档位**不许换身份**：`id()` 用于落盘分组与事件归属。
    #[test]
    fn test_scope_id_does_not_change_with_the_mode() {
        let ws = temp_dir("ws");
        let a = scope_with(ApprovalMode::Auto, &ws, vec![]);
        let b = scope_with(ApprovalMode::Full, &ws, vec![]);
        assert_eq!(a.id(), b.id(), "同一个工作区在不同档位下必须归到同一个 id");
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 完全访问下**相对路径仍相对工作区根**，且工作区内路径照常放行。
    #[test]
    fn test_full_access_keeps_relative_paths_working() {
        let ws = temp_dir("ws");
        std::fs::create_dir_all(ws.join("src")).expect("建子目录失败");
        let scope = scope_with(ApprovalMode::Full, &ws, vec![]);
        let ok = scope.resolve_path("src/lib.rs").expect("工作区内相对路径必须放行");
        assert!(ok.starts_with(agent_toolkit::resolve_real_path(&ws)));
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// P3b · SP-62：插件**改写出的越界路径**在默认档被拦、在完全访问档放行。
    ///
    /// 这一条把"改写后的重校验"与"权限档位"缝在一起：引擎用的是**外层同一个** `Scope`
    /// （`agent_base::engine::check_paths`），所以档位语义自动一致——完全访问下越界本来就
    /// 允许（那正是它的语义），默认档必须拦下。用例放这里而不是引擎侧：档位实现在本 crate，
    /// 而引擎不认识档位（AGENTS §14 的口径：策略归端口）。
    #[test]
    fn test_replacement_path_check_follows_the_approval_mode() {
        use agent_base::domain::{Access, PathSelector};
        use agent_base::engine::check_paths;

        let ws = temp_dir("ws");
        let outside = temp_dir("outside");
        let escape = format!(
            "{}/../{}/file.txt",
            ws.display(),
            outside.file_name().unwrap().to_string_lossy()
        );
        let access = Access::Mutates {
            paths: PathSelector::Single("path"),
        };
        let args = serde_json::json!({ "path": escape });

        let strict = scope_with(ApprovalMode::Auto, &ws, vec![]);
        let denied = check_paths(&strict, &access, &args);
        assert!(
            denied.is_err(),
            "默认档下改写出的越界路径必须被拦下：{denied:?}"
        );

        let full = scope_with(ApprovalMode::Full, &ws, vec![]);
        assert!(
            check_paths(&full, &access, &args).is_ok(),
            "完全访问档下越界本来就被允许（逐次留痕由引擎的 `hook.replace` 诊断负责）"
        );

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// 保留目录里的**插件/技能子树**在完全访问下放行，凭据文件仍然拒。
    ///
    /// 这条锁的是"收窄红线"的边界：只开 `extensions/` 与 `skills/`，
    /// 同一棵 `~/.a-da` 里的 `config.json` 不许因此被带出去。
    #[test]
    fn test_allowed_subtree_opens_plugins_but_keeps_credentials_closed() {
        let ws = temp_dir("ws");
        let app_home = temp_dir("app_home");
        let ext_dir = app_home.join("agent-coding").join("extensions").join("demo");
        std::fs::create_dir_all(&ext_dir).expect("建插件目录失败");
        let plugin_file = ext_dir.join("index.ts");
        std::fs::write(&plugin_file, "export default {}").expect("写插件文件失败");
        let secret = app_home.join("config.json");
        std::fs::write(&secret, r#"{"apiKey":"sk-should-never-leak"}"#).expect("写配置失败");

        let full = ApprovalAwareScope::new(
            WorkspaceScope::new(ws.clone()),
            Arc::new(FixedMode(ApprovalMode::Full)),
            vec![app_home.clone()],
        )
        .with_allowed_subtrees(vec![app_home.join("agent-coding").join("extensions")]);

        assert!(
            full.resolve_path(&plugin_file.to_string_lossy()).is_ok(),
            "完全访问下插件源码必须可读"
        );
        assert!(
            matches!(
                full.resolve_path(&secret.to_string_lossy()),
                Err(DenialKind::Sandbox { .. })
            ),
            "白名单不能顺带把 config.json 打开"
        );

        // 非完全访问：白名单**不生效**，仍按工作区沙箱（插件目录在工作区外）
        let ask = ApprovalAwareScope::new(
            WorkspaceScope::new(ws.clone()),
            Arc::new(FixedMode(ApprovalMode::Ask)),
            vec![app_home.clone()],
        )
        .with_allowed_subtrees(vec![app_home.join("agent-coding").join("extensions")]);
        assert!(
            ask.resolve_path(&plugin_file.to_string_lossy()).is_err(),
            "非完全访问档不许因为白名单就放开工作区"
        );

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&app_home);
    }
}
