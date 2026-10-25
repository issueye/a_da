//! xtask: a_da 仓自动化任务工作流（编译、出包、PE 补丁与完整性验证）

mod gates;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "a_da 内部开发与出包工具集")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 执行全仓单测与协议/合规检验
    Verify {
        /// 快速模式：仅执行静态架构校验（spec、wiring、compat、docs），跳过耗时的全仓单测
        #[arg(long, default_value_t = false)]
        fast: bool,

        /// 单测最长等待时间（秒，防止死锁卡死），默认 600 秒
        #[arg(long, default_value_t = 600)]
        timeout_secs: u64,
    },
    /// 校验产品声明 ↔ 实现一致（toolkit 有模块、字段有生产消费者）
    VerifySpec,
    /// 校验跨产品 base 协议一致（base 唯一、base ∪ ext 无交集）
    Compat,
    /// 接线结构审计（方法覆盖 / 内置目录单一真源 / 描述符可达 / 端口有生产实现）
    VerifyWiring,
    /// 文档时效性审计（主干 markdown 不得把归档布局当现行路径）
    VerifyDocs,
    /// 生产打包产品单文件可执行二进制
    Ship {
        /// 要打包的产品名称（如 ada-coding, ada-gateway, ada-electron 或 all）
        #[arg(long, default_value = "ada-coding")]
        product: String,

        /// 是否应用 Windows GUI 子系统补丁（消除控制台黑框）
        #[arg(long, default_value_t = false)]
        gui: bool,

        /// 自定义输出目录
        #[arg(long, default_value = "dist")]
        out_dir: String,
    },
}

/// 打印报告并按违约数决定退出码（0 = 转绿）。
fn gate(rep: anyhow::Result<gates::Report>) -> anyhow::Result<()> {
    let violations = rep?.print();
    if violations > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// 执行子命令，具备环境注入、实时流式输出与超时看门狗，彻底避免无声无限等待
fn run_command_supervised(mut cmd: Command, desc: &str, timeout: Duration) -> anyhow::Result<()> {
    // 1. 自动注入编译缓存目录与并发度限制（若环境变量未显式提供）
    if std::env::var("CARGO_TARGET_DIR").is_err() {
        cmd.env("CARGO_TARGET_DIR", "../cargo_target_ada");
    }
    if std::env::var("CARGO_BUILD_JOBS").is_err() {
        cmd.env("CARGO_BUILD_JOBS", "2");
    }

    // 2. 保证流式直通终端，消除黑盒假死感
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    println!("\x1b[36m==> [{desc}] 开始执行 (超时上限: {}s)...\x1b[0m", timeout.as_secs());
    let start = Instant::now();

    let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("启动子进程失败 [{desc}]: {e}"))?;

    // 3. 超时看门狗轮询，防止子进程死锁或无限挂起
    let poll_interval = Duration::from_millis(200);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let elapsed = start.elapsed();
                if status.success() {
                    println!("\x1b[32m✔ [{desc}] 执行成功 (耗时 {:.1}s)\x1b[0m\n", elapsed.as_secs_f32());
                    return Ok(());
                } else {
                    anyhow::bail!("任务 [{desc}] 失败，退出码: {:?}", status.code());
                }
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    anyhow::bail!("任务 [{desc}] 执行超时（超过 {}s 未退出），已强制终止", timeout.as_secs());
                }
                std::thread::sleep(poll_interval);
            }
            Err(e) => {
                let _ = child.kill();
                anyhow::bail!("监控子进程异常 [{desc}]: {e}");
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Verify { fast, timeout_secs } => {
            let total_start = Instant::now();
            println!("\x1b[1;35m=================================================================\x1b[0m");
            println!("\x1b[1;35m              a_da 全仓综合检验 (Verify Pipeline)                 \x1b[0m");
            println!("\x1b[1;35m=================================================================\x1b[0m\n");

            // 阶段 1: 静态高速架构审计 (秒级完成，fail-fast)
            println!("\x1b[33m[阶段 1/5]\x1b[0m 校验产品声明 (verify-spec)...");
            gate(gates::verify_spec())?;

            println!("\x1b[33m[阶段 2/5]\x1b[0m 校验接线与单源描述符 (verify-wiring)...");
            gate(gates::verify_wiring())?;

            println!("\x1b[33m[阶段 3/5]\x1b[0m 校验跨产品协议一致性 (compat)...");
            gate(gates::compat())?;

            println!("\x1b[33m[阶段 4/5]\x1b[0m 校验文档时效性 (verify-docs)...");
            gate(gates::verify_docs())?;

            // 阶段 2: 全仓单元测试
            if fast {
                println!("\x1b[33m[阶段 5/5]\x1b[0m \x1b[90m已启用 --fast，跳过全量单测 (cargo test)\x1b[0m\n");
            } else {
                println!("\x1b[33m[阶段 5/5]\x1b[0m 运行全仓 Rust 单元测试 (串行 --test-threads=1)...");
                let mut cmd = Command::new("cargo");
                cmd.args(["test", "--workspace", "-j", "2", "--", "--test-threads=1"]);
                run_command_supervised(cmd, "cargo test --workspace", Duration::from_secs(timeout_secs))?;
            }

            println!("\x1b[1;32m=================================================================\x1b[0m");
            println!("\x1b[1;32m       🎉 全仓 Verify 检验全绿通过！ (总耗时 {:.1}s)            \x1b[0m", total_start.elapsed().as_secs_f32());
            println!("\x1b[1;32m=================================================================\x1b[0m\n");
        }
        Commands::VerifySpec => gate(gates::verify_spec())?,
        Commands::Compat => gate(gates::compat())?,
        Commands::VerifyWiring => gate(gates::verify_wiring())?,
        Commands::VerifyDocs => gate(gates::verify_docs())?,
        Commands::Ship { product, gui, out_dir } => {
            ship_product(&product, gui, &out_dir)?;
        }
    }

    Ok(())
}

