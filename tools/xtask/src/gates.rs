//! W0 接线审计闸：`verify-spec` / `compat` / `verify-wiring`。
//!
//! 口径见 `docs/agent-base-wiring-plan.md` §5 W0。三条设计约束：
//!
//! 1. **故意不放进 `cargo test`**：这些闸当前**预期为红**（红例基线）。写成单测会让主门禁
//!    长期变红、掩盖其它信号；因此它们是独立的 xtask 子命令，由 W1–W6 的任务逐个转绿。
//! 2. **只做机械判定**：能靠"有没有这个模块 / 有没有这个 impl / 集合是否相等"判断的，
//!    就绝不做启发式猜测；判不了的地方**如实记到 notes 里**，不静默放过。
//! 3. **不新增名单**：能调用真源的（`standard_tool_descriptors()`）就调用真源；
//!    其余从源码文本提取，不在 xtask 里另抄一份工具/方法清单。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

// ── 基础工具 ────────────────────────────────────────────────────────────────

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask 应位于 <root>/tools/xtask")
        .to_path_buf()
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn read(root: &Path, rel_path: &str) -> anyhow::Result<String> {
    let p = root.join(rel_path);
    fs::read_to_string(&p).map_err(|e| anyhow::anyhow!("读取 {} 失败: {e}", rel(root, &p)))
}

/// 递归收集 `*.rs`（跳过 target / node_modules）。
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if matches!(name.as_str(), "target" | "node_modules" | ".git") {
                continue;
            }
            collect_rs(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn rust_sources(root: &Path, dirs: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in dirs {
        collect_rs(&root.join(d), &mut out);
    }
    out.sort();
    out
}

/// 截掉文件末尾的测试块（本仓约定：`#[cfg(test)] mod tests` 在文件末尾）。
///
/// 一个文件里出现多处 `#[cfg(test)]` 时返回 `None`——**不猜**，让调用方如实记一条 note。
fn production_prefix(src: &str) -> Option<&str> {
    let count = src.matches("#[cfg(test)]").count();
    if count == 0 {
        return Some(src);
    }
    if count > 1 {
        return None;
    }
    let idx = src.find("#[cfg(test)]").expect("count 已确认大于 0");
    Some(&src[..idx])
}

/// 从 `match <header> {` 起做花括号配平，取出整个 match 体（不含最外层花括号）。
fn match_region<'a>(src: &'a str, header: &str) -> Option<&'a str> {
    let start = src.find(header)?;
    let after = &src[start + header.len()..];
    let open_rel = after.find('{')?;
    let mut depth = 0i32;
    for (i, b) in after.as_bytes().iter().enumerate().skip(open_rel) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&after[open_rel + 1..i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// FNV-1a 64：稳定哈希，避免为 `compat` 拉入 sha2 依赖。
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// ── 报告输出 ────────────────────────────────────────────────────────────────

pub struct Report {
    pub title: &'static str,
    pub violations: Vec<String>,
    pub notes: Vec<String>,
}

impl Report {
    fn new(title: &'static str) -> Self {
        Self { title, violations: Vec::new(), notes: Vec::new() }
    }

    /// 打印报告并返回违约条数（0 = 该闸转绿）。
    pub fn print(&self) -> usize {
        println!("\n==> {} ...", self.title);
        for n in &self.notes {
            println!("  · {n}");
        }
        if self.violations.is_empty() {
            println!("  \x1b[32m✔ 无违约\x1b[0m");
        } else {
            for v in &self.violations {
                println!("  \x1b[31m✘\x1b[0m {v}");
            }
            println!(
                "  \x1b[31m{} 条违约（红例基线；处置任务见 docs/agent-base-wiring-plan.md §5）\x1b[0m",
                self.violations.len()
            );
        }
        self.violations.len()
    }
}

// ── 闸 1：verify-spec（声明 ↔ 实现） ─────────────────────────────────────────

/// 每个产品的 `agent.spec.json` 必须能解析、声明的 toolkit 必须有对应模块、
/// 声明的字段必须有**生产**消费者（不是只有测试读到）。
///
/// 记录的口径偏差：计划 W0-T1 原写"跑 `ProductBuilder::validate`"。实测
/// `CompositeToolCatalog::validate` 只遍历**已注入的工具**（`catalog.rs:102-106`），
/// 未注入时恒返回空 → 对空 catalog 跑 validate 是**空断言**。故改为
/// "解析 + toolkit 模块存在性 + 字段生产消费者"三项机械检查；等 W2-T1 有了
/// toolkit→tools 映射后再补上真实装配校验。
pub fn verify_spec() -> anyhow::Result<Report> {
    let root = workspace_root();
    let mut rep = Report::new("校验产品声明（verify-spec）");

    let products_dir = root.join("products");

    // agent-toolkit 实际提供的工具包：查**真源**（`agent_toolkit::TOOLKIT_NAMES`），
    // 不再靠"crates/agent-toolkit/src 下有没有同名目录"猜——那样 `patch` 这种
    // 声明了却没实现的工具包只能靠文件名巧合被发现。
    let available: BTreeSet<String> = agent_toolkit::TOOLKIT_NAMES
        .iter()
        .map(|s| s.to_string())
        .collect();

    let mut product_count = 0usize;
    let mut skipped: BTreeSet<String> = BTreeSet::new();
    for entry in fs::read_dir(&products_dir)?.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let spec_path = dir.join("agent.spec.json");
        if !spec_path.exists() {
            continue;
        }
        product_count += 1;
        let name = entry.file_name().to_string_lossy().to_string();
        let raw = fs::read_to_string(&spec_path)?;

        let spec: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                rep.violations.push(format!("products/{name}/agent.spec.json 解析失败: {e}"));
                continue;
            }
        };

        // (a) 声明的 toolkit 必须有对应模块
        let declared: Vec<String> = spec
            .get("toolkits")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        if declared.is_empty() {
            rep.violations.push(format!("products/{name}: 未声明任何 toolkit"));
        }
        for tk in &declared {
            if !agent_toolkit::is_known_toolkit(tk) {
                rep.violations.push(format!(
                    "products/{name}: 声明了 toolkit `{tk}`，但 agent-toolkit 没有这个工具包（已知：{}）",
                    agent_toolkit::TOOLKIT_NAMES.join(", ")
                ));
            }
        }
        // (b) 反向：有实现但没声明
        for a in &available {
            if !declared.contains(a) {
                rep.notes.push(format!(
                    "products/{name}: agent-toolkit 提供 `{a}` 工具包，但 spec 未声明"
                ));
            }
        }

        // (c) 声明字段必须有生产消费者
        for field in ["identity", "capabilities", "toolkits"] {
            if spec.get(field).is_none() {
                continue;
            }
            let consumers = production_consumers(&root, field, &mut skipped)?;
            if consumers.is_empty() {
                rep.violations.push(format!(
                    "products/{name}: 声明了 `{field}`，但没有任何生产代码读取它"
                ));
            } else {
                rep.notes.push(format!(
                    "products/{name}: `{field}` 的生产消费者 → {}",
                    consumers.join(", ")
                ));
            }
        }
    }

    for f in &skipped {
        rep.notes
            .push(format!("跳过 {f}：含多处 #[cfg(test)]，无法机械划分生产段"));
    }
    if product_count == 0 {
        rep.violations.push("products/ 下没有找到任何 agent.spec.json".to_string());
    } else {
        rep.notes.push(format!("共校验 {product_count} 个产品声明"));
    }
    Ok(rep)
}

