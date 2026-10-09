//! ModelClient 端口契约合规断言（INV-2, §8.1）
//!
//! 断言点：
//! 1. 流可拼接：增量 Delta 能够无损累加为最终文本与工具调用
//! 2. 终止原因合法：`stop_reason` 必须为有效语义，不可为空
//! 3. 取消响应：`CancelToken` 触发后停止流产出并返回相应错误
//! 4. 能力位声明与实际行为一致
//!
//! # 为什么要重写（W6-T5）
//!
//! 旧签名是 `verify_model_client_contract(client, caps)`，而测试传的是
//! `caps = client.capabilities()`——**自己和自己比**，恒真。
//! 它检查的是"客户端报告的 streaming 等于客户端报告的 streaming"，
//! 与"声明与**实际行为**一致"毫无关系。
//!
//! 现在改成：**只传客户端与一个探针请求**，由函数自己去问 `capabilities()`，
//! 然后**真的跑一次流**，用观察到的行为去对账声明。

use agent_base::domain::AgentError;
use agent_base::model::{ChatCompletionMessage, ModelChatOptions, ProviderConfig, StreamDelta};
use agent_base::ports::{CancelToken, CompletionRequest, ModelClient};
use agent_base::testing::{ManualCancel, NeverCancel};

/// 一次流式探测的观察结果。
#[derive(Debug, Default, PartialEq, Eq)]
struct StreamObservation {
    /// 拼接后的文本
    text: String,
    /// `Text`/`Thinking`/`ToolCall` 增量的条数
    deltas: usize,
    /// `Done` 给出的终止原因（没有 `Done` 则为 `None`）
    stop_reason: Option<String>,
    /// `Done` 出现的次数
    done_count: usize,
}

async fn observe_stream<M: ModelClient>(
    client: &M,
    req: CompletionRequest,
    cancel: Option<&dyn CancelToken>,
) -> Result<StreamObservation, AgentError> {
    let mut rx = client
        .stream(req, cancel)
        .await
        .map_err(|e| AgentError::Internal(format!("发起流失败：{e:?}")))?;
    let mut obs = StreamObservation::default();
    while let Some(delta) = rx.recv().await {
        match delta {
            StreamDelta::Text { text } => {
                obs.deltas += 1;
                obs.text.push_str(&text);
            }
            StreamDelta::Thinking { .. } | StreamDelta::ToolCall { .. } => obs.deltas += 1,
            StreamDelta::Done { stop_reason } => {
                obs.done_count += 1;
                obs.stop_reason = Some(stop_reason);
            }
            StreamDelta::Error { .. } | StreamDelta::Usage { .. } => {}
        }
    }
    Ok(obs)
}

/// 构造"取消探测"用的最小请求（与业务探针无关，只要求客户端接受它）。
fn cancel_probe() -> CompletionRequest {
    CompletionRequest {
        config: ProviderConfig {
            id: "cancel-probe".into(),
            name: "cancel-probe".into(),
            protocol: Default::default(),
            base_url: "http://localhost".into(),
            api_key: "k".into(),
            model: "m".into(),
            max_output_tokens: None,
            custom_headers: None,
            proxy_url: None,
        },
        messages: vec![ChatCompletionMessage {
            role: "user".into(),
            content: Some("取消探测".into()),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }],
        options: ModelChatOptions::default(),
    }
}

