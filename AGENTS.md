# AGENTS.md

给在本仓库工作的 AI 智能体与协作者的注意事项。**本文件会被自动注入系统提示词**，因此只留
「接手就必须知道的事」；深度约定全文已迁至 **[docs/architecture/conventions.md](docs/architecture/conventions.md)**
（开头有一张「§1–§15 现行对照表」，告诉你哪些章节的正文已经是历史）。

**文档按功能域组织，入口是 [docs/README.md](docs/README.md)**。

项目速览：**纯 Rust 微内核**（`crates/agent-base/`：零 IO 的领域层 + 13 个端口 + **唯一**多轮引擎 `run_turn`）
+ **两层拆包**（S4）：`crates/agent-node/`（节点：会话 / 审批 / 检查点 / 委派 / 插件 / 技能）
与 `crates/agent-rpc/`（桥接面：JSON-RPC 分发 + WS 宿主 + UI 投影）
+ **网关**（S5/S6）：`crates/agent-gateway/`（`ada-gateway`：AGENT 管理平台 + 交互平台 + 桥接平台）
+ **网页界面**：`crates/agent-web/`（SSR 投影层，产品代号 `ssr-h`；网关默认挂载，一个进程一个端口）
+ **桌面客户端宿主**：**Electron 桌面壳**（`electron/`：彻底根除 Windows WebView2 权限污染与目录锁死问题，支持 `bun run electron:dev` 与 `bun run electron:pack`）+ **React 前端**（`desktop-ui/`：通过 `src/client/desktop-bridge.ts` 与 Electron 宿主通信）；
`crates/ts-engine/` 是独立的 TS 执行引擎，只作插件运行时。
**TypeScript 时代的实现（Bun + GPUIX 客户端 + TS 侧 agent/宿主）已于 2026-10-17 整体删除**
（原 `archive/ts-legacy/`，可从 git 历史取回）：它不再是参考设计、不参与构建与测试。
设计与约定的唯一口径是 [docs/architecture/kernel-design.md](docs/architecture/kernel-design.md)（设计）
与 [docs/architecture/conventions.md](docs/architecture/conventions.md)（约定）；
历史计划入口在 [docs/plans/README.md](docs/plans/README.md)。
**功能缺口清单在 [docs/quality/unfinished-features.md](docs/quality/unfinished-features.md)**：接活前先看一眼，
别把"已知未做"当成 bug 去修；做完一项顺手划掉。
**历史接线记录已精简为归档说明**（见 [docs/plans/README.md](docs/plans/README.md)）。

## 开发与验证

```bash
cargo build --workspace                       # 基座四 crate + agent-node + agent-rpc + 产品
cargo test --workspace -- --test-threads=1     # 门二（必须串行：有共享全局态的用例）
bun run typecheck                             # 门一：desktop-ui 的 tsc --noEmit
cargo xtask verify-wiring                     # 接线门：方法覆盖 / 描述符单源 / 端口实现 / 依赖方向
cargo xtask verify-spec                       # 声明门：产品/预设 spec 校验
cargo xtask verify-docs                       # 文档门：归档布局字样只在白名单文档里 + 协议文档与事件主题同步
node tools/verify-gateway-web.mjs             # 网关内置 SSR：单进程 / 单端口 / API 隔离 / 登录 cookie
node tools/verify-ssr-streaming.mjs           # SSR 流式与图片输入（单进程，假模型）
node tools/verify-ssr-approval-abort.mjs      # SSR 审批批准 → 工具执行 → 流式中止（单进程，假模型）
node tools/verify-ssr-workspaces.mjs          # SSR 工作区树与目录选择（单进程）
cargo xtask verify --fast                     # 上面四道静态门一次跑完（跳过全量单测）
bun run electron:dev                          # Electron 桌面端开发（前端热重载 + 网关监督）
bun run electron:pack                         # 打包 Electron 免安装版 (dist/electron/win-unpacked)
cargo run -p agent-gateway -- --port 52353    # 网页访问：页面与 /rpc 同端口（浏览器开 http://127.0.0.1:52353/）
pwsh -NoProfile -File tools/verify-desktop-shell.ps1                    # 桌面壳验收（Electron 交付形态）
```

- **`typecheck` 与 `cargo test` 是两个独立的门，两个都要过**，别只跑一个。
- **Rust 测试必须串行跑**（`--test-threads=1`，`bun run test` 已带）：`test_plugin_and_skill_lifecycle`
  读 `~/.a-da` 配置，而提问链路仍走全局单例 `approval::question_manager::GLOBAL_QUESTION_MANAGER`
  （`OnceLock`），并行会互相污染（单独跑各自通过）。根因是共享全局态（INV-8）；
  提问端口化（S1a）会去掉这个单例，届时再改回并行。
