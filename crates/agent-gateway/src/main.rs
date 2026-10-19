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
use agent_gateway::relay::AppState;
use agent_gateway::static_ui::StaticSite;
use agent_gateway::supervisor::ReapPolicy;
use agent_gateway::Gateway;
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
    // 组合根显式记一次默认工作区（构造 `Gateway` 本身不再有落盘副作用，见 `Gateway::new` 注释）
    if !workspace.trim().is_empty() {
        gateway.workspaces.record(&workspace, Some(&args.product));
    }

    let (listener, port) = bind_listener(&args.host, args.port).await?;
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

    // 前端产物的托管目录（S8 · 网页访问）。
    //
    // 没找到不算错：桌面端走 file://，`/rpc` 也照常可用，只是浏览器打开根路径会拿到
    // 一页说明。所以这里是 warn 而不是退出——**别把一个可选能力做成启动前置条件**。
    let site = StaticSite::resolve(args.ui_dir.as_deref());
    match site.as_ref() {
        Some(site) => info!(
            "网页访问：http://{}:{}/（前端产物 {}）",
            if is_loopback(&args.host) { "127.0.0.1" } else { args.host.as_str() },
            port,
            site.display_root()
        ),
        None => warn!(
            "未找到前端产物（desktop-ui/dist），浏览器访问根路径只会得到一页说明；\
             可先 `npm --prefix desktop-ui run build`，或用 --ui-dir 指定目录"
        ),
    }

    // 实例回收：没有它，访问过 N 个工作区就常驻 N 个 ada-coding 进程。
    // 策略可用环境变量微调（默认：空闲 15 分钟回收、最多 6 个实例、每分钟扫一次）
    let policy = ReapPolicy {
        idle_after: std::time::Duration::from_secs(
            std::env::var("A_DA_GATEWAY_IDLE_REAP_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(15 * 60),
        ),
        max_live: std::env::var("A_DA_GATEWAY_MAX_LIVE_AGENTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6),
        interval: std::time::Duration::from_secs(
            std::env::var("A_DA_GATEWAY_REAP_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
        ),
        ..ReapPolicy::default()
    };
    gateway.spawn_reaper(policy);
    info!(
        "实例回收策略：空闲 {:?} 回收、同时在跑上限 {}、每 {:?} 扫一次",
        policy.idle_after, policy.max_live, policy.interval
    );
    // 网关收到 Ctrl-C / 终止信号时，把自己拉起的 agent 一并收掉（不留孤儿进程）
    let gateway_for_shutdown = gateway.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let n = gateway_for_shutdown.registry.shutdown_all();
            info!("网关收到退出信号，已请求关停 {n} 个 agent 实例");
            std::process::exit(0);
        }
    });

    // ── 路由：页面与 WebSocket 在同一个 axum Router 里 ──────────────────────
    //
    // 以前这里是"裸 TCP 监听 + 手写 HTTP 状态行 + tungstenite 握手"三套东西拼在一起，
    // 还得先 peek 请求头再分流。现在 `/rpc` 与静态资源是同一个 service 的两条路由：
    // 升级、校验、连接收尾全部由 hyper 负责，没有"我们绕过它自己 close"的路径
    // （那个路径正是之前 RST/白屏的来源）。
    if let Err(e) = write_endpoint_file(port, pid, &workspace, auth_required) {
        warn!("写入网关发现文件失败（不影响服务）：{e}");
    }

    let state = AppState {
        gateway: gateway.clone(),
        default_workspace: workspace.clone(),
        site: site.map(Arc::new),
    };
    let app = agent_gateway::router::router(state);

    info!("HTTP 已就绪（axum）：页面 / 与 WebSocket /rpc 共用端口 {port}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// 绑定监听端口。
///
/// 🔴 桌面端会**固定**传 52353：浏览器要"打开就能用"，端口就不能是随机的。
/// 端口被占用时顺延（52353 → 52363），只有显式 `--port 0` 才让内核分配。
/// 实际端口会写进发现文件（`gateway_endpoint.json`），界面据此给出可点击的地址。
async fn bind_listener(host: &str, port: u16) -> anyhow::Result<(TcpListener, u16)> {
    if port == 0 {
        let listener = TcpListener::bind((host, 0)).await?;
        let actual = listener.local_addr()?.port();
        return Ok((listener, actual));
    }

    let mut last_err = None;
    for candidate in port..=port.saturating_add(10) {
        match TcpListener::bind((host, candidate)).await {
            Ok(listener) => {
                if candidate != port {
                    warn!("端口 {port} 被占用，顺延到 {candidate}（地址见 gateway_endpoint.json）");
                }
                return Ok((listener, candidate));
            }
            Err(e) => last_err = Some(e),
        }
    }

    Err(anyhow::anyhow!(
        "无法绑定 {host}:{port}..{}：{}",
        port.saturating_add(10),
        last_err.map(|e| e.to_string()).unwrap_or_default()
    ))
}

/// 应用目录（`A_DA_HOME` 优先，否则 `~/.a-da`）。
fn app_home() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("A_DA_HOME") {
        if !home.trim().is_empty() {
            return std::path::PathBuf::from(home.trim());
        }
    }
    let user = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    if user.is_empty() {
        std::path::PathBuf::from(".a-da")
    } else {
        std::path::PathBuf::from(user).join(".a-da")
    }
}

