import { spawnHostProcess } from '../src/ui/client/host-bootstrap';
import { createWebSocketClient } from '../src/ui/client/ws';
import type { AgentMode } from '../src/agent/types';

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

  console.log('\n[步骤 1] 校验初始模式...');
  console.log(`当前客户端 mode: ${client.state.mode}`);
  const initialSnap = await client.request('session.snapshot', {}) as any;
  console.log(`初始快照 config.mode: ${initialSnap.config.mode}`);

  // 1. 测试切换到 create 创造模式
  console.log('\n[步骤 2] 发送 thread.setMode 切换到 create 模式...');
  await client.request('thread.setMode', { mode: 'create' as AgentMode });

  // 等待快照推送或轮询确认
  let modeConfirmed = false;
  for (let i = 0; i < 30; i++) {
    await sleep(50);
    const snap = await client.request('session.snapshot', {}) as any;
    if (snap.config.mode === 'create') {
      modeConfirmed = true;
      console.log(`✔ [服务端快照确认] config.mode 已成功变为: ${snap.config.mode}`);
      const activeThread = snap.threads.find((t: any) => t.id === snap.activeThreadId);
      console.log(`✔ [会话模式确认] 当前会话 thread.mode 已变为: ${activeThread?.mode}`);
      break;
    }
  }

  if (!modeConfirmed) {
    console.error('❌ 切换到 create 模式失败！服务端快照未更新为 create');
    process.exit(1);
  }

  // 2. 测试切换到 plan 规划模式
  console.log('\n[步骤 3] 发送 thread.setMode 切换到 plan 模式...');
  await client.request('thread.setMode', { mode: 'plan' as AgentMode });
  await sleep(100);
  const planSnap = await client.request('session.snapshot', {}) as any;
  if (planSnap.config.mode !== 'plan') {
    console.error(`❌ 切换到 plan 模式失败！当前 mode: ${planSnap.config.mode}`);
    process.exit(2);
  }
  console.log(`✔ [成功] config.mode 已变为: ${planSnap.config.mode}`);

  // 3. 测试切换回 code 编码模式
  console.log('\n[步骤 4] 发送 thread.setMode 切换回 code 模式...');
  await client.request('thread.setMode', { mode: 'code' as AgentMode });
  await sleep(100);
  const codeSnap = await client.request('session.snapshot', {}) as any;
  if (codeSnap.config.mode !== 'code') {
    console.error(`❌ 切换回 code 模式失败！当前 mode: ${codeSnap.config.mode}`);
    process.exit(3);
  }
  console.log(`✔ [成功] config.mode 已变为: ${codeSnap.config.mode}`);

  console.log('\n========================================');
  console.log('🎉 模式切换端到端测试全部通过！code ↔ create ↔ plan 无缝切换');
  console.log('========================================');

  client.close();
  process.exit(0);
}

main().catch((err) => {
  console.error('测试异常中断:', err);
  process.exit(1);
});
