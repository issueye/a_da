/**
 * Start the app and keep the child process handle.
 *
 * `launch()` from the automation package does the same, but it hides the child,
 * and the window chrome checks need its pid: every user32 call there is addressed
 * by process id.
 *
 *   const { app, child } = await launchWithPid(['app.tsx'])
 */

import { spawn, type Subprocess } from 'bun'
import path from 'node:path'
import { connectStdio } from '@gpuix/react/automation'

const root = path.join(import.meta.dir, '..')

export async function launchWithPid(
  args: string[],
  env: Record<string, string | undefined> = { GPUIX_BACKGROUND: '1' },
): Promise<{ app: Awaited<ReturnType<typeof connectStdio>>; child: Subprocess<'pipe', 'pipe', 'inherit'> }> {
  const child = spawn(['bun', ...args], {
    cwd: root,
    stdin: 'pipe',
    stdout: 'pipe',
    stderr: 'inherit',
    env: { ...process.env, ...env },
  })

  const app = await connectStdio({
    write: (chunk) => {
      child.stdin.write(chunk)
      child.stdin.flush()
    },
    feed: (listener) => {
      void (async () => {
        const decoder = new TextDecoder()
        const reader = child.stdout.getReader()
        for (;;) {
          const { done, value } = await reader.read()
          if (done) break
          listener(decoder.decode(value, { stream: true }))
        }
      })()
    },
    close: async () => {
      child.kill()
    },
  })

  return { app, child }
}
