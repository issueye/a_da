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
  hooks: { count: 1, names: ["afterTurn"] },
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

  ada.on("turn_end", async (event, ctx) => {
    ada.trace(`${ctx.point} ${ctx.threadId}`);
    return { action: "note", note: `completed step ${event.step}` };
  });
}
```

`registerTool` declarations are collected during isolated inspection and installed in the
runtime catalog. Built-in names keep their canonical descriptor. New third-party tool
names receive a conservative descriptor (write-classified and approval-required), and
their `execute` factory closure is recreated inside the script sandbox for each call.
Thus package-defined tools are not limited to names already present in the built-in
registry; unknown plugin tools fail safe as writes and must pass approval.

The package entrypoint must be `<package-root>/index.ts` or `<package-root>/index.js`.
Node-style relative module loading is not currently supported inside the Boa sandbox.
Loose single-file plugins and object-style default exports are no longer discovered or
accepted by `plugin.validate`.

`metadata` counts and required `names` arrays are checked against the declarations
collected by the isolated inspector. `ada.on(event, handler)` is an alias for hook
registration. Handlers receive `(event, ctx)`; `ctx` exposes workspace/session
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

- Actions do **not** enter the model tool catalog, approval gate, or checkpoints.
- They are called by the SSR bridge `plugin.invoke`; a unified package's
  `plugin.json` maps a frontend binding to an action name.
- `mode: "read"` is required for `intent: "read"`; write actions are rejected when
  the caller declares read intent.
- `metadata.actions` is optional but mandatory once an action is registered;
  `count` / `names` are validated like tools and hooks.
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

SQL is not a model tool and does not enter the approval gate: it is a backend
capability, same as `fs`. Use `readOnly: true` for read-only databases.

**Skills are directories, not declarations.** Put `skills/<name>/SKILL.md` next to
`index.ts`; the host scans it during assembly and reads the YAML frontmatter
(`name` must equal the directory name, `description` 1-1024 chars — see
[`docs/SKILL_SPEC.md`](../SKILL_SPEC.md)). `ada.registerSkill(...)` was removed on
2026-10-19: calling it is ignored and reported as a `skill_spec_violation:*` diagnostic.
`metadata.skills` is declarative only and is not checked for consistency.

## Hook points and capability gates (as of 2026-10-17)

* Every point in `agent_base::ports::hooks::points::IMPLEMENTED` (33 of them, including
  `beforeTodoUpdate` / `afterTodoUpdate`) is callable and registrable; a point that is not
  registered is reported as `no_subscriber` rather than silently skipped.
* Which **actions** a hook may take is graded per point
  (`HookLevel` + `allows`): third-party plugins can `Deny` / `Replace` on data-plane points
  (`beforeToolCall`, `beforeTodoUpdate`) only; control-plane `Deny` is limited to builtin plugins.
* The user's plugin capability switches gate actions on top of that: with
  `allowThirdPartyHooks` off, third-party hook declarations are rejected at assembly time
  (`third_party_hooks_disabled`); `allowTextRewrite` off drops `note` results;
  `allowThreadDeleteBlock` / `allowCompactionReplace` off make the corresponding
  deny/replace a no-op. Every such block is recorded with a reason you can read in the
  diagnostics panel.
* `ctx.registerTool` / `registerSkill` / `registerSystemPrompt` inside a handler are
  currently **collected but not applied** (the host does not install them yet); the runtime
  logs a warning when a handler asks for them. There is **no** `ctx.registerHook`: hooks must
  be declared at factory time via `ada.on(point, handler)` — runtime hook registration would
  mutate the hook table mid-turn, which the assembly model forbids.

See [`extension-api.d.ts`](extension-api.d.ts) for the type contract.
