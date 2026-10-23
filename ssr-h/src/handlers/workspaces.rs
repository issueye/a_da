//! 工作区：侧栏树的分组、添加/选择工作区。
//!
//! 与 desktop-ui 的口径保持一致：
//!
//! - **工作区 = 会话属性**：同一个 agent 进程能看到所有工作区的会话，所以侧栏树是
//!   "快照里的会话按 `workspace` 分组"，切换工作区不需要重连、也不换 agent；
//! - **选择工作区 = 设当前项目 + 打开/新建一条该工作区的会话**（desktop-ui 的
//!   `setActiveProject()` + `createThread(ws)` 是同一套动作）；
//! - **添加工作区 = 选目录**：目录里没有会话时选它就是"加进来"，
//!   选完建一条会话，它就会出现在侧栏树里。
//!
//! 目录浏览走网关的 `fs.roots` / `fs.list` / `fs.mkdir`（与桌面端文件选择器同一组端点），
//! 不自己读盘：ssr-h 可能被放到另一台机器上、而网关在宿主上。

use std::collections::BTreeMap;

use axum::extract::{Form, Query, State};
use axum::response::Response;
use serde::Deserialize;
use serde_json::Value;

use super::*;
use crate::templates::{FsEntryRow, FsRootRow, WorkspaceGroup, WorkspaceRow, WorkspacesPage};