/// 找出**生产代码**里读取 spec 某字段的位置。
///
/// 判定方式是"符号级"而不是"猜语义"：`capabilities`/`identity` 看其类型
/// `CapabilitySpec`/`IdentitySpec` 是否在 `spec.rs` 之外被**使用**；`toolkits` 看 `.toolkits`。
/// **再导出不算消费者**（`pub use spec::{CapabilitySpec}` 只是转发名字，没人真的读它）。
fn production_consumers(
    root: &Path,
    field: &str,
    skipped: &mut BTreeSet<String>,
) -> anyhow::Result<Vec<String>> {
    let needle = match field {
        "capabilities" => "CapabilitySpec",
        "identity" => "IdentitySpec",
        "toolkits" => ".toolkits",
        other => anyhow::bail!("未知字段 {other}"),
    };

    let mut hits = BTreeSet::new();
    for p in rust_sources(root, &["crates", "products", "src-tauri"]) {
        let rel_path = rel(root, &p);
        // spec 的定义处不算消费者；testing 是测试替身
        if rel_path.ends_with("agent-runtime/src/spec.rs")
            || rel_path.contains("agent-base/src/testing")
        {
            continue;
        }
        let Ok(src) = fs::read_to_string(&p) else { continue };
        let Some(prod) = production_prefix(&src) else {
            skipped.insert(rel_path);
            continue;
        };
        if uses_symbol(prod, needle) {
            hits.insert(rel_path);
        }
    }
    Ok(hits.into_iter().collect())
}

/// 生产段里是否**真的使用**了某个符号（跳过 `pub use` 再导出，含跨行块）。
fn uses_symbol(src: &str, needle: &str) -> bool {
    let mut in_reexport = false;
    for line in src.lines() {
        let t = line.trim();
        if in_reexport {
            if t.ends_with(';') {
                in_reexport = false;
            }
            continue;
        }
        if t.starts_with("pub use") {
            if !t.ends_with(';') {
                in_reexport = true;
            }
            continue;
        }
        if t.contains(needle) {
            return true;
        }
    }
    false
}

// ── 闸 2：compat（跨产品 base 协议一致） ─────────────────────────────────────

/// base 协议只有一份，且任何产品的 `*.ext.json` **不得**重新定义 base 方法。
pub fn compat() -> anyhow::Result<Report> {
    let root = workspace_root();
    let mut rep = Report::new("校验跨产品协议一致（compat）");

    let spec_dir = root.join("spec/proto");
    let mut base_files = Vec::new();
    let mut ext_files = Vec::new();
    if let Ok(rd) = fs::read_dir(&spec_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") {
                let n = e.file_name().to_string_lossy().to_string();
                if n.ends_with(".ext.json") {
                    ext_files.push(p);
                } else {
                    base_files.push(p);
                }
            }
        }
    }
    base_files.sort();
    ext_files.sort();

    if base_files.len() != 1 {
        rep.violations
            .push(format!("spec/proto 下 base 协议必须恰好一份，实际 {}", base_files.len()));
    }

    let mut base_methods: BTreeSet<String> = BTreeSet::new();
    for b in &base_files {
        let raw = fs::read_to_string(b)?;
        let hash = fnv1a64(raw.as_bytes());
        rep.notes.push(format!("base 协议 {} 哈希 fnv1a64={hash:016x}", rel(&root, b)));
        for m in spec_method_names(&raw)? {
            base_methods.insert(m);
        }
    }

    for e in &ext_files {
        let raw = fs::read_to_string(e)?;
        let ext = spec_method_names(&raw)?;
        let overlap: Vec<&String> = ext.iter().filter(|m| base_methods.contains(*m)).collect();
        for m in &overlap {
            rep.violations.push(format!(
                "{}: 扩展重新定义了 base 方法 `{m}`（base ∪ ext 必须无交集）",
                rel(&root, e)
            ));
        }
        rep.notes.push(format!(
            "{}: {} 个扩展方法",
            rel(&root, e),
            ext.len()
        ));
    }

    rep.notes.push(format!("base 共 {} 个方法", base_methods.len()));
    Ok(rep)
}

