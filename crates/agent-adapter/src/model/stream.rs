use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER};
use serde_json::Value;
use tokio::sync::{mpsc, watch};

use agent_base::model::{
    ChatCompletionMessage, ModelChatOptions, ModelProtocol, ProviderConfig, StreamDelta, TokenUsage, ToolCallInfo,
};
use agent_base::model::{ThinkFilterPart, ThinkTagFilter};

/// 异步流式调用模型并将 Delta 增量推送到 Channel
pub async fn stream_model_chat(
    config: ProviderConfig,
    messages: Vec<ChatCompletionMessage>,
    options: ModelChatOptions,
    mut abort_rx: Option<watch::Receiver<bool>>,
) -> mpsc::Receiver<StreamDelta> {
    let (tx, rx) = mpsc::channel(64);

    tokio::spawn(async move {
        if let Err(e) = run_stream(config, messages, options, tx.clone(), &mut abort_rx).await {
            let _ = tx.send(StreamDelta::Error { error: e.to_string() }).await;
        }
    });

    rx
}

async fn wait_abort_signal(abort_rx: &mut Option<watch::Receiver<bool>>) {
    if let Some(rx) = abort_rx.as_mut() {
        if *rx.borrow() {
            return;
        }
        while let Ok(_) = rx.changed().await {
            if *rx.borrow() {
                return;
            }
        }
        if *rx.borrow() {
            return;
        }
    }
    std::future::pending::<()>().await;
}

#[derive(Debug)]
enum SendOutcome {
    Success(reqwest::Response),
    Aborted,
}

async fn send_http_request_with_retry(
    client: &reqwest::Client,
    url: &str,
    headers: HeaderMap,
    body: Value,
    provider_name: &'static str,
    max_retries: usize,
    abort_rx: &mut Option<watch::Receiver<bool>>,
    tx: &mpsc::Sender<StreamDelta>,
) -> Result<SendOutcome> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let request = client.post(url).headers(headers.clone()).json(&body);

        let send_result = tokio::select! {
            res = request.send() => res,
            _ = wait_abort_signal(abort_rx) => {
                let _ = tx.send(StreamDelta::Done { stop_reason: "aborted".to_string() }).await;
                return Ok(SendOutcome::Aborted);
            }
        };

        match send_result {
            Ok(res) => {
                let status = res.status();
                if status.is_success() {
                    return Ok(SendOutcome::Success(res));
                }

                // 429 的上游常常会带 `Retry-After`：能读就按它退避，别拿固定指数退避硬撞
                let retry_after_ms = parse_retry_after_ms(res.headers());
                let err_text = res.text().await.unwrap_or_default();
                let is_retryable = status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS;

                if is_retryable && attempt <= max_retries {
                    #[cfg(not(test))]
                    let backoff_ms = 500u64 * (1 << (attempt - 1));
                    #[cfg(test)]
                    let backoff_ms = 10u64 * (1 << (attempt - 1));
                    let backoff_ms = retry_after_ms.map_or(backoff_ms, |ra| ra.max(backoff_ms));
                    tracing::warn!(
                        "{} API 返回 HTTP {}，第 {}/{} 次重试，退避 {}ms: {}",
                        provider_name, status, attempt, max_retries, backoff_ms, err_text
                    );
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(backoff_ms)) => continue,
                        _ = wait_abort_signal(abort_rx) => {
                            let _ = tx.send(StreamDelta::Done { stop_reason: "aborted".to_string() }).await;
                            return Ok(SendOutcome::Aborted);
                        }
                    }
                }

                // 失败信息要**说清重试过几次**：界面上的错误卡片直接照搬它，
                // 用户据此知道"不是没重试，是重试完确实不行了"（重试次数可在设置里调）。
                anyhow::bail!(
                    "{} API 错误 (HTTP {}){}: {}",
                    provider_name,
                    status,
                    retry_exhausted_suffix(attempt, max_retries),
                    err_text
                );
            }
            Err(e) => {
                if attempt <= max_retries {
                    #[cfg(not(test))]
                    let backoff_ms = 500u64 * (1 << (attempt - 1));
                    #[cfg(test)]
                    let backoff_ms = 10u64 * (1 << (attempt - 1));
                    tracing::warn!(
                        "发起 {} 请求失败，第 {}/{} 次重试，退避 {}ms: {}",
                        provider_name, attempt, max_retries, backoff_ms, e
                    );
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(backoff_ms)) => continue,
                        _ = wait_abort_signal(abort_rx) => {
                            let _ = tx.send(StreamDelta::Done { stop_reason: "aborted".to_string() }).await;
                            return Ok(SendOutcome::Aborted);
                        }
                    }
                }

                anyhow::bail!(
                    "发起 {} 请求失败{}: {}",
                    provider_name,
                    retry_exhausted_suffix(attempt, max_retries),
                    e
                );
            }
        }
    }
}

/// 退避耗尽后的说明后缀（`max_retries == 0` 时如实说"未重试"）。
fn retry_exhausted_suffix(attempts_made: usize, max_retries: usize) -> String {
    if max_retries == 0 {
        "（未重试）".to_string()
    } else {
        format!("（已重试 {} 次仍失败）", attempts_made - 1)
    }
}

/// 解析 429/503 响应里的 `Retry-After`（只认秒数形式；HTTP-date 形式忽略）。
fn parse_retry_after_ms(headers: &HeaderMap) -> Option<u64> {
    let raw = headers.get(RETRY_AFTER)?.to_str().ok()?;
    let secs: f64 = raw.trim().parse().ok()?;
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    // 上限 30s：上游说 1 小时也不能把界面吊在那儿
    Some(((secs * 1000.0) as u64).min(30_000))
}

