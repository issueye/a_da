# a_da 产品色板 · Black Swan（墨与宣纸）

| 项 | 值 |
|---|---|
| 日期 | 2026-10-24 |
| 输入 | `assets/logo.png`（512×512，黑天鹅 + 暖米白圆角底） |
| 性质 | **主题设计 + Phase 1 已落地**（ssr-h / plugin-hub，2026-10-24）；desktop-ui 为 Phase 2（见 §7） |
| 依据 | LOGO 实际取色、WCAG 对比度计算、现有 `design/tokens.css` 令牌结构 |

---

## 1. LOGO 取色分析

对 `assets/logo.png` 逐像素采样（4px 网格）：

| 颜色 | 占比 | 角色 |
|---|---|---|
| `#F6F3EC` | 约 96% | 背景：暖米白（略偏黄，非纯白） |
| `#16171A` / `#17181B` | 约 6% | 天鹅主体：近黑，**带一点冷调**（蓝灰而非暖黑） |
| `#000000` | 少量 | 轮廓最深处 |

两个事实决定了整套色板：

1. **这是单色 LOGO**——没有任何彩色元素。产品主色不该凭空发明一个蓝/绿；
   最能"长在 LOGO 上"的主色就是这枚**墨黑**，底色就是这块**暖米白**。
2. **黑是冷调、白是暖调**。这组冷暖对比本身就是识别度：纸暖、墨冷，界面要有"纸感"，
   不能退回中性灰（`#fafafa` / `#ffffff` 会把 LOGO 的暖意洗掉）。

---

## 2. 设计主张：墨是主角，纸是背景

- **品牌色 = 墨黑**（`#16171A`）：主按钮、发送键、焦点环、选中态都靠它，不靠彩色。
- **画布 = 宣纸暖白**（`#F6F3EC`，与 LOGO 底色完全一致）：大面积留白，界面像一张纸。
- **蓝色退场**：现有 `#2563eb` 与 LOGO 无关，且让"墨黑 + 米白"的识别度被稀释。
- **语义色保留但暖化**：成功 / 警告 / 危险仍用绿 / 琥珀 / 红，色相向暖侧收一点，
  保证在米白纸上不刺眼。
- **层级靠字重、留白与线，不靠色相**：单色品牌下，标题/正文/辅助靠 `ink → muted → faint`
  三级明度与字号字重区分。
- **点面色（可选，不进 UI）**：青铜 `#8C6239` 只用于品牌物料/插画点缀，不参与交互语义
  （避免与警告琥珀打架）。

暗色模式做**墨纸反转**：纸变暖黑，墨变暖白。主按钮在暗色下是"白纸黑字"，
不是把黑按钮调亮成灰。

---

## 3. 色板（Light）

| Token | 值 | 用途 |
|---|---|---|
| `--ph-canvas` | `#F6F3EC` | 页面画布（= LOGO 底色） |
| `--ph-surface` | `#FCFAF5` | 主区/卡片（比画布亮一档，仍带暖意） |
| `--ph-elevated` | `#FFFFFF` | 浮层/菜单/弹窗 |
| `--ph-line` | `#E6E0D4` | 普通分割线 |
| `--ph-line-strong` | `#D5CCBC` | 输入框/可交互边界 |
| `--ph-ink` | `#16171A` | 主文字（= LOGO 墨色） |
| `--ph-muted` | `#54524C` | 次级文字 |
| `--ph-faint` | `#6E6A60` | 辅助/占位（比原方案深一点，保证 4.5:1） |
| `--ph-brand` | `#16171A` | 主操作、焦点、选中 |
| `--ph-brand-strong` | `#000000` | hover/按下 |
| `--ph-brand-soft` | `#ECE5D8` | 选中底、品牌浅底（暖沙） |
| `--ph-brand-ink` | `#16171A` | 浅底上的品牌文字 |
| `--ph-on-brand` | `#F6F3EC` | **新增**：品牌底上的文字/图标 |
| `--ph-subtle` | `#F0EBE1` | 侧栏/次级表面 |
| `--ph-subtle-ink` | `#54524C` | 次级表面上的文字 |
| `--ph-success-soft` | `#E7F0E7` | 成功浅底 |
| `--ph-success-ink` | `#2F6B4A` | 成功文字 |
| `--ph-warn-soft` | `#F8EEDC` | 警告/等待批准浅底 |
| `--ph-warn-ink` | `#8A5A12` | 警告文字 |
| `--ph-danger-soft` | `#F8E7E4` | 危险/失败浅底 |
| `--ph-danger-ink` | `#A3342B` | 危险文字 |
| `--ph-glass` | `rgba(246, 243, 236, 0.84)` | 毛玻璃底（顶栏/flash） |
| `--ph-focus` | `rgba(22, 23, 26, 0.28)` | 3px 焦点光环 |
| `--ph-code-bg` | `#17181B` | 代码块（LOGO 墨色） |
| `--ph-code-ink` | `#EDE8DD` | 代码文字 |
| `--ph-inline-code-bg` | `#F0EBE1` | 行内代码底 |
| `--ph-inline-code-ink` | `#3B3A36` | 行内代码文字 |
| `--ph-shadow` | `0 1px 2px rgba(22,23,26,.05), 0 12px 32px -24px rgba(22,23,26,.22)` | 卡片阴影（暖黑投影） |

