//! 后台任务工具族：`run_background` / `check_task` / `kill_task`（W2-T3）。
//!
//! # 为什么是"三个一起"
//!
//! R3（补实现或删声明，不许留半吊子）在这里的具体含义：**只实现 `run_background` 是半吊子**——
//! 能起一个 dev server 却看不到它的输出、也停不掉它。所以要么整族实现，要么整族删声明。
//! 本文件选择整族实现，因为它是编码助手的真实能力（界面也一直在广告这三个工具）。
//!
//! # INV-8：任务表是**实例态**，不是全局单例
//!
//! [`BackgroundTasks`] 由 `command_tools()` 每次装配时创建一份，三个工具共享同一个
//! `Arc`。同一进程里两个 runtime 各自持有自己的任务表，互不可见——这正是
//! INV-8（无隐藏全局态）要的形状。
//!
//! # 与取消的关系
//!
//! 后台任务**故意不跟随轮次取消**：它的语义就是"活过这次工具调用"。要停它请用
//! `kill_task`（用户显式动作），这与 `run_command` 的"随轮次取消"是刻意的不对称。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agent_base::domain::{
    Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
    ToolReceipt,
};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::cmd_tools::{kill_process_tree, truncate_buffer};
use super::sandbox::check_workspace_sandbox;

/// 已知 INV-8 缝隙：`ToolContext` 目前**没有**时钟端口，所以这里直接读系统时间。
/// 与同目录的 `command/run.rs` 是同一个缺口，一并等 `ToolContext` 拿到 clock 后收敛。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

struct BackgroundTask {
    id: String,
    command: String,
    pid: Option<u32>,
    started_at: i64,
    /// 输出滑动窗口（头尾保留）
    output: Mutex<String>,
    finished: AtomicBool,
    exit_code: Mutex<Option<i32>>,
}

impl BackgroundTask {
    fn render_status(&self) -> String {
        let state = if self.finished.load(Ordering::Relaxed) {
            match *self.exit_code.lock().expect("退出码锁中毒") {
                Some(code) => format!("已结束（退出码 {code}）"),
                None => "已结束（被终止）".to_string(),
            }
        } else {
            "运行中".to_string()
        };
        let out = self.output.lock().expect("输出锁中毒").clone();
        let out = if out.trim().is_empty() { "(暂无输出)".to_string() } else { out };
        format!(
            "任务 {} | {} | pid={} | 启动于 {} | 命令: {}\n--- 输出 ---\n{}",
            self.id,
            state,
            self.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
            self.started_at,
            self.command,
            out
        )
    }
}

/// 后台任务表（实例态）。
pub struct BackgroundTasks {
    workspace: PathBuf,
    tasks: Mutex<HashMap<String, Arc<BackgroundTask>>>,
    counter: AtomicU64,
}

impl BackgroundTasks {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            tasks: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(0),
        }
    }

    /// 启动一个后台命令，立即返回任务 id。
    ///
    /// **必须在 Tokio 运行时内调用**：它要 spawn 两个后台任务（输出泵 + 收尾记录退出码），
    /// 没有 reactor 时会 panic。
    pub fn start(&self, command: &str, cwd: Option<&str>) -> Result<String, String> {
        let run_cwd = match cwd {
            Some(p) if !p.trim().is_empty() => {
                check_workspace_sandbox(&self.workspace, p.trim()).map_err(|e| e.to_string())?
            }
            _ => self.workspace.clone(),
        };

        let (shell, shell_args) = if cfg!(windows) {
            let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
            (
                comspec,
                vec!["/d".to_string(), "/s".to_string(), "/c".to_string(), command.to_string()],
            )
        } else {
            ("/bin/sh".to_string(), vec!["-c".to_string(), command.to_string()])
        };

        let mut cmd = Command::new(shell);
        cmd.args(&shell_args)
            .current_dir(run_cwd)
            .env("A_DA_AGENT", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        let mut child = cmd.spawn().map_err(|e| format!("启动后台进程失败: {e}"))?;
        let pid = child.id();

        let id = format!("task_{}", self.counter.fetch_add(1, Ordering::Relaxed) + 1);
        let task = Arc::new(BackgroundTask {
            id: id.clone(),
            command: command.to_string(),
            pid,
            started_at: now_ms(),
            output: Mutex::new(String::new()),
            finished: AtomicBool::new(false),
            exit_code: Mutex::new(None),
        });

        // 输出泵：stdout + stderr 合并进滑动窗口
        if let Some(out) = child.stdout.take() {
            let t = task.clone();
            tokio::spawn(pump(out, "", t));
        }
        if let Some(err) = child.stderr.take() {
            let t = task.clone();
            tokio::spawn(pump(err, "[stderr] ", t));
        }

        // 收尾：记录退出码
        let t = task.clone();
        tokio::spawn(async move {
            let code = child.wait().await.ok().and_then(|s| s.code());
            *t.exit_code.lock().expect("退出码锁中毒") = Some(code.unwrap_or(-1));
            t.finished.store(true, Ordering::Relaxed);
        });

        self.tasks
            .lock()
            .expect("任务表锁中毒")
            .insert(id.clone(), task);
        Ok(id)
    }

    /// 查询任务状态与输出。
    pub fn status(&self, task_id: &str) -> Result<String, String> {
        let tasks = self.tasks.lock().expect("任务表锁中毒");
        match tasks.get(task_id) {
            Some(t) => Ok(t.render_status()),
            None => Err(format!(
                "未知的后台任务 id: {task_id}（已知 {} 个任务）",
                tasks.len()
            )),
        }
    }

    /// 终止任务（整棵进程树）。
    pub fn kill(&self, task_id: &str) -> Result<String, String> {
        let tasks = self.tasks.lock().expect("任务表锁中毒");
        let Some(t) = tasks.get(task_id) else {
            return Err(format!("未知的后台任务 id: {task_id}"));
        };
        if t.finished.load(Ordering::Relaxed) {
            return Ok(format!("任务 {task_id} 早已结束，无需终止"));
        }
        let Some(pid) = t.pid else {
            return Err(format!("任务 {task_id} 没有可终止的进程（pid 缺失）"));
        };
        kill_process_tree(pid);
        Ok(format!("已终止任务 {task_id}（进程树 pid={pid}）"))
    }

    /// 任务数量（诊断/测试）。
    pub fn count(&self) -> usize {
        self.tasks.lock().expect("任务表锁中毒").len()
    }
}

