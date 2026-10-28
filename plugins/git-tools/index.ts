// @ada-needs fs,exec
//
// Git 工具（插件版）。
//
// 原实现是编译在 `agent-toolkit` 里的 `plugin_tools::git`；现在整体搬到沙箱，
// 基座只保留 ts 引擎能力（`fs` 只做目录探测、`exec` 跑子进程）。
//
// 为什么是**两个**工具而不是一个：
//
// - `git`      全部动作（含 add/commit/push/reset 等写仓库动作）——不声明 `access`，
//              宿主按"外部/写"处理（插件不能替它证明作用域）；
// - `git_read` 只读五个查询动作，声明 `access: "read"`，于是只读档、plan 模式、
//              只读子智能体仍然查得到状态。
//
// 把 `git` 整体标成只读等于给 commit/push 开后门（自述只读会免掉写审批），
// 所以只读的部分必须单独成一个工具——这是拆两条的唯一理由。
//
// 🔴 参数一律走 argv、**不经 shell**：提交信息、路径、ref 里到处是用户输入，
// 拼命令行再交给 `cmd /c` 会把 `%VAR%` 展开、把 `&` 当分隔符。

// ── 依赖（懒加载：注入发生在模块执行之前，但没必要在每次装载时都取一遍） ──────
let _deps = null;
function deps() {
  if (_deps === null) {
    _deps = {
      fs: require("node:fs"),
      path: require("node:path"),
      cp: require("node:child_process"),
    };
  }
  return _deps;
}

// ── 常量 ────────────────────────────────────────────────────────────────────
// 支持的动作用**单一真源**：描述符的 enum 与 argv 构造必须同源，否则模型会看到
// 一个"允许但不可执行"的动作。
const ALL_ACTIONS = [
  "status", "diff", "log", "show", "blame",
  "add", "commit", "checkout", "restore", "reset",
  "stash", "branch", "merge", "push", "pull",
];
const READ_ACTIONS = ["status", "diff", "log", "show", "blame"];

// 与基座实现同一个值：整仓 diff 动辄几 MB，一次性塞给模型只会撑爆上下文。
const MAX_GIT_BYTES = 48 * 1024;
const GIT_TIMEOUT_MS = 30000;
const MAX_MESSAGE_CHARS = 2000;

const metadata = {
  name: "git-tools",
  description: "Git 读写工具：只读查询（status/diff/log/show/blame）与写操作集于两枚工具",
  tools: { count: 2, names: ["git", "git_read"] },
  systemPrompt: { count: 0, names: [] },
  events: { count: 0, names: [] },
  actions: { count: 0, names: [] },
  routes: { count: 0, names: [] },
};

// ── 入参校验（失败即拒） ─────────────────────────────────────────────────────
// 宁可让模型改参数重试，也不要把可疑字符串交给 git。
function reject(message) {
  const error = new Error(message);
  error.__adaToolMessage = true;
  return error;
}

/// 仓库内的相对路径：拒绝绝对路径与 `..` 段。
/// `git restore -- ../outside` 会真的改到工作区外的文件，所以这里必须挡。
function repoPath(raw) {
  const p = String(raw === undefined || raw === null ? "" : raw).trim();
  if (!p) throw reject("路径不能为空。");
  if (/[\u0000-\u001f]/.test(p)) throw reject("路径里不能有控制字符（换行等）。");
  const normalized = p.replace(/\\/g, "/");
  const isAbsolute =
    normalized.charAt(0) === "/" || (normalized.length > 1 && normalized.charAt(1) === ":");
  if (isAbsolute) {
    throw reject("`" + p + "` 是绝对路径：git 工具只接受工作区内的相对路径。");
  }
  const segments = normalized.split("/");
  for (let i = 0; i < segments.length; i++) {
    if (segments[i] === "..") throw reject("`" + p + "` 含 `..`：不允许越出工作区。");
  }
  return p;
}

/// ref / 分支 / 远端名：以 `-` 开头一律拒（`--upload-pack=...` 这类"看着像值、
/// 其实是选项"的输入只能靠这条挡住）。
function gitRef(raw) {
  const s = String(raw === undefined || raw === null ? "" : raw).trim();
  if (!s) throw reject("引用名不能为空。");
  if (s.charAt(0) === "-") {
    throw reject("`" + s + "` 以 `-` 开头：引用名不允许像选项一样书写。");
  }
  if (/[\s\u0000-\u001f]/.test(s)) {
    throw reject("`" + s + "` 含空白或控制字符：不是合法的引用名。");
  }
  if (s.length > 200) throw reject("引用名过长（上限 200 字符）。");
  return s;
}

