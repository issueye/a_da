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
//! 落点是**工作区**的插件根（`<ws>/.ada/agent-coding/extensions/<id>/`）：
//! 它和 `PluginManager::agent_extensions_dir` 同一口径，于是"改完就能在这个工作区里
//! 看到"；要全局生效，把整个目录复制到 `~/.a-da/agent-coding/extensions/` 即可
//! （回执里会明说这一步——工具不越权去写应用目录）。

use std::fs;
use std::path::{Path, PathBuf};

use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt, ToolStatus};
use agent_base::ports::{BoxFuture, Tool, ToolContext};

use crate::fs_tools::ToolResult;

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
    pub name: Option<&'a str>,
    pub description: Option<&'a str>,
    /// 示例工具名（默认 `<id>_hello`）
    pub tool: Option<&'a str>,
    pub overwrite: bool,
}

/// 生成插件包。返回的相对路径都相对工作区（便于直读）。
pub fn scaffold_plugin(workspace: &Path, req: ScaffoldRequest<'_>) -> ToolResult {
    let id = req.id.trim();
    if let Err(e) = validate_id(id) {
        return ToolResult::error(e);
    }

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

    let root = workspace_extensions_dir(workspace);
    let dir = root.join(id);
    if dir.exists() && !req.overwrite {
        return ToolResult::error(format!(
            "插件目录已存在：{}（要覆盖生成的文件请传 overwrite=true；不会动你自己的其它文件）",
            dir.display()
        ));
    }

    let skills_dir = dir.join("skills");
    if let Err(e) = fs::create_dir_all(&skills_dir) {
        return ToolResult::error(format!("创建插件目录失败（{}）：{}", skills_dir.display(), e));
    }

    let skill_body = skill_body(display, desc);
    let files: Vec<(PathBuf, String)> = vec![
        (dir.join("index.ts"), render_index_ts(id, display, desc, &tool, &skill_body)),
        (dir.join("README.md"), render_readme(id, display, desc)),
        (skills_dir.join(format!("{id}.md")), format!("{skill_body}\n")),
    ];

    let mut written = Vec::new();
    for (path, content) in files {
        if let Err(e) = fs::write(&path, content) {
            return ToolResult::error(format!("写入 {} 失败：{}", path.display(), e));
        }
        written.push(rel(workspace, &path));
    }

    let mut output = format!("已生成插件 `{id}`（{} 个文件）：\n", written.len());
    for p in &written {
        output.push_str(&format!("  {p}\n"));
    }
    output.push_str(&format!(
        "\n插件标识：`workspace:{id}`（目录名就是标识，重命名目录会变成另一个插件）\n\
         \n下一步：\n\
         1. 在「插件与能力中心」确认它处于启用状态（新建插件默认启用）；\n\
         2. 贡献在**装配期**结算：改完 index.ts 要重新加载插件 / 新开一轮才会生效；\n\
         3. 增删注册项要同步 metadata 的 count/names，否则装载期会拒（卡片显示 broken + 原因）；\n\
         4. 要对该工作区之外也生效：把整个目录复制到 ~/.a-da/agent-coding/extensions/{id}/。\n\
         \n示例工具：`{tool}` —— 现在只会回显入参，把 execute 换成你的逻辑即可。"
    ));

    ToolResult {
        ok: true,
        output,
        patch: None,
        details: Some(serde_json::json!({
            "id": id,
            "pluginId": format!("workspace:{id}"),
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

fn skill_body(display: &str, desc: &str) -> String {
    format!(
        "# {display}\n\n\
         - 做什么：{desc}\n\
         - 什么时候用：<写清可判定的触发条件；「需要时」这种话等于没写>\n\
         - 不要用它做什么：<最容易误用的场景>\n\
         \n\
         > 这段正文会进系统提示词，每一行都在花字节：删掉不成立的，补上最容易踩错的那条。"
    )
}

fn render_index_ts(id: &str, display: &str, desc: &str, tool: &str, body: &str) -> String {
    let template = r#"// @ada-needs none
//
// @@ID@@ —— @@DESC_RAW@@
//
// 由 create_plugin 生成的**最小可加载插件包**。四类贡献各留了一处示例：
//   · 工具    @@TOOL@@       —— 模型可调用，execute 的返回值就是回执
//   · 提示词  @@ID@@-usage   —— 进系统提示词，带 plugin:<id> 归因
//   · 技能    @@ID@@         —— 进技能列表（正文也落了一份在 skills/@@ID@@.md）
//   · 钩子    无（count 必须是量出来的 0，别为了"看起来完整"写 1）
//
// 🔴 metadata 要让宿主**看得见**：工厂里第一句 `ada.metadata = metadata;`。
//    宿主在 `await desc(api)` 之后读 `api.metadata`；写成 `export const metadata`
//    没用——转译把 `export default fn` 译成 `module.exports = exports.default = fn`，
//    整个 module.exports 被换成函数，先前挂在 exports 上的东西全丢（真踩过）。
// 🔴 metadata 的 count / names 必须与下面的注册**逐项、按顺序**一致，否则装载期
//    直接拒（agent-node/src/plugins/sandbox.rs::validate_package_metadata）。
//    增删一条注册就要同步改 metadata，否则插件会变成 broken。
// 🔴 能力声明 `// @ada-needs`：**有指令就是指令里列的那些**，不再叠加宿主地板。
//    本骨架是纯计算（none）。要读文件写 `fs`（或 `fs=read` / `fs=write`）、
//    要联网写 `net`，逗号或空格分隔，例如 `// @ada-needs fs,net`。

const metadata = {
  name: '@@ID@@',
  description: @@DESC_TS@@,
  tools: { count: 1, names: ['@@TOOL@@'] },
  systemPrompt: { count: 1, names: ['@@ID@@-usage'] },
  skills: { count: 1, names: ['@@ID@@'] },
  hooks: { count: 0, names: [] },
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

  ada.registerSkill({
    name: '@@ID@@',
    description: @@SKILL_DESC_TS@@,
    path: 'skills/@@ID@@.md',
    body: @@BODY_TS@@,
  });
};
"#;

    let prompt = format!(
        "本工作区装有 {id} 插件（{desc}）。需要相关能力时调用 {tool}，不要凭记忆编造结果。"
    );
    let skill_desc = format!("{display} 的用法说明");

    template
        .replace("@@DESC_TS@@", &ts_str(desc))
        .replace("@@DESC_RAW@@", &desc.replace(['\n', '\r'], " "))
        .replace("@@NAME_TS@@", &ts_str(display))
        .replace("@@PROMPT_TS@@", &ts_str(&prompt))
        .replace("@@SKILL_DESC_TS@@", &ts_str(&skill_desc))
        .replace("@@BODY_TS@@", &ts_str(body))
        .replace("@@TOOL@@", tool)
        .replace("@@ID@@", id)
}

fn render_readme(id: &str, display: &str, desc: &str) -> String {
    format!(
        r#"# {display}

{desc}

> 由 `create_plugin` 生成。**目录名就是插件标识**：`workspace:{id}`。

## 布局

| 文件 | 作用 |
|---|---|
| `index.ts` | Extension Factory：`export default function (ada) {{ ... }}`，声明工具 / 提示词 / 技能 / 钩子 |
| `README.md` | 这份说明 |
| `skills/{id}.md` | 技能正文（与 index.ts 里 `body` 同源，改正文记得两处都改） |

## 让它生效

1. 「插件与能力中心」里确认 `workspace:{id}` 已启用（新建默认启用）；
2. 贡献在**装配期**结算——改完 `index.ts` 要重新加载插件 / 新开一轮才会用上；
3. 装载失败会在卡片上给出诊断（broken + 原因），原始报错带文件与行号。

## 三条容易踩的规矩

1. **metadata 必须与注册一致**：`tools` / `systemPrompt` / `skills` / `hooks` 的
   `count` 与 `names` 是量出来的，改注册就要同步改它，否则装载期直接拒。
2. **能力声明写在首部注释**：`// @ada-needs fs,net`。有指令就**只有**指令里列的那些
   （`net` 不附带 `fs`）；没有指令则按宿主地板（`fs` 开、`net` 关）。拼错的 token
   不会静默变成"已声明"，会进诊断。
3. **工具返回值就是回执**：字符串 / `{{ ok, output, details }}` / 抛异常，三种都认。

## 想变成全局插件

把整个目录复制到 `~/.a-da/agent-coding/extensions/{id}/`，它就对所有工作区可见。
"#
    )
}

/// `create_plugin` 工具。
pub struct CreatePluginTool {
    descriptor: ToolDescriptor,
    workspace: PathBuf,
}

/// `forge` 工具包：插件包脚手架。
pub fn forge_tools(workspace: &Path) -> Vec<std::sync::Arc<dyn Tool>> {
    vec![std::sync::Arc::new(CreatePluginTool::new(workspace))]
}

impl CreatePluginTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let descriptor = crate::registry::find_tool_descriptor("create_plugin")
            .expect("`create_plugin` 必须在 ToolDescriptor 注册表里（INV-3）")
            .clone();
        Self {
            descriptor,
            workspace: workspace.into(),
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
                ScaffoldRequest {
                    id: text("id").unwrap_or(""),
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
            ScaffoldRequest {
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
        assert!(index.contains("hooks: { count: 0, names: [] }"));
        assert!(index.contains("ada.registerTool({"));
        // 描述里的引号必须被转义，否则 TS 直接语法错
        assert!(index.contains("'拦截没有验收标准的 todo'"));

        assert!(read(&ws, &format!("{base}/README.md")).contains("workspace:todo-guard"));
        assert!(read(&ws, &format!("{base}/skills/todo-guard.md")).contains("# 待办守卫"));

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
            ScaffoldRequest {
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
            ScaffoldRequest {
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
            ScaffoldRequest {
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
            ScaffoldRequest {
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
                ScaffoldRequest {
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
            ScaffoldRequest {
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
}
