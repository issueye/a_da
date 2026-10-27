//! 插件的**能力声明解析**（P4 · SP-71）。
//!
//! # 声明面：源码首部的一行指令
//!
//! ```ts
//! // @ada-needs fs,net
//! export default function (api) { /* ... */ }
//! ```
//!
//! # 为什么是"源码里的注释"而不是运行时对象
//!
//! 能力必须在**执行之前**定下来：插件要跑起来才知道它声明了什么，那就意味着先给它能力、
//! 再按声明收回——而"注入后再拦"是本仓明确否掉的形态（绕过的路子永远比拦点多，
//! 见 `ada-ts-engine` 的 `Needs` 说明）。注释能静态读，于是装载期的沙箱**从一开始**
//! 就只有它声明的那几项。
//!
//! # 口径
//!
//! - **没有指令** → 宿主地板（`fs` 开、`net` 关，见 [`ScriptNeeds::plugin_floor`]）。
//! - **有指令** → **就是**指令里列的那些，不再叠加地板（`// @ada-needs net` = 只有网络，
//!   连 fs 都没有）。这样"声明了什么"与"拿到什么"能一一对上，不需要读者做心算。
//! - **认不出的 token** 一律**不授予**，并如实报进 [`ParsedNeeds::unknown`]
//!   （装配期据此产诊断）——拼错的 `nett` 绝不能静默变成"没声明"或"声明了"。

use agent_base::ports::ScriptNeeds;

/// 指令关键字。
const DIRECTIVE: &str = "@ada-needs";
/// 只在文件首部找：指令写在第 500 行没有意义（那时插件早已有机会写别的）。
const SCAN_LINES: usize = 40;

/// 解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedNeeds {
    pub needs: ScriptNeeds,
    /// 源码里到底有没有这条指令（诊断要说清"是没声明，还是声明了空集"）
    pub declared: bool,
    /// 认不出的 token（**不授予**，逐条报出去）
    pub unknown: Vec<String>,
}

