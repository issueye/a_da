/** @type {import('tailwindcss').Config} */
// plugin-hub 的设计令牌层：颜色全部指向 `static/input.css` 里的 CSS 变量，
// 这样亮/暗主题只在一处切换，模板里不再到处写 `dark:` 变体。
// 调色板与桌面端 `desktop-ui/` 对齐：zinc 中性灰 + 蓝色主色。
module.exports = {
  content: [
    "./crates/plugin-hub/templates/**/*.html",
    // Rust 侧的错误页直接拼 HTML 字符串（AppError::into_response），
    // 扫描源码才能保证它用到的工具类也被生成，否则错误页会掉样式。
    "./crates/plugin-hub/src/**/*.rs",
  ],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        canvas: "var(--ph-canvas)",
        surface: "var(--ph-surface)",
        elevated: "var(--ph-elevated)",
        line: {
          DEFAULT: "var(--ph-line)",
          strong: "var(--ph-line-strong)",
        },
        ink: "var(--ph-ink)",
        muted: "var(--ph-muted)",
        faint: "var(--ph-faint)",
        brand: {
          DEFAULT: "var(--ph-brand)",
          strong: "var(--ph-brand-strong)",
          soft: "var(--ph-brand-soft)",
          ink: "var(--ph-brand-ink)",
        },
        subtle: {
          DEFAULT: "var(--ph-subtle)",
          ink: "var(--ph-subtle-ink)",
        },
        success: {
          soft: "var(--ph-success-soft)",
          ink: "var(--ph-success-ink)",
        },
        warn: {
          soft: "var(--ph-warn-soft)",
          ink: "var(--ph-warn-ink)",
        },
        danger: {
          soft: "var(--ph-danger-soft)",
          ink: "var(--ph-danger-ink)",
        },
      },
      fontFamily: {
        sans: [
          "-apple-system",
          "BlinkMacSystemFont",
          '"Segoe UI"',
          "Roboto",
          '"Helvetica Neue"',
          "Arial",
          '"Microsoft YaHei"',
          "sans-serif",
        ],
        mono: [
          '"Cascadia Code"',
          '"JetBrains Mono"',
          "Consolas",
          '"Courier New"',
          "monospace",
        ],
      },
      boxShadow: {
        card: "var(--ph-shadow)",
      },
      maxWidth: {
        content: "72rem",
      },
    },
  },
  plugins: [],
};
