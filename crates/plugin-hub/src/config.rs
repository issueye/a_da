use std::net::SocketAddr;
use std::path::PathBuf;

const DEFAULT_SECRET: &str = "dev-only-plugin-hub-secret-change-me-please-32b";

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url: String,
    pub data_dir: PathBuf,
    pub secret: String,
    pub secure_cookies: bool,
    pub max_upload_bytes: usize,
    /// OAuth 回调拼接用的外部访问地址（例如 https://plugins.example.com）。
    pub public_url: String,
    pub github_client_id: Option<String>,
    pub github_client_secret: Option<String>,
    /// 便于测试/企业代理覆盖；默认 https://api.github.com。
    pub github_api_base: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind = std::env::var("PLUGIN_HUB_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8787".to_string())
            .parse()?;
        let database_url = std::env::var("PLUGIN_HUB_DATABASE_URL")
            .unwrap_or_else(|_| "sqlite://data/plugin-hub.db".to_string());
        let data_dir = std::env::var("PLUGIN_HUB_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("data/plugin-assets"));
        let secret =
            std::env::var("PLUGIN_HUB_SECRET").unwrap_or_else(|_| DEFAULT_SECRET.to_string());
        let secure_cookies = std::env::var("PLUGIN_HUB_SECURE_COOKIES")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false);
        let max_upload_bytes = std::env::var("PLUGIN_HUB_MAX_UPLOAD_MB")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(20)
            .saturating_mul(1024 * 1024);
        let public_url = std::env::var("PLUGIN_HUB_PUBLIC_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8787".to_string())
            .trim_end_matches('/')
            .to_string();
        let github_client_id = std::env::var("PLUGIN_HUB_GITHUB_CLIENT_ID")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let github_client_secret = std::env::var("PLUGIN_HUB_GITHUB_CLIENT_SECRET")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let github_api_base = std::env::var("PLUGIN_HUB_GITHUB_API_BASE")
            .unwrap_or_else(|_| "https://api.github.com".to_string())
            .trim_end_matches('/')
            .to_string();

        Ok(Self {
            bind,
            database_url,
            data_dir,
            secret,
            secure_cookies,
            max_upload_bytes,
            public_url,
            github_client_id,
            github_client_secret,
            github_api_base,
        })
    }

    pub fn github_enabled(&self) -> bool {
        self.github_client_id.is_some() && self.github_client_secret.is_some()
    }

    pub fn is_default_secret(&self) -> bool {
        self.secret == DEFAULT_SECRET
    }

    /// 测试/嵌入式使用：给定临时目录拼一份不依赖环境变量的配置。
    pub fn for_tests(data_dir: PathBuf) -> Self {
        Self {
            bind: "127.0.0.1:0".parse().expect("测试地址合法"),
            database_url: "sqlite::memory:".to_string(),
            data_dir,
            secret: "test-secret-test-secret-test-secret-0123456789".to_string(),
            secure_cookies: false,
            max_upload_bytes: 8 * 1024 * 1024,
            public_url: "http://127.0.0.1:8787".to_string(),
            github_client_id: None,
            github_client_secret: None,
            github_api_base: "https://api.github.com".to_string(),
        }
    }
}
