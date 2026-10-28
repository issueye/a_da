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

use agent_base::domain::{ToolCall, ToolDescriptor, ToolReceipt};
use agent_base::ports::{BoxFuture, Tool, ToolContext};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::cmd_tools::{kill_process_tree, truncate_buffer};
use super::sandbox::resolve_path;

/// 已知 INV-8 缝隙：`ToolContext` 目前**没有**时钟端口，所以这里直接读系统时间。
/// 与同目录的 `command/run.rs` 是同一个缺口，一并等 `ToolContext` 拿到 clock 后收敛。
struct BackgroundTask {
    id: String,
    command: String,
    pid: Option<u32>,
    started_at: i64,
    /// 发起这次后台任务的会话（空 = 旧调用点/测试未提供）。
    thread_id: String,
    /// 输出滑动窗口（头尾保留）
    output: Mutex<String>,
    finished: AtomicBool,
    exit_code: Mutex<Option<i32>>,
}

/// 后台任务的只读快照（给界面/诊断用；不把可变句柄交出去）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTaskSnapshot {
    pub id: String,
    pub command: String,
    pub pid: Option<u32>,
    pub started_at: i64,
    pub thread_id: String,
    pub running: bool,
    pub exit_code: Option<i32>,
    /// 输出原文（截断到 4000 字符；完整内容仍可用 `check_task` 查）。
    pub output_preview: String,
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

    /// 创建任务表时的工作区（供工具在共用任务表的场景下默认取用）。
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// 启动一个后台命令，立即返回任务 id。
    ///
    /// **必须在 Tokio 运行时内调用**：它要 spawn 两个后台任务（输出泵 + 收尾记录退出码），
    /// 没有 reactor 时会 panic。
    pub fn start(&self, command: &str, cwd: Option<&str>) -> Result<String, String> {
        self.start_in_thread(command, cwd, "")
    }

    /// 同 [`Self::start`]，但记录发起会话；`task.list` 可按 threadId 过滤。
    pub fn start_in_thread(
        &self,
        command: &str,
        cwd: Option<&str>,
        thread_id: &str,
    ) -> Result<String, String> {
        self.start_in_workspace(&self.workspace, command, cwd, thread_id)
    }

    /// 同 [`Self::start_in_thread`]，但显式指定工作区。
    ///
    /// 多工作区引擎共用一张任务表时必须走这个入口：`cwd` 沙箱与缺省目录要按
    /// **发起任务的引擎工作区**判定，不能按任务表创建时的那一个。
    pub fn start_in_workspace(
        &self,
        workspace: &Path,
        command: &str,
        cwd: Option<&str>,
        thread_id: &str,
    ) -> Result<String, String> {
        let run_cwd = match cwd {
            Some(p) if !p.trim().is_empty() => resolve_path(workspace, p.trim()),
            _ => workspace.to_path_buf(),
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
            // 同 `run_command`：空设备而不是继承 stdin——否则等输入的后台命令会永远挂着
            .stdin(Stdio::null())
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
            started_at: crate::now_ms(),
            thread_id: thread_id.trim().to_string(),
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

    /// 当前任务快照；`thread_id` 非空时只返回该会话发起的任务。
    ///
    /// 任务表是**实例态**（不落盘）；宿主重启后自然为空。输出只给前 4000 字符，
    /// 完整内容仍由 `check_task` 提供。
    pub fn snapshot(&self, thread_id: Option<&str>) -> Vec<BackgroundTaskSnapshot> {
        let tasks = self.tasks.lock().expect("任务表锁中毒");
        let filter = thread_id.map(str::trim).filter(|value| !value.is_empty());
        let mut out = Vec::new();
        for task in tasks.values() {
            if let Some(thread_id) = filter {
                if task.thread_id != thread_id {
                    continue;
                }
            }
            let running = !task.finished.load(Ordering::Relaxed);
            let exit_code = *task.exit_code.lock().expect("退出码锁中毒");
            let output = task.output.lock().expect("输出锁中毒").clone();
            let mut preview: String = output.chars().take(4000).collect();
            if preview.len() < output.len() {
                preview.push_str("\n…（输出已截断，完整内容用 check_task 查看）");
            }
            out.push(BackgroundTaskSnapshot {
                id: task.id.clone(),
                command: task.command.clone(),
                pid: task.pid,
                started_at: task.started_at,
                thread_id: task.thread_id.clone(),
                running,
                exit_code,
                output_preview: preview,
            });
        }
        out.sort_by(|a, b| a.started_at.cmp(&b.started_at).then_with(|| a.id.cmp(&b.id)));
        out
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

// ── run_background ──────────────────────────────────────────────────────────

pub struct RunBackgroundTool {
    tasks: Arc<BackgroundTasks>,
    /// 发起任务的引擎工作区：共用任务表时 `cwd` 沙箱与缺省目录按它判定。
    workspace: PathBuf,
    descriptor: ToolDescriptor,
}

impl RunBackgroundTool {
    pub fn new(tasks: Arc<BackgroundTasks>) -> Self {
        let workspace = tasks.workspace().to_path_buf();
        Self::new_in(workspace, tasks)
    }

    pub fn new_in(workspace: impl Into<PathBuf>, tasks: Arc<BackgroundTasks>) -> Self {
        let descriptor = crate::registry::find_tool_descriptor("run_background")
            .expect("run_background descriptor must be registered in agent-toolkit registry")
            .clone();
        Self {
            tasks,
            workspace: workspace.into(),
            descriptor,
        }
    }
}

impl Tool for RunBackgroundTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(&'a self, call: &'a ToolCall, ctx: &'a ToolContext<'a>) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            let t0 = crate::now_ms();
            let command = call.args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if command.trim().is_empty() {
                return ToolReceipt::error("缺少必填参数 `command`", t0, crate::now_ms());
            }
            let cwd = call.args.get("cwd").and_then(|v| v.as_str());

            match self
                .tasks
                .start_in_workspace(&self.workspace, command, cwd, ctx.thread_id)
            {
                Ok(id) => ToolReceipt::success(
                    format!("后台任务已启动：{id}（用 check_task 查看输出，kill_task 终止）"),
                    t0,
                    crate::now_ms(),
                ),
                Err(e) => ToolReceipt::error(e, t0, crate::now_ms()),
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
        let descriptor = crate::registry::find_tool_descriptor("check_task")
            .expect("check_task descriptor must be registered in agent-toolkit registry")
            .clone();
        Self { tasks, descriptor }
    }
}

impl Tool for CheckTaskTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(&'a self, call: &'a ToolCall, _ctx: &'a ToolContext<'a>) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            let t0 = crate::now_ms();
            let id = call.args.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
            if id.trim().is_empty() {
                return ToolReceipt::error("缺少必填参数 `task_id`", t0, crate::now_ms());
            }
            match self.tasks.status(id) {
                Ok(s) => ToolReceipt::success(s, t0, crate::now_ms()),
                Err(e) => ToolReceipt::error(e, t0, crate::now_ms()),
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
        let descriptor = crate::registry::find_tool_descriptor("kill_task")
            .expect("kill_task descriptor must be registered in agent-toolkit registry")
            .clone();
        Self { tasks, descriptor }
    }
}

impl Tool for KillTaskTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn execute<'a>(&'a self, call: &'a ToolCall, _ctx: &'a ToolContext<'a>) -> BoxFuture<'a, ToolReceipt> {
        Box::pin(async move {
            let t0 = crate::now_ms();
            let id = call.args.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
            if id.trim().is_empty() {
                return ToolReceipt::error("缺少必填参数 `task_id`", t0, crate::now_ms());
            }
            match self.tasks.kill(id) {
                Ok(msg) => ToolReceipt::success(msg, t0, crate::now_ms()),
                Err(e) => ToolReceipt::error(e, t0, crate::now_ms()),
            }
        })
    }
}

