//! `create_plugin`：生成**可加载的插件包骨架**（"创建插件的插件"的实现）。
//!
//! # 为什么要有它
//!
//! 手写插件包最容易死在三处，全都不是"不会写 TypeScript"：
//!
//! 1. **目录层级**：只有 `<root>/<id>/index.ts` 会被发现（`PluginManager::scan_plugins`
//!    只扫包根），散落的 `foo.ts` 永远不生效，而界面只说"没找到插件"；
//! 2. **metadata 与注册项对不上**：`count` / `names` 是**量出来的**，多注册一个工具
//!    忘了同步就直接装载失败（`validate_package_metadata`）；
//! 3. **能力声明**：`// @ada-needs` 一写就是"只有列出的那些"，`net` 不附带 `fs`。
//!
//! 所以这里给的不是"空模板"，而是**已经通过装载期校验的骨架**：一个工具、一段提示词、
//! 一个技能，metadata 与之逐项对齐，作者只需要替换 `execute` 里的逻辑。
//!
//! 落点由 `scope` 决定（**必填**，见 [`ASK_SCOPE_MESSAGE`]）：
//! - `workspace` → `<ws>/.ada/agent-coding/extensions/<id>/`（与
//!   `PluginManager::agent_extensions_dir` 同一口径）；
//! - `global` → `<app_home>/agent-coding/extensions/<id>/`（对所有工程生效）。
//!
//! 应用目录由**组合根注入**（`ToolkitEnv::app_home`），工具自己不读环境变量：
//! 自己读就会在测试里写到用户真实目录（本项目为这类污染出过事故）。

use std::fs;
use std::path::{Path, PathBuf};

use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt, ToolStatus};
use agent_base::ports::{BoxFuture, Tool, ToolContext};

use crate::fs_tools::ToolResult;

/// 用户没说清楚要装在哪儿时，工具**不做猜测**给出的回复。
///
/// 为什么是"拒绝 + 指路"而不是默认工作区：插件装到哪儿决定了它的**生命周期**
/// （工作区插件随项目走、全局插件对所有工程生效）。猜错的代价是用户在一个工程里
/// 调好的 AI 行为，在另一个工程里莫名其妙地出现/消失——而且他根本不知道是哪次操作造成的。
pub const ASK_SCOPE_MESSAGE: &str =
    "⚠️ 还没确定这个插件装在哪，**不要自己猜**。请先用 `ask_user` 工具问用户：\n\
     · **工作区插件**（scope=\"workspace\"）——只在这个工程生效，目录随项目版本库走；\n\
     · **全局插件**（scope=\"global\"）——对本机**所有**工程生效。\n\
     拿到用户的选择后，再调用 `create_plugin` 并带上 scope。\n\
     （如果用户在这条消息里已经明确说了\"全局\"或\"这个项目/工作区\"，直接带上对应 scope 调用即可。）";

/// 工作区内的插件根。口径必须与 `PluginManager::agent_extensions_dir` 一致。
pub fn workspace_extensions_dir(workspace: &Path) -> PathBuf {
    workspace
        .join(".ada")
        .join("agent-coding")
        .join("extensions")
}

/// `create_plugin` 的入参。
pub struct ScaffoldRequest<'a> {
    pub id: &'a str,
    /// `workspace` / `global`（**必填**，缺了就回 [`ASK_SCOPE_MESSAGE`]）。
    pub scope: &'a str,
    pub name: Option<&'a str>,
    pub description: Option<&'a str>,
    /// 示例工具名（默认 `<id>_hello`）
    pub tool: Option<&'a str>,
    pub overwrite: bool,
}

