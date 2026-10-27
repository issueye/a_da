use super::*;

/// P4·SP-73：运行时错误的栈里要能看见**文件名与行号**。
///
/// 不带源码名时 Boa 报的是 `(unknown at :4:2)`——文件是 "unknown"、行号也没法跟编辑器
/// 对上，插件作者只能靠猜。这条用例断言"名字真的传下去了"。
#[tokio::test]
async fn test_runtime_error_carries_the_source_name_and_line() {
    let runtime = PureTsRuntime::with_workspace(None);
    let err = runtime
        .eval_ts(
            "function boom() {\n  return missingThing();\n}\nboom();\n",
            Some("plugins/probe.ts"),
        )
        .await
        .expect_err("引用未定义的名字必须报错");
    assert!(
        err.contains("plugins/probe.ts"),
        "错误文案里必须出现源码名（否则作者不知道是哪个文件）：{err}"
    );
    assert!(
        !err.contains("unknown at"),
        "不该再出现 `unknown at`（那等于没有位置信息）：{err}"
    );
    // 位置由 Boa 给出（调用点 / 定义点各一条），只断言"是文件里的某一行"
    let has_line = (1..=4).any(|n| err.contains(&format!("plugins/probe.ts:{n}:")));
    assert!(has_line, "要点回源码里的某一行：{err}");
}

/// SP-12 的核心判据：**同步死循环必须必然返回**。
///
/// 为什么这条用例是 P1 的关键：`terminate` 只是往事件循环发消息，同步段期间它排不上队，
/// 所以"取消/超时"在死循环面前完全无效——只有进程内的指令上限能让同步段回来。
#[tokio::test]
async fn test_loop_limit_turns_sync_infinite_loop_into_an_error() {
    let runtime = PureTsRuntime::with_workspace_and_limits(None, Limits::tight(100_000));
    let started = std::time::Instant::now();
    let res = runtime.eval_ts("while (true) {}", None).await;
    let elapsed = started.elapsed();

    let err = res.expect_err("死循环必须被限额打断，而不是永远跑不完");
    assert!(
        is_limit_error(&err),
        "错误必须带限额标记（宿主靠它映射成 Timeout）：{err}"
    );
    assert!(
        err.contains("loop_iterations"),
        "要能看出撞的是哪一项限额：{err}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "必须在很短时间内返回，实际 {elapsed:?}"
    );
    runtime.terminate();
}

/// 递归上限同样生效（证明限额是**三项**都接上了，不是只接了循环那一项）。
#[tokio::test]
async fn test_recursion_limit_is_reported() {
    let runtime = PureTsRuntime::with_workspace(None);
    let res = runtime
        .eval_ts("function f(){ return f() + 1 } f()", None)
        .await;
    let err = res.expect_err("无限递归必须被递归上限打断");
    assert!(is_limit_error(&err), "递归超限也要带限额标记：{err}");
    assert!(err.contains("recursion_depth"), "{err}");
    runtime.terminate();
}

/// SP-13：**原生桥也必须按声明裁剪**。
///
/// 🔴 这条守的是一个真实的绕过路径：异步原生是经 `register_global_callable` 挂到
/// `globalThis` 上的，只关 JS 侧的 `fetch` 包装脚本挡不住
/// `globalThis.__native_fetch_async(...)` 这种**直呼**。
#[tokio::test]
async fn test_net_needs_gates_the_native_bridge_too() {
    // 默认（插件口径）：网络关 → 包装不存在，原生桥也不存在
    let closed = PureTsRuntime::with_environment(
        None,
        Limits::default(),
        crate::env::Needs::plugin_default(),
    );
    for expr in ["typeof fetch", "typeof globalThis.__native_fetch_async"] {
        let got = closed.eval_ts(expr, None).await.expect("eval 应当成功");
        assert_eq!(got, "undefined", "`{expr}` 在默认档下必须不存在");
    }
    closed.terminate();

    // 全开：两者都在
    let open =
        PureTsRuntime::with_environment(None, Limits::default(), crate::env::Needs::everything());
    for expr in ["typeof fetch", "typeof globalThis.__native_fetch_async"] {
        let got = open.eval_ts(expr, None).await.expect("eval 应当成功");
        assert_eq!(got, "function", "`{expr}` 在声明了网络之后必须存在");
    }
    open.terminate();
}

