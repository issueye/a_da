//! `ScriptHost` 端口契约（P1 · SP-19）。
//!
//! 与其它端口契约同一口径：**能通用断言的部分**才写在这里——"`load`/`invoke`/`dispose`
//! 三件套的语义、预算与取消必须生效、释放必须幂等"，而不是某个具体实现的细节
//! （例如"输出上限怎么截断"由实现自己的用例管）。
//!
//! 探针是这里**自己造**的，不依赖调用方提供夹具：契约要能被任何一个实现跑起来，
//! 就像 `event_probe_stream` 那样。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agent_base::ports::{
    InvokeRequest, LoadRequest, ModuleHandle, ScriptBudget, ScriptCtx, ScriptError, ScriptHost,
    ScriptSource,
};
use agent_base::testing::{ManualCancel, NeverCancel};

/// 契约用的脚本探针（临时目录里的一个插件文件 + 两件工具）。
pub struct ScriptProbe {
    pub dir: PathBuf,
    pub path: PathBuf,
}

impl Drop for ScriptProbe {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 探针里的正常入口名。
pub const PROBE_ECHO_EXPORT: &str = "probe_echo";
/// 探针里的**同步死循环**入口名。
pub const PROBE_SPIN_EXPORT: &str = "probe_spin";

/// 造一个探针插件（写完即可用；`Drop` 时清理临时目录）。
pub fn write_script_probe() -> Result<ScriptProbe, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("a_da_script_probe_{}_{}", std::process::id(), nanos));
    std::fs::create_dir_all(&dir).map_err(|e| format!("建临时目录失败: {e}"))?;
    let path = dir.join("probe_plugin.ts");
    // 刻意让**第一个属性是 tools**：这是行级转译器曾经踩雷的排版（见 ts-engine 的回归用例），
    // 契约探针也顺带守住它。
    std::fs::write(
        &path,
        format!(
            "export default {{ tools: [\
             {{ name: '{PROBE_ECHO_EXPORT}', description: '回显', \
             execute: async (args: any) => {{ return {{ ok: true, output: 'probe:' + args.text }} }} }},\
             {{ name: '{PROBE_SPIN_EXPORT}', description: '同步死循环', \
             execute: async (_a: any) => {{ while (true) {{}} }} }}\
             ] }}\n"
        ),
    )
    .map_err(|e| format!("写探针失败: {e}"))?;
    Ok(ScriptProbe { dir, path })
}

fn load_req(probe: &ScriptProbe) -> LoadRequest {
    LoadRequest {
        source: ScriptSource::Path(probe.path.clone()),
        workspace: Some(probe.dir.clone()),
    }
}