/// 提交信息：允许中文与标点，拒绝换行（`-m` 重复传会改变语义）与超长正文。
function commitMessage(raw) {
  const s = String(raw === undefined || raw === null ? "" : raw).trim();
  if (!s) throw reject("提交信息不能为空。");
  if (s.indexOf("\n") >= 0 || s.indexOf("\r") >= 0) {
    throw reject("提交信息不能含换行：需要多段请分行写成 `；`，或改用 run_command 走 git commit -F。");
  }
  if (s.length > MAX_MESSAGE_CHARS) {
    throw reject("提交信息过长（上限 " + MAX_MESSAGE_CHARS + " 字符）。");
  }
  return s;
}

function str(args, key) {
  const value = args[key];
  if (value === undefined || value === null) return null;
  const text = String(value).trim();
  return text === "" ? null : text;
}

function bool(args, key) {
  return args[key] === true;
}

// ── 参数 → argv（`argv[0]` 之外的部分） ──────────────────────────────────────
function buildArgv(args, allowed) {
  const rawAction = String(args.action === undefined || args.action === null ? "status" : args.action)
    .trim()
    .toLowerCase();
  if (allowed.indexOf(rawAction) < 0) {
    throw reject(
      "未知 git action `" + rawAction + "`（可用：" + allowed.join(" / ") + "）"
    );
  }
  const action = rawAction;

  // 执行目录：多仓库工作区用 `cwd` 指定"在哪个仓库执行"。
  // 🔴 与 file/path 是两回事：后者是 pathspec（过滤哪些文件），前者是子进程的 current_dir。
  const cwdRaw = str(args, "cwd") || str(args, "repo");
  const cwd = cwdRaw === null ? null : repoPath(cwdRaw);

  const files = [];
  const single = str(args, "file") || str(args, "path");
  if (single !== null) files.push(repoPath(single));
  const many = args.files;
  if (Array.isArray(many)) {
    for (let i = 0; i < many.length; i++) {
      if (many[i] === undefined || many[i] === null) continue;
      const item = String(many[i]).trim();
      if (item === "") continue;
      const path = repoPath(item);
      if (files.indexOf(path) < 0) files.push(path);
    }
  }

  const refRaw = str(args, "ref");
  const reference = refRaw === null ? null : gitRef(refRaw);
  const branchRaw = str(args, "branch");
  const branch = branchRaw === null ? null : gitRef(branchRaw);
  const staged = bool(args, "staged");
  let limit = 10;
  if (typeof args.limit === "number" && isFinite(args.limit)) {
    limit = Math.max(1, Math.min(200, Math.floor(args.limit)));
  }

  function needFiles(what) {
    if (files.length === 0) {
      throw reject("`" + what + "` 需要指定文件：传 file / path，或 files 数组。");
    }
  }

  let argv;
  if (action === "status") {
    argv = ["status", "--porcelain=v1", "-b"];
  } else if (action === "diff") {
    argv = ["diff"];
    if (staged) argv.push("--cached");
    if (reference !== null) argv.push(reference);
    argv.push("--");
    argv = argv.concat(files);
  } else if (action === "log") {
    argv = ["log", "-n", String(limit), "--oneline", "--decorate"];
    if (reference !== null) argv.push(reference);
    if (files.length > 0) {
      argv.push("--");
      argv = argv.concat(files);
    }
  } else if (action === "show") {
    argv = ["show", reference === null ? "HEAD" : reference, "--stat", "--patch"];
    if (files.length > 0) {
      argv.push("--");
      argv = argv.concat(files);
    }
  } else if (action === "blame") {
    needFiles("blame");
    argv = ["blame", "--date=short", files[0]];
  } else if (action === "add") {
    // 不指定文件 = 暂存全部改动（含新增与删除），这是 `git add -A` 的语义。
    argv = files.length === 0 ? ["add", "-A"] : ["add", "--"].concat(files);
  } else if (action === "commit") {
    const rawMessage = args.message;
    if (rawMessage === undefined || rawMessage === null) {
      throw reject("`commit` 需要 message（提交信息）。");
    }
    argv = ["commit", "-m", commitMessage(rawMessage)];
    if (bool(args, "all")) argv.push("-a");
    if (files.length > 0) {
      argv.push("--");
      argv = argv.concat(files);
    }
  } else if (action === "checkout") {
    if (bool(args, "create")) {
      const name = branch !== null ? branch : reference;
      if (name === null) throw reject("`checkout` 创建分支需要 branch（新分支名）。");
      argv = ["checkout", "-b", name];
    } else {
      const target = reference !== null ? reference : branch;
      if (target === null) throw reject("`checkout` 需要 ref 或 branch。");
      argv = ["checkout", target];
    }
  } else if (action === "restore") {
    needFiles("restore");
    argv = ["restore"];
    if (staged) argv.push("--staged");
    argv.push("--");
    argv = argv.concat(files);
  } else if (action === "reset") {
    const rawMode = String(args.mode === undefined || args.mode === null ? "mixed" : args.mode)
      .trim()
      .toLowerCase();
    if (rawMode !== "soft" && rawMode !== "mixed" && rawMode !== "hard") {
      throw reject("未知 reset mode `" + rawMode + "`（可用：soft / mixed / hard）");
    }
    argv = ["reset", "--" + rawMode, reference === null ? "HEAD" : reference];
  } else if (action === "stash") {
    const sub = String(args.sub === undefined || args.sub === null ? "list" : args.sub)
      .trim()
      .toLowerCase();
    if (sub === "list" || sub === "pop" || sub === "apply" || sub === "drop" || sub === "show") {
      argv = ["stash", sub];
    } else if (sub === "push" || sub === "save") {
      argv = ["stash", "push"];
      const rawMessage = args.message;
      if (rawMessage !== undefined && rawMessage !== null) {
        argv.push("-m", commitMessage(rawMessage));
      }
    } else {
      throw reject("未知 stash sub `" + sub + "`（可用：list / push / pop / apply / drop / show）");
    }
  } else if (action === "branch") {
    if (bool(args, "delete")) {
      if (branch === null) throw reject("`branch` 删除分支需要 branch（分支名）。");
      argv = ["branch", "-d", branch];
    } else if (branch !== null) {
      argv = ["branch", branch];
    } else {
      // 不传 branch = 列本地分支（带上游与最后提交）
      argv = ["branch", "-vv"];
    }
  } else if (action === "merge") {
    const target = reference !== null ? reference : branch;
    if (target === null) throw reject("`merge` 需要 ref 或 branch。");
    argv = ["merge", "--no-edit", target];
  } else {
    // push / pull
    const remoteRaw = str(args, "remote");
    const remote = remoteRaw === null ? "origin" : gitRef(remoteRaw);
    argv = [action, remote];
    const target = reference !== null ? reference : branch;
    if (target !== null) argv.push(target);
    if (action === "push" && bool(args, "force")) argv.push("--force-with-lease");
  }

  return { action: action, argv: argv, cwd: cwd };
}

