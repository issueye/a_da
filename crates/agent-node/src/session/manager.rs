use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use agent_base::ports::AppHome;
use anyhow::{Context, Result};


use super::slug::{safe_id, workspace_slug};
use super::types::{
    AgentMessage, CURRENT_SESSION_VERSION, SessionCompactEntry, SessionEntry, SessionHeader,
    SessionSummary,
};
use agent_proto::{AgentMode, Item, Thread};

/// 进程内的应用目录端口（组合根/测试可注入；默认见 `default_app_home`）。
///
/// 为什么是全局单例：现状有 10 多处直接调 `get_app_home()`，一次性改成参数注入会把
/// 本批次（目录迁移）与行为改动混在一起。这里是**过渡期的显式缝隙**：
/// 迁移完成后 `get_app_home()` 会消失，调用方改为接收 `&dyn AppHome`。
static APP_HOME: OnceLock<Box<dyn AppHome>> = OnceLock::new();

/// 当前应用目录端口。
pub fn app_home() -> &'static dyn AppHome {
    APP_HOME.get_or_init(default_app_home_boxed).as_ref()
}

/// 注入应用目录。返回 `Err` 表示本进程已经初始化过（只允许一次）。
pub fn set_app_home(home: Box<dyn AppHome>) -> Result<(), Box<dyn AppHome>> {
    APP_HOME.set(home)
}

/// 单元测试默认落在临时目录：**绝不碰用户真实的 `~/.a-da`**。
/// （TS 时代靠 `scripts/test-preload.ts` 重定向，Rust 侧此前没有等价物。）
///
/// 目录名带 PID：`cargo test` 每次运行都是新的空目录，避免"上一轮跑剩下的 config.json
/// 影响下一轮"这种跨运行状态泄漏（这类泄漏会伪装成随机失败）。
#[cfg(test)]
fn default_app_home_boxed() -> Box<dyn AppHome> {
    Box::new(agent_base::testing::TempAppHome::at(
        std::env::temp_dir().join(format!("a_da_agent_core_test_home_{}", std::process::id())),
    ))
}

#[cfg(not(test))]
fn default_app_home_boxed() -> Box<dyn AppHome> {
    Box::new(agent_adapter::app_home::SystemAppHome::from_env())
}

/// 获取默认的 a-da 配置和数据主目录（兼容旧签名；实现在 `AppHome` 端口里）。
pub fn get_app_home() -> PathBuf {
    app_home().root().to_path_buf()
}

/// 获取 a-da 配置文件路径，优先遵循 `A_DA_CONFIG` 环境变量（实现在 `AppHome` 端口里）。
pub fn get_config_path() -> PathBuf {
    app_home().config_file()
}

/// 会话管理器
#[derive(Debug, Clone)]
pub struct SessionManager {
    explicit_dir: Option<PathBuf>,
}

/// 模式落盘：直接借 `AgentMode` 的 serde 命名（`code`/`plan`/`create`/`pm`），
/// 避免再手写一份 match 与协议漂移。
fn mode_to_header(mode: agent_proto::AgentMode) -> String {
    serde_json::to_value(mode)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "code".to_string())
}

/// 模式读盘：认不出来就当没有（调用方自己兜底），绝不猜。
fn mode_from_header(raw: &str) -> Option<agent_proto::AgentMode> {
    serde_json::from_value::<agent_proto::AgentMode>(serde_json::Value::String(raw.to_string())).ok()
}

/// 子会话标题的**唯一短 id 回填**（老数据自愈）。
///
/// 新派发的子会话标题形如 `子代理 #36fb30 [全能执行专员]: <任务>`；更早的没有 `#<短id>`，
/// 同 profile 连派多个时标题逐字相同，界面上分不出来。这里在**读盘时**补一次并写回 header
/// （幂等：标题里已含该 id 就原样返回，不再写盘）。用户自己改过名（不以「子代理」开头）的不动。
fn backfill_subagent_title<F>(raw_title: &str, thread_id: &str, file_path: &Path, write: F) -> String
where
    F: FnOnce(&Path, &str) -> Result<()>,
{
    let trimmed = raw_title.trim();
    if !trimmed.starts_with("子代理") {
        return raw_title.to_string();
    }
    let short_id = thread_id.rsplit('_').next().unwrap_or("");
    if short_id.is_empty() || trimmed.contains(&format!("#{short_id}")) {
        return raw_title.to_string();
    }
    let rest = trimmed.trim_start_matches("子代理").trim_start();
    let new_title = format!("子代理 #{short_id} {rest}");
    // 写盘失败也不能影响还原：标题退回原样即可
    if write(file_path, &new_title).is_ok() {
        new_title
    } else {
        raw_title.to_string()
    }
}

/// 磁盘回填时给卡片发的时间戳：**按折叠顺序严格递增**。
///
/// 为什么回填侧也必须管这件事（而不只是"用消息自己的时间戳"）：
/// 增量帧（`evt.state.patch`）只带 `upserted`，而它是**按 id 排序**发的（帧内容要可复现），
/// 所以界面给新卡片定位**只能按 `at`**（`insertByItemTime`）。
/// 一条 assistant 消息会派生好几张卡（思考 / 工具 / 正文），它们共用同一个毫秒时，
/// 先后就落到 id 字典序上：`..._restored_asst_*` < `..._restored_think_*`，
/// 于是轮次一收尾（内存卡片被磁盘回填整份换掉、id 全变），**思考块会掉到正文下面**。
/// 实测一条 132 张卡的会话里有 68 张卡位置被换乱。
///
/// 所以这里按折叠顺序发时间戳：思考 < 正文 < 工具卡（与流式现场一致），且整条会话单调递增。
/// 内存侧遵守同一条不变量（`agent_rpc::state::store::next_item_at`）。
struct CardStamps {
    clock: u64,
}

impl CardStamps {
    fn new() -> Self {
        Self { clock: 0 }
    }

    /// 取下一个时间戳：**不比上一张小**。消息自己的时间戳更大时用它（与真实时间贴合），
    /// 否则顺延 1ms——顺延量只用来表达顺序，没有任何地方把它当墙上时钟显示。
    fn next(&mut self, preferred: u64) -> u64 {
        self.clock = if preferred > self.clock { preferred } else { self.clock + 1 };
        self.clock
    }
}

impl SessionManager {
    pub fn new(explicit_dir: Option<PathBuf>) -> Self {
        Self { explicit_dir }
    }

    /// 获取会话持久化根目录（`~/.a-da/sessions` 或指定目录）
    pub fn sessions_dir(&self) -> PathBuf {
        match &self.explicit_dir {
            Some(dir) => dir.clone(),
            None => get_app_home().join("sessions"),
        }
    }

    /// 确保会话持久化根目录存在
    fn ensure_sessions_dir(&self) -> Result<PathBuf> {
        let dir = self.sessions_dir();
        if !dir.exists() {
            fs::create_dir_all(&dir)
                .with_context(|| format!("创建会话目录失败: {}", dir.display()))?;
        }
        Ok(dir)
    }

    /// 获取某个工作区的专属散列目录
    pub fn workspace_dir(&self, workspace: &str) -> PathBuf {
        let slug = workspace_slug(workspace);
        self.sessions_dir().join(slug)
    }

    /// 工作区目录下的边车文件路径（`workspace.json`）
    fn workspace_pointer(&self, workspace: &str) -> PathBuf {
        self.workspace_dir(workspace).join("workspace.json")
    }

