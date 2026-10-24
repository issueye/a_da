//! 侧栏（应用外壳左侧列）。
//!
//! 结构（自上而下）：
//!
//! 1. 品牌
//! 2. 新会话（描边按钮：侧栏里唯一的填充色留给"当前会话"）
//! 3. 工作区切换器（一行：名字 + 会话数 + 箭头；点进 `/workspaces`）
//! 4. 会话搜索（仅聊天页）
//! 5. 会话列表：**唯一**的伸缩与滚动区
//! 6. 系统导航（前端插件链接 + 插件/概览）
//! 7. 主题 + 网关
//!
//! 为什么由组件库渲染而不是模板：这段标记原先在 `chat.html` 与 `layout.html`
//! 各有一份（约 70 行逐字重复），改一次要改两处，漏一处就出现"首页与二级页
//! 侧栏不一样"。收进这里之后，模板只写 `{{ base.sidebar|safe }}`。
//!
//! 折叠态（`.ada-shell.is-sidebar-collapsed`）不隐藏侧栏，而是切成 56px 窄轨：
//! 收起之后仍能新建、跳会话、进系统页，不再"一收就没"。

use crate::plugins::NavLink;
use crate::templates::{PageBase, ThreadRow, WorkspaceGroup};
use crate::ui::components::Render;
use crate::ui::icons::icon;
use crate::ui::theme::{attr, class_list, escape};

/// 窄轨里最多列出几条会话：再多就失去"一眼定位"的意义。
const RAIL_THREAD_LIMIT: usize = 8;
/// 底部直出几条插件链接，其余收成"查看全部"。
const PLUGIN_LINK_LIMIT: usize = 3;

#[derive(Debug, Clone)]
pub struct Sidebar {
    pub active: String,
    pub workspace_name: String,
    pub workspace_path: String,
    pub gateway: String,
    /// "新会话"提交到哪个工作区（空串 = 不带该字段，交给 agent 的当前项目）。
    pub submit_workspace: String,
    pub groups: Vec<WorkspaceGroup>,
    pub active_thread_id: String,
    pub plugin_nav: Vec<NavLink>,
    pub sidebar_top: String,
    pub sidebar_bottom: String,
    /// 是否渲染会话搜索（只有聊天页有列表）。
    pub search: bool,
    /// 连接状态：`None` = 本页不掌握，不画状态点（宁可不显示，也不猜）。
    pub connected: Option<bool>,
}

impl Sidebar {
    /// 从页面外壳取公共字段；树与搜索由调用方按需补。
    pub fn from_base(base: &PageBase) -> Self {
        Self {
            active: base.active.clone(),
            workspace_name: base.active_workspace_name.clone(),
            workspace_path: base.active_workspace.clone(),
            gateway: base.gateway.clone(),
            submit_workspace: String::new(),
            groups: Vec::new(),
            active_thread_id: String::new(),
            plugin_nav: base.plugin_nav.clone(),
            sidebar_top: base.slots.sidebar_top.clone(),
            sidebar_bottom: base.slots.sidebar_bottom.clone(),
            search: false,
            connected: None,
        }
    }

    pub fn with_tree(
        mut self,
        groups: Vec<WorkspaceGroup>,
        active_thread_id: impl Into<String>,
        submit_workspace: impl Into<String>,
    ) -> Self {
        self.groups = groups;
        self.active_thread_id = active_thread_id.into();
        self.submit_workspace = submit_workspace.into();
        self.search = true;
        self
    }

    pub fn connected(mut self, connected: Option<bool>) -> Self {
        self.connected = connected;
        self
    }

    pub fn render(&self) -> String {
        format!(
            r#"<aside class="ada-sidebar" id="chat-sidebar" aria-label="侧栏">{}{}</aside>"#,
            self.full(),
            self.rail()
        )
    }

    // ── 展开态 ───────────────────────────────────────────────────────────

