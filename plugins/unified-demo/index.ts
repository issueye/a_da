// @ada-needs fs
//
// 统一插件示例的后端入口：一个包同时被前端注册表与 agent 插件装配器发现。
// 动作不是模型工具：它们只由 SSR 的 /ext/unified-demo/backend/* 路由调用。

const metadata = {
  name: "unified-demo",
  description: "统一插件示例：前端插槽 + 后端动作",
  tools: { count: 0, names: [] },
  systemPrompt: { count: 0, names: [] },
  hooks: { count: 0, names: [] },
  actions: { count: 2, names: ["status", "refresh"] },
};

function readState() {
  const fs = require("node:fs");
  const path = ".a-da-unified-demo.json";
  try {
    if (fs.existsSync(path)) {
      const parsed = JSON.parse(fs.readFileSync(path, "utf-8"));
      return {
        count: Number(parsed.count) || 0,
        lastRefresh: parsed.lastRefresh || null,
      };
    }
  } catch (_) {
    // 状态文件损坏就按空状态处理；动作本身仍然可用。
  }
  return { count: 0, lastRefresh: null };
}

function writeState(value) {
  const fs = require("node:fs");
  fs.writeFileSync(".a-da-unified-demo.json", JSON.stringify(value));
}

export default function (ada) {
  ada.metadata = metadata;

  ada.registerAction({
    name: "status",
    label: "读取示例状态",
    description: "读取工作区里的示例状态文件",
    mode: "read",
    async execute(_input, ctx) {
      return {
        ...readState(),
        workspace: ctx.workspace,
        checkedAt: ctx.now,
      };
    },
  });

  ada.registerAction({
    name: "refresh",
    label: "刷新示例状态",
    description: "递增示例计数并写入工作区",
    mode: "write",
    async execute(_input, ctx) {
      const current = readState();
      const next = {
        count: current.count + 1,
        lastRefresh: ctx.now,
      };
      writeState(next);
      return {
        ...next,
        workspace: ctx.workspace,
      };
    },
  });
}
