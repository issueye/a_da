// @ada-needs fs
//
// 审批面板的后端：只维护“待批准 + 已批准/拒绝计数”，不参与策略判定。
// 真正的审批闸门仍在核心 HostApprovalGate；本插件不能改变 needs_approval/decide。
//
// 状态按 threadId 写在用户工作区 `.ada/approval-state.json`：
// - beforeApproval：记录 callId/tool/args/isWrite；
// - afterApproval：删除 pending，累计 approved/rejected；
// - pending action：给前端审批插槽读取。

const STATE_PATH = ".ada/approval-state.json";

const metadata = {
  name: "approval",
  description: "审批面板：beforeApproval/afterApproval → 工作区状态 → pending action",
  tools: { count: 0, names: [] },
  systemPrompt: { count: 0, names: [] },
  hooks: { count: 2, names: ["beforeApproval", "afterApproval"] },
  actions: { count: 1, names: ["pending"] },
};

function emptyState() {
  return { version: 1, threads: {} };
}

function readState() {
  const fs = require("node:fs");
  try {
    if (!fs.existsSync(STATE_PATH)) return emptyState();
    const parsed = JSON.parse(fs.readFileSync(STATE_PATH, "utf-8"));
    if (!parsed || typeof parsed !== "object") return emptyState();
    if (!parsed.threads || typeof parsed.threads !== "object") parsed.threads = {};
    return parsed;
  } catch (_) {
    return emptyState();
  }
}

function writeState(state) {
  const fs = require("node:fs");
  fs.mkdirSync(".ada", { recursive: true });
  fs.writeFileSync(STATE_PATH, JSON.stringify(state));
}

function threadEntry(state, threadId) {
  if (!state.threads[threadId] || typeof state.threads[threadId] !== "object") {
    state.threads[threadId] = { pending: {}, approved: 0, rejected: 0 };
  }
  const entry = state.threads[threadId];
  if (!entry.pending || typeof entry.pending !== "object") entry.pending = {};
  if (typeof entry.approved !== "number") entry.approved = 0;
  if (typeof entry.rejected !== "number") entry.rejected = 0;
  return entry;
}

function pendingItems(entry, now) {
  const items = [];
  const keys = Object.keys(entry.pending);
  for (let i = 0; i < keys.length; i += 1) {
    const item = entry.pending[keys[i]];
    if (!item || typeof item !== "object") continue;
    const askedAt = Number(item.askedAt) || 0;
    const seconds = askedAt > 0 && now > askedAt
      ? Math.max(0, Math.floor((now - askedAt) / 1000))
      : 0;
    items.push({
      callId: String(item.callId || keys[i]),
      tool: String(item.tool || "工具"),
      args: item.args === undefined ? "" : item.args,
      isWrite: item.isWrite === true,
      waited: seconds > 0 ? "已等待 " + seconds + "s" : "",
    });
  }
  return items;
}

export default function (ada) {
  ada.metadata = metadata;

  ada.on("beforeApproval", async (event, ctx) => {
    if (!event || !ctx || !ctx.threadId || !ctx.callId) return;
    const state = readState();
    const entry = threadEntry(state, ctx.threadId);
    entry.pending[ctx.callId] = {
      callId: ctx.callId,
      tool: typeof event.tool === "string" ? event.tool : "工具",
      args: event.args === undefined ? {} : event.args,
      isWrite: event.isWrite === true,
      askedAt: Date.now(),
    };
    writeState(state);
    // 控制面：只观察，不拦下、不改写。
  }, { actions: ["observe"] });

  ada.on("afterApproval", async (event, ctx) => {
    if (!event || !ctx || !ctx.threadId || !ctx.callId) return;
    const state = readState();
    const entry = threadEntry(state, ctx.threadId);
    delete entry.pending[ctx.callId];
    if (event.approved === true) entry.approved += 1;
    else entry.rejected += 1;
    writeState(state);
  }, { actions: ["observe"] });

  ada.registerAction({
    name: "pending",
    label: "读取待批准列表",
    description: "读取某个会话由 beforeApproval 记录的待批准工具调用",
    mode: "read",
    async execute(input, ctx) {
      const threadId = input && typeof input.threadId === "string" ? input.threadId : "";
      if (!threadId) return { ok: true, data: { items: [], approved: 0, rejected: 0 } };
      const entry = threadEntry(readState(), threadId);
      const now = ctx && typeof ctx.now === "number" ? ctx.now : Date.now();
      return {
        ok: true,
        data: {
          items: pendingItems(entry, now),
          approved: entry.approved,
          rejected: entry.rejected,
        },
      };
    },
  });
}
