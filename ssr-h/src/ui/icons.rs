//! SVG 图标集：**全系统唯一来源**。
//!
//! 过去图标散落在各模板里内联，同一个「刷新」写了两遍、改色要改多处。
//! 现在图标定义只存在这里：
//!
//! - Rust 组件用 [`icon`] 直接输出 `<svg>`；
//! - Askama 模板用 [`sprite`] 注入的 `<symbol>` + `<use href="#i-xxx">` 引用。
//!
//! 规范：24×24 视窗、1.75 描边、圆角线帽、`currentColor` 描边（不写死颜色），
//! 这样深浅主题只需要换 `--ph-*` 变量，图标自动跟随。

/// 图标定义：键名 + 图形内容（`<svg>` 的子节点）。
pub struct IconDef {
    pub key: &'static str,
    pub body: &'static str,
}

macro_rules! icon {
    ($key:literal, $body:literal) => {
        IconDef {
            key: $key,
            body: $body,
        }
    };
}

/// 全部图标。键名以 `i-` 开头，与模板里的 `<use href="#i-xxx">` 对应。
pub const ICONS: &[IconDef] = &[
    icon!(
        "i-diamond",
        r#"<path d="M12 2.5 21.5 12 12 21.5 2.5 12Z"/>"#
    ),
    icon!("i-plus", r#"<path d="M12 5v14M5 12h14"/>"#),
    icon!("i-minus", r#"<path d="M5 12h14"/>"#),
    icon!(
        "i-chat",
        r#"<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2Z"/>"#
    ),
    icon!(
        "i-grid",
        r#"<rect x="4" y="4" width="7" height="7" rx="1.5"/><rect x="13" y="4" width="7" height="7" rx="1.5"/><rect x="4" y="13" width="7" height="7" rx="1.5"/><rect x="13" y="13" width="7" height="7" rx="1.5"/>"#
    ),
    icon!(
        "i-server",
        r#"<rect x="3" y="4" width="18" height="7" rx="2"/><rect x="3" y="13" width="18" height="7" rx="2"/><path d="M7 8h.01M7 17h.01"/>"#
    ),
    icon!(
        "i-chart",
        r#"<path d="M3 20h18"/><path d="M6 20v-6"/><path d="M11 20V8"/><path d="M16 20v-9"/><path d="M21 20V5"/>"#
    ),
    icon!(
        "i-folder",
        r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>"#
    ),
    icon!(
        "i-search",
        r#"<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>"#
    ),
    icon!(
        "i-moon",
        r#"<path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8Z"/>"#
    ),
    icon!(
        "i-sun",
        r#"<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>"#
    ),
    icon!(
        "i-refresh",
        r#"<path d="M3 12a9 9 0 0 1 15-6.7L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-15 6.7L3 16"/><path d="M3 21v-5h5"/>"#
    ),
    icon!(
        "i-stop",
        r#"<rect x="7" y="7" width="10" height="10" rx="2"/>"#
    ),
    icon!("i-send", r#"<path d="M12 19V5"/><path d="m5 12 7-7 7 7"/>"#),
    icon!(
        "i-clock",
        r#"<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>"#
    ),
    icon!("i-check", r#"<path d="M20 6 9 17l-5-5"/>"#),
    icon!(
        "i-check-circle",
        r#"<circle cx="12" cy="12" r="9"/><path d="m8.5 12.5 2.5 2.5 4.5-5"/>"#
    ),
    icon!("i-x", r#"<path d="M18 6 6 18M6 6l12 12"/>"#),
    icon!(
        "i-x-circle",
        r#"<circle cx="12" cy="12" r="9"/><path d="m15 9-6 6M9 9l6 6"/>"#
    ),
    icon!(
        "i-trash",
        r#"<path d="M3 6h18"/><path d="M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6M14 11v6"/>"#
    ),
    icon!("i-chevron", r#"<path d="m9 6 6 6-6 6"/>"#),
    icon!("i-chevron-down", r#"<path d="m6 9 6 6 6-6"/>"#),
    icon!("i-chevron-up", r#"<path d="m6 15 6-6 6 6"/>"#),
    icon!(
        "i-arrow-right",
        r#"<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>"#
    ),
    icon!("i-circle", r#"<circle cx="12" cy="12" r="9"/>"#),
    icon!(
        "i-arrow-up",
        r#"<path d="M12 19V5"/><path d="m5 12 7-7 7 7"/>"#
    ),
    icon!(
        "i-alert",
        r#"<path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/><path d="M12 9v4"/><path d="M12 17h.01"/>"#
    ),
    icon!(
        "i-bulb",
        r#"<path d="M9 18h6"/><path d="M10 21h4"/><path d="M12 3a6 6 0 0 0-3.6 10.8c.6.5.9 1.2.9 2h5.4c0-.8.3-1.5.9-2A6 6 0 0 0 12 3Z"/>"#
    ),
    icon!(
        "i-brain",
        r#"<path d="M12 5a3 3 0 0 0-3 3 2.5 2.5 0 0 0-1.5 4.5A2.5 2.5 0 0 0 9 17a3 3 0 0 0 3 2z"/><path d="M12 5a3 3 0 0 1 3 3 2.5 2.5 0 0 1 1.5 4.5A2.5 2.5 0 0 1 15 17a3 3 0 0 1-3 2z"/><path d="M12 5v14"/>"#
    ),
    icon!(
        "i-terminal",
        r#"<path d="m4 17 6-6-6-6"/><path d="M12 19h8"/>"#
    ),
    icon!(
        "i-bot",
        r#"<rect x="5" y="8" width="14" height="11" rx="3"/><path d="M12 8V5"/><circle cx="12" cy="4" r="1"/><path d="M9.5 12.5h.01M14.5 12.5h.01"/><path d="M9.5 16c1.6 1 3.4 1 5 0"/>"#
    ),
    icon!(
        "i-shield",
        r#"<path d="M12 22s8-3.6 8-10V5.5L12 2 4 5.5V12c0 6.4 8 10 8 10Z"/>"#
    ),
    icon!(
        "i-shield-check",
        r#"<path d="M12 22s8-3.6 8-10V5.5L12 2 4 5.5V12c0 6.4 8 10 8 10Z"/><path d="m9 12 2 2 4-4"/>"#
    ),
    icon!(
        "i-plug",
        r#"<path d="M9 3v6M15 3v6"/><path d="M6 9h12v3a6 6 0 0 1-12 0Z"/><path d="M12 18v3"/>"#
    ),
    // 未登记工具的兜底：不是文件夹也不是终端，用扳手最诚实。
    icon!(
        "i-wrench",
        r#"<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76Z"/>"#
    ),
    icon!(
        "i-external",
        r#"<path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><path d="M15 3h6v6"/><path d="M10 14 21 3"/>"#
    ),
    icon!("i-menu", r#"<path d="M4 6h16M4 12h16M4 18h16"/>"#),
    icon!(
        "i-git-branch",
        r#"<line x1="6" x2="6" y1="4" y2="16"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/>"#
    ),
    icon!(
        "i-sparkles",
        r#"<path d="M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9Z"/><path d="M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8Z"/>"#
    ),
    icon!(
        "i-file",
        r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8Z"/><path d="M14 3v5h5"/><path d="M9 13h6M9 17h4"/>"#
    ),
    icon!(
        "i-edit",
        r#"<path d="M12 20h9"/><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z"/>"#
    ),
    icon!(
        "i-package",
        r#"<path d="m7.5 4.3 9 5.2"/><path d="M21 16V8a2 2 0 0 0-1-1.7l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.7l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/><path d="m3.3 7 8.7 5 8.7-5"/><path d="M12 22V12"/>"#
    ),
    icon!(
        "i-code",
        r#"<path d="m16 18 6-6-6-6"/><path d="m8 6-6 6 6 6"/>"#
    ),
    icon!(
        "i-list",
        r#"<path d="m3 7 2 2 4-4"/><path d="m3 17 2 2 4-4"/><path d="M13 6h8M13 12h8M13 18h8"/>"#
    ),
    icon!(
        "i-copy",
        r#"<rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>"#
    ),
    icon!(
        "i-layers",
        r#"<path d="m12 2 9 5-9 5-9-5 9-5Z"/><path d="m3 12 9 5 9-5"/><path d="m3 17 9 5 9-5"/>"#
    ),
    icon!(
        "i-wave",
        r#"<path d="M2 12h2M7 7v10M12 3v18M17 7v10M22 12h-2"/>"#
    ),
];

/// 图标是否存在。
pub fn has(key: &str) -> bool {
    ICONS.iter().any(|icon| icon.key == key)
}

/// 输出一个可直接内联的 `<svg>`。
///
/// 未登记的键返回空串——调用方不该因为一个拼错的图标名而 panic。
pub fn icon(key: &str, size: u16) -> String {
    let Some(def) = ICONS.iter().find(|icon| icon.key == key) else {
        return String::new();
    };
    format!(
        r#"<svg class="ada-icon" width="{size}" height="{size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{}</svg>"#,
        def.body
    )
}

/// 输出图标 sprite（`<symbol>` 集合），注入模板后可用 `<use href="#i-xxx">`。
///
/// 这是模板侧引用图标的唯一通道：模板不再内联任何图形，图标改一处即可全局生效。
pub fn sprite() -> String {
    let mut out = String::with_capacity(8192);
    out.push_str(
        r#"<svg class="ada-sprite" aria-hidden="true" focusable="false" width="0" height="0">"#,
    );
    for def in ICONS {
        out.push_str(&format!(
            r#"<symbol id="{}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">{}</symbol>"#,
            def.key, def.body
        ));
    }
    out.push_str("</svg>");
    out
}
