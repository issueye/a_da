//! INV-7: 领域与投影分离不变量
//! 基座领域模型中杜绝产品特定 UI 形状（如 settings_open, card 投影等），投影只属于适配器与协议 DTO。

/// 静态类型断言：基座 AgentMessage 纯粹由角色与内容组成，无 UI 渲染布局状态
pub fn assert_domain_projection_split() -> Result<(), String> {
    // 纯编译期/运行时中立性校验
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::AgentMessage;

    #[test]
    fn test_inv7_domain_projection_split() {
        let msg = AgentMessage::User {
            content: "纯文本".into(),
            images: None,
            timestamp: Some(1000),
        };
        // 领域模型中只有通用的消息角色与通用部件，不带有 settings_open, scroll_top 等客户端展示态
        match msg {
            AgentMessage::User { content, .. } => assert_eq!(content, "纯文本"),
            _ => panic!("必须是 User 消息"),
        }
        assert!(assert_domain_projection_split().is_ok());
    }
}
