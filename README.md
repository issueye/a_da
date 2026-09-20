# a_da

<img src="./assets/logo.svg" width="88" align="right" alt="a_da logo" />

一个本地 AI 编码 Agent 的桌面程序，用 [GPUIX](../gpuix) 写界面：React 组件直接由
GPUI 渲染到 GPU（Windows 上是 DirectX），没有 Electron、没有 WebView。

标志是一条 shell 提示符 `>_`：Agent 就是在工作区里跑命令的那个东西，方块的落点正好是
名字里那条下划线。源文件是 `assets/logo.svg`（1024 网格，暗色圆角块 + 白 chevron +
红色光标块）和 `assets/logo-mark.svg`（去掉底块的单色版，用 `style.color` 上色）。

![a_da 主界面](./docs/app.png)

一轮真实的回合：流式回复、模型先想一段（可展开的「思考」行）、需要批准的写入，以及点开
之后由原生 `<diff>` 渲染的改动卡片（这张图来自 `bun scripts/smoke.ts`，模型是本地 mock）：

![一轮完整的 Agent 回合](./docs/turn.png)

窗口里的每一像素都是 GPUIX 画的：侧边栏、会话列表、欢迎卡片、输入框和底部的三个
设置芯片。标题栏是一条 36px 的窄条：图标与标签页之间那片空白是拖拽区，整条都能拖动
窗口；右侧的最小化 / 最大化 / 关闭按钮在 Windows 上通过 `user32.dll` 真正作用于窗口。

## 设置弹窗

侧边栏底部的齿轮打开设置：标题栏与内容之间是一条 1px 的分隔线，内容是左右两栏——
左边切分节，右边是该节的表单。默认停在「供应商」，因为这是不填就没法工作的那项。

![设置弹窗的供应商一栏](./docs/settings.png)

- **供应商**：预设（OpenAI / DeepSeek / 阿里云百炼 / Moonshot / Ollama / 自定义）
  会填好接口地址和常用模型；下面三个字段可以随便改
- **保存** 写进 `~/.a-da/config.json`，并保留文件里其它字段（手写的键不会被覆盖）
- **测试连接** 用当前的地址、Key、模型发一次最小请求，结果直接显示在按钮右边
- **工作区**：当前项目、索引统计、项目与会话数量、配置文件的落盘位置、工具清单
- 按 `Esc` 或点右上角关闭；弹层会同时吞掉点击和滚轮，后面的会话区不会被误操作

> 环境变量（`A_DA_API_KEY` / `A_DA_MODEL` / `A_DA_BASE_URL`，以及 `OPENAI_*`）优先于
> 配置文件。检测到它们生效时，弹窗会明确提示“运行时会用环境变量的值”。
> `A_DA_CONFIG` 可以换掉配置文件的路径（测试就靠它不碰你真实的配置）。

> 配置、会话流水、全局扩展、调试日志都在同一个数据目录 `~/.a-da` 下；
> `A_DA_HOME` 可以换掉这个目录。
> `A_DA_NO_DIALOG=1` 关掉原生目录选择弹窗（脚本与自动化用；界面会退回到手输路径）。
> 网络要走代理就设 `HTTPS_PROXY` / `HTTP_PROXY`：Bun 的 fetch 认这两个变量，模型请求和
> 扩展的网络请求都会跟着走。

## 它做什么

在下面的输入框里用自然语言描述任务。Agent 会在**当前项目目录**内工作：

| 工具 | 作用 |
|---|---|
| `list_files` | 列目录（跳过 `node_modules`、`.git`、`dist`、`build`、`coverage` 等） |
| `read_file` | 读文本文件，支持 `offset`/`limit` 按行段读（默认 400 行）；超 512KB、二进制、目录都拒绝 |
| `search_files` | 用正则搜索代码（最多 200 条命中） |
| `write_file` | 新建或整体覆盖文件，返回 unified diff |
| `edit_file` | 精确替换一段文本，要求 `old_string` 唯一；也可以一次给多组 `edits` |
| `run_command` | 在工作区内执行 shell 命令（默认 120s 超时，可延长到 600s，输出截断） |

