# archive/ —— 冻结的 TS 实现（不再是参考设计，也不是构建/测试的一部分）

## 这是什么

`archive/ts-legacy/` 是 **2026 年之前的 TypeScript 实现整体**：Bun + GPUIX 客户端、TS 侧 agent 主循环与宿主、
`store` 状态层、工具与内置插件、以及配套的真窗口检查脚本。

它被整体搬进这个文件夹，原因见 [docs/agent-base-plan.md](../docs/agent-base-plan.md) §5：

- 权威运行时已经是 **Rust**（`agent_core`，默认由 `host-bootstrap` / `src-tauri` 拉起），TS 那套是**第二份实现**；
- 两套实现的契约（协议方法、钩子、工具元数据）已经在漂移（实测：Rust 76 个方法常量 vs TS 90 个键，
  10 个 Rust-only 方法、2 个声明了却没有 dispatch 臂的常量）；
- 维护"等价性"的成本高于删掉第二份实现。

**从归档之日起**：它不是参考设计，不被任何构建/测试/CI 引用，`docs/` 之外不应出现指向它的路径。

## 目录

```
ts-legacy/
  src/           原 src/**（agent、ui、platform、shared/protocol、AgentWindow.tsx、theme.ts…）
  scripts/       原 scripts/**（真窗口检查、构建、插件检查、图标工具链…）
  app.tsx        原单文件双角色入口（UI / --host）
  screenshot.ts  原截图工具
  assets.d.ts    原 *.svg 声明
  env.d.ts       原 *.exe 声明
```

**为什么保留 `src/` 的原始形状**：归档内部的相对 import（`src/ui/main.tsx → ../AgentWindow`、
`src/ui/*.test.tsx → ../agent/store`、`src/agent/…/*.test.ts → ../../../../scripts/test-preload`）
只有在目录形状不变时才成立。压缩成 `agent/`、`ui/`、`gpui/` 会把它们全部打断。

## 怎么临时复活（考古用，不要在主干恢复）

```bash
# 冻结点标签：archive/ts-legacy-final
git worktree add ../ts-legacy archive/ts-legacy-final
cd ../ts-legacy
bun install && bun run link          # link 需要同级 ../gpuix
bun run dev                          # 或 bun app.tsx
```

用完 `git worktree remove ../ts-legacy`。

## 需要读它的两种正当理由

1. **移植一段只有 TS 有实现的逻辑**（例如 decision 引擎的 `decide`/`check_gate`、GPUIX 真窗口自动化）。
   先在设计/计划文档里登记任务，再移植；不要从归档直接 import。
2. **查历史行为**（"当初为什么这么判"）。此时以 `docs/agent-conventions.md` 与 `docs/*-design.md` 的结论为准，
   归档只是证据。

## 不许做的事

- 不许把归档路径写进构建脚本、测试配置、CI、`tsconfig`。
- 不许在主干里"临时 import 一下"——那条路会用两次实现把漂移重新引进来。
- 门禁（`bun tools/verify-archive.ts`）会检查以上两条。
