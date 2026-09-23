import { afterEach, describe, expect, test } from 'bun:test'
import {
  setNotificationHandler,
  showCompletionNotification,
} from './notification'

describe('notification helper', () => {
  afterEach(() => {
    setNotificationHandler(null)
  })

  test('calls injected notification handler when configured', async () => {
    const received: any[] = []
    setNotificationHandler((opts) => {
      received.push(opts)
      return true
    })

    const ok = await showCompletionNotification({
      title: '任务完成：测试项目',
      body: '所有代码修改已验证通过。',
      threadId: 't-123',
    })

    expect(ok).toBe(true)
    expect(received.length).toBe(1)
    expect(received[0].title).toBe('任务完成：测试项目')
    expect(received[0].threadId).toBe('t-123')
  })

  test('returns true in test environment when A_DA_NO_DIALOG=1 is set', async () => {
    const ok = await showCompletionNotification({
      title: '测试',
      body: '测试内容',
    })
    expect(ok).toBe(true)
  })
})
