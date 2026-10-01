use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;
use tokio::sync::{mpsc, watch};

use super::think_filter::{ThinkFilterPart, ThinkTagFilter};
use super::types::{
    ChatCompletionMessage, ModelChatOptions, ProviderConfig, StreamDelta, TokenUsage, ToolCallInfo,
};

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

async fn run_stream(
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

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if !config.api_key.trim().is_empty() {
        if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", config.api_key.trim())) {
            headers.insert(AUTHORIZATION, val);
        }
    }

    let request = client.post(&url).headers(headers).json(&body);

    let response = match request.send().await {
        Ok(res) => {
            if !res.status().is_success() {
                let status = res.status();
                let err_text = res.text().await.unwrap_or_default();
                anyhow::bail!("API 错误 (HTTP {}): {}", status, err_text);
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
                                        let usage = TokenUsage {
                                            prompt_tokens: usage_val.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                            completion_tokens: usage_val.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                            total_tokens: usage_val.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                            thinking_tokens: usage_val.get("completion_tokens_details")
                                                .and_then(|d| d.get("reasoning_tokens"))
                                                .and_then(|v| v.as_u64()),
                                            cached_tokens: None,
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
                                                    .and_then(|r| r.as_str());

                                                if let Some(thinking_str) = reasoning {
                                                    let _ = tx.send(StreamDelta::Thinking {
                                                        thinking: thinking_str.to_string(),
                                                    }).await;
                                                }

                                                // 正文
                                                if let Some(content_str) = delta.get("content").and_then(|c| c.as_str()) {
                                                    let parts = think_filter.feed(content_str);
                                                    for part in parts {
                                                        match part {
                                                            ThinkFilterPart::Thinking(th) => {
                                                                let _ = tx.send(StreamDelta::Thinking { thinking: th }).await;
                                                            }
                                                            ThinkFilterPart::Text(txt) => {
                                                                let _ = tx.send(StreamDelta::Text { text: txt }).await;
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
                        // 流读取完成
                        break;
                    }
                }
            }
            _ = async {
                if let Some(rx) = abort_rx {
                    while rx.changed().await.is_ok() {
                        if *rx.borrow() {
                            return;
                        }
                    }
                }
                std::future::pending::<()>().await;
            } => {
                let _ = tx.send(StreamDelta::Done { stop_reason: "aborted".to_string() }).await;
                return Ok(());
            }
        }
    }

    // 结算残余 ThinkFilter 内容
    for part in think_filter.flush() {
        match part {
            ThinkFilterPart::Thinking(th) => {
                let _ = tx.send(StreamDelta::Thinking { thinking: th }).await;
            }
            ThinkFilterPart::Text(txt) => {
                let _ = tx.send(StreamDelta::Text { text: txt }).await;
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