fn ship_product(product: &str, gui: bool, out_dir: &str) -> anyhow::Result<()> {
    if product == "all" {
        for p in &["ada-coding", "ada-gateway", "ada-electron"] {
            ship_product(p, gui, out_dir)?;
        }
        return Ok(());
    }

    if product == "ada-electron" {
        println!("\x1b[1;36m=================================================================\x1b[0m");
        println!("\x1b[1;36m           a_da 生产出包流水线 (Ship: ada-electron)              \x1b[0m");
        println!("\x1b[1;36m=================================================================\x1b[0m\n");

        println!("\x1b[33m[步骤 1/2]\x1b[0m 检查并构建前端静态产物...");
        let frontend_dist = Path::new("desktop-ui").join("dist");
        if !frontend_dist.exists() || !frontend_dist.join("index.html").exists() {
            let npm_cmd = if cfg!(windows) {
                let mut c = Command::new("cmd");
                c.args(["/C", "npm", "--prefix", "desktop-ui", "run", "build"]);
                c
            } else {
                let mut c = Command::new("npm");
                c.args(["--prefix", "desktop-ui", "run", "build"]);
                c
            };
            run_command_supervised(npm_cmd, "npm run build (desktop-ui)", Duration::from_secs(180))?;
        }

        println!("\x1b[33m[步骤 2/2]\x1b[0m 执行 Electron 构建并打包目录 (electron:pack)...");
        let pack_cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "bun", "run", "electron:pack"]);
            c
        } else {
            let mut c = Command::new("bun");
            c.args(["run", "electron:pack"]);
            c
        };
        run_command_supervised(pack_cmd, "bun run electron:pack", Duration::from_secs(300))?;

        #[cfg(windows)]
        {
            let win_unpacked = Path::new("dist/electron/win-unpacked");
            if win_unpacked.exists() {
                println!("\x1b[33m[权限自愈]\x1b[0m 注入 Medium 完整性级别与沙箱访问权限...");
                let _ = Command::new("icacls")
                    .args([win_unpacked.to_str().unwrap(), "/setintegritylevel", "(CI)(OI)M", "/T"])
                    .status();
                let _ = Command::new("icacls")
                    .args([win_unpacked.to_str().unwrap(), "/grant", "*S-1-15-2-2:(OI)(CI)(RX)", "/T"])
                    .status();
            }
        }

        println!("\n\x1b[1;32m=================================================================\x1b[0m");
        println!("\x1b[1;32m       🎉 产品 ada-electron 交付完成！ (dist/electron/win-unpacked) \x1b[0m");
        println!("\x1b[1;32m=================================================================\x1b[0m\n");
        return Ok(());
    }

    let (pkg, bin) = match product {
        "ada-gateway" => ("agent-gateway", Some("ada-gateway")),
        _ => (product, None),
    };

    println!("\x1b[1;36m=================================================================\x1b[0m");
    println!("\x1b[1;36m           a_da 生产出包流水线 (Ship: {})                        \x1b[0m", product);
    println!("\x1b[1;36m=================================================================\x1b[0m\n");

    // 1. 编译 release 二进制
    println!("\x1b[33m[步骤 1/3]\x1b[0m 正在使用 Cargo 编译 release 二进制 (-p {})...", pkg);
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--release", "-p", pkg]);
    if let Some(b) = bin {
        cmd.args(["--bin", b]);
    }
    // 控制编译并发为 2，防止 Windows 页面文件上限 2GB 触发 os error 1455
    cmd.args(["-j", "2"]);

    // 单个产品 release 编译设置 900 秒看门狗超时
    run_command_supervised(
        cmd,
        &format!("cargo build --release -p {pkg}"),
        Duration::from_secs(900),
    )?;

    // 2. 查找产物并复制到 dist 目录
    println!("\x1b[33m[步骤 2/3]\x1b[0m 正在收集编译产物并复制至目标目录 ({})...", out_dir);
    fs::create_dir_all(out_dir)?;

    let exe_suffix = if cfg!(windows) { ".exe" } else { "" };
    let exe_name = format!("{}{}", product, exe_suffix);

    let candidate_dirs = vec![
        std::env::var("CARGO_TARGET_DIR").map(PathBuf::from).ok(),
        Some(PathBuf::from("../cargo_target_ada")),
        Some(PathBuf::from("target")),
    ];

    let mut found_src: Option<PathBuf> = None;
    for dir in candidate_dirs.into_iter().flatten() {
        let p = dir.join("release").join(&exe_name);
        if p.exists() {
            found_src = Some(p);
            break;
        }
    }

    let src_path = match found_src {
        Some(p) => p,
        None => anyhow::bail!("未在任何目标目录中找到编译产物: {}", exe_name),
    };

    let target_path = Path::new(out_dir).join(&exe_name);
    fs::copy(&src_path, &target_path)?;
    let file_size = fs::metadata(&target_path)?.len();
    let size_mb = (file_size as f64) / 1024.0 / 1024.0;
    println!("  ✔ 单文件产物就绪: {} ({:.2} MB)", target_path.display(), size_mb);

    // 网关内置 SSR 界面时，示例前端插件随产物分发（模板与静态资源已编译进二进制）。
    if product == "ada-gateway" {
        let plugins_src = Path::new("crates/agent-web/frontend-plugins");
        if plugins_src.is_dir() {
            let plugins_dst = Path::new(out_dir).join("frontend-plugins");
            copy_dir_all(plugins_src, &plugins_dst)?;
            println!("  ✔ 前端插件目录: {}", plugins_dst.display());
        }
    }

    // 3. 可选应用 Windows PE GUI 子系统补丁（消除控制台黑框）
    if cfg!(windows) && gui {
        println!("\x1b[33m[步骤 3/3]\x1b[0m 正在配置 Windows GUI 子系统补丁 (WINDOWS_GUI 2)...");
        patch_windows_pe_gui_subsystem(&target_path)?;
    } else {
        println!("\x1b[33m[步骤 3/3]\x1b[0m 保持标准控制台/宿主模式 (Console 子系统)...");
    }

    println!("\n\x1b[1;32m=================================================================\x1b[0m");
    println!("\x1b[1;32m                   🎉 产品 {} 交付完成！                         \x1b[0m", product);
    println!("\x1b[1;32m=================================================================\x1b[0m");
    println!("• 交付二进制路径: {}", target_path.display());
    println!("• 交付体积: {:.2} MB", size_mb);
    println!("• 运行说明: 命令行直接运行可查看标准就绪行，带参数启动宿主服务。\n");

    Ok(())
}

