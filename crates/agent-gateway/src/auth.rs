//! 网关的**接入鉴权**（S7）：token、作用域、Origin 策略。
//!
//! # 为什么网关必须自己鉴权
//!
//! S5/S6 的网关只在回环上跑，注释里写着"鉴权是上游的事"。一旦要**远程接入**
//! （浏览器 / 另一台机器），这个假设就不成立了：谁能连上端口，谁就能驱动
//! 所有 agent——包括让 coding agent 改工作区里的文件。
//!
//! # 三条规则
//!
//! | 规则 | 理由 |
//! |---|---|
//! | **未配置 token = 只跑回环**；非回环地址**拒绝启动** | 失败安全：不安全配置**起不来**，而不是"起来了但没人发现" |
//! | 浏览器 Origin **默认拒绝**，只放行白名单 | WS 没有 CORS 预检，但 `Origin` 头在握手里有——默认拒绝才安全 |
//! | 未认证连接**只允许 `gateway.info`** | 发现要能用（浏览器得先知道这是不是网关），但发现**不得泄露任何秘密** |
//!
//! # 为什么 `Origin: None` 放行
//!
//! 非浏览器客户端（桌面端 Tauri、curl、测试）不发 `Origin`。
//! 把它们一并拒掉会让"回环上的桌面端"也用不了——而它们**本来就不是**跨源攻击的载体
//! （浏览器才会自动带上受害者站点的 Origin）。所以策略是：
//! **有 Origin 就必须在白名单里；没有 Origin 则不因此拒绝**（token 仍然要）。

use std::collections::BTreeMap;

/// 鉴权失败的原因——**每种都如实区分**，让客户端知道该改什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// 需要 token 但没带
    MissingToken,
    /// token 不认识
    BadToken,
    /// 浏览器来源不在白名单
    OriginNotAllowed(String),
    /// token 有效，但它的作用域不包含这个工作区
    WorkspaceNotInScope { workspace: String },
    /// 配对码不认识 / 已用过 / 已过期
    BadPairingCode,
    /// 这个网关没开配对（没配 token，或配对码没生成）
    PairingNotAvailable,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingToken => write!(f, "需要 token（在 URL 上带 `?token=…`）"),
            Self::BadToken => write!(f, "token 无效"),
            Self::OriginNotAllowed(o) => {
                write!(f, "来源 `{o}` 不在白名单里（用 --allow-origin 显式放行）")
            }
            Self::WorkspaceNotInScope { workspace } => {
                write!(f, "这个 token 不允许访问工作区 `{workspace}`")
            }
            Self::BadPairingCode => {
                write!(f, "配对码无效（可能已用过或已过期——配对码是**一次性**的）")
            }
            Self::PairingNotAvailable => {
                write!(f, "这个网关没有可用的配对码（未配置 token，或配对码已全部用掉）")
            }
        }
    }
}

impl std::error::Error for AuthError {}

/// 一个 token 及其**作用域**（允许访问的工作区）。
#[derive(Debug, Clone)]
pub struct TokenGrant {
    pub token: String,
    /// 允许的工作区；**空 = 不限制**（单租户部署的常见形态）
    pub workspaces: Vec<String>,
}

/// 配对码：**一次性 + 限时**，用来把"操作员在网关控制台看到的短码"
/// 换成"浏览器可以长期使用的 token"。
///
/// 为什么需要它：token 由部署侧注入，而**浏览器没有地方拿它**——
/// 把 token 写进网页等于公开。配对码是标准的解法：人工转抄一次短码。
#[derive(Debug)]
pub struct PairingCode {
    pub code: String,
    /// 换出来的 token 的作用域
    pub workspaces: Vec<String>,
    pub expires_at: std::time::Instant,
    pub used: bool,
}

/// 配对码默认有效期。
///
/// 10 分钟：够操作员从控制台抄到浏览器里，又不至于长期挂着等人猜。
pub const PAIRING_TTL: std::time::Duration = std::time::Duration::from_secs(600);

/// 接入鉴权配置。
///
/// `grants` 用 `std::sync::RwLock`：配对换来的 token 要**立刻生效**，
/// 而 `authorize` 是同步的（临界区不含 await）。用异步锁反而要求跨 await 持锁。
#[derive(Debug)]
pub struct AuthConfig {
    grants: std::sync::RwLock<Vec<TokenGrant>>,
    allow_origins: Vec<String>,
    /// 已发放的配对码（一次性）
    pairing: std::sync::Mutex<Vec<PairingCode>>,
}