// ── 执行 ────────────────────────────────────────────────────────────────────
// 工作区根**必须**从 `ada.workspace` 取（沙箱里的 `ws` 只存在于工厂那一次 eval 的
// 作用域，模块级函数看不到它——写 `workspace` 会直接 ReferenceError）。

/// 尾部空白一律去掉（与基座实现的 `trim_end()` 同义）。
function trimEnd(text) {
  return String(text).replace(/\s+$/, "");
}

/// `not a git repository` 的诊断：多仓库工作区是很常见的形态，而 `path` 的语义是
/// **文件过滤**（pathspec），不是"在哪个仓库执行"。所以把一层子目录里的独立仓库
/// 直接列出来，并给出可以照抄的 `cwd`。
function childRepos(root) {
  const found = [];
  let entries;
  try {
    entries = deps().fs.readdirSync(root);
  } catch (_) {
    return found;
  }
  for (let i = 0; i < entries.length; i++) {
    const name = String(entries[i]);
    const dir = deps().path.join(root, name);
    let isDir = false;
    try {
      isDir = deps().fs.statSync(dir).isDirectory();
    } catch (_) {
      continue;
    }
    if (!isDir) continue;
    // `.git` 可能是目录（普通仓库），也可能是文件（worktree / submodule）
    if (!deps().fs.existsSync(deps().path.join(dir, ".git"))) continue;
    found.push(name);
  }
  found.sort();
  return found;
}