#[derive(Debug, Default, Deserialize)]
pub struct WorkspaceQuery {
    path: Option<String>,
    error: Option<String>,
    notice: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SelectWorkspaceForm {
    path: String,
}

#[derive(Debug, Deserialize)]
pub struct MkdirForm {
    parent: String,
    name: String,
}

/// 路径归一化：与 desktop-ui `normalizePathKey` 同一口径
/// （斜杠统一、去尾斜杠、大小写折叠）。
pub(super) fn workspace_path_key(path: &str) -> String {
    path.trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

/// 把侧栏会话按工作区归纳成树。
///
/// 规则（逐条对应 desktop-ui 的 `workspaceGroups`）：
/// - 空 `workspace` 的会话归到当前工作区（再空就归"默认工作区"）；
/// - **当前工作区即使没有任何会话也要出现**（否则用户选完目录看不到它）；
/// - 组内按会话时间倒序；组间"当前工作区优先"，其余按最近会话倒序。
pub(super) fn workspace_groups(
    threads: &[ThreadRow],
    active_workspace: &str,
) -> Vec<WorkspaceGroup> {
    let active_key = workspace_path_key(active_workspace);
    let mut groups: BTreeMap<String, WorkspaceGroup> = BTreeMap::new();

    for thread in threads {
        let key = ensure_group(&mut groups, &thread.workspace, active_workspace);
        if let Some(group) = groups.get_mut(&key) {
            group.threads.push(thread.clone());
        }
    }
    if !active_key.is_empty() || groups.is_empty() {
        ensure_group(&mut groups, active_workspace, active_workspace);
    }

    let mut out: Vec<WorkspaceGroup> = groups
        .into_values()
        .map(|mut group| {
            group.threads.sort_by(|left, right| {
                right
                    .created_at
                    .cmp(&left.created_at)
                    .then_with(|| left.id.cmp(&right.id))
            });
            group.count = group.threads.len();
            group.running = group.threads.iter().any(|thread| thread.running);
            group.active = group.key == active_key;
            group
        })
        .collect();

    out.sort_by(|left, right| {
        right
            .active
            .cmp(&left.active)
            .then_with(|| newest(&right.threads).cmp(&newest(&left.threads)))
            .then_with(|| left.key.cmp(&right.key))
    });
    out
}

/// 取/建一个工作区分组；返回它的规范化键。
fn ensure_group(
    groups: &mut BTreeMap<String, WorkspaceGroup>,
    raw: &str,
    active_workspace: &str,
) -> String {
    let raw = raw.trim();
    let path = if raw.is_empty() {
        if active_workspace.trim().is_empty() {
            "默认工作区".to_string()
        } else {
            active_workspace.trim().to_string()
        }
    } else {
        raw.to_string()
    };
    let key = workspace_path_key(&path);
    groups.entry(key.clone()).or_insert_with(|| WorkspaceGroup {
        key: key.clone(),
        name: workspace_name(&path),
        path,
        threads: Vec::new(),
        count: 0,
        running: false,
        active: false,
    });
    key
}

fn newest(threads: &[ThreadRow]) -> u64 {
    threads
        .iter()
        .map(|thread| thread.created_at)
        .max()
        .unwrap_or(0)
}

/// `GET /workspaces`：添加/选择工作区（目录浏览器）。
pub async fn workspace_picker(
    State(state): State<AppState>,
    Query(query): Query<WorkspaceQuery>,
) -> Result<Response, AppError> {
    let mut flash: Option<Flash> = if let Some(error) = query
        .error
        .as_deref()
        .map(str::trim)
        .filter(|error| !error.is_empty())
    {
        Some(Flash {
            kind: "error".to_string(),
            message: error.to_string(),
        })
    } else {
        query.notice.as_deref().map(|notice| Flash {
            kind: "success".to_string(),
            message: match notice {
                "workspace_mkdir" => "已新建目录".to_string(),
                other => other.to_string(),
            },
        })
    };

    // 最近工作区与当前项目：拿不到快照也不影响浏览目录。
    let snapshot = state.agent.snapshot().await.ok();
    let active_workspace = snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.get("workspace"))
        .and_then(|workspace| workspace.get("project"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|workspace| !workspace.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| state.agent.workspace.clone());

    let recent = snapshot
        .as_ref()
        .map(|snapshot| recent_workspaces(snapshot, &active_workspace))
        .unwrap_or_default();

    let roots = match state.agent.fs_roots().await {
        Ok(roots) => roots.iter().map(fs_root_row).collect::<Vec<_>>(),
        Err(error) => {
            flash = Some(Flash {
                kind: "error".to_string(),
                message: format!("读取根目录失败：{}", public_reason(&error)),
            });
            Vec::new()
        }
    };

    // 初始目录：显式 path → 当前工作区 → 第一个根。
    let requested = query
        .path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .or_else(|| (!active_workspace.trim().is_empty()).then(|| active_workspace.clone()))
        .or_else(|| roots.first().map(|root| root.path.clone()))
        .unwrap_or_default();

    let (current, parent, entries, truncated, omitted) =
        match load_directories(&state, &requested, flash.as_mut()).await {
            Some((current, parent, entries, truncated, omitted)) => {
                (current, parent, entries, truncated, omitted)
            }
            None => (
                requested.clone(),
                parent_of(&requested),
                Vec::new(),
                false,
                0,
            ),
        };

    // 当前浏览目录若还是空串，就让"选择此目录"不可点（模板按 current.is_empty() 判断）。
    let page_base = match snapshot.as_ref() {
        Some(snapshot) => {
            base_with_snapshot(&state, "工作区", "workspaces", flash, Some(snapshot), None).await
        }
        None => base(&state, "工作区", "workspaces", flash).await,
    };
    let page = WorkspacesPage {
        base: page_base,
        roots,
        recent,
        parent_href: format!("/workspaces?path={}", encode_query(&parent)),
        current,
        parent,
        entries,
        truncated,
        omitted,
    };
    Ok(render(&page)?.into_response())
}

/// `POST /workspaces/select`：设当前工作区，并打开（没有则新建）一条会话。
pub async fn select_workspace(
    State(state): State<AppState>,
    Form(form): Form<SelectWorkspaceForm>,
) -> Result<Response, AppError> {
    let path = form.path.trim().to_string();
    if path.is_empty() {
        return Ok(workspace_redirect("", "工作区路径不能为空"));
    }
    // 先用网关的目录列表确认它是真实存在的目录（不要凭 URL 里的字符串建会话）。
    if state.agent.fs_list(&path).await.is_err() {
        return Ok(workspace_redirect(
            &path,
            "无法打开该目录；请确认路径存在且可读",
        ));
    }
    if let Err(error) = state.agent.add_workspace(&path).await {
        return Ok(workspace_redirect(
            &path,
            &format!("切换工作区失败：{}", public_reason(&error)),
        ));
    }

    // 该工作区已有会话 → 打开最近的一条；没有 → 新建。
    let key = workspace_path_key(&path);
    let existing = state.agent.snapshot().await.ok().and_then(|snapshot| {
        array_at(&snapshot, "threads")
            .iter()
            .filter(|thread| thread.get("isSubagent").and_then(Value::as_bool) != Some(true))
            .filter(|thread| workspace_path_key(str_at(thread, "workspace")) == key)
            .max_by_key(|thread| thread.get("createdAt").and_then(Value::as_u64).unwrap_or(0))
            .map(|thread| str_at(thread, "id").to_string())
            .filter(|id| !id.is_empty())
    });

    let thread_id = match existing {
        Some(id) => id,
        None => match state.agent.create_thread_in("新会话", &path).await {
            Ok(result) => match result
                .get("threadId")
                .and_then(Value::as_str)
                .or_else(|| result.get("id").and_then(Value::as_str))
            {
                Some(id) => id.to_string(),
                None => {
                    return Ok(workspace_redirect(
                        &path,
                        "切换工作区失败：网关没有返回新会话 id",
                    ))
                }
            },
            Err(error) => {
                return Ok(workspace_redirect(
                    &path,
                    &format!("在工作区新建会话失败：{}", public_reason(&error)),
                ))
            }
        },
    };
    Ok(redirect(&format!(
        "/threads/{thread_id}?notice=workspace_selected"
    )))
}

/// `POST /workspaces/mkdir`：在浏览位置新建目录，然后进入它。
pub async fn make_workspace_dir(
    State(state): State<AppState>,
    Form(form): Form<MkdirForm>,
) -> Result<Response, AppError> {
    let parent = form.parent.trim().to_string();
    let name = form.name.trim().to_string();
    if parent.is_empty() || name.is_empty() {
        return Ok(workspace_redirect(&parent, "新建目录需要父目录与目录名"));
    }
    if name.contains('/') || name.contains('\\') || name == ".." || name == "." {
        return Ok(workspace_redirect(&parent, "目录名不能包含路径分隔符"));
    }
    let target = format!("{}/{}", parent.trim_end_matches(['/', '\\']), name);
    match state.agent.fs_mkdir(&target).await {
        Ok(_) => Ok(redirect(&format!(
            "/workspaces?path={}&notice=workspace_mkdir",
            encode_query(&target)
        ))),
        Err(error) => Ok(workspace_redirect(
            &parent,
            &format!("新建目录失败：{}", public_reason(&error)),
        )),
    }
}

async fn load_directories(
    state: &AppState,
    path: &str,
    flash: Option<&mut Flash>,
) -> Option<(String, String, Vec<FsEntryRow>, bool, usize)> {
    if path.trim().is_empty() {
        return None;
    }
    let listing = match state.agent.fs_list(path).await {
        Ok(listing) => listing,
        Err(error) => {
            if let Some(flash) = flash {
                *flash = Flash {
                    kind: "error".to_string(),
                    message: format!("无法打开目录：{}", public_reason(&error)),
                };
            }
            return None;
        }
    };
    let current = str_at(&listing, "path").to_string();
    if current.is_empty() {
        return None;
    }
    let parent = listing
        .get("parent")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|parent| !parent.trim().is_empty())
        .unwrap_or_else(|| parent_of(&current));

    let mut entries: Vec<FsEntryRow> = array_at(&listing, "entries")
        .iter()
        .filter(|entry| entry.get("isDir").and_then(Value::as_bool) == Some(true))
        .filter_map(|entry| {
            let path = str_at(entry, "path");
            if path.is_empty() {
                return None;
            }
            Some(FsEntryRow {
                name: str_at(entry, "name").to_string(),
                path: path.to_string(),
                href: format!("/workspaces?path={}", encode_query(path)),
            })
        })
        .collect();
    entries.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });

    let truncated = listing
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let omitted = listing.get("omitted").and_then(Value::as_u64).unwrap_or(0) as usize;
    Some((current, parent, entries, truncated, omitted))
}

