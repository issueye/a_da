/**
 * 自定义请求头的行为验收。
 *
 * 这里最要紧的一条是**大小写不敏感的覆盖**：HTTP 头名不区分大小写，朴素展开会
 * 让 `Authorization`（用户写的）与 `authorization`（默认的）**同时发出**，服务器
 * 取哪个由实现决定——这正是"我配了自定义头却不生效"的典型成因，而且不报错。
 */

import { describe, expect, test } from 'bun:test'
import { buildRequestHeaders } from './headers'

describe('请求头构造：默认行为不变', () => {
  test('无自定义头时就是 content-type + bearer', () => {
    expect(buildRequestHeaders({ apiKey: 'sk-abc' })).toEqual({
      'content-type': 'application/json',
      authorization: 'Bearer sk-abc',
    })
  })

  test('apiKey 为空或纯空白时不带 authorization', () => {
    // 带一个 `Bearer ` 会让某些网关报"凭证格式错误"，而真正原因是压根没配 key
    expect(buildRequestHeaders({ apiKey: '' })).toEqual({ 'content-type': 'application/json' })
    expect(buildRequestHeaders({ apiKey: '   ' })).toEqual({ 'content-type': 'application/json' })
    expect(buildRequestHeaders({})).toEqual({ 'content-type': 'application/json' })
  })
})

describe('请求头构造：自定义头', () => {
  test('追加自定义头，不影响默认头', () => {
    const headers = buildRequestHeaders({
      apiKey: 'sk-abc',
      headers: { 'X-Api-Key': 'custom', 'X-Org': 'team-a' },
    })
    expect(headers).toEqual({
      'content-type': 'application/json',
      authorization: 'Bearer sk-abc',
      'X-Api-Key': 'custom',
      'X-Org': 'team-a',
    })
  })

  test('同名覆盖是**大小写不敏感**的（同个头不会发出两份）', () => {
    const headers = buildRequestHeaders({
      apiKey: 'sk-abc',
      headers: { Authorization: 'Api-Key custom-token' },
    })
    // 默认的 authorization 必须被摘掉，否则会同时发两个头
    expect(headers.authorization).toBeUndefined()
    expect(headers.Authorization).toBe('Api-Key custom-token')
    expect(Object.keys(headers).filter((k) => k.toLowerCase() === 'authorization')).toHaveLength(1)
  })

  test('Content-Type 同样能被覆盖，且只留一份', () => {
    const headers = buildRequestHeaders({
      headers: { 'Content-Type': 'application/json; charset=utf-8' },
    })
    expect(Object.keys(headers).filter((k) => k.toLowerCase() === 'content-type')).toHaveLength(1)
    expect(headers['Content-Type']).toBe('application/json; charset=utf-8')
  })

  test('空名字或空值的项被丢弃（不会发出无名头）', () => {
    const headers = buildRequestHeaders({
      headers: { '  ': 'x', 'X-Empty': '   ', 'X-Ok': 'yes' },
    })
    expect(headers['X-Ok']).toBe('yes')
    expect(Object.keys(headers)).toHaveLength(2) // content-type + X-Ok
  })

  test('值两侧空白被裁掉（从 config.json 手写时常见）', () => {
    const headers = buildRequestHeaders({ headers: { 'X-Pad': '  spaced  ' } })
    expect(headers['X-Pad']).toBe('spaced')
  })
})
