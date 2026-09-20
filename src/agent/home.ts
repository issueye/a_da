/**
 * 应用数据目录。
 *
 * 配置、会话流水、全局扩展都落在这一个目录下——一个应用不该有两个家。
 * `A_DA_HOME` 可以换掉它（测试就靠这个不碰你的真实目录）；配置文件另有
 * `A_DA_CONFIG` 指向单个文件。
 */

import { homedir } from 'node:os'
import { join } from 'node:path'

export function getAppHome(): string {
  return process.env.A_DA_HOME || join(homedir(), '.a-da')
}
