# 审批面板（approval）

待批准工具卡片的统一插件。**策略与闸门仍在核心**；本包只负责“待批准状态 + 卡片呈现”。

## 组成

```text
plugin.json          # frontend.slots = thread.approval；frontend.actions.pending
index.ts             # beforeApproval / afterApproval → .ada/approval-state.json
slot.tsx             # 审批卡片；表单提交核心 /approvals/{callId}/decision
static/approval.css  # 插件作用域内的统计行样式
```

## 数据流

```text
核心 HostApprovalGate.needs_approval
  → beforeApproval 钩子
      → 后端插件写 .ada/approval-state.json（按 threadId 记 pending）
  → AgentEventBody::ApprovalRequested
      → props.data.approvals（核心投影，决定"现在有哪些待批准"）
      → 后端 pending action 补 args/waited + 已批准/拒绝统计
      → thread.approval 插槽渲染
  → 用户点批准/拒绝
      → POST /approvals/{callId}/decision（核心）
  → afterApproval 钩子
      → 后端删除 pending、累计统计
```

前端只在**核心 `data.approvals` 有该项**时渲染；后端状态里残留的 callId 不会显示，
避免 abort/timeout 后出现幽灵审批卡。后端停用/未就绪时直接用核心投影。

## 边界

- 插件**不能**改变 `needs_approval` / `decide`；审批策略仍由 `approval-guard`
  （内置插件）与 `HostApprovalGate` 决定；
- 批准/拒绝只走核心路由，插件文本/按钮不能伪造结果；
- `data.approvals` 没有参数原文之外的敏感信息；`args` 已截断；
- 状态文件是运行时产物，已加入仓库 `.gitignore`。

## 相关

- [unified-package.md](../../docs/plugins/unified-package.md)
- [PLUGIN-DATA.md](../../crates/agent-web/docs/PLUGIN-DATA.md) §2.13
- [agent-web/README.md](../../crates/agent-web/README.md) 插槽列表
