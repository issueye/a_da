# Slang：见 [SLANG-V2](SLANG-V2.md)

> **旧语法已移除（2026-10-21）**。现行语言、组件模型、运行期与安全设计见
> **[SLANG-V2.md](SLANG-V2.md)**；脚本层见 **[SLANG-SCRIPT.md](SLANG-SCRIPT.md)**。
>
> 不再支持的写法：
>
> - `@slang 1`（只支持 `slang/2`）
> - `@def Name(...)` 参数列表（props 用 `@props { ... }`）
> - `@script setup`、`defineProps` / `defineEmits` / `emit`
> - `ref` / `computed` / `provide` / `inject` / lambda
> - 组合组件裸子节点填 `body` 插槽（默认插槽固定为 `default`）
> - `plugin.json` 的 `"slang": 1`
>
> 拒绝原因与新写法见 [SLANG-V2.md §11](SLANG-V2.md)。