/// SP-14：控制台输出必须**被结构化捕获**，且超限时**如实标注截断**。
///
/// 判据里同时断言"原始字节数 > 文本长度"——那正是"截了多少"的依据；
/// 只看文本的话，"只有两行"与"截得只剩两行"分不出来。
#[tokio::test]
async fn test_console_output_is_captured_and_truncation_is_marked() {
    let runtime = PureTsRuntime::with_environment_and_console(
        None,
        Limits::default(),
        crate::env::Needs::plugin_default(),
        ConsoleMode::Capture,
    );

    runtime
        .eval_ts("console.log('hello'); console.error('oops');", None)
        .await
        .expect("eval 应当成功");

    let snap = runtime.take_console();
    assert!(snap.stdout.contains("hello"), "stdout 必须被捕获：{snap:?}");
    assert!(snap.stderr.contains("oops"), "stderr 必须被捕获：{snap:?}");
    assert!(!snap.truncated);

    // 取走即清空（按调用归属）
    let again = runtime.take_console();
    assert!(again.is_empty(), "取走之后必须清空：{again:?}");

    // 刷屏：文本被截，但**字节数继续如实累计**
    let big = format!("for (let i = 0; i < 5000; i++) console.log('x'.repeat(40));");
    runtime.eval_ts(big, None).await.expect("eval 应当成功");
    let flooded = runtime.take_console();
    assert!(flooded.truncated, "超限必须标记截断");
    assert!(
        (flooded.stdout.len() as u64) < flooded.stdout_bytes,
        "文本必须短于原始字节数（截断的依据）：text={} bytes={}",
        flooded.stdout.len(),
        flooded.stdout_bytes
    );
    assert!(
        flooded.stdout_bytes > CONSOLE_CAPTURE_LIMIT as u64,
        "原始字节数要如实超过上限：{}",
        flooded.stdout_bytes
    );

    runtime.terminate();
}

/// 默认限额**必须有限**：`while(true){}` 的危险不该靠"记得传限额"来避免。
#[test]
fn test_default_limits_are_finite_but_wide() {
    let d = Limits::default();
    assert!(d.loop_iterations > 0, "默认不限循环 = 死循环会占死线程");
    assert!(
        d.loop_iterations >= 1_000_000,
        "默认要给宽值，不能在正常脚本上误伤"
    );
    assert!(d.recursion_depth > 0 && d.stack_size > 0);

    // `tight` 只收循环那一项，另外两项沿用默认（避免测试里连带改语义）
    let t = Limits::tight(123);
    assert_eq!(t.loop_iterations, 123);
    assert_eq!(t.recursion_depth, d.recursion_depth);
    assert_eq!(t.stack_size, d.stack_size);

    // 0 = 不限（显式选择才生效）
    assert_eq!(Limits::unlimited().loop_iterations, 0);
}

#[tokio::test]
async fn test_pure_ts_runtime_eval_sync() {
    let runtime = PureTsRuntime::new();
    let ts_code = r#"
            interface MathOp {
                a: number;
                b: number;
            }
            const op: MathOp = { a: 15, b: 27 };
            const sum = (x: number, y: number): number => x + y;
            `Result: ${sum(op.a, op.b)}`;
        "#;

    let result = runtime
        .eval_ts(ts_code, Some("calc.ts"))
        .await
        .expect("执行失败");
    assert_eq!(result, "Result: 42");
}

#[tokio::test]
async fn test_pure_ts_runtime_async_await_syntax() {
    let runtime = PureTsRuntime::new();
    let ts_code = r#"
            async function compute(): Promise<number> {
                return 100 + 200;
            }
            async function run(): Promise<string> {
                const val = await compute();
                return `Computed: ${val}`;
            }
            run();
        "#;

    let result = runtime
        .eval_ts(ts_code, Some("async.ts"))
        .await
        .expect("执行失败");
    assert!(result.contains("Promise") || result.contains("Computed"));
}

