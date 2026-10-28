# pi-style a-da Extension API

> 本文原名 `docs/plugin-sdk/pi-style-extension.md`，文档整理时移入 `plugins/` 并改名为
> `extension-factory.md`（旧 SDK 形态的 `plugin-sdk/v1.md` 已删除）。
> 平台层面的说明（发现规则、贡献模型、能力开关、排错）见 [README.md](README.md)。

Plugin packages use a single factory entrypoint, matching pi's extension pattern:
`export default function (ada: ExtensionAPI) {}` in the package's `index.ts`.
`index.js` is accepted for precompiled JavaScript packages. A JSDoc variant
keeps editor types in JavaScript:

```js
/** @typedef {import('./extension-api').ExtensionAPI} ExtensionAPI */
/** @param {ExtensionAPI} ada */
```

```js
const metadata = {
  name: "test",
  description: "测试插件",
  tools: { count: 1, names: ["test_tool"] },
  systemPrompt: { count: 1, names: ["增强命中"] },
  skills: { count: 1, names: ["test_skill"] },
  events: { count: 1, names: ["event_after_turn"] },
};

export default function (ada) {
  // Make package metadata visible to the isolated inspector.
  ada.metadata = metadata;

  ada.registerTool({
    name: "test_tool",
    description: "Example plugin tool",
    parameters: { type: "object", properties: {} },
    async execute(args) {
      return { ok: true, output: `workspace=${ada.workspace}` };
    },
  });

  ada.registerSystemPrompt({ id: "增强命中", content: "Prefer precise matches." });

  ada.on("event_after_turn", async (event, ctx) => {
    ada.trace(`${ctx.event} ${ctx.threadId}`);
    return { action: "note", note: `completed step ${event.step}` };
  });
}
```

`registerTool` declarations are collected during isolated inspection and installed in the
runtime catalog. Built-in names keep their canonical descriptor. New third-party tool
names receive a conservative descriptor (write-classified), and
their `execute` factory closure is recreated inside the script sandbox for each call.
Thus package-defined tools are not limited to names already present in the built-in
registry; unknown plugin tools fail safe as writes.

The package entrypoint must be `<package-root>/index.ts` or `<package-root>/index.js`.
Node-style relative module loading is not currently supported inside the Boa sandbox.
Loose single-file plugins and object-style default exports are no longer discovered or
accepted by `plugin.validate`.

`metadata` counts and required `names` arrays are checked against the declarations
collected by the isolated inspector. `ada.on(event, handler)` is the only event registration entrypoint; the name must be a unified `event_*` value and legacy camelCase names are rejected. Handlers receive `(event, ctx)`; `ctx` exposes workspace/session
identifiers, timeout budget, and validated registration methods, but not raw host
IO or arbitrary host actions.

## UI actions: `registerAction`

A factory may also register **UI-only actions**:

```js
ada.registerAction({
  name: "status",
  description: "读取状态",
  mode: "read",                 // read | write; default write
  async execute(input, ctx) {
    return { ok: true, data: { count: 0 } };
  },
});
```

- Actions do **not** enter the model tool catalog or checkpoints.
- They are called by the SSR bridge `plugin.invoke`; a unified package's
  `plugin.json` maps a frontend binding to an action name.
- `mode: "read"` is required for `intent: "read"`; write actions are rejected when
  the caller declares read intent.
- `metadata.actions` is optional but mandatory once an action is registered;
  `count` / `names` are validated like tools and events.
- The action result is `{ ok: true, data }`; a returned `{ ok: false, error }` is a
  soft failure with a user-readable message.
- Each call creates a fresh sandbox; actions have no cross-request module state.

See [unified-package.md](unified-package.md) for the one-package `frontend` /
`backend` layout, the SSR bridge route, and the current v1 limits.

## Dynamic HTTP routes: `ada.route`

A factory can declare its own HTTP routes. The route table lives in TypeScript and is
matched inside the sandbox on every request, so **adding a backend endpoint does not
require a Rust change**:

```js
ada.route({
  method: "GET",                 // default GET; "*" matches every method
  path: "/status/:name",         // :param and *rest (last segment only)
  async handler(req, ctx) {
    return { name: req.params.name, q: req.query.q, at: ctx.now };
  },
});
```

Browser entrypoint (only for an enabled unified package with a `backend` section):

```text
GET  /ext/<frontend-id>/route/status/ada?q=1
POST /ext/<frontend-id>/route/echo
```

Request shape:

| field | meaning |
|---|---|
| `method` / `path` | the HTTP request as received |
| `params` | matched `:param` / `*rest` segments |
| `query` | query object; repeated keys become string arrays |
| `headers` | sanitized request headers; `cookie` / `authorization` are **not** forwarded |
| `body` / `bodyKind` | `json` / `form` / `text` / `base64` / `empty` |
| `workspace` / `threadId` / `now` | host context (`threadId` comes from `?threadId=...`) |

Response helpers on `ada`:

```js
return { ok: true };                      // JSON 200
return ada.json({ ok: true }, 201);       // explicit status
return ada.text("plain", 200, { "x-note": "1" });
return ada.html("<h1>hi</h1>");
return ada.redirect("/threads/abc");      // 303 by default
return ada.response({ status: 204, headers: { "cache-control": "no-store" } });
```

- Route paths are normalized at registration; duplicate `method + path` is an
  inspection error.
- Matching prefers static segments over `:param`, and `:param` over `*rest`.
  `HEAD` falls back to a `GET` route when no `HEAD` route exists.
- Request bodies are capped at 1 MB; responses at 2 MB. A request must finish within
  15 seconds of sandbox time.
- Response headers are allowlisted (`content-*`, `cache-control`, `etag`,
  `location`, `set-cookie`, `x-*`, …); hop-by-hop headers are dropped.
- Every request creates a fresh sandbox, so module state does not persist. Use
  workspace files for state.
- `ada.route` is only reachable through an enabled unified package (frontend id in the
  URL, backend id in the package). Backend-only packages have no browser entrypoint.
- `metadata.routes` is optional; if present, `count` / `names` are validated against
  the registered routes.

## SQLite: `require('node:sqlite')`

Declare the capability first; without it the module is not injected at all:

```ts
// @ada-needs sqlite
export default function (ada) {
  ada.registerAction({
    name: "bump",
    description: "累加计数",
    mode: "write",
    async execute() {
      const { DatabaseSync } = require("node:sqlite");
      const db = new DatabaseSync(".data/counter.db");
      db.exec("CREATE TABLE IF NOT EXISTS counts(name TEXT PRIMARY KEY, n INTEGER)");
      db.prepare(
        "INSERT INTO counts(name, n) VALUES (?, 1) " +
        "ON CONFLICT(name) DO UPDATE SET n = n + 1",
      ).run("demo");
      const row = db.prepare("SELECT n FROM counts WHERE name = ?").get("demo");
      db.close();
      return { count: row.n };
    },
  });
}
```

Supported surface (Node-compatible subset):

- `new DatabaseSync(path, { readOnly?, timeout?, enableForeignKeyConstraints?, open? })`
- `db.exec(sql)`, `db.prepare(sql)`, `db.open()`, `db.close()`, `db.isOpen`, `db.location()`
- `stmt.run(...params)` → `{ changes, lastInsertRowid }`
- `stmt.get(...params)` → object / `undefined`
- `stmt.all(...params)` → array
- Positional (`?`) and named (`:name` / `$name` / `@name` / bare `name`) parameters
- `null` / number / string / boolean (0/1) / `Buffer`/`Uint8Array` (BLOB) / `BigInt`
- Query results are plain objects; BLOB columns come back as `Buffer`
- `:memory:` is always allowed; an empty path is treated as `:memory:`
- Database files must resolve inside the plugin workspace; `file:` URIs are rejected
- `ATTACH DATABASE` / `VACUUM INTO` go through the same workspace check and must use
  **absolute** paths inside the workspace (relative paths are denied because SQLite
  resolves them against the host process CWD); `load_extension` / `readfile` /
  `writefile` functions are denied
- Result sets are capped at 10,000 rows / 8 MB, SQL at 1 MB; default busy timeout is 5s
- SQLite runtime limits: value/row 8 MB, SQL 1 MB, 4 attached databases,
  expression depth 500, 1,000 variables, trigger depth 100
- SQLite extension loading is not exposed

SQL is not a model tool: it is a backend
capability, same as `fs`. Use `readOnly: true` for read-only databases.

## child_process: sync subset + async subset

Declare `@ada-needs exec` (aliases: `child_process`, `child-process`, `shell`).

```ts
// @ada-needs exec
const { execFileSync, spawnSync, execSync, spawn, exec } = require("node:child_process");

// argv，不经 shell
const out = execFileSync("git", ["status", "--short"], { encoding: "utf8" });

// shell 命令
execSync("echo shell-ok", { encoding: "utf8" });

// 不抛错的形态
const result = spawnSync("cmd.exe", ["/d", "/s", "/c", "exit 3"]);
// result.status === 3, result.stdout / result.stderr 是 string 或 Buffer

// 异步：真流式，进程还在跑的时候就能一块一块收到输出
const child = spawn("git", ["log", "--oneline"]);
child.stdout.on("data", (chunk) => { /* Buffer */ });
child.on("close", (code, signal) => { /* stdio 全关之后 */ });

// 异步 + 回调
exec("git status --short", (error, stdout, stderr) => { /* ... */ });
```

Supported options (shared by both subsets):

- `cwd`（缺省是工作区；绝对路径可指向任意目录，相对路径以工作区为基准）
- `env`（给了就**替换**环境；始终补 `A_DA_AGENT=1`）
- `stdio`：`pipe` / `ignore` / `null` / `inherit`（`inherit` 直接接宿主 stdout/stderr）
- `encoding`：`utf8`（默认）或 `buffer` / `null`
- `timeout`：毫秒；`0` / 省略 = 不超时，超时杀整棵进程树
- `maxBuffer`：默认 1 MB；`0` = 不限制
- `shell`：`spawnSync` / `execFileSync` / `spawn` / `execFile` 传 `shell:true` 时把
  file+args 交给系统 shell；`execSync` / `exec` 本身就是 shell 命令