/// 写一份端点发现文件：`<app_home>/gateway_endpoint.json`。
///
/// 浏览器与外部工具都不知道"这次网关起在哪个端口"（桌面端会顺延），文件就是答案。
/// **不写 token**：凭据只从启动参数或 `gateway.pair` 配对码来，不落在磁盘上等人捡。
fn write_endpoint_file(
    port: u16,
    pid: u32,
    workspace: &str,
    auth_required: bool,
) -> std::io::Result<()> {
    use std::io::Write;

    let home = app_home();
    std::fs::create_dir_all(&home)?;
    let payload = serde_json::json!({
        "port": port,
        "pid": pid,
        "workspace": workspace,
        "url": format!("http://127.0.0.1:{port}/"),
        "rpc": format!("ws://127.0.0.1:{port}/rpc"),
        "authRequired": auth_required,
    });

    let mut file = std::fs::File::create(home.join("gateway_endpoint.json"))?;
    let text = serde_json::to_string_pretty(&payload)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    file.write_all(text.as_bytes())
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
    /// 前端产物目录（`--ui-dir`；不传则自动探测）
    ui_dir: Option<String>,
}

fn parse_args(argv: Vec<String>) -> anyhow::Result<Args> {
    let mut port = 52353u16;
    let mut host = "127.0.0.1".to_string();
    let mut product = "ada-coding".to_string();
    let mut workspace = String::new();
    let mut tokens: Vec<String> = Vec::new();
    let mut origins: Vec<String> = Vec::new();
    let mut allow_plaintext = false;
    let mut ui_dir: Option<String> = None;
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
            "--ui-dir" => {
                ui_dir = Some(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--ui-dir 需要一个值"))?,
                );
            }
            "--allow-origin" => {
                origins.push(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--allow-origin 需要一个值"))?,
                );
            }
            "--help" | "-h" => {
                println!(
                    "ada-gateway —— AGENT 管理平台 / 交互平台 / 桥接平台\n\n\
                     --port <p>        监听端口（默认 52353；0 = 自动分配）\n\
                     --product <id>    要管理的产品（默认 ada-coding）\n\
                     --workspace <p>   默认工作区（默认当前目录）\n\
                     --host <h>        监听地址（默认 127.0.0.1；非回环**必须**给 --token）\n\
                     --token <t>       接入令牌；`<t>` 或不限制工作区，`<t>=<ws1>|<ws2>` 限定；可重复\n\
                     --allow-origin <o>  放行的浏览器来源（可重复；默认**拒绝**任何带 Origin 的请求）\n\
                     --allow-plaintext   显式承认「非回环 + 明文 ws」（典型：反向代理终止 TLS）\n\
                     --ui-dir <p>      前端产物目录（默认自动探测 desktop-ui/dist；给了就用它托管网页）\n"
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
        ui_dir,
    })
}
