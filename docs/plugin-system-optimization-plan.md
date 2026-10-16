# 插件系统优化计划（基于 pi 借鉴）

## 0. 背景与目标

当前插件系统已具备：
- Rust 端口化插件 HookRuntime
- TS 脚本宿主（needs / budget / timeout / isolation）
- 单源 ToolDescriptor 与能力 gating
- 基础点位：ToolCall / TodoUpdate / PluginToggle / ThreadCreate / Turn

目标是把 a_da 从“可执行脚本宿主”推进到“可编程 Agent 扩展平台”，同时保留 Rust 带来的安全、预算、契约治理优势。

## 1. 核心原则

1. **安全优先**：所有插件能力继续受 `capabilities`、budget、needs、sandbox 约束。
2. **点位先行**：runtime 未接火前，先完成 HookRuntime wiring。
3. **rich context 次之**：先提供消息/工具/system prompt 上下文，再扩展高级 rewrite。
4. **生态能力最后**：命令、快捷键、renderer、provider 作为体验扩展。

## 2. Phase 1： production 接火

### 2.1 HookRuntime 构造与注册

- 在组合根创建 `HookRuntime::new(script_host, budget)`。
- 扫描 global/workspace 插件并 collect_plugin_tools。
- 使用 handles 注册 hooks。
- `AgentRuntime::with_hooks(...)`。

### 2.2 Runtime 观测

- 将 hook observation 转成 diagnostic/event sink。
- UI 能看到插件 hook 的点位、耗时、拒绝原因。

### 验收

- beforeTurn/afterTurn 在真实 ada-coding 会话 firing。
- 一个 global/plugin script 能注册 hook 并被调用。
- cargo tests + xtask verify-wiring 通过。

## 3. Phase 2：rich context & turn 语义

### 3.1 beforeTurn payload

- 传入 `step`、`messages` summary、`effectiveTools`、`systemPromptBytes`、`threadId`、`kind`。
- 支持 Deny / AbortTurn / Replace（args only 先行）。

### 3.2 afterTurn payload

- 传入 `step`、`assistant` 文本长度、`toolResults` summary、`effectiveToolNames`、`llmDurationMs`、`toolsDurationMs`。
- 支持 Note_append。

### 3.3 effectiveToolNames

- 从实际执行的工具列表生成，不只 beforeTurn  declared tools。

### 验收

- 一个 beforeTurn observe hook 能读取上下文。
- 一个 afterTurn note 能追加信息。
- golden/conformance tests 更新并通过。

## 4. Phase 3：点位扩展

### 4.1 Session/Agent level

- beforeAgentStart / afterAgentEnd
- beforeSubagentStart / afterSubagentEnd
- onThreadSwitch / beforeThreadDelete / afterThreadDelete

### 4.2 Compaction/Model

- beforeCompaction / afterCompaction
- beforePersist / afterCheckpoint
- beforeLlmRequest / afterLlmResponse
- beforeSystemPrompt

### 4.3 Tool/Approval

- beforeApproval / afterApproval
- afterToolCall semantics richer

### 验收

- 每个点位都有 HookLevel mapping 与 allows policy
- 运行时在对应链路调用 point
- point_not_implemented tests 更新

## 5. Phase 4：context 与 provider 多态

### 5.1 Context transform

- 支持 extension context event（消息转换）
- beforeProviderRequest / beforeProviderHeaders / afterProviderResponse

### 5.2 Provider/Model registration

- late binding provider register/unregister
- extension context actions stubs → real actions

### 5.3 Command/flag/renderer

- registerCommand, registerShortcut, registerFlag
- message renderer / entry renderer

### 验收

- extension 可注册新 provider/model
- slash command 可执行
- message render 可自定义

## 6. Phase 5：开发者体验与.reload

### 6.1 Discovery & Manifest

- project/global/configured extensions
- package.json pi manifest 支持

### 6.2 Cache & Reload

- module cache by cwd/generation
- clear/reload API

### 6.3 Source Maps

- runtime error stack 映射到 TS 原始位置

### 验收

- 重新加载扩展不重启进程
- 错误信息指向原始 .ts 文件/行

## 7. 风险与缓解

| 风险 | 缓解 |
|---|---|
| hook 滥用全局状态 | INV-8 + app home injection in tests |
| 指针 Escalation | allows() matrix + privilege tests |
| Replace bypass | structural checks + risk downgrade |
| Plugin 调用链误 firing | event sink diag + tests |
| 生态 API 过多 | 分阶段：P1-P3 优先，P4/P5 可选 |

## 8. 交付标准

- `cargo test --workspace -- --test-threads=1`
- `cargo xtask verify-wiring`
- `cargo xtask verify-spec`
- `bun run typecheck`
- 增量 conformance tests for hooks/context/provider
- docs/unfinished-features.md 对应条目划掉

## 8.5. 借鉴 pi 的战略优先级

借鉴 pi 的成功经验，插件系统应按以下顺序推进，避免一次性过载：

1. 先把 `beforeTurn` / `afterTurn` 变成真实可编程点位；
2. 再开放 `session` / `agent` / `provider` / `context` 事件；
3. 最后补 `command` / `UI` / `provider ecology`。

### 原则

- 先把“点有真实运行时载体”做扎实，再扩大事件面。
- 再把“ richer payload + context transform”做好，避免 point 空转。
- 最后补 command/UI/provider 生态，避免过早复杂化。

## 9. 下一步建议

1. 先实现 Phase 1 + Phase 2 中的 beforeTurn/afterTurn 上下文传输。
2. 每个 phase 结束时更新 wiring plan 与 feature catalog。
3. 保持 Rust 安全优势，不 просто复制 pi 的所有交互复杂度。
