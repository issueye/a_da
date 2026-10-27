//! 埋点关联上下文与来源标注（P0 埋点地基）。
//!
//! 为什么需要它：跨"钩子 → 插件 → 提示词 → 技能 → 工具"没有共同关联键时，
//! 诊断只能靠人按方法名与耗时猜。`TraceCtx` 是**信封级**的关联键，`Origin` 是
//! "这件事是谁产生的"来源标注；两者合起来回答三个问题——这一轮里哪个来源参与了、
//! 花了多久、结果如何。
//!
//! 三条设计约束（见 `docs/plugins/README.md` §7 "诊断与排错"）：
//! 1. 挂在**信封**（[`crate::domain::event::AgentEvent`]）上，不挂在各个 body 上：
//!    一次改动覆盖全部事件类型；
//! 2. 全部字段可缺省（[`TraceCtx::none`]）：引擎之外的事件构造点（宿主、子智能体、
//!    契约测试）不受影响；
//! 3. **只记元数据，不记内容**；`origin` 由产生方填写，不许消费方猜。

/// 事件来源：谁产生了这件事。
///
/// 这是"归因"的唯一口径——诊断面板按它过滤，门禁用它判断"某个扩展点是否真被调用过"。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Origin {
    /// 宿主/组合根（装配期、进程级动作）
    Host,
    /// 引擎自身（轮次循环、提示词组装、审批等待计时）
    Engine,
    /// 内置实现（内置工具、内置插件），`name` 取工具名或插件 id
    Builtin { name: String },
    /// 第三方/工作区插件
    Plugin { id: String },
    /// 技能
    Skill { id: String },
    /// 钩子点位
    Hook { point: String },
    /// 子智能体
    Subagent { id: String },
}

impl Default for Origin {
    fn default() -> Self {
        Self::Host
    }
}

impl Origin {
    /// 稳定的标签串（诊断过滤、日志、测试断言共用；不许各处自己拼格式）。
    pub fn label(&self) -> String {
        match self {
            Self::Host => "host".to_string(),
            Self::Engine => "engine".to_string(),
            Self::Builtin { name } => format!("builtin:{name}"),
            Self::Plugin { id } => format!("plugin:{id}"),
            Self::Skill { id } => format!("skill:{id}"),
            Self::Hook { point } => format!("hook:{point}"),
            Self::Subagent { id } => format!("subagent:{id}"),
        }
    }
}

/// 信封级的埋点关联上下文。
///
/// `turn_id` 由引擎在 `run_turn` 开头生成（每轮唯一）；`step_index` 是轮内步号
/// （一次模型调用 = 一步）；`call_id` 在工具执行期间有效。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TraceCtx {
    pub turn_id: Option<String>,
    pub step_index: u32,
    pub call_id: Option<String>,
    pub origin: Origin,
}

impl Default for TraceCtx {
    fn default() -> Self {
        Self::none()
    }
}

impl TraceCtx {
    /// 无轮次上下文（引擎之外的事件构造点用这个，与既有行为完全一致）。
    pub fn none() -> Self {
        Self {
            turn_id: None,
            step_index: 0,
            call_id: None,
            origin: Origin::Host,
        }
    }

    /// 一轮的起点：轮次 id + 产生方。
    pub fn turn(turn_id: impl Into<String>, origin: Origin) -> Self {
        Self {
            turn_id: Some(turn_id.into()),
            step_index: 0,
            call_id: None,
            origin,
        }
    }

    pub fn with_step(mut self, step_index: u32) -> Self {
        self.step_index = step_index;
        self
    }

    pub fn with_call(mut self, call_id: impl Into<String>) -> Self {
        self.call_id = Some(call_id.into());
        self
    }

    pub fn with_origin(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }

    pub fn clear_call(mut self) -> Self {
        self.call_id = None;
        self
    }
}

/// 每轮唯一的轮次 id。
///
/// 为什么不用随机数/时间戳：`agent-base` 零 IO、零系统时间（时间只能从 `Clock` 端口读），
/// 所以轮次 id 由**进程内单调计数 + 线程 id** 组成——同一进程内唯一，跨进程不冲突，
/// 且不引入任何新依赖。
pub fn make_turn_id(thread_id: &str, ordinal: u64) -> String {
    format!("{thread_id}#{ordinal}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_none_context() {
        let t = TraceCtx::default();
        assert_eq!(t, TraceCtx::none());
        assert!(t.turn_id.is_none());
        assert_eq!(t.origin, Origin::Host);
    }

    #[test]
    fn test_builders_are_chainable_and_do_not_leak_call_id() {
        let t = TraceCtx::turn("t1", Origin::Engine)
            .with_step(3)
            .with_call("c1")
            .with_origin(Origin::Builtin { name: "read_file".into() });
        assert_eq!(t.turn_id.as_deref(), Some("t1"));
        assert_eq!(t.step_index, 3);
        assert_eq!(t.call_id.as_deref(), Some("c1"));
        assert_eq!(t.origin.label(), "builtin:read_file");
        assert!(t.clear_call().call_id.is_none());
    }

    #[test]
    fn test_origin_labels_are_distinct_and_stable() {
        let all = [
            Origin::Host,
            Origin::Engine,
            Origin::Builtin { name: "x".into() },
            Origin::Plugin { id: "p".into() },
            Origin::Skill { id: "s".into() },
            Origin::Hook { point: "beforeToolCall".into() },
            Origin::Subagent { id: "a".into() },
        ];
        let labels: Vec<String> = all.iter().map(|o| o.label()).collect();
        let mut unique = labels.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), labels.len(), "来源标签必须互不混淆：{labels:?}");
        assert_eq!(Origin::Host.label(), "host");
    }

    #[test]
    fn test_turn_id_is_unique_per_ordinal() {
        assert_ne!(make_turn_id("t", 1), make_turn_id("t", 2));
        assert!(make_turn_id("t", 7).starts_with("t#"));
    }
}