/// 生成插件包。
///
/// `app_home` 只在 `scope = "global"` 时用；`None` 表示这个装配面没有应用目录
/// （测试 / 子体 / 合规套件），此时全局插件**拒绝创建**并说明原因，绝不猜路径。
pub fn scaffold_plugin(
    workspace: &Path,
    app_home: Option<&Path>,
    req: ScaffoldRequest<'_>,
) -> ToolResult {
    let id = req.id.trim();
    if let Err(e) = validate_id(id) {
        return ToolResult::error(e);
    }

    // 🔴 作用域是**必填**：用户没说清就先问，不要替他把插件装到某个地方。
    let (root, scope) = match req.scope.trim() {
        "" => return ToolResult::error(ASK_SCOPE_MESSAGE),
        "workspace" => (workspace_extensions_dir(workspace), "workspace"),
        "global" => match app_home {
            Some(home) => (home.join("agent-coding").join("extensions"), "global"),
            None => {
                return ToolResult::error(
                    "当前装配面没有注入应用目录（`~/.a-da`），无法创建全局插件。\
                     请改用 scope=\"workspace\"，或让宿主在装配工具包时注入应用目录。",
                )
            }
        },
        other => {
            return ToolResult::error(format!(
                "scope `{other}` 不合法：只能是 \"workspace\"（只在本工程生效）或 \
                 \"global\"（对本机所有工程生效）。"
            ))
        }
    };
    let display = req.name.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(id);
    let desc = req
        .description
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("（一句话说明这个插件做什么）");
    let tool = match req.tool.map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => t.to_string(),
        None => format!("{}_hello", id.replace('-', "_")),
    };
    if let Err(e) = validate_tool_name(&tool) {
        return ToolResult::error(e);
    }

    let dir = root.join(id);
    if dir.exists() && !req.overwrite {
        return ToolResult::error(format!(
            "插件目录已存在：{}（要覆盖生成的文件请传 overwrite=true；不会动你自己的其它文件）",
            dir.display()
        ));
    }

    // 技能目录：`skills/<name>/`（规范：一个 Skill = 一个目录 + SKILL.md）
    let skill_dir = dir.join("skills").join(id);
    if let Err(e) = fs::create_dir_all(&skill_dir) {
        return ToolResult::error(format!("创建插件目录失败（{}）：{}", skill_dir.display(), e));
    }

    let files: Vec<(PathBuf, String)> = vec![
        (dir.join("index.ts"), render_index_ts(id, display, desc, &tool)),
        (dir.join("README.md"), render_readme(id, scope, display, desc)),
        (skill_dir.join("SKILL.md"), render_skill_md(id, display, desc)),
    ];

    let mut written = Vec::new();
    for (path, content) in files {
        if let Err(e) = fs::write(&path, content) {
            return ToolResult::error(format!("写入 {} 失败：{}", path.display(), e));
        }
        written.push(rel(workspace, &path));
    }

    let mut output = format!("已生成插件 `{id}`（{} 个文件，{scope} 作用域）：\n", written.len());
    for p in &written {
        output.push_str(&format!("  {p}\n"));
    }
    output.push_str(&format!(
        "\n插件标识：`{scope}:{id}`（目录名就是标识，重命名目录会变成另一个插件）\n\
         \n下一步：\n\
         1. 在「插件与能力中心」确认它处于启用状态（新建插件默认启用）；\n\
         2. 宿主会在增删后**立刻重装配**；如果这是手改 index.ts / SKILL.md，点插件页的「重新加载」即可生效；\n\
         3. 增删工具/提示词/钩子要同步 metadata 的 count/names，否则装载期会拒（卡片显示 broken + 原因）；\n\
         4. 技能正文在 `skills/{id}/SKILL.md`：frontmatter 的 `name` 必须等于目录名 `{id}`，\
         `description` 要写清「做什么 + 什么时候用」（Agent Skills 规范见 docs/SKILL_SPEC.md）。"
    ));
    if scope == "workspace" {
        output.push_str(&format!(
            "\n         5. 想让它对本机所有工程生效：改用 scope=\"global\" 重新生成，\
             或把整个目录复制到 ~/.a-da/agent-coding/extensions/{id}/。"
        ));
    }
    output.push_str(&format!(
        "\n\n示例工具：`{tool}` —— 现在只会回显入参，把 execute 换成你的逻辑即可。"
    ));

    ToolResult {
        ok: true,
        output,
        patch: None,
        details: Some(serde_json::json!({
            "id": id,
            "scope": scope,
            "pluginId": format!("{scope}:{id}"),
            "dir": dir.to_string_lossy(),
            "tool": tool,
            "files": written,
        })),
        terminate: None,
        failure: None,
    }
}