fn fs_root_row(value: &Value) -> FsRootRow {
    let path = str_at(value, "path").to_string();
    FsRootRow {
        label: str_at(value, "label").to_string(),
        href: format!("/workspaces?path={}", encode_query(&path)),
        path,
        kind: str_at(value, "kind").to_string(),
    }
}

/// 最近工作区（侧栏树的同源数据，取有会话的分组）。
fn recent_workspaces(snapshot: &Value, active_workspace: &str) -> Vec<WorkspaceRow> {
    // 用侧栏同一套过滤：空占位会话不该把"会话数"撑大。
    let threads = super::chat::thread_rows(snapshot, "");
    workspace_groups(&threads, active_workspace)
        .into_iter()
        .filter(|group| group.count > 0)
        .take(12)
        .map(|group| WorkspaceRow {
            path: group.path,
            name: group.name,
            sessions: group.count,
            active: group.active,
        })
        .collect()
}

/// 失败的目录操作回到选择器：带上路径与原因，不把整页换成裸错误页。
fn workspace_redirect(path: &str, message: &str) -> Response {
    let mut target = String::from("/workspaces");
    if !path.trim().is_empty() {
        target.push_str("?path=");
        target.push_str(&encode_query(path));
        target.push_str("&error=");
    } else {
        target.push_str("?error=");
    }
    target.push_str(&encode_query(message));
    redirect(&target)
}

