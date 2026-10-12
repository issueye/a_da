use agent_proto::PROTOCOL_VERSION;
use agent_rpc::server::{start_parent_watchdog, WsHostServer};
use agent_rpc::state::AgentStore;
use clap::Parser;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

/// 产品声明（W3-T2：宿主按它装配真引擎）。
const SPEC_JSON: &str = include_str!("../agent.spec.json");
/// **PM 模式预设**（不是第二个产品）。
///
/// PM 曾经是一个独立产品（`products/ada-pm`：另起一个二进制 + 一份重复的装配函数）。
/// 实测代价全在"两个产品"这件事本身：身份落盘/还原、徽标丢失、进程面板歧义、
/// 子智能体工具池按产品裁切两次返工——而它真正独有的能力（跨进程 gateway 委派）
/// 在真实使用里几乎没被走到。
///
/// 现在它只是一个**模式预设**：同一份 `AgentSpec` 形状、同一处装配函数、
/// 同一个二进制；差异只剩声明本身（persona / 不含 `fs`+`command` 的工具包 / 委派配置）。
const PM_MODE_SPEC_JSON: &str = include_str!("../modes/pm.agent.spec.json");

/// 模式预设的展示名（日志与错误信息用；**不是产品 id**）。
const PM_MODE_LABEL: &str = "PM 模式预设（ada-coding-pm）";

#[derive(Parser, Debug)]
#[command(author, version, about = "a_da 原生 Agent 核心服务 (Rust)", long_about = None)]
struct CliArgs {
    /// 启用主机模式
    #[arg(long, default_value_t = false)]
    host: bool,

    /// 监听端口（0 表示系统自动分配空闲端口）
    #[arg(long, default_value_t = 0)]
    port: u16,

    /// 握手认证令牌（与 UI 进程共享）
    #[arg(long)]
    token: Option<String>,

    /// 父进程 PID（提供后将启动看门狗自动防孤儿退出）
    #[arg(long)]
    parent_pid: Option<u32>,

    /// 工作区默认路径
    #[arg(long, default_value = "")]
    workspace: String,

    /// 网关服务 WebSocket 端点（如 ws://127.0.0.1:4000/rpc），指定后将主动连接并自注册
    #[arg(long)]
    gateway: Option<String>,
}