/// 目录名既要当标识又要当路径：只放行小写字母/数字/连字符。
fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("缺少 id：用 kebab-case 命名，例如 todo-guard（目录名就是插件标识）。".to_string());
    }
    if id.chars().count() > 48 {
        return Err(format!("id 太长（{} 字符）：上限 48。", id.chars().count()));
    }
    if id == "index" || id.starts_with('.') {
        return Err(format!("id `{id}` 不可用：不能以点开头，也不能叫 index。"));
    }
    let legal = id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !legal || id.starts_with('-') || id.ends_with('-') || id.contains("--") {
        return Err(format!(
            "id `{id}` 不合法：只允许小写字母、数字与连字符，且不能以连字符开头/结尾或连续出现（目录名就是插件标识）。"
        ));
    }
    Ok(())
}

/// 工具名会被模型直接调用：按标识符规则收窄，避免空格/中文导致它永远调不到。
fn validate_tool_name(name: &str) -> Result<(), String> {
    let legal = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !name.starts_with('_')
        && !name.is_empty();
    if !legal {
        return Err(format!(
            "工具名 `{name}` 不合法：只允许小写字母、数字与下划线，且不能以下划线开头。"
        ));
    }
    Ok(())
}

fn rel(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

/// 转成 TS 单引号字符串字面量（`\n` 用转义写法，便于排版）。
fn ts_str(text: &str) -> String {
    let mut out = String::from("'");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// 生成 `skills/<id>/SKILL.md`（Agent Skills 规范：`docs/SKILL_SPEC.md`）。
///
/// # 为什么技能是"目录 + SKILL.md"
///
/// 规范定死了一件事：一个 Skill 是一个**目录**，入口必须是 `SKILL.md`，frontmatter 里
/// `name` 必须等于目录名。这不是排版偏好——`references/`、`scripts/`、`assets/` 的
/// 相对根就是技能目录；技能一旦变成"一个散装 .md"，这些配套文件就没有确定位置了。
///
/// 2026-10-19 起插件技能统一走这个形态，旧的 `registerSkill({ body })` 已移除。
fn render_skill_md(id: &str, display: &str, desc: &str) -> String {
    format!(
        "---\n\
         name: {id}\n\
         description: {desc_yaml}\n\
         ---\n\
         \n\
         # {display}\n\
         \n\
         ## 什么时候用\n\
         \n\
         <写清**可判定**的触发条件：用户说什么、代码里出现什么，才该用这个插件。「需要时」这种话等于没写>\n\
         \n\
         ## 步骤\n\
         \n\
         1. 先确认输入：<需要哪些参数、从哪来>\n\
         2. 调用 `{id}_hello` 完成主流程（或换成你自己的工具名）\n\
         3. 校验返回值，再把结果交给用户\n\
         \n\
         ## 示例\n\
         \n\
         ```text\n\
         输入：text = \"ping\"\n\
         输出：收到：ping\n\
         ```\n\
         \n\
         ## 边缘情况\n\
         \n\
         - 输入为空 → 返回明确的失败说明，不要猜一个默认值。\n\
         - 调用失败 → 如实报错（带上错误原文），不要静默重试超过一次。\n\
         \n\
         ## 联网与代理（用到 fetch 时）\n\
         \n\
         - 先在 `index.ts` 首部声明 `// @ada-needs net`，否则沙箱里没有 `fetch`。\n\
         - 请求级代理：`fetch(url, {{ proxy: 'http://127.0.0.1:7897' }})`（`127.0.0.1:7897` 不带 scheme 也认）。\n\
         - 进程级代理：宿主设 `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY`（大小写都认）。\n\
         - 代理配错会**直接报错**，不会静默改直连。\n\
         \n\
         > 这段正文会在技能被激活时整份加载（建议 < 500 行）。较长的参考资料放到\n\
         > `references/` 下按需读取，不要全塞进来。\n",
        id = id,
        display = display,
        desc_yaml = yaml_scalar(&skill_description(display, desc)),
    )
}

/// 技能 `description`：规范要求 1-1024 字符，写清"做什么 + 什么时候用"。
fn skill_description(display: &str, desc: &str) -> String {
    let base = desc.trim().trim_end_matches('。');
    let text = format!(
        "{base}。用于「{display}」插件：说明什么时候该用它、按什么步骤做、失败时怎么办。"
    );
    let mut out: String = text.chars().take(900).collect();
    if text.chars().count() > 900 {
        out.push('…');
    }
    out
}

/// YAML 标量：整体加双引号，内部换行压成空格、双引号改单引号。
///
/// 为什么不用 YAML 库：frontmatter 只写一个 `description`，而引入完整 YAML 依赖
/// 只为了转义一行字符串不划算。这里的输出由同仓的 `skills::spec` 解析器读取，
/// 两边口径一致（都只认"一层键值 + 双引号去壳"）。
fn yaml_scalar(s: &str) -> String {
    let cleaned = s
        .replace(['\n', '\r'], " ")
        .replace('"', "'")
        .trim()
        .to_string();
    format!("\"{cleaned}\"")
}

fn render_index_ts(id: &str, display: &str, desc: &str, tool: &str) -> String {
    let template = r#"// @ada-needs none
//
// @@ID@@ —— @@DESC_RAW@@
//
// 由 create_plugin 生成的**最小可加载插件包**。四类贡献各留了一处示例：
//   · 工具    @@TOOL@@       —— 模型可调用，execute 的返回值就是回执
//   · 提示词  @@ID@@-usage   —— 进系统提示词，带 plugin:<id> 归因
//   · 技能    @@ID@@         —— **不在脚本里注册**：技能是目录 skills/@@ID@@/SKILL.md
//                              （docs/SKILL_SPEC.md），宿主装配期直接扫描
//   · 钩子    无（count 必须是量出来的 0，别为了"看起来完整"写 1）
//
// 🔴 metadata 要让宿主**看得见**：工厂里第一句 `ada.metadata = metadata;`。
//    宿主在 `await desc(api)` 之后读 `api.metadata`；写成 `export const metadata`
//    没用——转译把 `export default fn` 译成 `module.exports = exports.default = fn`，
//    整个 module.exports 被换成函数，先前挂在 exports 上的东西全丢（真踩过）。
// 🔴 metadata 的 count / names 必须与实际的工具/提示词/钩子注册**逐项、按顺序**一致，
//    否则装载期直接拒（agent-node/src/plugins/sandbox.rs::validate_package_metadata）。
//    `skills` 是**声明性**的（宿主以目录扫描为准），不参与那条一致性校验。
// 🔴 能力声明 `// @ada-needs`：**有指令就是指令里列的那些**，不再叠加宿主地板。
//    本骨架是纯计算（none）。要读文件写 `fs`（或 `fs=read` / `fs=write`）、
//    要联网写 `net`，逗号或空格分隔，例如 `// @ada-needs fs,net`。
//    联网后 `fetch` 支持代理：`fetch(url, { proxy: 'http://127.0.0.1:7897' })`
//    （不带 scheme 的 `127.0.0.1:7897` 也认），或由宿主进程设
//    `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY`。代理配错会直接报错，**不会静默直连**。

const metadata = {
  name: '@@ID@@',
  description: @@DESC_TS@@,
  // 发布到插件市场（plugin-hub）时 author / version 是**必填**：
  // 发布时从包根 index.ts 静态读取并校验，缺一个就会被拒。
  author: 'a_da',
  version: '0.1.0',
  tools: { count: 1, names: ['@@TOOL@@'] },
  systemPrompt: { count: 1, names: ['@@ID@@-usage'] },
  skills: { count: 1, names: ['@@ID@@'] },
  events: { count: 0, names: [] },
};

export default function (ada) {
  // 宿主靠这一句拿到 metadata（必须在工厂里赋值，见文件头部的说明）
  ada.metadata = metadata;
  const workspace = ada.workspace || '';

  ada.registerTool({
    name: '@@TOOL@@',
    label: @@NAME_TS@@,
    description: '示例工具：回显入参文本（骨架生成，换成你自己的逻辑）',
    parameters: {
      type: 'object',
      properties: {
        text: { type: 'string', description: '要回显的文本' },
      },
      required: ['text'],
    },
    // 返回值三种形态都认：
    //   字符串                    → 成功回执正文
    //   { ok, output, details }  → 结构化回执（details 原样进工具卡片）
    //   抛出异常                  → 失败回执（错误文案带文件与行号）
    // 注意：**一个形参**是 (args)，两个形参是 (callId, args)——别按习惯写成 (args, ctx)。
    execute: async (args) => {
      const text = (args && args.text) || '';
      return {
        ok: true,
        output: '收到：' + text,
        details: { length: text.length, workspace: workspace },
      };
    },
  });

  ada.registerSystemPrompt({
    id: '@@ID@@-usage',
    content: @@PROMPT_TS@@,
  });

  // 技能**不在这里注册**：它是一个目录 + SKILL.md（docs/SKILL_SPEC.md），
  // 宿主在装配期扫 `skills/<name>/SKILL.md`。旧的 `ada.registerSkill({...})`
  // 已于 2026-10-19 移除（调用会被忽略并记一条诊断）。
};
"#;

    let prompt = format!(
        "本工作区装有 {id} 插件（{desc}）。需要相关能力时调用 {tool}，不要凭记忆编造结果。"
    );

    template
        .replace("@@DESC_TS@@", &ts_str(desc))
        .replace("@@DESC_RAW@@", &desc.replace(['\n', '\r'], " "))
        .replace("@@NAME_TS@@", &ts_str(display))
        .replace("@@PROMPT_TS@@", &ts_str(&prompt))
        .replace("@@TOOL@@", tool)
        .replace("@@ID@@", id)
}

fn render_readme(id: &str, scope: &str, display: &str, desc: &str) -> String {
    let scope_section = if scope == "workspace" {
        format!(
            "## 想变成全局插件\n\n\
             用 `create_plugin` 带 `scope=\"global\"` 重新生成，或把整个目录复制到\n\
             `~/.a-da/agent-coding/extensions/{id}/`，它就对所有工作区可见。\n"
        )
    } else {
        format!(
            "## 作用域\n\n\
             这是**全局插件**（`global:{id}`），对本机所有工作区生效。\n\
             只想让它在一个工程里生效时，改用 `scope=\"workspace\"` 生成。\n"
        )
    };
    format!(
        r#"# {display}

{desc}

> 由 `create_plugin` 生成。**目录名就是插件标识**：`{scope}:{id}`。

## 布局

| 文件 | 作用 |
|---|---|
| `index.ts` | Extension Factory：`export default function (ada) {{ ... }}`，声明工具 / 提示词 / 技能 / 钩子 |
| `README.md` | 这份说明 |
| `skills/{id}/SKILL.md` | 技能（Agent Skills 规范）：YAML frontmatter + Markdown 正文；`name` 必须等于目录名 |

## 让它生效

1. 「插件与能力中心」里确认 `{scope}:{id}` 已启用（新建默认启用）；
2. 增删插件或保存能力开关后宿主会**立刻重装配**；手改 `index.ts` 则点「重新加载」；
3. 装载失败会在卡片上给出诊断（broken + 原因），原始报错带文件与行号。

## 技能：目录 + SKILL.md（Agent Skills 规范）

技能**不是**一个散装 `.md`，而是一个目录：

```
skills/
└── {id}/
    ├── SKILL.md          # 必需：YAML frontmatter + Markdown 正文
    ├── references/       # 可选：按需读取的详细资料
    ├── scripts/          # 可选：可执行脚本
    └── assets/           # 可选：模板 / 图片 / 数据
```

frontmatter 必填两项：

- `name`：**必须等于目录名**，1-64 个字符，只允许小写字母、数字与连字符，不能以 `-` 起止、不能出现 `--`；
- `description`：1-1024 个字符，写清"做什么 + 什么时候用"（这是模型决定要不要激活它的唯一依据）。

可选：`license`、`compatibility`（≤500）、`metadata`（字符串键值映射）、`allowed-tools`（空格分隔，实验性）。
完整规范见 `docs/SKILL_SPEC.md`。

🔴 **旧的 `ada.registerSkill({{ ... }})` 已移除**（2026-10-19）：技能只能来自磁盘目录，
脚本里调用会被忽略并记一条 `skill_spec_violation:*` 诊断——静默忽略会让作者以为技能装上了。

## 三条容易踩的规矩

1. **metadata 必须与实际注册一致**：`tools` / `systemPrompt` / `events` 的
   `count` 与 `names` 是量出来的，改注册就要同步改它，否则装载期直接拒。
   （`skills` 是声明性的：宿主以目录扫描为准。）
2. **能力声明写在首部注释**：`// @ada-needs fs,net`。有指令就**只有**指令里列的那些
   （`net` 不附带 `fs`）；没有指令则按宿主地板（`fs` 开、`net` 关）。拼错的 token
   不会静默变成"已声明"，会进诊断。
3. **工具返回值就是回执**：字符串 / `{{ ok, output, details }}` / 抛异常，三种都认。

## 联网与代理

`fetch` 只在声明了 `// @ada-needs net` 时才存在（否则 `typeof fetch === 'undefined'`，
连 `globalThis.__native_fetch_async` 也不会注册）。它支持 HTTP / SOCKS5 代理，两种方式：

```ts
// ① 请求级：只让这一次请求走代理
await fetch('https://api.example.com/x', {{ proxy: 'http://127.0.0.1:7897' }});
// 不带 scheme 也认（按 http 处理）：
await fetch('https://api.example.com/x', {{ proxy: '127.0.0.1:7897' }});

// ② 进程级：宿主启动前设环境变量，插件里所有 fetch 自动走代理
//    HTTPS_PROXY / HTTP_PROXY / ALL_PROXY（大小写都认）
```

规则：

- **代理配错会直接报错**，不会静默直连——设了代理就意味着流量必须走代理，
  悄悄直连比请求失败严重得多；
- 单次请求有超时（默认 30s，宿主 `A_DA_PLUGIN_NET_TIMEOUT_SECS` 可调）；
- 宿主可配域名白名单（`A_DA_PLUGIN_NET_ALLOW=a.example,b.example`），白名单之外
  的连接**在建立之前**就被拒，代理也绕不过去。

{scope_section}"#
    )
}

/// `create_plugin` 工具。
pub struct CreatePluginTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
    /// 应用目录（`~/.a-da`）：`scope = "global"` 时插件落在这里。
    /// `None` = 装配面没注入（测试 / 子体），全局插件会被拒绝并说明原因。
    app_home: Option<PathBuf>,
}

/// `forge` 工具包：插件包脚手架（不带应用目录——全局作用域会如实拒绝）。
pub fn forge_tools(workspace: &Path) -> Vec<std::sync::Arc<dyn Tool>> {
    forge_tools_in(workspace, None)
}

/// 带应用目录的 `forge` 装配（生产组合根走这个）。
pub fn forge_tools_in(workspace: &Path, app_home: Option<&Path>) -> Vec<std::sync::Arc<dyn Tool>> {
    vec![std::sync::Arc::new(CreatePluginTool::new(workspace, app_home))]
}

impl CreatePluginTool {
    pub fn new(workspace: impl Into<PathBuf>, app_home: Option<&Path>) -> Self {
        let descriptor = crate::registry::find_tool_descriptor("create_plugin")
            .expect("`create_plugin` 必须在 ToolDescriptor 注册表里（INV-3）")
            .clone();
        Self {
            descriptor,
            workspace: workspace.into(),
            app_home: app_home.map(Path::to_path_buf),
        }
    }
}

impl Tool for CreatePluginTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        ctx: &'a ToolContext<'a>,
    ) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            let started_at = crate::now_ms();

            if ctx.cancel.is_cancelled() {
                return ToolReceipt::new(ToolStatus::Aborted, "操作已取消", started_at, started_at);
            }

            let text = |key: &str| call.args.get(key).and_then(|v| v.as_str());
            let res = scaffold_plugin(
                &self.workspace,
                self.app_home.as_deref(),
                ScaffoldRequest {
                    id: text("id").unwrap_or(""),
                    scope: text("scope").unwrap_or(""),
                    name: text("name"),
                    description: text("description"),
                    tool: text("tool"),
                    overwrite: call
                        .args
                        .get("overwrite")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                },
            );

            let finished_at = crate::now_ms().max(started_at);
            let status = if res.ok { ToolStatus::Success } else { ToolStatus::Error };
            let mut receipt = ToolReceipt::new(status, res.output, started_at, finished_at);
            receipt.details = res.details;
            receipt
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ws() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("a_da_forge_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read(dir: &Path, rel: &str) -> String {
        fs::read_to_string(dir.join(rel)).unwrap_or_else(|e| panic!("读 {rel} 失败：{e}"))
    }

    #[test]
    fn test_scaffold_writes_a_consistent_package() {
        let ws = temp_ws();
        let res = scaffold_plugin(
            &ws,
            None,
            ScaffoldRequest {
                scope: "workspace",
                id: "todo-guard",
                name: Some("待办守卫"),
                description: Some("拦截没有验收标准的 todo"),
                tool: None,
                overwrite: false,
            },
        );
        assert!(res.ok, "{}", res.output);

        let base = ".ada/agent-coding/extensions/todo-guard";
        let index = read(&ws, &format!("{base}/index.ts"));
        assert!(index.contains("const metadata = {"));
        // 宿主读 api.metadata：这一句丢了，包根入口就会报"缺少 metadata 对象"
        assert!(index.contains("ada.metadata = metadata;"));
        assert!(index.contains("export default function (ada)"));
        // metadata 与注册项必须逐项对齐（装载期就是按这个顺序比的）
        assert!(index.contains("tools: { count: 1, names: ['todo_guard_hello'] }"));
        assert!(index.contains("systemPrompt: { count: 1, names: ['todo-guard-usage'] }"));
        assert!(index.contains("skills: { count: 1, names: ['todo-guard'] }"));
        assert!(index.contains("events: { count: 0, names: [] }"));
        assert!(index.contains("ada.registerTool({"));
        // 描述里的引号必须被转义，否则 TS 直接语法错
        assert!(index.contains("'拦截没有验收标准的 todo'"));

        assert!(read(&ws, &format!("{base}/README.md")).contains("workspace:todo-guard"));
        // 技能是**目录 + SKILL.md**（2026-10-19 起的 Agent Skills 规范），不是裸 .md
        let skill = read(&ws, &format!("{base}/skills/todo-guard/SKILL.md"));
        assert!(
            skill.contains("name: todo-guard"),
            "frontmatter 的 name 必须等于目录名：{skill}"
        );
        assert!(skill.contains("# 待办守卫"));

        // 回执说清落点与下一步（模型据此继续，而不是再猜一遍）
        assert!(res.output.contains("workspace:todo-guard"));
        assert!(res.output.contains("~/.a-da/agent-coding/extensions/todo-guard"));

        let _ = fs::remove_dir_all(ws);
    }

    #[test]
    fn test_scaffold_escapes_quotes_in_description() {
        let ws = temp_ws();
        let res = scaffold_plugin(
            &ws,
            None,
            ScaffoldRequest {
                scope: "workspace",
                id: "quoted",
                name: None,
                description: Some("别让 'go test' 跑偏"),
                tool: None,
                overwrite: false,
            },
        );
        assert!(res.ok, "{}", res.output);
        let index = read(&ws, ".ada/agent-coding/extensions/quoted/index.ts");
        assert!(
            index.contains(r"description: '别让 \'go test\' 跑偏'"),
            "单引号必须转义，否则生成的 TS 语法错：\n{index}"
        );
        let _ = fs::remove_dir_all(ws);
    }

    #[test]
    fn test_scaffold_refuses_to_clobber_without_overwrite() {
        let ws = temp_ws();
        let first = scaffold_plugin(
            &ws,
            None,
            ScaffoldRequest {
                scope: "workspace",
                id: "dup",
                name: None,
                description: None,
                tool: None,
                overwrite: false,
            },
        );
        assert!(first.ok, "{}", first.output);

        let second = scaffold_plugin(
            &ws,
            None,
            ScaffoldRequest {
                scope: "workspace",
                id: "dup",
                name: None,
                description: None,
                tool: None,
                overwrite: false,
            },
        );
        assert!(!second.ok, "已存在的目录不许静默覆盖");
        assert!(second.output.contains("overwrite=true"), "{}", second.output);

        // 显式覆盖才放行
        let third = scaffold_plugin(
            &ws,
            None,
            ScaffoldRequest {
                scope: "workspace",
                id: "dup",
                name: Some("覆盖"),
                description: None,
                tool: None,
                overwrite: true,
            },
        );
        assert!(third.ok, "{}", third.output);
        let _ = fs::remove_dir_all(ws);
    }

    #[test]
    fn test_id_and_tool_name_validation() {
        for bad in ["", "Todo", "todo guard", "todo--guard", "-todo", "todo-", "index", ".hidden"] {
            let ws = temp_ws();
            let res = scaffold_plugin(
                &ws,
                None,
                ScaffoldRequest {
                    scope: "workspace",
                    id: bad,
                    name: None,
                    description: None,
                    tool: None,
                    overwrite: false,
                },
            );
            assert!(!res.ok, "`{bad}` 不该被接受：{}", res.output);
            let _ = fs::remove_dir_all(ws);
        }

        let ws = temp_ws();
        let res = scaffold_plugin(
            &ws,
            None,
            ScaffoldRequest {
                scope: "workspace",
                id: "ok-id",
                name: None,
                description: None,
                // 空格/中文工具名模型永远调不到，必须当场拒
                tool: Some("我的 工具"),
                overwrite: false,
            },
        );
        assert!(!res.ok);
        assert!(res.output.contains("工具名"), "{}", res.output);
        let _ = fs::remove_dir_all(ws);
    }

    /// 2026-10-19 需求：**用户没说清装在哪时，工具不许猜**。
    ///
    /// 猜错的代价不是"多一个目录"，而是插件在两个工程里的行为不一致，而用户
    /// 根本不知道是哪次操作造成的。所以缺 `scope` 时返回的是"去问用户"的指引。
    #[test]
    fn test_missing_scope_asks_the_user_instead_of_guessing() {
        let ws = temp_ws();
        let res = scaffold_plugin(
            &ws,
            None,
            ScaffoldRequest {
                id: "no-scope",
                scope: "",
                name: None,
                description: None,
                tool: None,
                overwrite: false,
            },
        );
        assert!(!res.ok, "缺 scope 必须拒绝，而不是默认装进工作区");
        assert!(res.output.contains("ask_user"), "要明确让模型去问用户：{}", res.output);
        assert!(
            res.output.contains("workspace") && res.output.contains("global"),
            "两个选项都要讲清楚：{}",
            res.output
        );
        assert!(
            !ws.join(".ada/agent-coding/extensions/no-scope").exists(),
            "拒绝时不许留下任何目录"
        );
        let _ = fs::remove_dir_all(ws);
    }

    #[test]
    fn test_bad_scope_is_rejected_with_the_allowed_values() {
        let ws = temp_ws();
        let res = scaffold_plugin(
            &ws,
            None,
            ScaffoldRequest {
                id: "bad-scope",
                scope: "somewhere",
                name: None,
                description: None,
                tool: None,
                overwrite: false,
            },
        );
        assert!(!res.ok);
        assert!(res.output.contains("不合法"), "{}", res.output);
        let _ = fs::remove_dir_all(ws);
    }

    /// 全局插件要写应用目录：装配面没注入 `~/.a-da` 时必须**如实拒绝**，
    /// 而不是悄悄装进工作区（那会让用户以为"全局生效了"）。
    #[test]
    fn test_global_scope_requires_injected_app_home() {
        let ws = temp_ws();
        let res = scaffold_plugin(
            &ws,
            None,
            ScaffoldRequest {
                id: "glb",
                scope: "global",
                name: None,
                description: None,
                tool: None,
                overwrite: false,
            },
        );
        assert!(!res.ok);
        assert!(res.output.contains("应用目录"), "{}", res.output);
        assert!(!ws.join(".ada").exists(), "不许退化成写工作区");
        let _ = fs::remove_dir_all(ws);
    }

    #[test]
    fn test_global_scope_writes_into_app_home() {
        let ws = temp_ws();
        let home = temp_ws();
        let res = scaffold_plugin(
            &ws,
            Some(&home),
            ScaffoldRequest {
                id: "glb-one",
                scope: "global",
                name: Some("全局插件"),
                description: None,
                tool: None,
                overwrite: false,
            },
        );
        assert!(res.ok, "{}", res.output);
        assert!(
            home.join("agent-coding").join("extensions").join("glb-one").join("index.ts").is_file(),
            "全局插件要落在 <app_home>/agent-coding/extensions/<id>/index.ts"
        );
        assert!(!ws.join(".ada").exists(), "全局插件不该落到工作区");
        let details = res.details.expect("回执要带 details");
        assert_eq!(details["pluginId"], "global:glb-one");
        assert_eq!(details["scope"], "global");

        let _ = fs::remove_dir_all(ws);
        let _ = fs::remove_dir_all(home);
    }
}
