# pi-style a-da Extension API

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
  ada.registerSkill({
    name: "test_skill",
    description: "A package skill",
    path: "skills/test_skill.md",
  });

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