/// 从 spec JSON 里取 `methods[].name`。
fn spec_method_names(raw: &str) -> anyhow::Result<BTreeSet<String>> {
    let v: serde_json::Value = serde_json::from_str(raw)?;
    let mut out = BTreeSet::new();
    if let Some(arr) = v.get("methods").and_then(|x| x.as_array()) {
        for item in arr {
            if let Some(n) = item.get("name").and_then(|x| x.as_str()) {
                out.insert(n.to_string());
            }
        }
    }
    Ok(out)
}

// ── 闸 3：verify-wiring（接线结构审计） ──────────────────────────────────────

/// 四项结构审计，对应计划 W0-T3/T4/T5/T6：
/// A. `ALL_METHODS` 每个方法都有 dispatch 臂；
/// B. `PLUGIN_BUILTIN_CATALOG` 与注册表**同一真源**；
/// C. 每个 `ToolDescriptor` 都有可达执行路径；
/// D. 每个端口都有**生产**实现（非 testing double）。
pub fn verify_wiring() -> anyhow::Result<Report> {
    let root = workspace_root();
    let mut rep = Report::new("接线结构审计（verify-wiring）");

    check_dispatch_arms(&root, &mut rep)?;
    check_builtin_catalog(&root, &mut rep)?;
    check_descriptor_reachability(&root, &mut rep)?;
    check_port_impls(&root, &mut rep)?;
    check_fail_direction_is_consumed(&root, &mut rep)?;
    check_frontend_reconnect(&root, &mut rep)?;
    check_event_emitters(&root, &mut rep)?;

    Ok(rep)
}

/// A. 协议方法覆盖性：
/// - 缺失臂 = `ALL_METHODS` 里有、`dispatch.rs` 的 `match method` 里没有；
/// - 孤儿臂 = match 臂引用了不是协议常量的标识符。
fn check_dispatch_arms(root: &Path, rep: &mut Report) -> anyhow::Result<()> {
    let methods_src = read(root, "crates/agent-proto/src/methods.rs")?;
    let consts = parse_proto_consts(&methods_src);
    let all_methods = parse_all_methods(&methods_src);
    let dispatch_src = read(root, "crates/agent-core/src/server/dispatch.rs")?;

    let Some(region) = match_region(&dispatch_src, "match method") else {
        rep.violations
            .push("dispatch.rs 里找不到 `match method {`，无法审计方法覆盖性".to_string());
        return Ok(());
    };
    let arms = parse_arms(region);

    if all_methods.is_empty() {
        rep.violations
            .push("methods.rs 里解析不出 ALL_METHODS，审计无效".to_string());
        return Ok(());
    }

    let missing: Vec<&String> = all_methods.iter().filter(|n| !arms.contains(*n)).collect();
    for n in &missing {
        let value = consts.get(*n).map(String::as_str).unwrap_or("<未知常量>");
        rep.violations.push(format!(
            "协议方法 `{value}`（常量 {n}）在 ALL_METHODS 里，但 dispatch.rs 没有对应的 match 臂"
        ));
    }
    let orphans: Vec<&String> = arms.iter().filter(|n| !consts.contains_key(*n)).collect();
    for n in &orphans {
        rep.violations
            .push(format!("dispatch.rs 有 match 臂 `{n} =>`，但它不是 agent-proto 的方法常量（孤儿臂）"));
    }
    rep.notes.push(format!(
        "协议方法 {} 个，match 臂 {} 个，缺失 {}、孤儿 {}",
        all_methods.len(),
        arms.len(),
        missing.len(),
        orphans.len()
    ));
    Ok(())
}

/// B. `PLUGIN_BUILTIN_CATALOG` 必须**由注册表派生**，不得内联工具名清单。
///
/// 口径演进（W2-T5）：升级前这里解析臂里的 `"name": "x"` 字面量与注册表比集合，
/// 于是"把清单改成派生"之后反而会报"漏了 23 个"。正确的不变量是
/// **这一段永远是派生的**，因此现在断言两件事：
/// 1. 臂体引用了 `standard_tool_descriptors()`；
/// 2. 臂体里**没有任何** `"name":` 字面量（有就说明又抄了一张清单）。
///
/// 行为侧的逐名相等断言在 `agent-core` 的
/// `test_builtin_catalog_matches_registry_exactly`（那里能真的调 dispatch）。
fn check_builtin_catalog(root: &Path, rep: &mut Report) -> anyhow::Result<()> {
    let dispatch_src = read(root, "crates/agent-core/src/server/dispatch.rs")?;
    let Some(region) = match_region(&dispatch_src, "PLUGIN_BUILTIN_CATALOG =>") else {
        rep.violations.push(
            "dispatch.rs 里找不到 `PLUGIN_BUILTIN_CATALOG =>` 臂，无法审计内置工具目录".to_string(),
        );
        return Ok(());
    };

    if !region.contains("standard_tool_descriptors()") {
        rep.violations.push(
            "PLUGIN_BUILTIN_CATALOG 臂没有引用 `standard_tool_descriptors()`——目录必须是注册表的投影"
                .to_string(),
        );
    }

    let inlined = extract_json_names(region);
    for n in &inlined {
        rep.violations.push(format!(
            "PLUGIN_BUILTIN_CATALOG 臂内联了工具名字面量 `{n}`——那就是第 6 张名单，必须改为派生"
        ));
    }

    let registry_count = agent_toolkit::registry::standard_tool_descriptors().len();
    rep.notes.push(format!(
        "内置工具目录：派生自注册表（{registry_count} 个描述符），内联字面量 {} 个",
        inlined.len()
    ));
    Ok(())
}

