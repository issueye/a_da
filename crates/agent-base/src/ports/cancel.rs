//! 取消端口。现状是 `Option<watch::Receiver<bool>>` 直接出现在循环与执行器签名里，
//! 而且**中途断链**：`run_command(..., None)` 让子进程收不到中止（见计划 §1.3）。
//!
//! 做成端口后，取消必须一路传到底：模型流、工具执行、子进程、子智能体。

pub trait CancelToken: Send + Sync {
    fn is_cancelled(&self) -> bool;
}
