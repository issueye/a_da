/**
 * 两个"打开文件选择器"的请求构造（把组件里的接线变成可确定性测试的东西）。
 *
 * ## 为什么单独抽出来
 *
 * 这两条接线（"添加工作区"、"选图片附件"）原本写在组件的事件处理里，只能靠**真窗口 + 点坐标**
 * 才能触发。而本仓库的 UI 测试有个硬约束：**同一时刻只让一个真窗口活着**——bun 会把各测试文件
 * 放在同一进程里并发跑，别的文件也开着窗口时，按坐标派发的 click 会落到别人的窗口上
 * （`AGENTS.md` §13 末尾；实测这两条用例单独跑绿、全量跑红，且诊断显示点击根本没到本窗口）。
 *
 * 所以接线抽到这里，用**不挂窗口**的测试覆盖：请求形状（模式/起始目录/类型过滤）是否正确、
 * `onPicked` 之后是否发出该发的命令。界面本身的浏览行为由 `FilePicker.test.tsx` 覆盖，
 * 主机侧的目录读写由 `src/agent/host/fs-service.test.ts` 覆盖。
 */

import type { AgentClient, FilePickerRequest } from './client'

/** "添加工作区"：选一个目录，确认后加进工作区并绑到当前会话。 */
export function directoryPickerRequest(client: AgentClient, startPath?: string): FilePickerRequest {
  return {
    mode: 'directory',
    title: '选择工作区目录',
    startPath,
    onPicked: ([path]) => {
      if (!path) return
      void client.request('workspace.add', { path }).then(({ error }) => {
        // 加不进去就到此为止：错误已经由主机说出来（`workspace.add` 的 error 文案），
        // 这里再绑一次会话只会把失败掩盖掉
        if (error) return
        void client.request('thread.setWorkspace', {
          threadId: client.state.active.id,
          workspace: path,
        })
      })
    },
  }
}

/** 选图片附件：按图片扩展名过滤，选中后把路径交给调用方（Composer 负责去重与落 state）。 */
export function imagePickerRequest(
  client: AgentClient,
  accept: RegExp,
  addPaths: (paths: string[]) => void,
  startPath?: string
): FilePickerRequest {
  return {
    mode: 'files',
    title: '选择图片',
    startPath: startPath ?? client.state.active.workspace,
    accept,
    onPicked: (paths) => addPaths(paths),
  }
}
