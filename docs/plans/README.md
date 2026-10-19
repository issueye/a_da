# 计划与历史归档

本目录**只保留历史决策的入口**，不再维护逐日执行记录。

| 文件 | 内容 | 现行替代 |
|---|---|---|
| [wiring.md](wiring.md) | W0–W6 接线收口记录（已精简为归档说明） | [../quality/unfinished-features.md](../quality/unfinished-features.md) |
| [milestones.md](milestones.md) | MVP 分期与执行基线（已精简为归档说明） | [../quality/feature-catalog.md](../quality/feature-catalog.md) |

完整正文从 git 历史取回：

```bash
git log --all -- docs/plans/wiring.md
git log --all -- docs/plans/milestones.md
```

新增功能不要再写进这两个文件；现状与缺口分别维护在 `quality/` 下。