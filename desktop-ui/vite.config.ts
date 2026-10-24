import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// https://vitejs.dev/config/
export default defineConfig({
  base: './',
  // 品牌资源单一真源：仓库根的 `assets/`（logo.png / logo.ico）。
  // 开发时以站点根提供，打包时复制到 dist 根——不在 desktop-ui 里放第二份。
  publicDir: '../assets',
  plugins: [
    react(),
    {
      name: 'remove-crossorigin',
      transformIndexHtml(html: string) {
        return html.replace(/ crossorigin/g, '')
      },
    },
  ],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: ['es2021', 'chrome105', 'safari13'],
    minify: 'esbuild',
    sourcemap: false,
  },
})