/// 鉴权结果。
#[derive(Debug, Clone)]
pub enum AuthOutcome {
    /// **未认证**：只允许 `gateway.info`（发现用），不得触达任何 agent。
    Anonymous,
    /// 已认证，带作用域。
    Scoped(TokenScope),
}

/// 已认证 token 的作用域。
#[derive(Debug, Clone)]
pub struct TokenScope {
    workspaces: Vec<String>,
}

impl TokenScope {
    /// 这个作用域是否允许某个工作区（空作用域 = 全部允许）。
    pub fn allows(&self, workspace: &str) -> bool {
        self.workspaces.is_empty()
            || self
                .workspaces
                .iter()
                .any(|w| normalize_ws(w) == normalize_ws(workspace))
    }

    /// 作用域描述（**不含 token**，可安全进日志/发现响应）。
    pub fn describe(&self) -> serde_json::Value {
        if self.workspaces.is_empty() {
            serde_json::json!({ "scope": "all" })
        } else {
            serde_json::json!({ "scope": "workspaces", "workspaces": self.workspaces })
        }
    }
}

/// 工作区比较：大小写与分隔符归一（Windows 路径同一目录有 `\` / `/` 两种写法）。
fn normalize_ws(s: &str) -> String {
    s.trim().replace('\\', "/").to_lowercase()
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            grants: std::sync::RwLock::new(Vec::new()),
            allow_origins: Vec::new(),
            pairing: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl AuthConfig {
    /// **开放模式**：不要求 token（只允许在回环上用）。
    pub fn open() -> Self {
        Self::default()
    }

    /// 从命令行参数构造。
    ///
    /// token 语法：`<token>`（不限制工作区）或 `<token>=<ws1>|<ws2>`（限定工作区）。
    /// 同一个 token 可以出现多次来追加工作区。
    pub fn from_args(tokens: &[String], allow_origins: &[String]) -> Result<Self, String> {
        let mut grants: Vec<TokenGrant> = Vec::new();
        for entry in tokens {
            let entry = entry.trim();
            if entry.is_empty() {
                return Err("--token 不能为空".to_string());
            }
            let (token, ws_part) = match entry.split_once('=') {
                Some((t, w)) => (t.trim(), Some(w)),
                None => (entry, None),
            };
            if token.is_empty() {
                return Err(format!("--token 的 token 部分为空：`{entry}`"));
            }
            let scope: Vec<String> = ws_part
                .map(|w| {
                    w.split('|')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();

            match grants.iter_mut().find(|g| g.token == token) {
                Some(g) => g.workspaces.extend(scope),
                None => grants.push(TokenGrant {
                    token: token.to_string(),
                    workspaces: scope,
                }),
            }
        }
        Ok(Self {
            grants: std::sync::RwLock::new(grants),
            allow_origins: allow_origins
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            pairing: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// 是否要求 token。
    pub fn requires_token(&self) -> bool {
        !self.grants.read().expect("grants 锁").is_empty()
    }

    /// 发放一个**一次性**配对码（返回人可读的短码）。
    ///
    /// `workspaces` 空 = 换出来的 token 不限制工作区。
    pub fn issue_pairing_code(&self, workspaces: Vec<String>) -> String {
        // 8 位大写十六进制：够短能抄，够长不易猜（4×10^9 组合，且限时+一次性）
        let raw = uuid::Uuid::new_v4().simple().to_string();
        let code = raw[..8].to_uppercase();
        self.pairing.lock().expect("pairing 锁").push(PairingCode {
            code: code.clone(),
            workspaces,
            expires_at: std::time::Instant::now() + PAIRING_TTL,
            used: false,
        });
        code
    }

    /// 兑换配对码 → 一个**新 token**（并立刻加入可用列表）。
    ///
    /// 三种失败**如实区分**：不认识/已用过/已过期 → `BadPairingCode`；
    /// 根本没有码 → `PairingNotAvailable`。都归一成"无效"会让操作员无从排查。
    pub fn redeem_pairing_code(&self, code: &str) -> Result<(String, TokenScope), AuthError> {
        let code = code.trim().to_uppercase();
        let mut table = self.pairing.lock().expect("pairing 锁");
        if table.is_empty() {
            return Err(AuthError::PairingNotAvailable);
        }
        let now = std::time::Instant::now();
        let Some(entry) = table
            .iter_mut()
            .find(|p| p.code == code && !p.used && p.expires_at > now)
        else {
            return Err(AuthError::BadPairingCode);
        };
        entry.used = true;
        let scope = TokenScope {
            workspaces: entry.workspaces.clone(),
        };
        let token = uuid::Uuid::new_v4().simple().to_string();
        self.grants
            .write()
            .expect("grants 锁")
            .push(TokenGrant {
                token: token.clone(),
                workspaces: scope.workspaces.clone(),
            });
        Ok((token, scope))
    }

    /// 还有几个**未用过且未过期**的配对码。
    pub fn live_pairing_codes(&self) -> usize {
        let now = std::time::Instant::now();
        self.pairing
            .lock()
            .expect("pairing 锁")
            .iter()
            .filter(|p| !p.used && p.expires_at > now)
            .count()
    }

    /// 放行的浏览器来源（**不含**"无 Origin"这条隐含规则）。
    pub fn allowed_origins(&self) -> &[String] {
        &self.allow_origins
    }

    /// 鉴权。
    ///
    /// - 未配置 token → 开放模式，返回 [`AuthOutcome::Scoped`]（不限制）。
    /// - 配置了 token 但请求没带 → [`AuthOutcome::Anonymous`]（**不是错误**：
    ///   发现通道要能用，只是能力被限制到 `gateway.info`）。
    /// - 带了但不认识 → [`AuthError::BadToken`]。
    pub fn authorize(&self, token: Option<&str>) -> Result<AuthOutcome, AuthError> {
        if !self.requires_token() {
            return Ok(AuthOutcome::Scoped(TokenScope {
                workspaces: Vec::new(),
            }));
        }
        let Some(t) = token.map(str::trim).filter(|t| !t.is_empty()) else {
            return Ok(AuthOutcome::Anonymous);
        };
        let grants = self.grants.read().expect("grants 锁");
        match grants.iter().find(|g| g.token == t) {
            Some(g) => Ok(AuthOutcome::Scoped(TokenScope {
                workspaces: g.workspaces.clone(),
            })),
            None => Err(AuthError::BadToken),
        }
    }

    /// Origin 策略。
    ///
    /// 无 `Origin` → 放行（非浏览器客户端）；有 `Origin` → 必须在白名单里。
    pub fn check_origin(&self, origin: Option<&str>) -> Result<(), AuthError> {
        let Some(o) = origin.map(str::trim).filter(|o| !o.is_empty()) else {
            return Ok(());
        };
        if self.allow_origins.iter().any(|a| a == o) {
            Ok(())
        } else {
            Err(AuthError::OriginNotAllowed(o.to_string()))
        }
    }
}

/// 从 URL query 里取值（含最小百分号解码）。
///
/// 为什么不引 `url` crate：网关的依赖面刻意很小（只依赖线协议），
/// 而这里需要的只是"取一个 query 参数"。浏览器会把 Windows 路径里的
/// `:` 和 `\` 编码（`E%3A%5Cproj`），所以必须解码。
pub fn query_param(uri: &str, key: &str) -> Option<String> {
    let q = uri.split_once('?')?.1;
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return Some(percent_decode(v));
        }
    }
    None
}

/// 最小百分号解码（`%XX` → 字节；`+` **不**当空格——query 里路径的 `+` 是字面量）。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 判断一个 host 是否回环（`127.0.0.1` / `::1` / `localhost`）。
pub fn is_loopback(host: &str) -> bool {
    matches!(host.trim(), "127.0.0.1" | "::1" | "localhost" | "[::1]")
}

/// 网关对外的**能力清单**（进 `gateway.info`，也是文档口径）。
pub fn gateway_capabilities() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("info", "发现：无需 token，不含任何秘密"),
        ("listAgents", "列出被管理的 agent 实例（已剥 token）"),
        ("attach", "确保某工作区有实例（按 token 作用域）"),
        ("detach", "从注册表移除实例"),
        ("status", "网关自身状态"),
        ("delegate", "派活：驱动目标 agent 跑一轮并取回结果"),
        ("cancelDelegation", "取消进行中的派活（跨网关穿透）"),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(tokens: &[&str], origins: &[&str]) -> AuthConfig {
        AuthConfig::from_args(
            &tokens.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &origins.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .expect("配置合法")
    }

    /// 开放模式：不要求 token（只该在回环上用）。
    #[test]
    fn test_open_mode_allows_everything() {
        let c = AuthConfig::open();
        assert!(!c.requires_token());
        let out = c.authorize(None).expect("开放模式必须放行");
        assert!(matches!(out, AuthOutcome::Scoped(_)));
        assert!(c.check_origin(None).is_ok());
    }

    /// 配了 token：不带 → **Anonymous**（不是错误，发现通道要能用）。
    #[test]
    fn test_missing_token_is_anonymous_not_error() {
        let c = cfg(&["secret"], &[]);
        assert!(c.requires_token());
        let out = c.authorize(None).expect("不带 token 不是错误");
        assert!(matches!(out, AuthOutcome::Anonymous));
    }

    /// 带了不认识的 token → 明确报错（不能退化成 Anonymous：那是**静默降级**）。
    #[test]
    fn test_bad_token_is_rejected_not_downgraded() {
        let c = cfg(&["secret"], &[]);
        assert_eq!(c.authorize(Some("guess")).unwrap_err(), AuthError::BadToken);
    }

    /// 作用域：token 只能访问自己那几个工作区。
    #[test]
    fn test_token_scope_limits_workspaces() {
        let c = cfg(&["t1=E:/a|E:/b", "t2=E:/c"], &[]);
        let AuthOutcome::Scoped(s1) = c.authorize(Some("t1")).unwrap() else {
            panic!("应已认证");
        };
        assert!(s1.allows("E:/a"));
        assert!(s1.allows("e:\\A"), "同一目录的另一种写法必须算同一个工作区");
        assert!(s1.allows("E:/b"));
        assert!(!s1.allows("E:/c"), "别的租户的工作区不得放行");

        let AuthOutcome::Scoped(s2) = c.authorize(Some("t2")).unwrap() else {
            panic!("应已认证");
        };
        assert!(s2.allows("E:/c"));
        assert!(!s2.allows("E:/a"));
    }

    /// 不带工作区的 token = 不限制（单租户部署）。
    #[test]
    fn test_unscoped_token_allows_all_workspaces() {
        let c = cfg(&["t"], &[]);
        let AuthOutcome::Scoped(s) = c.authorize(Some("t")).unwrap() else {
            panic!("应已认证");
        };
        assert!(s.allows("E:/anywhere"));
    }

    /// 同一 token 多次出现 → 工作区**追加**（不是覆盖）。
    #[test]
    fn test_repeated_token_appends_workspaces() {
        let c = cfg(&["t=E:/a", "t=E:/b"], &[]);
        let AuthOutcome::Scoped(s) = c.authorize(Some("t")).unwrap() else {
            panic!("应已认证");
        };
        assert!(s.allows("E:/a") && s.allows("E:/b"));
    }

    /// Origin：无 Origin 放行（桌面端/curl）；有 Origin 必须白名单（默认拒绝）。
    #[test]
    fn test_origin_default_deny_but_no_origin_allowed() {
        let c = cfg(&["t"], &[]);
        assert!(c.check_origin(None).is_ok(), "非浏览器客户端必须能用");
        assert_eq!(
            c.check_origin(Some("https://evil.example")).unwrap_err(),
            AuthError::OriginNotAllowed("https://evil.example".to_string())
        );

        let c2 = cfg(&["t"], &["https://app.example"]);
        assert!(c2.check_origin(Some("https://app.example")).is_ok());
        assert!(c2.check_origin(Some("https://evil.example")).is_err());
        // `file://` 页面发 `Origin: null` —— 不是"没有 Origin"，必须走白名单
        assert!(c2.check_origin(Some("null")).is_err(), "Origin: null 必须默认拒绝");
    }

    /// 空 token 条目要**报错**，不能静默忽略（否则用户以为配上了）。
    #[test]
    fn test_empty_token_entry_is_an_error() {
        assert!(AuthConfig::from_args(&["".to_string()], &[]).is_err());
        assert!(AuthConfig::from_args(&["=E:/a".to_string()], &[]).is_err());
    }

    /// query 解析：浏览器会把 Windows 路径编码。
    #[test]
    fn test_query_param_and_percent_decoding() {
        assert_eq!(
            query_param("/rpc?token=abc&workspace=E%3A%5Cproj", "token"),
            Some("abc".to_string())
        );
        assert_eq!(
            query_param("/rpc?token=abc&workspace=E%3A%5Cproj", "workspace"),
            Some("E:\\proj".to_string())
        );
        assert_eq!(query_param("/rpc?token=abc", "workspace"), None);
        assert_eq!(query_param("/rpc", "token"), None);
        // 未编码的路径也要能用（桌面端可能直接拼）
        assert_eq!(
            query_param("/rpc?workspace=E:/proj", "workspace"),
            Some("E:/proj".to_string())
        );
        // 坏编码不 panic，按字面返回
        assert_eq!(query_param("/rpc?t=%ZZ", "t"), Some("%ZZ".to_string()));
    }

    #[test]
    fn test_is_loopback() {
        for h in ["127.0.0.1", "::1", "localhost", "[::1]"] {
            assert!(is_loopback(h), "{h} 应算回环");
        }
        for h in ["0.0.0.0", "192.168.1.5", "example.com"] {
            assert!(!is_loopback(h), "{h} 不该算回环");
        }
    }

    /// `describe()` 不得含 token（它能进日志与发现响应）。
    #[test]
    fn test_scope_description_never_leaks_token() {
        let c = cfg(&["supersecret=E:/a"], &[]);
        let AuthOutcome::Scoped(s) = c.authorize(Some("supersecret")).unwrap() else {
            panic!("应已认证");
        };
        let d = s.describe().to_string();
        assert!(!d.contains("supersecret"), "作用域描述泄露了 token：{d}");
        assert!(d.contains("E:/a"));
    }

    /// **过期的配对码必须失效**（限时是配对码的一半安全性）。
    #[test]
    fn test_expired_pairing_code_is_rejected() {
        let c = cfg(&["t"], &[]);
        let code = c.issue_pairing_code(Vec::new());
        assert_eq!(c.live_pairing_codes(), 1);
        // 把它手动改成过期（测试子模块可以碰私有字段）
        {
            let mut table = c.pairing.lock().expect("pairing 锁");
            table[0].expires_at =
                std::time::Instant::now() - std::time::Duration::from_secs(1);
        }
        assert_eq!(
            c.redeem_pairing_code(&code).unwrap_err(),
            AuthError::BadPairingCode,
            "过期码必须被拒"
        );
        assert_eq!(c.live_pairing_codes(), 0);
    }

    /// 根本没发过码 → 报 `PairingNotAvailable`（与"码错了"区分开，便于排查）。
    #[test]
    fn test_pairing_not_available_when_none_issued() {
        let c = AuthConfig::open();
        assert_eq!(
            c.redeem_pairing_code("ABCD1234").unwrap_err(),
            AuthError::PairingNotAvailable
        );
    }

    /// 配对换来的 token **立刻可用**，且与部署 token 互不影响。
    #[test]
    fn test_redeemed_token_works_immediately() {
        let c = cfg(&["deploy"], &[]);
        let code = c.issue_pairing_code(vec!["E:/a".to_string()]);
        let (token, scope) = c.redeem_pairing_code(&code).expect("兑换成功");
        assert_ne!(token, "deploy", "换来的必须是**新** token");
        assert!(scope.allows("E:/a"));
        let AuthOutcome::Scoped(s) = c.authorize(Some(&token)).expect("新 token 应立刻可用") else {
            panic!("应已认证");
        };
        assert!(s.allows("E:/a"));
        assert!(!s.allows("E:/b"), "作用域要跟着配对码走");
        // 部署 token 仍然有效
        assert!(c.authorize(Some("deploy")).is_ok());
    }

    /// 配对码大小写不敏感（人工抄写常全大写/小写混杂）。
    #[test]
    fn test_pairing_code_is_case_insensitive() {
        let c = cfg(&["t"], &[]);
        let code = c.issue_pairing_code(Vec::new());
        assert!(c.redeem_pairing_code(&code.to_lowercase()).is_ok());
    }
}