#[tokio::test]
async fn test_pure_ts_runtime_tokio_bridge() {
    let runtime = PureTsRuntime::new();

    // 注册一个 Tokio 驱动的原生异步函数
    runtime.register_async_fn("nativeSleepEcho", |args| async move {
        let msg = args.get(0).cloned().unwrap_or_default();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        Ok(format!("ECHO_{msg}"))
    });

    let ts_code = r#"
            // @ts-ignore
            globalThis.bridgeOutput = "pending";
            async function testBridge(): Promise<void> {
                // @ts-ignore
                const reply = await nativeSleepEcho("Antigravity");
                // @ts-ignore
                globalThis.bridgeOutput = `Got: ${reply}`;
            }
            testBridge();
        "#;

    let result = runtime
        .eval_ts(ts_code, Some("bridge.ts"))
        .await
        .expect("执行失败");
    assert!(result.contains("Promise") || result.contains("undefined"));

    // 稍作等待，让 Tokio 异步任务完成并触发 resolve 与 microtask 执行
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    let check_res = runtime
        .eval_ts("globalThis.bridgeOutput;", None)
        .await
        .expect("查询状态失败");
    assert_eq!(check_res, "Got: ECHO_Antigravity");
}

#[tokio::test]
async fn test_mvp_e2e_ts_tool_execution() {
    let temp_dir = std::env::temp_dir().join(format!("a_da_mvp_e2e_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let target_file = temp_dir.join("hello.txt");
    std::fs::write(&target_file, "Antigravity Pure Rust TS Runtime MVP!").unwrap();

    let runtime = PureTsRuntime::new();

    // 1. 注册沙箱底层异步读取文件的原生函数
    let base_dir = temp_dir.clone();
    runtime.register_async_fn("__native_read_file", move |args| {
        let rel_path = args.get(0).cloned().unwrap_or_default();
        let abs_path = base_dir.join(rel_path);
        async move {
            match tokio::fs::read_to_string(&abs_path).await {
                Ok(text) => Ok(text),
                Err(e) => Err(format!("读取文件失败: {e}")),
            }
        }
    });

    // 2. 注入使用真实 TypeScript 语法编写的插件工具代码
    let ts_tool_code = r#"
            interface ReadFileArgs {
                path: string;
            }

            interface ReadFileResult {
                file: string;
                content: string;
                bytes: number;
            }

            // @ts-ignore
            globalThis.ts_read_file = async function(args: ReadFileArgs): Promise<string> {
                // @ts-ignore
                const raw: string = await __native_read_file(args.path);
                const result: ReadFileResult = {
                    file: args.path,
                    content: raw,
                    bytes: raw.length,
                };
                return JSON.stringify(result);
            };
        "#;

    runtime
        .eval_ts(ts_tool_code, Some("read_file.ts"))
        .await
        .expect("TS 工具加载失败");

    // 3. 模拟 Agent 模型发出的 ToolCall 派发执行
    let tool_args = serde_json::json!({ "path": "hello.txt" }).to_string();
    let tool_result_str = runtime
        .call_async_fn("ts_read_file", tool_args)
        .await
        .expect("TS 工具执行失败");

    // 4. 验证执行结果
    let parsed_result: serde_json::Value =
        serde_json::from_str(&tool_result_str).expect("工具输出必须为合法 JSON");
    assert_eq!(parsed_result["file"], "hello.txt");
    assert_eq!(
        parsed_result["content"],
        "Antigravity Pure Rust TS Runtime MVP!"
    );
    assert_eq!(parsed_result["bytes"], 37);

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn test_pure_ts_runtime_p0_apis() {
    let ws = std::path::PathBuf::from("E:/codes/mock_project");
    let runtime = PureTsRuntime::with_workspace(Some(ws));

    let ts_code = r#"
            // 验证 process 全局对象
            const cwd = process.cwd();
            const isWin = process.platform === "win32";

            // 验证 path 模块
            const fullPath = path.join(cwd, "src", "index.ts");
            const ext = path.extname(fullPath);

            // 验证 Buffer
            const buf = Buffer.from("Antigravity Pure Rust", "utf-8");
            const bufStr = buf.toString("utf-8");

            // 验证 EventEmitter
            const ee = new EventEmitter();
            let triggered = false;
            ee.on("event", (val: boolean) => {
                triggered = val;
            });
            ee.emit("event", true);

            // 验证 require 虚拟模块
            const pathReq = require("node:path");
            const sameExt = pathReq.extname("sample.tsx");

            // 验证 node:crypto
            const cryptoReq = require("node:crypto");
            const cryptoHash = cryptoReq.createHash("sha256").update("PureTsRuntime").digest("hex");

            JSON.stringify({
                cwd,
                isWin,
                ext,
                bufStr,
                triggered,
                sameExt,
                cryptoHash
            });
        "#;

    let result = runtime
        .eval_ts(ts_code, Some("p0_test.ts"))
        .await
        .expect("执行 P0 测试失败");
    let parsed: serde_json::Value = serde_json::from_str(&result).expect("结果应为合法 JSON");

    assert!(parsed["cwd"].as_str().unwrap().contains("mock_project"));
    assert_eq!(parsed["ext"], ".ts");
    assert_eq!(parsed["bufStr"], "Antigravity Pure Rust");
    assert_eq!(parsed["triggered"], true);
    assert_eq!(parsed["sameExt"], ".tsx");
    assert_eq!(
        parsed["cryptoHash"],
        "d9cd7b8c78b9481c8c22505db00a81514b5e772258069babbb0a9b9792c98510"
    );
}

#[tokio::test]
async fn test_pure_ts_runtime_async_fs_and_sandbox() {
    let temp_dir = std::env::temp_dir().join(format!("a_da_async_fs_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let runtime = PureTsRuntime::with_workspace(Some(temp_dir.clone()));

    let ts_code = r##"
            // @ts-ignore
            globalThis.asyncFsTest = async function(): Promise<string> {
                const fsPromises = require("node:fs/promises");

                // 1. 异步写入文件
                await fsPromises.writeFile("async_note.md", "Hello Tokio Async FS");

                // 2. 异步读取文件
                const readBack = await fsPromises.readFile("async_note.md", "utf-8");

                // 3. 异步沙箱越界拦截验证
                let sandboxBlocked = false;
                try {
                    await fsPromises.readFile("../../../../../etc_shadow.txt", "utf-8");
                } catch (err) {
                    sandboxBlocked = true;
                }

                return JSON.stringify({
                    readBack,
                    sandboxBlocked
                });
            };
        "##;

    runtime
        .eval_ts(ts_code, Some("async_fs.ts"))
        .await
        .expect("加载 async fs 测试脚本失败");

    let res_json_str = runtime
        .call_async_fn("asyncFsTest", "{}")
        .await
        .expect("执行 asyncFsTest 失败");
    let parsed: serde_json::Value =
        serde_json::from_str(&res_json_str).expect("返回值必须为合法 JSON");

    assert_eq!(parsed["readBack"], "Hello Tokio Async FS");
    assert_eq!(parsed["sandboxBlocked"], true);

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn test_pure_ts_runtime_global_fetch() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    // 启动后台本地 Mock HTTP 服务器
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let body = r#"{"message":"Hello from Mock Server!","code":200}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            let _ = socket.flush().await;
        }
    });

    let runtime = PureTsRuntime::new();
    let url = format!("http://127.0.0.1:{port}/api/greet");

    let ts_code = format!(
        r#"
            // @ts-ignore
            globalThis.fetchTest = async function(): Promise<string> {{
                const res = await fetch("{url}");
                const status = res.status;
                const ok = res.ok;
                const text = await res.text();
                const json = JSON.parse(text);
                return JSON.stringify({{
                    status,
                    ok,
                    text,
                    code: json.code
                }});
            }};
        "#
    );

    runtime
        .eval_ts(ts_code, Some("fetch_test.ts"))
        .await
        .expect("加载 fetch 测试脚本失败");

    let res_json_str = runtime
        .call_async_fn("fetchTest", "{}")
        .await
        .expect("调用 fetchTest 失败");
    let parsed: serde_json::Value = serde_json::from_str(&res_json_str).expect("解析 JSON 失败");

    assert_eq!(parsed["status"], 200);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["code"], 200);
    assert!(
        parsed["text"]
            .as_str()
            .unwrap()
            .contains("Hello from Mock Server!")
    );
}

/// 代理**真的生效**：目标端口必然拒绝连接，只有经过代理才可能拿到响应。
///
/// 顺带钉住两件事：
/// - `proxy` 选项**不带 scheme** 也能用（`127.0.0.1:port`，从 curl/浏览器插件抄来的
///   最常见写法）；
/// - 走 HTTP 代理时请求行是**绝对 URI**（`GET http://host/path HTTP/1.1`），
///   这是"真的走了代理"而不是"直连成功"的证据。
#[tokio::test]
async fn test_fetch_routes_through_explicit_proxy() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();

    // mock HTTP 代理：收到请求就回一个固定 JSON，并把请求行交回来
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 2048];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            let req_line = String::from_utf8_lossy(&buf[..n])
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            let _ = tx.send(req_line);
            let body = r#"{"via":"proxy"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            let _ = socket.flush().await;
        }
    });

    let runtime = PureTsRuntime::new();
    // 端口 1：直连必然 connection refused，只有经过代理才可能成功
    let ts_code = format!(
        r#"
            globalThis.proxyFetch = async function(): Promise<string> {{
                const res = await fetch("http://127.0.0.1:1/ping", {{ proxy: "127.0.0.1:{proxy_port}" }});
                return await res.text();
            }};
        "#
    );
    runtime
        .eval_ts(ts_code, Some("proxy_fetch.ts"))
        .await
        .expect("加载代理测试脚本失败");

    let out = runtime
        .call_async_fn("proxyFetch", "{}")
        .await
        .expect("带 proxy 的 fetch 失败");
    assert!(out.contains("via"), "响应应当来自 mock 代理：{out}");

    let req_line = rx.await.expect("代理没收到请求");
    assert!(
        req_line.starts_with("GET http://127.0.0.1:1/ping"),
        "HTTP 代理必须收到**绝对 URI** 的请求行（这才是真的走了代理）：{req_line}"
    );
}

