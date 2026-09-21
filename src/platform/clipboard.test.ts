import { describe, expect, test } from 'bun:test'
import { copyToClipboard } from './clipboard'

describe('clipboard helper', () => {
  test('returns false for empty text', async () => {
    const ok = await copyToClipboard('')
    expect(ok).toBe(false)
  })

  test('copies valid text or handles environment without throwing', async () => {
    const ok = await copyToClipboard('test clipboard content')
    expect(typeof ok).toBe('boolean')
  })
})