function repoHint(root, cwd) {
  if (cwd !== null) {
    return "`" + cwd + "` 及其上层都不是 Git 仓库（工作区：" + root + "）。确认 `cwd` 指向的是仓库根或其子目录。";
  }
  const repos = childRepos(root);
  if (repos.length === 0) {
    return "当前目录及其上层都不是 Git 仓库：" + root;
  }
  const example = JSON.stringify({ action: "status", cwd: repos[0] });
  return (
    "工作区根本身不是 Git 仓库，但检测到以下子目录是**独立仓库**：" + repos.join(" / ") + "。\n" +
    "请用 `cwd` 指定其中一个再试，例如 " + example + "。\n" +
    "（`path` / `file` 是**文件过滤**，不是仓库目录；`cwd` 才决定在哪个仓库执行。）"
  );
}

/// 截断并按 UTF-8 字符边界回退，附上"怎么拿全文"的指引。
function truncate(res, action) {
  const text = String(res.output);
  if (text.length <= MAX_GIT_BYTES) return res;
  let cut = MAX_GIT_BYTES;
  // JS 的 `.length` 是 UTF-16 码元数：切在低位代理（emoji 等）中间会产生半个字符。
  let guard = 0;
  while (cut > 0 && guard < 2) {
    const code = text.charCodeAt(cut);
    if (code >= 0xdc00 && code <= 0xdfff) {
      cut -= 1;
    } else {
      break;
    }
    guard += 1;
  }
  const origKb = Math.floor(text.length / 1024);
  const keptKb = Math.floor(cut / 1024);
  return {
    ok: res.ok,
    output:
      text.slice(0, cut) +
      "\n\n……（输出已截断：原始约 " + origKb + " KB，仅保留前 " + keptKb + " KB。" +
      "需要全文请缩小范围：`action:\"diff\"` 指定单个 file、`action:\"log\"` 调低 limit、" +
      "或改用 `read_file` 读具体文件；整仓 " + action + " 不要一次性读入。）",
  };
}

function failureMessage(err) {
  const status = err && typeof err.status === "number" ? err.status : -1;
  const stderr = err && err.stderr ? trimEnd(err.stderr) : "";
  const stdout = err && err.stdout ? trimEnd(err.stdout) : "";
  let message;
  if (err && err.code === "ETIMEDOUT") {
    message = "git 执行超时（" + Math.floor(GIT_TIMEOUT_MS / 1000) + "s）：";
  } else if (err && err.code === "ENOENT") {
    return "启动 git 失败（未安装或不在 PATH 上）：" + (err.message || "ENOENT");
  } else if (err && err.code === "ENOBUFS") {
    message = "git 输出超过 maxBuffer（已放大到不限制，仍溢出说明实在太大了）：";
  } else {
    message = "git 执行失败（退出码 " + status + "）";
    if (stderr) message += "：";
  }
  if (stderr) message += "\n" + stderr;
  if (stdout) message += "\n" + stdout;
  return message;
}

function runGit(argv, cwd, root) {
  const dir = cwd === null ? root : deps().path.join(root, cwd);
  if (!deps().fs.existsSync(dir)) {
    return { ok: false, output: "git 执行目录不存在：" + dir };
  }
  // env 一旦给出就是**替换**语义：所以必须从 process.env 拷一份再覆盖。
  // - GIT_PAGER=cat：没有 pager 可等，输出直接出来
  // - GIT_TERMINAL_PROMPT=0：要凭据时立刻失败，而不是挂着等满超时
  const env = Object.assign({}, process.env || {});
  env.GIT_PAGER = "cat";
  env.GIT_TERMINAL_PROMPT = "0";
  try {
    const raw = deps().cp.execFileSync("git", argv, {
      cwd: dir,
      env: env,
      encoding: "utf8",
      timeout: GIT_TIMEOUT_MS,
      maxBuffer: 0,
      // stdin 给空设备：git 在需要输入时会自己失败，而不是等我们喂
      stdio: ["ignore", "pipe", "pipe"],
    });
    const text = trimEnd(raw === null || raw === undefined ? "" : raw);
    return { ok: true, output: text === "" ? "（命令成功，无输出）" : text };
  } catch (err) {
    return { ok: false, output: failureMessage(err) };
  }
}