async fn run_stream(
    config: ProviderConfig,
    messages: Vec<ChatCompletionMessage>,
    options: ModelChatOptions,
    tx: mpsc::Sender<StreamDelta>,
    abort_rx: &mut Option<watch::Receiver<bool>>,
) -> Result<()> {
    match config.protocol {
        ModelProtocol::Anthropic => {
            run_stream_anthropic(config, messages, options, tx, abort_rx).await
        }
        ModelProtocol::OpenAiResponses => {
            run_stream_openai_responses(config, messages, options, tx, abort_rx).await
        }
        ModelProtocol::OpenAiChat => {
            run_stream_openai_chat(config, messages, options, tx, abort_rx).await
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 图片编码（三家协议的形状各不相同，共用这里解析 data URI）
// ─────────────────────────────────────────────────────────────────────────────

/// 拆开 `data:<mime>;base64,<payload>`，返回 `(mime, base64)`。
///
/// 只认这一种形态（界面产出的就是它：`fs.read_base64` 与粘贴/拖拽都走 data URI）。
/// 非 data URI（http(s) 链接）返回 `None`，由调用方决定降级形态。
fn parse_data_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    // 非 base64 的 data URI（如 `data:text/plain,hello`）不按图片处理
    let mime = meta.strip_suffix(";base64")?;
    if mime.is_empty() || payload.is_empty() {
        return None;
    }
    Some((mime.to_string(), payload.to_string()))
}

/// OpenAI Chat Completions 的一条 `messages[]` 项。
///
/// 出参刻意与原来的 `#[derive(Serialize)]` 逐键一致（无图时）；有图才切到内容块数组。
fn openai_chat_message_value(m: ChatCompletionMessage) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("role".to_string(), Value::String(m.role));

    let images = m
        .images
        .filter(|imgs| !imgs.is_empty())
        .unwrap_or_default();
    if images.is_empty() {
        if let Some(content) = m.content {
            obj.insert("content".to_string(), Value::String(content));
        }
    } else {
        let mut parts: Vec<Value> = Vec::with_capacity(images.len() + 1);
        // 纯图片消息（正文为空）只发图片块，不发空的 text 块
        if let Some(text) = m.content.filter(|t| !t.is_empty()) {
            parts.push(serde_json::json!({ "type": "text", "text": text }));
        }
        for uri in images {
            parts.push(serde_json::json!({ "type": "image_url", "image_url": { "url": uri } }));
        }
        obj.insert("content".to_string(), Value::Array(parts));
    }

    if let Some(tcs) = m.tool_calls {
        obj.insert("tool_calls".to_string(), Value::Array(tcs));
    }
    if let Some(id) = m.tool_call_id {
        obj.insert("tool_call_id".to_string(), Value::String(id));
    }
    if let Some(rc) = m.reasoning_content {
        obj.insert("reasoning_content".to_string(), Value::String(rc));
    }
    Value::Object(obj)
}

/// Anthropic 的一个图片内容块。
///
/// base64 形态用 `source.base64`；拿不到 data URI（例如 http(s) 链接）时退回
/// `source.url`——Anthropic 两种都认，发错形态才是真的失败。
fn anthropic_image_block(uri: &str) -> Value {
    match parse_data_uri(uri) {
        Some((media_type, data)) => serde_json::json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data }
        }),
        None => serde_json::json!({
            "type": "image",
            "source": { "type": "url", "url": uri }
        }),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求准备（三家协议共用）
// ─────────────────────────────────────────────────────────────────────────────

/// 建连超时（只覆盖 TCP/TLS 握手，不覆盖读响应体）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 流式读取的**空闲**超时：多久没收到任何字节就判定为死流。
///
/// 🔴 这里刻意**不设总时限**。早先用的是
/// `reqwest::Client::builder().timeout(Duration::from_secs(120))`，而 reqwest 的
/// `timeout` 覆盖"建连 → 响应体读完"的**全过程**——SSE 是一条长连接，
/// 思考型模型一条回答跑两三分钟是常态，于是 120 秒一到就被从中间切断。
/// 真事故（2026-10-18）：`durationMs=120008`、思考流了 58k 字符后正文停在半句。
///
/// 空闲超时只掐"确实没有数据进来"的死流：只要还在吐字节，跑多久都不会被打断。
/// 运维可用 `A_DA_STREAM_IDLE_SECS` 调整（本地慢模型可能单块间隔很久）。
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// 测试用的空闲超时覆盖（秒，`0` = 不覆盖）。
///
/// 用静态量而不是 `set_var`：本仓测试串行跑，但 `set_var` 在 edition 2024 是 `unsafe`，
/// 而且环境变量会漏到同进程的其它用例里（AGENTS.md 对 `set_var` 的告诫同理）。
#[cfg(test)]
pub(crate) static TEST_IDLE_OVERRIDE_SECS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// 判定死流用的空闲超时（测试覆盖 > 环境变量 > 常量）。
fn stream_idle_timeout() -> Duration {
    #[cfg(test)]
    {
        let secs = TEST_IDLE_OVERRIDE_SECS.load(std::sync::atomic::Ordering::Relaxed);
        if secs > 0 {
            return Duration::from_secs(secs);
        }
    }
    std::env::var("A_DA_STREAM_IDLE_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map(Duration::from_secs)
        .unwrap_or(STREAM_IDLE_TIMEOUT)
}

/// 建 HTTP 客户端：代理 + 超时。
///
/// 抽出来的理由不是"省行数"，而是这两项是**运维配置**：改一次超时或代理语义就得三处同改，
/// 漏一处会变成"只有某个协议不走代理"这类极难复现的故障（用户配了代理，偏偏某一家的流断）。
fn build_http_client(config: &ProviderConfig) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT);

    if let Some(ref proxy_str) = config.proxy_url {
        let trimmed = proxy_str.trim();
        if !trimmed.is_empty() {
            if let Ok(proxy) = reqwest::Proxy::all(trimmed) {
                builder = builder.proxy(proxy);
            }
        }
    }

    Ok(builder.build()?)
}

/// 鉴权头的形态。三家协议的差异**只在这里**，其余请求头完全一致。
#[derive(Clone, Copy)]
enum AuthStyle {
    /// `Authorization: Bearer <key>`（OpenAI Chat / OpenAI Responses）
    Bearer,
    /// `x-api-key: <key>` + 固定 `anthropic-version`（Anthropic）
    AnthropicKey,
}