/// C. 每个注册表描述符都要有可达执行路径。
///
/// "可达"= 以下任一：
/// 1. **由某个工具包工厂提供 `Tool` 实例**（`catalog` 装配即可达）；
/// 2. **由 core 侧提供 `Tool` 实现**（宿主耦合工具：`impl Tool for` 所在文件里出现该名字）。
///    例：`invoke_subagent` 需要 `SubagentManager` / 父 provider / 检查点，
///    工具包工厂（纯 `(workspace) -> tools`）构造不出来，只能由组合根注入 catalog。
///
/// 口径演进过两次，每次都是"红灯与事实脱节"：
/// - W2-T1 之前只认"legacy 字符串分派里出现过"，于是 `todo`/`finish` 这类
///   "实现早已存在、只是没被装配"的工具被误报不可达；
/// - W3-T4 删掉 legacy 后第 (1) 条来源消失，替换为上面的 (2)。
///
/// 注意 (2) 只是**结构**证据："有实现"不等于"被装配进 catalog"。
/// 后者由行为断言保证（`agent-host` 的
/// `test_subagent_delegation_tool_is_in_the_catalog`）。
fn check_descriptor_reachability(root: &Path, rep: &mut Report) -> anyhow::Result<()> {
    let registry: BTreeSet<String> = agent_toolkit::registry::standard_tool_descriptors()
        .iter()
        .map(|d| d.name.clone())
        .collect();
    let mut reachable = BTreeSet::new();

    // (1) 工具包工厂提供的 Tool 实例
    let from_catalog = agent_toolkit::all_toolkit_tool_names(root);
    for name in &from_catalog {
        if registry.contains(name) {
            reachable.insert(name.clone());
        }
    }

    // (2) core 侧 Tool 实现（宿主耦合工具）
    let mut from_core = BTreeSet::new();
    for path in rust_sources(root, &["crates/agent-core/src"]) {
        let Ok(src) = std::fs::read_to_string(&path) else { continue };
        // 只认**真的实现了 Tool** 的文件，避免把名字字面量出现在任意文件里当成"可达"
        if !src.contains("impl Tool for") {
            continue;
        }
        for name in &registry {
            if src.contains(&format!("\"{name}\"")) {
                from_core.insert(name.clone());
            }
        }
    }
    for name in &from_core {
        reachable.insert(name.clone());
    }

    let unreachable: Vec<&String> = registry.difference(&reachable).collect();
    for n in &unreachable {
        rep.violations.push(format!(
            "工具 `{n}` 在注册表里有 ToolDescriptor，但没有任何执行路径\
             （既没有工具包工厂提供实例，也没有 core 侧 `impl Tool for` 实现）"
        ));
    }
    rep.notes.push(format!(
        "注册表 {} 个描述符，可达 {} 个（工具包工厂 {} 个、core 侧 Tool 实现 {} 个），不可达 {} 个",
        registry.len(),
        reachable.len(),
        from_catalog.intersection(&registry).count(),
        from_core.intersection(&registry).count(),
        unreachable.len()
    ));
    Ok(())
}

/// E. `FailDirection` 必须被**真实消费**（W5-T5）。
///
/// 口径：`ApprovalGate::direction()` 必须在**生产代码**里有调用点。
///
/// 为什么单列一条：这个端口方法曾经在 3 个闸门实现里都写了、却**没有任何调用点**——
/// 也就是"声明了安全默认，但代码路径上不存在"。这类漂移静态审查很难发现
/// （trait 方法有默认实现，编译器不会提醒你没人调）。
///
/// 只断言"引擎里出现方向裁决"这一件具体的事，不做宽泛的"所有 trait 方法都要被调用"
/// 启发式（那会把合法的扩展点也判红）。
fn check_fail_direction_is_consumed(root: &Path, rep: &mut Report) -> anyhow::Result<()> {
    let engine_dir = root.join("crates/agent-base/src/engine");
    let mut callers: Vec<String> = Vec::new();

    let mut files: Vec<PathBuf> = Vec::new();
    collect_rs(&engine_dir, &mut files);
    for p in &files {
        let Ok(src) = fs::read_to_string(p) else { continue };
        // 只看生产段（跳过 `#[cfg(test)]` 之后的断言）
        let prod = production_prefix(&src).unwrap_or(&src);
        if prod.contains(".direction()") {
            callers.push(rel(root, p));
        }
    }

    if callers.is_empty() {
        rep.violations.push(
            "`FailDirection` 没有被真实消费：`crates/agent-base/src/engine` 里没有任何 \
             `.direction()` 调用点（声明了安全默认，但代码路径上不存在）"
                .to_string(),
        );
    } else {
        rep.notes
            .push(format!("`FailDirection` 的生产消费者 → {}", callers.join(", ")));
    }
    Ok(())
}

