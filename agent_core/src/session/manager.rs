use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};


use super::slug::{safe_id, workspace_slug};
use super::types::{
    AgentMessage, CURRENT_SESSION_VERSION, SessionCompactEntry, SessionEntry, SessionHeader,
    SessionMessageEntry, SessionSummary,
};
use crate::protocol::{AgentMode, Item, Thread};

/// 获取默认的 a-da 配置和数据主目录
pub fn get_app_home() -> PathBuf {
    if let Ok(dir) = std::env::var("A_DA_HOME") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir.trim());
        }
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        return PathBuf::from(userprofile).join(".a-da");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".a-da");
    }
    PathBuf::from(".a-da")
}

/// 获取 a-da 配置文件路径，优先遵循 A_DA_CONFIG 环境变量
pub fn get_config_path() -> PathBuf {
    if let Ok(cfg) = std::env::var("A_DA_CONFIG") {
        if !cfg.trim().is_empty() {
            return PathBuf::from(cfg.trim());
        }
    }
    get_app_home().join("config.json")
}

/// 会话管理器
#[derive(Debug, Clone)]
pub struct SessionManager {
    explicit_dir: Option<PathBuf>,
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

        let now = now_ms();
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
            plugin_data: None,
        };

        let json_line = serde_json::to_string(&header)?;
        let mut file = File::create(&file_path)?;
        writeln!(file, "{}", json_line)?;

        Ok(header)
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

    /// 向会话以 Append-only 方式追加一条消息
    pub fn append_message(
        &self,
        session_id: &str,
        message: AgentMessage,
        workspace: Option<&str>,
    ) -> Result<()> {
        let file_path = if let Some(ws) = workspace {
            let dir = self.workspace_dir(ws);
            if !dir.exists() {
                fs::create_dir_all(&dir)?;
            }
            self.get_session_path(ws, session_id)
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(());
        };

        let now = now_ms();
        let random_suffix = uuid::Uuid::new_v4().to_string();
        let entry = SessionMessageEntry {
            entry_type: "message".to_string(),
            id: format!("msg_{}_{}", now, &random_suffix[..6]),
            timestamp: now,
            message,
        };

        let line = serde_json::to_string(&entry)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)?;
        writeln!(file, "{}", line)?;
        Ok(())
    }

    /// 向会话追加一条上下文压缩摘要记录
    pub fn append_compact_entry(
        &self,
        session_id: &str,
        mut compact_entry: SessionCompactEntry,
        workspace: Option<&str>,
    ) -> Result<()> {
        let file_path = if let Some(ws) = workspace {
            let dir = self.workspace_dir(ws);
            if !dir.exists() {
                fs::create_dir_all(&dir)?;
            }
            self.get_session_path(ws, session_id)
        } else if let Some(path) = self.find_session_path(session_id) {
            path
        } else {
            return Ok(());
        };

        compact_entry.entry_type = "compact".to_string();
        let line = serde_json::to_string(&compact_entry)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)?;
        writeln!(file, "{}", line)?;
        Ok(())
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
            header.updated_at = now_ms();
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
            header.updated_at = now_ms();
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

        let file = File::open(&file_path)?;
        let reader = BufReader::new(file);

        let mut header: Option<SessionHeader> = None;
        let mut messages: Vec<AgentMessage> = Vec::new();

        for line_res in reader.lines() {
            let line = match line_res {
                Ok(l) => l,
                Err(_) => continue,
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Ok(entry) = serde_json::from_str::<SessionEntry>(trimmed) {
                match entry {
                    SessionEntry::Header(h) => {
                        header = Some(h);
                    }
                    SessionEntry::Message(m) => {
                        messages.push(m.message);
                    }
                    SessionEntry::Notice(_) => {
                        // Notice 不计入 messages 模型上下文
                    }
                    SessionEntry::Compact(c) => {
                        // 遇到 compact 记录，折叠前面的上下文，生成连续总结消息
                        let continuation_msg = AgentMessage::User {
                            content: format!("[系统自动压缩摘要]\n{}", c.summary),
                            images: None,
                            timestamp: Some(c.timestamp),
                        };
                        messages.clear();
                        messages.push(continuation_msg);
                    }
                }
            }
        }

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

            for (index, entry) in raw_entries.into_iter().enumerate() {
                match entry {
                    SessionEntry::Header(_) => {}
                    SessionEntry::Notice(_) => {}
                    SessionEntry::Compact(c) => {
                        let _pruned = std::mem::take(&mut items);
                        items.push(Item::Compact {
                            id: c.id.clone(),
                            at: c.timestamp as u64,
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
                                    at,
                                    text: content,
                                    images,
                                    queued: None,
                                });
                            }
                            AgentMessage::Assistant { content, thinking, tool_calls, usage, duration_ms, turn_duration_ms, stop_reason, error_message, .. } => {
                                // 用量与耗时是遥测条与单条回复徽章的数据源，恢复时原样带回卡片
                                let restored_usage = usage
                                    .as_ref()
                                    .and_then(|v| serde_json::from_value::<crate::ai::TokenUsage>(v.clone()).ok());
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
                                        items.push(Item::Thinking {
                                            id: format!("{}_restored_think_{}", header.id, index),
                                            at,
                                            text: th,
                                            ended_at: Some(at),
                                        });
                                    }
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

                                        if let Some(AgentMessage::ToolResult { content: res_content, is_error: res_err, details: res_det, patch: res_patch, checkpoint_id: res_cp, timestamp: res_ts, .. }) = res {
                                            if res_content == "用户拒绝了此工具调用" || res_content == "用户拒绝了这次调用。不要重试同样的调用，先说明原因或换一种做法。" {
                                                is_denied = true;
                                            }
                                            is_error = res_err.unwrap_or(false);
                                            output = Some(res_content.clone());
                                            patch = res_patch.clone();
                                            checkpoint_id = res_cp.clone();
                                            details = res_det.clone();
                                            if let Some(ts) = res_ts { tool_at = *ts as u64; }
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
                                            at: tool_at,
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
                                        });
                                    }
                                }

                                if !content.trim().is_empty() {
                                    items.push(Item::Assistant {
                                        id: format!("{}_restored_asst_{}", header.id, index),
                                        at,
                                        text: content,
                                        streaming: Some(false),
                                        duration_ms,
                                        turn_duration_ms,
                                        usage: restored_usage,
                                    });
                                }
                            }
                            AgentMessage::ToolResult { tool_call_id, tool_name, content, is_error, details, patch, checkpoint_id, timestamp: _ } => {
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
                                        at,
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
                                    });
                                }
                            }
                            AgentMessage::Unknown => {}
                        }
                    }
                }
            }

            let is_sub = header.parent_id.is_some() || header.subagent_id.is_some();
            let thread = Thread {
                id: header.id,
                title: header.title.unwrap_or_else(|| "新会话".to_string()),
                created_at: header.created_at as u64,
                workspace: final_workspace,
                items,
                messages,
                mode: Some(AgentMode::Code),
                parent_id: header.parent_id,
                subagent_id: header.subagent_id,
                is_subagent: Some(is_sub),
                plugin_data: header.plugin_data,
            };

            threads_with_mtime.push((thread, file_mtime));
        }

        threads_with_mtime.sort_by(|a, b| b.1.cmp(&a.1));
        let mut threads: Vec<Thread> = threads_with_mtime.into_iter().map(|(t, _)| t).collect();

        // 纠偏与自愈：如果子会话缺少 parent_id，从主会话的 invoke_subagent 卡片中寻找
        let mut sub_to_parent: HashMap<String, String> = HashMap::new();
        for t in &threads {
            if t.is_subagent != Some(true) {
                for it in &t.items {
                    if let Item::Tool { name, details, output, .. } = it {
                        if name == "invoke_subagent" {
                            if let Some(sub_id) = details.as_ref().and_then(|d| d.get("subagent_thread_id")).and_then(|v| v.as_str()) {
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

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(thread.items.len(), 4, "应生成 User, Thinking, Tool, Assistant 共 4 个卡片");
        match &thread.items[0] {
            Item::User { text, .. } => assert_eq!(text, "请帮我运行 git status"),
            other => panic!("第一张卡片应为 User 卡片，实际为 {:?}", other),
        }
        match &thread.items[1] {
            Item::Thinking { text, .. } => assert!(text.contains("用户需要了解")),
            other => panic!("第二张卡片应为 Thinking 卡片，实际为 {:?}", other),
        }
        match &thread.items[2] {
            Item::Tool { call_id: cid, name, status, output, .. } => {
                assert_eq!(cid, call_id);
                assert_eq!(name, "run_command");
                assert_eq!(status, "done");
                assert_eq!(output.as_deref(), Some("On branch main\nnothing to commit"));
            }
            other => panic!("第三张卡片应为 Tool 卡片，实际为 {:?}", other),
        }
        match &thread.items[3] {
            Item::Assistant { text, .. } => assert_eq!(text, "我将检查当前 git 状态。"),
            other => panic!("第四张卡片应为 Assistant 卡片，实际为 {:?}", other),
        }

        // 5. 清理
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
