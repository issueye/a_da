//! xtask: a_da 仓自动化任务工作流（编译、出包、PE 补丁与完整性验证）

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
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
    Verify,
    /// 校验 TS 归档完整性（不被主干引用、无第二份引擎）
    VerifyArchive,
    /// 生产打包产品单文件可执行二进制
    Ship {
        /// 要打包的产品名称（如 ada-coding, ada-skeleton）
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Verify => {
            println!("==> 运行 cargo test 验证协议与合规性...");
            let status = Command::new("cargo")
                .args(["test", "--workspace", "--", "--test-threads=1"])
                .status()?;
            if !status.success() {
                anyhow::bail!("cargo test 失败");
            }
            println!("==> 运行 TS 归档校验...");
            run_bun_script("tools/verify-archive.ts")?;
            println!("\x1b[32m✔ 全仓 Verify 通过！\x1b[0m");
        }
        Commands::VerifyArchive => {
            println!("==> 校验 TS 归档完整性...");
            run_bun_script("tools/verify-archive.ts")?;
        }
        Commands::Ship { product, gui, out_dir } => {
            ship_product(&product, gui, &out_dir)?;
        }
    }

    Ok(())
}

fn run_bun_script(script: &str) -> anyhow::Result<()> {
    let status = Command::new("bun").arg(script).status()?;
    if !status.success() {
        anyhow::bail!("脚本 {} 执行失败", script);
    }
    Ok(())
}

fn ship_product(product: &str, gui: bool, out_dir: &str) -> anyhow::Result<()> {
    println!("\x1b[1;36m=================================================================\x1b[0m");
    println!("\x1b[1;36m           a_da 生产出包流水线 (Ship: {})                        \x1b[0m", product);
    println!("\x1b[1;36m=================================================================\x1b[0m\n");

    // 1. 编译 release 二进制
    println!("\x1b[33m[步骤 1/3]\x1b[0m 正在使用 Cargo 编译 release 二进制 (-p {})...", product);
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--release", "-p", product]);

    // 继承环境变量中的 CARGO_TARGET_DIR
    if let Ok(td) = std::env::var("CARGO_TARGET_DIR") {
        cmd.env("CARGO_TARGET_DIR", td);
    }

    let status = cmd.status()?;
    if !status.success() {
        anyhow::bail!("编译产品 {} 失败", product);
    }

    // 2. 查找产物并复制到 dist 目录
    println!("\x1b[33m[步骤 2/3]\x1b[0m 正在收集编译产物并复制至目标目录 ({})...", out_dir);
    fs::create_dir_all(out_dir)?;

    let exe_suffix = if cfg!(windows) { ".exe" } else { "" };
    let exe_name = format!("{}{}", product, exe_suffix);

    let candidate_dirs = vec![
        std::env::var("CARGO_TARGET_DIR").map(PathBuf::from).ok(),
        Some(PathBuf::from("target")),
        Some(PathBuf::from("../cargo_target_ada")),
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