/// F. 前端重连必须具备四项能力（W6-T6）。
///
/// 为什么用**源码断言**而不是前端单测：本仓没有前端测试运行器
/// （TS 测试已随旧实现整体冻结，`bun test` 会得到 "No tests found"）。
/// 而这四项是**结构性**要求，源码断言足够精确、毫秒级、且不会误报。
///
/// 断言的四项：
/// 1. 退避策略是**独立纯模块**（可独立加载验证，不被客户端模块的副作用拖住）；
/// 2. 客户端**真的用它**（不许退回固定间隔）；
/// 3. 有**代次保护**（旧连接的回调不得覆盖新连接状态）；
/// 4. 断线时**立刻失败在途请求**（不许让调用方干等超时）。
fn check_frontend_reconnect(root: &Path, rep: &mut Report) -> anyhow::Result<()> {
    let policy_path = "tauri-ui/src/client/reconnect-policy.ts";
    let client_path = "tauri-ui/src/client/ws-client.ts";

    let Ok(policy) = read(root, policy_path) else {
        rep.violations
            .push(format!("前端重连退避策略模块 `{policy_path}` 缺失（W6-T6）"));
        return Ok(());
    };
    let Ok(client) = read(root, client_path) else {
        rep.violations
            .push(format!("前端客户端 `{client_path}` 缺失"));
        return Ok(());
    };

    // 1. 策略模块：必须有导出函数 + 上限常量
    if !policy.contains("export function reconnectDelayMs") {
        rep.violations
            .push(format!("`{policy_path}` 必须导出 `reconnectDelayMs`"));
    }
    if !policy.contains("RECONNECT_MAX_MS") {
        rep.violations
            .push(format!("`{policy_path}` 必须有退避**上限**常量（纯指数会退化成「几小时后再连」）"));
    }
    if !policy.contains("JITTER") {
        rep.violations.push(format!(
            "`{policy_path}` 必须带**抖动**（否则多客户端同时重连会惊群）"
        ));
    }

    // 2. 客户端真的用了它
    if !client.contains("from './reconnect-policy'") || !client.contains("reconnectDelayMs(") {
        rep.violations.push(format!(
            "`{client_path}` 必须使用 `reconnect-policy` 的 `reconnectDelayMs`（不许自写固定间隔）"
        ));
    }

    // 3. 代次保护：必须有代次计数 + 回调里的比对
    if !client.contains("connectionGeneration") {
        rep.violations.push(format!(
            "`{client_path}` 缺少**代次保护**（`connectionGeneration`）：\
             旧连接的异步回调会覆盖新连接的 url/token，导致多条 socket 并存"
        ));
    }
    let guard_checks = client.matches("gen !== this.connectionGeneration").count();
    if guard_checks < 3 {
        rep.violations.push(format!(
            "`{client_path}` 的代次比对只出现在 {guard_checks} 处（onopen/onmessage/onclose 至少各一处）"
        ));
    }

    // 4. 断线立刻失败在途请求
    if !client.contains("failAllPending") || !client.contains("pendingRequests.clear()") {
        rep.violations.push(format!(
            "`{client_path}` 断线时必须**立刻失败在途请求**（`failAllPending` + `pendingRequests.clear()`），\
             否则调用方要干等 15s 超时"
        ));
    }
    // `onclose` 里必须调它（取 `onclose` 的花括号体）
    let onclose_ok = match_region(&client, "socket.onclose = () =>").is_some_and(|region| {
        region.contains("failAllPending")
    });
    if !onclose_ok {
        rep.violations.push(format!(
            "`{client_path}` 的 `onclose` 里没有调用 `failAllPending`"
        ));
    }

    // 5. 回归抗体：固定间隔重连不许回来
    if client.contains("}, 2000)") {
        rep.violations.push(format!(
            "`{client_path}` 又出现了固定 2000ms 重连（W6-T6 修掉的形态）"
        ));
    }

    if rep
        .violations
        .iter()
        .all(|v| !v.contains("tauri-ui/src/client"))
    {
        rep.notes.push(
            "前端重连：退避（指数+封顶+抖动）、代次保护、断线清空在途请求 均就位".to_string(),
        );
    }
    Ok(())
}

/// 尚未有发射者的 `AgentEventBody` 变体（**显式豁免，逐项写明理由**）。
///
/// 机制与 `LEGACY_ONLY_TOOLS` 相同：**临时豁免必须收敛**——
/// 一旦某变体有了发射者，它必须从这张表里删掉，否则门会报"陈旧豁免"。
/// 这样缺口不会被"记在某个文档里然后忘掉"，而是**钉在门禁上**。
const UNEMITTED_EVENT_ALLOW: &[(&str, &str)] = &[
    (
        "QuestionAsked",
        "缺口：`ask_user` 工具（agent-toolkit/src/core/ask_user.rs）不提问就返回错误，\
         因此无人注册 waiter、无人发射本事件 → 界面永不显示提问卡片、`question.answer` 找不到 waiter",
    ),
    (
        "SubagentStarted",
        "缺口：子智能体生命周期未上报（engine_bridge 把本事件映射为 None）；\
         子智能体进度通道 `SubagentStepUpdate` 在 `InvokeSubagentTool` 里被置为 None",
    ),
    (
        "SubagentFinished",
        "同上（与 SubagentStarted 同一处缺口）",
    ),
];