/// 按声明装配真引擎（**唯一实现**，Coding 与 PM 模式共用）；任何失败都如实降级为 legacy。
///
/// 注意三件事都是刻意的：
/// 1. **复用宿主的 `store`**：引擎的审批闸门要读 `config.approval`，各造一份会出现
///    "界面上是只读档、引擎按默认档跑"；
/// 2. **会话落盘到 app home**（不是工作区），与 legacy 路径一致；
/// 3. 返回的 `approval_mgr` 会被 ws 宿主用于构造 `Dispatcher`——**必须是同一个实例**，
///    否则 UI 的批准送不到闸门。
///
/// `spec_json` 决定"装哪一套"：产品声明（coding）或**模式预设**（pm）。
/// 早先这里是两份几乎逐行相同的拷贝（`build_engine_injection` /
/// `build_pm_engine_injection`）——一处改动要同步两遍，正是"第二个产品"的税。
fn build_injection(
    spec_json: &'static str,
    label: &'static str,
    store: &Arc<RwLock<AgentStore>>,
    workspace: &str,
    gateway_override: Option<&str>,
    apply_delegation: bool,
) -> Option<agent_rpc::server::dispatch::EngineInjection> {
    let mut spec = match agent_runtime::AgentSpec::from_json_str(spec_json) {
        Ok(s) => s,
        Err(e) => {
            warn!("{label} 规格解析失败，降级为 legacy 引擎：{e}");
            return None;
        }
    };

    // 委派目标按**启动参数**覆盖（只有 PM 模式预设需要这层覆盖：
    // 有网关就用网关委派，没有就退化成进程内本地委派，避免向虚构网关请求被拒）。
    if apply_delegation {
        match gateway_override {
            Some(endpoint) => {
                spec.capabilities.delegation = agent_runtime::DelegationMode::Gateway;
                spec.gateway = Some(agent_runtime::GatewaySpec {
                    endpoint: endpoint.to_string(),
                });
            }
            None => {
                spec.capabilities.delegation = agent_runtime::DelegationMode::Local;
            }
        }
    }

    let ws = if workspace.trim().is_empty() {
        let app_home = agent_node::session::app_home();
        app_home.dir("workspace")
    } else {
        std::path::PathBuf::from(workspace)
    };

    let sessions_root =
        std::path::Path::new(&agent_node::session::get_app_home()).join("sessions");
    let options = agent_host::HostOptions::new(ws)
        .with_store(store.clone())
        .with_sessions_root(sessions_root);

    match agent_host::run_from_spec(spec, options) {
        Ok(hosted) => {
            let tool_count = hosted.tool_names().len();
            let agent_host::HostedProduct { runtime, approval, spec, .. } = hosted;
            info!("已按 {label} 装配真引擎：工具 {tool_count} 个");

            let store_for_factory = store.clone();
            let gateway_override_owned = gateway_override.map(|s| s.to_string());
            let factory: agent_rpc::server::dispatch::EngineFactory = Arc::new(move |ws_path: &std::path::Path| -> Option<Arc<agent_base::engine::AgentRuntime>> {
                let mut spec_dyn = agent_runtime::AgentSpec::from_json_str(spec_json).ok()?;
                if apply_delegation {
                    match &gateway_override_owned {
                        Some(endpoint) => {
                            spec_dyn.capabilities.delegation = agent_runtime::DelegationMode::Gateway;
                            spec_dyn.gateway = Some(agent_runtime::GatewaySpec {
                                endpoint: endpoint.clone(),
                            });
                        }
                        None => {
                            spec_dyn.capabilities.delegation = agent_runtime::DelegationMode::Local;
                        }
                    }
                }
                let s_root = std::path::Path::new(&agent_node::session::get_app_home()).join("sessions");
                let opts = agent_host::HostOptions::new(ws_path)
                    .with_store(store_for_factory.clone())
                    .with_sessions_root(s_root);
                match agent_host::run_from_spec(spec_dyn, opts) {
                    Ok(h) => {
                        info!("为会话工作区 {} 动态装配 {label} 真引擎就绪", ws_path.display());
                        Some(Arc::new(h.runtime))
                    }
                    Err(err) => {
                        warn!("为工作区 {} 装配 {label} 引擎失败: {err}", ws_path.display());
                        None
                    }
                }
            });

            Some(agent_rpc::server::dispatch::EngineInjection {
                runtime: Arc::new(runtime),
                approval_mgr: approval,
                // W3-T6：声明一起注入，握手才能如实回报能力位
                spec: Arc::new(spec),
                factory: Some(factory),
            })
        }
        Err(e) => {
            warn!("{label} 真引擎装配失败：{e}");
            None
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let app_home = agent_node::session::get_app_home();
    let log_file_path = std::path::Path::new(&app_home).join("host.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file_path);

    if let Ok(file) = log_file {
        let subscriber = FmtSubscriber::builder()
            .with_max_level(Level::INFO)
            .with_target(false)
            .with_writer(file)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    } else {
        let subscriber = FmtSubscriber::builder()
            .with_max_level(Level::INFO)
            .with_target(false)
            .with_writer(std::io::sink)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    }

    let args = CliArgs::parse();

    let token = match args.token {
        Some(t) if !t.trim().is_empty() => t,
        _ => uuid::Uuid::new_v4().simple().to_string(),
    };

    // 启动父进程看门狗
    if let Some(parent_pid) = args.parent_pid {
        start_parent_watchdog(parent_pid, 2000);
    }

    let store = Arc::new(RwLock::new(AgentStore::new(args.workspace.clone())));

    let initial_ws = {
        let s = store.read().await;
        if !args.workspace.trim().is_empty() {
            args.workspace.clone()
        } else if !s.workspace.project.trim().is_empty() {
            s.workspace.project.clone()
        } else {
            s.public_workspace.clone()
        }
    };

    // W3-T2：按声明装配**真引擎**（coding 产品 + PM 模式预设），共用同一处装配函数。
    let injection = build_injection(
        SPEC_JSON,
        "ada-coding 产品声明",
        &store,
        &initial_ws,
        args.gateway.as_deref(),
        false,
    );
    let pm_injection = build_injection(
        PM_MODE_SPEC_JSON,
        PM_MODE_LABEL,
        &store,
        &initial_ws,
        args.gateway.as_deref(),
        true,
    );

    // 启动 WebSocket 服务
    let server = WsHostServer::bind_with_engines(args.port, token.clone(), store, injection, pm_injection).await?;
    let current_pid = std::process::id();

    // 打印符合协议 §1.8 规范的标准就绪行至 stdout
    //
    // `products` 是**如实自述**：这个进程装配了 coding 产品 + **PM 模式预设**。
    // PM 不是第二个产品（早先是，见 `PM_MODE_SPEC_JSON` 的注释），所以这里只自述
    // `ada-coding` 一个产品；"这条会话在跑 PM"由**会话模式**表达，不再靠产品 id。
    let ready_json = serde_json::json!({
        "ready": true,
        "port": server.port,
        "token": token,
        "pid": current_pid,
        "protocolVersion": PROTOCOL_VERSION,
        "products": served_products(),
    });

    println!("A_DA_HOST_READY {}", ready_json);
    std::io::stdout().flush()?;

    info!("a-da 原生核心就绪，PID: {}, 监听端口: {}", current_pid, server.port);

    // 若配置了 --gateway，启动后台协程主动向网关注册自身并保持长连接通道
    if let Some(gateway_endpoint) = args.gateway {
        let server_port = server.port;
        let auth_token = token.clone();
        let ws_path = args.workspace.clone();
        tokio::spawn(async move {
            register_to_gateway("ada-coding", &gateway_endpoint, server_port, &auth_token, &ws_path, current_pid).await;
        });
    }

    // 持续运行服务直到收到终止信号或父进程看门狗触发退出
    #[cfg(target_os = "windows")]
    {
        match tokio::signal::windows::ctrl_c() {
            Ok(mut sig) => {
                tokio::select! {
                    Some(_) = sig.recv() => {
                        info!("收到 Ctrl+C 退出信号，a-da 原生核心安全关闭");
                    }
                    _ = std::future::pending::<()>() => {}
                }
            }
            Err(e) => {
                info!("当前环境未附加控制台，由看门狗看护退出: {}", e);
                std::future::pending::<()>().await;
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = tokio::signal::ctrl_c().await;
        info!("收到退出信号，a-da 原生核心安全关闭");
    }

    Ok(())
}

/// 本进程**实际服务**的产品清单（如实自述，供网关与进程管理面板使用）。
///
/// **只有一个产品**：PM 已经降级为 ada-coding 的模式预设（同一二进制、同一装配函数），
/// 不再是一个产品。会话是 coding 还是 pm 由 `mode` 表达——面板不再需要靠产品清单
/// 去猜"这条会话是不是 PM"。
fn served_products() -> Vec<&'static str> {
    vec!["ada-coding"]
}

/// 主动连接网关并完成自主注册。
async fn register_to_gateway(
    product: &str,
    gateway_url: &str,
    server_port: u16,
    token: &str,
    workspace: &str,
    pid: u32,
) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let ws = if workspace.trim().is_empty() {
        std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
    } else {
        workspace.to_string()
    };

    let mut connect_url = gateway_url.trim().to_string();
    let delim = if connect_url.contains('?') { "&" } else { "?" };
    connect_url.push_str(&format!("{delim}role=agent"));

    info!("正在连接网关进行自注册: {}", connect_url);

    let mut stream = None;
    for attempt in 1..=30 {
        match connect_url.clone().into_client_request() {
            Ok(req) => match tokio_tungstenite::connect_async(req).await {
                Ok((ws_stream, _)) => {
                    stream = Some(ws_stream);
                    break;
                }
                Err(e) => {
                    warn!("连接网关失败（尝试 {attempt}/30）: {e}");
                }
            },
            Err(e) => {
                warn!("构造网关连接请求失败: {e}");
                return;
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    }

    let Some(mut ws_stream) = stream else {
        warn!("未能连上网关服务，放弃自注册");
        return;
    };

    let endpoint = format!("ws://127.0.0.1:{server_port}/rpc?token={token}");
    let register_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "gateway.register",
        "params": {
            "product": product,
            "workspace": ws,
            "endpoint": endpoint,
            "pid": pid,
            "products": served_products(),
        }
    });

    if let Err(e) = ws_stream.send(tokio_tungstenite::tungstenite::Message::Text(register_req.to_string().into())).await {
        warn!("发送 gateway.register 失败: {e}");
        return;
    }

    if let Some(Ok(resp)) = ws_stream.next().await {
        info!("网关注册响应: {resp}");
    }

    info!("{} 已成功向网关自注册，保持长连接通道...", product);
    while let Some(msg) = ws_stream.next().await {
        if let Ok(m) = msg {
            if m.is_close() {
                warn!("网关控制连接已关闭");
                break;
            }
        } else {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    const SPEC_JSON: &str = include_str!("../agent.spec.json");
    const PM_MODE_SPEC_JSON: &str = include_str!("../modes/pm.agent.spec.json");

    #[test]
    fn test_ada_coding_spec_is_valid() {
        let spec = agent_runtime::AgentSpec::from_json_str(SPEC_JSON).expect("spec 必须合法");
        assert_eq!(spec.id, "ada-coding");
        assert_eq!(spec.archetype, "coding");
        assert!(spec.capabilities.rollback);
        assert!(spec.capabilities.plugins);
    }

    /// 守门：**PM 是模式预设，不是第二个产品**。
    ///
    /// 这条钉住三件事：
    /// 1. 预设与产品走**同一个** `AgentSpec` 形状（同一处装配函数吃得下）；
    /// 2. 预设 id **不是**产品 id（`ada-coding-pm`，避免"又一个产品"的错觉）；
    /// 3. 角色边界仍在声明里：**不含** `fs` 与 `command`（PM 不亲自改代码/跑命令）。
    #[test]
    fn test_pm_is_a_mode_preset_not_a_product() {
        let preset = agent_runtime::AgentSpec::from_json_str(PM_MODE_SPEC_JSON)
            .expect("PM 模式预设必须能按同一 AgentSpec 解析");

        assert_eq!(preset.id, "ada-coding-pm", "预设 id 必须与产品 id 区分开");
        assert_ne!(preset.id, "ada-coding");
        assert!(
            !preset.toolkits.iter().any(|t| t == "fs" || t == "command"),
            "PM 预设不得声明写/命令工具包（角色边界）：{:?}",
            preset.toolkits
        );
        assert!(
            preset.capabilities.subagents,
            "PM 的价值就是拆解与分派 → 必须声明子智能体"
        );

        // 声明 → 工具包真源：预设声明的每个工具包都必须能真的装配出来
        let tools = agent_toolkit::tools_for_toolkits(&preset.toolkits, std::path::Path::new("."))
            .expect("PM 预设声明的工具包必须都能装配");
        assert!(!tools.is_empty());
        let names: Vec<String> = tools.iter().map(|t| t.descriptor().name.clone()).collect();
        assert!(
            !names.iter().any(|n| n == "write_file" || n == "run_command"),
            "PM 自己的手不能写盘/跑命令：{names:?}"
        );
    }

    /// 守门：本进程自述的产品清单里**只有 ada-coding**（PM 不再是产品）。
    #[test]
    fn test_served_products_lists_only_the_product() {
        assert_eq!(super::served_products(), vec!["ada-coding"]);
    }

    /// W2-T2 守门：产品**声明的工具包**必须能真的装配出工具，且与注册表逐名对齐。
    ///
    /// 这条把"声明 → 工具包真源 → catalog"整条链路钉在产品自己的测试里：
    /// 声明了一个不存在的工具包（例如曾经的 `patch`）会在这里直接红。
    #[test]
    fn test_ada_coding_declared_toolkits_assemble() {
        use std::collections::BTreeSet;

        let spec = agent_runtime::AgentSpec::from_json_str(SPEC_JSON).expect("spec 必须合法");
        let tools = agent_toolkit::tools_for_toolkits(&spec.toolkits, std::path::Path::new("."))
            .expect("声明的工具包必须都能装配（未知名/重名都会 Err）");

        let assembled: BTreeSet<String> =
            tools.iter().map(|t| t.descriptor().name.clone()).collect();
        let registry: BTreeSet<String> = agent_toolkit::standard_tool_descriptors()
            .iter()
            .map(|d| d.name.clone())
            .collect();

        // 工具包只能提供注册表里的工具（不许造出注册表没有的名字）
        assert!(
            assembled.is_subset(&registry),
            "工具包提供了注册表里没有的工具：{:?}",
            assembled.difference(&registry).collect::<Vec<_>>()
        );

        // W4-T5：唯一**不由工具包**提供的是宿主注入的委派工具
        // （它需要 SubagentManager/父 provider/检查点，工具包工厂构造不出来）。
        let host_provided: BTreeSet<String> =
            registry.difference(&assembled).cloned().collect();
        assert_eq!(
            host_provided,
            BTreeSet::from(["invoke_subagent".to_string()]),
            "除 invoke_subagent 外不应有其它工具指望宿主注入"
        );
        assert!(
            spec.capabilities.subagents,
            "ada-coding 声明支持子智能体 → 宿主会注入 invoke_subagent；\
             若这里为 false，注册表里的 invoke_subagent 就会成为不可达工具"
        );

        assert!(
            assembled.contains("git_status") && assembled.contains("project_inspect"),
            "W2-T2 的插件工具应已进 catalog：{assembled:?}"
        );
    }

    #[tokio::test]
    async fn test_ada_coding_conformance() {
        agent_conformance::assert_single_engine_executes_turn(SPEC_JSON)
            .await
            .expect("ada-coding 产品单一引擎合规性校验通过");
    }
}
