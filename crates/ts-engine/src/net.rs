//! 插件联网的**策略**（P4 · SP-72）：超时 + 域名白名单。
//!
//! # 为什么"给了网"还不够
//!
//! `@ada-needs net` 回答的是"这个插件能不能碰网络"。碰得到之后，还能问两个更细的问题：
//! **能碰多久**（一个慢端点能把脚本拖到超时）与**能碰哪里**（白名单之外的域名一概不通）。
//! 这两个都是**宿主**说了算的：插件自己声明"我允许访问 example.com"没有任何意义。
//!
//! # 默认值刻意保守
//!
//! - **超时 30s**：与既有行为一致（此前是硬编码），显式可配；
//! - **白名单为空 = 不限制域名**：这与"未声明即不注入"是两件事——`needs.net` 已经决定了
//!   "能不能上网"，白名单是在**已联网**的前提下进一步收窄。空表表示"宿主没有设置域名限制"
//!   （不是"什么都不许"，那和 `net: false` 重复了，只会让配置项互相打架）。
//!
//! # 匹配口径
//!
//! 精确域名 **或子域后缀**：白名单里写 `example.com`，`api.example.com` 允许、
//! `notexample.com` 拒绝（后缀匹配必须带点，否则 `evilexample.com` 会被放行——
//! 这是白名单实现最常见的错）。

use std::time::Duration;

/// 插件联网策略（宿主级）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetPolicy {
    /// 单次请求的超时。
    pub timeout: Duration,
    /// 允许访问的域名（空 = 不限域名）。
    pub allow_domains: Vec<String>,
}

impl Default for NetPolicy {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            allow_domains: Vec::new(),
        }
    }
}

impl NetPolicy {
    /// 只允许这些域名（其余一律拒）。
    pub fn allow_only(domains: &[&str]) -> Self {
        Self {
            timeout: Duration::from_secs(30),
            allow_domains: domains.iter().map(|d| d.trim().to_ascii_lowercase()).collect(),
        }
    }

    /// 从环境变量读策略（**运维层的显式开关**）：
    ///
    /// - `A_DA_PLUGIN_NET_TIMEOUT_SECS`：单次请求超时（秒），默认 30；
    /// - `A_DA_PLUGIN_NET_ALLOW`：允许的域名，逗号分隔（**空 = 不限域名**）。
    ///
    /// 为什么先用环境变量而不是配置文件：这两个值是**部署决定**（"这台机器上的插件只准访问
    /// 内网"），由拉起进程的人设置最自然；配置文件 + 界面的图形化留给后续
    /// （见 `docs/quality/unfinished-features.md`）。读不到就用默认值——**默认值本身就是安全的那一端**
    /// （超时有限、域名不额外收窄），所以"没配"不等于"没护栏"。
    pub fn from_env() -> Self {
        let mut policy = Self::default();
        if let Ok(secs) = std::env::var("A_DA_PLUGIN_NET_TIMEOUT_SECS") {
            if let Ok(n) = secs.trim().parse::<u64>() {
                policy = policy.with_timeout_secs(n);
            }
        }
        if let Ok(list) = std::env::var("A_DA_PLUGIN_NET_ALLOW") {
            policy.allow_domains = list
                .split(',')
                .map(|d| d.trim().to_ascii_lowercase())
                .filter(|d| !d.is_empty())
                .collect();
        }
        policy
    }

    /// 配超时。
    pub fn with_timeout_secs(mut self, secs: u64) -> Self {
        self.timeout = Duration::from_secs(secs.max(1));
        self
    }

    /// 这个 URL 允许访问吗？拒绝时返回**给人看的原因**（要能直接回给插件作者）。
    pub fn check_url(&self, url: &str) -> Result<(), String> {
        let host = host_of(url)
            .ok_or_else(|| format!("无法解析出主机名（只支持 http/https）：{url}"))?;
        if self.allow_domains.is_empty() {
            return Ok(());
        }
        let allowed = self.allow_domains.iter().any(|d| domain_matches(&host, d));
        if allowed {
            Ok(())
        } else {
            Err(format!(
                "域名 `{host}` 不在白名单内（允许：{}）",
                self.allow_domains.join("、")
            ))
        }
    }
}

