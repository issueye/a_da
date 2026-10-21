//! 插件渲染上下文：只暴露显式投影，不把 `AppState` 整份交给插件。

use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct AppMeta {
    pub workspace: String,
    pub gateway: String,
    pub product: String,
    pub version: String,
}

#[derive(Debug, Clone, Default)]
pub struct RouteMeta {
    pub path: String,
    pub query: String,
}

#[derive(Debug, Clone, Default)]
pub struct RenderContext {
    pub app: AppMeta,
    pub route: RouteMeta,
    pub data: Value,
}

impl RenderContext {
    pub fn new(app: AppMeta) -> Self {
        Self {
            app,
            route: RouteMeta::default(),
            data: Value::Null,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }

    pub fn with_route_path(mut self, path: impl Into<String>) -> Self {
        self.route.path = path.into();
        self
    }
}
