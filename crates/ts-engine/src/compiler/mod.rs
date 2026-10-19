//! TypeScript → JavaScript 转译与缓存。
//!
//! 具体实现按职责拆在 `diagnostics` / `cache` / `transpile` 三个子模块，
//! 本文件只负责模块声明与公开 API 再导出。

mod cache;
mod diagnostics;
mod transpile;

pub use cache::{
    TRANSPILE_CACHE_CAP, TranspileCacheStats, clear_transpile_cache, transpile_cache_stats,
    transpile_ts_module_cached,
};
pub use transpile::{oxc_strip_types, transpile_ts_module};

#[cfg(test)]
mod tests;
