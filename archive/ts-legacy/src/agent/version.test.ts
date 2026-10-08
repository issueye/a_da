/**
 * 版本号与 `engines.a_da` 的比对。
 *
 * `APP_VERSION` 是抄在代码里的常量（不 import package.json，见 `version.ts` 的说明），
 * 所以这里钉住它和 package.json 一致——两处不一致时应当是这测试红，而不是等到
 * 某个插件因为版本判定错误被标成 incompatible 才发现。
 */

import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { APP_VERSION, parseVersion, satisfiesRange } from './version'

describe('APP_VERSION', () => {
  test('与 package.json 的 version 一致', () => {
    const pkg = JSON.parse(
      readFileSync(join(import.meta.dir, '..', '..', 'package.json'), 'utf8'),
    ) as { version: string }
    expect(APP_VERSION).toBe(pkg.version)
  })

  test('本身是个能解析的版本号', () => {
    expect(parseVersion(APP_VERSION)).not.toBeUndefined()
  })
})

describe('parseVersion', () => {
  test('接受 x / x.y / x.y.z，允许 v 前缀', () => {
    expect(parseVersion('1')).toEqual([1, 0, 0])
    expect(parseVersion('1.2')).toEqual([1, 2, 0])
    expect(parseVersion('v1.2.3')).toEqual([1, 2, 3])
    expect(parseVersion(' 2.0.1 ')).toEqual([2, 0, 1])
  })

  test('非版本号返回 undefined', () => {
    expect(parseVersion('latest')).toBeUndefined()
    expect(parseVersion('1.2.3-beta')).toBeUndefined()
    expect(parseVersion('')).toBeUndefined()
  })
})

describe('satisfiesRange', () => {
  test('星号与空范围视为兼容', () => {
    expect(satisfiesRange('1.2.3', '*')).toBe(true)
    expect(satisfiesRange('1.2.3', '')).toBe(true)
  })

  test('精确版本要完全相等', () => {
    expect(satisfiesRange('1.2.3', '1.2.3')).toBe(true)
    expect(satisfiesRange('1.2.4', '1.2.3')).toBe(false)
  })

  test('大小比较符', () => {
    expect(satisfiesRange('1.2.3', '>=1.2.3')).toBe(true)
    expect(satisfiesRange('1.2.2', '>=1.2.3')).toBe(false)
    expect(satisfiesRange('2.0.0', '>1.9.9')).toBe(true)
    expect(satisfiesRange('1.0.0', '<2.0.0')).toBe(true)
    expect(satisfiesRange('2.0.0', '<2.0.0')).toBe(false)
  })

  test('^ 锁主版本、~ 锁主次版本', () => {
    expect(satisfiesRange('1.9.0', '^1.2.3')).toBe(true)
    expect(satisfiesRange('2.0.0', '^1.2.3')).toBe(false)
    expect(satisfiesRange('1.2.9', '~1.2.3')).toBe(true)
    expect(satisfiesRange('1.3.0', '~1.2.3')).toBe(false)
  })

  test('空格分隔的多条件要全部满足', () => {
    expect(satisfiesRange('1.5.0', '>=1.2.3 <2.0.0')).toBe(true)
    expect(satisfiesRange('2.0.0', '>=1.2.3 <2.0.0')).toBe(false)
  })

  test('看不懂的范围返回 unparsable，而不是 false', () => {
    // 版本声明是软约束：看不懂时调用方照常加载并给诊断，不该把插件判死
    expect(satisfiesRange('1.2.3', 'latest-ish')).toBe('unparsable')
    expect(satisfiesRange('1.2.3', '>=1.x')).toBe('unparsable')
    expect(satisfiesRange('not-a-version', '>=1.0.0')).toBe('unparsable')
  })
})