/// 创建后台任务工具族的三个工具实例（**共享同一张任务表**）。
pub fn background_tools(workspace: &Path) -> Vec<Arc<dyn Tool>> {
    background_tools_with_store(workspace, Arc::new(BackgroundTasks::new(workspace)))
}

/// 同 [`background_tools`]，但复用宿主注入的任务表。
///
/// 管理面（`task.list` / `task.kill`）与三个工具必须看到**同一张表**；
/// 各建一份就会出现"工具起了任务、列表里没有"。
pub fn background_tools_with_store(
    workspace: &Path,
    tasks: Arc<BackgroundTasks>,
) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(RunBackgroundTool::new_in(workspace, tasks.clone())),
        Arc::new(CheckTaskTool::new(tasks.clone())),
        Arc::new(KillTaskTool::new(tasks)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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
        fn resolve_path(&self, raw: &str) -> PathBuf {
            self.0.join(raw)
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
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1", hooks: None };

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
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1", hooks: None };

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
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1", hooks: None };

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
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1", hooks: None };

        let r = run.execute(&call("run_background", json!({})), &ctx).await;
        assert_eq!(r.status, ToolStatus::Error);
        assert!(r.output.contains("command"), "{}", r.output);
    }

    #[tokio::test]
    async fn test_shared_store_uses_the_engine_workspace_as_default_cwd() {
        // 多工作区引擎共用一张任务表时，缺省目录跟着**发起任务的引擎**走，
        // 而不是任务表创建时的工作区；cwd 本身只解析、不再裁决边界。
        let root = std::env::temp_dir().join(format!("a_da_bg_shared_{}", uuid::Uuid::new_v4()));
        let primary = root.join("primary");
        let other = root.join("other");
        std::fs::create_dir_all(&primary).unwrap();
        std::fs::create_dir_all(&other).unwrap();

        let tasks = Arc::new(BackgroundTasks::new(&primary));
        let run = RunBackgroundTool::new_in(&other, tasks.clone());
        let (scope, cancel, sink) = ctx_parts(&other);
        let ctx = ToolContext { scope: &scope, cancel: &cancel, events: &sink, thread_id: "t1", hooks: None };

        let ok = run
            .execute(
                &call("run_background", json!({
                    "command": echo_cmd("ALT_WS"),
                    "cwd": other.to_string_lossy(),
                })),
                &ctx,
            )
            .await;
        assert_eq!(ok.status, ToolStatus::Success, "本工作区 cwd 应被接受：{}", ok.output);

        let other_cwd = run
            .execute(
                &call("run_background", json!({
                    "command": echo_cmd("x"),
                    "cwd": primary.to_string_lossy(),
                })),
                &ctx,
            )
            .await;
        assert_eq!(
            other_cwd.status,
            ToolStatus::Success,
            "cwd 只解析、不再做边界裁决：{}",
            other_cwd.output
        );

        let _ = std::fs::remove_dir_all(&root);
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

    #[tokio::test]
    async fn test_snapshot_filters_by_thread_and_keeps_task_metadata() {
        let ws = std::env::current_dir().unwrap();
        let tasks = BackgroundTasks::new(&ws);
        let first = tasks.start_in_thread(&echo_cmd("first"), None, "thread-a").unwrap();
        let second = tasks.start_in_thread(&echo_cmd("second"), None, "thread-b").unwrap();

        let all = tasks.snapshot(None);
        assert_eq!(all.len(), 2, "{all:?}");
        assert!(all.iter().all(|task| !task.command.is_empty()));

        let only_a = tasks.snapshot(Some("thread-a"));
        assert_eq!(only_a.len(), 1, "{only_a:?}");
        assert_eq!(only_a[0].id, first);
        assert_eq!(only_a[0].thread_id, "thread-a");
        assert_ne!(only_a[0].id, second);
    }
}
