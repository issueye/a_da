# a_da 运行时内存深度优化开发计划方案

> **文档状态**：实施中  
> **制定日期**：2026-10-01  
> **目标**：彻底解决 a_da 桌面客户端与后台 Agent 在运行过程中的内存暴涨、持续泄漏和 GC 频繁卡顿问题，将空闲及长会话运行时内存控制在合理低位。

---

## 一、 背景与现状瓶颈

当前项目架构为 **Bun + GPUIX 原生渲染 + Agent Core 运行时 + UI/Host 双进程 WebSocket 通信**。
在实际运行与长对话过程中，用户反馈内存占用极高，甚至出现卡顿和数百 MB 乃至数 GB 的峰值。

经过代码全链路分析，确诊以下 **6 大核心内存吞噬源**：

1. **跨进程快照全量广播风暴（最致命的短时间内存与 GC 炸弹）**：
   - 触发点：`src/agent/host/snapshot.ts`、`src/agent/host/server.ts`、`src/agent/host/emitter.ts`
   - 根因：模型流式打字或工具推进时，每 16ms 进行一次 WebSocket 广播。每次广播均调用 `readHostSnapshot()`，将**全部工作区的所有历史会话**（含所有卡片与消息）以及**全部 120 条调试日志**打包，执行全量 `JSON.stringify`，UI 侧执行 `JSON.parse`。在打字过程中每秒生成数百 MB 的短期内存垃圾，JSC 堆内存急速膨胀。
2. **Debug 日志即时深序列化与超大历史上下文堆积**：
   - 触发点：`src/agent/store.ts` 的 `logLlmRequest()` 与 `push()`
   - 根因：每次请求都把包含长会话全量消息与工具描述的巨型数组写入 payload，并在 `push` 时立即调用 `JSON.stringify(..., null, 2)` 生成带缩进的长字符串。最多保留 120 条，吃掉上百 MB 且每 16ms 随快照全量复制。
3. **检查点管理器（CheckpointManager）单例永久持有 Base64 内存缓存**：
   - 触发点：`src/agent/checkpoint.ts`
   - 根因：`cache` 为 `Map<string, Entry[]>`，只要加载过一次会话快照（如打开改动审阅面板），该会话所有文件的 `contentBase64`（单文件可达 5MB）全部永久驻留在内存中，直到删除会话才释放。
4. **终端命令输出无上限追加**：
   - 触发点：`src/agent/tools/builtins/bash.ts`
   - 根因：`stdoutText += chunk` 缺乏累积上限保护。一旦命令打印大批量日志（如几万行测试输出或查看大文件），变量无界增长，瞬时侵占大量堆空间。
5. **应用启动时全量同步预加载所有会话与消息（Eager Loading）**：
   - 触发点：`src/agent/store.ts` 的 `restore()` 与 `src/agent/session/manager.ts`
   - 根因：启动时读取磁盘上所有会话文件，不仅读取摘要，还将全部文件解析两遍成完整 `Thread` 存入 `this.threads`，导致无用历史会话长期常驻内存。
6. **缺少空闲内存回收（GC）机制**：
   - 根因：Bun 使用的 JavaScriptCore 引擎在 Windows 下存在分配器内存池保留策略，高并发垃圾产生后不会主动将物理内存（Working Set）归还操作系统。

---

## 二、 优化目标与预期效果

| 优化维度 | 优化前状态 | 优化后目标 |
| :--- | :--- | :--- |
| **流式打字时每秒垃圾分配** | 50MB ~ 200MB / 秒 | **< 1MB / 秒（降幅 > 98%）** |
| **单次快照体积** | 几百 KB ~ 数十 MB（随历史膨胀） | **< 20KB（仅活动会话与必要元数据）** |
| **调试日志内存常驻** | 50MB ~ 150MB | **< 5MB（延迟序列化 + 剪裁 messages）** |
| **文件快照内存占用** | 几十 MB ~ 上百 MB Base64 驻留 | **平时 0 MB（按需读盘，LRU 淘汰）** |
| **启动后空闲内存 (Working Set)** | 250MB ~ 600MB+ | **< 120MB ~ 180MB** |

---

## 三、 实施阶段与技术方案

### Phase 1：核心高频止血（P0，立即可做，收益最大）

#### 1.1 快照传输结构轻量化与按需下发
- **涉及模块**：
  - `src/agent/host/snapshot.ts`
  - `src/shared/protocol/dto.ts`
  - `src/ui/client/view-store.ts`
