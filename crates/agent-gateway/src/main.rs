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
//! # 远程接入（S7）
//!
//! ```bash
//! # 回环：默认，不需要 token
//! a-da-gateway --port 0
//!
//! # 对外：**必须**给 token（否则拒绝启动），并显式放行浏览器来源
//! a-da-gateway --host 0.0.0.0 --token "$(openssl rand -hex 16)" \
//!              --allow-origin https://app.example
//!
//! # 多租户：每个 token 限定自己的工作区
//! a-da-gateway --host 0.0.0.0 --token t1=E:/a --token t2=E:/b
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

use agent_gateway::auth::{is_loopback, AuthConfig};
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

    // 🔴 失败安全：**非回环地址 + 没有 token = 拒绝启动**。
    //
    // 为什么是"拒绝启动"而不是"警告后继续"：网关能驱动 agent 改工作区里的文件。
    // 一个对外可达、无人认证的网关是**真漏洞**，而"启动了但没人发现"是它最常见的形态。
    // 宁可起不来。
    if !is_loopback(&args.host) && !args.auth.requires_token() {
        anyhow::bail!(
            "拒绝启动：`--host {}` 不是回环地址，但没有任何 `--token`。\n\
             对外可达且无认证的网关等于把 agent（以及它能改的文件）公开出去。\n\
             用法：--token <token>（多租户可写 --token <token>=<工作区>）",
            args.host
        );
    }

    // 🔴 明文规则（S7b）：token 走明文 `ws://` 是**可嗅探的**。
    //
    // 非回环部署必须**显式承认**这件事（`--allow-plaintext`），典型场景是
    // "反向代理终止 TLS、网关只在内网明文"——那时明文是可接受的，
    // 但必须由人确认，而不是默认发生。
    if !is_loopback(&args.host) && !args.allow_plaintext {
        anyhow::bail!(
            "拒绝启动：`--host {}` 不是回环地址，而网关只提供明文 `ws://`。\n\
             明文连接上的 token 可被嗅探。两种做法：\n\
             ① 让反向代理终止 TLS（网关只在内网监听），并显式加 `--allow-plaintext` 表示已知情；\n\
             ② 监听回环，由本机的桌面端/浏览器连 `127.0.0.1`。",
            args.host
        );
    }

    let auth_required = args.auth.requires_token();

    // 配了 token 就发一个**一次性配对码**：浏览器没有地方拿 token，
    // 操作员从网关控制台抄一次短码即可（见 `gateway.pair`）。
    if auth_required {
        let code = args.auth.issue_pairing_code(Vec::new());
        // 只写日志、**不进 READY 行**：READY 行是机器解析的、可能被采集，
        // 而配对码是给**人**抄的，人在控制台看。
        info!(
            "配对码：{code}（一次性，{} 分钟内有效）——浏览器用它换 token：gateway.pair {{ code }}",
            agent_gateway::auth::PAIRING_TTL.as_secs() / 60
        );
    }
    let auth_summary = if auth_required {
        format!("token 鉴权已启用（{} 个令牌）", args.token_count)
    } else {
        "开放模式（仅回环；未配置 --token）".to_string()
    };
    let gateway = Arc::new(
        Gateway::new(args.product.clone(), workspace.clone()).with_auth(args.auth),
    );

    let listener = TcpListener::bind((args.host.as_str(), args.port)).await?;
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
            "host": args.host,
            // 客户端据此知道要不要带 token（**不是**把 token 放进来）
            "authRequired": auth_required,
        })
    );
    use std::io::Write;
    std::io::stdout().flush()?;
    info!(
        "a-da 网关就绪，监听 {}:{}（pid {}；{}）",
        args.host, port, pid, auth_summary
    );

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
            // S7：`?workspace=` 与 `?token=` 都在**握手里**解析（`serve_client` 内），
            // 这里只把网关默认工作区作为兜底传进去。
            // 这让"一个网关管多个工作区"在不改协议的前提下可用。
            if let Err(e) = serve_client(gw, stream, Some(default_ws)).await {
                warn!("客户端连接结束（{}）：{e}", peer);
            }
        });
    }
}

struct Args {
    port: u16,
    host: String,
    product: String,
    workspace: String,
    auth: AuthConfig,
    /// 配了几个令牌（**只报数量**，不报内容——就绪行可能被写进日志）
    token_count: usize,
    /// 显式承认"非回环 + 明文"（典型：反向代理终止 TLS）
    allow_plaintext: bool,
}

fn parse_args(argv: Vec<String>) -> anyhow::Result<Args> {
    let mut port = 52353u16;
    let mut host = "127.0.0.1".to_string();
    let mut product = "ada-coding".to_string();
    let mut workspace = String::new();
    let mut tokens: Vec<String> = Vec::new();
    let mut origins: Vec<String> = Vec::new();
    let mut allow_plaintext = false;
    let mut it = argv.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--port" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!("--port 需要一个值"))?;
                port = v.parse().map_err(|_| anyhow::anyhow!("--port 不是合法端口：{v}"))?;
            }
            "--host" => {
                host = it.next().ok_or_else(|| anyhow::anyhow!("--host 需要一个值"))?;
            }
            "--product" => {
                product = it.next().ok_or_else(|| anyhow::anyhow!("--product 需要一个值"))?;
            }
            "--workspace" => {
                workspace = it.next().ok_or_else(|| anyhow::anyhow!("--workspace 需要一个值"))?;
            }
            "--token" => {
                tokens.push(it.next().ok_or_else(|| anyhow::anyhow!("--token 需要一个值"))?);
            }
            "--allow-plaintext" => {
                allow_plaintext = true;
            }
            "--allow-origin" => {
                origins.push(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--allow-origin 需要一个值"))?,
                );
            }
            "--help" | "-h" => {
                println!(
                    "a-da-gateway —— AGENT 管理平台 / 交互平台 / 桥接平台\n\n\
                     --port <p>        监听端口（默认 52353；0 = 自动分配）\n\
                     --product <id>    要管理的产品（默认 ada-coding）\n\
                     --workspace <p>   默认工作区（默认当前目录）\n\
                     --host <h>        监听地址（默认 127.0.0.1；非回环**必须**给 --token）\n\
                     --token <t>       接入令牌；`<t>` 或不限制工作区，`<t>=<ws1>|<ws2>` 限定；可重复\n\
                     --allow-origin <o>  放行的浏览器来源（可重复；默认**拒绝**任何带 Origin 的请求）\n\
                     --allow-plaintext   显式承认「非回环 + 明文 ws」（典型：反向代理终止 TLS）\n"
                );
                std::process::exit(0);
            }
            other => anyhow::bail!("未知参数：{other}（用 --help 看用法）"),
        }
    }
    let auth = AuthConfig::from_args(&tokens, &origins).map_err(|e| anyhow::anyhow!(e))?;
    let token_count = tokens.len();
    Ok(Args {
        port,
        host,
        product,
        workspace,
        auth,
        token_count,
        allow_plaintext,
    })
}
