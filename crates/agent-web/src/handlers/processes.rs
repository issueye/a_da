//! 进程管理浮窗的局部片段（htmx 拉取）。
//!
//! 数据来自网关自己的管理面：`gateway.status` + `gateway.listAgents`。
//! SSR 进程内后端直接调网关函数；独立运行时走 `/api/gateway/*`（带 token）。
//! 这里只读展示，不提供终止/重启——SSR 页面就跑在网关进程里，
//! "重启自己"既不可靠也不该由浏览器触发。

use axum::extract::State;
use axum::response::{Html, IntoResponse, Response};
use serde_json::Value;

use crate::handlers::AppState;
use crate::ui::{self, theme::escape};

pub async fn processes_partial(State(state): State<AppState>) -> Response {
    let status = state.agent.gateway_status().await;
    let agents = state.agent.gateway_agents().await;
    match (status, agents) {
        (Ok(status), Ok(agents)) => Html(render_processes(&status, &agents)).into_response(),
        (Err(error), _) | (_, Err(error)) => Html(format!(
            r#"<div class="ada-process__empty ada-process__error">{}<div class="ada-process__hint">网关不可达时，请先确认桌面端或网关进程仍在运行。</div></div>"#,
            escape(&format!("读取进程失败：{error}"))
        ))
        .into_response(),
    }
}

fn render_processes(status: &Value, agents: &Value) -> String {
    let pid = u64_at(status, "pid");
    let product = str_at(status, "product");
    let workspace = str_at(status, "defaultWorkspace");
    let agent_count = u64_at(status, "agentCount");
    let routable = u64_at(status, "routableCount");

    let mut out = format!(
        r#"<div class="ada-process"><div class="ada-process__hero"><div class="ada-process__hero-head"><span class="ada-process__icon">{server}</span><div class="ada-process__hero-title"><strong>ada-gateway</strong><span>网关 · {product}</span></div><span class="ada-chip ada-chip--success"><span class="ada-dot ada-dot--success"></span>运行中</span></div><dl class="ada-process__facts"><div><dt>PID</dt><dd>{pid}</dd></div><div><dt>实例</dt><dd>{agent_count}</dd></div><div><dt>可路由</dt><dd>{routable}</dd></div><div><dt>默认工作区</dt><dd title="{workspace_title}">{workspace_name}</dd></div></dl></div><div class="ada-process__section"><div class="ada-process__section-title">AGENT 实例</div>"#,
        server = ui::icon("i-server", 15),
        product = escape(&product),
        pid = if pid == 0 { "—".to_string() } else { pid.to_string() },
        agent_count = agent_count,
        routable = routable,
        workspace_title = escape(&workspace),
        workspace_name = escape(&workspace_name(&workspace)),
    );

    let rows = agents
        .get("agents")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if rows.is_empty() {
        out.push_str(
            r#"<div class="ada-process__empty">当前没有 agent 实例；发送消息时会按工作区自动拉起。</div>"#,
        );
    } else {
        for agent in rows {
            out.push_str(&render_agent_row(agent));
        }
    }

    out.push_str(
        r#"</div><div class="ada-process__hint">只读视图：实例由网关按工作区按需拉起，空闲会自动回收。</div></div>"#,
    );
    out
}

fn render_agent_row(agent: &Value) -> String {
    let product = str_at(agent, "product");
    let workspace = str_at(agent, "workspace");
    let endpoint = str_at(agent, "endpoint");
    let pid = agent.get("pid").and_then(Value::as_u64);
    let status = str_at(agent, "status");
    let last = relative_age(agent.get("lastActiveMs").and_then(Value::as_u64).unwrap_or(0));

    let (tone, dot, label) = match status.as_str() {
        "ready" => ("success", "success", "就绪"),
        "starting" => ("warn", "warn", "启动中"),
        "unhealthy" => ("danger", "danger", "异常"),
        _ => ("", "", "已停止"),
    };
    let chip = if tone.is_empty() {
        r#"<span class="ada-chip">已停止</span>"#.to_string()
    } else {
        format!(
            r#"<span class="ada-chip ada-chip--{tone}"><span class="ada-dot ada-dot--{dot}"></span>{label}</span>"#
        )
    };

    format!(
        r#"<div class="ada-process__row"><span class="ada-process__row-icon">{bot}</span><div class="ada-process__main"><div class="ada-process__line"><span class="ada-process__name">{product}</span>{chip}</div><div class="ada-process__meta" title="{workspace_title}"><span class="ada-process__ws">{workspace_name}</span><span>· PID {pid}</span><span>· {last}</span></div><div class="ada-process__endpoint" title="{endpoint_title}">{endpoint}</div></div></div>"#,
        bot = ui::icon("i-bot", 15),
        product = escape(&if product.is_empty() {
            "ada-coding".to_string()
        } else {
            product
        }),
        chip = chip,
        workspace_title = escape(&workspace),
        workspace_name = escape(&workspace_name(&workspace)),
        pid = pid
            .map(|value| value.to_string())
            .unwrap_or_else(|| "—".to_string()),
        last = escape(&last),
        endpoint_title = escape(&endpoint),
        endpoint = escape(&if endpoint.is_empty() { "—" } else { &endpoint }),
    )
}

fn str_at(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn u64_at(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn workspace_name(path: &str) -> String {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .next_back()
        .unwrap_or(path)
        .to_string()
}

fn relative_age(ms: u64) -> String {
    if ms == 0 {
        return "—".to_string();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0);
    let delta = now.saturating_sub(ms);
    if delta < 60_000 {
        "刚刚".to_string()
    } else if delta < 3_600_000 {
        format!("{} 分钟前", delta / 60_000)
    } else if delta < 86_400_000 {
        format!("{} 小时前", delta / 3_600_000)
    } else {
        format!("{} 天前", delta / 86_400_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_gateway_and_agent_rows_without_tokens() {
        let status = json!({
            "pid": 4321,
            "product": "ada-coding",
            "defaultWorkspace": "E:/codes/demo",
            "agentCount": 1,
            "routableCount": 1
        });
        let agents = json!({
            "count": 1,
            "agents": [{
                "id": "ada-coding::abc",
                "product": "ada-coding",
                "workspace": "E:/codes/demo",
                "endpoint": "ws://127.0.0.1:5555/rpc",
                "status": "ready",
                "pid": 777,
                "startedAt": 1,
                "lastActiveMs": 1,
                "products": ["ada-coding"]
            }]
        });
        let html = render_processes(&status, &agents);
        assert!(html.contains("ada-gateway"), "{html}");
        assert!(html.contains("运行中"), "{html}");
        assert!(html.contains("demo"), "{html}");
        assert!(html.contains("ada-coding"), "{html}");
        assert!(html.contains("就绪"), "{html}");
        assert!(html.contains("PID 777"), "{html}");
        assert!(html.contains("ws://127.0.0.1:5555/rpc"), "{html}");
        // 网关给的是 redacted 端点；这里再兜一层，绝不把 token 带进页面。
        assert!(!html.contains("token="), "{html}");
    }

    #[test]
    fn empty_agent_list_has_a_readable_hint() {
        let html = render_processes(&json!({"pid": 1}), &json!({"agents": []}));
        assert!(html.contains("当前没有 agent 实例"), "{html}");
    }

    #[test]
    fn workspace_name_handles_windows_and_unix_paths() {
        assert_eq!(workspace_name("E:/codes/demo"), "demo");
        assert_eq!(workspace_name("E:\\codes\\demo\\"), "demo");
        assert_eq!(workspace_name("/home/me/project"), "project");
    }
}
