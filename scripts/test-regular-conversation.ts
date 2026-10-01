import { spawnHostProcess } from '../src/ui/client/host-bootstrap';
import { createWebSocketClient } from '../src/ui/client/ws';

async function sleep(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function main() {
  console.log('[测试] 启动原生 agent_core 核心...');
  const host = await spawnHostProcess({
    execPath: 'E:\\codes\\rust_projects\\a_da\\dist\\agent_core.exe',
    compiled: true,
    timeoutMs: 10000,
  });
  console.log(`[测试] 核心监听端口: ${host.port}`);

  const client = createWebSocketClient({
    url: `ws://127.0.0.1:${host.port}`,
    token: host.token,
  });
  await client.ready();

  // 1. 创建全新的独立会话进行纯净测试
  console.log('\n[步骤 1] 创建全新常规对话会话...');
  const createRes = await client.request('thread.create', {
    workspace: process.cwd(),
    mode: 'code',
  }) as any;
  const threadId = createRes.id || createRes.threadId;
  console.log(`[步骤 1] 创建成功，threadId: ${threadId}`);

  // 打开并激活 Tab
  await client.request('ui.openTab', { threadId });

  // 辅助函数：发送消息并等待流式完成
  async function askAndCollect(turnNum: number, question: string) {
    console.log(`\n========================================`);
    console.log(`【轮次 ${turnNum}】发送提问: "${question}"`);
    console.log(`========================================`);

    await client.request('thread.send', { threadId, text: question });

    const startTime = Date.now();
    let streamChunks = 0;
    let lastSeenText = '';

    // 轮询检查是否完成（最长等待 45 秒）
    while (Date.now() - startTime < 45000) {
      await sleep(300);
      const snap = await client.request('session.snapshot', {}) as any;
      const running = snap.runningThreadIds.includes(threadId);

      const thread = snap.threads.find((t: any) => t.id === threadId);
      if (thread && thread.items.length > 0) {
        const lastItem = thread.items[thread.items.length - 1];
        if (lastItem.kind === 'assistant' || lastItem.role === 'assistant') {
          if (lastItem.text && lastItem.text !== lastSeenText) {
            streamChunks++;
            lastSeenText = lastItem.text;
            process.stdout.write('.');
          }
        }
      }

      if (!running) {
        console.log(`\n[轮次 ${turnNum}] 完成！总耗时: ${Date.now() - startTime}ms, 收到流式增量更新: ${streamChunks} 次`);
        console.log(`[回答内容预览]:\n${lastSeenText.slice(0, 300)}...`);
        if (lastSeenText.length > 300) {
          console.log(`... (全文共 ${lastSeenText.length} 字)`);
        }
        return { success: true, text: lastSeenText };
      }
    }

    console.error(`\n[轮次 ${turnNum}] ❌ 严重超时！45 秒内未完成，当前仍处于 running 状态！`);
    const finalSnap = await client.request('session.snapshot', {}) as any;
    console.error('[当前快照日志]:', finalSnap.log);
    return { success: false, text: '' };
  }

  // 轮次 1：自我介绍
  const r1 = await askAndCollect(1, '你好，请用一句话介绍你自己');
  if (!r1.success) process.exit(1);

  await sleep(1000);

  // 轮次 2：生成代码
  const r2 = await askAndCollect(2, '请用 Python 写一个简单的冒泡排序函数，包含注释');
  if (!r2.success) process.exit(2);

  await sleep(1000);

  // 轮次 3：上下文关联代码修改（这是最考验多轮上下文的地方！）
  const r3 = await askAndCollect(3, '把上面的冒泡排序改写为快速排序');
  if (!r3.success) process.exit(3);

  await sleep(1000);

  // 轮次 4：总结复杂度
  const r4 = await askAndCollect(4, '简要总结一下上面两个函数的时间和空间复杂度对比');
  if (!r4.success) process.exit(4);

  console.log('\n========================================');
  console.log('🎉 恭喜！连续 4 轮常规对话全部顺畅完成！');
  console.log('========================================');

  client.close();
  process.exit(0);
}

main().catch((err) => {
  console.error('测试异常中断:', err);
  process.exit(1);
});