## 4. 色板（Dark：墨纸反转）

| Token | 值 | 说明 |
|---|---|---|
| `--ph-canvas` | `#121110` | 暖黑画布（带一点棕，不是纯黑） |
| `--ph-surface` | `#1A1917` | 主区/卡片 |
| `--ph-elevated` | `#22201D` | 浮层/菜单/弹窗 |
| `--ph-line` | `#2E2B27` | 分割线 |
| `--ph-line-strong` | `#433F38` | 输入框边界 |
| `--ph-ink` | `#F3EFE6` | 主文字（暖白，= 纸色提亮） |
| `--ph-muted` | `#B9B2A5` | 次级文字 |
| `--ph-faint` | `#928B7E` | 辅助文字 |
| `--ph-brand` | `#F3EFE6` | 主操作（白纸按钮） |
| `--ph-brand-strong` | `#FFFFFF` | hover |
| `--ph-brand-soft` | `rgba(243, 239, 230, 0.12)` | 选中底 |
| `--ph-brand-ink` | `#F0E7D6` | 浅底上的品牌文字 |
| `--ph-on-brand` | `#16171A` | 品牌底上的文字（深色） |
| `--ph-subtle` | `#201E1B` | 侧栏/次级表面 |
| `--ph-subtle-ink` | `#B9B2A5` | 次级表面文字 |
| `--ph-success-soft` | `#1B2A22` | 成功浅底 |
| `--ph-success-ink` | `#8FD3B0` | 成功文字 |
| `--ph-warn-soft` | `#2C2418` | 警告浅底 |
| `--ph-warn-ink` | `#E8C583` | 警告文字 |
| `--ph-danger-soft` | `#2E1D1B` | 危险浅底 |
| `--ph-danger-ink` | `#F0A59C` | 危险文字 |
| `--ph-glass` | `rgba(18, 17, 16, 0.82)` | 毛玻璃底 |
| `--ph-focus` | `rgba(243, 239, 230, 0.30)` | 焦点光环 |
| `--ph-code-bg` | `#0E0D0C` | 代码块 |
| `--ph-code-ink` | `#EDE8DD` | 代码文字 |
| `--ph-inline-code-bg` | `#26231F` | 行内代码底 |
| `--ph-inline-code-ink` | `#E6DFD2` | 行内代码文字 |
| `--ph-shadow` | `0 1px 2px rgba(0,0,0,.4), 0 12px 32px -24px rgba(0,0,0,.5)` | 暗色阴影 |

---

## 5. 对比度（WCAG 2.1，已计算）

| 组合 | 比值 | 结论 |
|---|---|---|
| 浅色 主文字 / 画布 | **16.17** | AAA |
| 浅色 次级文字 / 画布 | **7.05** | AAA |
| 浅色 辅助文字 / 画布 | **4.87** | AA（普通文字） |
| 浅色 品牌底文字 / 品牌底 | **16.17** | AAA |
| 浅色 品牌文字 / 品牌浅底 | **14.31** | AAA |
| 浅色 成功 / 成功浅底 | **5.42** | AA |
| 浅色 警告 / 警告浅底 | **5.14** | AA |
| 浅色 危险 / 危险浅底 | **5.69** | AA |
| 浅色 代码文字 / 代码底 | **14.53** | AAA |
| 暗色 主文字 / 画布 | **16.44** | AAA |
| 暗色 次级文字 / 画布 | **8.96** | AAA |
| 暗色 辅助文字 / 画布 | **5.58** | AA |
| 暗色 品牌底文字 / 品牌底 | **15.62** | AAA |
| 暗色 品牌文字 / 品牌浅底 | **12.11** | AAA |
| 暗色 成功 / 浅底 | **8.65** | AAA |
| 暗色 警告 / 浅底 | **9.29** | AAA |
| 暗色 危险 / 浅底 | **8.08** | AAA |
| 暗色 代码文字 / 代码底 | **15.89** | AAA |

> 焦点环不是文字，按 3:1 的非文本对比要求：浅色 `rgba(22,23,26,.28)` 叠在米白上约 1.6:1，
> **不够**。落地时焦点环保持"2px 品牌描边 + 3px 低透明光环"的组合（描边提供 3:1 边界），
> 不要只留柔光环。

---

## 6. 组件映射规则

| 组件 | 规则 | 备注 |
|---|---|---|
| 主按钮 / 发送键 | `background: brand` + `color: on-brand` | 浅色=黑底米白字；暗色=米白底墨字（与现有 send 语义一致） |
| 次按钮 | `surface` + `line-strong` + `ink` | 不变 |
| 选中项 / 当前会话 | `brand-soft` 底 + `brand-ink` 字 | 不再用蓝色浅底 |
| 焦点环 | 2px `brand` 描边 + 3px `focus` 光环 | 文本框/卡片同口径（UED §7.5） |
| 行内链接 | `ink` + 下划线（或 `brand-ink` + 下划线） | 单色品牌下链接必须靠下划线区分，不能只靠颜色 |
| 运行中 / 等待批准 | `warn-soft` / `warn-ink` | 保留琥珀，是"等待"的唯一彩色信号 |
| 失败 / 危险 | `danger-soft` / `danger-ink` | 保留红，但偏砖红（`#A3342B`）不刺眼 |
| 代码块 | `code-bg` / `code-ink` | 直接用 LOGO 墨色 |
| 主题色 / 品牌 mark | `ink` | 侧栏品牌位、空状态菱形标记都用墨色 |