- 🔴 **本机 `C:\pagefile.sys` 上限只有 2 GB**：默认并行度下编译/链接大 crate 会
  `failed to mmap rmeta ... os error 1455（页面文件太小）` 或 `link.exe 1102`。
  用 `CARGO_BUILD_JOBS=2`（或 `-j 2`，**注意 `-j` 要放在 `--` 之前**）即可。
  **这是环境限制，不是代码缺陷**——单独编译失败的 crate 能过、重跑也能过。
- 本机 `cargo` 默认 target 目录编译 `ring` 会报 MSVC `D8050`；加上
  `CARGO_TARGET_DIR=../cargo_target_ada` 复用已有缓存即可（与代码无关）。
- **工具链环境**：现行版本为 **Rust 1.99.0 / Cargo 1.99.0 / Rustup 1.29.1**（MSVC）。
- **应用目录端口在 `cfg(test)` 下指向临时目录**（`agent_node::session::app_home()` →
  `temp/a_da_agent_core_test_home_<pid>`），**但那只覆盖 crate 自身的单测**：
  集成测试（`crates/*/tests/`）里依赖是按**非 test** 编译的，端口仍指真实 `~/.a-da`。
  所以集成测试必须显式注入（见下一条三条铁律），**不要用 `std::env::set_var`**。
- **TS 时代的测试已随归档一并删除**（`bunfig.toml` 也不再需要）；门是 `cargo test`。
- 🔴 **测试绝不能写用户真实的 `~/.a-da`**。2026-10-12 真事故：跑一次 `cargo test -p agent-rpc`，
  用户配置里的激活供应商被改成了测试数据（`anthropic-claude`）、顶层 baseUrl/apiKey/model 一起被改。
  铁律三条：
  ① 应用目录端口是 `OnceLock`——**只要有一个用例先构造过 `AgentStore::new`，同进程后续的
  `std::env::set_var("A_DA_HOME")` 就完全无效**。集成测试一律用
  `AgentStore::with_home(ws, &TempAppHome::at(dir))`（见 `tests/dispatch_integration.rs::store_for_test`）。
  ② `PluginManager` / `SkillManager` 会写 `config.json` / `skills_state.json`，测试里必须
  `PluginManager::with_home(dir)` / `SkillManager::with_home(dir)`（生产路径 `new()` 不变）。
  ③ `save_app_config` 取 **store 自己的 `config_path`**，不再问全局 `get_config_path()`。
  跑完门二顺手比对 `~/.a-da/config.json` 的哈希没变。
- **网关的实例模型是"一工作区一进程"**（`AgentRegistry::id_for_workspace`）：同一工作区的
  **多个会话/多个客户端共用同一个 `ada-coding`**，这是"进程数可控"的前提。
  三条硬约束：
  ① 身份键走 `workspace_key()`：统一分隔符、折叠 `.`/`..`、Windows 下折叠大小写——
     `E:\x` / `E:/x/` / `e:/X` 是同一个目录，**写法差异不许起出第二个进程**；
  ② **实例回收**（`supervisor::ReapPolicy` + `Gateway::spawn_reaper`）：空闲 15 分钟回收、
     同时在跑上限 6（LRU 驱逐）、每分钟扫一次，环境变量
     `A_DA_GATEWAY_IDLE_REAP_SECS` / `A_DA_GATEWAY_MAX_LIVE_AGENTS` /
     `A_DA_GATEWAY_REAP_INTERVAL_SECS` 可调；没有它，访问过 N 个工作区就常驻 N 个进程；
  ③ **有客户端连着就不回收**（`AgentRegistry::connections` + `relay` 的 ConnGuard）：
     用户可能正对着一条长回答发呆十几分钟，掐掉就是"莫名其妙掉线"。
  另外：工作区目录不存在时 `ensure_agent` **拒绝拉起**（为一个空气目录起进程只会白占一个）。
