// @ada-needs fs
//
// 提问插件的后端：模型工具 `ask_user` + 界面动作 `list` / `answer`。
//
// 为什么提问不走核心：核心只保留引擎/事件/工具端口；"等人回答"是一种插件行为。
// 工具把问题写进工作区状态文件并阻塞轮询，界面插槽通过只读动作看到待答项，
// 作答动作写回答案后工具被唤醒。跨调用没有共享内存，文件是唯一的交接面。

const fs = require("node:fs");

const STORE_DIR = ".ada/ask-user";
const STORE_FILE = "questions.json";
const MAX_QUESTIONS = 10;
const MAX_QUESTION_CHARS = 600;
const WAIT_TIMEOUT_MS = 30 * 60 * 1000;
const POLL_MS = 250;

const metadata = {
  name: "ask-user",
  description: "向用户提问：模型工具 + 界面插槽 + 后端动作",
  tools: { count: 1, names: ["ask_user"] },
  systemPrompt: { count: 0, names: [] },
  hooks: { count: 0, names: [] },
  actions: { count: 2, names: ["list", "answer"] },
  routes: { count: 0, names: [] },
};

function workspaceOf(ctx) {
  if (ctx && typeof ctx.workspace === "string" && ctx.workspace) return ctx.workspace;
  const globalCtx = globalThis.__ada_tool_context;
  if (globalCtx && typeof globalCtx.workspace === "string" && globalCtx.workspace) {
    return globalCtx.workspace;
  }
  return process.cwd();
}

function storePath(root) {
  return path.join(root, STORE_DIR, STORE_FILE);
}

function loadQuestions(root) {
  const file = storePath(root);
  try {
    if (!fs.existsSync(file)) return [];
    const parsed = JSON.parse(fs.readFileSync(file, "utf-8"));
    return parsed && Array.isArray(parsed.questions) ? parsed.questions : [];
  } catch (_) {
    return [];
  }
}

function saveQuestions(root, questions) {
  const file = storePath(root);
  const dir = path.dirname(file);
  fs.mkdirSync(dir, { recursive: true });
  // 直接用目标文件：Boa 的 node:fs 当前没有 renameSync，临时文件会让状态永远读不到。
  fs.writeFileSync(file, JSON.stringify({ questions: questions }, null, 2));
}

function normalizeChoices(raw) {
  const list = Array.isArray(raw) ? raw : [];
  return list.map(function (choice, index) {
    const value = choice && typeof choice === "object" ? choice : {};
    const id =
      typeof value.id === "string" && value.id.trim()
        ? value.id.trim()
        : "c" + String(index + 1);
    return {
      id: id,
      label: typeof value.label === "string" && value.label.trim() ? value.label : id,
      description:
        typeof value.description === "string" && value.description.trim()
          ? value.description
          : undefined,
    };
  });
}

function prepareQuestions(args) {
  const input = args && typeof args === "object" ? args : {};
  const rawList =
    Array.isArray(input.questions) && input.questions.length > 0
      ? input.questions
      : [input];
  if (rawList.length > MAX_QUESTIONS) {
    throw new Error("一次最多问 " + MAX_QUESTIONS + " 个问题");
  }
  return rawList.map(function (raw, index) {
    const value = raw && typeof raw === "object" ? raw : {};
    const text = typeof value.question === "string" ? value.question.trim() : "";
    if (!text) throw new Error("第 " + String(index + 1) + " 个问题缺少 question 文本");
    if (Array.from(text).length > MAX_QUESTION_CHARS) {
      throw new Error("第 " + String(index + 1) + " 个问题超过 " + MAX_QUESTION_CHARS + " 字");
    }
    return {
      id:
        typeof value.id === "string" && value.id.trim()
          ? value.id.trim()
          : "q" + String(index + 1),
      question: text,
      choices: normalizeChoices(value.choices),
      allowText: value.allow_text !== false && value.allowText !== false,
      multiSelect: value.multi_select === true || value.multiSelect === true,
    };
  });
}

function labelOf(item, id) {
  const hit = (item.choices || []).find(function (choice) {
    return choice.id === id;
  });
  return hit ? hit.label : id;
}

function renderAnswer(question, answer) {
  const lines = [];
  for (let i = 0; i < question.items.length; i += 1) {
    const item = question.items[i];
    const hit =
      (answer.answers || []).find(function (entry) {
        return entry.id === item.id;
      }) || {};
    const picks = Array.isArray(hit.choices) && hit.choices.length > 0
      ? hit.choices
      : hit.choice
        ? [hit.choice]
        : [];
    const picked = picks.map(function (id) {
      return labelOf(item, id);
    });
    const text = typeof hit.text === "string" ? hit.text.trim() : "";
    let rendered = "未作答";
    if (picked.length > 0 && text) rendered = "选择 [" + picked.join("、") + "]，说明：" + text;
    else if (picked.length > 0) rendered = "选择 [" + picked.join("、") + "]";
    else if (text) rendered = text;
    if (question.items.length === 1) {
      lines.push(rendered);
    } else {
      lines.push(String(i + 1) + ". " + item.question + " → " + rendered);
    }
  }
  const body = question.items.length === 1 ? "：" + lines[0] : "\n" + lines.join("\n");
  return "答复已记录（来源: user）" + body;
}

