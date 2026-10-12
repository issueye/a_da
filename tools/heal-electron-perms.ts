import { execSync } from 'child_process'
import * as fs from 'fs'
import * as path from 'path'

if (process.platform === 'win32') {
  const target = path.resolve('dist/electron/win-unpacked')
  if (fs.existsSync(target)) {
    try {
      execSync(`icacls "${target}" /setintegritylevel "(CI)(OI)M" /T`, { stdio: 'ignore' })
      execSync(`icacls "${target}" /grant "*S-1-15-2-2:(OI)(CI)(RX)" /T`, { stdio: 'ignore' })
      console.log('✔ 已成功注入并自愈 Electron 打包目录权限 (Medium Mandatory Level & ALL_APPLICATION_PACKAGES)')
    } catch (e) {
      console.warn('权限自愈执行失败 (非致命):', e)
    }
  }
}
