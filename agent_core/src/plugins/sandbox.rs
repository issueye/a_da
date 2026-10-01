use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Result;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::types::PluginToolDeclaration;
use crate::tools::ToolResult;

const RUNNER_JS: &str = r#"
import { pathToFileURL } from 'node:url';

async function main() {
  const chunks = [];
  for await (const chunk of process.stdin) {
    chunks.push(chunk);
  }
  const input = JSON.parse(Buffer.concat(chunks).toString('utf-8'));
  const { action, pluginPath, toolName, args, workspace } = input;

  try {
    const mod = await import(pathToFileURL(pluginPath).href);
    const desc = mod.default || mod;

    if (action === 'inspect') {
      const tools = (desc.tools || []).map(t => {
        const inst = typeof t === 'function' ? t(workspace || process.cwd()) : t;
        return {
          name: inst.name,
          label: inst.label,
          description: inst.description,
          parameters: inst.parameters || {}
        };
      });
      process.stdout.write(JSON.stringify({ ok: true, tools }) + '\n');
      return;
    }

    if (action === 'call') {
      const toolFactory = (desc.tools || []).find(t => {
        const inst = typeof t === 'function' ? t(workspace || process.cwd()) : t;
        return inst.name === toolName;
      });

      if (!toolFactory) {
        process.stdout.write(JSON.stringify({ ok: false, error: `插件中未找到工具: ${toolName}` }) + '\n');
        return;
      }

      const inst = typeof toolFactory === 'function' ? toolFactory(workspace || process.cwd()) : toolFactory;
      const res = await inst.execute('call_sandbox', args || {});
      process.stdout.write(JSON.stringify({ ok: true, result: res }) + '\n');
      return;
    }

    process.stdout.write(JSON.stringify({ ok: false, error: `未知操作: ${action}` }) + '\n');
  } catch (err) {
    process.stdout.write(JSON.stringify({ ok: false, error: String(err && err.message ? err.message : err) }) + '\n');
  }
}

main().catch(err => {
  process.stderr.write(String(err) + '\n');
  process.exit(1);
});
"#;

pub struct PluginSandbox;

impl PluginSandbox {
    /// 在沙箱中检查插件暴露的工具列表
    pub async fn inspect_plugin(plugin_path: &Path, workspace: &Path) -> Result<Vec<PluginToolDeclaration>> {
        let input_json = serde_json::json!({
            "action": "inspect",
            "pluginPath": plugin_path.to_string_lossy(),
            "workspace": workspace.to_string_lossy(),
        });

        let output_str = Self::exec_sandbox(&input_json, 15).await?;
        let resp: serde_json::Value = serde_json::from_str(&output_str)?;

        if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            let tools: Vec<PluginToolDeclaration> = serde_json::from_value(
                resp.get("tools").cloned().unwrap_or(serde_json::json!([])),
            )?;
            Ok(tools)
        } else {
            let err = resp.get("error").and_then(|v| v.as_str()).unwrap_or("检查插件失败");
            anyhow::bail!("{}", err);
        }
    }

    /// 在沙箱中执行插件工具
    pub async fn call_tool(
        plugin_path: &Path,
        tool_name: &str,
        args: serde_json::Value,
        workspace: &Path,
        timeout_s: u64,
    ) -> Result<ToolResult> {
        let input_json = serde_json::json!({
            "action": "call",
            "pluginPath": plugin_path.to_string_lossy(),
            "toolName": tool_name,
            "args": args,
            "workspace": workspace.to_string_lossy(),
        });

        let output_str = Self::exec_sandbox(&input_json, timeout_s).await?;
        let resp: serde_json::Value = serde_json::from_str(&output_str)?;

        if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            let res: ToolResult = serde_json::from_value(
                resp.get("result").cloned().unwrap_or(serde_json::json!({ "ok": true, "output": "" })),
            )?;
            Ok(res)
        } else {
            let err = resp.get("error").and_then(|v| v.as_str()).unwrap_or("执行插件工具失败");
            Ok(ToolResult::error(err))
        }
    }

    async fn exec_sandbox(input: &serde_json::Value, timeout_s: u64) -> Result<String> {
        let mut cmd = Command::new("bun");
        cmd.args(["-e", RUNNER_JS])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        let mut child = cmd.spawn()?;
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("打开 stdin 失败"))?;
        let payload = serde_json::to_vec(input)?;

        let write_task = async move {
            stdin.write_all(&payload).await?;
            stdin.flush().await?;
            drop(stdin);
            Ok::<(), std::io::Error>(())
        };

        let wait_task = async {
            let output = child.wait_with_output().await?;
            Ok::<_, anyhow::Error>(output)
        };

        let timeout_duration = Duration::from_secs(timeout_s);

        tokio::select! {
            res = async {
                let _ = write_task.await;
                wait_task.await
            } => {
                let output = res?;
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if stdout.is_empty() && !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    anyhow::bail!("沙箱退出非零 ({}): {}", output.status, stderr);
                }
                Ok(stdout)
            }
            _ = tokio::time::sleep(timeout_duration) => {
                anyhow::bail!("插件沙箱执行超时（超过 {} 秒）", timeout_s);
            }
        }
    }
}