    /// 在工作区目录下记录边车文件 `workspace.json`，方便反向定位真实工作区路径
    pub fn remember_workspace(&self, workspace: &str) -> Result<()> {
        let dir = self.workspace_dir(workspace);
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }
        let pointer_path = self.workspace_pointer(workspace);
        let content = serde_json::json!({
            "workspace": workspace
        });
        let mut file = File::create(&pointer_path)?;
        serde_json::to_writer_pretty(&mut file, &content)?;
        file.write_all(b"\n")?;
        Ok(())
    }

    /// 获取指定会话的完整文件路径
    pub fn get_session_path(&self, workspace: &str, session_id: &str) -> PathBuf {
        let safe = safe_id(session_id);
        self.workspace_dir(workspace).join(format!("{}.jsonl", safe))
    }

    /// 创建新的持久化会话文件（幂等）
    pub fn create_session(
        &self,
        id: &str,
        workspace: &str,
        title: Option<&str>,
        parent_id: Option<&str>,
        subagent_id: Option<&str>,
    ) -> Result<SessionHeader> {
        self.create_session_bound(id, workspace, title, parent_id, subagent_id, None, None)
    }

    /// 与 [`Self::create_session`] 相同，但把**会话身份**（产品 id / 模式）一起写进 header。
    ///
    /// 为什么必须落盘：重启后 `restore_all_threads` 只能从 header 还原会话绑定的产品，
    /// 不写就只能一律当成 `ada-coding`——左侧树里的 PM 标识就是这么丢的。
    pub fn create_session_bound(
        &self,
        id: &str,
        workspace: &str,
        title: Option<&str>,
        parent_id: Option<&str>,
        subagent_id: Option<&str>,
        agent_id: Option<&str>,
        mode: Option<agent_proto::AgentMode>,
    ) -> Result<SessionHeader> {
        self.ensure_sessions_dir()?;
        let _ = self.remember_workspace(workspace);

        let file_path = self.get_session_path(workspace, id);
        if file_path.exists() {
            if let Ok(file) = File::open(&file_path) {
                let reader = BufReader::new(file);
                if let Some(Ok(first_line)) = reader.lines().next() {
                    if let Ok(header) = serde_json::from_str::<SessionHeader>(&first_line) {
                        if header.entry_type == "session" {
                            return Ok(header);
                        }
                    }
                }
            }
        }

        let now = agent_toolkit::now_ms();
        let header = SessionHeader {
            entry_type: "session".to_string(),
            version: CURRENT_SESSION_VERSION,
            id: id.to_string(),
            title: Some(title.unwrap_or("新会话").to_string()),
            workspace: workspace.to_string(),
            created_at: now,
            updated_at: now,
            parent_id: parent_id.map(|s| s.to_string()),
            subagent_id: subagent_id.map(|s| s.to_string()),
            agent_id: agent_id.map(|s| s.to_string()),
            mode: mode.map(mode_to_header),
            plugin_data: None,
        };

        let json_line = serde_json::to_string(&header)?;
        let mut file = File::create(&file_path)?;
        writeln!(file, "{}", json_line)?;

        Ok(header)
    }

    /// 更新会话身份（产品 id / 模式）——切换模式或换 Agent 时必须同步落盘，
    /// 否则重启后又变回默认的 `ada-coding`。
    pub fn update_session_identity(
        &self,
        session_id: &str,
        agent_id: Option<&str>,
        mode: Option<agent_proto::AgentMode>,
        workspace: Option<&str>,
    ) -> Result<()> {
        let file_path = if let Some(ws) = workspace {
            let path = self.get_session_path(ws, session_id);
            if !path.exists() {
                self.create_session(session_id, ws, None, None, None)?;
            }
            path
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(());
        };

        self.update_header(&file_path, |header| {
            if let Some(a) = agent_id {
                header.agent_id = Some(a.to_string());
            }
            if let Some(m) = mode {
                header.mode = Some(mode_to_header(m));
            }
            header.updated_at = agent_toolkit::now_ms();
        })
    }

    /// 寻找某个会话当前所在的实际文件路径（未指明工作区时跨目录扫描）
    pub fn find_session_path(&self, session_id: &str) -> Option<PathBuf> {
        let safe = safe_id(session_id);
        let target_filename = format!("{}.jsonl", safe);
        let root = self.sessions_dir();
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        let candidate = entry.path().join(&target_filename);
                        if candidate.exists() {
                            return Some(candidate);
                        }
                    }
                }
            }
        }
        None
    }

    /// 解析"要写进哪个会话文件"：给了工作区就按其目录取，否则跨目录扫。
    ///
    /// `Ok(None)` = 这个会话还没有文件（调用方**安静跳过**，与既有 append 语义一致）。
    fn resolve_write_path(&self, session_id: &str, workspace: Option<&str>) -> Result<Option<PathBuf>> {
        if let Some(ws) = workspace {
            let dir = self.workspace_dir(ws);
            if !dir.exists() {
                fs::create_dir_all(&dir)?;
            }
            Ok(Some(self.get_session_path(ws, session_id)))
        } else {
            Ok(self.find_session_path(session_id))
        }
    }

    /// 向会话以 Append-only 方式追加一条消息
    pub fn append_message(
        &self,
        session_id: &str,
        message: AgentMessage,
        workspace: Option<&str>,
    ) -> Result<()> {
        let Some(file_path) = self.resolve_write_path(session_id, workspace)? else {
            return Ok(());
        };

        // 行级写入与 id 规则走适配器的**同一份**实现（R2：落盘格式只有一个真源）。
        // 原先这里是内联的 `SessionMessageEntry` 构造 + 追加写，与
        // `agent_adapter::store::fs_store` 各写一份；W1-T2a 收敛掉。
        let entry = agent_adapter::store::jsonl::new_message_entry(agent_toolkit::now_ms(), message);
        agent_adapter::store::jsonl::append_message(&file_path, &entry)?;
        Ok(())
    }

    /// 向会话追加一条上下文压缩摘要记录
    pub fn append_compact_entry(
        &self,
        session_id: &str,
        mut compact_entry: SessionCompactEntry,
        workspace: Option<&str>,
    ) -> Result<()> {
        let Some(file_path) = self.resolve_write_path(session_id, workspace)? else {
            return Ok(());
        };

        compact_entry.entry_type = "compact".to_string();
        agent_adapter::store::jsonl::append_compact(&file_path, &compact_entry)?;
        Ok(())
    }

    /// 向会话追加一条 `notice` 记录（**面向人的提示，不进模型上下文**）。
    ///
    /// 用途是"模型请求失败"这类只该显示给人看的信息：早先它被写进助手正文，
    /// 于是既劈开了一段连贯回答，又会被当成模型自己说过的话回传给它。
    pub fn append_notice_entry(
        &self,
        session_id: &str,
        text: &str,
        level: &str,
        workspace: Option<&str>,
    ) -> Result<()> {
        let Some(file_path) = self.resolve_write_path(session_id, workspace)? else {
            return Ok(());
        };
        let entry = agent_adapter::store::jsonl::new_notice_entry(agent_toolkit::now_ms(), text, level);
        agent_adapter::store::jsonl::append_notice(&file_path, &entry)?;
        Ok(())
    }

    /// 删除该会话**所有** `notice` 记录（整份重写，保留其余行原样）。
    ///
    /// 重试时要连磁盘一起清：只清内存的话，1.5s 后的磁盘同步会发现两边不一致、
    /// 把整份 `items` 覆盖回去，界面上那张卡片"闪一下又回来"。
    /// 返回删掉的条数（`0` = 本来就没有，调用方可据此跳过重写）。
    pub fn clear_notice_entries(&self, session_id: &str) -> Result<usize> {
        let Some(file_path) = self.find_session_path(session_id) else {
            return Ok(0);
        };
        if !file_path.exists() {
            return Ok(0);
        }
        let content = fs::read_to_string(&file_path)?;
        let mut kept: Vec<&str> = Vec::new();
        let mut removed = 0usize;
        for line in content.split('\n') {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // 用解析而不是子串匹配：`text` 里出现的 `"type":"notice"` 不该被误判
            let is_notice = serde_json::from_str::<SessionEntry>(trimmed)
                .map(|e| matches!(e, SessionEntry::Notice(_)))
                .unwrap_or(false);
            if is_notice {
                removed += 1;
            } else {
                kept.push(line);
            }
        }
        if removed == 0 {
            return Ok(0);
        }
        let mut output = String::with_capacity(content.len());
        for line in kept {
            output.push_str(line);
            output.push('\n');
        }
        fs::write(&file_path, output)?;
        Ok(removed)
    }

    /// 更新会话标题
    pub fn update_session_title(
        &self,
        session_id: &str,
        title: &str,
        workspace: Option<&str>,
    ) -> Result<()> {
        let file_path = if let Some(ws) = workspace {
            let path = self.get_session_path(ws, session_id);
            if !path.exists() {
                self.create_session(session_id, ws, Some(title), None, None)?;
            }
            path
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(());
        };

        self.update_header(&file_path, |header| {
            header.title = Some(title.to_string());
            header.updated_at = agent_toolkit::now_ms();
        })
    }

    /// 更新会话的元数据（父子关系、插件数据）
    pub fn update_session_meta(
        &self,
        session_id: &str,
        parent_id: Option<Option<String>>,
        subagent_id: Option<Option<String>>,
        plugin_data: Option<HashMap<String, serde_json::Value>>,
        workspace: Option<&str>,
    ) -> Result<()> {
        let file_path = if let Some(ws) = workspace {
            let path = self.get_session_path(ws, session_id);
            if !path.exists() {
                self.create_session(session_id, ws, None, None, None)?;
            }
            path
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(());
        };

        self.update_header(&file_path, |header| {
            if let Some(pid) = parent_id {
                header.parent_id = pid;
            }
            if let Some(sid) = subagent_id {
                header.subagent_id = sid;
            }
            if let Some(data) = plugin_data {
                let current = header.plugin_data.get_or_insert_with(HashMap::new);
                for (k, v) in data {
                    current.insert(k, v);
                }
            }
            header.updated_at = agent_toolkit::now_ms();
        })
    }

    /// 辅助方法：原子更新第一行 Header 并重写保留后续内容
    fn update_header<F>(&self, file_path: &Path, update_fn: F) -> Result<()>
    where
        F: FnOnce(&mut SessionHeader),
    {
        if !file_path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(file_path)?;
        let lines: Vec<&str> = content.split('\n').collect();
        if lines.is_empty() || lines[0].trim().is_empty() {

            return Ok(());
        }

        let mut header: SessionHeader = serde_json::from_str(lines[0])?;
        if header.entry_type != "session" {
            return Ok(());
        }

        update_fn(&mut header);
        let new_first_line = serde_json::to_string(&header)?;

        let mut output = String::with_capacity(content.len() + 64);
        output.push_str(&new_first_line);
        output.push('\n');

        for line in &lines[1..] {
            if !line.trim().is_empty() {
                output.push_str(line);
                output.push('\n');
            }
        }

        fs::write(file_path, output)?;
        Ok(())
    }

    /// 读取并重构会话的历史消息
    pub fn load_session(
        &self,
        session_id: &str,
        workspace: Option<&str>,
    ) -> Result<Option<(SessionHeader, Vec<AgentMessage>)>> {
        let file_path = if let Some(ws) = workspace {
            self.get_session_path(ws, session_id)
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(None);
        };

        if !file_path.exists() {
            return Ok(None);
        }

        // 行级读取与折叠语义走适配器的**同一份**实现（R2）。
        // 原先这里是内联的逐行解析 + compact 折叠，与
        // `agent_adapter::store::fs_store` 各写一份；W1-T2a 收敛掉。
        let outcome = agent_adapter::store::jsonl::read_entries(&file_path)?;
        if outcome.skipped_lines > 0 {
            tracing::warn!(
                "会话 {} 有 {} 行无法解析，已跳过（文件：{}）",
                session_id,
                outcome.skipped_lines,
                file_path.display()
            );
        }
        let (header, messages) = agent_adapter::store::jsonl::fold_messages(&outcome.entries);

        match header {
            Some(h) => Ok(Some((h, messages))),
            None => Ok(None),
        }
    }

    /// 读一个工作区目录的 `workspace.json`
    fn read_workspace_pointer(&self, dir: &Path) -> Option<String> {
        let pointer_path = dir.join("workspace.json");
        if !pointer_path.exists() {
            return None;
        }
        let content = fs::read_to_string(pointer_path).ok()?;
        let val: serde_json::Value = serde_json::from_str(&content).ok()?;
        val.get("workspace")
            .and_then(|w| w.as_str())
            .map(|s| s.to_string())
    }

    /// 列举指定工作区的所有历史会话摘要（按 updatedAt 倒序）
    pub fn list_sessions_for_workspace(&self, workspace: &str) -> Result<Vec<SessionSummary>> {
        let dir = self.workspace_dir(workspace);
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut summaries = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return Ok(Vec::new()),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }

            if let Ok(file) = File::open(&path) {
                let mut reader = BufReader::new(file);
                let mut first_line = String::new();
                if reader.read_line(&mut first_line).is_ok() && !first_line.trim().is_empty() {
                    if let Ok(header) = serde_json::from_str::<SessionHeader>(first_line.trim()) {
                        if header.entry_type == "session" {
                            let metadata = entry.metadata().ok();
                            let mtime = metadata
                                .and_then(|m| m.modified().ok())
                                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                                .map(|d| d.as_millis() as i64)
                                .unwrap_or(header.updated_at);

                            summaries.push(SessionSummary {
                                id: header.id,
                                title: header.title.unwrap_or_else(|| "新会话".to_string()),
                                workspace: header.workspace,
                                created_at: header.created_at,
                                updated_at: mtime,
                                file_path: path.to_string_lossy().to_string(),
                                parent_id: header.parent_id,
                                subagent_id: header.subagent_id,
                            });
                        }
                    }
                }
            }
        }

        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(summaries)
    }

    /// 列举所有工作区的全部会话摘要
    pub fn list_all_sessions(&self) -> Result<Vec<SessionSummary>> {
        let root = self.sessions_dir();
        if !root.exists() {
            return Ok(Vec::new());
        }

        let mut summaries = Vec::new();
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(workspace) = self.read_workspace_pointer(&path) {
                        if let Ok(list) = self.list_sessions_for_workspace(&workspace) {
                            summaries.extend(list);
                        }
                    }
                }
            }
        }

        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(summaries)
    }

    /// 还原所有工作区的所有持久化会话（深度重建为 UI Item 卡片流与 Messages 上下文）
    /// **便宜的会话文件“变没变”探针**：只 stat，不解析。
    ///
    /// 为什么需要：`restore_all_threads` 会把每个会话文件重新解析一遍，实测环境
    /// （2.8MB + 1.3MB + 454KB + …）是**秒级** CPU，而它每 1.5 秒被调用一次、
    /// 还握着 store 写锁——用户“点批准要等 2–6 秒”就是这么来的。
    /// 先用这个探针比对 `(mtime, len)`：一个文件都没动就**一次解析都不做**。
    pub fn session_file_stamps(&self) -> Vec<(std::path::PathBuf, u64, u64)> {
        let mut out = Vec::new();
        let root = self.sessions_dir();
        let Ok(entries) = std::fs::read_dir(&root) else { return out };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if !ft.is_dir() { continue }
            let Ok(files) = std::fs::read_dir(entry.path()) else { continue };
            for f in files.flatten() {
                let p = f.path();
                if !p.extension().map(|e| e == "jsonl").unwrap_or(false) { continue }
                if let Ok(md) = f.metadata() {
                    let mtime_ms = md.modified().ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    out.push((p, mtime_ms, md.len()));
                }
            }
        }
        out.sort();
        out
    }

    pub fn restore_all_threads(&self) -> Vec<Thread> {
        let root = self.sessions_dir();
        if !root.exists() {
            return Vec::new();
        }

        let mut session_files: Vec<(PathBuf, Option<String>)> = Vec::new();

        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                    // 根目录下直接存在的 .jsonl 文件
                    session_files.push((path, None));
                } else if path.is_dir() {
                    let ws = self.read_workspace_pointer(&path);
                    if let Ok(sub_entries) = fs::read_dir(&path) {
                        for sub_entry in sub_entries.flatten() {
                            let sub_path = sub_entry.path();
                            if sub_path.is_file() && sub_path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                                session_files.push((sub_path, ws.clone()));
                            }
                        }
                    }
                }
            }
        }

        let mut threads_with_mtime: Vec<(Thread, i64)> = Vec::new();

        for (file_path, pointer_ws) in session_files {
            let file = match File::open(&file_path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let reader = BufReader::new(file);
            let mut lines = reader.lines();

            let first_line = match lines.next() {
                Some(Ok(l)) if !l.trim().is_empty() => l,
                _ => continue,
            };
            let header: SessionHeader = match serde_json::from_str::<SessionHeader>(first_line.trim()) {
                Ok(h) if h.entry_type == "session" => h,
                _ => continue,
            };

            let file_mtime = file_path.metadata().ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(header.updated_at);

            // 工作区归属：会话头 → 目录边车文件 → 公共区。
            // 不回落到进程当前目录：从哪个目录启动进程，不代表历史会话属于那个目录
            let final_workspace = {
                let header_ws = header.workspace.trim();
                let pointer = pointer_ws
                    .as_deref()
                    .map(str::trim)
                    .filter(|ws| !ws.is_empty());
                if !header_ws.is_empty() {
                    header_ws.to_string()
                } else if let Some(ws) = pointer {
                    ws.to_string()
                } else {
                    get_app_home().join("workspace").to_string_lossy().to_string()
                }
            };

            let mut raw_entries: Vec<SessionEntry> = Vec::new();
            let mut tool_results: HashMap<String, AgentMessage> = HashMap::new();

            for line_res in lines {
                let line = match line_res {
                    Ok(l) => l,
                    Err(_) => continue,
                };
                let trimmed = line.trim();
                if trimmed.is_empty() { continue; }

                if let Ok(entry) = serde_json::from_str::<SessionEntry>(trimmed) {
                    if let SessionEntry::Message(ref m) = entry {
                        if let AgentMessage::ToolResult { ref tool_call_id, .. } = m.message {
                            tool_results.insert(tool_call_id.clone(), m.message.clone());
                        }
                    }
                    raw_entries.push(entry);
                }
            }

            let mut items: Vec<Item> = Vec::new();
            let mut messages: Vec<serde_json::Value> = Vec::new();
            let mut rendered_tool_call_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            // 卡片时间戳按**折叠顺序**发（见 `CardStamps`）
            let mut stamps = CardStamps::new();

            for (index, entry) in raw_entries.into_iter().enumerate() {
                match entry {
                    SessionEntry::Header(_) => {}
                    // 提示是**给人看的**：还原成一张卡片，但**不进 `messages`**
                    // （上下文里绝不能出现它，否则界面上的报错会变成"模型说过的话"）。
                    SessionEntry::Notice(n) => {
                        items.push(Item::Notice {
                            id: n.id.clone(),
                            at: stamps.next(n.timestamp as u64),
                            text: n.text.clone(),
                            level: n.level.clone(),
                        });
                    }
                    SessionEntry::Compact(c) => {
                        let _pruned = std::mem::take(&mut items);
                        items.push(Item::Compact {
                            id: c.id.clone(),
                            at: stamps.next(c.timestamp as u64),
                            summary: c.summary.clone(),
                            pre_tokens: c.pre_tokens,
                            post_tokens: c.post_tokens,
                            saved_tokens: c.saved_tokens,
                            turns_summarized: c.turns_summarized as u64,
                        });

                        let continuation_msg = serde_json::json!({
                            "role": "user",
                            "content": format!("[系统自动压缩摘要]\n{}", c.summary),
                            "timestamp": c.timestamp,
                        });
                        messages.clear();
                        messages.push(continuation_msg);
                    }
                    SessionEntry::Message(m) => {
                        let at = match &m.message {
                            AgentMessage::User { timestamp, .. } => timestamp.unwrap_or(header.created_at) as u64,
                            AgentMessage::Assistant { timestamp, .. } => timestamp.unwrap_or(header.created_at) as u64,
                            AgentMessage::ToolResult { timestamp, .. } => timestamp.unwrap_or(header.created_at) as u64,
                            _ => header.created_at as u64,
                        };

                        match m.message {
                            AgentMessage::User { content, images, .. } => {
                                if content.starts_with("This session is being continued from a previous conversation") {
                                    continue;
                                }
                                messages.push(serde_json::json!({
                                    "role": "user",
                                    "content": &content,
                                    "images": &images,
                                    "timestamp": at,
                                }));
                                items.push(Item::User {
                                    id: format!("{}_restored_user_{}", header.id, index),
                                    at: stamps.next(at),
                                    text: content,
                                    images,
                                    queued: None,
                                });
                            }
                            AgentMessage::Assistant { content, thinking, tool_calls, usage, duration_ms, turn_duration_ms, stop_reason, error_message, .. } => {
                                // 用量与耗时是遥测条与单条回复徽章的数据源，恢复时原样带回卡片
                                let restored_usage = usage
                                    .as_ref()
                                    .and_then(|v| serde_json::from_value::<agent_base::model::TokenUsage>(v.clone()).ok());
                                let mut asst_val = serde_json::json!({
                                    "role": "assistant",
                                    "content": &content,
                                    "timestamp": at,
                                });
                                if let Some(th) = &thinking { asst_val["thinking"] = serde_json::json!(th); }
                                if let Some(tc) = &tool_calls { asst_val["toolCalls"] = serde_json::to_value(tc).unwrap_or_default(); }
                                if let Some(us) = &usage { asst_val["usage"] = us.clone(); }
                                if let Some(d) = duration_ms { asst_val["durationMs"] = serde_json::json!(d); }
                                if let Some(td) = turn_duration_ms { asst_val["turnDurationMs"] = serde_json::json!(td); }
                                if let Some(sr) = &stop_reason { asst_val["stopReason"] = serde_json::json!(sr); }
                                if let Some(em) = &error_message { asst_val["errorMessage"] = serde_json::json!(em); }
                                messages.push(asst_val);

                                if let Some(th) = thinking {
                                    if !th.trim().is_empty() {
                                        let think_at = stamps.next(at);
                                        items.push(Item::Thinking {
                                            id: format!("{}_restored_think_{}", header.id, index),
                                            at: think_at,
                                            text: th,
                                            ended_at: Some(think_at),
                                        });
                                    }
                                }

                                // 🔴 **正文排在工具卡前面**：这条消息里的正文是"我要做什么"的引子，
                                // 引擎是**流结束后**才执行工具调用的，所以现场看到的顺序恒为
                                // 思考 → 正文 → 工具卡。早先这里把正文压到最后，于是轮次一收尾
                                // （内存卡片被磁盘回填整份换掉）正文会**跳到自己的工具卡下面**——
                                // 与思考块掉下去是同一个事故的两种形态（实机验收
                                // `tools/verify-card-order-e2e.mjs --with-tool` 钉着顺序不变）。
                                if !content.trim().is_empty() {
                                    items.push(Item::Assistant {
                                        id: format!("{}_restored_asst_{}", header.id, index),
                                        at: stamps.next(at),
                                        text: content,
                                        streaming: Some(false),
                                        duration_ms,
                                        turn_duration_ms,
                                        usage: restored_usage,
                                    });
                                }

                                if let Some(calls) = tool_calls {
                                    for call in calls {
                                        rendered_tool_call_ids.insert(call.id.clone());
                                        let res = tool_results.get(&call.id);
                                        let mut is_denied = false;
                                        let mut is_error = false;
                                        let mut output: Option<String> = None;
                                        let mut patch: Option<String> = None;
                                        let mut checkpoint_id: Option<String> = None;
                                        let mut details: Option<serde_json::Value> = None;
                                        let mut tool_at = at;
                                        let mut duration_ms: Option<u64> = None;
                                        let mut started_at: Option<i64> = None;
                                        let mut finished_at: Option<i64> = None;

                                        if let Some(AgentMessage::ToolResult {
                                            content: res_content,
                                            is_error: res_err,
                                            details: res_det,
                                            patch: res_patch,
                                            checkpoint_id: res_cp,
                                            timestamp: res_ts,
                                            duration_ms: res_dur,
                                            started_at: res_st,
                                            finished_at: res_fin,
                                            ..
                                        }) = res {
                                            if res_content == "用户拒绝了此工具调用" || res_content == "用户拒绝了这次调用。不要重试同样的调用，先说明原因或换一种做法。" {
                                                is_denied = true;
                                            }
                                            is_error = res_err.unwrap_or(false);
                                            output = Some(res_content.clone());
                                            patch = res_patch.clone();
                                            checkpoint_id = res_cp.clone();
                                            details = res_det.clone();
                                            if let Some(ts) = res_ts { tool_at = *ts as u64; }
                                            duration_ms = *res_dur;
                                            started_at = *res_st;
                                            finished_at = *res_fin;
                                            if duration_ms.is_none() {
                                                if let Ok(val) = serde_json::from_str::<serde_json::Value>(res_content) {
                                                    duration_ms = val.get("duration_ms").and_then(|v| v.as_u64()).or_else(|| val.get("durationMs").and_then(|v| v.as_u64()));
                                                    if started_at.is_none() {
                                                        started_at = val.get("started_at").and_then(|v| v.as_i64()).or_else(|| val.get("startedAt").and_then(|v| v.as_i64()));
                                                    }
                                                    if finished_at.is_none() {
                                                        finished_at = val.get("finished_at").and_then(|v| v.as_i64()).or_else(|| val.get("finishedAt").and_then(|v| v.as_i64()));
                                                    }
                                                }
                                            }
                                        }

                                        let status = if is_denied {
                                            "denied".to_string()
                                        } else if is_error {
                                            "error".to_string()
                                        } else {
                                            "done".to_string()
                                        };

                                        let raw_args = if call.raw_arguments.is_empty() {
                                            serde_json::to_string(&call.arguments).unwrap_or_default()
                                        } else {
                                            call.raw_arguments
                                        };

                                        items.push(Item::Tool {
                                            id: format!("{}_restored_tool_{}", header.id, call.id),
                                            at: stamps.next(tool_at),
                                            call_id: call.id,
                                            name: call.name,
                                            args: call.arguments,
                                            raw_args,
                                            status,
                                            output,
                                            patch,
                                            details,
                                            thread_id: Some(header.id.clone()),
                                            checkpoint_id,
                                            reverted: None,
                                            duration_ms,
                                            started_at,
                                            finished_at,
                                        });
                                    }
                                }
                            }
                            AgentMessage::ToolResult {
                                tool_call_id,
                                tool_name,
                                content,
                                is_error,
                                details,
                                patch,
                                checkpoint_id,
                                timestamp: _,
                                duration_ms,
                                started_at,
                                finished_at,
                                status: res_status,
                            } => {
                                let mut dur = duration_ms;
                                let mut st = started_at;
                                let mut fin = finished_at;
                                if dur.is_none() {
                                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                                        dur = val.get("duration_ms").and_then(|v| v.as_u64()).or_else(|| val.get("durationMs").and_then(|v| v.as_u64()));
                                        if st.is_none() {
                                            st = val.get("started_at").and_then(|v| v.as_i64()).or_else(|| val.get("startedAt").and_then(|v| v.as_i64()));
                                        }
                                        if fin.is_none() {
                                            fin = val.get("finished_at").and_then(|v| v.as_i64()).or_else(|| val.get("finishedAt").and_then(|v| v.as_i64()));
                                        }
                                    }
                                }

                                messages.push(serde_json::json!({
                                    "role": "toolResult",
                                    "toolCallId": &tool_call_id,
                                    "toolName": &tool_name,
                                    "content": &content,
                                    "isError": is_error,
                                    "details": &details,
                                    "patch": &patch,
                                    "checkpointId": &checkpoint_id,
                                    "timestamp": at,
                                    "durationMs": dur,
                                    "startedAt": st,
                                    "finishedAt": fin,
                                    "status": res_status,
                                }));

                                if !rendered_tool_call_ids.contains(&tool_call_id) {
                                    rendered_tool_call_ids.insert(tool_call_id.clone());
                                    let is_denied = content == "用户拒绝了此工具调用" || content == "用户拒绝了这次调用。不要重试同样的调用，先说明原因或换一种做法。";
                                    let status = if is_denied {
                                        "denied".to_string()
                                    } else if is_error.unwrap_or(false) {
                                        "error".to_string()
                                    } else {
                                        "done".to_string()
                                    };
                                    items.push(Item::Tool {
                                        id: format!("{}_restored_tool_{}", header.id, tool_call_id),
                                        at: stamps.next(at),
                                        call_id: tool_call_id,
                                        name: tool_name,
                                        args: serde_json::json!({}),
                                        raw_args: String::new(),
                                        status,
                                        output: Some(content),
                                        patch,
                                        details,
                                        thread_id: Some(header.id.clone()),
                                        checkpoint_id,
                                        reverted: None,
                                        duration_ms: dur,
                                        started_at: st,
                                        finished_at: fin,
                                    });
                                }
                            }
                            AgentMessage::Unknown => {}
                        }
                    }
                }
            }

            let is_sub = header.parent_id.is_some() || header.subagent_id.is_some();
            // 会话身份以 header 为准（创建/切换时落的盘）；老文件没有这两个字段才退回缺省，
            // 否则重启后 PM 会话会被一律当成 ada-coding（左侧树标识就丢了）。
            //
            // **PM 降级为模式后的归一**：历史上 PM 是一个独立产品，`agent_id` 会写成
            // `ada-pm`/`pm-assistant`。现在 `agent_id` 恒为本机唯一产品 `ada-coding`，
            // PM 由 `mode` 表达——所以这里把老身份**读时归一并写回**（幂等，只改一次）。
            let raw_agent = header.agent_id.clone();
            let legacy_pm_agent = matches!(raw_agent.as_deref(), Some("ada-pm") | Some("pm-assistant"));
            let restored_agent = Some(if legacy_pm_agent {
                "ada-coding".to_string()
            } else {
                raw_agent.clone().unwrap_or_else(|| "ada-coding".to_string())
            });
            let restored_mode = header
                .mode
                .as_deref()
                .and_then(mode_from_header)
                .or_else(|| {
                    // 老文件兜底：PM 身份至少能从 agent_id 推出来
                    match raw_agent.as_deref() {
                        Some("ada-pm") | Some("pm-assistant") => Some(AgentMode::Pm),
                        _ => Some(AgentMode::Code),
                    }
                });
            // 归一写回：让磁盘上的老会话也只剩"一个产品 + 一个模式"这一个真源
            if legacy_pm_agent {
                let _ = self.update_header(&file_path, |h| {
                    h.agent_id = Some("ada-coding".to_string());
                    h.mode = Some(mode_to_header(restored_mode.unwrap_or(AgentMode::Pm)));
                    h.updated_at = agent_toolkit::now_ms();
                });
            }
            // 自愈式回填：老的子会话标题没有唯一短 id（同 profile 连派多个时完全一样，
            // 标签页/侧边栏里分不出谁是谁）。这里按新格式补一次，之后标题里已含 id 就不再动。
            let raw_title = header.title.clone().unwrap_or_else(|| "新会话".to_string());
            let title = if is_sub {
                backfill_subagent_title(&raw_title, &header.id, &file_path, |path, new_title| {
                    self.update_header(path, |h| {
                        h.title = Some(new_title.to_string());
                        h.updated_at = agent_toolkit::now_ms();
                    })
                })
            } else {
                raw_title
            };

            let thread = Thread {
                id: header.id,
                title,
                created_at: header.created_at as u64,
                workspace: final_workspace,
                item_count: items.len(),
                updated_at: items
                    .iter()
                    .map(|item| item.at())
                    .max()
                    .unwrap_or(0)
                    .max(header.updated_at.max(0) as u64),
                items,
                messages,
                mode: restored_mode,
                agent_id: restored_agent,
                parent_id: header.parent_id,
                subagent_id: header.subagent_id,
                is_subagent: Some(is_sub),
                plugin_data: header.plugin_data,
            };

            threads_with_mtime.push((thread, file_mtime));
        }

        threads_with_mtime.sort_by(|a, b| b.1.cmp(&a.1));
        let mut threads: Vec<Thread> = threads_with_mtime.into_iter().map(|(t, _)| t).collect();

        // 纠偏与自愈：如果子会话缺少 parent_id，从各级会话的 invoke_subagent 卡片中寻找
        let mut sub_to_parent: HashMap<String, String> = HashMap::new();
        for t in &threads {
            for it in &t.items {
                if let Item::Tool { name, details, output, .. } = it {
                    if name == "invoke_subagent" {
                        if let Some(sub_id) = details.as_ref().and_then(|d| d.get("subagent_thread_id").or_else(|| d.get("threadId"))).and_then(|v| v.as_str()) {
                            sub_to_parent.insert(sub_id.to_string(), t.id.clone());
                        } else if let Some(out) = output {
                            for word in out.split_whitespace() {
                                if word.starts_with("subagent_") {
                                    sub_to_parent.insert(word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_').to_string(), t.id.clone());
                                }
                            }
                        }
                    }
                }
            }
        }

        for t in &mut threads {
            if t.is_subagent == Some(true) && t.parent_id.is_none() {
                if let Some(pid) = sub_to_parent.get(&t.id) {
                    t.parent_id = Some(pid.clone());
                }
            }
        }

        threads
    }

    /// 删除指定会话并级联删除名下的子会话
    pub fn delete_session(&self, session_id: &str, workspace: Option<&str>) -> Result<()> {
        let file_path = if let Some(ws) = workspace {
            self.get_session_path(ws, session_id)
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(());
        };

        if file_path.exists() {
            let _ = fs::remove_file(&file_path);
        }

        // 级联删除名下的所有子智能体会话
        if let Some(parent_dir) = file_path.parent() {
            if let Some(ws) = self.read_workspace_pointer(parent_dir) {
                if let Ok(sub_list) = self.list_sessions_for_workspace(&ws) {
                    for item in sub_list {
                        if item.parent_id.as_deref() == Some(session_id) {
                            let _ = fs::remove_file(&item.file_path);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// 归档一份会话副本（`<id>.jsonl.archived`）
    pub fn archive_session(&self, session_id: &str, workspace: Option<&str>) -> Result<bool> {
        let file_path = if let Some(ws) = workspace {
            self.get_session_path(ws, session_id)
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(false);
        };

        if !file_path.exists() {
            return Ok(false);
        }

        let archived_path = PathBuf::from(format!("{}.archived", file_path.display()));
        fs::copy(&file_path, &archived_path)?;
        Ok(true)
    }

    /// 删除整个工作区目录
    pub fn delete_workspace(&self, workspace: &str) -> Result<()> {
        let dir = self.workspace_dir(workspace);
        if dir.exists() {
            fs::remove_dir_all(dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 端到端：老格式子会话在**读盘时**就被补上短 id，并且**写回磁盘**、且只写一次。
    #[test]
    fn test_restore_backfills_old_subagent_title_on_disk() -> Result<()> {
        let temp_dir = std::env::temp_dir().join(format!("a_da_title_backfill_{}", uuid::Uuid::new_v4()));
        let manager = SessionManager::new(Some(temp_dir.clone()));
        let ws = "E:/backfill_ws";
        let child = "thread_1791721619138_36fb30";

        manager.create_session("thread_parent_x", ws, Some("父会话"), None, None)?;
        manager.create_session(
            child,
            ws,
            Some("子代理 [全能执行专员]: 在工作区 E:/x 中创建"),
            Some("thread_parent_x"),
            Some("general_purpose"),
        )?;

        let threads = SessionManager::new(Some(temp_dir.clone())).restore_all_threads();
        let t = threads.iter().find(|t| t.id == child).expect("子会话必须被还原");
        assert_eq!(
            t.title,
            "子代理 #36fb30 [全能执行专员]: 在工作区 E:/x 中创建",
            "还原出来的标题必须带唯一短 id"
        );

        let path = manager.get_session_path(ws, child);
        let first = std::fs::read_to_string(&path)?;
        assert!(first.contains("#36fb30"), "回填必须写回磁盘（否则重启又变回一样）：{first}");

        let _ = SessionManager::new(Some(temp_dir.clone())).restore_all_threads();
        let second = std::fs::read_to_string(&path)?;
        assert_eq!(first, second, "回填必须幂等：第二次读盘不得再改写文件");

        let _ = std::fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    /// 守门：老子会话标题的**唯一短 id 回填**必须幂等，且不动用户改过的名字。
    #[test]
    fn test_backfill_subagent_title_is_idempotent() {
        let path = Path::new("E:/x/thread_1_abcdef.jsonl");
        let mut writes: Vec<String> = Vec::new();

        // 1. 老格式 → 补上短 id（与新建时的格式一致）
        let out = backfill_subagent_title("子代理 [全能执行专员]: 在工作区 E:/x 中创建", "thread_1791721619138_36fb30", path, |_, t| {
            writes.push(t.to_string());
            Ok(())
        });
        assert_eq!(out, "子代理 #36fb30 [全能执行专员]: 在工作区 E:/x 中创建");
        assert_eq!(writes.len(), 1, "老标题要写回一次");

        // 2. 幂等：已含短 id 的标题不再写盘
        let out = backfill_subagent_title(&out, "thread_1791721619138_36fb30", path, |_, t| {
            writes.push(t.to_string());
            Ok(())
        });
        assert!(out.starts_with("子代理 #36fb30"), "{out}");
        assert_eq!(writes.len(), 1, "已经带 id 的标题不得反复改写");

        // 3. 用户自己改过名（不以「子代理」开头）的不动
        let out = backfill_subagent_title("我的调研助手", "thread_1791721619138_36fb30", path, |_, t| {
            writes.push(t.to_string());
            Ok(())
        });
        assert_eq!(out, "我的调研助手");
        assert_eq!(writes.len(), 1);

        // 4. 写盘失败 → 标题退回原样（不影响还原）
        let out = backfill_subagent_title("子代理 [X]: 任务", "thread_1_abcdef", path, |_, _| {
            Err(anyhow::anyhow!("disk full"))
        });
        assert_eq!(out, "子代理 [X]: 任务");
    }

    /// 守门：**会话身份必须落盘**，重启后左侧树的 PM/CODING 标识才不会丢。
    ///
    /// 早先 `SessionHeader` 里根本没有 `agentId`/`mode`，`restore_all_threads` 一律
    /// 写死 `ada-coding` —— 于是 PM 会话重启后全部变成 CODING。
    #[test]
    fn test_session_identity_survives_restore() -> Result<()> {
        let temp_dir = std::env::temp_dir().join(format!("a_da_identity_{}", uuid::Uuid::new_v4()));
        let manager = SessionManager::new(Some(temp_dir.clone()));
        let ws = "E:/identity_ws";

        // 1. PM 会话：**产品恒为 ada-coding**，PM 由 mode 表达
        let header = manager.create_session_bound(
            "thread_pm_1",
            ws,
            Some("PM 会话"),
            None,
            None,
            Some("ada-coding"),
            Some(agent_proto::AgentMode::Pm),
        )?;
        assert_eq!(header.agent_id.as_deref(), Some("ada-coding"));
        assert_eq!(header.mode.as_deref(), Some("pm"));

        // 2. 重启（重新构造管理器）后还原
        let restarted = SessionManager::new(Some(temp_dir.clone()));
        let threads = restarted.restore_all_threads();
        let pm = threads
            .iter()
            .find(|t| t.id == "thread_pm_1")
            .expect("PM 会话必须被还原");
        assert_eq!(pm.agent_id.as_deref(), Some("ada-coding"));
        assert_eq!(
            pm.mode,
            Some(agent_proto::AgentMode::Pm),
            "PM 必须靠**模式**活下来（产品只有一个）"
        );

        // 3. 老文件（header 里没有 agentId/mode）仍要能解析，并按缺省处理
        let legacy_path = manager.get_session_path(ws, "thread_legacy_1");
        std::fs::write(
            &legacy_path,
            "{\"type\":\"session\",\"version\":1,\"id\":\"thread_legacy_1\",\"title\":\"老会话\",\"workspace\":\"E:/identity_ws\",\"createdAt\":1,\"updatedAt\":1}\n",
        )?;
        let threads = SessionManager::new(Some(temp_dir.clone())).restore_all_threads();
        let legacy = threads
            .iter()
            .find(|t| t.id == "thread_legacy_1")
            .expect("老会话必须被还原");
        assert_eq!(legacy.agent_id.as_deref(), Some("ada-coding"));
        assert_eq!(legacy.mode, Some(agent_proto::AgentMode::Code));

        // 4. 运行期切换模式也要落盘（PM 模式不改产品身份）
        manager.update_session_identity(
            "thread_legacy_1",
            Some("ada-coding"),
            Some(agent_proto::AgentMode::Pm),
            Some(ws),
        )?;
        let threads = SessionManager::new(Some(temp_dir.clone())).restore_all_threads();
        let switched = threads
            .iter()
            .find(|t| t.id == "thread_legacy_1")
            .expect("切换后的会话必须被还原");
        assert_eq!(switched.agent_id.as_deref(), Some("ada-coding"));
        assert_eq!(switched.mode, Some(agent_proto::AgentMode::Pm));

        // 5. **历史遗留身份自愈**：老盘上写成 `ada-pm` 的会话，读盘时归一为
        //    `ada-coding` + `mode=pm`，并且**写回磁盘**（幂等，只改一次）
        let old_pm_path = manager.get_session_path(ws, "thread_old_pm");
        std::fs::write(
            &old_pm_path,
            "{\"type\":\"session\",\"version\":1,\"id\":\"thread_old_pm\",\"title\":\"老 PM 会话\",\"workspace\":\"E:/identity_ws\",\"createdAt\":1,\"updatedAt\":1,\"agentId\":\"ada-pm\",\"mode\":\"pm\"}\n",
        )?;
        let threads = SessionManager::new(Some(temp_dir.clone())).restore_all_threads();
        let old_pm = threads
            .iter()
            .find(|t| t.id == "thread_old_pm")
            .expect("老 PM 会话必须被还原");
        assert_eq!(
            old_pm.agent_id.as_deref(),
            Some("ada-coding"),
            "PM 已降级为模式：产品身份必须归一"
        );
        assert_eq!(old_pm.mode, Some(agent_proto::AgentMode::Pm));
        let rewritten = std::fs::read_to_string(&old_pm_path)?;
        assert!(
            rewritten.contains("\"agentId\":\"ada-coding\""),
            "归一必须写回磁盘（否则每次重启都要再猜一遍）：{rewritten}"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn test_session_lifecycle() -> Result<()> {
        let temp_dir = std::env::temp_dir().join(format!("a_da_test_{}", uuid::Uuid::new_v4()));
        let manager = SessionManager::new(Some(temp_dir.clone()));

        let ws = "E:/test_project";
        let sid = "session_001";

        // 1. 创建会话
        let header = manager.create_session(sid, ws, Some("测试初始会话"), None, None)?;
        assert_eq!(header.id, sid);
        assert_eq!(header.title, Some("测试初始会话".to_string()));

        // 2. 追加消息
        manager.append_message(
            sid,
            AgentMessage::User {
                content: "你好 Rust 核心".to_string(),
                images: None,
                timestamp: Some(1000),
            },
            Some(ws),
        )?;

        manager.append_message(
            sid,
            AgentMessage::Assistant {
                content: "你好，已成功收到消息。".to_string(),
                thinking: None,
                tool_calls: None,
                stop_reason: Some("stop".to_string()),
                error_message: None,
                timestamp: Some(1001),
                usage: None,
                duration_ms: None,
                turn_duration_ms: None,
            },
            Some(ws),
        )?;

        // 3. 读取会话
        let loaded = manager.load_session(sid, Some(ws))?.expect("会话应存在");
        assert_eq!(loaded.0.id, sid);
        assert_eq!(loaded.1.len(), 2);

        // 4. 改名
        manager.update_session_title(sid, "更新后的标题", Some(ws))?;
        let loaded2 = manager.load_session(sid, Some(ws))?.expect("会话应存在");
        assert_eq!(loaded2.0.title, Some("更新后的标题".to_string()));
        assert_eq!(loaded2.1.len(), 2);

        // 5. 列举工作区会话
        let list = manager.list_sessions_for_workspace(ws)?;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, sid);
        assert_eq!(list[0].title, "更新后的标题");

        // 6. 列举全部会话
        let all = manager.list_all_sessions()?;
        assert_eq!(all.len(), 1);

        // 7. 归档
        assert!(manager.archive_session(sid, Some(ws))?);

        // 8. 清理临时目录
        let _ = fs::remove_dir_all(temp_dir);
        Ok(())
    }

    #[test]
    fn test_restore_all_threads_and_cards() -> Result<()> {
        let temp_dir = std::env::temp_dir().join(format!("a_da_test_{}", uuid::Uuid::new_v4()));
        let manager = SessionManager::new(Some(temp_dir.clone()));

        let ws = "E:/code/test_ws";
        let sid = "session_deep_restore";

        manager.create_session(sid, ws, Some("会话卡片恢复测试"), None, None)?;

        // 1. 用户消息
        manager.append_message(
            sid,
            AgentMessage::User {
                content: "请帮我运行 git status".to_string(),
                images: None,
                timestamp: Some(1000),
            },
            Some(ws),
        )?;

        // 2. 助手消息：思考链 + 工具调用
        let call_id = "call_git_001";
        manager.append_message(
            sid,
            AgentMessage::Assistant {
                content: "我将检查当前 git 状态。".to_string(),
                thinking: Some("用户需要了解 git 仓库分支与修改情况".to_string()),
                tool_calls: Some(vec![crate::session::ToolCallBlock {
                    id: call_id.to_string(),
                    name: "run_command".to_string(),
                    arguments: serde_json::json!({ "command": "git status" }),
                    raw_arguments: "{\"command\":\"git status\"}".to_string(),
                }]),
                stop_reason: Some("tool_calls".to_string()),
                error_message: None,
                timestamp: Some(1002),
                usage: None,
                duration_ms: Some(150),
                turn_duration_ms: Some(200),
            },
            Some(ws),
        )?;

        // 3. 工具结果返回
        manager.append_message(
            sid,
            AgentMessage::ToolResult {
                tool_call_id: call_id.to_string(),
                tool_name: "run_command".to_string(),
                content: "On branch main\nnothing to commit".to_string(),
                is_error: Some(false),
                details: None,
                patch: None,
                checkpoint_id: None,
                timestamp: Some(1005),
                status: Some("success".to_string()),
                duration_ms: Some(50),
                started_at: Some(1000),
                finished_at: Some(1050),
            },
            Some(ws),
        )?;

        // 4. 执行 restore_all_threads
        let threads = manager.restore_all_threads();
        assert_eq!(threads.len(), 1, "应还原出一个会话");
        let thread = &threads[0];
        assert_eq!(thread.id, sid);
        assert_eq!(thread.title, "会话卡片恢复测试");
        assert_eq!(thread.workspace, ws);

        // 验证 items 卡片流
        //
        // 🔴 顺序是「用户 → 思考 → 正文 → 工具卡」：**正文在工具卡前面**。
        // 这条消息里的正文是"我要做什么"的引子，而引擎是流结束后才执行工具调用的，
        // 所以现场（流式）看到的顺序恒为思考 → 正文 → 工具卡。
        // 早先这里把正文压到最后，于是轮次一收尾正文会跳到自己的工具卡下面
        // （见 `test_restored_cards_have_increasing_at_so_client_order_matches` 与
        // `tools/verify-card-order-e2e.mjs`）。
        assert_eq!(thread.items.len(), 4, "应生成 User, Thinking, Assistant, Tool 共 4 个卡片");
        match &thread.items[0] {
            Item::User { text, .. } => assert_eq!(text, "请帮我运行 git status"),
            other => panic!("第一张卡片应为 User 卡片，实际为 {:?}", other),
        }
        match &thread.items[1] {
            Item::Thinking { text, .. } => assert!(text.contains("用户需要了解")),
            other => panic!("第二张卡片应为 Thinking 卡片，实际为 {:?}", other),
        }
        match &thread.items[2] {
            Item::Assistant { text, .. } => assert_eq!(text, "我将检查当前 git 状态。"),
            other => panic!("第三张卡片应为 Assistant 卡片，实际为 {:?}", other),
        }
        match &thread.items[3] {
            Item::Tool { call_id: cid, name, status, output, .. } => {
                assert_eq!(cid, call_id);
                assert_eq!(name, "run_command");
                assert_eq!(status, "done");
                assert_eq!(output.as_deref(), Some("On branch main\nnothing to commit"));
            }
            other => panic!("第四张卡片应为 Tool 卡片，实际为 {:?}", other),
        }

        // 5. 清理
        let _ = fs::remove_dir_all(temp_dir);
        Ok(())
    }

    /// **回填出来的卡片，`at` 必须按折叠顺序严格递增**。
    ///
    /// 为什么这条是硬要求：轮次一收尾，内存里的卡片会被磁盘回填**整份换掉**（id 全变），
    /// 那一帧走的是增量通道，而增量帧的 `upserted` 是**按 id 排序**发的（帧内容要可复现），
    /// 界面因此只能按 `at` 给新卡片定位。同一条 assistant 消息派生的几张卡
    /// （思考 / 工具 / 正文）原本共用一个毫秒，先后就落到 id 字典序上，
    /// 而 `..._restored_asst_*` 恰恰排在 `..._restored_think_*` 前面——
    /// 用户看到的就是"对话一完成，思考块从最上面掉到最下面"（实测一条 132 张卡的会话里
    /// 有 68 张卡位置被换乱）。这条用例按**界面那条插入规则**重放一遍，钉住顺序。
    #[test]
    fn test_restored_cards_have_increasing_at_so_client_order_matches() -> Result<()> {
        let temp_dir = std::env::temp_dir().join(format!("a_da_test_{}", uuid::Uuid::new_v4()));
        let manager = SessionManager::new(Some(temp_dir.clone()));
        let ws = "E:/code/test_ws";
        let sid = "session_order_restore";
        manager.create_session(sid, ws, Some("回填顺序测试"), None, None)?;

        // 一条"思考 + 工具调用 + 正文"的助手消息：三张卡的**消息时间戳完全相同**
        manager.append_message(
            sid,
            AgentMessage::User { content: "跑一下 git status".to_string(), images: None, timestamp: Some(1000) },
            Some(ws),
        )?;
        let call_id = "call_order_001";
        manager.append_message(
            sid,
            AgentMessage::Assistant {
                content: "我看了一下，工作区是干净的。".to_string(),
                thinking: Some("先看看有没有未提交的改动".to_string()),
                tool_calls: Some(vec![crate::session::ToolCallBlock {
                    id: call_id.to_string(),
                    name: "run_command".to_string(),
                    arguments: serde_json::json!({ "command": "git status" }),
                    raw_arguments: "{\"command\":\"git status\"}".to_string(),
                }]),
                stop_reason: Some("stop".to_string()),
                error_message: None,
                timestamp: Some(2000),
                usage: None,
                duration_ms: None,
                turn_duration_ms: None,
            },
            Some(ws),
        )?;
        // 工具结果**也**落在同一个毫秒上（最容易踩的那一档）
        manager.append_message(
            sid,
            AgentMessage::ToolResult {
                tool_call_id: call_id.to_string(),
                tool_name: "run_command".to_string(),
                content: "nothing to commit".to_string(),
                is_error: Some(false),
                details: None,
                patch: None,
                checkpoint_id: None,
                timestamp: Some(2000),
                status: Some("success".to_string()),
                duration_ms: Some(10),
                started_at: Some(2000),
                finished_at: Some(2010),
            },
            Some(ws),
        )?;

        let threads = manager.restore_all_threads();
        let items = &threads.iter().find(|t| t.id == sid).expect("会话应被还原").items;

        let kinds: Vec<&str> = items
            .iter()
            .map(|i| match i {
                Item::User { .. } => "user",
                Item::Thinking { .. } => "thinking",
                Item::Tool { .. } => "tool",
                Item::Assistant { .. } => "assistant",
                Item::Notice { .. } => "notice",
                Item::Compact { .. } => "compact",
            })
            .collect();
        assert_eq!(kinds, vec!["user", "thinking", "assistant", "tool"], "折叠顺序：思考 → 正文 → 工具卡（与流式现场一致）");

        // ① 时间戳严格递增（同毫秒的卡片必须被顺延开）
        let times: Vec<u64> = items
            .iter()
            .map(|i| match i {
                Item::User { at, .. }
                | Item::Thinking { at, .. }
                | Item::Tool { at, .. }
                | Item::Assistant { at, .. }
                | Item::Notice { at, .. }
                | Item::Compact { at, .. } => *at,
            })
            .collect();
        for w in times.windows(2) {
            assert!(w[1] > w[0], "回填卡片的 at 必须严格递增，实际 {times:?}");
        }

        // ② 按**界面那条规则**重放：upserted 按 id 字典序到达，逐张按 `at` 稳定插入。
        //    这一条才是用户看到的东西——顺序对了，但 `at` 一样时它会按 id 排。
        let id_of = |i: &Item| -> String {
            match i {
                Item::User { id, .. }
                | Item::Thinking { id, .. }
                | Item::Tool { id, .. }
                | Item::Assistant { id, .. }
                | Item::Notice { id, .. }
                | Item::Compact { id, .. } => id.clone(),
            }
        };
        let at_of = |i: &Item| -> u64 {
            match i {
                Item::User { at, .. }
                | Item::Thinking { at, .. }
                | Item::Tool { at, .. }
                | Item::Assistant { at, .. }
                | Item::Notice { at, .. }
                | Item::Compact { at, .. } => *at,
            }
        };
        let mut frame: Vec<&Item> = items.iter().collect();
        frame.sort_by_key(|i| id_of(i));
        let mut client: Vec<&Item> = Vec::new();
        for it in frame {
            let at = at_of(it);
            let mut idx = client.len();
            while idx > 0 && at_of(client[idx - 1]) > at {
                idx -= 1;
            }
            client.insert(idx, it);
        }
        let client_kinds: Vec<&str> = client
            .iter()
            .map(|i| match i {
                Item::User { .. } => "user",
                Item::Thinking { .. } => "thinking",
                Item::Tool { .. } => "tool",
                Item::Assistant { .. } => "assistant",
                Item::Notice { .. } => "notice",
                Item::Compact { .. } => "compact",
            })
            .collect();
        assert_eq!(
            client_kinds, kinds,
            "界面按 at 重建出来的顺序必须与宿主数组一致（否则思考块会掉到正文下面）"
        );

        let _ = fs::remove_dir_all(temp_dir);
        Ok(())
    }

    /// 恢复会话时用量与耗时必须回到卡片上，否则重新打开的历史会话遥测全是 0
    #[test]
    fn test_restore_keeps_assistant_usage_and_durations() -> Result<()> {
        let temp_dir = std::env::temp_dir().join(format!("a_da_test_{}", uuid::Uuid::new_v4()));
        let manager = SessionManager::new(Some(temp_dir.clone()));

        let ws = "E:/code/test_ws";
        let sid = "session_usage_restore";
        manager.create_session(sid, ws, Some("用量恢复测试"), None, None)?;
        manager.append_message(
            sid,
            AgentMessage::Assistant {
                content: "答案".to_string(),
                thinking: Some("推理".to_string()),
                tool_calls: None,
                stop_reason: Some("stop".to_string()),
                error_message: None,
                timestamp: Some(2000),
                usage: Some(serde_json::json!({
                    "promptTokens": 1234,
                    "completionTokens": 56,
                    "totalTokens": 1290,
                    "cachedTokens": 1000,
                })),
                duration_ms: Some(2500),
                turn_duration_ms: Some(4100),
            },
            Some(ws),
        )?;

        let threads = manager.restore_all_threads();
        let thread = threads.iter().find(|t| t.id == sid).expect("会话未恢复");

        let (usage, duration_ms, turn_duration_ms) = thread
            .items
            .iter()
            .find_map(|item| match item {
                Item::Assistant { text, usage, duration_ms, turn_duration_ms, .. } if text == "答案" => {
                    Some((usage.clone(), *duration_ms, *turn_duration_ms))
                }
                _ => None,
            })
            .expect("助手卡片未恢复");

        let usage = usage.expect("usage 不能丢");
        assert_eq!(usage.prompt_tokens, 1234);
        assert_eq!(usage.completion_tokens, 56);
        assert_eq!(usage.cached_tokens, Some(1000));
        assert_eq!(duration_ms, Some(2500));
        assert_eq!(turn_duration_ms, Some(4100));

        let _ = fs::remove_dir_all(temp_dir);
        Ok(())
    }
}
