# 基于网关的工作区选择与极限瘦客户端架构设计

> 本文原名 `docs/gateway-workspace-and-thin-client-design.md`，文档整理时移入 `gateway/`
> 并成为本目录的 README。现状（网关三合一职责、一工作区一进程、实例回收、
> `gateway.*` 协议面）以 [`../quality/feature-catalog.md`](../quality/feature-catalog.md)
> 与 [`../protocol/README.md`](../protocol/README.md) 为准；本文给的是**设计意图与取舍**。
>
> 状态：**设计草案（v1.0）**  
> 目标：**彻底废弃桌面端原生文件/文件夹选择弹窗，将工作区与文件目录浏览能力完全收拢至网关（`ada-gateway`），桌面端彻底退化为极限瘦客户端（Extreme Thin Client），实现桌面与 Web 浏览器 100% 同构。**

---

## 0. 一页结论

1. **废弃 Native Dialog**：系统不保留任何操作系统原生文件/文件夹选择弹窗（如 Electron Dialog、Win32/Cocoa/GTK 原生选择器）。所有目录树与文件选择统一通过前端内置的 Web 原生模态框 [`FilePicker.tsx`](../../desktop-ui/src/components/FilePicker.tsx) 实现。
2. **网关作为文件系统与工作区门户**：工作区根目录选择、目录树遍历、历史工作区记录统一由网关管理面（`gateway.fs.*` 与 `gateway.workspaces.*`）提供。无工作区、冷启动、多实例切换及远程访问场景下，客户端均直面网关获取统一视图。
3. **桌面端蜕化为极限瘦壳**：桌面宿主（[`electron`](../../electron)）彻底移除业务逻辑、文件操作以及复杂的多子进程管理，仅保留最基础的原生窗口外壳（Frame、无边框缩放、拖拽）和 WebView 宿主，以及针对本地单机部署的网关守护自启能力。
4. **全平台 100% 同构体验**：无论运行于 Electron 桌面壳、普通 Chrome/Safari 浏览器、平板设备还是远程云端开发环境，UI 交互、工作区管理和文件浏览行为逐像素一致，没有任何平台特化分支。

```
┌────────────────────────────────────────────────────────────────────────┐
│                        前端 UI（React 100% 同构）                        │
│   • 会话与消息渲染                                                     │
│   • 统一 Web 原生 FilePicker 模态框（选择工作区 / 引用文件）           │
│   • 统一 JSON-RPC 客户端（ws-client）                                  │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ WebSocket (JSON-RPC)
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│                       ada-gateway 网关（平台面）                        │
│   • 管理面：gateway.fs.* (roots, list, mkdir)                          │
│   • 管理面：gateway.workspaces.* (recent, remove, switch)              │
│   • 实例注册与路由：supervisor + registry (ensure_agent / attach)       │
│   • 鉴权与安全沙箱：路径遍历防护 + Scoped Token                         │
└───────────────────┬────────────────────────────────┬───────────────────┘
                    │ 内部转发                       │ 内部转发
                    ▼                                ▼
       ┌────────────────────────┐       ┌────────────────────────┐
       │   ada-coding 实例      │       │      ada-pm 实例       │
       │ (工作区 A 代码沙箱与引擎) │       │ (项目管理分派引擎)      │
       └────────────────────────┘       └────────────────────────┘
```

> ⚠️ **架构更正（2026-10-11）：PM 已从"第二个产品"降级为 `ada-coding` 的**模式预设**。**
>
> 上面这张图是 S6 的设计草图，**从未如实反映实现**：实际一直是"一个 `ada-coding` 进程
> 装配两套引擎"（Coding + PM），而不是两个进程。降级后差异更小——PM 只是
> `products/ada-coding/modes/pm.agent.spec.json` 这份声明（persona + 不含 `fs`/`command`
> 的工具包 + 委派配置），与产品共用同一个二进制、同一处装配函数（`build_injection`）。
> 会话是 coding 还是 pm 由 **`mode`** 表达，`agent_id` 恒为 `ada-coding`。
> 详见 [AGENTS.md](../../AGENTS.md) 的代码结构一节。

---

## 1. 角色契约与架构拓扑

### 1.1 整体拓扑流程

