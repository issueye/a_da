//! 网关 HTTP 路由的**唯一组装点**。
//!
//! 生产（`main.rs`）与集成测试都调 [`router`]，所以"测试里跑的路由"和"线上跑的路由"
//! 是同一份。早先测试自己复制过一段分流逻辑，结果只有测试覆盖到的那条路是对的——
//! 这种坑不该有第二次。
//!
//! 路由只有两条：
//!
//! | 路径 | 去向 |
//! |---|---|
//! | `/rpc` | [`crate::relay::ws_handler`]：鉴权后升级为 WebSocket |
//! | 其他 | [`crate::static_ui::static_handler`]：前端产物 / 404 / 503 |
//!
//! 两条路在**同一个端口**上，所以浏览器里"页面"和"数据通道"天然同源。

use axum::routing::get;
use axum::Router;

use crate::relay::{ws_handler, AppState};
use crate::static_ui::static_handler;

/// 组装网关的 HTTP service。
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/rpc", get(ws_handler))
        .fallback(static_handler)
        .with_state(state)
}