- **网关 ↔ agent 的本体链路有两种**（`A_DA_GATEWAY_AGENT_LINK`，默认 `tcp`）：`tcp` 是回环端口
  （`--host --port 0`，端点 `ws://127.0.0.1:<port>/rpc?token=…`），`pipe` 是
  **Windows 命名管道 / Unix domain socket**（`--pipe <名>`，端点 `pipe://\\.\pipe\…` 或
  `unix:///…`，**端口 0、不监听 TCP**）。两种形态跑同一套 WS 帧与 token 握手，
  客户端察觉不到差别。🔴 **端点解析只有一处**：`agent-gateway/src/link.rs` 的 `AgentEndpoint`，
  `relay` / `http_api::call_agent` / `gateway.delegate` 都走它——各自 `connect_async` 过一份
  就会漂（本轮就是这么发现 `/api/*` 与 `delegate` 会绕过管道的）。管道实例**不做
  `gateway.register` 自注册**（端点无法用 `ws://` 表达）。
- 🔴 **工具内部的路径校验必须走 `ctx.scope`**（`agent_toolkit::fs_tools::PathGate::Scoped`），
  不许再自己调 `check_workspace_sandbox`：引擎在工具执行前用 `Scope` 判过一次，工具内部再按
  `workspace` 硬判一次，`Full`（完全访问）就会在第二处被挡回去——表现是"界面横幅写着可读写
  工作区之外的路径，模型却一直收到『拒绝访问工作区外的路径』"（2026-10-20 用户实拍）。
  插件工具与内部调用用 `PathGate::Workspace`（只认工作区，行为与 P1b 之前一致）。
  **保留目录的红线是"凭据"，不是整棵树**：`~/.a-da` 里 `agent-coding/extensions/`（插件）与
  `agent-coding/skills/`（技能）在完全访问下**可读写**（`ApprovalAwareScope::with_allowed_subtrees`），
  `config.json`、网关发现文件与会话仍然拒；判 `allow` 用的是**规范化后**的路径，
  所以从白名单目录指向 `config.json` 的符号链接照样命中黑名单。
  实机回归：`bun tools/verify-full-access-paths.mjs`（假模型 + 真协议，四个场景各跑一轮）。
- **工具超时是真的会生效的**（`RunPolicy::tool_timeout` ← 产品声明 `policies.toolTimeoutSec`）：
  引擎对**非** `Tool::long_running()` 的工具加超时，超时给一条 `ToolStatus::Timeout` 回执
  （回执里说清是哪个工具、让模型换路子），整轮**继续**而不是永远挂住。
  哪些工具豁免由 `agent_toolkit::registry::LONG_RUNNING_TOOLS` 单一真源声明
  （等审批 / 等提问 / 派活 / 命令执行 / 决策插件），契约测试
  `test_long_running_set_matches_tool_declarations` 守着名单与实现一致。
  🔴 **子进程的 stdin 一律 `Stdio::null()`**（`cmd_tools::run_command` 与 `background`）：
  继承 stdin 时，一条等输入的命令（npm 确认、git 凭据、忘了 `--yes` 的安装器）会**卡满整个超时**，
  界面上的表现正是"工具卡片卡住 → 智能体停止 → 过一会儿才继续"。给空设备它会立刻 EOF 退出。
- 🔴 **文件编辑的换行：匹配按 LF 归一化，写回按文件 EOL 还原**（`agent-toolkit/src/newline.rs`）。
  `read_file` 用 `str::lines()` 输出，**CRLF 文件的 `\r` 不出现在工具输出里**，模型抄回来的
  `old_string` 只带 LF——早先 `after.find(&old_s)` 让跨行片段必然匹配失败（"原文本不存在"，
  模型只能盲试）。现在定位走 `LfView`（折成 LF 再找，带"归一化位置 → 原文位置"映射表，
  命中区间映射回原文切，不能按长度加减），替换文本按文件主导换行还原（否则一次小改把整份
  CRLF 变 LF，diff 全是噪声）；`write_file` 覆写已存在文件时同样跟随其原换行。
  字面 `\r\n` 转义只在"整段没有真实换行"时才当换行解读（有真实换行就不猜，免得改坏
  `println!("a\nb")`）。新增"按文本匹配再写回"的工具都走这套，别自己再写一遍 `find`。
- **别让 `Gateway::new` 有落盘副作用**：它以前在构造时就往工作区存储里记默认工作区，
  而 `WorkspacesStore::new(None)` 指用户真实的 `~/.a-da/gateway_workspaces.json`——
  于是**任何构造 Gateway 的测试都会写用户的真实工作区列表**（历史事故：用户列表里出现
  一堆 `a_da_gw_e2e_*`）。现在构造期不落盘，生产在组合根显式记一次，测试注入
  `Gateway::with_workspaces(临时存储)`。