/// 递归复制目录（出包时携带示例插件）。目标已存在时覆盖同名文件。
fn copy_dir_all(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// 补丁 Windows PE 子系统为 WINDOWS_GUI (2)，消灭控制台黑框
fn patch_windows_pe_gui_subsystem(path: &Path) -> anyhow::Result<()> {
    let mut bytes = fs::read(path)?;
    if bytes.len() < 0x200 || bytes[0] != b'M' || bytes[1] != b'Z' {
        println!("  [跳过] 非合法 PE 二进制文件");
        return Ok(());
    }

    let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into()?) as usize;
    if &bytes[pe_offset..pe_offset + 4] != b"PE\0\0" {
        println!("  [跳过] PE 签名不匹配");
        return Ok(());
    }

    let subsystem_offset = pe_offset + 4 + 20 + 68;
    if subsystem_offset + 2 > bytes.len() {
        println!("  [跳过] 文件长度不足以包含 Subsystem 字段");
        return Ok(());
    }

    let current = u16::from_le_bytes(bytes[subsystem_offset..subsystem_offset + 2].try_into()?);
    if current != 2 {
        bytes[subsystem_offset..subsystem_offset + 2].copy_from_slice(&2u16.to_le_bytes());
        fs::write(path, bytes)?;
        println!("  ✔ 已成功修改 PE Subsystem 为 WINDOWS_GUI (2) 无控制台黑框");
    } else {
        println!("  ✔ PE Subsystem 已是 WINDOWS_GUI (2)");
    }

    Ok(())
}
