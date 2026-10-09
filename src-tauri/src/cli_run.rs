//! `a-da run` —— 命令行单轮任务执行器（W6-T3）。
//!
//! # 为什么单独一个模块
//!
//! 原先 `main.rs` 的 `Run` 分支只做了三件事：建一个**内存** `AgentStore`、
//! `create_thread`、打印两行——**既不跑模型也不落盘**。命令行用户看到
//! "创建任务会话成功"之后什么都没有发生。
//!
//! 现在这里做真事：按产品声明装配引擎 → 建会话 → 落用户消息 → 跑一轮 →
//! 从会话里读回复 → 输出。**会话消息由 `FsSessionStore` 落成 JSONL 文件**。
//!
//! # 可测性
//!
//! 生产路径用真实 `NetworkModelClient`（`model = None`）；测试注入脚本化模型。
//! 另提供 `--dry-run`：装配 + 跑一轮 + 落盘全部照常，只把模型换成回显替身——
//! 这样**没有网络也能验证装配是否正确**（也是集成测试的抓手）。

use std::path::PathBuf;
use std::sync::Arc;

use agent_base::domain::{AgentMessage, TurnStopReason};
use agent_base::model::ProviderConfig;
use agent_base::ports::ModelClient;
use agent_host::{HostOptions, HostedProduct};
use agent_runtime::AgentSpec;
use tokio::sync::RwLock;

/// 一次命令行任务的结果。
#[derive(Debug, Clone)]
pub struct CliRunOutcome {
    pub thread_id: String,
    /// 助手最终回复（从会话里读回，与落盘内容一致）
    pub text: String,
    /// 停机原因（`Completed` / `Aborted` / `BudgetExhausted` / …）
    pub stop_reason: String,
    /// 会话落盘目录
    pub sessions_root: PathBuf,
}

/// 产品声明（与宿主同源：`products/ada-coding/agent.spec.json`）。
pub const PRODUCT_SPEC_JSON: &str = include_str!("../../products/ada-coding/agent.spec.json");

