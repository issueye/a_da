use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
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

    let mut body = serde_json::json!({
        "model": config.model,
        "messages": messages,
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

    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(120));

    if let Some(ref proxy_str) = config.proxy_url {
        let trimmed = proxy_str.trim();
        if !trimmed.is_empty() {
            if let Ok(proxy) = reqwest::Proxy::all(trimmed) {
                builder = builder.proxy(proxy);
            }
        }
    }

    let client = builder.build()?;

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if !config.api_key.trim().is_empty() {
        if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", config.api_key.trim())) {
            headers.insert(AUTHORIZATION, val);
        }
    }
    if let Some(ref custom) = config.custom_headers {
        for (k, v) in custom {
            if let (Ok(hname), Ok(hval)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                headers.insert(hname, hval);
            }
        }
    }

    let request = client.post(&url).headers(headers).json(&body);

    let response = match tokio::select! {
        res = request.send() => res,
        _ = wait_abort_signal(abort_rx) => {
            let _ = tx.send(StreamDelta::Done { stop_reason: "aborted".to_string() }).await;
            return Ok(());
        }
    } {
        Ok(res) => {
            if !res.status().is_success() {
                let status = res.status();
                let err_text = res.text().await.unwrap_or_default();
                anyhow::bail!("OpenAI API 错误 (HTTP {}): {}", status, err_text);
            }
            res
        }
        Err(e) => anyhow::bail!("发起模型请求失败: {}", e),
    };

    let mut byte_stream = response.bytes_stream();
    let mut line_buffer = String::new();
    let mut think_filter = ThinkTagFilter::new();
    let mut active_tool_calls: HashMap<usize, ToolCallInfo> = HashMap::new();
    let mut final_stop_reason = "stop".to_string();

    'stream_loop: loop {
        tokio::select! {
            chunk_opt = byte_stream.next() => {
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
            anthropic_messages.push(serde_json::json!({
                "role": "user",
                "content": m.content.unwrap_or_default()
            }));
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

    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(120));

    if let Some(ref proxy_str) = config.proxy_url {
        let trimmed = proxy_str.trim();
        if !trimmed.is_empty() {
            if let Ok(proxy) = reqwest::Proxy::all(trimmed) {
                builder = builder.proxy(proxy);
            }
        }
    }

    let client = builder.build()?;

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    if !config.api_key.trim().is_empty() {
        if let Ok(val) = HeaderValue::from_str(config.api_key.trim()) {
            headers.insert("x-api-key", val);
        }
    }
    if let Some(ref custom) = config.custom_headers {
        for (k, v) in custom {
            if let (Ok(hname), Ok(hval)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                headers.insert(hname, hval);
            }
        }
    }

    let request = client.post(&url).headers(headers).json(&body);

    let response = match tokio::select! {
        res = request.send() => res,
        _ = wait_abort_signal(abort_rx) => {
            let _ = tx.send(StreamDelta::Done { stop_reason: "aborted".to_string() }).await;
            return Ok(());
        }
    } {
        Ok(res) => {
            if !res.status().is_success() {
                let status = res.status();
                let err_text = res.text().await.unwrap_or_default();
                anyhow::bail!("Anthropic API 错误 (HTTP {}): {}", status, err_text);
            }
            res
        }
        Err(e) => anyhow::bail!("发起 Anthropic 请求失败: {}", e),
    };

    let mut byte_stream = response.bytes_stream();
    let mut line_buffer = String::new();
    let mut think_filter = ThinkTagFilter::new();
    let mut active_tool_calls: HashMap<usize, ToolCallInfo> = HashMap::new();
    let mut current_block_index = 0usize;
    let mut final_stop_reason = "stop".to_string();
    let mut prompt_tokens = 0u64;

    'stream_loop: loop {
        tokio::select! {
            chunk_opt = byte_stream.next() => {
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
            input_items.push(serde_json::json!({
                "role": m.role,
                "content": m.content.unwrap_or_default()
            }));
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

    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(120));

    if let Some(ref proxy_str) = config.proxy_url {
        let trimmed = proxy_str.trim();
        if !trimmed.is_empty() {
            if let Ok(proxy) = reqwest::Proxy::all(trimmed) {
                builder = builder.proxy(proxy);
            }
        }
    }

    let client = builder.build()?;

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if !config.api_key.trim().is_empty() {
        if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", config.api_key.trim())) {
            headers.insert(AUTHORIZATION, val);
        }
    }
    if let Some(ref custom) = config.custom_headers {
        for (k, v) in custom {
            if let (Ok(hname), Ok(hval)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                headers.insert(hname, hval);
            }
        }
    }

    let request = client.post(&url).headers(headers).json(&body);

    let response = match tokio::select! {
        res = request.send() => res,
        _ = wait_abort_signal(abort_rx) => {
            let _ = tx.send(StreamDelta::Done { stop_reason: "aborted".to_string() }).await;
            return Ok(());
        }
    } {
        Ok(res) => {
            if !res.status().is_success() {
                let status = res.status();
                let err_text = res.text().await.unwrap_or_default();
                anyhow::bail!("OpenAI Responses API 错误 (HTTP {}): {}", status, err_text);
            }
            res
        }
        Err(e) => anyhow::bail!("发起 OpenAI Responses 请求失败: {}", e),
    };

    let mut byte_stream = response.bytes_stream();
    let mut line_buffer = String::new();
    let mut think_filter = ThinkTagFilter::new();
    let mut active_tool_calls: HashMap<String, ToolCallInfo> = HashMap::new();
    let mut final_stop_reason = "stop".to_string();

    'stream_loop: loop {
        tokio::select! {
            chunk_opt = byte_stream.next() => {
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