这份工具表不是写死的常量：每轮开始时向 `ToolRegistry` 要一次，所以扩展注册的工具也会
出现在模型的工具表里（见下面的「扩展」）。

所有路径都会被解析回该会话所属项目的根目录，任何指到项目外的路径都会被拒绝——
包括 `run_command` 的 `cwd`。这是侧边栏红字那句承诺的实现位置：
`src/agent/tools/workspace.ts`。它按路径字符串判断，不解析符号链接：工作区内一个指向
外面的 symlink 仍会被放行。

### 一轮对话是怎么跑的

模型侧的一切都在 `src/agent/core`。`runAgentLoop` 是一个异步生成器：一边流式接收
`ai/stream.ts` 的增量，一边把生命周期事件 yield 出去；`store.ts` 订阅这些事件并翻译成
界面卡片，它自己不跑循环、也不自己拼消息。

- **审批闸门**挂在 `beforeToolCall` 上。拒绝走的是 `block` 而不是「工具执行失败」：循环会
  把拒绝理由当成一次工具结果回给模型，模型知道是被拒了，不会以为调用成功了
- **一行一次调用**。会话区里每个工具调用默认只占一行：三角、图标、工具名、目标
  （文件名在前、目录是暗色小字）、改动量 `+N −M`、以及还没有结束的状态（等待批准 /
  执行中 / 失败 / 已拒绝）。点这一行才展开细节——`<diff>` 或工具输出；命令展开后是
  一份终端记录，先是 `$ 命令` 再是它的输出。跑完的行不再挂状态字，行不再动就是结束了。
  **失败和被拒的行也默认收起**，红色的状态字已经说明了出过事，点开才看原因
- 行距按「行高 + 3」排：一屏要能读下几十次调用。超出宽度的部分以**省略号**收尾——
  这需要在弹性行里给文字 `minWidth: 0`，否则弹性项撑着内容宽度不收缩，只会被硬裁
  （`scripts/rows-check.ts` 按像素盯住这三件事：行距、收起行下面不该有线、长参数不许顶到
  内容列边缘）
- **思考链单独一行**，默认收起，只显示「思考 · 持续 N 秒」，点开才是推理原文。它通常
  是整轮里最长的东西，不该把回答挤下去。这一行只在接口真的吐了 `reasoning_content` /
  `reasoning` 时出现
- 工具**顺序执行**——审批一次只该问一件事，命令之间也不该互相抢工作目录。`runAgentLoop`
  本身也支持并行（每个工具各有一条驱动任务，事件按到达顺序转发）
- 工具输出**实时**上浮：`run_command` 的标准输出在命令还在跑的时候就到达卡片，
  而不是等结束才补发
- 会话历史就是 `thread.messages`（`core/types.ts` 的 `AgentMessage`），也就是真正发出去过的
  那一份，没有第二套消息形状需要转换
- 撞上单轮 24 步上限会被事件流告知（`agent_end` 的 `reason`），界面据此提示可以继续输入

### 扩展

打开一个项目时会自动加载两处 TypeScript 扩展（用 `jiti` 直接执行 `.ts`）：

- `<项目>/.ada/extensions/*.ts`
- `~/.a-da/extensions/*.ts`

扩展拿到一个 `ExtensionContext`：`registerTool` 注册工具、`onEvent` 订阅 Agent 生命周期
事件、`trace` 往调试面板打点。注册进来的工具会和内置工具一起发给模型，也能被调用。

仓库里带了一个能跑的例子：**`.ada/extensions/web-search.ts`**（联网搜索）。它演示了
整套 API，也刻意不 import 应用里的任何东西——扩展是要能被复制的独立文件。想让它对
每个项目都生效，把它拷到全局目录：