/// 把一个子进程输出流泵进任务的滑动窗口。
async fn pump<R>(mut reader: R, prefix: &'static str, task: Arc<BackgroundTask>)
where
    R: AsyncReadExt + Unpin,
{
    let mut chunk = [0u8; 8192];
    while let Ok(n) = reader.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&chunk[..n]).to_string();
        let mut buf = task.output.lock().expect("输出锁中毒");
        if !prefix.is_empty() {
            buf.push_str(prefix);
        }
        buf.push_str(&text);
        let truncated = truncate_buffer(&buf);
        *buf = truncated;
    }
}

fn descriptor(name: &str, summary: &str, schema: serde_json::Value, access: Access) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_string(),
        summary: summary.to_string(),
        schema,
        access,
        // 后台命令与终止进程都属于"要受审批约束"的动作；策略归 approval-guard
        approval: ApprovalPolicy::Named("approval-guard"),
        rollback: RollbackPolicy::None,
        execution: Execution::Sequential,
        termination: Termination::ContinueTurn,
    }
}

fn started() -> i64 {
    now_ms()
}

// ── run_background ──────────────────────────────────────────────────────────

pub struct RunBackgroundTool {
    tasks: Arc<BackgroundTasks>,
    descriptor: ToolDescriptor,
}

impl RunBackgroundTool {
    pub fn new(tasks: Arc<BackgroundTasks>) -> Self {
        Self {
            tasks,
            descriptor: descriptor(
                "run_background",
                "后台启动长运行命令（dev server、watcher 等），立即返回任务 id。用 check_task 看输出、kill_task 终止。",
                json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "要后台执行的命令行指令" },
                        "cwd": { "type": "string", "description": "子目录路径（相对工作区根目录，可选）" }
                    },
                    "required": ["command"]
                }),
                Access::Executes { command_arg: "command" },
            ),
        }
    }
}

impl Tool for RunBackgroundTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(&'a self, call: &'a ToolCall, _ctx: &'a ToolContext<'a>) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            let t0 = started();
            let command = call.args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if command.trim().is_empty() {
                return ToolReceipt::error("缺少必填参数 `command`", t0, started());
            }
            let cwd = call.args.get("cwd").and_then(|v| v.as_str());

            match self.tasks.start(command, cwd) {
                Ok(id) => ToolReceipt::success(
                    format!("后台任务已启动：{id}（用 check_task 查看输出，kill_task 终止）"),
                    t0,
                    started(),
                ),
                Err(e) => ToolReceipt::error(e, t0, started()),
            }
        })
    }
}

// ── check_task ──────────────────────────────────────────────────────────────

pub struct CheckTaskTool {
    tasks: Arc<BackgroundTasks>,
    descriptor: ToolDescriptor,
}

impl CheckTaskTool {
    pub fn new(tasks: Arc<BackgroundTasks>) -> Self {
        Self {
            tasks,
            descriptor: descriptor(
                "check_task",
                "查询后台任务的状态与输出（含退出码）。",
                json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "run_background 返回的任务 id" }
                    },
                    "required": ["task_id"]
                }),
                Access::ReadOnly,
            ),
        }
    }
}

