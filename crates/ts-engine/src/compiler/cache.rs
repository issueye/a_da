use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use super::transpile::transpile_ts_module;
struct TranspileCache {
    map: HashMap<String, String>,
    order: VecDeque<String>,
    hits: u64,
    misses: u64,
}

/// 缓存容量上限（条目数）。
pub const TRANSPILE_CACHE_CAP: usize = 64;

fn cache() -> &'static Mutex<TranspileCache> {
    static CACHE: OnceLock<Mutex<TranspileCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(TranspileCache {
            map: HashMap::new(),
            order: VecDeque::new(),
            hits: 0,
            misses: 0,
        })
    })
}

/// 缓存统计（**没有它，缓存测试等于空转**：纯函数"同样输入同样输出"本来就成立，
/// 分不出"命中了"与"又算了一遍"）。同时它也是埋点数据的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranspileCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub len: usize,
    pub capacity: usize,
}

pub fn transpile_cache_stats() -> TranspileCacheStats {
    let guard = cache().lock().expect("转译缓存锁中毒");
    TranspileCacheStats {
        hits: guard.hits,
        misses: guard.misses,
        len: guard.map.len(),
        capacity: TRANSPILE_CACHE_CAP,
    }
}

/// 清空缓存（测试用：让计数可预期）。
pub fn clear_transpile_cache() {
    let mut guard = cache().lock().expect("转译缓存锁中毒");
    guard.map.clear();
    guard.order.clear();
    guard.hits = 0;
    guard.misses = 0;
}

fn cache_key(source: &str, filename: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    // 文件名参与键：转译的 `SourceType` 由扩展名决定（`.ts` / `.tsx` / `.js` 解析规则不同）
    format!(
        "{}|{}|{}",
        env!("CARGO_PKG_VERSION"),
        filename.unwrap_or("<inline>"),
        hex
    )
}

/// 带缓存的转译（热路径用这个）。
///
/// 语义与 [`transpile_ts_module`] **完全一致**——缓存只影响耗时，不影响结果。
pub fn transpile_ts_module_cached(source: &str, filename: Option<&str>) -> Result<String, String> {
    let key = cache_key(source, filename);
    {
        let mut guard = cache().lock().expect("转译缓存锁中毒");
        if let Some(hit) = guard.map.get(&key).cloned() {
            guard.hits += 1;
            return Ok(hit);
        }
        guard.misses += 1;
    }

    // 转译在锁外做：转译本身可能要几毫秒到几十毫秒，不该把别人堵在临界区外
    let produced = transpile_ts_module(source, filename)?;

    let mut guard = cache().lock().expect("转译缓存锁中毒");
    if !guard.map.contains_key(&key) {
        guard.map.insert(key.clone(), produced.clone());
        guard.order.push_back(key);
        while guard.order.len() > TRANSPILE_CACHE_CAP {
            if let Some(oldest) = guard.order.pop_front() {
                guard.map.remove(&oldest);
            }
        }
    }
    Ok(produced)
}
