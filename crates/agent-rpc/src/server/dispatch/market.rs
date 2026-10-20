//! `market.*` 方法臂：配置、搜索、安装。
//!
//! 纯逻辑在 [`crate::server::market`]（可单测、无 Dispatcher 依赖），
//! 这里只做参数解包、错误映射与安装后的重装配。

use std::path::Path;

use super::*;
use crate::server::market as market_client;

fn market_error(error: anyhow::Error) -> ProtocolError {
    ProtocolError::internal_error(error.to_string())
}

impl Dispatcher {
    fn market_home(&self) -> PathBuf {
        self.plugin_mgr.app_home_dir()
    }

    pub(super) async fn dispatch_market(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ProtocolError> {
        match method {
            MARKET_GET_CONFIG => {
                let config = market_client::MarketConfig::load(&self.market_home());
                serde_json::to_value(market_client::MarketConfigView::from(&config))
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))
            }

            MARKET_SET_CONFIG => {
                let object = params.as_object().ok_or_else(|| {
                    ProtocolError::invalid_params("market.setConfig 需要一个参数对象")
                })?;
                let base_url = object
                    .get("baseUrl")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let api_token = object.get("apiToken").and_then(serde_json::Value::as_str);
                let clear_token = object
                    .get("clearToken")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let config = market_client::update_config(
                    &self.market_home(),
                    base_url,
                    api_token,
                    clear_token,
                )
                .await
                .map_err(market_error)?;
                Ok(serde_json::json!({
                    "ok": true,
                    "config": market_client::MarketConfigView::from(&config),
                }))
            }

            MARKET_SEARCH => {
                let query = params
                    .get("query")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let limit = params
                    .get("limit")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(24);
                let offset = params
                    .get("offset")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                let result = market_client::search(&self.market_home(), query, limit, offset)
                    .await
                    .map_err(market_error)?;
                serde_json::to_value(result)
                    .map_err(|e| ProtocolError::internal_error(e.to_string()))
            }

            MARKET_INSTALL => {
                let owner = params
                    .get("owner")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let slug = params
                    .get("slug")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let version = params
                    .get("version")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let overwrite = params
                    .get("overwrite")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if owner.is_empty() || slug.is_empty() || version.is_empty() {
                    return Err(ProtocolError::invalid_params(
                        "market.install 需要 owner / slug / version",
                    ));
                }

                let installed =
                    market_client::install(&self.market_home(), owner, slug, version, overwrite)
                        .await
                        .map_err(market_error)?;

                // 与 plugin.delete / plugin.createTemplate 同一条规矩：
                // 磁盘变了必须立刻重装配，否则引擎里还是旧贡献集合。
                let report = match &self.plugin_reassembler {
                    Some(reassembler) => reassembler.refresh("marketInstall").await,
                    None => None,
                };
                Ok(serde_json::json!({
                    "ok": true,
                    "plugin": installed,
                    "applied": report.as_ref().map(|r| r.applied),
                    "report": report,
                }))
            }

            MARKET_PUBLISH => {
                let file_path = params
                    .get("filePath")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let workspace = params.get("workspace").and_then(serde_json::Value::as_str);
                if file_path.is_empty() {
                    return Err(ProtocolError::invalid_params(
                        "market.publish 需要 filePath（插件包根 index.ts/index.js）",
                    ));
                }
                let entry = Path::new(file_path);
                // 只允许发布扩展目录下的包；避免把任意文件/目录打成 ZIP 上传
                if !self.plugin_mgr.is_extension_entry(workspace, entry) {
                    return Err(ProtocolError::invalid_params(
                        "只能发布扩展目录下的插件包（包根 index.ts/index.js）",
                    ));
                }
                let Some(package_dir) = entry.parent() else {
                    return Err(ProtocolError::invalid_params("无法定位插件包目录"));
                };
                let published = market_client::publish(
                    &self.market_home(),
                    market_client::PublishRequest {
                        package_dir,
                        slug: params
                            .get("slug")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(""),
                        name: params
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(""),
                        description: params
                            .get("description")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(""),
                        tags: params
                            .get("tags")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(""),
                        license: params
                            .get("license")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(""),
                        readme: params
                            .get("readme")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(""),
                        version: params
                            .get("version")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(""),
                        notes: params
                            .get("notes")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(""),
                    },
                )
                .await
                .map_err(market_error)?;
                Ok(serde_json::json!({
                    "ok": true,
                    "plugin": published,
                }))
            }

            _ => Err(ProtocolError::invalid_params(format!(
                "未知的市场方法: {method}"
            ))),
        }
    }
}