/// 验证 `ModelClient` 端口契约合规性（**用行为对账声明**）。
pub async fn verify_model_client_contract<M: ModelClient>(
    client: &M,
    probe: CompletionRequest,
) -> Result<(), AgentError> {
    let caps = client.capabilities();

    // 1. 流式声明与实际行为一致
    let first = observe_stream(client, probe.clone(), Some(&NeverCancel)).await?;

    if caps.streaming && first.deltas == 0 {
        return Err(AgentError::Internal(
            "声明 streaming=true，但整个流没有任何增量产出".into(),
        ));
    }
    if !caps.streaming && first.deltas > 1 {
        return Err(AgentError::Internal(format!(
            "声明 streaming=false，却产出了 {} 个增量",
            first.deltas
        )));
    }

    // 2. 终止原因合法：恰好一个 Done，且 stop_reason 非空
    if first.done_count != 1 {
        return Err(AgentError::Internal(format!(
            "一轮流必须恰好一个 Done，实际 {}",
            first.done_count
        )));
    }
    match first.stop_reason.as_deref() {
        Some(s) if !s.trim().is_empty() => {}
        other => {
            return Err(AgentError::Internal(format!(
                "stop_reason 不可为空，实际 {other:?}"
            )))
        }
    }

    // 3. 无损拼接 + 确定性：同一请求跑两次结果必须一致
    let second = observe_stream(client, probe, Some(&NeverCancel)).await?;
    if first != second {
        return Err(AgentError::Internal(format!(
            "同一请求两次结果不一致（拼接有损或非确定）：{first:?} vs {second:?}"
        )));
    }

    // 4. 取消响应：提前取消必须被感知（返回错误，或至少不产出任何增量）
    let cancel = ManualCancel::new();
    cancel.cancel();
    match observe_stream(client, cancel_probe(), Some(&cancel)).await {
        Err(_) => {}
        Ok(obs) => {
            if obs.deltas > 0 {
                return Err(AgentError::Internal(
                    "提前取消后仍然产出了增量——取消没有被感知".into(),
                ));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::testing::ScriptedModelClient;

    fn probe() -> CompletionRequest {
        CompletionRequest {
            config: ProviderConfig {
                id: "test".into(),
                name: "test".into(),
                protocol: Default::default(),
                base_url: "http://localhost".into(),
                api_key: "k".into(),
                model: "m".into(),
                max_output_tokens: None,
                custom_headers: None,
                proxy_url: None,
            },
            messages: vec![ChatCompletionMessage {
                role: "user".into(),
                content: Some("你好".into()),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
            }],
            options: ModelChatOptions::default(),
        }
    }

    #[tokio::test]
    async fn test_scripted_model_client_conformance() {
        let client = ScriptedModelClient::new(vec![
            vec![
                StreamDelta::Text { text: "你好".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
            vec![
                StreamDelta::Text { text: "你好".into() },
                StreamDelta::Done { stop_reason: "stop".into() },
            ],
        ]);

        // 注意：**不再传 `client.capabilities()`**——那会变成自己和自己比
        verify_model_client_contract(&client, probe())
            .await
            .expect("契约校验必须通过");
    }

    /// 契约**真的会红**：终止原因为空必须被抓住。
    #[tokio::test]
    async fn test_empty_stop_reason_is_rejected() {
        let client = ScriptedModelClient::new(vec![
            vec![
                StreamDelta::Text { text: "你好".into() },
                StreamDelta::Done { stop_reason: String::new() },
            ],
            vec![
                StreamDelta::Text { text: "你好".into() },
                StreamDelta::Done { stop_reason: String::new() },
            ],
        ]);
        let err = match verify_model_client_contract(&client, probe()).await {
            Ok(_) => panic!("空 stop_reason 必须被拒绝"),
            Err(e) => e,
        };
        assert!(
            format!("{err}").contains("stop_reason"),
            "错误信息应指出 stop_reason：{err}"
        );
    }

    /// 契约**真的会红**：没有 `Done` 的流必须被抓住。
    #[tokio::test]
    async fn test_missing_done_is_rejected() {
        let client = ScriptedModelClient::new(vec![
            vec![StreamDelta::Text { text: "半截".into() }],
            vec![StreamDelta::Text { text: "半截".into() }],
        ]);
        let err = match verify_model_client_contract(&client, probe()).await {
            Ok(_) => panic!("缺少 Done 的流必须被拒绝"),
            Err(e) => e,
        };
        assert!(format!("{err}").contains("Done"), "{err}");
    }
}
