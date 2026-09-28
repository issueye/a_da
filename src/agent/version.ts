/**
 * 应用版本，以及插件 `engines.a_da` 声明范围的比对。
 *
 * 这里不 import `package.json`：tsconfig 没开 resolveJsonModule，而且模块图越简单，
 * 单文件打包越不容易出意外。代价是版本号有两处，`version.test.ts` 是守门测试
 * （读 package.json 比对），改名或升版本时它会红。
 */

/** 必须与 package.json 的 version 一致。 */
export const APP_VERSION = '0.1.0'

/** 解析 `1.2.3`（允许 `1` / `1.2` 这种省略写法），非法返回 undefined。 */
export function parseVersion(raw: string): [number, number, number] | undefined {
  const matched = /^v?(\d+)(?:\.(\d+))?(?:\.(\d+))?$/.exec(raw.trim())
  if (!matched) return undefined
  return [Number(matched[1]), Number(matched[2] ?? 0), Number(matched[3] ?? 0)]
}

function compare(a: [number, number, number], b: [number, number, number]): number {
  for (let index = 0; index < 3; index++) {
    if (a[index] !== b[index]) return a[index]! < b[index]! ? -1 : 1
  }
  return 0
}

/**
 * 版本是否落在声明范围内。
 *
 * 只支持插件声明里真正会用到的几种写法：`*`、精确 `1.2.3`、比较符 `>` `>=` `<` `<=`、
 * `^1.2.3`（同主版本）、`~1.2.3`（同主次版本），以及空格分隔的多条件
 * （`>=1.2.3 <2.0.0`，全部满足才算符合）。
 *
 * 解析不了时返回 `'unparsable'` 而不是 false：调用方对"看不懂"的处理是**照常加载并
 * 给出诊断**。版本声明是软约束，看不懂不该让插件失效——这与"失败安全"的取向一致：
 * 这里拦错的代价（一个能用的插件被判死）大于放过的代价（版本提示不够精确）。
 */
export function satisfiesRange(version: string, range: string): boolean | 'unparsable' {
  const parsed = parseVersion(version)
  if (!parsed) return 'unparsable'

  const raw = range.trim()
  if (!raw || raw === '*' || raw === 'x' || raw === 'latest') return true

  for (const clause of raw.split(/\s+/).filter(Boolean)) {
    const matched = /^(\^|~|>=|<=|>|<|=)?\s*(.+)$/.exec(clause)
    if (!matched) return 'unparsable'
    const operator = matched[1] ?? '='
    const target = parseVersion(matched[2]!)
    if (!target) return 'unparsable'

    const order = compare(parsed, target)
    const ok =
      operator === '='
        ? order === 0
        : operator === '>'
          ? order > 0
          : operator === '>='
            ? order >= 0
            : operator === '<'
              ? order < 0
              : operator === '<='
                ? order <= 0
                : operator === '^'
                  ? parsed[0] === target[0] && order >= 0
                  : // `~`：同主次版本且不低于目标
                    parsed[0] === target[0] && parsed[1] === target[1] && order >= 0
    if (!ok) return false
  }
  return true
}
