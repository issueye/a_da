//! INV-5: 回执结构化且必填不变量
//! status, started_at, finished_at 非 Option，duration_ms 自动派生，不可省略。

use agent_base::domain::ToolReceipt;

/// 断言回执数据结构的完整性与不可篡改时间逻辑
pub fn assert_receipt_structure_integrity(receipt: &ToolReceipt) -> Result<(), String> {
    if receipt.started_at < 0 || receipt.finished_at < 0 {
        return Err("时间戳必须非负".into());
    }
    if receipt.finished_at < receipt.started_at {
        return Err("finished_at 不得早于 started_at".into());
    }
    let duration = receipt.duration_ms();
    let expected = (receipt.finished_at - receipt.started_at) as u64;
    if duration != expected {
        return Err("派生 duration_ms 存在偏差".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::ToolStatus;

    #[test]
    fn test_inv5_receipt_structure() {
        let receipt = ToolReceipt {
            status: ToolStatus::Success,
            output: "完成".into(),
            data: None,
            details: None,
            started_at: 100,
            finished_at: 200,
        };
        assert_receipt_structure_integrity(&receipt).expect("回执结构完整");
    }
}
