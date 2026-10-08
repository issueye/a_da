//! 系统时钟：**全仓唯一**允许直接读系统时间的地方（内核只能经 `agent_base::ports::Clock`）。

use std::time::{SystemTime, UNIX_EPOCH};

use agent_base::ports::Clock;

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}