/// G. 每个 `AgentEventBody` 变体都必须有**生产发射者**（W6-T7 后新增）。
///
/// 为什么需要这条：`AgentEventBody` 是领域事件的**定义处**，而"定义了却没人发"
/// 是一类**静默失效**——消费方写好了、界面渲染写好了、测试也在（喂的是手工构造的事件），
/// 唯独生产路径上没有任何东西发出它。`QuestionAsked` 就是这么躺了很久的：
/// `ask_user` 在 catalog 里、工具实现存在、UI 卡片组件存在，**但那条链路永远不触发**。
///
/// 判据：变体必须出现在 `crates/agent-base/src/engine/**` 的生产段里——
/// 引擎是 `AgentEvent` 的**唯一生产者**（INV-6：`seq` 由 runtime 生成）。
///
/// 豁免必须显式登记 + 写明理由，且**陈旧豁免会报红**（已有发射者却还在豁免表里）。
fn check_event_emitters(root: &Path, rep: &mut Report) -> anyhow::Result<()> {
    let event_src = read(root, "crates/agent-base/src/domain/event.rs")?;
    let Some(body) = match_region(&event_src, "pub enum AgentEventBody") else {
        rep.violations
            .push("解析不出 `AgentEventBody` 枚举体——审计失效（不会静默通过）".to_string());
        return Ok(());
    };

    let variants: Vec<String> = body
        .lines()
        .filter_map(|l| {
            let t = l.trim_start();
            if l.len() - t.len() != 4 || !t.starts_with(|c: char| c.is_ascii_uppercase()) {
                return None;
            }
            let name: String = t
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name.is_empty() {
                None
            } else {
                Some(name)
            }
        })
        .collect();

    if variants.is_empty() {
        rep.violations
            .push("`AgentEventBody` 解析出 0 个变体——审计失效".to_string());
        return Ok(());
    }

    // 引擎生产段（发射者只可能在这里）
    let mut engine_src = String::new();
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rs(&root.join("crates/agent-base/src/engine"), &mut files);
    for p in &files {
        let Ok(src) = fs::read_to_string(p) else { continue };
        let prod = production_prefix(&src).unwrap_or(&src);
        engine_src.push_str(prod);
        engine_src.push('\n');
    }

    let allowed: BTreeSet<&str> = UNEMITTED_EVENT_ALLOW.iter().map(|(v, _)| *v).collect();
    let mut unemitted: Vec<&String> = Vec::new();
    let mut emitted: BTreeSet<&String> = BTreeSet::new();

    for v in &variants {
        if engine_src.contains(&format!("AgentEventBody::{v}")) {
            emitted.insert(v);
        } else {
            unemitted.push(v);
        }
    }

    // 1. 没有发射者的变体必须在豁免表里
    for v in &unemitted {
        if !allowed.contains(v.as_str()) {
            rep.violations.push(format!(
                "事件变体 `{v}` 定义了却**没有任何生产发射者**（引擎里不出现）——\
                 消费方/界面可能已经写好，但那条链路永远不触发。\
                 要么补发射者，要么加入 `UNEMITTED_EVENT_ALLOW` 并写明理由"
            ));
        }
    }

    // 2. 陈旧豁免：已有发射者却还在豁免表里 → 必须收敛
    for (v, _) in UNEMITTED_EVENT_ALLOW {
        if emitted.contains(&v.to_string()) {
            rep.violations.push(format!(
                "陈旧豁免：`{v}` 已经有生产发射者了，请从 `UNEMITTED_EVENT_ALLOW` 里删掉它\
                 （临时豁免必须收敛）"
            ));
        }
        // 3. 豁免项必须是**真实存在的变体**——拼错名字会让豁免表悄悄失效
        if !variants.iter().any(|x| x == v) {
            rep.violations.push(format!(
                "`UNEMITTED_EVENT_ALLOW` 里的 `{v}` 不是 `AgentEventBody` 的变体\
                 （拼错名字会让豁免表悄悄失效）"
            ));
        }
    }

    let mut unemitted_allowed: Vec<&str> = unemitted
        .iter()
        .map(|s| s.as_str())
        .filter(|s| allowed.contains(s))
        .collect();
    unemitted_allowed.sort_unstable();
    rep.notes.push(format!(
        "事件变体 {} 个：有发射者 {} 个、**待补发射者 {} 个**（已登记豁免：{}）",
        variants.len(),
        emitted.len(),
        unemitted.len(),
        if unemitted_allowed.is_empty() {
            "无".to_string()
        } else {
            unemitted_allowed.join(", ")
        }
    ));
    Ok(())
}

/// D. 每个**真实存在的**端口 trait 都必须有生产实现（排除 `agent-base/src/testing` 的测试替身）。
///
/// 口径演进（W6-T5）：原先这里硬编码了一张 **10 个名字**的清单，而
/// `crates/agent-base/src/ports/` 实际有 **11 个** trait（漏了 `Tool`）——
/// 于是"清单"和"事实"各说各话：`Tool` 有没有生产实现根本没人在管。
///
/// 现在**从 trait 定义派生**：扫 `crates/agent-base/src/ports/*.rs` 里的 `pub trait X`，
/// 逐个要求有生产实现。这样新增端口会自动纳入审计，不需要记得改这张表
/// （与 W2-T5「消灭第 6 张名单」同一条原则）。
fn check_port_impls(root: &Path, rep: &mut Report) -> anyhow::Result<()> {
    // ── 派生端口清单 ─────────────────────────────────────────────────────
    let ports_dir = root.join("crates/agent-base/src/ports");
    let mut ports: BTreeSet<String> = BTreeSet::new();
    let mut port_files: Vec<PathBuf> = Vec::new();
    collect_rs(&ports_dir, &mut port_files);
    for p in &port_files {
        let Ok(src) = fs::read_to_string(p) else { continue };
        for line in src.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("pub trait ") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    ports.insert(name);
                }
            }
        }
    }
    if ports.is_empty() {
        rep.violations.push(
            "解析不出 `agent-base/src/ports` 的端口 trait——审计失效（不会静默通过）".to_string(),
        );
        return Ok(());
    }

    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for p in rust_sources(root, &["crates", "products", "src-tauri"]) {
        let rel_path = rel(root, &p);
        if rel_path.contains("agent-base/src/testing") {
            continue;
        }
        let Ok(src) = fs::read_to_string(&p) else { continue };
        let Some(prod) = production_prefix(&src) else { continue };
        for port in &ports {
            if prod.contains(&format!("impl {port} for")) || prod.contains(&format!("impl {port}<")) {
                found.entry(port.clone()).or_default().insert(rel_path.clone());
            }
        }
    }

    let mut missing: Vec<&String> = Vec::new();
    for port in &ports {
        match found.get(port) {
            Some(files) => rep.notes.push(format!(
                "端口 {port} → {}",
                files.iter().cloned().collect::<Vec<_>>().join(", ")
            )),
            None => missing.push(port),
        }
    }
    for port in &missing {
        rep.violations.push(format!(
            "端口 {port} 没有生产实现（只有 agent-base/src/testing 里的测试替身）"
        ));
    }
    rep.notes.push(format!(
        "端口 trait {} 个（从 `agent-base/src/ports` 派生），缺生产实现 {} 个",
        ports.len(),
        missing.len()
    ));
    Ok(())
}