    fn full(&self) -> String {
        let mut out = String::with_capacity(2048);
        out.push_str(r#"<div class="ada-side">"#);
        out.push_str(&self.brand());
        out.push_str(&self.sidebar_top);
        out.push_str(&self.new_thread_form("ada-btn ada-btn--secondary ada-btn--sm", false));
        out.push_str(&self.workspace_switch());
        if self.search {
            out.push_str(&self.search_box());
        }
        out.push_str(&self.list());
        out.push_str(&self.util_nav());
        out.push_str(&self.sidebar_bottom);
        out.push_str(&self.foot());
        out.push_str("</div>");
        out
    }

    /// 品牌位：对话页是品牌（a_da）；二级页同一位置改成「返回会话」——
    /// 它本来就链接回对话，但读作 a_da 时没有任何"点我会回去"的示意。
    fn brand(&self) -> String {
        let mark = |icon_key: &str| {
            format!(r#"<span class="ada-brand__mark">{}</span>"#, icon(icon_key, 18))
        };
        if self.active == "chat" {
            format!(
                r#"<a href="/" class="ada-brand" aria-label="返回对话"><span class="ada-brand__mark ada-brand__mark--logo"><img class="ada-brand__logo" src="/static/logo.png" alt="" width="28" height="28" decoding="async"></span><span class="ada-brand__name">a_da</span><span class="ada-brand__badge">SSR</span></a>"#
            )
        } else {
            format!(
                r#"<a href="/" class="ada-brand ada-brand--back" aria-label="返回会话" title="返回会话">{}<span class="ada-brand__name">返回会话</span><span class="ada-brand__badge">SSR</span></a>"#,
                mark("i-arrow-left")
            )
        }
    }

    fn new_thread_form(&self, button_class: &str, icon_only: bool) -> String {
        let workspace = if self.submit_workspace.is_empty() {
            String::new()
        } else {
            format!(
                r#"<input type="hidden" name="workspace" value="{}">"#,
                escape(&self.submit_workspace)
            )
        };
        let body = if icon_only {
            icon("i-plus", 14)
        } else {
            format!("{}<span>新会话</span>", icon("i-plus", 14))
        };
        format!(
            r#"<form class="{}" method="post" action="/threads"><input type="hidden" name="title" value="新会话">{workspace}<button class="{button_class}" type="submit"{title}>{body}</button></form>"#,
            if icon_only {
                "ada-rail__new"
            } else {
                "ada-new"
            },
            title = if icon_only {
                attr("title", "新会话")
            } else {
                String::new()
            },
        )
    }

    /// 一行工作区切换器：`图标 + 目录名 + 会话数 + 箭头`。
    ///
    /// 取代原先的"分区标题 + 全路径药丸 + 组头当前徽标"三处表达——同一个状态
    /// 说三遍，占掉约 100px 却不增加信息。完整路径进 `title`。
    fn workspace_switch(&self) -> String {
        let count: usize = self.groups.iter().map(|group| group.count).sum();
        let count_html = if count == 0 {
            String::new()
        } else {
            format!(r#"<span class="ada-ws-switch__count">{count}</span>"#)
        };
        format!(
            r#"<a class="ada-ws-switch" href="/workspaces"{title}>{folder}<span class="ada-ws-switch__name">{name}</span>{count_html}{caret}</a>"#,
            title = attr(
                "title",
                &format!("当前工作区：{}（点击切换）", self.workspace_path)
            ),
            folder = icon("i-folder", 14),
            name = escape(&self.workspace_name),
            caret = icon("i-chevron-down", 12),
        )
    }

    fn search_box(&self) -> String {
        format!(
            r#"<div class="ada-search">{icon}<input id="chat-thread-search" class="ada-search__input" type="search" placeholder="搜索会话…" autocomplete="off" aria-label="搜索会话"><kbd class="ada-search__kbd">Ctrl K</kbd><span class="ada-search__count" hidden></span></div>"#,
            icon = icon("i-search", 14),
        )
    }

    /// 会话列表：整条侧栏唯一的滚动区。
    fn list(&self) -> String {
        format!(
            r#"<div class="ada-thread-list" id="thread-list">{}</div>"#,
            Self::thread_list_inner(&self.groups, &self.active_thread_id)
        )
    }

    /// `#thread-list` 的 innerHTML：树 + 空态提示。
    ///
    /// 这一段是 **live parts 的增量换新单元**（chat.js 的 `threads` 目标整块替换
    /// innerHTML）：其它客户端的新会话/改名/运行点不再要手动刷新。空态提示必须
    /// 一起进这里——搜索 JS 按 id 找它，换帧后引用由 apply() 现查。
    pub fn thread_list_inner(groups: &[WorkspaceGroup], active_thread_id: &str) -> String {
        format!(
            "{}{}",
            Self::render_tree(groups, active_thread_id),
            r#"<p class="ada-list-empty" id="thread-empty" hidden>没有匹配的会话 <button type="button" class="ada-list-empty__clear">清除</button></p>"#,
        )
    }

    /// 会话树：当前工作区的会话直接铺开（切换器就是它的表头），
    /// 其它工作区收成可折叠分组。只有 1 个工作区时不会出现任何组头。
    fn render_tree(groups: &[WorkspaceGroup], active_thread_id: &str) -> String {
        if groups.is_empty() {
            return r#"<p class="ada-list-empty">还没有会话</p>"#.to_string();
        }
        // 当前工作区排最前，且不带组头——否则"中间夹一个无头分组"会失去归属。
        let head = groups.iter().find(|group| group.active).or_else(|| groups.first());
        let mut out = String::with_capacity(1024);
        if let Some(head) = head {
            if head.threads.is_empty() {
                out.push_str(r#"<p class="ada-ws__empty">暂无会话</p>"#);
            } else {
                for thread in &head.threads {
                    out.push_str(&Self::render_thread_row(thread, active_thread_id));
                }
            }
        }
        for group in groups.iter().filter(|group| !group.active) {
            out.push_str(&Self::render_other_group(group, active_thread_id));
        }
        out
    }

    fn render_thread_row(thread: &ThreadRow, active_thread_id: &str) -> String {
        let active = !active_thread_id.is_empty() && thread.id == active_thread_id;
        let classes = class_list(&["ada-thread", if active { "is-active" } else { "" }]);
        // 条目数不进可见行（它几乎不被读，却占掉整行 30% 的高度），只留在 title 里。
        let hint = format!("{} · {} 项", thread.title, thread.item_count);
        let dot = if thread.running {
            r#"<span class="ada-thread__dot" title="运行中" aria-label="运行中"></span>"#
                .to_string()
        } else {
            String::new()
        };
        // 回跳目标 = 提交时所在的会话（删的就是它时由服务端回首页）。
        format!(
            r#"<div class="{classes}"><a class="ada-thread__link" href="/threads/{id}"{hint}><span class="ada-thread__title">{title}</span>{dot}</a><span class="ada-thread__actions"><details class="ada-thread__rename"><summary title="重命名会话" aria-label="重命名会话">{edit}</summary><form class="ada-thread__rename-form" method="post" action="/threads/{id}/rename"><input type="hidden" name="back" value="{back}"><input class="ada-input ada-focusable" name="title" value="{title}" maxlength="200" aria-label="会话标题" placeholder="会话标题"><button class="ada-btn ada-btn--sm ada-btn--primary" type="submit">保存</button></form></details><form method="post" action="/threads/{id}/delete" data-confirm="删除会话「{title}」？会话历史将一并删除，不可恢复。"><input type="hidden" name="back" value="{back}"><button class="ada-thread__action" type="submit" title="删除会话" aria-label="删除会话">{trash}</button></form></span></div>"#,
            id = escape(&thread.id),
            hint = attr("title", &hint),
            title = escape(&thread.title),
            back = escape(active_thread_id),
            edit = icon("i-edit", 12),
            trash = icon("i-trash", 12),
        )
    }

    /// 组头 `<details>` 的稳定 DOM id：key 里有盘符/斜杠不能直接当 id，
    /// FNV-1a 32 位足够——live 换帧靠它还原展开状态（replaceKeepingOpen）。
    fn ws_dom_id(key: &str) -> String {
        let mut hash: u32 = 0x811c_9dc5;
        for byte in key.as_bytes() {
            hash ^= u32::from(*byte);
            hash = hash.wrapping_mul(0x0100_0193);
        }
        format!("ws-{hash:08x}")
    }

    fn render_other_group(group: &WorkspaceGroup, active_thread_id: &str) -> String {
        let mut threads = String::new();
        if group.threads.is_empty() {
            threads.push_str(r#"<p class="ada-ws__empty">暂无会话</p>"#);
        } else {
            for thread in &group.threads {
                threads.push_str(&Self::render_thread_row(thread, active_thread_id));
            }
        }
        let running = if group.running {
            r#"<span class="ada-ws__running" title="该工作区有会话正在运行"><span class="ada-dot ada-dot--warn"></span></span>"#.to_string()
        } else {
            String::new()
        };
        format!(
            r#"<div class="ada-ws" data-ws-key="{key}" data-ws-path="{path_attr}"><details class="ada-ws__details" id="{dom_id}"><summary class="ada-ws__summary"{path}>{chevron}{folder}<span class="ada-ws__name">{name}</span>{running}<span class="ada-ws__count">{count}</span></summary><div class="ada-ws__threads">{threads}</div></details><div class="ada-ws__actions"><form method="post" action="/threads"><input type="hidden" name="title" value="新会话"><input type="hidden" name="workspace" value="{path_raw}"><button class="ada-ws__action" type="submit" title="在此工作区新建对话" aria-label="在此工作区新建对话">{plus}</button></form><form method="post" action="/workspaces/select"><input type="hidden" name="path" value="{path_raw}"><button class="ada-ws__action" type="submit" title="切换到该工作区（会设为当前项目）" aria-label="切换到该工作区">{check}</button></form></div></div>"#,
            key = escape(&group.key),
            path_attr = escape(&group.path),
            dom_id = Self::ws_dom_id(&group.key),
            path = attr("title", &group.path),
            chevron = icon("i-chevron", 12),
            folder = icon("i-folder", 14),
            name = escape(&group.name),
            count = group.count,
            threads = threads,
            path_raw = escape(&group.path),
            plus = icon("i-plus", 12),
            check = icon("i-check", 12),
        )
    }

    /// 系统导航：插件链接 + 插件 / 概览。
    ///
    /// 这些是低频设置面，放在底部是已经被训练出来的位置；同时把顶部让给会话列表。
    fn util_nav(&self) -> String {
        let mut out = String::with_capacity(512);
        out.push_str(r#"<nav class="ada-nav ada-nav--util" aria-label="系统">"#);
        if !self.plugin_nav.is_empty() {
            out.push_str(r#"<div class="ada-nav__label"><span>前端插件</span></div>"#);
            for link in self.plugin_nav.iter().take(PLUGIN_LINK_LIMIT) {
                // 插件页的 active 是 `ext:{plugin_id}`（见 plugin_page）——
                // 点开哪个插件，哪个链接就要亮，否则点了像没点上。
                let active = self.active == format!("ext:{}", link.plugin_id);
                let classes = class_list(&[
                    "ada-nav__item",
                    if active { "is-active" } else { "" },
                ]);
                let current = if active {
                    r#" aria-current="page""#.to_string()
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    r#"<a class="{classes}" href="{href}"{current}{title}>{icon}<span class="ada-nav__text">{label}</span></a>"#,
                    href = escape(&link.href),
                    title = attr("title", &link.plugin_id),
                    icon = icon("i-plug", 14),
                    label = escape(&link.label),
                ));
            }
            let rest = self.plugin_nav.len().saturating_sub(PLUGIN_LINK_LIMIT);
            if rest > 0 {
                out.push_str(&format!(
                    r#"<a class="ada-nav__item ada-nav__item--more" href="/extensions">还有 {rest} 个…</a>"#
                ));
            }
        }
        out.push_str(&self.nav_item("extensions", "i-plug", "插件", "/extensions"));
        out.push_str(&self.nav_item("dashboard", "i-chart", "概览", "/overview"));
        out.push_str("</nav>");
        out
    }

    fn nav_item(&self, key: &str, icon_key: &str, label: &str, href: &str) -> String {
        let active = self.active == key;
        let classes = class_list(&["ada-nav__item", if active { "is-active" } else { "" }]);
        let current = if active {
            r#" aria-current="page""#.to_string()
        } else {
            String::new()
        };
        format!(
            r#"<a class="{classes}" href="{href}"{current}>{icon}<span class="ada-nav__text">{label}</span></a>"#,
            icon = icon(icon_key, 14),
            label = escape(label),
        )
    }

    fn foot(&self) -> String {
        let (dot, status) = match self.connected {
            Some(true) => (
                r#"<span class="ada-gateway__dot is-on"></span>"#.to_string(),
                "已连接".to_string(),
            ),
            Some(false) => (
                r#"<span class="ada-gateway__dot is-off"></span>"#.to_string(),
                "未连接".to_string(),
            ),
            None => (String::new(), String::new()),
        };
        let title = if status.is_empty() {
            self.gateway.clone()
        } else {
            format!("{status} · {}", self.gateway)
        };
        format!(
            r#"<div class="ada-sidebar__foot"><button class="ada-icon-btn ada-theme" type="button" data-theme-toggle title="切换深浅色" aria-label="切换深浅色"><span class="ada-theme__moon">{moon}</span><span class="ada-theme__sun">{sun}</span></button><span class="ada-gateway"{title}>{dot}<span class="ada-gateway__addr">{addr}</span></span></div>"#,
            moon = icon("i-moon", 14),
            sun = icon("i-sun", 14),
            title = attr("title", &title),
            addr = escape(&display_address(&self.gateway)),
        )
    }

    // ── 折叠窄轨 ─────────────────────────────────────────────────────────
    //
    // 收起后不再是 `display:none`：品牌、新建、最近的会话、系统页都还在，
    // 用户不需要为了跳一条会话把侧栏重新展开。

    fn rail(&self) -> String {
        let mut out = String::with_capacity(1024);
        out.push_str(r#"<div class="ada-rail">"#);
        out.push_str(&format!(
            r#"<a class="ada-rail__mark" href="/" title="{rail_title}" aria-label="{rail_title}">{}</a>"#,
            if self.active == "chat" {
                r#"<img class="ada-rail__logo" src="/static/logo.png" alt="" width="24" height="24" decoding="async">"#
                    .to_string()
            } else {
                icon("i-diamond", 18)
            },
            rail_title = if self.active == "chat" {
                "a_da · 返回对话"
            } else {
                "返回会话"
            },
        ));
        out.push_str(&self.new_thread_form("ada-rail__btn ada-rail__btn--primary", true));
        // 窄轨里的搜索：点它先展开侧栏再聚焦输入框（见 chat.js）。
        // 少了这一颗，收起侧栏就等于把"找会话"这条路径断掉。
        if self.search {
            out.push_str(&format!(
                r#"<button class="ada-rail__btn" type="button" data-rail-search title="搜索会话（Ctrl K）" aria-label="搜索会话">{}</button>"#,
                icon("i-search", 18)
            ));
        }
        let threads = self.recent_threads();
        if !threads.is_empty() {
            out.push_str(r#"<span class="ada-rail__sep"></span>"#);
            for thread in threads {
                out.push_str(&self.rail_thumb(thread));
            }
        }
        out.push_str(r#"<span class="ada-rail__spacer"></span>"#);
        out.push_str(&self.rail_link("extensions", "i-plug", "插件", "/extensions"));
        out.push_str(&self.rail_link("dashboard", "i-chart", "概览", "/overview"));
        out.push_str(&format!(
            r#"<button class="ada-rail__btn" type="button" data-theme-toggle title="切换深浅色" aria-label="切换深浅色">{}</button>"#,
            icon("i-moon", 14)
        ));
        out.push_str("</div>");
        out
    }

    fn recent_threads(&self) -> Vec<&ThreadRow> {
        let head = self
            .groups
            .iter()
            .find(|group| group.active)
            .or_else(|| self.groups.first());
        head.map(|group| {
            group
                .threads
                .iter()
                .take(RAIL_THREAD_LIMIT)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
    }

    fn rail_thumb(&self, thread: &ThreadRow) -> String {
        let active = !self.active_thread_id.is_empty() && thread.id == self.active_thread_id;
        let classes = class_list(&["ada-rail__thumb", if active { "is-active" } else { "" }]);
        // 没有真头像可放：取标题首字当锚点，完整标题进 title。
        let initial = thread
            .title
            .trim()
            .chars()
            .next()
            .map(|ch| ch.to_string())
            .unwrap_or_else(|| "·".to_string());
        format!(
            r#"<a class="{classes}" href="/threads/{id}"{title}>{initial}</a>"#,
            id = escape(&thread.id),
            title = attr("title", &thread.title),
            initial = escape(&initial),
        )
    }

    fn rail_link(&self, key: &str, icon_key: &str, label: &str, href: &str) -> String {
        let classes = class_list(&[
            "ada-rail__btn",
            if self.active == key { "is-active" } else { "" },
        ]);
        format!(
            r#"<a class="{classes}" href="{href}"{title}>{icon}</a>"#,
            title = attr("title", label),
            icon = icon(icon_key, 18),
        )
    }
}

/// `http://127.0.0.1:52353` → `127.0.0.1:52353`：窄栏里的等宽地址不该把协议头
/// 也算成"信息"，它只会把有用的端口挤出可视区。
fn display_address(gateway: &str) -> String {
    let trimmed = gateway.trim();
    for scheme in [
        "http://", "https://", "ws://", "wss://", "pipe://", "unix://",
    ] {
        if let Some(rest) = trimmed.strip_prefix(scheme) {
            return rest.trim_end_matches('/').to_string();
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(name: &str, active: bool, threads: Vec<ThreadRow>) -> WorkspaceGroup {
        WorkspaceGroup {
            key: name.to_string(),
            path: format!("C:\\work\\{name}"),
            name: name.to_string(),
            count: threads.len(),
            running: threads.iter().any(|thread| thread.running),
            active,
            threads,
        }
    }

    fn thread(id: &str, title: &str) -> ThreadRow {
        ThreadRow {
            id: id.to_string(),
            title: title.to_string(),
            workspace: "C:\\work\\app".to_string(),
            mode: "craft".to_string(),
            running: false,
            item_count: 12,
            created_at: 1,
        }
    }

    fn sample() -> Sidebar {
        Sidebar {
            active: "chat".to_string(),
            workspace_name: "app".to_string(),
            workspace_path: "C:\\work\\app".to_string(),
            gateway: "http://127.0.0.1:52353".to_string(),
            submit_workspace: "C:\\work\\app".to_string(),
            groups: vec![
                group(
                    "app",
                    true,
                    vec![thread("t1", "写一个侧栏"), thread("t2", "排查管道")],
                ),
                group("other", false, vec![thread("t3", "别的会话")]),
            ],
            active_thread_id: "t2".to_string(),
            plugin_nav: vec![
                NavLink {
                    plugin_id: "hello".to_string(),
                    label: "Hello 插件".to_string(),
                    href: "/p/hello".to_string(),
                },
                NavLink {
                    plugin_id: "weather".to_string(),
                    label: "天气".to_string(),
                    href: "/p/weather".to_string(),
                },
                NavLink {
                    plugin_id: "chart".to_string(),
                    label: "图表".to_string(),
                    href: "/p/chart".to_string(),
                },
                NavLink {
                    plugin_id: "fourth".to_string(),
                    label: "第四个".to_string(),
                    href: "/p/fourth".to_string(),
                },
            ],
            sidebar_top: String::new(),
            sidebar_bottom: String::new(),
            search: true,
            connected: Some(true),
        }
    }

    #[test]
    fn sidebar_renders_one_scroll_region_and_a_real_new_thread_form() {
        let html = sample().render();
        assert_eq!(
            html.matches("class=\"ada-thread-list\"").count(),
            1,
            "会话列表必须是唯一的滚动区"
        );
        assert!(html.contains(r#"method="post" action="/threads""#));
        assert!(
            html.contains(r#"<input type="hidden" name="workspace" value="C:\work\app">"#),
            "新会话要落在当前工作区"
        );
    }

    #[test]
    fn active_workspace_has_no_group_header() {
        let html = sample().render();
        // 当前工作区的会话直接铺开：切换器就是它的表头，不再出现"当前"徽标。
        assert!(
            !html.contains("ada-ws__badge"),
            "当前工作区不该有第二处标记"
        );
        assert!(html.contains(r#"href="/threads/t1""#));
        // 其它工作区仍然可折叠，并保留两个组头操作。
        assert!(html.contains("data-ws-key=\"other\""));
        assert!(html.contains(r#"action="/workspaces/select""#));
    }

    #[test]
    fn done_threads_keep_their_count_in_the_title_only() {
        let html = sample().render();
        assert!(
            html.contains("写一个侧栏 · 12 项"),
            "条目数只留在 title 里，不进可见行"
        );
        assert!(!html.contains(">12 项<"), "可见行里不该再出现条目数");
    }

    #[test]
    fn rail_stays_usable_when_collapsed() {
        let html = sample().render();
        assert!(html.contains(r#"class="ada-rail""#));
        assert!(
            html.contains("class=\"ada-rail__thumb\""),
            "收起后仍要能跳最近会话"
        );
        assert!(!html.contains(r#"href="/providers""#), "供应商已插件化，核心入口应移除");
        assert!(html.contains(r#"href="/extensions""#), "窄轨保留系统导航");
    }

    /// 二级页的品牌位 = 返回会话（带左箭头），对话页保持品牌原样。
    #[test]
    fn brand_reads_back_to_thread_on_secondary_pages() {
        let mut sidebar = sample();
        sidebar.active = "dashboard".to_string();
        let html = sidebar.render();
        assert!(html.contains("ada-brand--back"), "{html}");
        assert!(html.contains("返回会话"), "{html}");
        // 图标名不进输出，箭头以 path 形态存在
        assert!(html.contains(r#"d="m12 19-7-7 7-7""#), "{html}");
        assert!(!html.contains(r#"<span class="ada-brand__name">a_da</span>"#), "{html}");

        let chat = sample().render();
        assert!(chat.contains(r#"<span class="ada-brand__name">a_da</span>"#), "{chat}");
        assert!(!chat.contains("ada-brand--back"), "{chat}");
    }

    #[test]
    fn gateway_drops_the_scheme_but_keeps_the_full_uri_in_title() {
        assert_eq!(
            display_address("http://127.0.0.1:52353/"),
            "127.0.0.1:52353"
        );
        assert_eq!(display_address("pipe://\\\\.\\pipe\\ada"), r"\\.\pipe\ada");
        let html = sample().render();
        assert!(html.contains(">127.0.0.1:52353<"));
        assert!(html.contains("title=\"已连接 · http://127.0.0.1:52353\""));
    }

    #[test]
    fn search_count_and_empty_hint_are_present_for_js_progressive_enhancement() {
        let html = sample().render();
        assert!(html.contains(r#"id="chat-thread-search""#));
        assert!(html.contains(r#"class="ada-search__count" hidden"#));
        assert!(html.contains(r#"id="thread-empty" hidden"#));
        // 没有 JS 时只剩输入框，不会出现"0/0"这种假计数。
        assert!(!html.contains(">0/0<"));
    }

    #[test]
    fn pages_without_a_list_do_not_render_a_dead_search_box() {
        let mut sidebar = sample();
        sidebar.groups.clear();
        sidebar.search = false;
        let html = sidebar.render();
        assert!(!html.contains("chat-thread-search"));
        assert!(html.contains("还没有会话"));
    }

    #[test]
    fn thread_rows_carry_inline_rename_and_delete_actions() {
        let html = sample().render();
        assert!(html.contains("action=\"/threads/t1/rename\""), "{html}");
        assert!(html.contains("action=\"/threads/t1/delete\""), "{html}");
        // 删除必须过确认弹窗；回跳目标是当前会话（sample 的 active 是 t2）
        assert!(html.contains("data-confirm"), "{html}");
        assert!(
            html.contains(r#"<input type="hidden" name="back" value="t2">"#),
            "{html}"
        );
    }

    #[test]
    fn workspace_groups_get_stable_details_ids_and_path_attributes() {
        let html = sample().render();
        assert!(
            html.contains(r#"data-ws-path="C:\work\other""#),
            "搜索要能匹配完整路径: {html}"
        );
        let dom_id = Sidebar::ws_dom_id("other");
        assert!(html.contains(&format!(r#"id="{dom_id}""#)), "{html}");
    }

    #[test]
    fn new_thread_is_a_plain_post_and_modal_is_gone() {
        // 选择挪到了空会话页（模式 + 工作区），侧栏不再渲染弹窗、也不再被 JS 拦截：
        // 点「新会话」直接建一条空会话，落到当前（或组内）工作区。
        let html = sample().render();
        assert!(!html.contains(r#"data-modal="new-thread""#), "{html}");
        assert!(!html.contains("data-new-thread"), "{html}");
        assert!(html.contains(r#"method="post" action="/threads""#), "{html}");
    }

    #[test]
    fn pages_without_groups_keep_direct_new_thread_form() {
        let mut sidebar = sample();
        sidebar.groups.clear();
        let html = sidebar.render();
        assert!(!html.contains(r#"data-modal="new-thread""#), "{html}");
        assert!(html.contains(r#"method="post" action="/threads""#), "{html}");
    }

    use super::*;

    /// 点开哪个插件，哪个导航链接就要亮（active 是 plugin_page 传的 ext:{id}），
    /// 且「插件」项不该跟着亮——它只代表 /extensions 页本身。
    #[test]
    fn plugin_nav_link_is_active_on_its_own_plugin_page() {
        let mut sidebar = sample();
        sidebar.active = "ext:hello".to_string();
        let html = sidebar.render();
        // hello 的链接有选中态 + aria-current
        let hello_index = html.find(r#"href="/p/hello""#).expect("hello link");
        let row = &html[hello_index.saturating_sub(200)..html.len().min(hello_index + 200)];
        assert!(row.contains("is-active"), "{row}");
        assert!(row.contains("aria-current"), "{row}");
        // 「插件」（/extensions）不亮：插件页不是扩展列表页
        let ext_index = html.find(r#"href="/extensions""#).expect("extensions link");
        let row = &html[ext_index.saturating_sub(160)..html.len().min(ext_index + 60)];
        assert!(
            !row.contains(r#"class="ada-nav__item is-active""#),
            "插件页不该点亮「插件」项: {row}"
        );
    }

    /// 聊天页所有插件链接都不亮。
    #[test]
    fn plugin_nav_links_are_plain_on_chat_page() {
        let sidebar = sample();
        let html = sidebar.render();
        let hello_index = html.find(r#"href="/p/hello""#).expect("hello link");
        let row = &html[hello_index.saturating_sub(200)..html.len().min(hello_index + 200)];
        assert!(!row.contains("is-active"), "{row}");
    }

}