```mermaid
sequenceDiagram
    autonumber
    actor User as 用户
    participant UI as 前端 Web UI (Electron / Browser)
    participant GW as ada-gateway 网关
    participant AG as ada-coding / ada-pm 实例

    User->>UI: 打开应用 (冷启动)
    UI->>GW: 建立 WebSocket 连接 (gateway.info / gateway.status)
    
    rect rgb(30, 41, 59)
    Note over User,GW: 工作区与文件选择阶段 (全部由网关服务提供)
    User->>UI: 点击 "添加工作区" / "切换项目"
    UI->>GW: 调用 gateway.fs.roots
    GW-->>UI: 返回驱动器与常用目录 (C:\, D:\, Home, Desktop)
    User->>UI: 点击展开某个目录
    UI->>GW: 调用 gateway.fs.listDirectory(path, directoriesOnly=true)
    GW-->>UI: 返回子目录清单 (过滤系统级受限目录)
    User->>UI: 选定目录 "E:/projects/my-app"
    end

    rect rgb(15, 23, 42)
    Note over UI,AG: 实例调度与透传接入阶段
    UI->>GW: 调用 gateway.attach(workspace="E:/projects/my-app")
    GW->>AG: 按需拉起或复用该工作区的 Agent 实例
    GW-->>UI: 绑定成功，后续会话级协议由网关透明透传至 Agent
    end

    UI->>AG: 正常发起会话对话与代码生成
```

### 1.2 角色契约矩阵

| 模块 | 所属组件 | 职责范围 | 绝对禁止事项 |
|---|---|---|---|
| **桌面瘦宿主** | `electron` | • 无边框窗口控制（最小化/最大化/关闭/拖拽）<br>• WebView 容器载入<br>• 本地守护伴侣（单机时静默起网关） | ❌ 禁止调用 OS 文件对话框<br>❌ 禁止直接读写会话/工作区文件<br>❌ 禁止直接管理 Coding 多进程 |
| **网关平台** | `ada-gateway` | • 提供 `gateway.fs.*` 目录遍历与磁盘探测<br>• 提供 `gateway.workspaces.*` 历史记录管理<br>• 路径沙箱安全判定（防 `../` 穿透）<br>• Agent 实例生命周期管理（Supervisor）与透传（Relay） | ❌ 严禁内嵌多轮 Agent 执行循环（INV-1）<br>❌ 严禁缓存会话消息或工具状态（INV-8） |
| **Agent 实例** | `ada-coding`（PM 是它的**模式预设**，不是第二个产品） | • 工作区沙箱内的多轮执行（`run_turn`）<br>• 编写代码、执行终端命令、检查点管理<br>• 工作区内文件 Base64 预览（`fs.read_base64`） | ❌ 禁止暴露全局磁盘根目录选择接口<br>❌ 禁止自行跨工作区穿透读盘 |
| **Web 前端** | `desktop-ui` | • 沉浸式内置 `FilePicker` 交互渲染<br>• 全局会话状态机管理<br>• 主题、命令面板、工作区树展示 | ❌ 禁止使用任何宿主原生文件对话框接口 |

---

## 2. 网关工作区与文件系统协议规范（RPC Specification）

网关在原有管理面方法（`gateway.info`, `gateway.attach`, `gateway.delegate` 等）基础上，正式扩充 `gateway.fs.*` 与 `gateway.workspaces.*` 命名空间。

所有方法均遵循 JSON-RPC 2.0 规范，通过统一的网关 WebSocket 链路传输。

### 2.1 磁盘根节点查询：`gateway.fs.roots`

获取当前宿主机可用的物理磁盘、挂载点以及常用快捷目录。

- **请求**：
```json
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "method": "gateway.fs.roots",
  "params": {}
}
```

- **响应**：
```json
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "result": {
    "roots": [
      { "id": "home", "name": "用户主目录", "path": "C:\\Users\\User", "kind": "home" },
      { "id": "desktop", "name": "桌面", "path": "C:\\Users\\User\\Desktop", "kind": "folder" },
      { "id": "drive-c", "name": "本地磁盘 (C:)", "path": "C:\\", "kind": "drive" },
      { "id": "drive-e", "name": "工作盘 (E:)", "path": "E:\\", "kind": "drive" }
    ]
  }
}
```

### 2.2 目录与文件列表浏览：`gateway.fs.listDirectory`

浏览指定路径下的子项。支持仅浏览目录（选择工作区场景）或浏览全量文件（选择引用附件场景）。

- **请求**：
```json
{
  "jsonrpc": "2.0",
  "id": "req-2",
  "method": "gateway.fs.listDirectory",
  "params": {
    "path": "E:\\codes\\rust_projects",
    "directoriesOnly": true,
    "showHidden": false,
    "limit": 500
  }
}
```

- **响应**：
```json
{
  "jsonrpc": "2.0",
  "id": "req-2",
  "result": {
    "path": "E:/codes/rust_projects",
    "parent": "E:/codes",
    "isRoot": false,
    "dirs": [
      { "name": "a_da", "path": "E:/codes/rust_projects/a_da", "isDir": true, "modifiedAt": 1728551200000 },
      { "name": "other-repo", "path": "E:/codes/rust_projects/other-repo", "isDir": true, "modifiedAt": 1728442000000 }
    ],
    "files": [],
    "total": 2,
    "truncated": false
  }
}
```

