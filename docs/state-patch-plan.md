# 增量状态帧（`evt.state.patch`）与 item 分页 —— 实施计划

> 背景：一条长会话（实测 **822 个 item / 一帧 1852 KB**；合成负载 **800 item / 3705 KB**）时，
> `evt.state.snapshot` 是**全量**帧，16ms 一帧（大帧已降到 100ms）→ 每秒几十 MB 的
> 序列化 + IPC + 前端 JSON 解析，渲染进程内存被顶到 GB 级。
> 窗口渲染 / 字段限幅只能缓解，**根治要按 item 增量**。

## 帧格式

```jsonc
// evt.state.patch —— 取代高频的全量 evt.state.snapshot（后者保留给首帧/自愈/重连）
{
  "seq": 42, "baseSeq": 41,      // 客户端发现不连续 → 主动请求一次全量快照自愈
  "meta": { /* queue / running / runningThreadIds / waitingThreadIds / log /
               config / ui / workspace / activeThreadId / providers / appearance */ },
  "threads": [{ "id": "…", "title": "…", "workspace": "…", "agentId": "…", "isSubagent": false }],
  "upserted": [{ "threadId": "…", "item": { /* 完整 item */ } }],
  "removed":  [{ "threadId": "…", "itemId": "…" }],
  "itemsWindow": { "from": 740, "to": 822, "total": 822 }
}
```

## 步骤（每步独立可验证、零回归）

1. **能力协商**：`session.initialize` 的 `client.capabilities` 里声明 `"state.patch"`；
   **没声明就继续发全量快照**——这是"宿主先落地、客户端后跟上"的安全绳，
   也是旧客户端/回滚路径的保证。
2. **宿主侧版本指纹**：复用 `crates/agent-rpc/src/state/store.rs` 的 `item_visible_key()`
   （id + 文本长度 + 状态 + 完成时刻 …），`StateBroadcaster` 按连接记住 `{itemId → 指纹}`，
   只推**指纹变了**的 item；连接首帧 / `baseSeq` 失配 / 有 item 被删 → 走全量或显式 `removed`。
3. **客户端合并**：`desktop-ui/src/client/ws-client.ts` 维护
   `Map<threadId, Map<itemId, Item>>`；patch 只 upsert/remove；`meta` 覆盖小字段；
   `seq` 不连续 → 发一次 `session.snapshot` 并全量应用（自愈）。
4. **分页 RPC** `thread.items.page(threadId, from, count)`：让"显示更早的 60 条"
   按需拉取、内存里只留窗口。属协议登记变更：`methods.rs`（计数 +1）、
   `spec/proto/ada-coding.ext.json`、`client-ts/{methods,dto}.ts` 三处同步
   （AGENTS §16 有守门测试，漏一处会红）。

## 验收（都不需要模型供应商）

- 负载：`pwsh -NoProfile -File tmp\synth-heavy-thread.ps1`（800 item，当前一帧 3705 KB）。
- 三组数字前后对比：
  1. **帧大小**：全量 3.7 MB → 稳态应降到**几十 KB**（一个流式 item + 小 meta）；
  2. **JS 堆**：`pwsh -NoProfile -File tmp\cdp-heap-probe.ps1`（`Runtime.getHeapUsage`）；
  3. **DOM 节点数**：同一脚本内 `document.querySelectorAll('*').length`。
- 单元测试（agent-rpc）：①只推变化项（未变 item 不出现在 `upserted`）；
  ②`baseSeq` 失配回落全量；③`removed` 生效；④未声明能力的连接仍收全量快照。
- 门：`cargo test --workspace -- --test-threads=1`（34 套件）+ `bun run typecheck` +
  `verify-wiring` / `verify-spec`；跑完比对 `~/.a-da/{config.json,gateway_workspaces.json,skills_state.json}` 哈希未变。

## 🔴 顺序：`upserted` 是按 **id** 排序发的，所以定位只能按 `at`

`upserted` 要保证帧内容可复现，因此按 `(threadId, itemId)` 排序——**到达顺序没有意义**。
客户端给"本地还没有的卡片"定位就只能按 `at`（`ws-client.ts::insertByItemTime`：
按时间插入，**等时间戳保持已有相对顺序**）。

于是有一条必须两处都守的不变量：**同一个会话里卡片的 `at` 严格递增**，且与显示顺序一致。

- 磁盘回填（`agent_node::session::SessionManager::restore_all_threads`）：一条 assistant 消息
  会派生好几张卡（思考 / 正文 / 工具），它们原本共用消息自己的毫秒时间戳，先后就落到 id 字典序上——
  `..._restored_asst_*` 恰好排在 `..._restored_think_*` 前面，于是**轮次一收尾（内存卡片被磁盘回填
  整份换掉、id 全变），思考块从最上面掉到最下面**（用户实拍）。现在按折叠顺序发时间戳
  （`CardStamps`），顺序为**思考 → 正文 → 工具卡**，与流式现场一致。
- 内存侧（`agent_rpc::state::store::next_item_at`）：同一毫秒里连建两张卡（思考 + 首段正文、
  非流式模型一次返回的思考与正文）也会踩同一个坑，所以新卡片的 `at` 不许小于上一张。

守门：`agent-node` 的 `test_restored_cards_have_increasing_at_so_client_order_matches`
（按客户端那条规则重放一遍，断言顺序一致）+ 实机验收
`bun tools/verify-card-order-e2e.mjs [--with-tool]`（本地假模型走真协议，比较
**收尾前后界面上看到的顺序**——只读服务端快照是看不出问题的，服务端数组一直是对的）。

## 已知取舍

- `meta` 仍是全量（但它只有几十字节到几 KB：queue/running/log/config 等），
  真正的大头是 `items`，所以增量只做 items 就能拿到绝大部分收益。
- `log`（调试条目）已按字节限幅（`DEBUG_PAYLOAD_LIMIT`，另见 `recordDebug` 注释），
  它随 `meta` 走，不需要增量。
- 旧路径 `evt.state.snapshot` **保留**：首帧、自愈、重连、未声明能力的客户端都走它。