/// 组装请求头：`Content-Type` → 协议固定头 → 鉴权头 → 自定义 header。
///
/// `custom_headers` **最后合并**，可以覆盖上面任何一个——这是三家原有实现的共同语义，
/// 用户靠它对接自建网关（例如把鉴权换成网关自己的 scheme）。
fn build_headers(config: &ProviderConfig, auth: AuthStyle) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    if matches!(auth, AuthStyle::AnthropicKey) {
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    }

    // 空 key 不发鉴权头：让服务端回 401，而不是发一个 `Bearer `（尾部空格）去撞网关的怪报错
    let key = config.api_key.trim();
    if !key.is_empty() {
        match auth {
            AuthStyle::Bearer => {
                if let Ok(val) = HeaderValue::from_str(&format!("Bearer {key}")) {
                    headers.insert(AUTHORIZATION, val);
                }
            }
            AuthStyle::AnthropicKey => {
                if let Ok(val) = HeaderValue::from_str(key) {
                    headers.insert("x-api-key", val);
                }
            }
        }
    }

    if let Some(ref custom) = config.custom_headers {
        for (k, v) in custom {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                headers.insert(name, val);
            }
        }
    }

    headers
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. OpenAI Chat Completions 协议实现
// ─────────────────────────────────────────────────────────────────────────────
async fn run_stream_openai_chat(
    config: ProviderConfig,
    messages: Vec<ChatCompletionMessage>,
    options: ModelChatOptions,
    tx: mpsc::Sender<StreamDelta>,
    abort_rx: &mut Option<watch::Receiver<bool>>,
) -> Result<()> {
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

    // 逐条手工构造而不是 `"messages": messages`：带图片的用户消息必须发成
    // `content: [{type:"text"},{type:"image_url"}]` 的内容块数组，**没有图片时又必须是
    // 裸字符串**（数组形态在部分兼容网关/旧模型上会直接 400）。这条分叉只有这里知道。
    let wire_messages: Vec<Value> = messages.into_iter().map(openai_chat_message_value).collect();

    let mut body = serde_json::json!({
        "model": config.model,
        "messages": wire_messages,
        "stream": true,
        "stream_options": { "include_usage": true }
    });

    if let Some(mot) = config.max_output_tokens {
        body["max_tokens"] = serde_json::json!(mot);
    }

    if let Some(tools) = options.tools {
        if !tools.is_empty() {
            body["tools"] = serde_json::to_value(tools)?;
        }
    }

    if let Some(temp) = options.temperature {
        body["temperature"] = serde_json::json!(temp);
    }

    if let Some(effort) = options.effort {
        body["reasoning_effort"] = serde_json::json!(effort);
    }

    let client = build_http_client(&config)?;

    let headers = build_headers(&config, AuthStyle::Bearer);

    let max_retries = options.max_retries.unwrap_or(3);
    let response = match send_http_request_with_retry(
        &client,
        &url,
        headers,
        body,
        "OpenAI",
        max_retries,
        abort_rx,
        &tx,
    )
    .await?
    {
        SendOutcome::Success(res) => res,
        SendOutcome::Aborted => return Ok(()),
    };

    let mut byte_stream = response.bytes_stream();
    let mut line_buffer = String::new();
    let mut think_filter = ThinkTagFilter::new();
    let mut active_tool_calls: HashMap<usize, ToolCallInfo> = HashMap::new();
    let mut final_stop_reason = "stop".to_string();
    let idle = stream_idle_timeout();
    // 是否见过**终止信号**（`[DONE]` / `finish_reason`）。只有它出现，EOF 才算正常收尾。
    let mut saw_terminator = false;

    'stream_loop: loop {
        tokio::select! {
            chunk_opt = tokio::time::timeout(idle, byte_stream.next()) => {
                let chunk_opt = match chunk_opt {
                    // 空闲超时：不是"总时限"，只掐确实不再吐字节的死流
                    Err(_) => anyhow::bail!(
                        "流式响应 {} 秒内没有新数据（判定为死流），本轮中止",
                        idle.as_secs()
                    ),
                    Ok(v) => v,
                };
                match chunk_opt {
                    Some(Ok(bytes)) => {
                        let text = String::from_utf8_lossy(&bytes);
                        line_buffer.push_str(&text);

                        while let Some(pos) = line_buffer.find('\n') {
                            let line = line_buffer[..pos].trim().to_string();
                            line_buffer = line_buffer[pos + 1..].to_string();

                            if line.is_empty() {
                                continue;
                            }

                            if let Some(data_str) = line.strip_prefix("data:") {
                                let trimmed_data = data_str.trim();
                                if trimmed_data == "[DONE]" {
                                    saw_terminator = true;
                                    break 'stream_loop;
                                }

                                if let Ok(val) = serde_json::from_str::<Value>(trimmed_data) {
                                    // 1. 提取 usage
                                    if let Some(usage_val) = val.get("usage") {
                                        let cached_tokens = usage_val
                                            .get("prompt_tokens_details")
                                            .and_then(|d| {
                                                d.get("cached_tokens").or_else(|| d.get("cache_read_tokens"))
                                            })
                                            .and_then(|v| v.as_u64())
                                            .or_else(|| usage_val.get("prompt_cache_hit_tokens").and_then(|v| v.as_u64()))
                                            .or_else(|| usage_val.get("cache_read_input_tokens").and_then(|v| v.as_u64()))
                                            .or_else(|| usage_val.get("cached_tokens").and_then(|v| v.as_u64()))
                                            .or_else(|| usage_val.get("prompt_cache_tokens").and_then(|v| v.as_u64()))
                                            .filter(|v| *v > 0);

                                        let usage = TokenUsage {
                                            prompt_tokens: usage_val.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                            completion_tokens: usage_val.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                            total_tokens: usage_val.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                            thinking_tokens: usage_val.get("completion_tokens_details")
                                                .and_then(|d| d.get("reasoning_tokens"))
                                                .and_then(|v| v.as_u64()),
                                            cached_tokens,
                                        };
                                        let _ = tx.send(StreamDelta::Usage { usage }).await;
                                    }

                                    // 2. 提取 choices
                                    if let Some(choices) = val.get("choices").and_then(|c| c.as_array()) {
                                        if let Some(first_choice) = choices.first() {
                                            if let Some(reason) = first_choice.get("finish_reason").and_then(|r| r.as_str()) {
                                                saw_terminator = true;
                                                if reason == "tool_calls" || reason == "function_call" {
                                                    final_stop_reason = "tool_calls".to_string();
                                                } else if reason == "length" {
                                                    final_stop_reason = "length".to_string();
                                                } else {
                                                    final_stop_reason = "stop".to_string();
                                                }
                                            }

                                            if let Some(delta) = first_choice.get("delta") {
                                                // 思考链
                                                let reasoning = delta.get("reasoning_content")
                                                    .or_else(|| delta.get("reasoning"))
                                                    .or_else(|| delta.get("thinking"))
                                                    .and_then(|r| r.as_str())
                                                    .filter(|s| !s.trim().is_empty());

                                                if let Some(thinking_str) = reasoning {
                                                    let _ = tx.send(StreamDelta::Thinking {
                                                        thinking: thinking_str.to_string(),
                                                    }).await;
                                                }

                                                // 正文
                                                if let Some(content_str) = delta.get("content").and_then(|c| c.as_str()).filter(|s| !s.is_empty()) {
                                                    let parts = think_filter.feed(content_str);
                                                    for part in parts {
                                                        match part {
                                                            ThinkFilterPart::Thinking(th) => {
                                                                if !th.trim().is_empty() {
                                                                    let _ = tx.send(StreamDelta::Thinking { thinking: th }).await;
                                                                }
                                                            }
                                                            ThinkFilterPart::Text(txt) => {
                                                                if !txt.is_empty() {
                                                                    let _ = tx.send(StreamDelta::Text { text: txt }).await;
                                                                }
                                                            }
                                                        }
                                                    }
                                                }

                                                // 工具调用
                                                if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                                                    for tc in tool_calls {
                                                        let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                                                        let entry = active_tool_calls.entry(idx).or_insert_with(|| ToolCallInfo {
                                                            id: tc.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string(),
                                                            name: String::new(),
                                                            args: String::new(),
                                                        });

                                                        if let Some(id_str) = tc.get("id").and_then(|i| i.as_str()) {
                                                            entry.id = id_str.to_string();
                                                        }
                                                        if let Some(fn_obj) = tc.get("function") {
                                                            if let Some(name_str) = fn_obj.get("name").and_then(|n| n.as_str()) {
                                                                entry.name.push_str(name_str);
                                                            }
                                                            if let Some(args_str) = fn_obj.get("arguments").and_then(|a| a.as_str()) {
                                                                entry.args.push_str(args_str);
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        anyhow::bail!("读取流式数据块错误: {}", e);
                    }
                    None => {
                        break;
                    }
                }
            }
            _ = wait_abort_signal(abort_rx) => {
                let _ = tx.send(StreamDelta::Done { stop_reason: "aborted".to_string() }).await;
                return Ok(());
            }
        }
    }

    // 🔴 EOF 不等于完成：只有见过终止信号（`[DONE]` / `finish_reason` / `message_stop`
    // / `response.completed`）才算正常收尾。早先这里默认 `final_stop_reason = "stop"`，
    // 于是一次被上游掐断的回答会被当成**正常完成**——没有错误卡、没有 Notice，
    // 界面只是停在半句话上（真事故 2026-10-18：usage 全 0、正文 0 字符、thinking 17k 字符）。
    if !saw_terminator {
        anyhow::bail!(
            "流式响应在收到终止信号前就结束了（上游或代理中断），本轮按失败处理，请重试"
        );
    }

    // 结算残余 ThinkFilter 内容
    for part in think_filter.flush() {
        match part {
            ThinkFilterPart::Thinking(th) => {
                if !th.trim().is_empty() {
                    let _ = tx.send(StreamDelta::Thinking { thinking: th }).await;
                }
            }
            ThinkFilterPart::Text(txt) => {
                if !txt.is_empty() {
                    let _ = tx.send(StreamDelta::Text { text: txt }).await;
                }
            }
        }
    }

    // 结算工具调用
    let has_tools = !active_tool_calls.is_empty();
    for (_, call) in active_tool_calls {
        if !call.name.is_empty() {
            let _ = tx.send(StreamDelta::ToolCall { call }).await;
        }
    }

    let final_reason = if has_tools {
        "tool_calls".to_string()
    } else {
        final_stop_reason
    };

    let _ = tx.send(StreamDelta::Done { stop_reason: final_reason }).await;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Anthropic Messages 协议实现
// ─────────────────────────────────────────────────────────────────────────────
async fn run_stream_anthropic(
    config: ProviderConfig,
    messages: Vec<ChatCompletionMessage>,
    options: ModelChatOptions,
    tx: mpsc::Sender<StreamDelta>,
    abort_rx: &mut Option<watch::Receiver<bool>>,
) -> Result<()> {
    let base = config.base_url.trim_end_matches('/');
    let url = if base.ends_with("/v1") {
        format!("{}/messages", base)
    } else {
        format!("{}/v1/messages", base)
    };

    let mut system_text = String::new();
    let mut anthropic_messages = Vec::new();

    for m in messages {
        if m.role == "system" {
            if let Some(content) = m.content {
                if !system_text.is_empty() {
                    system_text.push_str("\n\n");
                }
                system_text.push_str(&content);
            }
        } else if m.role == "tool" {
            let tool_use_id = m.tool_call_id.unwrap_or_else(|| "tool_call".to_string());
            let content_str = m.content.unwrap_or_default();
            anthropic_messages.push(serde_json::json!({
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": content_str
                    }
                ]
            }));
        } else if m.role == "assistant" {
            if let Some(tcs) = m.tool_calls {
                let mut content_parts = Vec::new();
                if let Some(txt) = m.content {
                    if !txt.is_empty() {
                        content_parts.push(serde_json::json!({
                            "type": "text",
                            "text": txt
                        }));
                    }
                }
                for tc in tcs {
                    let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let name = tc.get("function").and_then(|f| f.get("name")).and_then(|v| v.as_str()).unwrap_or("");
                    let args_str = tc.get("function").and_then(|f| f.get("arguments")).and_then(|v| v.as_str()).unwrap_or("{}");
                    let args_val = serde_json::from_str::<Value>(args_str).unwrap_or(serde_json::json!({}));
                    content_parts.push(serde_json::json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": args_val
                    }));
                }
                anthropic_messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": content_parts
                }));
            } else {
                anthropic_messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": m.content.unwrap_or_default()
                }));
            }
        } else {
            // 用户消息：无图时 `content` 给字符串，有图时给内容块数组（Anthropic 只在
            // 拿不到 data URI 时才退回 `source.url`，见 `anthropic_image_block`）。
            let images = m.images.filter(|imgs| !imgs.is_empty()).unwrap_or_default();
            let text = m.content.unwrap_or_default();
            if images.is_empty() {
                anthropic_messages.push(serde_json::json!({
                    "role": "user",
                    "content": text
                }));
            } else {
                let mut content_parts: Vec<Value> = Vec::with_capacity(images.len() + 1);
                if !text.is_empty() {
                    content_parts.push(serde_json::json!({ "type": "text", "text": text }));
                }
                for uri in images {
                    content_parts.push(anthropic_image_block(&uri));
                }
                anthropic_messages.push(serde_json::json!({
                    "role": "user",
                    "content": content_parts
                }));
            }
        }
    }

    let max_tokens = config.max_output_tokens.unwrap_or(8192);
    let mut body = serde_json::json!({
        "model": config.model,
        "max_tokens": max_tokens,
        "messages": anthropic_messages,
        "stream": true
    });

    if !system_text.is_empty() {
        body["system"] = serde_json::json!(system_text);
    }

    if let Some(tools) = options.tools {
        if !tools.is_empty() {
            let anthropic_tools: Vec<Value> = tools
                .into_iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.function.name,
                        "description": t.function.description,
                        "input_schema": t.function.parameters
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(anthropic_tools);
        }
    }

    if let Some(temp) = options.temperature {
        body["temperature"] = serde_json::json!(temp);
    }

    let client = build_http_client(&config)?;

    let headers = build_headers(&config, AuthStyle::AnthropicKey);

    let max_retries = options.max_retries.unwrap_or(3);
    let response = match send_http_request_with_retry(
        &client,
        &url,
        headers,
        body,
        "Anthropic",
        max_retries,
        abort_rx,
        &tx,
    )
    .await?
    {
        SendOutcome::Success(res) => res,
        SendOutcome::Aborted => return Ok(()),
    };

    let mut byte_stream = response.bytes_stream();
    let mut line_buffer = String::new();
    let mut think_filter = ThinkTagFilter::new();
    let mut active_tool_calls: HashMap<usize, ToolCallInfo> = HashMap::new();
    let mut current_block_index = 0usize;
    let mut final_stop_reason = "stop".to_string();
    let mut prompt_tokens = 0u64;
    let idle = stream_idle_timeout();
    // 是否见过终止信号（`message_stop` / `message_delta.stop_reason`）
    let mut saw_terminator = false;

    'stream_loop: loop {
        tokio::select! {
            chunk_opt = tokio::time::timeout(idle, byte_stream.next()) => {
                let chunk_opt = match chunk_opt {
                    // 空闲超时：不是"总时限"，只掐确实不再吐字节的死流
                    Err(_) => anyhow::bail!(
                        "流式响应 {} 秒内没有新数据（判定为死流），本轮中止",
                        idle.as_secs()
                    ),
                    Ok(v) => v,
                };
                match chunk_opt {
                    Some(Ok(bytes)) => {
                        let text = String::from_utf8_lossy(&bytes);
                        line_buffer.push_str(&text);

                        while let Some(pos) = line_buffer.find('\n') {
                            let line = line_buffer[..pos].trim().to_string();
                            line_buffer = line_buffer[pos + 1..].to_string();

                            if line.is_empty() {
                                continue;
                            }

                            if let Some(data_str) = line.strip_prefix("data:") {
                                let trimmed_data = data_str.trim();
                                if trimmed_data == "[DONE]" {
                                    saw_terminator = true;
                                    break 'stream_loop;
                                }

                                if let Ok(val) = serde_json::from_str::<Value>(trimmed_data) {
                                    let event_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");

                                    match event_type {
                                        "message_start" => {
                                            if let Some(msg_obj) = val.get("message") {
                                                if let Some(usage_obj) = msg_obj.get("usage") {
                                                    prompt_tokens = usage_obj.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                                }
                                            }
                                        }
                                        "content_block_start" => {
                                            current_block_index = val.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                                            if let Some(block) = val.get("content_block") {
                                                let current_block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                                if current_block_type == "tool_use" {
                                                    let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                                    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                                    active_tool_calls.insert(current_block_index, ToolCallInfo {
                                                        id,
                                                        name,
                                                        args: String::new(),
                                                    });
                                                }
                                            }
                                        }
                                        "content_block_delta" => {
                                            if let Some(delta) = val.get("delta") {
                                                let delta_type = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                                if delta_type == "text_delta" {
                                                    if let Some(txt) = delta.get("text").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                                                        let parts = think_filter.feed(txt);
                                                        for part in parts {
                                                            match part {
                                                                ThinkFilterPart::Thinking(th) => {
                                                                    if !th.trim().is_empty() {
                                                                        let _ = tx.send(StreamDelta::Thinking { thinking: th }).await;
                                                                    }
                                                                }
                                                                ThinkFilterPart::Text(t) => {
                                                                    if !t.is_empty() {
                                                                        let _ = tx.send(StreamDelta::Text { text: t }).await;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                } else if delta_type == "thinking_delta" {
                                                    if let Some(th) = delta.get("thinking").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
                                                        let _ = tx.send(StreamDelta::Thinking { thinking: th.to_string() }).await;
                                                    }
                                                } else if delta_type == "input_json_delta" {
                                                    if let Some(partial) = delta.get("partial_json").and_then(|v| v.as_str()) {
                                                        if let Some(tc) = active_tool_calls.get_mut(&current_block_index) {
                                                            tc.args.push_str(partial);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        "message_delta" => {
                                            if let Some(delta) = val.get("delta") {
                                                if let Some(sr) = delta.get("stop_reason").and_then(|v| v.as_str()) {
                                                    saw_terminator = true;
                                                    final_stop_reason = match sr {
                                                        "tool_use" => "tool_calls".to_string(),
                                                        "max_tokens" => "length".to_string(),
                                                        _ => "stop".to_string(),
                                                    };
                                                }
                                            }
                                            if let Some(usage_obj) = val.get("usage") {
                                                let completion_tokens = usage_obj.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                                let usage = TokenUsage {
                                                    prompt_tokens,
                                                    completion_tokens,
                                                    total_tokens: prompt_tokens + completion_tokens,
                                                    thinking_tokens: None,
                                                    cached_tokens: None,
                                                };
                                                let _ = tx.send(StreamDelta::Usage { usage }).await;
                                            }
                                        }
                                        "message_stop" => {
                                            saw_terminator = true;
                                            break 'stream_loop;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    Some(Err(e)) => anyhow::bail!("读取流式数据块错误: {}", e),
                    None => break,
                }
            }
            _ = wait_abort_signal(abort_rx) => {
                let _ = tx.send(StreamDelta::Done { stop_reason: "aborted".to_string() }).await;
                return Ok(());
            }
        }
    }

    // 🔴 EOF 不等于完成：只有见过终止信号（`[DONE]` / `finish_reason` / `message_stop`
    // / `response.completed`）才算正常收尾。早先这里默认 `final_stop_reason = "stop"`，
    // 于是一次被上游掐断的回答会被当成**正常完成**——没有错误卡、没有 Notice，
    // 界面只是停在半句话上（真事故 2026-10-18：usage 全 0、正文 0 字符、thinking 17k 字符）。
    if !saw_terminator {
        anyhow::bail!(
            "流式响应在收到终止信号前就结束了（上游或代理中断），本轮按失败处理，请重试"
        );
    }

    for part in think_filter.flush() {
        match part {
            ThinkFilterPart::Thinking(th) => {
                if !th.trim().is_empty() {
                    let _ = tx.send(StreamDelta::Thinking { thinking: th }).await;
                }
            }
            ThinkFilterPart::Text(txt) => {
                if !txt.is_empty() {
                    let _ = tx.send(StreamDelta::Text { text: txt }).await;
                }
            }
        }
    }

    let has_tools = !active_tool_calls.is_empty();
    for (_, call) in active_tool_calls {
        if !call.name.is_empty() {
            let _ = tx.send(StreamDelta::ToolCall { call }).await;
        }
    }

    let final_reason = if has_tools {
        "tool_calls".to_string()
    } else {
        final_stop_reason
    };

    let _ = tx.send(StreamDelta::Done { stop_reason: final_reason }).await;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. OpenAI Responses API 协议实现
// ─────────────────────────────────────────────────────────────────────────────
async fn run_stream_openai_responses(
    config: ProviderConfig,
    messages: Vec<ChatCompletionMessage>,
    options: ModelChatOptions,
    tx: mpsc::Sender<StreamDelta>,
    abort_rx: &mut Option<watch::Receiver<bool>>,
) -> Result<()> {
    let base = config.base_url.trim_end_matches('/');
    let url = if base.ends_with("/v1") {
        format!("{}/responses", base)
    } else {
        format!("{}/v1/responses", base)
    };

    let mut instructions = String::new();
    let mut input_items = Vec::new();

    for m in messages {
        if m.role == "system" {
            if let Some(c) = m.content {
                if !instructions.is_empty() {
                    instructions.push_str("\n\n");
                }
                instructions.push_str(&c);
            }
        } else if m.role == "tool" {
            input_items.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": m.tool_call_id.unwrap_or_default(),
                "output": m.content.unwrap_or_default()
            }));
        } else {
            // 图片在 Responses 协议里是 `input_image` 内容块；只有用户消息会带图，
            // 无图路径与改动前逐字一致（助手消息走这里也不受影响）。
            let images = m.images.filter(|imgs| !imgs.is_empty()).unwrap_or_default();
            let text = m.content.unwrap_or_default();
            if images.is_empty() {
                input_items.push(serde_json::json!({
                    "role": m.role,
                    "content": text
                }));
            } else {
                let mut parts: Vec<Value> = Vec::with_capacity(images.len() + 1);
                if !text.is_empty() {
                    parts.push(serde_json::json!({ "type": "input_text", "text": text }));
                }
                for uri in images {
                    parts.push(serde_json::json!({ "type": "input_image", "image_url": uri }));
                }
                input_items.push(serde_json::json!({
                    "role": m.role,
                    "content": parts
                }));
            }
        }
    }

    let mut body = serde_json::json!({
        "model": config.model,
        "input": input_items,
        "stream": true
    });

    if let Some(mot) = config.max_output_tokens {
        body["max_output_tokens"] = serde_json::json!(mot);
    }

    if !instructions.is_empty() {
        body["instructions"] = serde_json::json!(instructions);
    }

    if let Some(tools) = options.tools {
        if !tools.is_empty() {
            let resp_tools: Vec<Value> = tools
                .into_iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "name": t.function.name,
                        "description": t.function.description,
                        "parameters": t.function.parameters
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(resp_tools);
        }
    }

    if let Some(temp) = options.temperature {
        body["temperature"] = serde_json::json!(temp);
    }

    let client = build_http_client(&config)?;

    let headers = build_headers(&config, AuthStyle::Bearer);

    let max_retries = options.max_retries.unwrap_or(3);
    let response = match send_http_request_with_retry(
        &client,
        &url,
        headers,
        body,
        "OpenAI Responses",
        max_retries,
        abort_rx,
        &tx,
    )
    .await?
    {
        SendOutcome::Success(res) => res,
        SendOutcome::Aborted => return Ok(()),
    };

    let mut byte_stream = response.bytes_stream();
    let mut line_buffer = String::new();
    let mut think_filter = ThinkTagFilter::new();
    let mut active_tool_calls: HashMap<String, ToolCallInfo> = HashMap::new();
    let mut final_stop_reason = "stop".to_string();
    let idle = stream_idle_timeout();
    // 是否见过终止信号（`response.completed` / `response.incomplete`）
    let mut saw_terminator = false;

    'stream_loop: loop {
        tokio::select! {
            chunk_opt = tokio::time::timeout(idle, byte_stream.next()) => {
                let chunk_opt = match chunk_opt {
                    // 空闲超时：不是"总时限"，只掐确实不再吐字节的死流
                    Err(_) => anyhow::bail!(
                        "流式响应 {} 秒内没有新数据（判定为死流），本轮中止",
                        idle.as_secs()
                    ),
                    Ok(v) => v,
                };
                match chunk_opt {
                    Some(Ok(bytes)) => {
                        let text = String::from_utf8_lossy(&bytes);
                        line_buffer.push_str(&text);

                        while let Some(pos) = line_buffer.find('\n') {
                            let line = line_buffer[..pos].trim().to_string();
                            line_buffer = line_buffer[pos + 1..].to_string();

                            if line.is_empty() {
                                continue;
                            }

                            if let Some(data_str) = line.strip_prefix("data:") {
                                let trimmed_data = data_str.trim();
                                if trimmed_data == "[DONE]" {
                                    saw_terminator = true;
                                    break 'stream_loop;
                                }

                                if let Ok(val) = serde_json::from_str::<Value>(trimmed_data) {
                                    let event_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");

                                    match event_type {
                                        "response.output_text.delta" => {
                                            if let Some(delta) = val.get("delta").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                                                let parts = think_filter.feed(delta);
                                                for part in parts {
                                                    match part {
                                                        ThinkFilterPart::Thinking(th) => {
                                                            if !th.trim().is_empty() {
                                                                let _ = tx.send(StreamDelta::Thinking { thinking: th }).await;
                                                            }
                                                        }
                                                        ThinkFilterPart::Text(txt) => {
                                                            if !txt.is_empty() {
                                                                let _ = tx.send(StreamDelta::Text { text: txt }).await;
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        "response.reasoning_text.delta" => {
                                            if let Some(th) = val.get("delta").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
                                                let _ = tx.send(StreamDelta::Thinking { thinking: th.to_string() }).await;
                                            }
                                        }
                                        "response.output_item.added" => {
                                            if let Some(item) = val.get("item") {
                                                let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                                if item_type == "function_call" {
                                                    let call_id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                                    active_tool_calls.insert(call_id.clone(), ToolCallInfo {
                                                        id: call_id,
                                                        name,
                                                        args: String::new(),
                                                    });
                                                }
                                            }
                                        }
                                        "response.function_call_arguments.delta" => {
                                            let call_id = val.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                                            if let Some(delta) = val.get("delta").and_then(|v| v.as_str()) {
                                                if let Some(tc) = active_tool_calls.get_mut(call_id) {
                                                    tc.args.push_str(delta);
                                                }
                                            }
                                        }
                                        "response.completed" => {
                                            saw_terminator = true;
                                            if let Some(resp) = val.get("response") {
                                                if let Some(st) = resp.get("status").and_then(|v| v.as_str()) {
                                                    final_stop_reason = st.to_string();
                                                }
                                                if let Some(usage_val) = resp.get("usage") {
                                                    let usage = TokenUsage {
                                                        prompt_tokens: usage_val.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                                        completion_tokens: usage_val.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                                        total_tokens: usage_val.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                                        thinking_tokens: None,
                                                        cached_tokens: None,
                                                    };
                                                    let _ = tx.send(StreamDelta::Usage { usage }).await;
                                                }
                                            }
                                            break 'stream_loop;
                                        }
                                        "response.incomplete" => {
                                            saw_terminator = true;
                                            final_stop_reason = "length".to_string();
                                            break 'stream_loop;
                                        }
                                        "response.failed" => {
                                            let detail = val
                                                .get("response")
                                                .and_then(|r| r.get("error"))
                                                .and_then(|e| e.get("message"))
                                                .and_then(|m| m.as_str())
                                                .unwrap_or("上游未给出错误详情");
                                            anyhow::bail!("OpenAI Responses 返回 response.failed：{}", detail);
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    Some(Err(e)) => anyhow::bail!("读取流式数据块错误: {}", e),
                    None => break,
                }
            }
            _ = wait_abort_signal(abort_rx) => {
                let _ = tx.send(StreamDelta::Done { stop_reason: "aborted".to_string() }).await;
                return Ok(());
            }
        }
    }

    // 🔴 EOF 不等于完成：只有见过终止信号（`[DONE]` / `finish_reason` / `message_stop`
    // / `response.completed`）才算正常收尾。早先这里默认 `final_stop_reason = "stop"`，
    // 于是一次被上游掐断的回答会被当成**正常完成**——没有错误卡、没有 Notice，
    // 界面只是停在半句话上（真事故 2026-10-18：usage 全 0、正文 0 字符、thinking 17k 字符）。
    if !saw_terminator {
        anyhow::bail!(
            "流式响应在收到终止信号前就结束了（上游或代理中断），本轮按失败处理，请重试"
        );
    }

    for part in think_filter.flush() {
        match part {
            ThinkFilterPart::Thinking(th) => {
                if !th.trim().is_empty() {
                    let _ = tx.send(StreamDelta::Thinking { thinking: th }).await;
                }
            }
            ThinkFilterPart::Text(txt) => {
                if !txt.is_empty() {
                    let _ = tx.send(StreamDelta::Text { text: txt }).await;
                }
            }
        }
    }

    let has_tools = !active_tool_calls.is_empty();
    for (_, call) in active_tool_calls {
        if !call.name.is_empty() {
            let _ = tx.send(StreamDelta::ToolCall { call }).await;
        }
    }

    let final_reason = if has_tools {
        "tool_calls".to_string()
    } else {
        final_stop_reason
    };

    let _ = tx.send(StreamDelta::Done { stop_reason: final_reason }).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_http_5xx_retries_and_reports_final_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_url = format!("http://127.0.0.1:{}/v1/chat/completions", port);

        let request_count = Arc::new(AtomicUsize::new(0));
        let count_clone = request_count.clone();

        tokio::spawn(async move {
            loop {
                if let Ok((mut socket, _)) = listener.accept().await {
                    count_clone.fetch_add(1, Ordering::SeqCst);
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    let body = "server error detail";
                    let response = format!(
                        "HTTP/1.1 500 Internal Server Error\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                }
            }
        });

        let client = reqwest::Client::new();
        let (tx, _rx) = mpsc::channel(16);
        let mut abort_rx = None;

        let res = send_http_request_with_retry(
            &client,
            &server_url,
            HeaderMap::new(),
            serde_json::json!({}),
            "TestProvider",
            2, // 重试 2 次 -> 预期共请求 3 次
            &mut abort_rx,
            &tx,
        )
        .await;

        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(err.contains("HTTP 500"));
        assert!(err.contains("server error detail"));
        // 失败信息必须说清"重试过几次"——界面错误卡片直接展示它，用户才知道是重试耗尽
        assert!(
            err.contains("已重试 2 次仍失败"),
            "退避耗尽后应说明重试次数，实际：{err}"
        );
        assert_eq!(request_count.load(Ordering::SeqCst), 3);
    }

    /// 429 带 `Retry-After` 时按它退避（并设上限），同时仍然重试到配置的次数。
    #[tokio::test]
    async fn test_429_honours_retry_after_header() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_url = format!("http://127.0.0.1:{}/v1/chat/completions", port);

        let request_count = Arc::new(AtomicUsize::new(0));
        let count_clone = request_count.clone();

        tokio::spawn(async move {
            loop {
                if let Ok((mut socket, _)) = listener.accept().await {
                    count_clone.fetch_add(1, Ordering::SeqCst);
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    let body = r#"{"error":{"message":"Too Many Requests"}}"#;
                    let response = format!(
                        "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                }
            }
        });

        let client = reqwest::Client::new();
        let (tx, _rx) = mpsc::channel(16);
        let mut abort_rx = None;

        let res = send_http_request_with_retry(
            &client,
            &server_url,
            HeaderMap::new(),
            serde_json::json!({}),
            "TestProvider",
            1,
            &mut abort_rx,
            &tx,
        )
        .await;

        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(err.contains("HTTP 429"), "实际：{err}");
        assert!(err.contains("已重试 1 次仍失败"), "实际：{err}");
        assert_eq!(request_count.load(Ordering::SeqCst), 2, "重试 1 次 → 共 2 次请求");
    }

    /// 重试次数为 0 时**一次都不重试**，错误信息如实说"未重试"。
    #[tokio::test]
    async fn test_zero_retries_fails_immediately() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_url = format!("http://127.0.0.1:{}/v1/chat/completions", port);

        let request_count = Arc::new(AtomicUsize::new(0));
        let count_clone = request_count.clone();

        tokio::spawn(async move {
            loop {
                if let Ok((mut socket, _)) = listener.accept().await {
                    count_clone.fetch_add(1, Ordering::SeqCst);
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    let body = "boom";
                    let response = format!(
                        "HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                }
            }
        });

        let client = reqwest::Client::new();
        let (tx, _rx) = mpsc::channel(16);
        let mut abort_rx = None;

        let res = send_http_request_with_retry(
            &client,
            &server_url,
            HeaderMap::new(),
            serde_json::json!({}),
            "TestProvider",
            0,
            &mut abort_rx,
            &tx,
        )
        .await;

        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("（未重试）"));
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_http_400_fails_fast_without_retry() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_url = format!("http://127.0.0.1:{}/v1/chat/completions", port);

        let request_count = Arc::new(AtomicUsize::new(0));
        let count_clone = request_count.clone();

        tokio::spawn(async move {
            loop {
                if let Ok((mut socket, _)) = listener.accept().await {
                    count_clone.fetch_add(1, Ordering::SeqCst);
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    let body = "invalid payload";
                    let response = format!(
                        "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                }
            }
        });

        let client = reqwest::Client::new();
        let (tx, _rx) = mpsc::channel(16);
        let mut abort_rx = None;

        let res = send_http_request_with_retry(
            &client,
            &server_url,
            HeaderMap::new(),
            serde_json::json!({}),
            "TestProvider",
            3,
            &mut abort_rx,
            &tx,
        )
        .await;

        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(err.contains("HTTP 400"));
        assert!(err.contains("invalid payload"));
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
    }

    fn user_msg(content: Option<&str>, images: Option<Vec<&str>>) -> ChatCompletionMessage {
        ChatCompletionMessage {
            role: "user".to_string(),
            content: content.map(|c| c.to_string()),
            images: images.map(|v| v.into_iter().map(|s| s.to_string()).collect()),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    /// 无图片时必须保持**裸字符串** content：内容块数组在部分兼容网关/旧模型上直接 400。
    /// 这条断言防止"顺手统一成数组"把普通对话打挂。
    #[test]
    fn test_openai_message_without_images_stays_a_plain_string() {
        let v = openai_chat_message_value(user_msg(Some("你好"), None));
        assert_eq!(v["content"], Value::String("你好".to_string()));
        assert!(v.get("images").is_none(), "不得下发 images 键：{v}");

        // 空数组也按"没有图片"处理
        let v = openai_chat_message_value(user_msg(Some("你好"), Some(vec![])));
        assert_eq!(v["content"], Value::String("你好".to_string()));
    }

    /// 有图片时切成内容块数组：文本在前、图片在后。
    #[test]
    fn test_openai_message_with_images_becomes_content_parts() {
        let v = openai_chat_message_value(user_msg(
            Some("看看这张图"),
            Some(vec!["data:image/png;base64,AAA"]),
        ));
        let parts = v["content"].as_array().expect("必须是内容块数组");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "看看这张图");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,AAA");
    }

    /// 纯图片消息（正文为空）不该多发一个空的 text 块——空的 text 块会被部分上游拒绝。
    #[test]
    fn test_openai_message_with_only_images_omits_empty_text() {
        let v = openai_chat_message_value(user_msg(None, Some(vec!["data:image/png;base64,AAA"])));
        let parts = v["content"].as_array().expect("必须是内容块数组");
        assert_eq!(parts.len(), 1, "只该有图片块：{v}");
        assert_eq!(parts[0]["type"], "image_url");
    }

    /// OpenAI 侧的既有键一个都不能丢（工具调用 / 工具回执 / 思考链）。
    #[test]
    fn test_openai_message_keeps_tool_and_reasoning_keys() {
        let mut m = user_msg(Some("x"), None);
        m.role = "assistant".to_string();
        m.tool_calls = Some(vec![serde_json::json!({ "id": "call_1" })]);
        m.reasoning_content = Some("先看看".to_string());
        let v = openai_chat_message_value(m);
        assert_eq!(v["tool_calls"][0]["id"], "call_1");
        assert_eq!(v["reasoning_content"], "先看看");

        let mut t = user_msg(Some("ok"), None);
        t.role = "tool".to_string();
        t.tool_call_id = Some("call_1".to_string());
        let v = openai_chat_message_value(t);
        assert_eq!(v["tool_call_id"], "call_1");
    }

    #[test]
    fn test_parse_data_uri() {
        assert_eq!(
            parse_data_uri("data:image/png;base64,AAA"),
            Some(("image/png".to_string(), "AAA".to_string()))
        );
        assert_eq!(
            parse_data_uri("data:image/jpeg;base64,ZZZ"),
            Some(("image/jpeg".to_string(), "ZZZ".to_string()))
        );
        // 非 data URI / 非 base64 / 空载荷一律不认
        assert_eq!(parse_data_uri("https://example.com/a.png"), None);
        assert_eq!(parse_data_uri("data:text/plain,hello"), None);
        assert_eq!(parse_data_uri("data:image/png;base64,"), None);
    }

    /// Anthropic 认两种形态：base64 走 `source.base64`，拿不到 data URI 才退回 `source.url`。
    #[test]
    fn test_anthropic_image_block_forms() {
        let b64 = anthropic_image_block("data:image/png;base64,AAA");
        assert_eq!(b64["type"], "image");
        assert_eq!(b64["source"]["type"], "base64");
        assert_eq!(b64["source"]["media_type"], "image/png");
        assert_eq!(b64["source"]["data"], "AAA");

        let url = anthropic_image_block("https://example.com/a.png");
        assert_eq!(url["source"]["type"], "url");
        assert_eq!(url["source"]["url"], "https://example.com/a.png");
    }

    // ── 流收尾的两种"不告而别"（2026-10-18 真事故的两条回归） ────────────────

    /// 起一个只吐一段 SSE 文本、然后**直接关连接**的假上游。
    ///
    /// `body` 原样写进响应体，`hold_secs` > 0 时写完不关连接（用来触发空闲超时）。
    async fn spawn_sse_server(body: String, hold_secs: u64) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(body.as_bytes()).await;
                let _ = socket.flush().await;
                if hold_secs > 0 {
                    tokio::time::sleep(Duration::from_secs(hold_secs)).await;
                }
                let _ = socket.shutdown().await;
            }
        });
        format!("http://127.0.0.1:{port}/v1/chat/completions")
    }

    fn chat_config(base_url: String) -> ProviderConfig {
        ProviderConfig {
            id: "test".to_string(),
            name: "TestProvider".to_string(),
            protocol: ModelProtocol::OpenAiChat,
            base_url,
            api_key: "sk-test".to_string(),
            model: "test-model".to_string(),
            max_output_tokens: None,
            custom_headers: None,
            proxy_url: None,
        }
    }


    async fn collect(rx: &mut mpsc::Receiver<StreamDelta>) -> (Option<String>, Option<String>, bool) {
        let mut error = None;
        let mut stop = None;
        let mut saw_text = false;
        while let Some(delta) = rx.recv().await {
            match delta {
                StreamDelta::Error { error: e } => error = Some(e),
                StreamDelta::Done { stop_reason } => stop = Some(stop_reason),
                StreamDelta::Text { .. } => saw_text = true,
                _ => {}
            }
        }
        (error, stop, saw_text)
    }

    /// 上游没发 `[DONE]` 也没发 `finish_reason` 就断开 → 必须报错，**不许**当正常结束。
    ///
    /// 早先这里 EOF 直接落到 `final_stop_reason = "stop"`，界面上表现为"回答停在半句话、
    /// 没有错误卡、没有重试"，用户只能看到一次莫名其妙的"会话断开"。
    #[tokio::test]
    async fn test_stream_truncated_without_terminator_is_an_error() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"半截回答\"}}]}\n\n".to_string();
        let url = spawn_sse_server(body, 0).await;

        let mut rx = stream_model_chat(chat_config(url), vec![user_msg(Some("hi"), None)], ModelChatOptions::default(), None).await;
        let (error, stop, saw_text) = collect(&mut rx).await;

        assert!(saw_text, "半截正文应照常推到界面（不吞已收到的内容）");
        assert!(stop.is_none(), "截断的流不许报 Done，实际 stop={stop:?}");
        let err = error.expect("截断必须给出错误");
        assert!(err.contains("终止信号"), "错误要说清原因，实际：{err}");
    }

    /// `[DONE]` 之后正常结束 → 不许被新校验误判成失败。
    #[tokio::test]
    async fn test_stream_with_done_terminator_completes_normally() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"完整回答\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string();
        let url = spawn_sse_server(body, 0).await;

        let mut rx = stream_model_chat(chat_config(url), vec![user_msg(Some("hi"), None)], ModelChatOptions::default(), None).await;
        let (error, stop, _) = collect(&mut rx).await;

        assert!(error.is_none(), "正常结束不该报错：{error:?}");
        assert_eq!(stop.as_deref(), Some("stop"));
    }

    /// 上游连着头却不吐字节 → 空闲超时掐掉（而不是永远挂着）。
    ///
    /// 注意这是**空闲**超时：只要还在吐字节，跑多久都不该被打断（所以没有总时限）。
    #[tokio::test]
    async fn test_stream_idle_timeout_reports_dead_stream() {
        // 把 90s 压到 1s，免得用例真等一分半（串行跑，用完复位）
        TEST_IDLE_OVERRIDE_SECS.store(1, Ordering::SeqCst);
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"开头\"}}]}\n\n".to_string();
        let url = spawn_sse_server(body, 5).await;

        let mut rx = stream_model_chat(chat_config(url), vec![user_msg(Some("hi"), None)], ModelChatOptions::default(), None).await;
        let (error, stop, _) = collect(&mut rx).await;
        TEST_IDLE_OVERRIDE_SECS.store(0, Ordering::SeqCst);

        assert!(stop.is_none(), "死流不许报 Done，实际 stop={stop:?}");
        let err = error.expect("死流必须报错");
        assert!(err.contains("没有新数据"), "错误要说清是空闲超时，实际：{err}");
    }
}