### 2.3 新建文件夹：`gateway.fs.makeDirectory`

在选定目录下创建新的子目录（用于用户在新建项目时直接在模态框内建立新目录）。

- **请求**：
```json
{
  "jsonrpc": "2.0",
  "id": "req-3",
  "method": "gateway.fs.makeDirectory",
  "params": {
    "parentPath": "E:\\codes\\rust_projects",
    "folderName": "my-new-app"
  }
}
```

- **响应**：
```json
{
  "jsonrpc": "2.0",
  "id": "req-3",
  "result": {
    "path": "E:/codes/rust_projects/my-new-app",
    "created": true
  }
}
```

### 2.4 历史工作区查询与管理：`gateway.workspaces.list` / `gateway.workspaces.remove`

网关持久化维护用户常用与最近访问的工作区清单（位于网关数据目录 `~/.a-da/workspaces.json`）。

- **请求 (`gateway.workspaces.list`)**：
```json
{
  "jsonrpc": "2.0",
  "id": "req-4",
  "method": "gateway.workspaces.list",
  "params": {}
}
```

- **响应**：
```json
{
  "jsonrpc": "2.0",
  "id": "req-4",
  "result": {
    "workspaces": [
      {
        "workspace": "E:/codes/rust_projects/a_da",
        "name": "a_da",
        "lastAccessedAt": 1728551200000,
        "isOnline": true,
        "activeProduct": "ada-coding"
      }
    ],
    "activeWorkspace": "E:/codes/rust_projects/a_da"
  }
}
```

- **请求 (`gateway.workspaces.remove`)**：
```json
{
  "jsonrpc": "2.0",
  "id": "req-5",
  "method": "gateway.workspaces.remove",
  "params": {
    "workspace": "E:/codes/rust_projects/other-repo",
    "killInstance": true
  }
}
```

---

## 2.5 网页访问：网关直接托管前端产物（S8）

目标是一句话：**一个 `ada-gateway` 二进制 + 一个端口 = 整站**，浏览器打开就能用，
不再需要另起 `vite preview`/`http.server` 之类的静态服务器。

```bash
npm --prefix desktop-ui run build          # 产出 desktop-ui/dist
ada-gateway --port 52353 --workspace E:/proj
# 浏览器打开 http://127.0.0.1:52353/
```

### 2.5.1 路由：axum 统一

页面与 WebSocket 在**同一个 axum Router** 里，只有两条路由：

| 请求 | 去向 |
|---|---|
| `GET /rpc` + `Upgrade: websocket` | `relay::ws_handler`：先鉴权（Origin / token）再升级 |
| `GET /`、`GET /assets/*` | `static_ui::static_handler`：前端产物（目录 → `index.html`） |
| `GET /rpc`（无升级头） | `426 Upgrade Required` |
| `POST` 等非 GET/HEAD | `405 Method Not Allowed` |

组装点是 `crates/agent-gateway/src/router.rs` 的 `router(state)`：**生产与集成测试共用同一份**，
测试不会因为"另抄了一段分流"而只覆盖到其中一条路。

为什么不再手写 HTTP：早先为了在一个端口上分流，用的是"`TcpStream::peek` 读一眼 → 非升级请求
自己写状态行"。它踩过一个很实的坑——`peek` 不消费请求字节，响应写完直接 `close` 时接收缓冲里
还有未读数据，内核发的是 **RST 而不是 FIN**，客户端拿到 `ConnectionReset`（Windows `os error 10054`，
表现为页面偶尔白屏）。换成 axum 后这条路径**结构性消失**：响应与连接都由 hyper 收尾，
没有"绕过它自己 close"的地方。

### 2.5.2 鉴权位置

- **静态资源不需要 token**——页面只是 JS/CSS，不含任何用户数据；要 token 才能打开"输入 token 的页面"是死锁。
- **`/rpc` 在升级之前鉴权**：Origin 不合法 → `403`，token 不对 → `401`（都是普通 HTTP 状态码，
  浏览器与 curl 直接看得到）。未认证连接只能调 `gateway.info` / `gateway.pair`，
  而且**不绑定 agent 实例**——先升级再鉴权，会让一个不带 token 的连接也能触发 `ensure_agent` 起进程。
- 回环来源（`http://127.0.0.1:*` / `http://localhost:*`）默认放行；网页访问要带凭据时用
  `?token=<t>`（前端从 `location.search` 读取）。

