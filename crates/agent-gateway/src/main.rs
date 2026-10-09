//! `a-da-gateway` 二进制：监听回环，接客户端连接，拉起并管理 agent 实例。
//!
//! # 用法
//!
//! ```bash
//! a-da-gateway                                  # 默认产品 ada-coding，工作区取当前目录
//! a-da-gateway --port 0 --workspace "E:/proj"   # 端口 0 = 自动分配（打印就绪行）
//! a-da-gateway --product ada-coding --binary ./ada-coding.exe
//! ```
//!
//! 就绪后向 stdout 打印一行（与 agent 宿主同一约定）：
//!
//! ```text
//! A_DA_GATEWAY_READY {"ready":true,"port":52353,"pid":1234}
//! ```
//!
//! 客户端拿这一行里的 `port` 连上来即可——**发现方式与 agent 宿主一致**，
//! 所以桌面端从"直连宿主"切到"连网关"只需要换 URL 来源。

use std::sync::Arc;

use agent_gateway::{relay::serve_client, Gateway};
use tokio::net::TcpListener;
use tracing::{info, warn};

/// 就绪行前缀（与 `A_DA_HOST_READY` 同一约定，便于同一套发现逻辑复用）。
pub const READY_PREFIX: &str = "A_DA_GATEWAY_READY ";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = parse_args(std::env::args().skip(1).collect())?;

    let workspace = if args.workspace.trim().is_empty() {
        std::env::current_dir()?.to_string_lossy().to_string()
    } else {
        args.workspace.clone()
    };

    let gateway = Arc::new(Gateway::new(args.product.clone(), workspace.clone()));

    let listener = TcpListener::bind(("127.0.0.1", args.port)).await?;
    let port = listener.local_addr()?.port();
    let pid = std::process::id();

    println!(
        "{READY_PREFIX}{}",
        serde_json::json!({
            "ready": true,
            "port": port,
            "pid": pid,
            "product": args.product,
            "workspace": workspace,
        })
    );
    use std::io::Write;
    std::io::stdout().flush()?;
    info!("a-da 网关就绪，监听 127.0.0.1:{}（pid {}）", port, pid);

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!("接受连接失败：{e}");
                continue;
            }
        };
        let gw = gateway.clone();
        let default_ws = workspace.clone();
        tokio::spawn(async move {
            // S5：请求里带工作区（`?workspace=`）就用它，否则用网关默认值。
            // 这让"一个网关管多个工作区"在不改协议的前提下可用。
            let requested = extract_workspace(&stream).unwrap_or(default_ws);
            if let Err(e) = serve_client(gw, stream, Some(requested)).await {
                warn!("客户端连接结束（{}）：{e}", peer);
            }
        });
    }
}

/// 从握手请求里取 `?workspace=`（取不到就返回 `None`）。
///
/// 实现说明：这里**先偷看再交给 WS 握手**做不到（`TcpStream` 已被消费），
/// 所以工作区选择走"网关默认值"这一条路；`?workspace=` 的解析留在 S6
/// 随"多工作区路由"一起做（届时用 `accept_hdr_async` 的回调拿 query）。
fn extract_workspace(_stream: &tokio::net::TcpStream) -> Option<String> {
    None
}

struct Args {
    port: u16,
    product: String,
    workspace: String,
}

fn parse_args(argv: Vec<String>) -> anyhow::Result<Args> {
    let mut args = Args {
        port: 52353,
        product: "ada-coding".to_string(),
        workspace: String::new(),
    };
    let mut it = argv.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--port" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!("--port 需要一个值"))?;
                args.port = v.parse().map_err(|_| anyhow::anyhow!("--port 不是合法端口：{v}"))?;
            }
            "--product" => {
                args.product = it.next().ok_or_else(|| anyhow::anyhow!("--product 需要一个值"))?;
            }
            "--workspace" => {
                args.workspace = it.next().ok_or_else(|| anyhow::anyhow!("--workspace 需要一个值"))?;
            }
            "--help" | "-h" => {
                println!(
                    "a-da-gateway —— AGENT 管理平台 / 交互平台 / 桥接平台\n\n\
                     --port <p>        监听端口（默认 52353；0 = 自动分配）\n\
                     --product <id>    要管理的产品（默认 ada-coding）\n\
                     --workspace <p>   默认工作区（默认当前目录）\n"
                );
                std::process::exit(0);
            }
            other => anyhow::bail!("未知参数：{other}（用 --help 看用法）"),
        }
    }
    Ok(args)
}