// ── 闸 4：verify-docs（文档不得把归档布局当现行路径） ────────────────────────

/// 主干 markdown 不得把归档时代的布局当作**现行**路径来描述。
///
/// 背景：README 正文、`unfinished-features.md`、`feature-catalog.md` 仍在用
/// `src/agent/**`、`src/ui/**`、`../gpuix`、`bun test` 描述当前架构，
/// 与它们自己的顶部横幅（纯 Rust 微内核 + Tauri + TS 已归档）自相矛盾。
///
/// 判据：命中任一模式的文件必须出现在 [`DOC_ALLOW`] 里。白名单**逐项写明理由**，
/// 只放"用途就是描述归档时代或定义归档规矩"的文档——所以这份名单本身是可审阅的。
///
/// 为什么单独一个子命令而不是塞进 `verify-archive`：归档门的语义是"主干没有第二份实现"，
/// 文档时效性是另一件事；混在一起会让主门禁 `cargo xtask verify` 长期变红、
/// 掩盖其它信号（违反计划 R8「每个里程碑独立可交付」）。
pub fn verify_docs() -> anyhow::Result<Report> {
    let root = workspace_root();
    let mut rep = Report::new("校验文档时效性（verify-docs）");

    const PATTERNS: [(&str, &str); 8] = [
        ("src/agent/", "src/agent/（已归档的 TS agent）"),
        ("src/ui/", "src/ui/（已归档的 GPUIX 界面）"),
        ("../gpuix", "../gpuix（已冻结的 GPUIX 仓库）"),
        ("app.tsx", "app.tsx（已归档入口）"),
        ("bun run link", "bun run link（已归档的链接脚本）"),
        ("bun test", "bun test（已退出门禁的 TS 测试）"),
        ("AgentWindow.tsx", "AgentWindow.tsx（已归档窗口骨架）"),
        ("src/platform/", "src/platform/（已归档平台层）"),
    ];

    /// 允许提到归档路径的文件：其用途就是描述归档时代或定义归档规矩。
    const DOC_ALLOW: [(&str, &str); 16] = [
        ("AGENTS.md", "定义归档规矩本身（\"别跑 bun test\"、§ 索引声明 §1–§15 描述归档实现）"),
        ("docs/agent-base-plan.md", "历史执行记录（§5 归档范围分类表、附录 A 文件级迁移映射）"),
        ("docs/agent-base-design.md", "§10 迁移路径与附录迁移表"),
        ("docs/agent-conventions.md", "文件头声明 §1–§15 正文描述已归档的 TS 实现"),
        ("docs/plugin-system-design.md", "TS 时代插件系统设计文档"),
        ("docs/plugin-system-dev-plan.md", "TS 时代插件系统开发计划"),
        ("docs/core-extension-capabilities-design.md", "TS 时代扩展能力设计文档"),
        ("docs/decision-plugin-design.md", "TS 时代决策插件设计文档"),
        ("docs/ui-host-split-dev-plan.md", "UI/宿主拆分开发计划（TS 时代）"),
        ("docs/pure-rust-ts-microkernel-design.md", "从 TS 迁往纯 Rust 的过渡设计"),
        ("docs/memory-optimization-plan.md", "TS 时代内存优化计划"),
        ("docs/pure-rust-backend-dev-plan.md", "后端纯 Rust 化开发计划（含 TS 侧对照）"),
        ("docs/rust-backend-refactor-design.md", "Rust 后端重构设计（含 TS 侧对照）"),
        ("docs/jsonrpc-protocol.md", "协议文档（含 TS 客户端章节）"),
        ("docs/plugin-sdk/v1.md", "插件 SDK 契约（对照 TS 时代的 25 个钩子点位）"),
        ("docs/agent-base-wiring-plan.md", "接线收口计划自身在\"防漂移规则\"里引用这些路径"),
    ];

    let skip = ["node_modules", "target", "dist", "tmp", "archive", ".git", ".ada", ".commandcode"];
    let mut md_files: Vec<PathBuf> = Vec::new();
    collect_md(&root, &skip, &mut md_files);
    md_files.sort();

    let allow: BTreeSet<&str> = DOC_ALLOW.iter().map(|(f, _)| *f).collect();
    let mut scanned = 0usize;
    for f in &md_files {
        let path = rel(&root, f);
        if allow.contains(path.as_str()) {
            continue;
        }
        scanned += 1;
        let Ok(text) = fs::read_to_string(f) else { continue };
        for (lineno, line) in text.lines().enumerate() {
            for (pat, what) in PATTERNS {
                if line.contains(pat) {
                    rep.violations.push(format!(
                        "{path}:{} 把归档布局当现行路径（提到 {what}）",
                        lineno + 1
                    ));
                }
            }
        }
    }
    rep.notes.push(format!(
        "白名单 {} 个文件（见 gates.rs DOC_ALLOW 的逐项理由），实扫 {scanned} 个 markdown",
        DOC_ALLOW.len()
    ));

    // ── 协议文档存在且与登记表同步（W5-T3）──────────────────────────────────
    //
    // 为什么要有这条：`spec/proto/README.md` 与 `docs/protocol/README.md` 是
    // "协议怎么用"的**唯一人类可读入口**。没有机械守门的话，删掉它们、
    // 或加了事件主题却不更新文档，都不会有任何信号。
    //
    // 只断言"具体且不会误报"的东西：文档存在 + **每个事件主题都被文档提到**。
    // 不去解析文档结构（那会变成脆弱的启发式）。
    {
        let required = ["spec/proto/README.md", "docs/protocol/README.md"];
        for r in required {
            if !root.join(r).exists() {
                rep.violations
                    .push(format!("协议文档 `{r}` 缺失（W5-T3 的交付物）"));
            }
        }

        let methods_src = read(&root, "crates/agent-proto/src/methods.rs").unwrap_or_default();
        // `ALL_EVENTS` 里放的是**常量名**（`EVT_STATE_SNAPSHOT`），不是字符串字面量，
        // 所以要再查一次每个常量的值。`ALL_EVENTS` 是 `&[...]` 不是 `match { }`，
        // 因此也不能用 `match_region`。
        let event_values = |name: &str| -> Option<String> {
            let needle = format!("pub const {name}: &str = \"");
            let s = methods_src.find(&needle)?;
            let rest = &methods_src[s + needle.len()..];
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        };
        let events: Vec<String> = methods_src
            .find("pub const ALL_EVENTS")
            .and_then(|s| {
                let after = &methods_src[s..];
                // 必须定位**初始化式**里的 `[`——类型 `&[&str]` 里也有一个 `[`，
                // 直接 `find('[')` 会命中它，取出来的区域只有 `&str`（踩过一次）。
                let init = after.find("= &[")? + 4;
                let close = after[init..].find(']')?;
                Some(&after[init + 1..init + close])
            })
            .map(|r| {
                r.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .filter(|s| s.starts_with("EVT_"))
                    .filter_map(event_values)
                    .collect()
            })
            .unwrap_or_default();

        if events.is_empty() {
            rep.violations
                .push("解析不出 ALL_EVENTS 的事件主题，协议文档同步检查无效".to_string());
        } else if let Ok(proto_doc) = read(&root, "docs/protocol/README.md") {
            for e in &events {
                if !proto_doc.contains(e.as_str()) {
                    rep.violations.push(format!(
                        "docs/protocol/README.md 未提及事件主题 `{e}`（登记表加了主题，文档要跟上）"
                    ));
                }
            }
            rep.notes
                .push(format!("协议文档已覆盖全部 {} 个事件主题", events.len()));
        }
    }

    Ok(rep)
}

