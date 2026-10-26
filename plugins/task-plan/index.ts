// @ada-needs fs
//
// task-plan 的后端：订阅 afterTodoUpdate，把**已经生效**的任务清单按会话写进
// 工作区 `.ada/task-plan-state.json`；前端通过 `plan` action 读取。
//
// 为什么用 afterTodoUpdate 而不是 beforeToolCall：
// - 事后点位拿到的是工具真正应用后的全量 `todos`（含 beforeTodoUpdate 的 deny/replace 结果）；
// - hook ctx 带 `threadId`，可以按会话隔离；
// - 动作本身不需要知道模型消息或工具卡，只读自己的状态文件。

const STATE_PATH = ".ada/task-plan-state.json";

const metadata = {
  name: "task-plan",
  description: "任务规划卡片的后端状态：afterTodoUpdate → 工作区状态文件 → plan action",
  tools: { count: 0, names: [] },
  systemPrompt: { count: 0, names: [] },
  hooks: { count: 1, names: ["afterTodoUpdate"] },
  actions: { count: 1, names: ["plan"] },
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

function normalizeStatus(entry) {
  if (!entry || typeof entry !== "object") return "pending";
  const raw = typeof entry.status === "string" ? entry.status : "";
  if (raw === "completed" || raw === "done") return "completed";
  if (raw === "in_progress" || raw === "running") return "in_progress";
  if (entry.done === true) return "completed";
  return "pending";
}

function normalizeSteps(raw) {
  if (!Array.isArray(raw)) return null;
  const steps = [];
  for (let i = 0; i < raw.length; i += 1) {
    const entry = raw[i];
    if (!entry || typeof entry !== "object") continue;
    const title = typeof entry.title === "string"
      ? entry.title
      : (typeof entry.text === "string" ? entry.text : "");
    if (!title.trim()) continue;
    steps.push({
      id: typeof entry.id === "string" ? entry.id : "",
      title: title,
      status: normalizeStatus(entry),
    });
  }
  return steps;
}

function buildView(steps) {
  const done = steps.filter((step) => step.status === "completed").length;
  const total = steps.length;
  const activeStep = steps.find((step) => step.status === "in_progress")
    || steps.find((step) => step.status !== "completed");
  const active = activeStep ? activeStep.title : "";
  const allDone = total > 0 && done === total;
  const lines = ["### 任务规划步骤 (" + done + "/" + total + ")"];
  for (let i = 0; i < steps.length; i += 1) {
    lines.push((steps[i].status === "completed" ? "- [x] " : "- [ ] ") + steps[i].title);
  }
  return {
    done: done,
    total: total,
    allDone: allDone,
    active: active,
    notes: "",
    markdown: lines.join("\n"),
    steps: steps,
  };
}

export default function (ada) {
  ada.metadata = metadata;

  ada.on("afterTodoUpdate", async (event, ctx) => {
    const steps = normalizeSteps(event && event.todos);
    if (!steps || !ctx || !ctx.threadId) return;
    const state = readState();
    if (steps.length === 0) {
      delete state.threads[ctx.threadId];
    } else {
      state.threads[ctx.threadId] = { steps: steps };
    }
    writeState(state);
    // 只观察，不拦截、不追加旁注。
  }, { actions: ["observe"] });

  ada.registerAction({
    name: "plan",
    label: "读取当前任务计划",
    description: "读取某个会话由 afterTodoUpdate 维护的任务规划",
    mode: "read",
    async execute(input) {
      const threadId = input && typeof input.threadId === "string" ? input.threadId : "";
      if (!threadId) return { ok: true, data: null };
      const entry = readState().threads[threadId];
      const steps = entry && Array.isArray(entry.steps) ? entry.steps : [];
      if (steps.length === 0) return { ok: true, data: null };
      return { ok: true, data: buildView(steps) };
    },
  });
}