```bash
cp .ada/extensions/web-search.ts ~/.a-da/extensions/
```

它用 Bing 而不是 DuckDuckGo 抓结果，因为后者在部分网络里连不上（实测超时）、且不需要
key 的选择里 Bing 最稳。**网络要走代理时不用改代码**：Bun 的 fetch 认
`HTTPS_PROXY` / `HTTP_PROXY`，应用和扩展的请求都会跟着走，例如
`HTTPS_PROXY=http://127.0.0.1:7897 bun app.tsx`。

验证它有没有真的跑通：

```bash
bun scripts/extension-check.ts   # 加载 → 进工具表 → 真调一次 → 结果回给模型（需要联网）
```

> ⚠️ 项目里那份是随仓库克隆进来的第三方代码，**打开项目就会被执行**。因此「只读」模式下
> 扩展工具一律要审批：只读白名单是写死的三个内置名字，名单之外的一律当成写操作。

### 工作区与会话

一个会话（Thread）从建立那一刻起就绑定一个工作区目录，之后切换工作区不会改变它：
正在等待批准的写入，即使你已经切到别的工作区，落盘的位置仍然是它自己那个目录。

- 侧边栏 **工作区** 列出所有已打开的项目，点一下切换；**添加项目** 会打开系统原生的
  目录选择弹窗（Windows 的 PowerShell + FolderBrowserDialog），选中即加入。弹窗开不出来
  时（非 Windows、`A_DA_NO_DIALOG=1` 的自动化场景）同一个位置会留一个输入框可以直接
  粘贴路径。加进来之前都会先检查存在且是目录，失败时在行内报错，不会加进来一个永远
  报错的项目
- **会话** 只显示当前工作区的会话；每个工作区的会话列表互相独立。每行最右边有个垃圾桶
  图标，**点两下才删**：第一下把图标变成红色的「确认删除」（鼠标移出这一行就复位），
  第二下真的删——会话记录是这一轮的唯一副本，删掉就没了（盘上那份 JSONL 也一起删）
- 删掉某个工作区的最后一个会话时，会补一个新的空会话，工作区不会因此从侧边栏消失；
  正在跑的那一轮不能被删（先停止）

![两个项目与它们的会话](./docs/projects.png)

### 三个芯片

- **自动批准 / 每次询问 / 只读** —— 写入和执行何时需要你点“批准”。
  等待批准时，工具卡片里会出现批准 / 拒绝两个按钮，只有你点了才继续。
- **刷新** —— 重新扫描工作区。
- **调试** —— 打开右侧事件日志：模型端点、每次工具调用、每个错误。
- **最高 / 高 / 中 / 低** —— 传给接口的 `reasoning_effort`。

一轮还没跑完时继续输入，指令会排队（输入框会提示“继续输入以排队后续修改”），
同时芯片区会出现“停止”。

## 运行

```bash
bun install          # 安装 typescript / 类型
bun run link         # 把本地 ../gpuix 的两个包连进来（见下）
bun run icons        # 从 assets/logo.svg 生成 logo.png 与 logo.ico（改了 logo 才需要）
bun run dev          # 开发：bun --hot app.tsx，保存即重挂载
bun run build        # 产出 dist/a-da-core.exe（真正的程序）与 dist/a-da.exe（启动器）
bun run typecheck    # tsc --noEmit，和 bun test 是两个独立的门，两个都要过
```

先决条件：`../gpuix` 已经 `bun install` 且 `bun run build`（编译出
`packages/native/gpuix-native.*.node` 与 `packages/react/dist`）。

Windows 上 build 产出两个文件：`a-da-core.exe` 是真正的程序（带 native addon 与图标），
`a-da.exe` 是用 `csc.exe` 编译的 C# 启动器（`scripts/launcher.cs`），它用
`CREATE_NO_WINDOW` 静默拉起 core，双击时不会弹控制台黑框。构建脚本还会把 core 的 PE 头
改成 GUI 子系统（`IMAGE_SUBSYSTEM_WINDOWS_GUI`），因为 Bun 的 `windows.hideConsole` 目前
仍会留下子系统 3。机器上没有 .NET Framework 的 `csc.exe` 时启动器会被跳过（只留一条
warning），`a-da.exe` 也就不存在；`scripts/binary-check.ts` 因此优先直接启动 core。