function collectAnswer(question, input) {
  const answers = question.items.map(function (item) {
    const choice = input["choice__" + item.id];
    const choices = input["choices__" + item.id];
    const text = input["text__" + item.id];
    return {
      id: item.id,
      choice: typeof choice === "string" && choice ? choice : undefined,
      choices: Array.isArray(choices) ? choices : choices ? [choices] : undefined,
      text: typeof text === "string" && text.trim() ? text.trim() : undefined,
    };
  });
  return { answeredBy: "user", answeredAt: Date.now(), answers: answers };
}

function uniqueId() {
  return (
    "q_" +
    Date.now().toString(36) +
    "_" +
    Math.random().toString(36).slice(2, 8)
  );
}

export default function (ada) {
  ada.metadata = metadata;

  ada.registerTool({
    name: "ask_user",
    label: "向用户提问",
    description:
      "在交互界面向用户发起单选/多选或文本提问，并等待答复。一次可问多个问题。",
    access: "read",
    longRunning: true,
    parameters: {
      type: "object",
      properties: {
        question: { type: "string", description: "问题文本（单题）" },
        choices: {
          type: "array",
          description: "选项列表（可选）",
          items: {
            type: "object",
            properties: {
              id: { type: "string" },
              label: { type: "string" },
              description: { type: "string" },
            },
            required: ["label"],
          },
        },
        multi_select: { type: "boolean", description: "是否允许多选" },
        allow_text: { type: "boolean", description: "是否允许文本补充" },
        questions: {
          type: "array",
          description: "一次问多题（最多 10 题）",
          items: {
            type: "object",
            properties: {
              id: { type: "string" },
              question: { type: "string" },
              choices: { type: "array" },
              multi_select: { type: "boolean" },
              allow_text: { type: "boolean" },
            },
            required: ["question"],
          },
        },
      },
      anyOf: [{ required: ["question"] }, { required: ["questions"] }],
    },
    async execute(_callId, args) {
      let items;
      try {
        items = prepareQuestions(args);
      } catch (error) {
        return { ok: false, output: String(error && error.message ? error.message : error) };
      }

      const globalCtx = globalThis.__ada_tool_context || {};
      const root = workspaceOf(globalCtx);
      const threadId = typeof globalCtx.threadId === "string" ? globalCtx.threadId : "";
      const id = uniqueId();
      const question = {
        id: id,
        threadId: threadId,
        createdAt: Date.now(),
        items: items,
        answer: null,
      };
      saveQuestions(root, loadQuestions(root).concat([question]));

      const deadline = Date.now() + WAIT_TIMEOUT_MS;
      while (Date.now() < deadline) {
        await new Promise(function (resolve) {
          setTimeout(resolve, POLL_MS);
        });
        const current = loadQuestions(root).find(function (entry) {
          return entry.id === id;
        });
        if (!current) {
          return { ok: false, output: "提问记录已被删除，无法继续等待答复。" };
        }
        if (current.answer) {
          return { ok: true, output: renderAnswer(question, current.answer) };
        }
      }
      return { ok: false, output: "等待用户回答超时（30 分钟）。请基于现有上下文继续。" };
    },
  });

  ada.registerAction({
    name: "list",
    label: "读取待答提问",
    description: "读取当前会话尚未回答的提问",
    mode: "read",
    async execute(input, ctx) {
      const root = workspaceOf(ctx);
      const threadId = input && typeof input.threadId === "string" ? input.threadId : "";
      const cutoff = Date.now() - WAIT_TIMEOUT_MS;
      const questions = loadQuestions(root).filter(function (question) {
        return (
          !question.answer &&
          Number(question.createdAt) >= cutoff &&
          (!threadId || question.threadId === threadId)
        );
      });
      return { questions: questions };
    },
  });

  ada.registerAction({
    name: "answer",
    label: "提交回答",
    description: "把界面上的作答写回提问记录",
    mode: "write",
    async execute(input, ctx) {
      const root = workspaceOf(ctx);
      const questionId = input && typeof input.question_id === "string" ? input.question_id : "";
      if (!questionId) return { ok: false, error: "缺少 question_id" };
      const questions = loadQuestions(root);
      const question = questions.find(function (entry) {
        return entry.id === questionId;
      });
      if (!question) return { ok: false, error: "提问已失效（可能已超时或已作答）" };
      if (question.answer) return { ok: false, error: "这条提问已经回答过了" };
      if (Date.now() - Number(question.createdAt) > WAIT_TIMEOUT_MS) {
        return { ok: false, error: "提问已超时（30 分钟）" };
      }
      question.answer = collectAnswer(question, input || {});
      saveQuestions(root, questions);
      return { ok: true, data: { id: questionId } };
    },
  });
}
