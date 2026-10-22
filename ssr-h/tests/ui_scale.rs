//! 设计系统护栏（design-system guardrails）。
//!
//! 这里的测试不检查"好不好看"，只守住两条底线：
//!
//! 1. **真源唯一** —— 颜色令牌只在 `design/tokens.css` 定义，两个 crate 用的
//!    `app.css` 必须是同一次构建的产物。
//! 2. **标尺不再漂移** —— 圆角、字号、图标尺寸只能从标尺里取值，不许就地
//!    发明新台阶。
//!
//! 第 2 条用**棘轮（ratchet）** 实现：`BASELINE_*` 记录当前存量，测试保证
//! 「不超过基线」且「不出现新档位」。所以现在全绿，但新的 `border-radius: 9px`
//! 会立刻失败。收敛一处就把对应数字减一——数字只许变小。
//!
//! 为什么不直接禁绝：存量有 53 处圆角、102 处字号，一次性替换属于视觉重构，
//! 风险远大于收益。护栏的作用是**先止血，再逐步收口**。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_dir() -> PathBuf {
    crate_dir().parent().expect("ssr-h 应有上级目录").to_path_buf()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("读不到 {}: {error}", path.display()))
}

/// 组件库与页面样式的源文件（`app.css` 是构建产物，不参与统计）。
const STYLE_SOURCES: &[&str] = &[
    "static/components.css",
    "static/layout.css",
    "static/pages.css",
    "static/shell.css",
    "static/widgets.css",
];

