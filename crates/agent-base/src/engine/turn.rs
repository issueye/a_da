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
    /// 该模型的**上下文窗口上限**（tokens，`None` = 未登记 → 不做治理）。
    ///
    /// 由组合根按当前配置注入（可运行时变化），引擎在每轮模型调用前用它做一次
    /// "上一轮真实用量是否已逼近上限"的判定，撞顶就 `ContextWindowExceeded` 提前失败。
    pub context_window: Option<u64>,
    /// 模型请求**重试次数**（首次失败后最多再试几次；`None` = 用 [`DEFAULT_MODEL_RETRY_COUNT`]）。
    ///
    /// 单独占一个字段而不是塞进 `options`：`options` 一旦被调用方填了就会**整份替换**
    /// 引擎默认值（温度等），只为了改重试次数而覆盖掉其它默认项是坑。
    pub max_retries: Option<usize>,
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
            context_window: None,
            max_retries: None,
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

    /// 登记上下文窗口上限（`None` = 不治理）。
    pub fn with_context_window(mut self, window: Option<u64>) -> Self {
        self.context_window = window;
        self
    }

    /// 登记模型请求重试次数（`None` = 用默认值）。
    pub fn with_max_retries(mut self, retries: Option<usize>) -> Self {
        self.max_retries = retries;
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