### 为什么要 `bun run link`

`@gpuix/react` 通过 `workspace:` 协议依赖 `@gpuix/native`，这个协议只在 gpuix 仓库
内可解析，所以这里不把 gpuix 的两个包写进 `dependencies`。另外**整个应用必须和
gpuix 共用同一份 React**：reconciler 把 hook dispatcher 装在它 import 的那份 React
上，装进来第二份 React 会让每个 `useState` 直接报错。`scripts/link.ts` 用目录
junction 建好这几条链接（Windows 下不需要管理员权限），并检查两端是否都已编译。

### 配置模型

任何 OpenAI 兼容的 `/chat/completions` 端点都可以。优先级：环境变量 →
`~/.a-da/config.json`。

```bash
A_DA_API_KEY=sk-…
A_DA_MODEL=gpt-4o-mini
A_DA_BASE_URL=https://api.openai.com/v1   # 可选，默认 OpenAI
A_DA_WORKSPACE=/path/to/project           # 可选，默认进程当前目录
A_DA_HOME=/path/to/data                   # 可选，默认 ~/.a-da
```

也可以直接在设置弹窗里填写（写的就是这个文件，双击 exe 启动时同样生效）：

```json
{ "apiKey": "sk-…", "model": "gpt-4o-mini", "baseUrl": "https://api.openai.com/v1" }
```

没配置时程序不报错，进入**离线模式**：仍然真的跑一次 `list_files`，把工作区规模
和配置方法告诉你，方便先看界面。

## 代码结构

```
app.tsx                     入口：render(<AgentWindow />)、窗口选项与平台引导
src/AgentWindow.tsx         窗口骨架：标题栏 + 侧边栏 + 会话区 + 输入区 + 设置弹层
src/theme.ts                颜色、字号、行高、markdown 主题、路径缩写
src/icons.tsx               内联 SVG 图标（随二进制一起打包）
src/agent/home.ts           应用数据目录（~/.a-da，A_DA_HOME 可换）
src/agent/store.ts          界面状态层：把事件翻译成卡片、审批闸门、会话落盘
src/agent/config.ts         供应商配置：环境变量 / 配置文件 / 预设 / 连通性测试
src/agent/core/agent-loop.ts  事件循环：多轮流式、工具调度、生命周期事件与钩子
src/agent/core/agent.ts     Agent 控制器：订阅事件、维护状态、转向/后续消息队列
src/agent/core/types.ts     消息与事件模型（AgentMessage / AgentEvent / AgentTool）
src/agent/ai/stream.ts      OpenAI 兼容的流式客户端（SSE、思考链、工具调用分片累积）
src/agent/tools/registry.ts   工具注册中心（内置 + 扩展）与只读白名单
src/agent/tools/workspace.ts  路径沙箱与遍历跳过清单（所有工具共用一份）
src/agent/tools/loader.ts     用 jiti 加载工作区 / 全局的 .ts 扩展工具
src/agent/tools/builtins/*    六个内置工具
src/agent/tools.ts          对外适配层：runTool / describeTool / scanWorkspace
src/agent/session/manager.ts  会话 JSONL 落盘（~/.a-da/sessions）
src/agent/patch.ts          行级 LCS → unified diff（给 <diff> 渲染）
src/agent/types.ts          界面条目形状（Item / Thread / DebugEntry）
src/ui/SettingsDialog.tsx   设置弹窗：header + 横线 + 左右两栏
src/ui/*                    标题栏、侧边栏、会话区、输入区、事件日志、控件
src/platform/win32.ts       无边框窗口的拖动 / 最小化 / 最大化 / 关闭
src/platform/dialog.ts      原生目录选择弹窗（PowerShell + FolderBrowserDialog）
assets/logo.svg             应用标志（含底块，给图标与文档用）
assets/logo-mark.svg        单色标志（用 style.color 上色，欢迎卡片里就它）
assets/logo.ico|png         icons 脚本生成的产物，build 会把它塞进 exe
.ada/extensions/web-search.ts  扩展的例子（联网搜索），见「扩展」
scripts/*                   一套对着真实窗口 / 真实进程的检查，见「测试」
```