#[tokio::test]
async fn test_pure_ts_runtime_timers_and_wait_idle() {
    let runtime = PureTsRuntime::new();

    let ts_code = r#"
            // @ts-ignore
            globalThis.timerState = {
                timeoutFired: false,
                cancelledFired: false,
                promiseValue: null
            };

            // 1. 设置正常触发的 setTimeout
            setTimeout(() => {
                // @ts-ignore
                globalThis.timerState.timeoutFired = true;
            }, 30);

            // 2. 设置被取消的 setTimeout
            // @ts-ignore
            const cancelId = setTimeout(() => {
                // @ts-ignore
                globalThis.timerState.cancelledFired = true;
            }, 50);
            clearTimeout(cancelId);

            // 3. 验证 node:timers/promises
            const timersPromises = require("node:timers/promises");
            timersPromises.setTimeout(40, "AntigravityTimerOK").then((val: string) => {
                // @ts-ignore
                globalThis.timerState.promiseValue = val;
            });
        "#;

    runtime
        .eval_ts(ts_code, Some("timers_test.ts"))
        .await
        .expect("执行定时器脚本失败");

    // 等待所有未决定时器宏任务排空
    runtime.wait_idle().await.expect("等待空闲失败");

    let check_res = runtime
        .eval_ts("JSON.stringify(globalThis.timerState)", None)
        .await
        .expect("查询状态失败");
    let parsed: serde_json::Value = serde_json::from_str(&check_res).expect("JSON 解析失败");

    assert_eq!(parsed["timeoutFired"], true);
    assert_eq!(parsed["cancelledFired"], false);
    assert_eq!(parsed["promiseValue"], "AntigravityTimerOK");
}