fn collect_md(dir: &Path, skip: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if skip.contains(&name.as_str()) {
                continue;
            }
            collect_md(&p, skip, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

// ── 源码文本提取 ────────────────────────────────────────────────────────────

/// `pub const NAME: &str = "value";` → { NAME: value }
fn parse_proto_consts(src: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in src.lines() {
        let l = line.trim();
        let Some(rest) = l.strip_prefix("pub const ") else { continue };
        let Some((name, tail)) = rest.split_once(':') else { continue };
        let name = name.trim();
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            continue;
        }
        let Some((_, val)) = tail.split_once('=') else { continue };
        let val = val.trim();
        let Some(v) = val.strip_prefix('"') else { continue };
        let Some(end) = v.find('"') else { continue };
        out.insert(name.to_string(), v[..end].to_string());
    }
    out
}

/// `ALL_METHODS` 块里的标识符列表。
///
/// 注意必须找 `= &[` 而不是第一个 `&[`——后者会命中类型标注 `pub const ALL_METHODS: &[&str]`。
fn parse_all_methods(src: &str) -> Vec<String> {
    let Some(start) = src.find("pub const ALL_METHODS") else { return Vec::new() };
    let rest = &src[start..];
    let Some(open) = rest.find("= &[") else { return Vec::new() };
    let Some(close) = rest[open..].find("];") else { return Vec::new() };
    rest[open + 4..open + close]
        .split(',')
        .map(|s| s.split("//").next().unwrap_or("").trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// match 体里的臂名（支持 `A | B =>`）。
fn parse_arms(region: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in region.lines() {
        let l = line.trim();
        let Some(pos) = l.find("=>") else { continue };
        let left = l[..pos].trim();
        if left.is_empty() {
            continue;
        }
        let mut names = Vec::new();
        let mut ok = true;
        for part in left.split('|') {
            let p = part.trim();
            let valid = !p.is_empty()
                && p.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                && p.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if !valid {
                ok = false;
                break;
            }
            names.push(p.to_string());
        }
        if ok {
            out.extend(names);
        }
    }
    out
}

/// 提取 `"name": "xxx"` 里的值。
fn extract_json_names(region: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = region;
    while let Some(i) = rest.find("\"name\"") {
        rest = &rest[i + 6..];
        let Some(after_colon) = rest.trim_start().strip_prefix(':') else { continue };
        let Some(after_quote) = after_colon.trim_start().strip_prefix('"') else { continue };
        let Some(end) = after_quote.find('"') else { continue };
        out.insert(after_quote[..end].to_string());
        rest = &after_quote[end + 1..];
    }
    out
}
