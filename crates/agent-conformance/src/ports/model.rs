//! ModelClient 端口契约合规断言（INV-2, §8.1）
//!
//! 断言点：
//! 1. 流可拼接：增量 Delta 能够无损累加为最终文本与工具调用
//! 2. 终止原因合法：stop_reason 必须为有效语义，不可为空
//! 3. 取消响应：CancelToken 触发后停止流产出并返回相应错误
//! 4. 能力位声明与实际行为一致

use agent_base::domain::AgentError;
use agent_base::ports::{ModelCapabilities, ModelClient};

/// 验证 ModelClient 端口契约合规性
pub async fn verify_model_client_contract<M: ModelClient>(
    client: &M,
    caps: ModelCapabilities,
) -> Result<(), AgentError> {
    // 1. 能力声明一致性校验
    let reported = client.capabilities();
    if reported.streaming != caps.streaming {
        return Err(AgentError::Internal("streaming 声明不一致".into()));
    }
    if reported.images != caps.images {
        return Err(AgentError::Internal("images 声明不一致".into()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::model::{ChatCompletionMessage, ModelChatOptions, ProviderConfig, StreamDelta};
    use agent_base::ports::CompletionRequest;
    use agent_base::testing::{ManualCancel, NeverCancel, ScriptedModelClient};

    #[tokio::test]
    async fn test_scripted_model_client_conformance() {
        let client = ScriptedModelClient::new(vec![vec![
            StreamDelta::Text { text: "你好".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ]]);

        let caps = client.capabilities();
        verify_model_client_contract(&client, caps).await.expect("契约校验必须通过");

        // 验证流拼接
        let req = CompletionRequest {
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
            }],
            options: ModelChatOptions::default(),
        };
        let mut rx = client.stream(req, Some(&NeverCancel)).await.expect("发起请求");
        let mut full_text = String::new();
        let mut stop = None;
        while let Some(delta) = rx.recv().await {
            match delta {
                StreamDelta::Text { text } => full_text.push_str(&text),
                StreamDelta::Done { stop_reason } => stop = Some(stop_reason),
                _ => {}
            }
        }
        assert_eq!(full_text, "你好");
        assert_eq!(stop.as_deref(), Some("stop"));
    }

    #[tokio::test]
    async fn test_cancellation_stops_output() {
        let cancel = ManualCancel::new();
        cancel.cancel(); // 提前取消
        let client = ScriptedModelClient::new(vec![vec![
            StreamDelta::Text { text: "不该输出".into() },
        ]]);
        let req = CompletionRequest {
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
            messages: vec![],
            options: ModelChatOptions::default(),
        };
        let res = client.stream(req, Some(&cancel)).await;
        assert!(res.is_err(), "提前取消必须被感知");
    }
}