/// 从源码解析能力声明。
pub fn parse_needs(source: &str) -> ParsedNeeds {
    let line = source
        .lines()
        .take(SCAN_LINES)
        .find_map(|l| l.find(DIRECTIVE).map(|idx| l[idx + DIRECTIVE.len()..].to_string()));

    let Some(rest) = line else {
        return ParsedNeeds {
            needs: ScriptNeeds::plugin_floor(),
            declared: false,
            unknown: Vec::new(),
        };
    };

    // 行尾注释符与分隔符都当分隔符：`fs, net` / `fs net` / `fs,net */` 都能读
    let tokens: Vec<String> = rest
        .split(|c: char| c == ',' || c.is_whitespace() || c == '*' || c == '/')
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect();

    let mut needs = ScriptNeeds::none();
    let mut unknown = Vec::new();
    for t in &tokens {
        match t.as_str() {
            "fs" => { needs.fs_read = true; needs.fs_write = true; }
            "fs=read" | "fs_read" | "read" => needs.fs_read = true,
            "fs=write" | "fs_write" | "write" => needs.fs_write = true,
            "net" | "network" | "fetch" => needs.net = true,
            "sqlite" | "database" | "db" => needs.sqlite = true,
            "exec" | "child_process" | "child-process" | "shell" => needs.exec = true,
            // 显式空集：`// @ada-needs none` 比"声明一个空列表"更难读错
            "none" => {}
            other => unknown.push(other.to_string()),
        }
    }

    ParsedNeeds {
        needs,
        declared: true,
        unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 没有指令 → 宿主地板（fs 开、net 关）。**向后兼容**是硬要求：
    /// 既有的插件一个都没写声明，不能因为这一批就全变成"读不了文件"。
    #[test]
    fn test_absent_directive_falls_back_to_the_floor() {
        let p = parse_needs("export default { tools: [] }\n");
        assert_eq!(p.needs, ScriptNeeds::plugin_floor());
        assert!(!p.declared, "要能区分'没声明'与'声明了空集'");
        assert!(p.unknown.is_empty());
    }

    /// 有指令 → **就是**列出的那些（不叠加地板）：`net` 不给 fs。
    #[test]
    fn test_declared_needs_replace_the_floor() {
        let p = parse_needs("// @ada-needs net\nexport default {}");
        assert!(p.declared);
        assert!(p.needs.net, "声明了网络就要给网络");
        assert!(!p.needs.fs_read && !p.needs.fs_write, "声明里没有 fs 就不给 fs（一一对上，不做心算）");
    }

    /// 多 token 的各种写法都要能读（逗号 / 空格 / 行尾注释符）。
    #[test]
    fn test_multiple_token_separators_are_tolerated() {
        for src in [
            "// @ada-needs fs,net",
            "// @ada-needs fs net",
            "/// @ada-needs fs, net",
            "/* @ada-needs fs,net */",
        ] {
            let p = parse_needs(src);
            assert!(p.needs.fs_read && p.needs.fs_write && p.needs.net, "读不出两种能力：{src}");
        }
    }

    /// 显式空集。
    #[test]
    fn test_explicit_none_grants_nothing() {
        let p = parse_needs("// @ada-needs none\nexport default {}");
        assert!(p.declared);
        assert_eq!(p.needs, ScriptNeeds::none());
    }

    /// 认不出的 token **不授予**且如实报出来（拼错的 `nett` 绝不能静默通过）。
    #[test]
    fn test_unknown_token_is_reported_and_not_granted() {
        let p = parse_needs("// @ada-needs fs,nett\n");
        assert!(p.needs.fs_read && p.needs.fs_write);
        assert!(!p.needs.net, "拼错的 token 绝不能授予能力");
        assert_eq!(p.unknown, vec!["nett".to_string()]);
    }

    /// SQLite 是独立能力：声明了才有，且不会顺带把 fs 打开。
    #[test]
    fn test_sqlite_is_a_separate_capability() {
        let p = parse_needs("// @ada-needs sqlite\nexport default {}");
        assert!(p.declared);
        assert!(p.needs.sqlite);
        assert!(!p.needs.fs_read && !p.needs.fs_write, "sqlite 不应该顺带给 fs");
        assert!(!p.needs.net);

        let p = parse_needs("// @ada-needs fs\n");
        assert!(p.needs.fs_read && p.needs.fs_write);
        assert!(!p.needs.sqlite, "fs 不应该顺带给 sqlite");
        assert!(!p.needs.exec, "fs 不应该顺带给 exec");
    }

    /// exec 同样是独立能力：拼写错误不能静默放行。
    #[test]
    fn test_exec_is_granted_explicitly_only() {
        for token in ["exec", "child_process", "child-process", "shell"] {
            let p = parse_needs(&format!("// @ada-needs {token}\n"));
            assert!(p.needs.exec, "{token} 应该授予 exec");
            assert!(!p.needs.fs_read && !p.needs.sqlite);
        }
        let p = parse_needs("// @ada-needs exe\n");
        assert!(!p.needs.exec);
        assert_eq!(p.unknown, vec!["exe".to_string()]);
    }

    /// 指令只在前 40 行里找：写在文件深处不算（那时插件早有机会做别的）。
    #[test]
    fn test_directive_deep_in_the_file_is_ignored() {
        let mut src = String::new();
        for _ in 0..60 {
            src.push_str("// filler\n");
        }
        src.push_str("// @ada-needs net\n");
        let p = parse_needs(&src);
        assert!(!p.declared, "深处的指令不该被当成声明");
        assert!(!p.needs.net);
    }

    /// 单行里的指令后面还能跟别的字（`// @ada-needs fs -- 说明`），说明文字不当 token。
    #[test]
    fn test_trailing_prose_after_directive() {
        let p = parse_needs("// @ada-needs fs\n// 说明：为什么需要 fs\n");
        assert!(p.needs.fs_read && p.needs.fs_write);
        assert_eq!(p.unknown, Vec::<String>::new(), "下一行不算指令的一部分");
    }
}