impl Tool for CheckTaskTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(&'a self, call: &'a ToolCall, _ctx: &'a ToolContext<'a>) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            let t0 = started();
            let id = call.args.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
            if id.trim().is_empty() {
                return ToolReceipt::error("缺少必填参数 `task_id`", t0, started());
            }
            match self.tasks.status(id) {
                Ok(s) => ToolReceipt::success(s, t0, started()),
                Err(e) => ToolReceipt::error(e, t0, started()),
            }
        })
    }
}

// ── kill_task ───────────────────────────────────────────────────────────────

pub struct KillTaskTool {
    tasks: Arc<BackgroundTasks>,
    descriptor: ToolDescriptor,
}

impl KillTaskTool {
    pub fn new(tasks: Arc<BackgroundTasks>) -> Self {
        Self {
            tasks,
            // `kill_task` 不碰文件、也不跑命令文本，但它**确实有副作用**（终结进程树）。
            // `Access` 目前只有 ReadOnly / Mutates(文件) / Executes(命令) 三态，
            // 因此按"终结一个进程"归到 Executes，字段名说明它取的是任务 id 而不是命令。
            descriptor: descriptor(
                "kill_task",
                "终止后台任务及其整棵子进程树。",
                json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "要终止的后台任务 id" }
                    },
                    "required": ["task_id"]
                }),
                Access::Executes { command_arg: "task_id" },
            ),
        }
    }
}

impl Tool for KillTaskTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(&'a self, call: &'a ToolCall, _ctx: &'a ToolContext<'a>) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            let t0 = started();
            let id = call.args.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
            if id.trim().is_empty() {
                return ToolReceipt::error("缺少必填参数 `task_id`", t0, started());
            }
            match self.tasks.kill(id) {
                Ok(msg) => ToolReceipt::success(msg, t0, started()),
                Err(e) => ToolReceipt::error(e, t0, started()),
            }
        })
    }
}