/// `--dry-run` 用的回显模型：不联网，直接回一段可辨识的文本。
///
/// 它不是"假成功"——装配、会话、落盘、事件投影全部走真实路径，
/// 只有"模型怎么回答"被替换。因此它能证明**除网络以外**的链路是通的。
pub fn dry_run_model(prompt_echo: &str) -> Arc<dyn ModelClient> {
    use agent_base::model::StreamDelta;
    use agent_base::testing::ScriptedModelClient;

    Arc::new(ScriptedModelClient::new(vec![vec![
        StreamDelta::Text {
            text: format!("[dry-run] 已收到任务：{prompt_echo}"),
        },
        StreamDelta::Done {
            stop_reason: "stop".to_string(),
        },
    ]]))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 真实路径下必须**有**可用的 provider 配置——否则如实报错。
///
/// 抽成纯函数是为了**可确定地断言**：直接跑 `run_task` 会读机器上的真实配置，
/// 于是"有没有配置"取决于开发机状态（本机有配置 → 测试会真的发网络请求并挂住）。
///
/// legacy 在这里的旧行为是**造一个 `gemini-2.5-flash` + 空 api_key 的配置继续跑**——
/// 等于静默用一个用户没配过的模型。这条断言钉住"不许编造"。
fn check_provider_available(config: &ProviderConfig, model_injected: bool) -> Result<(), String> {
    if model_injected {
        return Ok(());
    }
    if config.base_url.trim().is_empty() {
        return Err(
            "没有可用的模型供应商配置。请先在应用设置里配置供应商，\
             或用 `--dry-run` 验证装配（不联网）"
                .to_string(),
        );
    }
    Ok(())
}

/// 跑一次命令行任务。
///
/// `model` 为 `None` 时走真实网络模型客户端。
pub async fn run_task(
    workspace: &str,
    prompt: &str,
    model: Option<Arc<dyn ModelClient>>,
) -> anyhow::Result<CliRunOutcome> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        anyhow::bail!("任务指令为空：用法 `a-da run --workspace . \"审查代码\"`");
    }

    let ws_path = if workspace.trim().is_empty() {
        std::env::current_dir()?
    } else {
        PathBuf::from(workspace)
    };
    if !ws_path.exists() {
        anyhow::bail!("工作区不存在：{}", ws_path.display());
    }

    let spec = AgentSpec::from_json_str(PRODUCT_SPEC_JSON)
        .map_err(|e| anyhow::anyhow!("产品声明解析失败：{e}"))?;

    // 会话态：命令行没有界面，所以自己造一个（配置从磁盘读）
    let store = Arc::new(RwLock::new(agent_rpc::state::AgentStore::new(
        ws_path.to_string_lossy().to_string(),
    )));
    let config = { store.read().await.provider.clone() };

    // 真实路径必须要有可用的 provider；**不编造**（legacy 会造一个 gemini 配置继续跑）
    check_provider_available(&config, model.is_some()).map_err(|e| anyhow::anyhow!(e))?;

    let sessions_root = ws_path.join(".a-da").join("sessions");
    let mut options = HostOptions::new(&ws_path)
        .with_store(store.clone())
        .with_sessions_root(&sessions_root);
    if let Some(m) = model {
        options = options.with_model(m);
    }

    let hosted: HostedProduct = agent_host::run_from_spec(spec, options)
        .map_err(|e| anyhow::anyhow!("按产品声明装配引擎失败：{e}"))?;

    // 建会话
    let thread_id = {
        let mut s = store.write().await;
        s.create_thread(Some(format!("CLI: {}", truncate(prompt, 40))), None)
    };

    // 用户消息落进会话（引擎从会话读历史；这一步同时产生会话文件）
    hosted
        .runtime
        .store
        .append_message(
            &thread_id,
            &AgentMessage::User {
                content: prompt.to_string(),
                images: None,
                timestamp: Some(now_ms()),
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("写入用户消息失败：{e}"))?;

    // 跑一轮
    let outcome = hosted
        .run_turn(&thread_id, config)
        .await
        .map_err(|e| anyhow::anyhow!("执行任务失败：{e}"))?;

    // 从会话读回助手回复（与落盘内容一致，不另造一份）
    let messages = hosted
        .runtime
        .store
        .load_messages(&thread_id)
        .await
        .map_err(|e| anyhow::anyhow!("读取会话失败：{e}"))?;
    let text = messages
        .iter()
        .rev()
        .find_map(|m| match m {
            AgentMessage::Assistant { content, .. } if !content.trim().is_empty() => {
                Some(content.clone())
            }
            _ => None,
        })
        .unwrap_or_default();

    let stop_reason = match &outcome.stop_reason {
        TurnStopReason::Completed => "Completed".to_string(),
        TurnStopReason::Aborted => "Aborted".to_string(),
        TurnStopReason::BudgetExhausted { limit_steps } => {
            format!("BudgetExhausted({limit_steps})")
        }
        TurnStopReason::ModelError => "ModelError".to_string(),
        TurnStopReason::Denied => "Denied".to_string(),
    };

    Ok(CliRunOutcome {
        thread_id,
        text,
        stop_reason,
        sessions_root,
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_ws(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "a_da_cli_{tag}_{}_{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).expect("建临时工作区");
        dir
    }

    /// **W6-T3 出口判据**：`run` 真的跑一轮并且**落盘**。
    #[tokio::test]
    async fn test_run_task_executes_and_persists_the_session() {
        let ws = tmp_ws("persist");
        let model = dry_run_model("审查代码");

        let outcome = run_task(
            ws.to_string_lossy().as_ref(),
            "审查代码",
            Some(model),
        )
        .await
        .expect("dry-run 必须成功");

        assert_eq!(outcome.stop_reason, "Completed");
        assert!(
            outcome.text.contains("[dry-run] 已收到任务：审查代码"),
            "回复应来自模型流：{}",
            outcome.text
        );

        // 会话文件必须真的落盘，且包含用户消息与助手回复
        assert!(
            outcome.sessions_root.exists(),
            "会话根目录必须存在：{}",
            outcome.sessions_root.display()
        );
        let mut files: Vec<PathBuf> = Vec::new();
        collect_files(&outcome.sessions_root, &mut files);
        assert!(
            !files.is_empty(),
            "必须产出会话文件：{}",
            outcome.sessions_root.display()
        );

        let all: String = files
            .iter()
            .filter_map(|f| std::fs::read_to_string(f).ok())
            .collect();
        assert!(all.contains("审查代码"), "落盘内容必须含用户消息：{all}");
        assert!(
            all.contains("[dry-run]"),
            "落盘内容必须含助手回复：{all}"
        );

        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 空指令必须**如实报错**，而不是"创建会话成功"就退出。
    #[tokio::test]
    async fn test_empty_prompt_is_an_error() {
        let ws = tmp_ws("empty");
        let err = run_task(ws.to_string_lossy().as_ref(), "   ", Some(dry_run_model("x")))
            .await
            .expect_err("空指令必须报错");
        assert!(err.to_string().contains("任务指令为空"), "{err}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 不存在的路径必须报错（而不是悄悄在别处建会话）。
    #[tokio::test]
    async fn test_missing_workspace_is_an_error() {
        let err = run_task(
            "E:/definitely/not/here/a_da_w6t3",
            "做点事",
            Some(dry_run_model("x")),
        )
        .await
        .expect_err("不存在的路径必须报错");
        assert!(err.to_string().contains("工作区不存在"), "{err}");
    }

    /// 没有 provider 配置且没有注入模型时，必须报错并提示 `--dry-run`——
    /// **不许编造一个配置继续跑**。
    ///
    /// 用纯函数断言而不是跑 `run_task`：后者会读**开发机上的真实配置**，
    /// 于是"有没有配置"取决于环境（本机有配置时测试会真的发网络请求并挂住）。
    #[test]
    fn test_missing_provider_is_an_error_not_a_fabricated_config() {
        let mut config = ProviderConfig {
            id: "p".into(),
            name: "p".into(),
            protocol: Default::default(),
            base_url: String::new(),
            api_key: String::new(),
            model: "m".into(),
            max_output_tokens: None,
            custom_headers: None,
            proxy_url: None,
        };

        // 没配置 + 没注入模型 → 报错
        let err = check_provider_available(&config, false).expect_err("必须报错");
        assert!(err.contains("没有可用的模型供应商配置"), "{err}");
        assert!(err.contains("--dry-run"), "错误信息应给出可行做法：{err}");

        // 注入模型（`--dry-run`）→ 允许（不联网也不需要 provider）
        assert!(check_provider_available(&config, true).is_ok());

        // 有配置 → 允许
        config.base_url = "https://api.example.com".into();
        assert!(check_provider_available(&config, false).is_ok());
    }

    fn collect_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_files(&p, out);
            } else {
                out.push(p);
            }
        }
    }
}
