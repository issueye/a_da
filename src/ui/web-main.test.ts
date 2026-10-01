import { describe, expect, test } from 'bun:test'
import { mountWebApp } from './web-main'

describe('Web / H5 端入口模块', () => {
  test('在非浏览器环境下安全空转，不抛出异常', () => {
    expect(() => mountWebApp('root')).not.toThrow()
  })
})