/// P4·SP-72：白名单之外的域名**在建立连接之前**就被拒，且理由能直接给作者看。
///
/// 直接打原生桥（而不是 `fetch(...)`）是为了让断言只落在**策略**这一件事上：
/// 走 JS 包装层会引入 Promise 与 polyfill 的噪音，而这里要证明的是
/// "策略先于请求"——真发出去就晚了。
#[tokio::test]
async fn test_net_policy_blocks_disallowed_domain_before_the_request() {
    let runtime = PureTsRuntime::with_environment_console_and_net(
        None,
        Limits::default(),
        crate::env::Needs::everything(),
        ConsoleMode::Capture,
        crate::net::NetPolicy::allow_only(&["allowed.example"]),
    );
    let err = runtime
        .call_async_fn(
            "__native_fetch_async",
            r#"["https://evil.example/x", "{}"]"#,
        )
        .await
        .expect_err("白名单之外的域名必须被拒");
    assert!(
        err.contains("网络请求被宿主策略拒绝"),
        "要说明是**策略**拒的（而不是网络不通）：{err}"
    );
    assert!(
        err.contains("evil.example") && err.contains("allowed.example"),
        "要说清是哪个域名、允许的是哪些：{err}"
    );
}

/// P4·SP-73：**JS 侧捕获到的**错误也要带得住位置。
///
/// 工具路径的错误是在 JS 里 catch 之后回传的（`__dispatch_tool_call` 的 catch），
/// 那里现在用 `err.stack` 而不是 `String(err)`——这条用例钉住的前提是
/// **Boa 真的把 `stack` 暴露成字符串**。若哪天引擎换了实现、`stack` 没了，
/// 报错会静默退回"只有一句话、没有位置"，而这条会红。
#[tokio::test]
async fn test_js_error_exposes_a_stack_string() {
    let runtime = PureTsRuntime::with_environment_and_console(
        None,
        Limits::default(),
        crate::env::Needs::everything(),
        ConsoleMode::Capture,
    );
    let _ = runtime
        .eval_ts(
            "try { undefinedName(); } catch (e) { console.log('TYPEOF=' + typeof e.stack); }\n",
            Some("plugins/probe.ts"),
        )
        .await;
    let out = runtime.take_console().stdout;
    assert!(
        out.contains("TYPEOF=string"),
        "Boa 必须把 `stack` 暴露成字符串（宿主靠它把位置带给插件作者）：{out}"
    );
}

