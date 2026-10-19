use std::path::Path;

use plugin_hub::config::Config;
use plugin_hub::storage::Storage;
use plugin_hub::{AppState, app, db};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "plugin_hub=info,tower_http=info".into()),
        )
        .init();

    let config = Config::from_env()?;
    if config.is_default_secret() {
        tracing::warn!("PLUGIN_HUB_SECRET 仍是默认值；生产环境请替换为随机长串");
    }

    // sqlite://data/plugin-hub.db 这类 URL：父目录不先建好，create_if_missing 也会失败。
    if let Some(path) = config.database_url.strip_prefix("sqlite://") {
        if !path.starts_with(":memory:") {
            if let Some(parent) = Path::new(path).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
        }
    }

    let pool = db::connect(&config.database_url).await?;
    db::migrate(&pool).await?;
    let storage = Storage::new(config.data_dir.clone())?;
    let addr = config.bind;
    let state = AppState::new(pool, storage, config);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!(
        "ada-plugin-hub listening on http://{}",
        listener.local_addr()?
    );
    axum::serve(listener, app(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
