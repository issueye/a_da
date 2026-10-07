# a_da 全系统功能清单与实现状态总览

> **版本**：v0.5.0  
> **更新时间**：2026-10-07  
> **系统定位**：基于纯 Rust 高性能微内核、支持双角色（GUI 桌面 + Headless 守护进程 + CLI 执行器）的本地 AI 敏捷编码智能体。

---

## 目录

- [一、 整体架构与运行形态](#一-整体架构与运行形态)
- [二、 纯 Rust 核心引擎功能 (agent_core)](#二-纯-rust-核心引擎功能-agent_core)
  - [2.1 AI 决策主循环与流式处理](#21-ai-决策主循环与流式处理)
  - [2.2 核心工具箱与工作区沙箱](#22-核心工具箱与工作区沙箱)
  - [2.3 检查点机制与版本回滚 (Checkpoint)](#23-检查点机制与版本回滚-checkpoint)
  - [2.4 多智能体协同调度 (Subagents)](#24-多智能体协同调度-subagents)
  - [2.5 纯 Rust 插件体系 (Plugins)](#25-纯-rust-插件体系-plugins)
  - [2.6 技能驱动系统 (Skills)](#26-技能驱动系统-skills)
  - [2.7 权限审批与安全拦截 (Approval Guard)](#27-权限审批与安全拦截-approval-guard)
  - [2.8 会话生命周期与自愈存储 (Session)](#28-会话生命周期与自愈存储-session)
  - [2.9 上下文压缩与紧凑化 (Compact)](#29-上下文压缩与紧凑化-compact)
  - [2.10 全双工 JSON-RPC 2.0 服务网关](#210-全双工-json-rpc-20-服务网关)
- [三、 前端交互界面功能 (UI Capabilities)](#三-前端交互界面功能-ui-capabilities)
- [四、 两套 UI 实现对比表 (GPUIX vs Tauri)](#四-两套-ui-实现对比表-gpuix-vs-tauri)
- [五、 CLI 与终端执行生态](#五-cli-与终端执行生态)

---

## 一、 整体架构与运行形态

系统采用**单二进制分发、双角色运行（Single Binary, Dual Role）**架构：

| 模块 / 路径 | 核心定位 | 关键技术栈 |
|---|---|---|
| [`agent_core`](file:///E:/codes/rust_projects/a_da/agent_core) | 纯 Rust 后端微内核，提供全部主循环、工具执行、插件沙箱与持久化 | Rust 2021、Tokio、Reqwest、Tracing |
| [`src-tauri`](file:///E:/codes/rust_projects/a_da/src-tauri) | 现代跨平台桌面宿主，负责窗口管理、进程级双角色分流与端口调度 | Tauri 2、Windows API、Clap |
| [`tauri-ui`](file:///E:/codes/rust_projects/a_da/tauri-ui) | 现代轻量化 Webview 桌面交互前端 | React 19、TypeScript、Tailwind CSS、Vite、Lucide |
| [`src/ui`](file:///E:/codes/rust_projects/a_da/src/ui) | 原生 GPU 硬件加速桌面前端 | GPUIX、React 19、Hermes 字节码引擎 |

### 运行形态矩阵

1. **默认桌面 GUI 模式** (`a-da.exe`)：双击启动，自动以同进程异步方式拉起核心服务并分配系统随机空闲端口（`--port 0`），隐藏 Windows 控制台黑框，实现开箱即用。
2. **Headless 守护进程模式** (`a-da.exe --headless` / `a-da.exe daemon`)：作为纯后台服务常驻运行，对外提供 WebSocket JSON-RPC 服务。
3. **CLI 单次任务执行模式** (`a-da.exe run --workspace <ws> "<prompt>"`)：命令行直接执行单次编码重构或代码审查任务，无需打开任何图形界面。

---

## 二、 纯 Rust 核心引擎功能 (agent_core)

### 2.1 AI 决策主循环与流式处理
- **流式增量生成 (SSE)**：基于 HTTP SSE 实时解析模型返回的数据流，具备网络断流与超时自动重试。
- **深度思考分离与过滤 (`ThinkTagFilter`)**：原生识别并拦截 `<think>...</think>` 标签，将模型推理思考内容与正式正文回复进行结构化隔离，防止思考内容混入后续上下文。
- **工具调用流水线解析**：支持并行多工具调用指令抽取与格式化，兼容 OpenAI 规范的 `tool_calls`。
- **动态 Prompt 组装**：自动组装系统基础规范、AGENTS 约定、当前工作区信息、已激活插件工具定义与技能元数据。

### 2.2 核心工具箱与工作区沙箱
核心工具完全重构为纯 Rust 原生实现，具备“失败安全”原则：

| 工具名称 | 分类 | 功能描述 |
|---|---|---|
| `list_files` | 只读工具 | 递归遍历工作区目录树，内置智能截断与忽略规则（忽略 target、node_modules、.git 等） |
| `read_file` | 只读工具 | 读取指定文本文件内容，支持行数限制与编码容错 |
| `search_files` | 只读工具 | 基于高效文本匹配搜索工作区内代码关键字，返回精准行号与代码片段 |
| `find_symbol` | 只读工具 | 跨文件查找函数、类、结构体、类型等符号声明与引用 |
| `todo` | 只读工具 | 任务规划追踪，用于分步骤拆解任务进度与动态状态维护 |
| `check_task` | 只读工具 | 查询后台命令进程的实时状态与输出日志 |
| `check_subagent`| 只读工具 | 查询子智能体的执行进度与结果报告 |
| `write_file` | 写入工具 | 全量写入或覆盖文件内容，写入前自动创建快照检查点 |
| `edit_file` | 写入工具 | 基于精准代码块定位进行局部替换，严格验证目标文本唯一性并创建快照 |
| `run_command` | 执行工具 | 在工作区沙箱内同步执行终端命令，捕获 stdout/stderr 并设置超时机制 |
| `run_background`| 执行工具 | 后台异步启动长周期命令（如开发服务器、监控进程），返回进程 Task ID |
| `kill_task` | 执行工具 | 强行终止指定后台进程或进程树 |
| `invoke_subagent`| 调度工具 | 委派具有特定专业角色的子智能体独立执行分支任务 |
| `ask_user` | 交互工具 | 暂停当前执行流，向用户发起结构化提问（单选、多选或自由文本输入） |

- **工作区安全沙箱屏障 (`checkWorkspaceSandbox`)**：所有文件与命令操作均进行严格规范化校验，禁止任何相对路径穿越（`..`）或越界访问工作区之外的敏感路径。

### 2.3 检查点机制与版本回滚 (Checkpoint)
- **写前快照**：每次执行 `write_file` 或 `edit_file` 前，自动对目标文件写入前的状态进行快照存储。
- **单卡回滚 (`change.revertCard`)**：支持用户在单条工具卡片上点击“撤销”，仅回滚该次工具修改。
- **逐文件回滚 (`change.revertFile`)**：按文件维度撤销自会话开始以来的全部修改。
- **全局一键还原 (`change.revertAll`)**：一键撤销本会话产生的所有工作区变动。
- **Unified Diff 实时计算**：生成标准的统一 Diff 差异文本，供审查对比。

### 2.4 多智能体协同调度 (Subagents)
- **特化 Profile 隔离**：内置规划（`plan`）、通用编码（`coder`）、深度研究（`researcher`）、代码审查（`reviewer`）等预设子智能体配置。
- **工具权限白名单**：子智能体严格受限于分配的工具白名单（例如规划智能体严格只读防写）。
- **工作区隔离**：支持派生独立工作区镜像，防止子智能体并发任务干扰主工程。
- **父子会话联动与唤醒**：子智能体运行结束或请求指令后，自动上报结果并唤醒父会话主循环。
- **会话只读屏障**：子智能体专属会话具备直接输入防护，防止用户外部输入打乱子智能体自主调度。

### 2.5 纯 Rust 插件体系 (Plugins)
- **内置官方插件**：内置 9 大核心插件（涵盖自动化安全守卫、上下文决策、通知集成等）。
- **插件生命周期钩子**：提供 `beforeToolCall`、`afterToolCall`、`beforeApproval`、`onContextCompact` 等成对拦截点位。
- **细粒度能力矩阵 (Capabilities)**：独立控制插件对网络访问、文件写入、子进程创建的权限边界。
- **持久化配置与密钥存储**：插件具备独立的作用域配置（Workspace / Global）与安全密钥存取。

### 2.6 技能驱动系统 (Skills)
- **标准规范定义**：兼容标准 `SKILL.md` 规范，支持提取 Frontmatter 元数据、触发条件与执行提示词。
- **多层级作用域**：按内置技能、全局用户技能（`~/.a-da/skills`）、项目专有技能（`.a-da/skills`）三级按需装载。
- **动态启停**：支持用户随时在界面或配置中动态启用/禁用具体技能。

### 2.7 权限审批与安全拦截 (Approval Guard)
- **三档审批策略**：
  - `auto`（自动批准）：无感执行所有工具操作。
  - `ask`（每次询问）：凡涉及文件读写、沙箱命令执行等敏感操作，一律拦截并挂起。
  - `readonly`（严格只读）：严禁一切写操作，只允许只读工具执行。
- **超时与兜底策略**：审批请求超时自动作废，防止无界死锁。

### 2.8 会话生命周期与自愈存储 (Session)
- **JSONL 持久化落盘**：会话内容按行序列化存储为 JSONL 文件，保障进程崩溃时的零数据丢失。
- **标题自愈与提取**：首轮交互后自动基于用户意图提炼短标题；损坏或迁移会话时自动自愈补齐元数据。
- **多工作区索引**：按所属工程路径分类归集历史会话。

### 2.9 上下文压缩与紧凑化 (Compact)
- **触发机制**：支持超限自动压缩与用户手动 `/compact` 触发。
- **关键决策保留**：基于模型或启发式算法将已完成步骤、工具输出提炼为紧凑摘要，释放上下文窗口容量。

### 2.10 全双工 JSON-RPC 2.0 服务网关
- **16ms 节流状态广播**：核心状态变动以 16ms 节流频率向所有已连接的 UI 客户端广播快照事件。
- **全方法支持**：提供涵盖会话管理（`thread.*`）、配置管理（`config.*`）、插件（`plugin.*`）、技能（`skill.*`）、文件服务（`fs.*`）、队列（`queue.*`）的 23+ 个标准化 RPC 方法。

---

## 三、 前端交互界面功能 (UI Capabilities)

### 3.1 消息对话流 (Transcript)
- **Markdown 深度渲染**：支持 GFM 表格、代码块行号、内联语法、代码一键复制。
- **紧凑行级工具调用**：紧凑单行排版展示图标、工具名、目标文件与目录细分、执行耗时、状态徽标；点击展开参数与详细输出。
- **结构化思考卡片**：折叠展示推理过程，配备垂直导引线、流式实时微预览、推理持续时间计时。
- **用户消息气泡**：右对齐气泡展示，支持内联编辑并丢弃后续重新发送。
- **轮次底栏遥测 (Turn Stats)**：展示单轮请求耗时、Token 统计（输入、输出、总计）、全文复制。

### 3.2 任务与思考可视化
- **任务规划看板 (`TodoFloatingPanel`)**：悬浮在对话流右上角，提取 `todo` 工具状态，以检查框方式展示待办与完成情况。
- **交互提问卡片 (`QuestionCard`)**：智能体调用 `ask_user` 时在消息流中唤起高亮卡片，支持多选选项与自定义输入框。
- **敏感审批横幅**：针对拦截的写操作工具弹出带有警示标志的批准/拒绝授权条。

### 3.3 输入控制台 (Composer)
- **富文本输入与自动伸缩**：支持根据输入内容自适应调整高度（最高 180px）。
- **附件集成**：
  - 剪贴板图片直接粘贴（`Ctrl+V`）。
  - 本地文件与图片拖拽放入。
  - 图片待发送缩略图胶囊与移除功能。
- **加号综合功能菜单**：
  - 协作模式切换：Code 敏捷编码 / Plan 只读规划 / Create 架构创造。
  - 快捷指令与提及入口。
  - 添加本地图片附件与引用本地文件。
- **控制药丸群**：
  - 模式切换快捷药丸。
  - 模型提供商与参数药丸。
  - 审批权限策略药丸（自动 / 每次询问 / 严格只读）。
  - 思考力度档位药丸（Max / High / Medium / Low）。
- **排队消息浮动面板 (`QueuedMessagesFloatingPanel`)**：展示连续发送排队中的消息，支持提升优先级（置顶）、删除、清空、重新编辑。
- **子智能体专属保护状态**：激活子智能体标签时，输入框自动切换为紫色只读保护面板，显示“子智能体专属执行会话”，提供停止、恢复执行与返回主会话操作。

### 3.4 遥测与上下文洞察
- **底栏实时遥测**：展示对话轮数、累计 Steps、本次请求 Token 数、请求耗时。
- **上下文构成细分 (`ContextUsagePopover`)**：
  - 上下文占用百分比动态指示条（分绿色正常、橙色警戒、红色超限）。
  - 详细细分展示：系统提示词占用、工具定义声明占用、历史对话轮次占用、当前请求占用。
  - 缓存命中率与上下文容量上限标注。

### 3.5 插件中心与扩展控制台
- **插件管理**：
  - 官方内置插件列表查看与一键启停。
  - 工作区插件与全局插件管理。
  - 插件参数配置即时编辑与保存。
  - 插件物理删除。
- **能力矩阵控制台**：细粒度控制网络、文件系统、子进程执行的能力开关。
- **内置工具目录 (Builtin Catalog)**：查看内置工具规范说明。
- **技能管理 (Skills)**：查看所有技能的详细描述、触发规则与状态启停。

### 3.6 供应商与模型配置
- **预设配置一键导入**：内置 OpenAI、DeepSeek、百炼、Moonshot、Ollama、OpenRouter 等官方预设。
- **自定义配置**：Base URL、API Key、模型名称、上下文窗口容量设定。
- **高级连接选项**：多模态图片识别开关、自定义 HTTP 请求头（Custom Headers）。
- **连接连通性即时检测**：即时测试 API Key 与服务连通性。

### 3.7 侧边栏与多工作区树 (Sidebar)
- **多工作区手风琴折叠**：按项目目录归集会话，支持展开/折叠。
- **父子智能体树形层级**：子智能体会话以树形缩进挂载在父会话下方。
- **会话快速搜索**：实时搜索会话标题与子智能体标题。
- **侧边栏快捷展开与折叠**。

### 3.8 多标签页系统 (TabStrip)
- **多会话并行标签**：支持打开多个会话标签页并快速切换。
- **标签关闭与新建**。
- **运行状态指示**：当前标签页存在异步执行时显示脉动加载动画。

---

## 四、 两套 UI 实现对比表 (GPUIX vs Tauri)

| 功能项 | GPUIX 原生版 ([`src/ui`](file:///E:/codes/rust_projects/a_da/src/ui)) | Tauri 版 ([`tauri-ui`](file:///E:/codes/rust_projects/a_da/tauri-ui/src)) | 状态评估 |
|---|---|---|:---:|
| **基础对话流与 Markdown** | 完整实现，GPU 加速渲染 | 完整实现，基于 `marked` 渲染 |  已对齐 |
| **思考折叠与单行工具卡** | 完整实现，微色调与导引线 | 完整实现，高度复刻交互与色彩 |  已对齐 |
| **单卡代码撤销** | 支持 | 支持（调用 `change.revertCard`） |  已对齐 |
| **敏感工具审批横幅** | 支持批准/拒绝 | 支持批准/拒绝 |  已对齐 |
| **交互式提问卡片** | 支持选项点击与自定义答复 | 支持选项点击与自定义答复 |  已对齐 |
| **输入框与加号综合菜单** | 完整实现 | 完整实现（模式/指令/附件） |  已对齐 |
| **排队消息管理浮动面板** | 完整实现 | 完整实现 |  已对齐 |
| **子智能体专属只读保护条**| 完整实现 | 完整实现 |  已对齐 |
| **待办任务看板 (Todo)** | 完整实现 | 完整实现 |  已对齐 |
| **上下文用量细分 Popover**| 完整实现 | 完整实现 |  已对齐 |
| **插件与能力管理弹窗** | 完整实现 ([`PluginsDialog.tsx`](file:///E:/codes/rust_projects/a_da/src/ui/PluginsDialog.tsx) 130KB) | 完整实现 ([`PluginsModal.tsx`](file:///E:/codes/rust_projects/a_da/tauri-ui/src/components/PluginsModal.tsx) 32KB) |  已对齐 |
| **供应商设置弹窗** | 完整实现 | 完整实现 |  已对齐 |
| **侧边栏工作区与父子树** | 完整实现 | 完整实现（手风琴 + 树形索引） |  已对齐 |
| **多标签页 (TabStrip)** | 完整实现 | 完整实现 |  已对齐 |
| **改动审查面板 (ChangesPanel)**| 独立面板，聚合文件 Diff、逐文件恢复与一键重置 | 完整实现 ([`ChangesPanel.tsx`](file:///E:/codes/rust_projects/a_da/tauri-ui/src/components/ChangesPanel.tsx))，支持 Diff 对比、逐文件与全部重置 | 🟢 已对齐 |
| **全局命令面板 (Ctrl+K)** | 独立面板 ([`CommandPalette.tsx`](file:///E:/codes/rust_projects/a_da/src/ui/CommandPalette.tsx))，支持键盘导航执行动作 | 完整实现 ([`CommandPalette.tsx`](file:///E:/codes/rust_projects/a_da/tauri-ui/src/components/CommandPalette.tsx))，支持模糊搜索与键盘动作导航 | 🟢 已对齐 |
| **内置文件/目录选择器** | 独立组件 ([`FilePicker.tsx`](file:///E:/codes/rust_projects/a_da/src/ui/FilePicker.tsx))，跨平台浏览主机文件系统 | 完整实现 ([`FilePicker.tsx`](file:///E:/codes/rust_projects/a_da/tauri-ui/src/components/FilePicker.tsx))，支持盘符切换、新建目录与路径浏览 | 🟢 已对齐 |
| **`@` 提及文件/技能/子智能体** | 键盘输入 `@` 自动呼出模糊联想列表并高亮补全 | 完整实现 ([`MentionMenu.tsx`](file:///E:/codes/rust_projects/a_da/tauri-ui/src/components/MentionMenu.tsx))，支持文件/技能/子体联想补全 | 🟢 已对齐 |
| **`/` 斜杠指令实时联想** | 键盘输入 `/` 自动呼出所有系统指令与 Prompt 模板 | 完整实现 ([`SlashCommandMenu.tsx`](file:///E:/codes/rust_projects/a_da/tauri-ui/src/components/SlashCommandMenu.tsx))，支持指令与 Prompt 模板联想 | 🟢 已对齐 |
| **通信底层调试面板** | 独立面板 ([`DebugPanel.tsx`](file:///E:/codes/rust_projects/a_da/src/ui/DebugPanel.tsx))，监视原始 RPC 与事件 | 完整实现 ([`DebugPanel.tsx`](file:///E:/codes/rust_projects/a_da/tauri-ui/src/components/DebugPanel.tsx))，实时监控 JSON-RPC 报文与一键复制 | 🟢 已对齐 |
| **全局 Toast 轻提示体系** | 全局队列通知宿主 ([`ToastHost.tsx`](file:///E:/codes/rust_projects/a_da/src/ui/ToastHost.tsx)) | 完整实现 ([`ToastHost.tsx`](file:///E:/codes/rust_projects/a_da/tauri-ui/src/components/ToastHost.tsx))，支持队列、各级别通知与撤销操作 | 🟢 已对齐 |
| **全局快捷键注册体系** | 完善的快捷键系统 ([`shortcuts.ts`](file:///E:/codes/rust_projects/a_da/src/ui/shortcuts.ts)) | 完整实现，支持 Ctrl+K/N/W/Shift+C/Shift+D/Shift+X 等快捷键联动 | 🟢 已对齐 |
| **会话内联重命名** | 双击标题内联重命名会话 | 完整实现，侧边栏双击进入内联重命名编辑模式 | 🟢 已对齐 |

---

## 五、 CLI 与终端执行生态

系统在命令行环境下具备完备的自举与调试能力：

```bash
# 1. 默认启动 Tauri GUI 桌面端
a-da.exe

# 2. 指定初始工作区启动 GUI
a-da.exe --workspace "E:/codes/my_project"

# 3. 以纯后台 Headless 守护进程启动（自动分配随机空闲端口）
a-da.exe --headless

# 4. 指定固定端口与认证令牌启动守护进程
a-da.exe daemon --port 52353 --token "my-secret-token" --workspace "."

# 5. CLI 单次自动化编程任务执行
a-da.exe run --workspace "." "审查当前项目所有的 Rust 文件并指出潜在错误"
```