function executeGit(args, allowed, root) {
  let planned;
  try {
    planned = buildArgv(args || {}, allowed);
  } catch (err) {
    // 参数错误是"用户可修正"的失败：如实返回，不抛（抛会把整次调用变成运行时错误）
    return { ok: false, output: err && err.message ? err.message : String(err) };
  }
  const result = runGit(planned.argv, planned.cwd, root);
  if (!result.ok && result.output.indexOf("not a git repository") >= 0) {
    return { ok: false, output: repoHint(root, planned.cwd) };
  }
  return truncate(result, planned.action);
}

// ── 描述符 ──────────────────────────────────────────────────────────────────
function gitSchema(actions) {
  return {
    type: "object",
    properties: {
      action: {
        type: "string",
        enum: actions,
        description:
          "要执行的 Git 动作。只读：status/diff/log/show/blame；" +
          "写入：add/commit/checkout/restore/reset/stash/branch/merge/push/pull",
      },
      file: {
        type: "string",
        description:
          "单个文件（相对 cwd，默认工作区根）；diff/log/show/add/restore/blame 用，path 是别名",
      },
      path: {
        type: "string",
        description: "file 的别名——它是**文件过滤**（pathspec），不是仓库目录；要换仓库请用 cwd",
      },
      files: {
        type: "array",
        items: { type: "string" },
        description: "多个文件（add/restore/commit 用，相对 cwd）",
      },
      cwd: {
        type: "string",
        description:
          "在哪个子目录执行 git（相对工作区，默认工作区根）。多仓库工作区" +
          "（根目录下 frontend/backend 各自是仓库）必须用它指定仓库，例如 cwd:\"frontend\"",
      },
      ref: {
        type: "string",
        description: "提交/分支/标签引用（diff/log/show/reset/checkout/merge/push/pull 用；默认 HEAD）",
      },
      branch: {
        type: "string",
        description: "分支名（branch 创建/删除、checkout 目标、push/pull 目标分支）",
      },
      message: { type: "string", description: "提交信息（commit 必填；stash push 可选，不能含换行）" },
      limit: { type: "integer", description: "log 返回条数（默认 10，上限 200）" },
      staged: { type: "boolean", description: "diff: 看暂存区（--cached）；restore: 从暂存区恢复（--staged）" },
      all: { type: "boolean", description: "commit: 先暂存已跟踪文件的改动（-a）" },
      mode: {
        type: "string",
        enum: ["soft", "mixed", "hard"],
        description: "reset 模式（默认 mixed；hard 会丢弃工作区改动，需要审批）",
      },
      sub: {
        type: "string",
        enum: ["list", "push", "pop", "apply", "drop", "show"],
        description: "stash 子操作（默认 list）",
      },
      remote: { type: "string", description: "push/pull 的远端名（默认 origin）" },
      create: { type: "boolean", description: "checkout: 创建并切换到新分支（-b）" },
      delete: { type: "boolean", description: "branch: 删除分支（-d）" },
      force: { type: "boolean", description: "push: 使用 --force-with-lease 强推（需要审批）" },
    },
    required: ["action"],
  };
}

export default function (ada) {
  ada.metadata = metadata;
  // 工作区根：插件执行期唯一可靠来源就是 `ada.workspace`（见文件头注释）。
  const root = typeof ada.workspace === "string" && ada.workspace ? ada.workspace : process.cwd();

  ada.registerTool({
    name: "git",
    label: "Git",
    description:
      "查看与操作 Git 版本库（action 选动作：status/diff/log/show/blame 只读；" +
      "add/commit/checkout/restore/reset/stash/branch/merge/push/pull 会改仓库）。" +
      "多仓库工作区用 cwd 指定在哪个仓库执行。",
    parameters: gitSchema(ALL_ACTIONS),
    async execute(args) {
      return executeGit(args, ALL_ACTIONS, root);
    },
  });

  ada.registerTool({
    name: "git_read",
    label: "Git 只读查询",
    // 声明只读：只读档 / plan 模式 / 只读子智能体也能用。
    // 这条承诺必须与实现一致——这里只允许 READ_ACTIONS。
    access: "read",
    description:
      "只读查询 Git 版本库（status/diff/log/show/blame）。写操作请用 git 工具；" +
      "本工具在只读模式下同样可用。",
    parameters: gitSchema(READ_ACTIONS),
    async execute(args) {
      return executeGit(args, READ_ACTIONS, root);
    },
  });
}