**不要做的事**：不要把品牌黑拿去铺大面积背景（会变成"黑色主题"而不是"纸感"）；
不要给主按钮加彩色渐变；不要用青铜色做可点击元素。

---

## 7. 落地范围与步骤

### 落地状态（2026-10-24）

**Phase 1 已完成**（`ssr-h` + `plugin-hub`，共享令牌）：

- `design/tokens.css`：深浅两套值全部替换，新增 `--ph-on-brand`（暗色主按钮的前提）；
- `ssr-h/static/components.css`：主按钮与发送键的 `color: #fff` 改为 `var(--ph-on-brand)`，
  发送键阴影从蓝色 `rgba(37,99,235,.24)` 改为 `color-mix(var(--ph-brand))`；
- `frontend-plugins/provider-admin`、`ask-user`：`--pa-accent` 与按钮文字改走令牌；
- Tailwind 产物已重建并同步（`crates/plugin-hub/static/app.css` = `ssr-h/static/app.css`）；
- 验收：`cargo test` 184 + 13（含深浅令牌覆盖与产物一致性门）、
  浅/深 × 空会话/有内容截图（`target/repro/theme-{light,dark}-{empty,thread}.png`），对比度见 §5。

**Phase 2 待做**（desktop-ui）：见下方 `desktop-ui` 行。它有自己的 `tailwind.config.js`
与 29 个文件里的 `blue-*` / 大量 `zinc-*` 工具类，不能用同一份 `--ph-*` 直接替换，
需要按语义收敛（建议先主操作/选中态，再装饰性细节），每批单独截图验收。

### 消费方现状

| 消费方 | 颜色来源 | 改动点 |
|---|---|---|
| `plugin-hub` / `ssr-h` | `design/tokens.css`（唯一真源）→ Tailwind 构建 `app.css` | 改令牌 + 重新构建并同步产物 |
| `desktop-ui` | 自己的 `tailwind.config.js`（`primary: #3b82f6`）+ 29 个文件里的 `blue-*` 工具类 | 改 `primary` 配置 + 批量替换 `blue-*`（或用 `primary` 语义类） |
| 二进制/图标 | `assets/logo.png` / `.ico` | 不改 |

### 步骤

1. **改真源**：`design/tokens.css` 按 §3/§4 替换 `--ph-*`，新增 `--ph-on-brand`。
2. **改硬编码**：`ssr-h/static/components.css` 里 `.ada-btn--primary` 的 `color: #fff`、
   `.ada-send` 的 `color: #ffffff` 与 `box-shadow: rgba(37,99,235,.24)` 改成
   `var(--ph-on-brand)` / `color-mix(in srgb, var(--ph-brand) 24%, transparent)`；
   plugin-hub 同类处一并检查。
3. **重建产物**：按 `design/tokens.css` 头部命令重建 Tailwind 输出，并同步到
   `ssr-h/static/app.css`（`cargo test` 会校验两份一致）。
4. **desktop-ui 跟随**：`tailwind.config.js` 的 `primary.DEFAULT/hover` 改为墨色/纯黑，
   把 29 个文件里的 `blue-*` 收敛到 `primary` 语义色（建议一次只改主操作与选中态，
   再改装饰性用法）。
5. **验收**：
   - `cargo test --workspace -- --test-threads=1`（含 `ui_scale` 的令牌/标尺门）
   - 深浅主题逐页截图（聊天页 / 空会话 / 审批 / 供应商插件页 / 工作区弹窗）
   - 对比度抽检（§5 表）、焦点态、禁用态、代码块、Toast/Flash
   - 窄屏（500/650px）与触屏 hover 补丁不回归

### 风险与取舍

- **单色品牌的层级风险**：去掉蓝色后，"可点"与"强调"的区分更依赖字重、下划线与留白；
  行内链接务必带下划线。
- **暗色反转需要新令牌**：`--ph-on-brand` 是必须项，否则主按钮在暗色下白字压白底。
- **语义色不可被主色吞掉**：主按钮变黑后，危险按钮必须是砖红实底或红描边，
  不能也做成黑色（否则"删除"和"保存"长得一样）。
- **迁移面积**：desktop-ui 的 `blue-*` 有 29 个文件，建议分两批（主操作/选中态 → 装饰性细节），
  每批单独截图验收。

---

## 8. 一句话总结

**主调 = LOGO 的墨黑 `#16171A` + 宣纸暖白 `#F6F3EC`；蓝色彩色退场，语义色暖化保留。
浅色是"纸上落墨"，暗色是"墨底透纸"，主按钮两端反转、靠 `--ph-on-brand` 保证可读。**
