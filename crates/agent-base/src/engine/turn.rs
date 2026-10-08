//! 单轮请求与结果类型。

use crate::domain::TurnStopReason;
use crate::model::{ModelChatOptions, ProviderConfig};

/// 一轮执行的入参请求。
#[derive(Debug, Clone)]
pub struct TurnRequest {
    /// 归属的会话/线程 ID
    pub thread_id: String,
    /// 本轮可选的用户输入（如有新提示词，引擎会先记录并落库到会话）
    pub user_prompt: Option<String>,
    /// 用户可选附加的图片路径/URL
    pub images: Option<Vec<String>>,
    /// 模型供应商配置
    pub provider_config: ProviderConfig,
    /// 可选的模型调用参数覆盖（温度、思维强度等）
    pub options: Option<ModelChatOptions>,
}

impl TurnRequest {
    pub fn new(
        thread_id: impl Into<String>,
        provider_config: ProviderConfig,
    ) -> Self {
        Self {
            thread_id: thread_id.into(),
            user_prompt: None,
            images: None,
            provider_config,
            options: None,
        }
    }

    pub fn with_user_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.user_prompt = Some(prompt.into());
        self
    }

    pub fn with_images(mut self, images: Vec<String>) -> Self {
        self.images = Some(images);
        self
    }

    pub fn with_options(mut self, options: ModelChatOptions) -> Self {
        self.options = Some(options);
        self
    }
}

/// 一轮执行的结束概要（也是供外部审计和测试断言的结果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOutcome {
    pub thread_id: String,
    pub stop_reason: TurnStopReason,
    pub steps_taken: u32,
    pub total_duration_ms: u64,
}
