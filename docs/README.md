# a_da 文档索引

本目录按**功能域**组织，描述仓库的**当前**实现：

> 纯 Rust 微内核（`crates/agent-base`）+ 节点/桥接/网关三层（`agent-node` / `agent-rpc` / `agent-gateway`）
> + 内置 SSR 网页界面（`agent-web`）+ Electron 桌面壳（`electron/` + `desktop-ui/`）
> + TS 插件运行时（`crates/ts-engine`）。

**历史文档已清理**（2026-10-18）：TS/Bun 时代的实现（Bun 宿主 + GPUIX 客户端 + TS 侧 agent）、
已被取代的设计、以及未落地的提案都已从 `docs/` 移除。需要考古请从 git 历史取回
（`git log --diff-filter=D --name-only -- docs/`）。
**本次整理的完整记录（删了什么、为什么、门禁怎么同步）见 [plans/wiring.md](plans/wiring.md) §13.37。**

---

## 一、按任务找文档

| 你想解决什么 | 先读 |
|---|---|
| 系统**现在能做什么** | [quality/feature-catalog.md](quality/feature-catalog.md) |
| 系统是**怎么分层**的、有哪些端口 | [architecture/kernel-design.md](architecture/kernel-design.md) |
| 客户端和宿主**怎么通信**（WS 线协议 + HTTP API 的统一接口文档） | [protocol/README.md](protocol/README.md) |
| **插件怎么写**、有哪些钩子点位 | [plugins/README.md](plugins/README.md) |
| 插件契约的精确形状（工厂 API / 类型） | [plugins/extension-factory.md](plugins/extension-factory.md)、[plugins/extension-api.d.ts](plugins/extension-api.d.ts) |
| **网关**、工作区模型与瘦客户端 | [gateway/README.md](gateway/README.md) |
| **ssr-h 合并进网关**的执行计划（未开工） | [gateway/PLAN-merge-ssr-h.md](gateway/PLAN-merge-ssr-h.md) |
| **插件库平台**（发布/下载插件 ZIP） | [../crates/plugin-hub/README.md](../crates/plugin-hub/README.md) |
| **还没做什么**（功能缺口，带证据） | [quality/unfinished-features.md](quality/unfinished-features.md) |
| 历史计划与决策入口（正文已精简） | [plans/README.md](plans/README.md) |
| 技能格式规范（Agent Skills） | [SKILL_SPEC.md](SKILL_SPEC.md) |
| 接手必须先知道的坑 | [AGENTS.md](../AGENTS.md) → [architecture/conventions.md](architecture/conventions.md) |

---

## 二、文件清单与权威性

| 文件 | 定位 | 权威性 |
|---|---|---|
| [architecture/kernel-design.md](architecture/kernel-design.md) | 内核与产品线的设计口径：分层、端口、协议单源、装配、交付 | **设计唯一口径** |
| [architecture/conventions.md](architecture/conventions.md) | `AGENTS.md` 的深度约定全文（§1–§18），接手须知 | **约定唯一口径** |
| [quality/feature-catalog.md](quality/feature-catalog.md) | 已实现功能清单（按功能域，逐条给出代码落点） | 现状清单 |
| [quality/unfinished-features.md](quality/unfinished-features.md) | **功能缺口**清单（证据 + 最小实现路径 + 状态） | 缺口唯一口径 |
| [plans/README.md](plans/README.md) | 历史计划入口（wiring / milestones 已精简） | 历史归档 |
| [protocol/README.md](protocol/README.md) | **接口文档（唯一）**：WS JSON-RPC 帧、HTTP `/api/*` 端点表、事件与增量帧契约、审批通道、错误码与 HTTP 映射、Phase 3 缺口 | **接口唯一口径** |
| [plugins/README.md](plugins/README.md) | 插件平台现状：发现规则、贡献模型、钩子点位与分级、能力开关、管理面、排错 | 插件现状总览 |
| [plugins/extension-factory.md](plugins/extension-factory.md) | Extension Factory 契约：包形态、`metadata`、注册 API、动作分级 | **插件契约唯一口径** |
| [plugins/extension-api.d.ts](plugins/extension-api.d.ts) | 插件作者用的 TypeScript 类型声明 | 类型契约 |
| [gateway/README.md](gateway/README.md) | 网关的三合一职责、工作区门户、极限瘦客户端、SSR 托管 | 设计（含更正记录） |
| [../crates/agent-web/README.md](../crates/agent-web/README.md) | SSR 网页界面：页面 / SSE / 前端插件 / 入口防护 | 界面现状总览 |
| [gateway/PLAN-merge-ssr-h.md](gateway/PLAN-merge-ssr-h.md) | ssr-h 合并进网关的批次、任务卡、验收与回退 | **执行计划（未开工）** |
| `app.png` | 界面截图（README 引用） | 示意 |

**机器可读的真源不在本目录**：

| 事实 | 唯一真源 |
|---|---|
| 协议方法 | [`spec/proto/`](../spec/proto/README.md) |
| 工具描述符与元数据 | `crates/agent-toolkit/src/registry.rs` |
| 钩子点位与动作分级 | `crates/agent-base/src/ports/hooks.rs` |
| 诊断点位词汇表 | `crates/agent-base/src/domain/diag.rs` |
| 产品声明 | `products/*/agent.spec.json`、`products/*/modes/*.agent.spec.json` |

---

## 三、维护规矩

1. **改功能要同批改文档**。至少要动：[quality/feature-catalog.md](quality/feature-catalog.md)
   （实现了什么）与 [quality/unfinished-features.md](quality/unfinished-features.md)（还缺什么）。
2. **文档不许把归档布局当现行路径**：`cargo xtask verify-docs` 会扫全仓 markdown，
   命中**归档时代的布局字样**（TS 时期的源码目录、入口文件与测试命令）而**不在白名单**里就会红。
   白名单在 `tools/xtask/src/gates.rs` 的 `DOC_ALLOW`，**逐项写明理由**——加白名单要写清楚为什么。
3. **协议文档要与登记表同步**：`verify-docs` 断言 [`spec/proto/README.md`](../spec/proto/README.md)
   与 [protocol/README.md](protocol/README.md) 存在，且后者提到 `ALL_EVENTS` 里的**每个事件主题**。
4. **新增文档请放进对应的功能域目录**（`architecture/` `protocol/` `plugins/` `gateway/`
   `quality/` `plans/`），并**在本索引里登记**。散落在 `docs/` 根下的新文件不会被门禁发现，
   但会破坏"按功能域组织"这条约定。
5. 文档内的相对链接请用**相对本文件的路径**（如 `../plans/README.md`），
   移动文档时至少检查一遍出链与入链（`git grep -n "<旧文件名>"`）。