fn style_text() -> String {
    STYLE_SOURCES
        .iter()
        .map(|name| read(&crate_dir().join(name)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 抽出 `prop: <数字>px` 里数字的分布。`0` 与 `var()` 天然不匹配，会被跳过。
fn count_px_values(css: &str, prop: &str) -> BTreeMap<u32, usize> {
    let needle = format!("{prop}:");
    let mut out = BTreeMap::new();
    let mut rest = css;
    while let Some(pos) = rest.find(&needle) {
        rest = &rest[pos + needle.len()..];
        let value: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        // 只认整数：10.5px 这种半档由「新档位」规则挡（它不会等于任何已有整数）
        if let Ok(number) = value.trim_end_matches('.').parse::<u32>() {
            let after = rest.trim_start()[value.len()..].trim_start();
            if after.starts_with("px") {
                *out.entry(number).or_insert(0) += 1;
            }
        }
    }
    out
}

/// 棘轮检查：值集合不得新增，每个值的数量不得超过基线。
fn assert_ratchet(label: &str, found: &BTreeMap<u32, usize>, baseline: &[(u32, usize)]) {
    let allowed: BTreeMap<u32, usize> = baseline.iter().copied().collect();
    let mut problems = Vec::new();

    for (value, count) in found {
        match allowed.get(value) {
            None => problems.push(format!(
                "  {value}px × {count} —— 标尺里没有这一档，请改用 var(--ada-*)，\
                 或先把它加进 components.css 的标尺并更新基线"
            )),
            Some(limit) if count > limit => problems.push(format!(
                "  {value}px × {count} —— 超过基数 {limit}，说明新增了字面值"
            )),
            Some(_) => {}
        }
    }

    assert!(
        problems.is_empty(),
        "{label} 出现了新的硬编码档位：\n{}\n\n\
         处理方式：改用标尺令牌；若确实是必要的收敛工作，把 BASELINE 里对应数字调小。",
        problems.join("\n")
    );
}

// ── 真源唯一 ─────────────────────────────────────────────────────────────

#[test]
fn color_tokens_live_in_one_file() {
    // 令牌源只有一个：design/tokens.css。业务 CSS 里不许再定义 --ph-*。
    let tokens = read(&repo_dir().join("design/tokens.css"));
    assert!(
        tokens.contains("--ph-canvas:") && tokens.contains("html.dark"),
        "design/tokens.css 应同时包含亮色与暗色令牌"
    );

    for name in STYLE_SOURCES {
        let css = read(&crate_dir().join(name));
        assert!(
            !css.contains("--ph-canvas:"),
            "{name} 里又定义了一份颜色令牌——令牌只能在 design/tokens.css 定义"
        );
    }
}

#[test]
fn dark_mode_covers_every_light_token() {
    // 漏一个 dark 值，深色模式下那个令牌会退回浏览器默认，界面直接花掉。
    let tokens = read(&repo_dir().join("design/tokens.css"));
    let (light, dark) = tokens
        .split_once("html.dark")
        .expect("tokens.css 应有 html.dark 块");

    let names = |block: &str| -> Vec<String> {
        block
            .match_indices("--ph-")
            .filter_map(|(pos, _)| {
                let rest = &block[pos..];
                let end = rest.find(':')?;
                let name = &rest[..end];
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
                    .then(|| name.to_string())
            })
            .collect()
    };

    let light_names = names(light);
    let dark_names = names(dark);
    let missing: Vec<&String> = light_names
        .iter()
        .filter(|name| !dark_names.contains(name))
        .collect();
    assert!(
        missing.is_empty(),
        "这些令牌缺暗色取值：{missing:?}（深色模式会失效）"
    );
}

#[test]
fn app_css_is_the_same_build_on_both_sides() {
    // ssr-h 用的是 plugin-hub 的 Tailwind 产物。手工复制容易忘，忘了就是
    // 「改了令牌但对话页没变」——这类问题在浏览器里极难察觉。
    let hub = read(&repo_dir().join("crates/plugin-hub/static/app.css"));
    let ssr = read(&crate_dir().join("static/app.css"));
    assert_eq!(
        hub.len(),
        ssr.len(),
        "两份 app.css 长度不同：请重新构建 Tailwind 并同步到 ssr-h/static/app.css"
    );
    assert!(
        hub == ssr,
        "两份 app.css 内容不同：ssr-h 的样式与 plugin-hub 已经分叉。\n\
         重新构建：bun desktop-ui/node_modules/tailwindcss/lib/cli.js \
         -c crates/plugin-hub/tailwind.config.cjs \
         -i crates/plugin-hub/static/input.css \
         -o crates/plugin-hub/static/app.css --minify\n\
         再复制到 ssr-h/static/app.css"
    );
}

// ── 标尺已定义 ───────────────────────────────────────────────────────────

#[test]
fn scale_tokens_are_defined() {
    let css = read(&crate_dir().join("static/components.css"));
    for token in [
        "--ada-radius-sm:",
        "--ada-radius-md:",
        "--ada-radius-lg:",
        "--ada-radius-xl:",
        "--ada-radius-pill:",
        "--ada-text-xs:",
        "--ada-text-sm:",
        "--ada-text-base:",
        "--ada-text-md:",
        "--ada-text-lg:",
        "--ada-text-xl:",
        "--ada-icon-sm:",
        "--ada-icon-md:",
        "--ada-icon-lg:",
        "--ada-shadow-raised:",
        "--ada-shadow-overlay:",
    ] {
        assert!(
            css.contains(token),
            "标尺变量 {token} 缺失——组件会退回字面值，护栏也就守不住了"
        );
    }
}

// ── 棘轮：存量基线 ───────────────────────────────────────────────────────
//
// 🔻 这些数字只许变小。每收敛一处就减一，直至清零后把对应规则改成「禁止」。

/// `border-radius: <n>px` 的存量（53 处）。目标：全部走 var(--ada-radius-*)。
const BASELINE_RADIUS: &[(u32, usize)] = &[
    (4, 1),
    (5, 4),
    (6, 5),
    (7, 9),
    (8, 13),
    (9, 3),
    (10, 4),
    (13, 1),
    (14, 1),
];

/// `font-size: <n>px` 的存量（98 处）。半档已全部收成整数档。
const BASELINE_FONT_SIZE: &[(u32, usize)] = &[
    (10, 23),
    (11, 33),
    (12, 27),
    (13, 15),
    (14, 2),
    (15, 1),
    (16, 1),
];

/// 图标尺寸的存量（7 档）。目标：收敛到 12 / 14 / 18。
const BASELINE_ICON_SIZE: &[(u32, usize)] =
    &[(11, 2), (12, 8), (13, 6), (14, 11), (15, 3), (16, 1), (20, 1)];

#[test]
fn no_new_hardcoded_radius() {
    let found = count_px_values(&style_text(), "border-radius");
    assert_ratchet("圆角", &found, BASELINE_RADIUS);
}

#[test]
fn no_new_hardcoded_font_size() {
    let found = count_px_values(&style_text(), "font-size");
    assert_ratchet("字号", &found, BASELINE_FONT_SIZE);
}

#[test]
fn no_half_step_font_sizes() {
    // 9.5 / 10.5 / 11.5 这种半档是"再调一点点"的产物，不会带来可读性提升，
    // 只会让同一屏里出现三种近似字号。存量已清零，新增一律拒绝。
    let css = style_text();
    let mut offenders = Vec::new();
    for (index, line) in css.lines().enumerate() {
        let Some(rest) = line.split("font-size:").nth(1) else {
            continue;
        };
        let value: String = rest
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if value.contains('.') {
            offenders.push(format!("  第 {} 行：font-size: {value}px", index + 1));
        }
    }
    assert!(
        offenders.is_empty(),
        "字号回到整数档（用 --ada-text-* 令牌）：\n{}",
        offenders.join("\n")
    );
}

#[test]
fn templates_avoid_arbitrary_text_sizes() {
    // Tailwind 的 `text-[11.5px]` 会绕过标尺；模板只该用语义字号。
    let mut offenders = Vec::new();
    let dir = crate_dir().join("templates");
    let mut stack = vec![dir.clone()];
    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(&path).expect("读模板目录") {
            let entry = entry.expect("目录项");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "html") {
                let html = read(&path);
                for (index, line) in html.lines().enumerate() {
                    if line.contains("text-[") && line.contains("px]") {
                        offenders.push(format!(
                            "  {}:{}",
                            path.strip_prefix(&dir).unwrap_or(&path).display(),
                            index + 1
                        ));
                    }
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "模板里出现任意值字号（改用语义类或组件）：\n{}",
        offenders.join("\n")
    );
}

#[test]
fn icon_sizes_come_from_the_scale() {
    // Rust 侧 ui::icon(name, size) 的 size 也要守标尺，否则 CSS 收得再紧，
    // 图标还是会一屏七种大小。
    let mut found: BTreeMap<u32, usize> = BTreeMap::new();
    let mut stack = vec![crate_dir().join("src")];
    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(&path).expect("读 src") {
            let entry = entry.expect("目录项");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = read(&path);
                for line in source.lines() {
                    // ui::icon("i-x", 14) / icon(&name, 12) / self.icon(name, 14)
                    let Some(pos) = line.find("icon(") else {
                        continue;
                    };
                    let rest = &line[pos + "icon(".len()..];
                    let Some(comma) = rest.find(',').or_else(|| rest.find('，')) else {
                        continue;
                    };
                    let tail = rest[comma + 1..].trim_start();
                    let size: String = tail.chars().take_while(char::is_ascii_digit).collect();
                    if size.is_empty() {
                        continue;
                    }
                    // 必须紧跟右括号，否则是别的函数调用（比如 icon(name, width) 的变量）
                    let after = &tail[size.len()..];
                    if !after.trim_start().starts_with(')') {
                        continue;
                    }
                    if let Ok(value) = size.parse::<u32>() {
                        *found.entry(value).or_insert(0) += 1;
                    }
                }
            }
        }
    }
    assert!(!found.is_empty(), "没扫描到任何图标尺寸，正则可能失效了");
    assert_ratchet("图标尺寸", &found, BASELINE_ICON_SIZE);
}
