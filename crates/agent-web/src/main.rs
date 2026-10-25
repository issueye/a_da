//! 独立运行入口：`cargo run -p agent-web --bin ssr-h`。
//!
//! 最终形态由网关挂载本 crate；这个二进制只用于开发与实机验收。

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    agent_web::standalone::run().await
}
