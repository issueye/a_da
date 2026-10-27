# Git 工具（git-tools）

按 `action` 执行 Git 动作。**这是插件实现的工具**：基座（`agent-toolkit`）不再自带 git 实现，
插件通过 `@ada-needs fs,exec` 用 `node:fs` 探目录、用 `node:child_process` 起 `git` 子进程。

## 为什么是两个工具

| 工具 | 动作 | 访问类别 | 只读档 / plan 模式 |
|---|---|---|---|
| `git` | 全部 15 个动作（含 add/commit/push/reset…） | 写（未自述 `access`，宿主按 `External` 处理） | 不可用 |
| `git_read` | `status` / `diff` / `log` / `show` / `blame` | **只读**（`access: "read"`） | 可用 |

`git` 里混着 `commit` / `push` / `reset --hard` 这类真的会改仓库的动作，把整个工具标成只读
等于给它们开后门（自述只读会免掉写审批）。所以只读查询单独成一枚工具——
这就是同一件事拆两条的唯一理由。

## 参数

| 参数 | 说明 |
|---|---|
| `action` | 动作名（必填）；两个工具的可用动作见上表 |
| `file` / `path` | 单个文件（**文件过滤**/pathspec，不是仓库目录；`path` 是别名） |
| `files` | 多个文件（`add` / `restore` / `commit` 用） |
| `cwd` | 在哪个子目录执行 git（相对工作区）。**多仓库工作区必须用它指定仓库**，`repo` 是别名 |
| `ref` | 提交/分支/标签引用（默认 `HEAD`） |
| `branch` | 分支名（`branch` 创建/删除、`checkout` 目标、`push`/`pull` 目标分支） |
| `message` | 提交信息（`commit` 必填；`stash push` 可选；**不能含换行**） |
| `limit` | `log` 条数（默认 10，上限 200） |
| `staged` | `diff` 看暂存区；`restore` 从暂存区恢复 |
| `all` | `commit` 先暂存已跟踪文件的改动（`-a`） |
| `mode` | `reset` 模式（`soft` / `mixed` / `hard`，默认 `mixed`） |
| `sub` | `stash` 子操作（默认 `list`） |
| `remote` | `push` / `pull` 的远端名（默认 `origin`） |
| `create` / `delete` / `force` | `checkout -b` / `branch -d` / `push --force-with-lease` |

## 安全与健壮性

- **argv 直传、不经 shell**：提交信息、路径、ref 里都是用户输入，拼命令行会把
  `%VAR%` 展开、把 `&` 当分隔符。这里直接 `execFileSync("git", argv)`，没有一处转义需要猜。
- **入参校验失败即拒**：绝对路径、含 `..` 的路径、以 `-` 开头的 ref（`--upload-pack=…`
  这类"看着像值、其实是选项"的输入）都在插件侧挡住，不交给 git。
- **输出按 48 KB 截断**，并明确告诉模型怎么拿全文（缩小 `file` / 调低 `limit` / 改用 `read_file`）。
- **超时 30 秒**，超时杀整棵进程树；`GIT_TERMINAL_PROMPT=0` 让"要凭据"立刻失败而不是挂满超时。
- **多仓库工作区**：工作区根不是仓库时，提示会点名一层子目录里的独立仓库，并给出可照抄的 `cwd`。

## 结构

```text
plugin.json   # 只声明 backend（纯后端包），没有前端插槽
index.ts      # 工厂：注册 git（全动作）与 git_read（只读五动作）
```

## 验收

```bash
# 真插件 + 真沙箱 + 真 git 子进程
cargo test -p agent-node --test git_plugin
```

覆盖：声明期 `access` 归一化（`git`=write / `git_read`=read、只读工具里不含写动作）、
`git status` 真的跑出 `## <branch>`、`git_read` 拒绝写动作、越界参数被拒。

## 相关

- [../../docs/plugins/README.md](../../docs/plugins/README.md)（插件平台与自述只读的口径）
- [../../docs/plugins/unified-package.md](../../docs/plugins/unified-package.md)（统一包形状）
- `crates/agent-base/src/ports/scripts.rs` 的 `ScriptToolAccess`（自述只读的代价与边界）
