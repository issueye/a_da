# a_da 后端 Rust + Deno 生态重构架构方案设计

> **文档状态**：方案定稿 / 待实施  
> **制定日期**：2026-10-01  
> **核心目标**：将当前基于 Bun + TypeScript 的 Agent 后端（Host 角色）全量重构为基于 **Rust 原生运行时**；同时集成 **`deno_core` + `deno_node` + `oxc_transform`** 构建轻量级 V8 扩展沙箱，无缝保留第三方 TypeScript 插件生态并提供常用的 Node.js 内置库支持。

---

## 一、 重构背景与核心收益

当前桌面客户端架构由两部分组成：
1. **UI 渲染端**：基于 GPUIX（React 风格的 GPU 原生渲染层）；
2. **Agent 后端（Host）**：基于 Bun 运行时，负责会话落盘、大模型通信、工具执行、进程看护与插件加载。

双方已通过纯粹的 **JSON-RPC 2.0（基于 WebSocket）** 解耦（详见 [`src/shared/protocol`](file:///E:/codes/rust_projects/a_da/src/shared/protocol)）。这为后端全量替换为 Rust 提供了坚实的架构基础。

### 核心预期收益

| 评估维度 | 当前现状（Bun + jiti） | 重构后（Rust + Deno 生态） |
| :--- | :--- | :--- |
| **空闲内存占用 (Host)** | 120MB ~ 250MB | **40MB ~ 60MB**（下降 70%+） |
| **高频对话峰值内存** | 300MB ~ 800MB+（JSC 内存池） | **60MB ~ 100MB**（严格 V8 Heap Limit 配额） |
| **启动耗时 (Cold Start)** | 180ms ~ 350ms | **< 15ms** |
| **垃圾回收停顿** | 存在高频 GC 垃圾与堆膨胀 | 核心逻辑无 GC，仅插件占用受控 V8 隔离区 |
| **进程管理健壮性** | Node 子进程树（软管理） | Windows 原生 Job Object（硬核防孤儿进程） |
| **插件 TS 执行能力** | 依赖 jiti 内存编译 | `oxc_transform` 微秒级擦除 + V8 极速执行 |
| **Node.js 常用库兼容** | 原生完整 Node 环境 | `deno_node` 官方支持绝大多数内置库 |

---

## 二、 整体技术选型矩阵

```mermaid
flowchart TD
    subgraph UI 渲染端
        UI["GPUIX Native Window (保留现有实现)"]
    end

    subgraph Rust Agent Host (a-da-host)
        WS["WebSocket Server (tokio-tungstenite)"]
        RPC["JSON-RPC 2.0 派发器 (serde / serde_json)"]
        STATE["AgentState (无锁并发 / 状态管理)"]
        LOOP["AgentLoop (多轮状态机 / SSE 解析器)"]
        SESS["SessionManager (追加式 JSONL 持久化)"]
        
        subgraph Native Builtin Tools (纯 Rust 原生实现)
            T_FS["文件操作 (tokio::fs / 沙箱检查)"]
            T_CMD["进程命令 (Windows Job Object)"]
            T_DIFF["Diff / Patch 计算 (similar / tree-sitter)"]
            T_GIT["Git 深度洞察 (git2 / CLI)"]
        end

        subgraph Deno 插件隔离沙箱 (Plugin Sandbox)
            OXC["oxc_transform (TypeScript 语法擦除)"]
            DENO_CORE["deno_core (V8 JsRuntime)"]
            DENO_NODE["deno_node (node:fs, node:child_process 等兼容层)"]
            DENO_WEB["deno_web (fetch, URL, Stream, TextEncoder)"]
            OPS["Rust Ops 桥接 (registerTool, onEvent, registerHooks)"]
        end
    end

    UI <== "WebSocket JSON-RPC 2.0 (端口 + Token 认证)" ==> WS
    WS --> RPC --> STATE
    STATE <--> LOOP
    LOOP --> T_FS
    LOOP --> T_CMD
    LOOP --> T_DIFF
    LOOP --> T_GIT
    LOOP <--> OPS
    
    OPS <--> DENO_CORE
    OXC --> DENO_CORE
    DENO_NODE --> DENO_CORE
    DENO_WEB --> DENO_CORE
```

### 关键依赖库选型

1. **核心异步与通信**：
   - `tokio` (full)：异步运行时底座。
   - `tokio-tungstenite`：高性能、轻量 WebSocket 服务端。
   - `serde` / `serde_json`：JSON-RPC 请求与响应的高性能序列化/反序列化。
2. **大模型与网络**：
   - `reqwest` (rustls-tls, stream)：调用大模型 OpenAI 兼容接口，解析 SSE 流式文本。
   - `eventsource-stream`：处理 Server-Sent Events 流。
3. **原生工具与系统级能力**：
   - `windows-sys` / `windows`：在 Windows 下创建 Windows Job Object，绑定子进程树，防止任务崩溃时残留后台孤儿进程。
   - `similar`：纯 Rust 实现的统一 Diff/Patch 计算库。
4. **插件与 JS/TS 引擎**：
   - `deno_core`：轻量 V8 引擎封装与 Op 扩展系统。
   - `deno_node`：提供 `node:fs`、`node:path`、`node:child_process`、`node:events`、`node:util`、`node:buffer` 等官方兼容层。
   - `deno_web`：提供标准 `fetch`、`URL`、`EventTarget` 等 Web API。
   - `oxc_transform` / `oxc_allocator`：超高速在内存中完成 TypeScript 类型剥离（Type Stripping），直接输出纯 ES 模块。

---

## 三、 详细技术架构设计

### 1. 协议层平替（100% 对齐现有规范）

后端暴露的 WebSocket 接口严格实现 [`src/shared/protocol`](file:///E:/codes/rust_projects/a_da/src/shared/protocol) 中定义的契约：

- **认证门禁**：连接握手时验证 `token`，握手前仅接受 `session.initialize`。
- **快照推送 (`evt.state.snapshot`)**：连接成功后首先推送一次 `ClientSnapshot`，随后在状态变更时合帧推送。
- **命令处理**：
  - `thread.send`：用户输入与任务排队。
  - `thread.abort`：中止当前轮次。
  - `approval.decide`：审批结果下发。
  - `fs.list` / `fs.roots` / `fs.mkdir`：为前端文件选择器提供服务。

### 2. TypeScript 动态执行引擎（Type Stripping 模块加载器）

传统方案中动态加载 TS 往往需要将编译产物落地磁盘，或运行繁重的 TypeScript Compiler。  
本设计采用 **内存级微秒转译（Memory-only Transpilation）**：

```rust
use deno_core::error::AnyError;
use deno_core::{ModuleLoader, ModuleSpecifier, ModuleType, ModuleSource, ModuleSourceCode};
use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;
use oxc_transform::{TransformOptions, Transformer};

pub struct TsModuleLoader;

impl ModuleLoader for TsModuleLoader {
    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleSpecifier>,
        _is_dyn_import: bool,
        _requested_module_type: deno_core::RequestedModuleType,
    ) -> deno_core::ModuleLoadResponse {
        let specifier = module_specifier.clone();
        
        let fut = async move {
            let path = specifier.to_file_path()
                .map_err(|_| AnyError::msg("非法文件路径"))?;
            let source_text = tokio::fs::read_to_string(&path).await?;
            
            // 仅对 .ts / .tsx / .mts 进行动态语法擦除
            let is_ts = path.extension().map_or(false, |ext| ext == "ts" || ext == "tsx" || ext == "mts");
            
            let final_code = if is_ts {
                let allocator = Allocator::default();
                let source_type = SourceType::from_path(&path).unwrap_or_default();
                let ret = Parser::new(&allocator, &source_text, source_type).parse();
                
                let mut program = ret.program;
                let transform_options = TransformOptions::default();
                let transformer = Transformer::new(&allocator, &path, source_type, transform_options);
                transformer.build(&mut program);
                
                oxc_codegen::CodeGenerator::new().build(&program).code
            } else {
                source_text
            };

            Ok(ModuleSource::new(
                ModuleType::JavaScript,
                ModuleSourceCode::String(final_code.into()),
                &specifier,
            ))
        };

        deno_core::ModuleLoadResponse::Async(Box::pin(fut))
    }
}
```

### 3. Node.js 兼容层集成（`deno_node`）

在创建 `JsRuntime` 时，通过 Extension 机制装载 `deno_node`：

```rust
use deno_core::{JsRuntime, RuntimeOptions};

pub fn create_plugin_runtime() -> Result<JsRuntime, AnyError> {
    let runtime = JsRuntime::new(RuntimeOptions {
        module_loader: Some(std::rc::Rc::new(TsModuleLoader)),
        extensions: vec![
            // 1. Web 标准 API (fetch, url, console 等)
            deno_webidl::deno_webidl::init_ops_and_esm(),
            deno_url::deno_url::init_ops_and_esm(),
            deno_web::deno_web::init_ops_and_esm::<deno_web::BlobStore>(Default::default(), None),
            deno_fetch::deno_fetch::init_ops_and_esm::<deno_fetch::DefaultHttpClient>(Default::default()),
            
            // 2. 底层 I/O 与文件系统
            deno_io::deno_io::init_ops_and_esm(Default::default()),
            deno_fs::deno_fs::init_ops_and_esm::<deno_fs::RealFs>(Default::default()),
            
            // 3. Node.js 兼容扩展 (提供 node:fs, node:child_process, node:path 等)
            deno_node::deno_node::init_ops_and_esm::<deno_fs::RealFs>(None),
            
            // 4. a_da 扩展宿主 API (注册工具与钩子)
            init_a_da_host_ops::init_ops_and_esm(),
        ],
        ..Default::default()
    });
    
    Ok(runtime)
}
```

### 4. 插件宿主 Ops 桥接（`ExtensionContext` 对齐）

为了让现有遵循 [`src/agent/tools/loader.ts`](file:///E:/codes/rust_projects/a_da/src/agent/tools/loader.ts#L59) 的第三方插件能够开箱即用，Rust 宿主通过 `#[op2]` 注入上下文接口：

```rust
#[deno_core::op2]
fn op_ada_register_tool(
    state: &mut deno_core::OpState,
    #[string] name: String,
    #[string] description: String,
    #[serde] parameters: serde_json::Value,
    #[global] callback: deno_core::v8::Global<deno_core::v8::Function>,
) -> Result<(), AnyError> {
    let registry = state.borrow_mut::<RustToolRegistry>();
    registry.register_js_tool(name, description, parameters, callback);
    Ok(())
}

#[deno_core::op2(async)]
async fn op_ada_call_host_hook(
    state: std::rc::Rc<std::cell::RefCell<deno_core::OpState>>,
    #[string] hook_name: String,
    #[serde] payload: serde_json::Value,
) -> Result<serde_json::Value, AnyError> {
    // 异步流转到 Rust 侧的 Hook Pipeline
}
```

---

## 四、 分阶段实施路线（Milestones）

为确保平稳迁移且不破坏现有桌面客户端，项目采取**双后端并行推进策略**（通过环境变量 `A_DA_HOST_BIN` 指定）：

```
M0: 骨架与协议验证 ──→ M1: 原生核心与工具 ──→ M2: 大模型多轮循环 ──→ M3: Deno 插件沙箱 ──→ M4: 全量切流交付
```

### 里程碑 1：M0 —— 协议服务骨架与握手验证
- **交付内容**：
  - 新建 Rust crate `crates/a-da-host`；
  - 实现基于 `tokio-tungstenite` 的 WebSocket 服务；
  - 完整实现 JSON-RPC 2.0 序列化/反序列化与鉴权（`session.initialize`）；
  - 输出首帧快照 `evt.state.snapshot`。
- **验收标准**：UI 客户端运行 `a-da.exe` 启动该 Rust 后端，界面成功画出首帧且状态对齐。

### 里程碑 2：M1 —— 会话持久化与原生内置工具
- **交付内容**：
  - 用 Rust 实现 `SessionManager`（兼容 `~/.a-da/sessions` 下的 JSONL 格式）；
  - 实现纯 Rust 原生工具：`read_file`, `write_file`, `edit_file`, `batch-ops`, `list_files`, `search_files`；
  - 基于 Windows Job Object 实现 `run_command` 与 `run_background`（彻底杜绝孤儿进程）；
  - 实现基于 `similar` 的文件快照与检查点回滚（`CheckpointManager`）。
- **验收标准**：通过现有单测用例对齐工具输出与沙箱路径防越界行为。

### 里程碑 3：M2 —— Agent 核心循环与流式对话
- **交付内容**：
  - 基于 `reqwest` + `tokio` 实现流式 SSE 客户端；
  - 实现多轮对话状态机（`AgentLoop`），对齐思考链（`thinking`）、工具调用分发与审批中断（`ask_user`）；
  - 实现上下文压缩裁剪（`compact`）。
- **验收标准**：可以完成端到端的 AI 编程对话、工具执行与代码写入。

### 里程碑 4：M3 —— Deno 插件沙箱与 TS 动态执行
- **交付内容**：
  - 引入 `deno_core` + `deno_node` + `oxc_transform`；
  - 实现 `TsModuleLoader`，在内存中动态擦除 TS 类型；
  - 实现 `init_a_da_host_ops`，向插件暴露 `ExtensionContext`；
  - 挂载工作区 `.a-da/plugins/` 下的第三方插件。
- **验收标准**：现有基于 TS 编写的外部插件在不需要经过 Node/Bun 的情况下被 Rust 后端动态加载并正常响应调用。

### 里程碑 5：M4 —— 单文件打包与最终交付
- **交付内容**：
  - 更新打包脚本，将编译产物与原生 UI 组合分发；
  - 验证全量基线测试与端到端运行体验。

---

## 五、 风险规避与关键设计考量

1. **V8 预编译二进制下载问题**：
   - `rusty_v8` crate 默认在构建时会从 GitHub Releases 下载对应平台的静态库。为保障 CI/CD 与国内环境稳定构建，配置镜像源（`RUSTY_V8_MIRROR`）或将静态库纳入本地依赖缓存。
2. **V8 内存硬限控制（Heap Limit）**：
   - 在构建 `RuntimeOptions` 时，通过 `create_params` 将 V8 最大堆内存限制为 **64MB**（`set_max_old_generation_size_in_bytes(64 * 1024 * 1024)`），杜绝任何第三方插件代码引起内存泄漏拖垮主进程。
3. **单向数据流与零拷贝**：
   - 充分利用 Rust 的 `Arc<str>` 与 `Bytes`，在读取工作区文件或向模型发送大请求时，避免像 JavaScript 一样反复产生重复的大字符串拷贝。
