import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

const methods = new Set<string>()

function walk(dir: string) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name)
    if (entry.isDirectory()) {
      walk(full)
    } else if (/\.(ts|tsx)$/.test(entry.name)) {
      const content = readFileSync(full, 'utf8')
      const matches = content.matchAll(/client\.request\(['"]([^'"]+)['"]/g)
      for (const m of matches) {
        methods.add(m[1])
      }
    }
  }
}

walk('src/ui')
console.log('--- UI 调用的全部 RPC 方法 ---')
console.log(JSON.stringify(Array.from(methods).sort(), null, 2))