/// 验证 `ScriptHost` 端口契约。
///
/// 断言清单（每条都对应"实现写错了会静默坏掉"的地方）：
/// 1. **往返**：`load` → `invoke` 正常入口拿到结果；
/// 2. **未知入口**：`invoke` 一个不存在的入口 → `UnknownExport`（**不该等超时、也不该报脚本错**）；
/// 3. **预算**：同步死循环 → `BudgetExceeded` / `TimedOut`，且**秒级返回**（不是挂住）；
/// 4. **取消**：已取消的 token → `Aborted`，且**不执行脚本**；
/// 5. **释放幂等**：`dispose` 两次不报错；释放后再调用 → `Unavailable`。
pub async fn verify_script_host_contract(
    host: &dyn ScriptHost,
    probe: &ScriptProbe,
) -> Result<(), String> {

    // ── 1. 往返 ─────────────────────────────────────────────────────────────
    let ctx = ScriptCtx {
        budget: ScriptBudget::default(),
        cancel: &NeverCancel,
        thread_id: "contract",
        origin_id: Some("conformance"),
    };
    let handle = host
        .load(&load_req(probe), &ctx)
        .await
        .map_err(|e| format!("`load` 应当成功，实际 {e:?}"))?;

    let ok = host
        .invoke(
            &handle,
            &InvokeRequest {
                export: PROBE_ECHO_EXPORT.to_string(),
                args: serde_json::json!({ "text": "hi" }),
            },
            &ctx,
        )
        .await
        .map_err(|e| format!("正常入口的 `invoke` 应当成功，实际 {e:?}"))?;
    if !ok.ok || !ok.output.contains("probe:hi") {
        return Err(format!(
            "正常入口的结果不对：ok={} output={:?}",
            ok.ok, ok.output
        ));
    }

    // ── 2. 未知入口 ─────────────────────────────────────────────────────────
    match host
        .invoke(
            &handle,
            &InvokeRequest {
                export: "no_such_export".to_string(),
                args: serde_json::json!({}),
            },
            &ctx,
        )
        .await
    {
        Err(ScriptError::UnknownExport { export }) => {
            if export != "no_such_export" {
                return Err(format!("`UnknownExport` 里的入口名不对：{export}"));
            }
        }
        other => {
            return Err(format!(
                "不存在的入口必须报 `UnknownExport`（它既不是脚本错、也不该等超时），实际 {other:?}"
            ))
        }
    }

    // ── 3. 预算：同步死循环必须秒级返回 ──────────────────────────────────────
    let spin_budget = ScriptBudget {
        // 墙钟给宽，好让"是限额打断的"与"是墙钟超时兜住的"分得出来
        timeout_ms: 60_000,
        loop_iterations: 100_000,
        ..ScriptBudget::default()
    };
    let spin_ctx = ScriptCtx {
        budget: spin_budget,
        cancel: &NeverCancel,
        thread_id: "contract",
        origin_id: Some("conformance"),
    };
    let started = Instant::now();
    let spin = host
        .invoke(
            &handle,
            &InvokeRequest {
                export: PROBE_SPIN_EXPORT.to_string(),
                args: serde_json::json!({}),
            },
            &spin_ctx,
        )
        .await;
    let elapsed = started.elapsed();
    match spin {
        Err(ScriptError::BudgetExceeded { .. }) | Err(ScriptError::TimedOut { .. }) => {}
        other => {
            return Err(format!(
                "同步死循环必须报 `BudgetExceeded` 或 `TimedOut`，实际 {other:?}"
            ))
        }
    }
    if elapsed > Duration::from_secs(20) {
        return Err(format!(
            "死循环必须在秒级返回（20s 内），实际 {elapsed:?}——这就是「挂住」的形态"
        ));
    }

    // ── 4. 取消 ─────────────────────────────────────────────────────────────
    let cancel = ManualCancel::new();
    cancel.cancel();
    let cancelled_ctx = ScriptCtx {
        budget: ScriptBudget::default(),
        cancel: &cancel,
        thread_id: "contract",
        origin_id: Some("conformance"),
    };
    match host
        .invoke(
            &handle,
            &InvokeRequest {
                export: PROBE_ECHO_EXPORT.to_string(),
                args: serde_json::json!({ "text": "x" }),
            },
            &cancelled_ctx,
        )
        .await
    {
        Err(ScriptError::Aborted) => {}
        other => return Err(format!("已取消的调用必须报 `Aborted`，实际 {other:?}")),
    }

    // ── 5. 释放幂等 ─────────────────────────────────────────────────────────
    host.dispose(handle.clone()).await;
    host.dispose(handle.clone()).await;
    match host
        .invoke(
            &handle,
            &InvokeRequest {
                export: PROBE_ECHO_EXPORT.to_string(),
                args: serde_json::json!({}),
            },
            &ctx,
        )
        .await
    {
        Err(ScriptError::Unavailable { .. }) => {}
        other => {
            return Err(format!(
                "释放之后再调用必须报 `Unavailable`（不能悄悄成功），实际 {other:?}"
            ))
        }
    }

    Ok(())
}

/// 端口的类型层契约：句柄必须可比较、可克隆（释放要能被调用两次）。
#[allow(dead_code)]
fn _type_level_contract(handle: &ModuleHandle) {
    let _: ModuleHandle = handle.clone();
    let _ = handle == handle;
    let _: &Path = Path::new(".");
}