界面层没有状态管理库：`store` 是模块级单例，异步轮次直接改它，React 通过
`subscribe` 收到通知后重渲染，所以流式回复在到达过程中就是对的。

## 测试

```bash
bun test                        # 85 个用例：沙箱与 diff、读取守卫、工具权限、事件循环与工具事件的实时性、扩展加载、思考行、会话落盘与删除、目录选择、工作区隔离、设置弹窗、菜单浮层、logo 资产、窗口 GPU 渲染、mock 模型的完整回合
bun run screenshot out.png      # 启动真实窗口并截图（GPUIX_BACKGROUND=1，不抢焦点）
bun scripts/smoke.ts            # 真实窗口里跑完整回合：输入 → 批准 → 写入 → diff 卡片
bun scripts/projects-check.ts   # 真实窗口里加项目：坏路径报错，好路径切换并重新扫描
bun scripts/session-delete-check.ts # 真实窗口里点一下会话行右边的垃圾桶：确认删除 + 真的少一行
bun scripts/rows-check.ts       # 一列工具行：行距、收起行下面有没有多余的线、长参数有没有用省略号（按像素量）
bun scripts/extension-check.ts  # 扩展插件：加载 → 进工具表 → 真调一次 → 结果回给模型（需要联网）
bun scripts/settings-check.ts   # 真实窗口里开设置：切分节、截图、Esc 关闭
bun scripts/menu-check.ts       # 截两张弹窗菜单，看圆角有没有露出深色背景
bun scripts/make-icon.tsx       # 由 assets/logo.svg 生成 png/ico，并校验四角是透明的
bun scripts/png-pixels.ts a.png # 解一个 PNG 像素出来（没有图像库时的放大镜）
bun scripts/window-controls.ts  # 对着真实 OS 窗口验证拖动 / 最大化 / 还原 / 最小化 / 关闭
bun scripts/drag-probe.ts       # 拖动出问题时用：把每次移动的坐标与应用到的位置打出来
bun scripts/window-probe.ts <pid> # 一个进程到底开了哪些窗口（类名 + 尺寸），找多出来的窗口时用
bun scripts/binary-check.ts     # 启动 dist/a-da-core.exe，等待首帧并截图
```

测试通过 `bunfig.toml` 的 preload（`scripts/test-preload.ts`）把 `A_DA_HOME` 指到临时
目录、并设上 `A_DA_NO_DIALOG=1`：前者保证用例不往你真实的 `~/.a-da` 里写会话，后者
保证它们不会真的弹出一个模态目录选择框把自动化挂在那里。

界面测试用 `createTestRoot()`，不开窗口、不抢键盘，走的是和线上完全相同的
`GpuixView` / `build_element()` / 绘制路径；`scripts/` 里的几个脚本则是对着**真实窗口**
和**真实进程**的检查，两种都要跑。

一个要注意的地方：`bun test` 的这些文件共用同一个 `store` 单例，文件之间怎么交错执行
由运行器决定。所以一个用例不能断言「另一个文件留下的状态」——要断言就断言自己这一段
代码能决定的文字（设置弹窗的用例原先断言会话标题，就因此在两次运行之间翻过面）。