- **审批事件必须带 `args`**（`AgentEventBody::ApprovalRequested { call_id, tool, args }`）：
  审批卡要回答的是"我到底在批准什么"——危险命令原文、要覆写的文件路径。
  引擎在建卡之前就发这个事件（顺序是 `ApprovalRequested` → 等用户 → `ToolCallStarted`），
  所以审批卡**拿不到** `ToolCallStarted` 的 args，只能自己带；`store.set_tool_waiting_approval`
  会把参数落进卡片（`args`/`raw_args`/`details.approval.args`），界面据此渲染待批准内容。
  只给工具名 = 让用户闭眼点"批准"，那道二次确认闸门形同虚设。
- **审批的操作面只有一处：输入框上方的浮动审批窗口**
  （`desktop-ui/src/components/ApprovalFloatingPanel.tsx`，挂在 `Composer` 的列里，因此**不随对话流滚动**；
  数据源是会话 items 里 `status == "waiting_approval"` 的卡片，回执走 `approval.decide`）。
  对话流里那张卡只留**记录**：待批准内容的预览 + "去浮窗处理"。
  同一件事摆两套按钮，用户会怀疑"点哪个才算数"——按钮只在浮窗一处；
  预览文案两处共用 `desktop-ui/src/utils/pending-approvals.ts` 的 `approvalPreviewOf`，别各写一份。
- **模型请求重试次数**（设置项，默认 5，范围 `0..=20`）这条链要一起改才生效：
  `ConfigSnapshot.retryCount`（wire/`client-ts`）↔ `~/.a-da/config.json` 的 `retryCount` ↔
  `config.setRetryCount`（`spec/proto/ada-coding.ext.json` 也要登记）↔ 每轮
  `TurnRequest::with_max_retries` → `ModelChatOptions.max_retries` → 适配器指数退避
  （429 认 `Retry-After`，上限 30s）。**重试耗尽即断开本轮对话**：不再消费排队指令，
  失败文本走 **`Item::Notice` 卡片**（`persist_failure_notice` 落盘）——只放内存里会被 1.5s 后的
  `sync_threads_from_disk` 整份覆盖，界面上的提示卡片会"闪一下就没"。
  🔴 **报错绝不写进助手正文**：早先它是 `append_assistant_delta` 拼进回答里的，
  于是既把一段连贯回答从中间劈开，又会被当成"模型说过的话"回传给它。
  Notice 的落盘记录在 `fold_messages` 里被明确跳过，所以它**只给人看、永不进模型上下文**；
  重试时的清理必须**磁盘与内存一起清**（`clear_notice_entries` + `clear_notices`），
  只清内存会被磁盘同步把卡片拽回来。界面侧由 `NoticeRow` 渲染（`Transcript.tsx`），
  「请求重试」按钮随之从助手错误卡搬到了这里——那是重试的唯一入口，别只删不搬。
  实机回归：`pwsh -NoProfile -File tools/verify-retry-and-disconnect.ps1`
  （起真产品、走真 JSON-RPC、改用户配置前先备份后还原）。
- 🔴 **流式请求不许设"总时限"，流结束必须验终止信号**（`agent-adapter/src/model/stream.rs`）。
  2026-10-18 真事故：`reqwest::Client::builder().timeout(120s)` 覆盖的是**建连到响应体读完**的
  全过程，而 SSE 是长连接——思考型模型一条回答跑两三分钟是常态，于是 120 秒一到就被从中间切断
  （`durationMs=120008`、thinking 流了 58k 字符后正文停在半句）。现在只留
  `connect_timeout(10s)` + **空闲**超时（`A_DA_STREAM_IDLE_SECS`，默认 90 秒没有新字节才判死流）。
  另一半同样致命：EOF **不等于**完成。三套协议都必须见过终止信号（`[DONE]` / `finish_reason` /
  `message_stop` / `response.completed`）才算正常收尾，否则 `bail!` 走失败路径（落 Notice 卡 + 可重试）。
  早先 EOF 直接落到 `final_stop_reason = "stop"`，于是一次被掐断的回答会被当成正常完成——
  没有错误卡、没有重试，界面只是停在半句话上，用户看到的就是"会话突然断开"。
  守门用例：`test_stream_truncated_without_terminator_is_an_error` /
  `test_stream_with_done_terminator_completes_normally` / `test_stream_idle_timeout_reports_dead_stream`。