/// 从 URL 取主机名（小写，去掉端口）。非 http/https 一律返回 `None`。
///
/// 不用 URL 解析库：这里只需要"主机名"这一件事，而引入依赖的代价比这段三十行更大。
/// 但也因此**只做保守解析**——解析不出来就拒绝（失败安全），而不是猜。
fn host_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("HTTPS://"))
        .or_else(|| url.strip_prefix("HTTP://"))?;
    // 去掉 userinfo（`user:pass@host`）
    let rest = rest.rsplit('@').next().unwrap_or(rest);
    // 取到第一个 `/`、`?`、`#` 之前
    let host_port = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .trim();
    if host_port.is_empty() {
        return None;
    }
    // 去掉端口（IPv6 的 `[::1]:8080` 也顺带处理：取 `]` 之后）
    let host = if let Some(rest_after_bracket) = host_port.strip_prefix('[') {
        match rest_after_bracket.split_once(']') {
            Some((inner, _)) => inner.to_string(),
            None => return None,
        }
    } else {
        host_port
            .rsplit_once(':')
            .map(|(h, _)| h.to_string())
            .unwrap_or_else(|| host_port.to_string())
    };
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

/// 精确匹配或**子域后缀**匹配。
///
/// 后缀必须带点（`.example.com`）——不带点的话 `evilexample.com` 会被 `example.com` 放行。
fn domain_matches(host: &str, allowed: &str) -> bool {
    let allowed = allowed.trim().trim_start_matches('.').to_ascii_lowercase();
    if allowed.is_empty() {
        return false;
    }
    host == allowed || host.ends_with(&format!(".{allowed}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空白名单 = 不限域名（但**只支持 http/https**：别的协议一律拒）。
    #[test]
    fn test_empty_allowlist_permits_http_and_https_only() {
        let p = NetPolicy::default();
        assert!(p.check_url("https://anywhere.example/x").is_ok());
        assert!(p.check_url("http://127.0.0.1:8080/y").is_ok());
        assert!(
            p.check_url("file:///etc/passwd").is_err(),
            "非 http(s) 必须拒（失败安全：解析不出来就不放行）"
        );
    }

    /// 白名单：精确 + 子域放行，**相似域名拒绝**（这是白名单实现最常见的漏洞）。
    #[test]
    fn test_allowlist_matches_subdomains_but_not_lookalikes() {
        let p = NetPolicy::allow_only(&["example.com"]);
        assert!(p.check_url("https://example.com/a").is_ok());
        assert!(p.check_url("https://api.example.com/a").is_ok(), "子域应放行");
        assert!(
            p.check_url("https://evilexample.com/a").is_err(),
            "相似域名绝不能放行（后缀匹配必须带点）"
        );
        assert!(p.check_url("https://example.com.evil.io/a").is_err());
        let err = p.check_url("https://other.org/a").expect_err("白名单之外必须拒");
        assert!(err.contains("不在白名单"), "理由要能直接给作者看：{err}");
    }

    /// 端口、userinfo、大小写、路径/查询串都不能影响主机名判定。
    #[test]
    fn test_host_extraction_ignores_noise() {
        assert_eq!(host_of("https://API.Example.COM:8443/p?q=1#f").as_deref(), Some("api.example.com"));
        assert_eq!(host_of("https://user:pw@example.com/x").as_deref(), Some("example.com"));
        assert_eq!(host_of("http://[::1]:9000/x").as_deref(), Some("::1"));
        assert_eq!(host_of("ftp://example.com"), None);
        assert_eq!(host_of("not a url"), None);
    }

    /// 超时可配，且不会被配成 0（0 在 reqwest 里是"无限等"）。
    #[test]
    fn test_timeout_is_configurable_and_never_zero() {
        assert_eq!(NetPolicy::default().timeout, Duration::from_secs(30));
        assert_eq!(NetPolicy::default().with_timeout_secs(5).timeout, Duration::from_secs(5));
        assert_eq!(
            NetPolicy::default().with_timeout_secs(0).timeout,
            Duration::from_secs(1),
            "0 必须被抬到 1 秒（否则变成无限等）"
        );
    }
}
