//! 真实应用目录：**全仓唯一**允许读 `A_DA_HOME` / `A_DA_CONFIG` / `USERPROFILE` / `HOME` 的地方。
//!
//! 这段逻辑原先长在 `agent_core::session::manager::get_app_home()` 里，被 10 多处直接调用；
//! 搬到适配器 + 端口之后，测试可以注入 [`agent_base::testing::TempAppHome`]，
//! "单元测试写用户真实的 `~/.a-da`"这个问题才有解。

use std::path::{Path, PathBuf};

use agent_base::ports::AppHome;

#[derive(Debug, Clone)]
pub struct SystemAppHome {
    root: PathBuf,
    config: PathBuf,
}

impl SystemAppHome {
    /// 从进程环境解析（与搬迁前的行为逐字一致）。
    pub fn from_env() -> Self {
        let root = match std::env::var("A_DA_HOME") {
            Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir.trim()),
            _ => match std::env::var("USERPROFILE") {
                Ok(userprofile) => PathBuf::from(userprofile).join(".a-da"),
                Err(_) => match std::env::var("HOME") {
                    Ok(home) => PathBuf::from(home).join(".a-da"),
                    Err(_) => PathBuf::from(".a-da"),
                },
            },
        };

        let config = match std::env::var("A_DA_CONFIG") {
            Ok(cfg) if !cfg.trim().is_empty() => PathBuf::from(cfg.trim()),
            _ => root.join("config.json"),
        };

        Self { root, config }
    }

    /// 显式指定根目录（配置文件随之推导）。
    pub fn at(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let config = root.join("config.json");
        Self { root, config }
    }
}

impl AppHome for SystemAppHome {
    fn root(&self) -> &Path {
        &self.root
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn config_file(&self) -> PathBuf {
        self.config.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_root_derives_config_path() {
        let home = SystemAppHome::at("/tmp/ada");
        assert_eq!(home.root(), Path::new("/tmp/ada"));
        assert_eq!(home.config_file(), PathBuf::from("/tmp/ada/config.json"));
        assert_eq!(home.dir("secrets"), PathBuf::from("/tmp/ada/secrets"));
        assert_eq!(home.dir("extensions"), PathBuf::from("/tmp/ada/extensions"));
    }
}