- **图片输入（多模态）这条链也要一起改才生效**：`thread.send` / `thread.editAndResend` 的
  `images`（**data URI**）→ `dispatch::normalize_images`（**全仓唯一的图片 IO 边界**：
  裸路径在这里读盘转 base64，读不到就丢弃并记日志）→ `spawn_thread_loop` 的 `current_images`
  → `run_agent_turn` → `TurnRequest::with_images` → `format_messages_for_model` 把 `images`
  挂到那条用户消息上 → 适配器按协议编成内容块（OpenAI `image_url` / Anthropic `source.base64`
  / Responses `input_image`）。
  🔴 `ChatCompletionMessage.images` **刻意 `#[serde(skip)]`**：三家协议的图片形状不同，
  且**没有图片时 `content` 必须是裸字符串**（数组形态会被部分上游 400），所以编码只在适配器里做，
  消息类型只负责把图片带过去——别把它改成可序列化字段，`test_chat_completion_message_does_not_serialize_images` 守着这条。
  声明侧：`capabilities.images` 来自产品 spec（`ada-coding` 与 PM 预设都是 `true`），
  单个模型认不认图由 `ModelEntry.supportsImages` 表达——**两者不是一回事**。
  界面那排"图片素材"胶囊只是投影：它在没接通之前也一样显示，所以别拿它当"已经发给模型了"的证据。
- 🔴 **卡片顺序由 `at` 表达，而且必须两处都守**：增量帧 `evt.state.patch` 的 `upserted`
  是**按 id 排序**发的（帧内容要可复现），界面给新卡片定位只能按 `at`
  （`snapshot-merge.ts::insertByItemTime`，等时间戳则保持已有相对顺序）。所以
  **一个会话内卡片的 `at` 必须严格递增**：
  内存侧 `agent_rpc::state::store::next_item_at`（同一毫秒连建两张卡——思考 + 首段正文、
  非流式模型一次返回的思考与正文——也会踩坑），磁盘回填侧
  `agent_node::session::SessionManager::CardStamps`（顺序为**思考 → 正文 → 工具卡**，
  与流式现场一致）。破了这条的表现是：**轮次一收尾，思考块从最上面掉到最下面**
  （内存卡片被磁盘回填整份换掉、id 全变，那一帧按 id 字典序落位）。
  只读 `session.snapshot` 是**看不出**这个问题的——服务端数组一直是对的，被换乱的是界面拼出来的那份；
  实机验收用 `bun tools/verify-card-order-e2e.mjs [--with-tool]`（本地假模型 + 真协议，
  比较收尾前后**界面上**的顺序），契约见 `docs/protocol/README.md` §4.3 的"顺序"一节。
- 🔴 **Electron 宿主里 `__dirname` 不可信**：`bun run electron:build:main`（`bun build --target node
  --format cjs`）会把 `__dirname` **硬编码成构建机的源码目录**。用
  `path.join(__dirname, 'preload.cjs')` 当 preload 路径 → preload 静默加载失败 →
  `window.electronAPI` undefined → **界面拿不到网关端口（显示"未连接"）、窗口按钮全部空操作**，
  这就是"Electron 版功能缺失"的根因形态。资源路径一律走 `resolveAppPath()`
  （`app.getAppPath()` 优先 + 候选清单 + 真实存在性检查），并且失败要**大声报**
  （`dialog.showErrorBox` + launcher.log），不要静默降级。
- 🔴 **无边框窗口的标题栏必须压在所有应用模态层之上**：应用内弹窗统一是 `fixed inset-0 z-50`，
  标题栏若没有更高层级（现为 `relative z-[100]`），任何弹窗打开时最小化/最大化/关闭**点不动**、
  窗口也拖不动——第二个"无法最小化/关闭窗口"的形态。`tools/verify-desktop-shell.ps1`
  专门守着这条（弹窗开着时点最大化仍须生效）。
- **渲染进程日志要能进 launcher.log**：Electron ≥32 的 `console-message` 签名是
  `(event, details)`（旧的是 `(event, level, message, line, sourceId)`）。按旧签名写就只剩
  undefined，界面里的错再也查不到；`preload-error` / `render-process-gone` / `did-fail-load`
  一并转发。打包后的 exe 没有控制台，**应用日志是唯一证据**（§17）。
- **桌面壳的行为要按"点一下"验收**：`pwsh -NoProfile -File tools/verify-desktop-shell.ps1`
  （Electron 交付形态）。真鼠标点窗口按钮 + 读 `launcher.log`
  断言"拉起网关 → 界面接入 → 窗口控制 → 状态同步 → 退出清理"。脚本注释里记了必踩的坑：
  窗口句柄要按标题找（`MainWindowHandle` 会挑到隐藏/辅助窗口）、按钮定位要按「与背景色的差异」
  而不是亮度（暗色主题下整条标题栏都算"暗"）、每次点击前重新量窗口矩形（最大化会改坐标）、
  截图前必须把窗口顶到前台（否则截到/点到的是盖住它的窗口）。
  另：脚本会清除 `ELECTRON_RUN_AS_NODE`——被污染时 Electron 会以"纯 Node"跑 main.cjs 当场崩掉。

