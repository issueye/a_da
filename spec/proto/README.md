# `spec/proto` —— 线协议登记表（唯一手写源）

本目录是 **INV-11「协议单源」**的落地处：线协议方法、事件主题与错误码在这里登记，
其他副本（Rust 常量、client-ts 常量、dispatch 臂）都是**手写副本 + 机械校验**。

> **为什么不生成**：见 `docs/agent-base-design.md` §6.2 的决策记录（W5-T1）。
> 一句话：方法集合变更频率极低，而生成器本身也要被校验——**防漂移靠校验，不靠生成**。

## 文件

| 文件 | 层 | 说明 |
|---|---|---|
| `base.json` | L0 + L1 | 基座协议：核心通信与调度方法、系统能力位、标准错误码。**全仓唯一** |
| `<product>.ext.json` | L2 | 产品扩展（当前：`ada-coding.ext.json`）。**不得重新定义 base 方法** |

## 命名规则

- **方法名**：`<域>.<动作>`，小驼峰动作。域与动作都用小写字母开头。
  - 例：`thread.start`、`config.setProvider`、`change.revertCheckpoint`、`plugin.builtinCatalog`
  - 嵌套域用点分隔，**最多三段**：`plugin.capabilities.set`
- **事件主题**：`evt.<域>.<事件>`，与命令方法**分开命名空间**，不得与方法名冲突。
  - 例：`evt.state.snapshot`、`evt.message.delta`、`evt.card.updated`
- **Rust 常量**：方法名的大写 + 下划线形式，放在 `crates/agent-proto/src/methods.rs`。
  - `thread.start` → `THREAD_START`；`config.setProvider` → `CONFIG_SET_PROVIDER`
  - 事件常量前缀 `EVT_`：`evt.state.snapshot` → `EVT_STATE_SNAPSHOT`
- **client-ts 常量**：与 Rust 常量**同名同值**，放在 `crates/agent-proto/client-ts/methods.ts`。

## 产品扩展的命名空间约定

产品扩展方法用 `x.<product>.*` 别名，并声明 `namespace`：

```json
{
  "product": "ada-coding",
  "namespace": "x.coding",
  "methods": [
    { "name": "workspace.add", "alias": "x.coding.workspace.add", "kind": "command", "since": "1.0", "description": "添加工作区" }
  ]
}
```

- `name` 是**线上真实方法名**（Rust 常量与 dispatch 臂用它）；
- `alias` 是**带命名空间的别名**，用于文档与跨产品辨识；
- `namespace` 必须是 `x.<短名>`，`alias` 必须等于 `<namespace>.<name>`。

## 字段

| 字段 | 必填 | 含义 |
|---|---|---|
| `name` | ✅ | 线上方法名（唯一） |
| `kind` | ✅ | `command` / `event` / `server_request`。`compat` 会断言 base 与 ext 的同名方法 `kind` 一致 |
| `since` | ✅ | 引入的协议版本（当前一律 `1.0`） |
| `description` | ✅ | 一句话说明（中文），面向界面与文档 |
| `alias` | ext 必填 | 见上 |

## 校验（三对副本，全部双向）

| # | 副本对 | 校验位置 |
|---|---|---|
| 1 | `spec/proto/*.json` ↔ `methods.rs` | `cargo test -p agent-proto` 的 `test_spec_consistency_across_rust_and_json_spec`（方法名集合相等） |
| 2 | `methods.rs` ↔ `dispatch.rs` | `cargo xtask verify-wiring`（每个方法都有 `match` 臂；无孤儿臂） |
| 3 | `methods.rs` ↔ `client-ts/methods.ts` | 同 #1（**`name → value` 映射双向相等**） |
| 4 | base ↔ ext | `cargo xtask compat`（ext 不得重定义 base 方法；`kind` 一致） |

**改一个方法要同步的地方**（AGENTS.md §16）：`spec/proto/*.json` → `methods.rs`（常量 +
`ALL_METHODS`）→ `dispatch.rs`（`match` 臂）→ `client-ts/methods.ts`。
**删一个方法同样四处**——W5-T2 删 `workspace.set` / `config.update` 时就是四处一起改，
漏任何一处都会被上表某一列抓住。

## 相关

- 设计口径：`docs/agent-base-design.md` §6
- 协议总览与客户端用法：`docs/protocol/README.md`
- 产品声明（工具包/能力位）：`products/<id>/agent.spec.json`（与线协议是**两件事**：
  前者是装配期声明，后者是运行期线协议）
