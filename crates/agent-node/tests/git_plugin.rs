//! 插件版 Git 工具的端到端验证：真插件 + 真沙箱 + 真 `git` 子进程。
//!
//! 这些用例的存在理由：Git 工具从基座（`agent-toolkit::plugin_tools`）搬到了
//! `plugins/git-tools/index.ts`，于是"它还能不能跑"不再由编译期保证——只能靠真跑一遍。
//! 覆盖三件事：
//!
//! 1. **声明**：两个工具的 `access` 归一化正确（`git` 写 / `git_read` 读）；
//! 2. **执行**：`git status` / `git log` 真能拿到输出（子进程 + 环境 + cwd 都在通路上）；
//! 3. **边界**：IPv6 之外的越界与"只读工具不接受写动作"这类谎话必须在插件侧就挡住。

use std::path::PathBuf;

use agent_node::plugins::PluginSandbox;

/// 仓根（`plugins/git-tools/index.ts` 与 `.git` 都在这层）。
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn plugin_entry() -> PathBuf {
    repo_root().join("plugins/git-tools/index.ts")
}

fn needs() -> ada_ts_engine::Needs {
    ada_ts_engine::Needs {
        fs_read: true,
        fs_write: true,
        net: false,
        sqlite: false,
        exec: true,
    }
}

/// 声明期：`git` 是写、`git_read` 是读，且 metadata 与实际注册一致。
#[tokio::test]
async fn git_plugin_declares_one_write_and_one_readonly_tool() {
    let workspace = repo_root();
    let inspection = PluginSandbox::inspect_plugin_full(&plugin_entry(), &workspace, needs())
        .await
        .expect("git-tools 插件必须能被检查（metadata 与注册逐项一致）");

    let names: Vec<String> = inspection.tools.iter().map(|t| t.name.clone()).collect();
    assert_eq!(names, vec!["git", "git_read"], "工具顺序要与 metadata 一致");

    let git = &inspection.tools[0];
    assert_eq!(git.access, "write", "含 commit/push 的工具不能自称只读");
    let read = &inspection.tools[1];
    assert_eq!(
        read.access, "read",
        "只读工具必须自述 read，否则只读档/plan 模式会把它整条过滤掉"
    );

    // 只读工具的 schema 不允许出现写动作——这是"自述只读"承诺的一半。
    let enum_values = read
        .parameters
        .get("properties")
        .and_then(|p| p.get("action"))
        .and_then(|a| a.get("enum"))
        .and_then(|e| e.as_array())
        .expect("action 必须有 enum");
    let actions: Vec<&str> = enum_values.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(actions, vec!["status", "diff", "log", "show", "blame"]);
    assert!(
        !actions.contains(&"commit") && !actions.contains(&"push"),
        "只读工具里不许出现写动作：{actions:?}"
    );
}

/// 执行期：`git status` 真的跑起来了（子进程、env、cwd 全通）。
#[tokio::test]
async fn git_plugin_runs_status_in_the_workspace() {
    let workspace = repo_root();
    let result = PluginSandbox::call_tool_budgeted(
        &plugin_entry(),
        "git",
        serde_json::json!({ "action": "status" }),
        &workspace,
        30,
        needs(),
        ada_ts_engine::Limits::default(),
    )
    .await
    .expect("工具调用本身不该失败");

    assert!(result.ok, "git status 必须成功：{}", result.output);
    // `--porcelain=v1 -b` 的首行一定是 `## <branch>...`。
    assert!(
        result.output.starts_with("## "),
        "porcelain=-b 的首行应是分支信息：{}",
        result.output
    );
}

/// 只读工具：查询可用，写动作必须被拒（不是静默降级成 status）。
#[tokio::test]
async fn git_read_rejects_write_actions() {
    let workspace = repo_root();
    let log = PluginSandbox::call_tool_budgeted(
        &plugin_entry(),
        "git_read",
        serde_json::json!({ "action": "log", "limit": 1 }),
        &workspace,
        30,
        needs(),
        ada_ts_engine::Limits::default(),
    )
    .await
    .expect("工具调用本身不该失败");
    assert!(log.ok, "git log 必须成功：{}", log.output);
    assert!(!log.output.trim().is_empty(), "仓库至少有一个提交");

    let commit = PluginSandbox::call_tool_budgeted(
        &plugin_entry(),
        "git_read",
        serde_json::json!({ "action": "commit", "message": "不该被执行" }),
        &workspace,
        30,
        needs(),
        ada_ts_engine::Limits::default(),
    )
    .await
    .expect("工具调用本身不该失败");
    assert!(!commit.ok, "只读工具不接受写动作");
    assert!(
        commit.output.contains("未知 git action"),
        "要说清是动作不被允许，而不是默默执行了别的：{}",
        commit.output
    );
}

/// 参数校验在插件侧就挡住越界（`..` 与绝对路径），不靠子进程报错。
#[tokio::test]
async fn git_plugin_rejects_paths_outside_the_workspace() {
    let workspace = repo_root();
    for args in [
        serde_json::json!({ "action": "status", "cwd": "../outside" }),
        serde_json::json!({ "action": "status", "cwd": "C:/Windows" }),
        serde_json::json!({ "action": "add", "file": "../escape.txt" }),
    ] {
        let result = PluginSandbox::call_tool_budgeted(
            &plugin_entry(),
            "git",
            args.clone(),
            &workspace,
            30,
            needs(),
            ada_ts_engine::Limits::default(),
        )
        .await
        .expect("工具调用本身不该失败");
        assert!(!result.ok, "越界参数必须被拒：{args}");
        assert!(
            result.output.contains("不允许越出工作区")
                || result.output.contains("只接受工作区内的相对路径"),
            "拒绝理由要能看懂：{}",
            result.output
        );
    }
}