Sync subset (`execFileSync` / `spawnSync` / `execSync`):

- `execFileSync` / `execSync` 在非零退出、超时、`maxBuffer`、启动失败时**抛错**，
  错误上带 `status` / `signal` / `stdout` / `stderr` / `code`（超时 `ETIMEDOUT`，
  超限 `ENOBUFS`，找不到可执行文件 `ENOENT`）。
- `spawnSync` 不抛错；失败放在 `result.error`。
- stdin 默认为空设备；只有传 `input` 时才写数据。

Async subset (`spawn` / `exec` / `execFile`):

- `spawn(file[, args][, options])` 返回 `ChildProcess`，事件顺序
  `spawn` → `stdout`/`stderr`(`data`…) → `end` → `exit(code, signal)` → `close(code, signal)`。
  `close` 一定晚于最后一块 `data`（与 Node 一致），所以 `exec` 的回调在输出收齐后才触发。
- `child.stdout` / `child.stderr` 支持 `on('data')`、`on('end')`、`setEncoding()`；
  **不支持 `pipe()`**（半实现的 pipe 比没有更糟），需要转发请自行在 `data` 里写。
- `child.stdin.write(chunk[, cb])` / `child.stdin.end()`（写入按调用顺序落地）。
- `child.kill([signal])` 杀**整棵进程树**；`pid` / `exitCode` / `signalCode` / `killed`
  与 Node 同义。
- `exec(command, options?, callback?)` / `execFile(file, args?, options?, callback?)`
  回调签名 `(error, stdout, stderr)`；非零退出、超时（`ETIMEDOUT`）、
  `maxBuffer`（`ENOBUFS`）都会给出 `error`。
- 超时由宿主侧（Tokio）计时并杀树，不走 JS 的 `setTimeout`——否则 `wait_idle()`
  会被一个已经没用的定时器拖满整个超时时间。
- `fork` **不提供**：它需要 IPC channel（父子进程间的结构化消息通道），
  用 `spawn` 起独立进程、走 stdin/stdout 通信。
- `ChildProcess` 上没人监听的 `'error'` 不会抛出（抛会把事件循环炸掉）：会记到
  `child.spawnError` 并打一条 `console.error`，在插件日志里能看到。
- ⚠️ `exec` 等价于任意代码执行。当前阶段优先功能实现，信任边界由插件安装者负责；
  后续再补策略。不要在不可信插件上声明 `exec`。

**Skills are directories, not declarations.** Put `skills/<name>/SKILL.md` next to
`index.ts`; the host scans it during assembly and reads the YAML frontmatter
(`name` must equal the directory name, `description` 1-1024 chars — see
[`docs/SKILL_SPEC.md`](../SKILL_SPEC.md)). `ada.registerSkill(...)` was removed on
2026-10-19: calling it is ignored and reported as a `skill_spec_violation:*` diagnostic.
`metadata.skills` is declarative only and is not checked for consistency.

## Events and capability gates (as of 2026-10-27)

* Plugins register exactly one namespace: `event_*`. `ada.on` and object-form `events`
  reject any other name at inspection time; the old camelCase names and short aliases
  (`turn_end`, `beforeToolCall`, …) are gone.
* Every internal point in `agent_base::ports::hooks::points::IMPLEMENTED` (29 of them) has
  a unified event name in `agent_base::ports::hooks::EVENT_POINTS`. A registered event is
  callable; an event nobody subscribed to is reported as `no_subscriber` rather than
  silently skipped.
* `beforeTodoUpdate` / `afterTodoUpdate` / `beforeApproval` / `afterApproval` were removed
  on 2026-10-27; todo updates and approval are no longer core extension points. Plugins
  that need policy behavior attach to `event_before_*` instead.
* Which **actions** an event may take is graded per point
  (`HookLevel` + `allows`): third-party plugins can `Deny` / `Replace` on the data-plane
  `event_before_tool_call` only; control-plane `Deny` is limited to builtin plugins.
* The user's plugin capability switches gate actions on top of that: with
  `allowThirdPartyEvents` off, third-party event declarations are rejected at assembly time
  (`third_party_events_disabled`); `allowTextRewrite` off drops `note` results;
  `allowThreadDeleteBlock` / `allowCompactionReplace` off make the corresponding
  deny/replace a no-op. Every such block is recorded with a reason you can read in the
  diagnostics panel.
* `ctx.registerTool` / `registerSystemPrompt` inside a handler are currently **collected
  but not applied** (the host does not install them yet); the runtime logs a warning when
  a handler asks for them. There is **no** `ctx.registerHook`: events must be declared at
  factory time via `ada.on(event, handler)` — runtime registration would mutate the event
  table mid-turn, which the assembly model forbids.

See [`extension-api.d.ts`](extension-api.d.ts) for the type contract.