对外部署仍然遵守第 4 节的硬规则：非回环必须 `--token` + `--allow-origin`，
且明文 `ws://` 要 `--allow-plaintext` 显式承认。

### 2.5.3 产物目录怎么找

`--ui-dir <p>` → `A_DA_UI_DIR` → 可执行文件旁 `ui/` → 可执行文件旁 `../desktop-ui/dist` →
当前目录 `desktop-ui/dist`。**只认含 `index.html` 的目录**；一个都找不到时网关照常启动，
根路径返回 `503` 并说明怎么修——网页访问是可选能力，不做成启动前置条件。

### 2.5.4 桌面端拉起的网关同样可访问

桌面端不再用 `--port 0`（随机端口，浏览器根本猜不到），改为**固定 52353**：
被占用时网关顺延（52353 → 52363），实际端口从 READY 行里拿。每次启动都写一份发现文件
`<A_DA_HOME>/gateway_endpoint.json`：

```json
{
  "port": 52353,
  "pid": 1234,
  "workspace": "E:/proj",
  "url": "http://127.0.0.1:52353/",
  "rpc": "ws://127.0.0.1:52353/rpc",
  "authRequired": false
}
```

**不写 token**：凭据只从启动参数或 `gateway.pair` 配对码来，不落到磁盘上等人捡。
界面「进程管理 → 浏览器打开」会拼出带 token 的地址（token 从 `core_url` 原样取，不二次编码）。

打包形态还有一个前提：网关是普通 Rust 进程，**读不到 asar 里的文件**——所以
`electron-builder.json` 把 `desktop-ui/dist` 额外复制到 `resources/bin/ui`，
Electron 再通过 `--ui-dir` 显式传给网关（见 `electron/main.ts` 的 `findUiDir`）。

### 2.5.5 路径安全

三道闸，任一不过一律 403：段级黑名单（`..`、反斜杠、冒号、NUL，含百分号编码后的形态）、
`canonicalize` 后必须仍在站点根内（拦符号链接穿透）、必须指向普通文件。
不存在的路径按"有扩展名 = 404 / 无扩展名 = 回 index.html（客户端路由兜底）"区分——
否则 `/assets/missing.js` 会拿到一份 HTML，报出来的是 MIME 错误而不是真正的原因。
错误页里的路径会先解码再 **HTML 转义**（`/<img src=x>.js` 不能变成标签）。

---

## 3. 安全防护与沙箱控制模型（Security & Sandboxing）

将文件系统目录浏览开放给网关 RPC 后，必须严格遵守最小权限与失败安全原则：

```
                ┌────────────────────────────────┐
                │ 客户端接入请求 (路径 path)      │
                └───────────────┬────────────────┘
                                │
                                ▼
            ┌────────────────────────────────────────┐
            │ 1. 规范化路径 (Canonicalization)       │  剔除 ../ ./ 冗余斜杠
            └───────────────────┬────────────────────┘
                                │
                                ▼
            ┌────────────────────────────────────────┐
            │ 2. 网关鉴权态判断 (Auth Check)         │
            └───────────────┬───────────────┬────────┘
                            │               │
       (本地回环 127.0.0.1 模式)       (远程 0.0.0.0 暴露模式)
                            │               │
                            ▼               ▼
                 ┌─────────────────┐  ┌───────────────────────────────┐
                 │ 系统常规目录访问 │  │ 必须强制 Scoped Token 校验     │
                 │ 仅阻断系统保留区 │  │ 路径必须属于 --allowed-roots │
                 └─────────────────┘  └───────────────────────────────┘
```

1. **路径规范化（Anti-Path Traversal）**：
   - 网关层收到任何路径字符串必须首先通过 `dunce::canonicalize` 或规范化算法展开符号链接；
   - 彻底拒绝相对路径穿透攻击（禁止包含逃逸父级的 `..` 序列）。
2. **远程与局域网网络隔离**：
   - 当网关以非回环模式启动（`--host 0.0.0.0` 或指定外网 IP）时，强制启用鉴权拦截：未认证或匿名连接**绝对禁止**调用 `gateway.fs.*`；
   - 远程模式可配置 `--allowed-roots <path_list>`（例如仅限 `/workspace` 或 `D:/projects`），任何超出该白名单前缀的 `listDirectory` 请求一律返回 `PermissionDenied` 错误码。
3. **系统保留与敏感路径过滤**：
   - 默认禁止遍历操作系统极敏感目录（如 Windows 下的 `C:\Windows\System32`、Linux 下的 `/etc/shadow`、`/proc`、`/sys` 等）；
   - 在目录检索响应中默认隐藏 `.git`, `node_modules`, `target` 等庞大构建产物内部文件遍历（提升传输性能与安全性）。

---
