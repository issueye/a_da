//! AGENT BASE 适配器层（`docs/agent-base-design.md` v0.2 §3 的 `agent-adapter`）。
//!
//! 这里放"与外部世界打交道"的实现：模型供应商、应用目录、系统时钟、会话存储、插件运行时、作用域。
//! 依赖方向永远是 **适配器 → agent-base**，不许反向。
//!
//! 当前落地进度：
//! - `model/`：三家协议的 SSE 解析 + 中止（原 `agent_core::ai::stream`）
//! - `app_home.rs` / `clock.rs`：`AppHome` 与 `Clock` 端口的真实实现（全仓唯一读环境变量/系统时间的地方）
//! - `scope/`：`Scope` 端口的真实实现 `WorkspaceScope`（W1-T1）
//! - `cancel.rs`：`CancelToken` 端口的真实实现 `CancelHandle` / `ChildCancel`（W1-T4）
//! - `prompt/`：`PromptSource` 端口的真实实现 `CodingPromptSource`（W1-T3）
//! - `store/`：会话与检查点落盘 + `SessionStore` 端口的真实实现 `FsSessionStore`（W1-T2）
//! - `approval/`：审批**策略**纯函数（W1-T6；执行侧在宿主）
//! - `plugin/`（ts_engine 沙箱）按计划 W2 逐步搬入

pub mod app_home;
pub mod approval;
pub mod cancel;
pub mod clock;
pub mod model;
pub mod prompt;
pub mod scope;
pub mod store;
