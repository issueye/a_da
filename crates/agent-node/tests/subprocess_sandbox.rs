//! P4 · SP-70 的验收：**子进程档**真的把插件隔出去了，而且超时会杀掉它。
//!
//! 为什么放在集成测试（`tests/`）里：唯一的证据是**真的启动一个子进程**。
//! `CARGO_BIN_EXE_runner` 这个环境变量只在集成测试里可用，单元测试拿不到
//! 入口路径——而"路径解析不到就静默退回进程内档"恰恰是这一档最不该有的失败形态。
//!
//! 顺带守住 DoD 的那句话：**内存暴涨的插件被 kill，并使宿主活下来**。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_base::ports::{
    InvokeRequest, LoadRequest, ScriptBudget, ScriptCtx, ScriptError, ScriptHost, ScriptSource,
};
use agent_base::testing::NeverCancel;
use agent_node::scripts::SubprocessScriptHost;

fn runner_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_runner"))
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("a_da_sp70_{tag}_{}_{}", std::process::id(), nanos));
    std::fs::create_dir_all(&dir).expect("建临时目录失败");
    dir
}

fn write_plugin(dir: &PathBuf, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("写插件失败");
    path
}

fn ctx<'a>(budget: ScriptBudget, cancel: &'a dyn agent_base::ports::CancelToken) -> ScriptCtx<'a> {
    ScriptCtx {
        budget,
        cancel,
        thread_id: "t1",
        origin_id: Some("plugin:e2e"),
    }
}

/// 正常插件在子进程档下照常工作（工具被调、结果带回）。
///
/// 没有这一条，"隔离生效"与"这一档根本跑不起来"分不开。
#[tokio::test]
async fn test_subprocess_host_runs_a_plugin_tool() {
    let dir = temp_dir("ok");
    let path = write_plugin(
        &dir,
        "echo.ts",
        "export default { tools: [ { name: 'echo_probe', description: '回显', \
         execute: async (args: any) => ({ ok: true, output: 'echo:' + args.text }) } ] }\n",
    );
    let host = SubprocessScriptHost::with_runner(runner_path());
    let ctx = ctx(ScriptBudget::default(), &NeverCancel);

    let handle = host
        .load(
            &LoadRequest {
                source: ScriptSource::Path(path),
                workspace: Some(dir.clone()),
            },
            &ctx,
        )
        .await
        .expect("装载应当成功");
    assert_eq!(host.exports(&handle).expect("入口清单"), vec!["echo_probe".to_string()]);

    let out = host
        .invoke(
            &handle,
            &InvokeRequest {
                export: "echo_probe".into(),
                args: serde_json::json!({ "text": "hi" }),
            },
            &ctx,
        )
        .await
        .expect("调用应当成功");
    assert!(out.ok, "{out:?}");
    assert_eq!(out.output, "echo:hi");

    host.dispose(handle).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// **DoD 的核心那条**：插件把内存吃光 → 宿主把它**杀掉**，宿主自己活着，
/// 而且下一次调用照常能起一个新的子进程。
///
/// 用例设计：插件在 `execute` 里无限往数组里塞大块内存。子进程档的墙钟预算很短，
/// 到点宿主就 `kill` 它并回一条 `TimedOut`（可纠正），**不阻塞整轮**。
#[tokio::test]
async fn test_memory_hog_is_killed_and_the_host_survives() {
    let dir = temp_dir("hog");
    let hog = write_plugin(
        &dir,
        "hog.ts",
        "export default { tools: [ { name: 'hog', description: '吃内存', \
         execute: async () => { const a: any[] = []; \
           while (true) { a.push(new Array(200000).fill('x')); } } } ] }\n",
    );
    let ok = write_plugin(
        &dir,
        "ok.ts",
        "export default { tools: [ { name: 'echo_probe', description: '回显', \
         execute: async () => ({ ok: true, output: 'alive' }) } ] }\n",
    );

    let host = Arc::new(SubprocessScriptHost::with_runner(runner_path()));
    // 预算给 2 秒：够它吃到很多内存，但绝不该让用例等太久
    let budget = ScriptBudget {
        timeout_ms: 2_000,
        // 循环上限放宽，让"是墙钟杀掉的"这件事成立（否则会先撞指令上限）
        loop_iterations: 0,
        ..Default::default()
    };
    let ctx = ctx(budget, &NeverCancel);

    let handle = host
        .load(
            &LoadRequest {
                source: ScriptSource::Path(hog),
                workspace: Some(dir.clone()),
            },
            &ctx,
        )
        .await
        .expect("装载应当成功");

    let started = std::time::Instant::now();
    let res = host
        .invoke(
            &handle,
            &InvokeRequest {
                export: "hog".into(),
                args: serde_json::json!({}),
            },
            &ctx,
        )
        .await;
    let elapsed = started.elapsed();

    let err = res.expect_err("吃内存的插件必须被拦下，而不是拖着宿主");
    assert!(
        matches!(err, ScriptError::TimedOut { .. }),
        "超时即杀，回报应当是 TimedOut（可纠正）：{err:?}"
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "必须在预算附近返回，实际 {elapsed:?}"
    );

    // 宿主还活着，而且能起新的子进程继续干活（隔离的意义就在这里）
    let handle2 = host
        .load(
            &LoadRequest {
                source: ScriptSource::Path(ok),
                workspace: Some(dir.clone()),
            },
            &ctx,
        )
        .await
        .expect("被杀掉一个子进程之后，宿主必须照常装载下一个");
    let out = host
        .invoke(
            &handle2,
            &InvokeRequest {
                export: "echo_probe".into(),
                args: serde_json::json!({}),
            },
            &ctx,
        )
        .await
        .expect("下一次调用必须成功");
    assert_eq!(out.output, "alive");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 能力裁剪在子进程档里同样成立（SP-71 的口径不能因为换档而变）：
/// 没声明 `net` 的插件在子进程里也拿不到 `fetch`。
#[tokio::test]
async fn test_needs_gating_holds_in_the_subprocess_tier() {
    let dir = temp_dir("needs");
    let path = write_plugin(
        &dir,
        "probe.ts",
        "export default { tools: [ { name: 'probe', description: '看有没有网', \
         execute: async () => ({ ok: true, output: typeof fetch }) } ] }\n",
    );
    let host = SubprocessScriptHost::with_runner(runner_path());
    let ctx = ctx(ScriptBudget::default(), &NeverCancel);
    let handle = host
        .load(
            &LoadRequest {
                source: ScriptSource::Path(path),
                workspace: Some(dir.clone()),
            },
            &ctx,
        )
        .await
        .expect("装载应当成功");
    let out = host
        .invoke(
            &handle,
            &InvokeRequest {
                export: "probe".into(),
                args: serde_json::json!({}),
            },
            &ctx,
        )
        .await
        .expect("调用应当成功");
    assert_eq!(
        out.output, "undefined",
        "子进程档下同样'未声明即不注入'（换档不改权限口径）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