## 代码结构

**目录结构按设计落地**（`docs/architecture/kernel-design.md` v0.2 §3）：

- `crates/agent-base` 基座内核（零 IO、零产品名词）：`model/`（模型面类型）、`domain/`（消息·工具描述符与回执·事件·错误与失败方向·诊断点位）、`ports/`（**13 个端口**，**无默认实现**：AppHome / ApprovalGate / CancelToken / Clock / EventSink / HookRunner / ModelClient / PromptSource / Scope / ScriptHost / SessionStore / Tool / ToolCatalog）、`engine/`（**唯一**多轮循环 `run_turn` + 钩子编排）、`testing/`（FixedClock/RecordingSink/TempAppHome）
- `crates/agent-proto` 线协议：JSON-RPC 帧、方法常量、线上 DTO、错误码
- `crates/agent-adapter` 适配器：`model/`（三家协议 SSE + 中止）、`app_home.rs`（**全仓唯一**读 `A_DA_HOME`/`USERPROFILE` 的地方）、`clock.rs`（**全仓唯一**读系统时间的地方）
- `crates/agent-toolkit` 工具包：文件读写、路径沙箱、命令执行、文本 diff、Git/项目查询、插件包脚手架；`registry.rs` 是**工具元数据与描述符单一真源**（20 个描述符），`toolkits.rs` 是工具包名单单一真源（`core`/`fs`/`command`/`git`/`project`/`forge`）
- `crates/agent-runtime` 组合根：`ProductBuilder`、产品声明 `AgentSpec`、贡献装配与工具目录；`agent-host` 按声明把它装配成带真实端口的运行时
- `crates/agent-host` 宿主装配层：`run_from_spec` / `HostedProduct`，产品二进制与测试共用同一条装配路径
- `crates/agent-conformance` 合规套件：端口契约、跨端口不变量与 golden 夹具（`dev-dependencies` 使用，不参与产品运行时）
- **`crates/agent-node`（S4 拆出，src 46 文件 / 约 1.46 万行）** —— 节点：`session` / `approval` / `checkpoint` / `subagents`（含 `agent_bus.rs` 委派总线端口与 `local_bus.rs` 本地实现；端口支持**多轮续跑** `thread_id`，本地总线对它是**如实拒绝**——一次性子智能体没有可续线程）/ `plugins` / `skills` / `node_config.rs`（节点配置端口）/ `delegation_depth.rs`（委派深度端口：被派活的节点靠它知道自己「在第几层」）。**不含协议管道**（不依赖 `tokio-tungstenite`）、**不含 UI 投影**（不依赖 `AgentStore`）。`AgentBus` 的网关实现（`GatewayAgentBus`）在 **`agent-rpc`**——跨进程委派需要 WS 客户端，那属于桥接面，节点只保留端口
- **`crates/agent-rpc`（S4 拆出，src 20 文件 / 约 9.5k 行）** —— 桥接面：`server/`（`dispatch/` 77 个方法的 JSON-RPC 分发、`ws.rs` 宿主、`emitter.rs` 快照合帧、`fs_service.rs`）、`state/`（`AgentStore`，**给界面看的投影**）、`runner/`（引擎调用 + 基座一等事件 `AgentEventBody` 直连投影至 UI store）。**依赖方向单向：`agent-rpc → agent-node`**
- **`crates/agent-gateway`（S5 新建）** —— a-da 网关，三合一：**AGENT 管理平台**（`registry.rs` 实例注册表 / `supervisor.rs` 生命周期）+ **交互平台**（S6：`delegate.rs` 派活/**多轮续跑**/取消跨网关）+ **桥接平台**（`relay.rs` 路由与透传）+ **WEB 接入面**（S7：`auth.rs` token/作用域/Origin/**配对码**）+ **SSR 挂载面**（`web.rs`：`GatewayBackend` / `GatewayLogin`）。二进制 `ada-gateway`（打印 `A_DA_GATEWAY_READY {port}`）。**只依赖线协议 `agent-proto` 与 UI 投影层 `agent-web`**——不含引擎（INV-1）、不缓存会话状态（INV-8），由 `verify-wiring` check J 守着。🔴 **非回环 `--host` 必须配 `--token`，且必须显式 `--allow-plaintext`**（网关只提供明文 `ws://`，明文上的 token 可嗅探；反代终止 TLS 时由人确认）。两条都**拒绝启动**而不是警告——失败安全。🔴 **SSR 的事件订阅必须持有 `AgentRegistry::open_connection` 守卫**：它不经过 `relay`，不计数的话回收器会把正在被浏览的 agent 当空闲杀掉（连开 15 分钟的页面会“突然不动”）
- **`crates/agent-web`（2026-10-25 由独立 `ssr-h` 合并）** —— SSR 网页界面：`agent.rs`（`AgentClient`）、`backend.rs`（`WebBackend` 四原语 + `HttpBackend`）、`router.rs`（可挂载 Router + 内嵌静态资源）、`handlers/`（页面/表单/SSE）、`plugins/` + `tsx/`（前端插件与 Boa/OXC 运行时）、`guard.rs`（Host/Origin/CSP/登录 cookie）。`agent-gateway` 依赖它并默认以 `--web-ui ssr` 挂载；独立 `ssr-h` 二进制仅供开发/验收。**不得依赖 `agent-node` / `agent-base` / `agent-runtime` 等节点/引擎 crate**（check J 扩展为同时扫描本 crate）
- `crates/ts-engine` 插件运行时（Boa + oxc）；插件契约与平台说明见 [docs/plugins/README.md](docs/plugins/README.md)
- `crates/plugin-hub` **插件库平台**（独立于 agent）：Axum MVC + SQLx(SQLite) + Askama SSR + HTMX/Alpine/Tailwind；入口见 [crates/plugin-hub/README.md](crates/plugin-hub/README.md)
- `products/ada-coding` 产品二进制（`--host/--port/--token/--parent-pid/--workspace`）
- **PM 不是第二个产品**（2026-10-11 降级）：它是 `products/ada-coding` 的**模式预设**
  （`products/ada-coding/modes/pm.agent.spec.json`）——同一个二进制、同一处装配函数
  （`build_injection`），只是喂了另一份声明：persona（拆解/分派/汇总）+ 不含 `fs`/`command`
  的工具包（角色边界：PM 不亲自改代码）+ 委派配置（有网关走网关，否则进程内本地委派）。
  会话是 coding 还是 pm 由 **`mode`** 表达，`agent_id` 恒为 `ada-coding`；老盘上
  `agentId=ada-pm`/`pm-assistant` 的会话在**读盘时自动归一并写回**（幂等）。
  模式预设同样受 `verify-spec` 门禁约束（`products/<产品>/modes/*.agent.spec.json` 也在扫描范围内）。
- 所有内部调用点直连基座（`agent-proto` / `agent-toolkit` / `agent-base` / `agent-adapter`），历史 shim 与 facade 已全量下线
- `electron/` **Electron 桌面宿主**：主进程监督 `ada-gateway` 生命周期、单实例锁、无边框原生控制与 IPC 桥接；`dist-electron/` 为构建中间产物
- `desktop-ui/` **React 桌面前端**：通过 `src/client/desktop-bridge.ts` 与 Electron 宿主通信（`ws-client.ts` 是唯一协议客户端）
- `dist/` **发布交付套件**：包含 `ada-gateway.exe`、`ada-coding.exe` 以及 `dist/electron/win-unpacked/`（Electron 完整绿色便携包，内置 Chromium 运行时与 `resources/bin/` 关联二进制，彻底规避 WebView2 权限污染）

## 核心红线与防踩坑规矩

- **不要复活 TS 时代的实现**：它已于 2026-10-17 整体删除（可从 git 历史取回）。仓库里只保留 Rust 这一份引擎/宿主；新增代码不得引入第二套多轮循环。
- **改 Rust 核心前先看 [docs/quality/feature-catalog.md](docs/quality/feature-catalog.md) 与 [docs/quality/unfinished-features.md](docs/quality/unfinished-features.md)**：审批闸门、取消贯穿、`terminate`、插件 `enabled`、空壳工具等曾被列为"看起来装上了、其实没接线"，那里逐条写明了处置口径（补实现 / 删声明）。**注**：该表是历史记录，其中大部分已收口——判断现状请以代码与 [docs/quality/feature-catalog.md](docs/quality/feature-catalog.md) 为准。
- **工具描述符与元数据单一真源**（W2-T2 + 单源化收敛）：读写性（`is_write`）、审批要求、
  检查点回滚策略（`rollback`）、终止语义，以及全部 20 个内置工具的标准描述符与参数 Schema，
  均收敛于 `crates/agent-toolkit/src/registry.rs`；具体实现统一调用 `descriptor()` 构造，严禁内联手写；
  `test_builtins_descriptor_equality` 契约测试（INV-3）机械守门。
  历史上那五处手写名单（`lib.rs` 读写判定、`approval/types.rs` 命令工具、`executor.rs` 检查点、
  子智能体白名单、插件自述 `is_write`）**只剩插件自述一处**，且已改为按描述符校验。
- **事件流单一真源**：引擎多轮循环向外发射基座一等事件 `AgentEventBody`（13 个变体，其中
  `Diagnostic` 承载扩展点遥测：点位名在 `point` 字段、词汇表在 `domain/diag.rs` 的 `DIAG_POINTS`，
  **只登记真有发射方的点位**，由 `verify-wiring` check K 盯着），
  `agent-rpc` 桥接层直接订阅并投影到 UI，严禁在桥接层私造二等中间事件流（历史 `AgentLoopEvent` 已废除）。

## § 索引（`AGENTS.md §N` 一律指下表第 N 条，正文见 [docs/architecture/conventions.md](docs/architecture/conventions.md)）

> ⚠️ **历史说明（2026-10-18 更新）**：下表与 conventions 的 §1–§15 写于 **TS 时代**
> （那套实现已删除，可从 git 历史取回），命名与路径可能已对不上代码。
> **`conventions.md` 开头有一张「现行对照表」**，逐条给出当前落点（例如 §2 的只读判定
> 现在是 `ToolDescriptor.access` + `plugin_tool_is_write`，§4 的检查点名单现在是描述符的
> `RollbackPolicy`）——接手时先看那张表。
> §16（协议方法多端同步）、§18（工具结构化回执）与 §19（文件换行）按原样继续生效。

| § | 一句话警告 |
|---|---|
| 1 | 子智能体 `allowedTools` 是写死白名单、与插件表**脱钩**：新工具不同步就白装 |
| 2 | `isWriteTool` **失败安全**：只读工具漏进 `READ_ONLY` 就被当写工具，plan/只读子体拿不到；非内置插件借走只读内置名也一律算写 |
| 3 | 子智能体只拿到 `profile.systemPrompt`，**没有** AGENTS.md 与主线程系统提示词 |
| 4 | 新写工具须进 `CHECKPOINT_TOOLS` 与 `checkpointPathsOf`（批量要逐文件进快照） |
| 5 | 工具结果的 `terminate: true` 结束的是**整轮**，不是"这批" |
| 6 | **当前工具始终串行**：`max_parallel_tools` / `Execution::ParallelSafe` 尚未被引擎消费，别按并行语义写代码 |
| 7 | 子智能体收尾的 `wakeParent()` 必须在 `runningThreadIds.delete()` 之后；离线兜底单独补 |
| 8 | 别轮询 `check_subagent`，用 `await_subagents` 挂起等待 |
| 9 | 决策插件**绝不捏造确定性**：拿不到就失败（**`decide` / `check_gate` 已于 2026-10-18 下线**，原则保留） |
| 10 | 插件生效需**多处同时登记**（READ_ONLY／目录／allowedTools），各有守门测试 |
| 11 | `LoadedPlugin` 是唯一产物；第三方插件 id 由加载器定为「scope:文件名」 |
| 12 | 钩子**成对**是硬约束，工具集**只能收窄**；两处刻意偏差别当 bug 修 |
| 13 | 审批／压缩的效力**刻意不对称**；文末另有 UI 单窗口约束 |
| 14 | 审批：**策略**归插件 `approval-guard`、**执行**归核心 `askUser`；点位顺序不能反 |
| 15 | 陷阱：钩子"声明了却没人调用"是静默失效——**断言副作用**，别断言钩子被调用 |
| 16 | 界面只认 `ui/client` 四条通道；**加协议方法要同步四处**（spec → methods.rs → dispatch → client-ts，有守门测试） |
| 17 | 打包后的 exe **收不到自动化通道**（`console.log` 被劫持进日志）——二进制检查要用应用日志当证据 |
| 18 | 工具结果是一份**结构化回执**（`status`／`duration_ms`／`started_at`／`finished_at`，读写两侧都认 snake/camel）：塞裸文本不报错，只是界面耗时与状态徽章**静默变空** |

> 兼容说明：历史上写作「`AGENTS.md` §9」「`AGENTS.md` 第 1 条」的引用，按上表第 N 条理解。
> 编号在迁出后**原样保留**，故 `docs/` 与源码注释里的既有引用无需改动。