/// 异步 `child_process` 的端到端：真进程 + JS 包装层 + 事件回流。
///
/// 刻意走 `call_async_fn`（工具路径）而不是 `eval_ts`：插件里真正决定"能不能等到子进程"
/// 的不是 JS 的 Promise，而是 `__native_tool_return` 那条回执链路——子进程事件没被
/// 投回事件循环，这条就会挂到墙钟超时，而不是悄悄通过。
#[tokio::test]
async fn test_async_child_process_streams_output_and_invokes_exec_callback() {
    let runtime =
        PureTsRuntime::with_environment(None, Limits::default(), crate::env::Needs::everything());
    runtime
        .eval_ts(
            r#"
            globalThis.__childAsyncProbe = async function () {
                const cp = require('node:child_process');
                const isWin = process.platform === 'win32';
                const stdoutChunks = [];
                const child = cp.spawn(
                    isWin ? 'cmd.exe' : '/bin/sh',
                    isWin ? ['/d', '/s', '/c', 'echo spawn-out'] : ['-c', 'printf spawn-out']
                );
                child.stdout.on('data', (d) => stdoutChunks.push(d.toString('utf8')));
                const closed = await new Promise((resolve, reject) => {
                    child.on('error', reject);
                    child.on('close', (code, signal) => resolve({ code: code, signal: signal }));
                });
                const execResult = await new Promise((resolve) => {
                    cp.exec(isWin ? 'echo exec-ok' : 'printf exec-ok', (error, stdout) => {
                        resolve({ error: error ? String(error.message) : null, stdout: String(stdout) });
                    });
                });
                return JSON.stringify({
                    stdout: stdoutChunks.join(''),
                    pidType: typeof child.pid,
                    exitCode: child.exitCode,
                    closed: closed,
                    exec: execResult,
                });
            };
            "#,
            Some("plugins/child_async.ts"),
        )
        .await
        .expect("注入异步探针失败");

    let raw = runtime
        .call_async_fn("__childAsyncProbe", "[]")
        .await
        .expect("异步探针应当成功");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("探针返回值必须是 JSON");

    assert!(
        parsed["stdout"].as_str().unwrap().contains("spawn-out"),
        "stdout 的 data 事件必须把内容送到 JS：{parsed}"
    );
    assert_eq!(
        parsed["pidType"], "number",
        "spawn 之后 pid 必须是可用的数字：{parsed}"
    );
    assert_eq!(parsed["exitCode"], 0, "close 之后 exitCode 要落定：{parsed}");
    assert_eq!(parsed["closed"]["code"], 0, "{parsed}");
    assert_eq!(
        parsed["exec"]["error"],
        serde_json::Value::Null,
        "exec 成功时回调的 error 必须是 null：{parsed}"
    );
    assert!(
        parsed["exec"]["stdout"].as_str().unwrap().contains("exec-ok"),
        "exec 的 stdout 必须攒齐后交给回调：{parsed}"
    );
    runtime.terminate();
}