- **方案**：
  1. 快照只包含 **当前激活会话（`activeThread`）** 或当前打开的 Tab 列表的完整内容。
  2. 非活动会话在快照中仅提供轻量级 `ThreadSummary`（`id`, `title`, `workspace`, `updatedAt`, `mode`, `isSubagent`, `parentId`），去除庞大的 `items`、`messages` 与 `pluginData`。
  3. `log` 在未开启调试面板（`ui.debugOpen === false`）时，快照中只提供轻量计数与最近一条摘要；仅在调试面板打开时传输详细条目。

#### 1.2 Debug 日志延迟序列化与超大请求瘦身
- **涉及模块**：
  - `src/agent/store.ts`
- **方案**：
  1. 移除 `push()` 中立刻进行 `JSON.stringify(entry.payload, null, 2)` 的逻辑，将 `raw` 计算改为 getter 或组件内按需格式化。
  2. 在 `logLlmRequest()` 中，对 `payload.messages` 做轻量化摘要（仅保留最近几条或对过长工具结果/文件内容截断至前 300 字符），防止把整个几十轮的历史大对象直接作为日志引用留在内存中。

#### 1.3 CheckpointManager 内存缓存生命周期管理
- **涉及模块**：
  - `src/agent/checkpoint.ts`
- **方案**：
  1. 将全量 `cache` 改造为 LRU 限制（最多保留 2 个会话的近期条目），或在不使用时及时主动驱逐。
  2. 对于 `contentBase64`，不在常驻快照数组中持有大字符串；或者只在需要执行 `revert` 时从 JSONL 文件中读取还原，释放常驻内存。

#### 1.4 Bash 命令输出滑动截断窗口
- **涉及模块**：
  - `src/agent/tools/builtins/bash.ts`
- **方案**：
  1. 设置进程输出累积上限（例如 2MB）。
  2. 在 `child.stdout.on('data')` 累加过程中，若超过上限，实施滑动截断（保留前 200KB 和最新的 1.8MB，中间标记 `... [中间输出已截断] ...`），防止单一命令跑出几百 MB 引起 OOM。

---

### Phase 2：会话生命周期冷热分级与懒加载（P1）

#### 2.1 会话元数据与内容分离（Lazy Loading）
- **涉及模块**：
  - `src/agent/store.ts` 的 `restore()`
  - `src/agent/session/manager.ts`
- **方案**：
  1. 启动时仅加载各工作区的会话摘要（`SessionSummary[]`），侧边栏直接消费摘要进行渲染。
  2. 仅对初始恢复的 `active` 会话读取并还原完整 `items` 与 `messages`。
  3. 当用户在标签栏或侧边栏切换到其他会话时，按需异步加载对应会话的流水。
  4. 支持后台闲置会话卸载机制（LRU 换出到磁盘）。

---

### Phase 3：系统级内存主动回收（P2）

#### 3.1 空闲时主动触发垃圾回收并缩容
- **涉及模块**：
  - `src/agent/store.ts`
  - `src/platform/`
- **方案**：
  1. 在单轮 Agent Loop 彻底结束、或用户切换会话并处于空闲时，检测 `Bun.gc(true)` 可用性，适度触发一次完全垃圾回收。
  2. 促使 JSC 运行时收缩未使用的堆内存并将物理页释放回系统。

---

## 四、 验证与守门标准

每一步改动必须通过项目规定的双门禁：
1. **类型检查**：`bun run typecheck` 必须 exit 0。
2. **测试验证**：`bun test src/agent` 以及全量 `bun test` 必须无新增失败。
3. **内存指标观测**：
   - 使用内存对比脚本验证大模型流式生成 100 轮过程中的堆内存波动与快照大小。
   - 快照序列化体积降至原有的 5% 以下。

---

## 五、 实施计划安排

- [x] **步骤 0**：完成瓶颈诊断并编写本方案文档。
- [x] **步骤 1**：实施 Phase 1.1 —— 快照结构瘦身（会话元数据分离与非调试状态 log 轻量化）。
- [x] **步骤 2**：实施 Phase 1.2 —— Debug 日志延迟序列化与请求瘦身。
- [x] **步骤 3**：实施 Phase 1.3 —— CheckpointManager 内存治理（LRU 缓存限制）。
- [x] **步骤 4**：实施 Phase 1.4 —— Bash 缓冲滑动窗口截断（1MB 上限保护）。
- [x] **步骤 5**：实施 Phase 3.1 —— 空闲主动 GC 触发（JSC 物理内存归还）。
- [x] **步骤 6**：全量测试与门禁验证，确保无副作用（599 个 agent 测试与 159 个 UI 测试全部通过）。
- [ ] **步骤 7 (后续推进)**：实施 Phase 2.1 —— 历史会话启动级懒加载（Lazy Loading）。