/// 创建后台任务工具族的三个工具实例（**共享同一张任务表**）。
pub fn background_tools(workspace: &Path) -> Vec<Arc<dyn Tool>> {
    let tasks = Arc::new(BackgroundTasks::new(workspace));
    vec![
        Arc::new(RunBackgroundTool::new(tasks.clone())),
        Arc::new(CheckTaskTool::new(tasks.clone())),
        Arc::new(KillTaskTool::new(tasks)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::ToolStatus;
    use agent_base::ports::{CancelToken, EventSink, Scope};
    use std::path::PathBuf;

    struct NoopSink;
    impl EventSink for NoopSink {
        fn emit(&self, _event: agent_base::domain::AgentEvent) {}
    }
    struct NoCancel;
    impl CancelToken for NoCancel {
        fn is_cancelled(&self) -> bool {
            false
        }
    }
    struct RootScope(PathBuf);
    impl Scope for RootScope {
        fn id(&self) -> &str {
            "test_root"
        }
        fn resolve_path(&self, raw: &str) -> Result<PathBuf, agent_base::domain::DenialKind> {
            Ok(self.0.join(raw))
        }
    }

    fn ctx_parts(ws: &Path) -> (RootScope, NoCancel, NoopSink) {
        (RootScope(ws.to_path_buf()), NoCancel, NoopSink)
    }

    fn call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall { id: "c1".into(), name: name.into(), args }
    }

    fn sleep_cmd() -> &'static str {
        if cfg!(windows) {
            "ping 127.0.0.1 -n 20 > nul"
        } else {
            "sleep 20"
        }
    }

    fn echo_cmd(tag: &str) -> String {
        format!("echo {tag}")
    }

    #[tokio::test]
    async fn test_run_then_check_reports_output_and_exit() {
        let ws = std::env::current_dir().unwrap();
        let tasks = Arc::new(BackgroundTasks::new(&ws));
        let run = RunBackgroundTool::new(tasks.clone());
        let check = CheckTaskTool::new(tasks.clone());

        let (scope, cancel, sink) = ctx_parts(&ws);
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1" };

        let receipt = run.execute(&call("run_background", json!({"command": echo_cmd("BG_OK")})), &ctx).await;
        assert_eq!(receipt.status, ToolStatus::Success, "启动必须成功：{}", receipt.output);

        // 从回执里取任务 id
        let id = receipt
            .output
            .split('：')
            .nth(1)
            .and_then(|s| s.split('（').next())
            .expect("回执应包含任务 id")
            .trim()
            .to_string();

        // 轮询等待命令结束（最多 ~5s）
        let mut last = String::new();
        for _ in 0..50 {
            last = check
                .execute(&call("check_task", json!({"task_id": id})), &ctx)
                .await
                .output;
            if last.contains("已结束") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(last.contains("BG_OK"), "输出应包含命令结果：{last}");
        assert!(last.contains("已结束"), "任务应已结束：{last}");
    }

    #[tokio::test]
    async fn test_kill_task_stops_long_running_command() {
        let ws = std::env::current_dir().unwrap();
        let tasks = Arc::new(BackgroundTasks::new(&ws));
        let run = RunBackgroundTool::new(tasks.clone());
        let kill = KillTaskTool::new(tasks.clone());
        let check = CheckTaskTool::new(tasks.clone());

        let (scope, cancel, sink) = ctx_parts(&ws);
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1" };

        let receipt = run
            .execute(&call("run_background", json!({"command": sleep_cmd()})), &ctx)
            .await;
        let id = receipt
            .output
            .split('：')
            .nth(1)
            .and_then(|s| s.split('（').next())
            .expect("回执应包含任务 id")
            .trim()
            .to_string();

        // 应当还在运行
        let before = check.execute(&call("check_task", json!({"task_id": id})), &ctx).await.output;
        assert!(before.contains("运行中"), "长命令应仍在运行：{before}");

        let killed = kill.execute(&call("kill_task", json!({"task_id": id})), &ctx).await;
        assert_eq!(killed.status, ToolStatus::Success, "终止必须成功：{}", killed.output);
        assert!(killed.output.contains("已终止"), "{}", killed.output);

        // 等收尾任务把 finished 置位
        let mut after = String::new();
        for _ in 0..30 {
            after = check.execute(&call("check_task", json!({"task_id": id})), &ctx).await.output;
            if after.contains("已结束") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(after.contains("已结束"), "终止后任务应标记为已结束：{after}");
    }

    #[tokio::test]
    async fn test_unknown_task_id_is_an_error() {
        let ws = std::env::current_dir().unwrap();
        let tasks = Arc::new(BackgroundTasks::new(&ws));
        let check = CheckTaskTool::new(tasks.clone());
        let kill = KillTaskTool::new(tasks.clone());

        let (scope, cancel, sink) = ctx_parts(&ws);
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1" };

        let r = check.execute(&call("check_task", json!({"task_id": "nope"})), &ctx).await;
        assert_eq!(r.status, ToolStatus::Error);
        assert!(r.output.contains("未知的后台任务"), "{}", r.output);

        let r = kill.execute(&call("kill_task", json!({"task_id": "nope"})), &ctx).await;
        assert_eq!(r.status, ToolStatus::Error);
    }

    #[tokio::test]
    async fn test_missing_required_arg_is_an_error() {
        let ws = std::env::current_dir().unwrap();
        let tasks = Arc::new(BackgroundTasks::new(&ws));
        let run = RunBackgroundTool::new(tasks.clone());
        let (scope, cancel, sink) = ctx_parts(&ws);
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1" };

        let r = run.execute(&call("run_background", json!({})), &ctx).await;
        assert_eq!(r.status, ToolStatus::Error);
        assert!(r.output.contains("command"), "{}", r.output);
    }

    #[tokio::test]
    async fn test_cwd_outside_workspace_is_rejected() {
        let ws = std::env::current_dir().unwrap();
        let tasks = Arc::new(BackgroundTasks::new(&ws));
        let run = RunBackgroundTool::new(tasks.clone());
        let (scope, cancel, sink) = ctx_parts(&ws);
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1" };

        let r = run
            .execute(
                &call("run_background", json!({"command": echo_cmd("x"), "cwd": "../../../../Windows"})),
                &ctx,
            )
            .await;
        assert_eq!(r.status, ToolStatus::Error, "越界 cwd 必须被拒：{}", r.output);
    }

    #[test]
    fn test_background_tools_share_one_registry_and_have_expected_descriptors() {
        let ws = std::env::current_dir().unwrap();
        let tools = background_tools(&ws);
        assert_eq!(tools.len(), 3, "整族三个工具");
        let names: Vec<String> = tools.iter().map(|t| t.descriptor().name.clone()).collect();
        assert_eq!(names, vec!["run_background", "check_task", "kill_task"]);
        // check_task 是只读的（只读过滤/plan 模式要能拿到它）
        assert!(tools[1].descriptor().is_readonly(), "check_task 必须是只读工具");
        assert!(!tools[0].descriptor().is_readonly(), "run_background 不是只读工具");
    }

    #[tokio::test]
    async fn test_registries_are_per_instance_not_global() {
        // INV-8：两份装配各自一张表，互不可见
        let ws = std::env::current_dir().unwrap();
        let a = BackgroundTasks::new(&ws);
        let b = BackgroundTasks::new(&ws);
        assert_eq!(a.count(), 0);
        assert_eq!(b.count(), 0);
        // 同一实例内计数递增
        let _ = a.start(&echo_cmd("x"), None);
        assert_eq!(a.count(), 1);
        assert_eq!(b.count(), 0, "另一个实例不该看到任务");
    }
}