fn parent_of(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    match trimmed.rfind(['/', '\\']) {
        Some(cut) if cut > 0 => {
            let parent = &trimmed[..cut];
            // Windows 盘符根（`C:`）要保留成 `C:/`
            if parent.len() == 2 && parent.ends_with(':') {
                format!("{parent}/")
            } else {
                parent.to_string()
            }
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::ThreadRow;

    fn thread(id: &str, workspace: &str, at: u64) -> ThreadRow {
        ThreadRow {
            id: id.to_string(),
            title: id.to_string(),
            workspace: workspace.to_string(),
            mode: "code".to_string(),
            running: false,
            item_count: 0,
            created_at: at,
        }
    }

    #[test]
    fn groups_fold_slashes_and_case_like_desktop() {
        assert_eq!(
            workspace_path_key("E:\\Code\\Project\\"),
            workspace_path_key("e:/code/project")
        );
        assert_eq!(workspace_path_key(""), "");
    }

    #[test]
    fn groups_by_workspace_and_orders_active_first() {
        let groups = workspace_groups(
            &[
                thread("a", "E:/one", 10),
                thread("b", "E:/two", 30),
                thread("c", "e:\\ONE", 20),
            ],
            "E:/two",
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].path, "E:/two");
        assert!(groups[0].active);
        assert_eq!(groups[0].threads[0].id, "b");
        let one = groups.iter().find(|g| g.key == "e:/one").unwrap();
        assert_eq!(one.count, 2);
        assert_eq!(
            one.threads
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "a"],
            "组内按创建时间倒序"
        );
    }

    #[test]
    fn active_workspace_without_threads_still_appears() {
        let groups = workspace_groups(&[], "E:/fresh");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].path, "E:/fresh");
        assert!(groups[0].active);
        assert_eq!(groups[0].count, 0);
    }

    #[test]
    fn empty_workspace_threads_fall_back_to_active_workspace() {
        let groups = workspace_groups(&[thread("a", "", 1)], "E:/active");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].path, "E:/active");
        assert_eq!(groups[0].count, 1);
    }

    #[test]
    fn parent_of_handles_drive_roots_and_plain_paths() {
        assert_eq!(parent_of("E:/code/app"), "E:/code");
        assert_eq!(parent_of("E:/code/"), "E:/");
        assert_eq!(parent_of("E:/"), "");
        assert_eq!(parent_of("E:"), "");
        assert_eq!(parent_of("/home/user"), "/home");
    }
}
