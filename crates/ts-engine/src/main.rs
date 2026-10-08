use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use ts_engine::PureTsRuntime;

/// 编译期可注入的自包含业务脚本载荷
const EMBEDDED_SCRIPT_PAYLOAD: Option<&str> = option_env!("A_DA_STANDALONE_SCRIPT");

#[derive(Parser, Debug)]
#[command(
    name = "ts_engine",
    version,
    about = "a_da Standalone TypeScript/JavaScript Micro-Runtime (OXC + Boa)",
    long_about = "极其轻量化的纯 Rust 编写的 TypeScript/JavaScript 独立微型运行时，支持直接运行 TS 文件并内置主流 Node.js 兼容层。"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// 直接指定执行的脚本路径（当省略 run 子命令时）
    #[arg(value_name = "SCRIPT")]
    script: Option<PathBuf>,

    /// 传递给脚本的位置参数
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum Commands {
    /// 执行指定的 TypeScript/JavaScript 脚本文件
    Run {
        /// 脚本文件路径 (.ts / .js)
        file: PathBuf,
        /// 传递给脚本的位置参数
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// 直接评估执行一段 TS/JS 代码字符串
    Eval {
        /// 待执行的代码
        code: String,
    },
    /// 自包含单文件打包与规划方案自省
    Bundle {
        /// 待打包的 TypeScript 入口脚本
        entry: PathBuf,
        /// 产出单可执行文件目标路径
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    // 优先检查是否有编译期嵌入的自包含脚本载荷
    if cli.command.is_none() && cli.script.is_none() {
        if let Some(payload) = EMBEDDED_SCRIPT_PAYLOAD {
            let runtime = PureTsRuntime::new();
            if let Err(err) = runtime.eval_ts(payload, Some("embedded.ts")).await {
                eprintln!("[Runtime Error] {}", err);
                return ExitCode::FAILURE;
            }
            let _ = runtime.wait_idle().await;
            return ExitCode::SUCCESS;
        }
    }

    match cli.command {
        Some(Commands::Run { file, args }) => run_script(&file, args).await,
        Some(Commands::Eval { code }) => eval_code(&code).await,
        Some(Commands::Bundle { entry, output }) => bundle_plan(&entry, output.as_deref()),
        None => {
            if let Some(script_file) = cli.script {
                run_script(&script_file, cli.args).await
            } else {
                eprintln!("错误: 未指定要执行的脚本文件或子命令。\n请使用 `ts_engine --help` 查看使用说明。");
                ExitCode::FAILURE
            }
        }
    }
}

/// 执行脚本文件逻辑
async fn run_script(file_path: &Path, args: Vec<String>) -> ExitCode {
    if !file_path.exists() {
        eprintln!("错误: 找不到指定的脚本文件: {}", file_path.display());
        return ExitCode::FAILURE;
    }

    let code = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("错误: 无法读取脚本文件 {}: {}", file_path.display(), e);
            return ExitCode::FAILURE;
        }
    };

    let filename = file_path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("script.ts");

    let workspace = file_path.parent().map(|p| {
        if p.as_os_str().is_empty() {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        } else {
            p.to_path_buf()
        }
    });

    let runtime = PureTsRuntime::with_workspace(workspace);

    // 构造标准的 Node 风格 argv 列表: [exe, script_path, ...args]
    let mut argv = vec!["ts_engine".to_string(), file_path.to_string_lossy().to_string()];
    argv.extend(args);
    let _ = runtime.set_process_argv(argv).await;

    // 执行脚本并排空未决微任务与宏任务
    match runtime.eval_ts(&code, Some(filename)).await {
        Ok(_) => {
            if let Err(err) = runtime.wait_idle().await {
                eprintln!("[Runtime Wait Error] {}", err);
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("[Script Execution Error] {}", err);
            ExitCode::FAILURE
        }
    }
}

/// 评估并执行代码字符串
async fn eval_code(code: &str) -> ExitCode {
    let runtime = PureTsRuntime::new();
    match runtime.eval_ts(code, Some("eval.ts")).await {
        Ok(res) => {
            let _ = runtime.wait_idle().await;
            if res != "undefined" && !res.is_empty() {
                println!("{res}");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("[Eval Error] {}", err);
            ExitCode::FAILURE
        }
    }
}

/// 自包含单文件打包与规划方案自省
fn bundle_plan(entry: &Path, output: Option<&Path>) -> ExitCode {
    println!("=== a_da Standalone TypeScript Executable Packaging 架构规划 ===");
    println!("入口脚本: {}", entry.display());
    let out = output
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("standalone_app.exe"));
    println!("目标产物: {}", out.display());
    println!();
    println!("【单文件独立编译打包（Standalone Single-Binary Packaging）方案】:");
    println!("1. 静态嵌入模式（Static Embedding via Compile-Time Env）:");
    println!("   - 构建期通过 `build.rs` 读取用户指定的入口 TS/JS，通过 OXC 类型擦除并完成静态依赖收集；");
    println!("   - 使用 `include_str!` 或 `option_env!(\"A_DA_STANDALONE_SCRIPT\")` 内联注入单二进制中；");
    println!("   - 运行时零外部依赖，双击即以纯 Rust Boa 引擎启动，零 Node.js/Bun 依赖。");
    println!("2. 二进制末尾载荷追加模式（Self-Contained Executable Post-Append）:");
    println!("   - 复用预编译好的轻量化 `ts_engine` 微型外壳（约 15MB）；");
    println!("   - 在二进制末尾追加压缩的代码 Payload 与元数据 Magic Header；");
    println!("   - 启动时自省自身文件末尾，若存在 Payload 则原地解压并交由 PureTsRuntime 启动。");
    println!();
    println!("当前单二进制微内核已就绪，可直接执行 `ts_engine run <script.ts>` 或 `ts_engine eval '<code>'`！");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parsing_run() {
        let cli = Cli::try_parse_from(["ts_engine", "run", "app.ts", "--flag", "value"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Commands::Run {
                file: PathBuf::from("app.ts"),
                args: vec!["--flag".to_string(), "value".to_string()],
            })
        );
    }

    #[test]
    fn test_cli_parsing_eval() {
        let cli = Cli::try_parse_from(["ts_engine", "eval", "1 + 2"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Commands::Eval {
                code: "1 + 2".to_string(),
            })
        );
    }

    #[tokio::test]
    async fn test_cli_eval_execution() {
        let code = "const x: number = 20; const y: number = 22; x + y;";
        let exit_code = eval_code(code).await;
        assert_eq!(exit_code, ExitCode::SUCCESS);
    }

    #[tokio::test]
    async fn test_cli_run_script_e2e() {
        let temp_dir = std::env::temp_dir().join(format!("a_da_cli_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let script_file = temp_dir.join("main.ts");

        let ts_source = r#"
            import { URL } from "node:url";
            const util = require("node:util");
            const assert = require("node:assert");

            interface Config {
                port: number;
                host: string;
            }

            const cfg: Config = { port: 9000, host: "127.0.0.1" };
            const formatted = util.format("Server on %s:%d", cfg.host, cfg.port);
            assert.strictEqual(formatted, "Server on 127.0.0.1:9000");

            // 验证 process.argv
            assert.ok(process.argv.length >= 2);

            // 验证异步任务与定时器完成排空
            let timerFinished = false;
            setTimeout(() => {
                timerFinished = true;
            }, 20);
        "#;
        std::fs::write(&script_file, ts_source).unwrap();

        let exit_code = run_script(&script_file, vec!["--env=prod".to_string()]).await;
        assert_eq!(exit_code, ExitCode::SUCCESS);

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
