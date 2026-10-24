/**
 * 产品品牌资源。
 *
 * LOGO 的唯一真源是仓库根目录 `assets/logo.png`（黑天鹅 + 暖米白底）；
 * `vite.config.ts` 的 `publicDir: '../assets'` 把它暴露到站点根目录，
 * 开发与打包（file://）都用相对 BASE_URL 取，不复制第二份文件。
 */
export const LOGO_URL = `${import.meta.env.BASE_URL}logo.png`