另一个坑在 `scripts/` 那边，写脚本时容易踩：会话区是**虚拟列表**，自动化报出来的元素
边界是按 `estimatedItemHeight` 估算的，文字定位器给出的点可能偏几十像素（实测报 625、
实际画在 561），所以要点里面的东西只能自己算坐标；而且**不先移动就发的合成点击在真实
窗口里落不到处理器上**——真人用鼠标本来就是先移过去再按下，脚本里补一次 `mouseMove`
就行。两点都在 `scripts/smoke.ts` 里有注释。

## 已知限制

- 窗口按钮与拖动是 **Windows 专有**（`bun:ffi` 调 user32）。macOS 上按钮不绘制，
  交给系统红绿灯；GPUIX 目前没有跨平台的窗口控制 JS API
- 拖动是**应用自己算的**，不是系统的移动循环：`WM_NCLBUTTONDOWN` + `HTCAPTION`
  会被 GPUI 自己的窗口过程消费掉（`DefWindowProc` 根本收不到），`WM_SYSCOMMAND` /
  `SC_MOVE` 在这台机器上也进不了移动循环（用 `GetGUIThreadInfo` 实测：UI 线程始终不在
  `GUI_INMOVESIZE` 里）。所以改成：按下时记下光标相对窗口的抓取偏移，之后每次指针移动
  算出新位置（`src/platform/win32.ts`）
- 位置按**手势是怎么开始的**决定用哪个坐标：按下时物理左键确实按着（真人拖拽）就用
  光标的屏幕坐标——`SetWindowPos` 由窗口线程异步应用，用事件里的窗口坐标会对着一个
  还没移动过的窗口，正好只走一半距离；按下时没有物理按键（自动化合成事件、触摸/手写笔）
  就用事件坐标，因为此时真实光标与手势无关，让它优先会劫持整段拖拽
- 代价：没有 Aero Snap（只有系统循环能做）；最大化时不能拖动（先点还原）
- 拖动从固定的拖拽区开始（标签页也可以）。这块区域不能有可点子元素：同时监听
  down 与 move 会让 GPUIX 在这棵子树上装 pointer capture，祖先一旦捕获，
  最小化/最大化/关闭的点击就被吞掉了——这三个按钮曾经真的因此失效
- 项目与会话只存在于本次运行：退出后列表清空。会话其实已经写进
  `~/.a-da/sessions/*.jsonl`（一行一条的追加流水），但还没有恢复入口——重启后列表仍会
  清空，磁盘上那份目前只增不读
- **目录选择弹窗是 Windows 专有**，而且用的是 PowerShell 5.1 + `FolderBrowserDialog`
  （旧式树状选择器，不是 Vista 之后那个新对话框；后者要走 COM 的 `IFileOpenDialog`，
  在 FFI 里代价太大）。它靠一个临时进程开窗，所以从点击到弹窗出现大约 1.4 秒——这段
  时间侧边栏的行会显示「正在打开目录选择…」。弹窗以主窗口为 owner，所以不会跑到应用
  后面。非 Windows 上没有这个弹窗，直接退回到手输路径。要小心的一个坑：应用自己是
  GUI 子系统、没有控制台，而 `powershell.exe` 是控制台程序，**不带上 `windowsHide`
  （`CREATE_NO_WINDOW`）就会多出一个黑框**陪着弹窗一起出现——`dialog.test.ts` 钉住了
  这个选项
- 弹窗菜单是**两层盒子**：GPUIX 的 `<anchored>` 会在子元素后面强制刷一层不透明的
  `#1A1A1A`（防止延迟层透出页面），如果圆角卡片就是那一层，四个角会切开这层深色——
  暗色主题里看不出来，浅色主题下一眼就是"黑角"。所以浮层那层是**方角纯色**，
  圆角 / 描边 / 阴影都在内层卡片上（`src/ui/controls.tsx` 有说明，`controls.test.ts`
  钉住了这个不变量，`scripts/menu-check.ts` 用来肉眼复核）
- 没有语法树感知的编辑，`edit_file` 是精确字符串替换
- 命令超时后只杀 shell 本身，不保证杀掉整棵进程树