/// stdin 回写 + `kill()`。
///
/// `kill` 这条特别重要：它是**唯一**能在超时之前叫停一个跑飞的子进程的入口。
/// 用例不挂死本身就是断言的一部分（不 kill 的话 `sleep 30` 会把这条测试拖到 30s）。
#[tokio::test]
async fn test_async_child_process_stdin_and_kill() {
    if cfg!(windows) {
        // 依赖 `/bin/cat` 与 POSIX 的信号语义；不为 Windows 再造一份（宁缺勿假绿）。
        return;
    }
    let runtime =
        PureTsRuntime::with_environment(None, Limits::default(), crate::env::Needs::everything());
    runtime
        .eval_ts(
            r#"
            globalThis.__childIoProbe = async function () {
                const cp = require('node:child_process');
                const cat = cp.spawn('/bin/cat', []);
                const chunks = [];
                cat.stdout.on('data', (d) => chunks.push(d.toString('utf8')));
                cat.stdin.write('ping-from-stdin\n');
                cat.stdin.end();
                await new Promise((resolve) => cat.on('close', resolve));

                const sleeper = cp.spawn('/bin/sh', ['-c', 'sleep 30']);
                const killed = await new Promise((resolve) => {
                    sleeper.on('close', (code, signal) => {
                        resolve({ code: code, signal: signal, killed: sleeper.killed });
                    });
                    setTimeout(() => sleeper.kill('SIGKILL'), 100);
                });
                return JSON.stringify({ echoed: chunks.join(''), killed: killed });
            };
            "#,
            Some("plugins/child_io.ts"),
        )
        .await
        .expect("注入 IO 探针失败");

    let raw = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        runtime.call_async_fn("__childIoProbe", "[]"),
    )
    .await
    .expect("kill 没生效：子进程把整条用例拖到了超时")
    .expect("IO 探针应当成功");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("探针返回值必须是 JSON");

    assert!(
        parsed["echoed"].as_str().unwrap().contains("ping-from-stdin"),
        "stdin 写进去的内容必须能被子进程读到：{parsed}"
    );
    assert_eq!(parsed["killed"]["killed"], true, "kill() 必须真的发出：{parsed}");
    runtime.terminate();
}

/// SP-13 的延续：**异步** child_process 也要按声明裁剪到原生桥这一层。
///
/// 与 `fetch` 一样的理由：只关 JS 包装挡不住 `globalThis.__native_child_process_spawn_async`
/// 这种直呼，所以门禁必须在注册原生桥时就生效。
#[tokio::test]
async fn test_exec_bridge_is_gated_by_needs() {
    let closed =
        PureTsRuntime::with_environment(None, Limits::default(), crate::env::Needs::plugin_default());
    for expr in [
        "typeof globalThis.__native_child_process_spawn_async",
        "typeof globalThis.__native_child_process_kill_async",
        "(function(){ try { require('node:child_process'); return 'injected'; } catch (e) { return 'blocked'; } })()",
    ] {
        let got = closed.eval_ts(expr, None).await.expect("eval 应当成功");
        assert!(
            got == "undefined" || got == "blocked",
            "未声明 exec 时 `{expr}` 必须不可用，实际：{got}"
        );
    }
    closed.terminate();

    let open =
        PureTsRuntime::with_environment(None, Limits::default(), crate::env::Needs::everything());
    for expr in [
        "typeof globalThis.__native_child_process_spawn_async",
        "typeof require('node:child_process').spawn",
        "typeof require('node:child_process').exec",
        "typeof require('node:child_process').execFile",
    ] {
        let got = open.eval_ts(expr, None).await.expect("eval 应当成功");
        assert_eq!(got, "function", "声明 exec 之后 `{expr}` 必须存在");
    }
    open.terminate();
}
