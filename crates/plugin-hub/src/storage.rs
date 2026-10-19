use std::io::Cursor;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use zip::ZipArchive;

#[derive(Debug, Clone)]
pub struct Storage {
    root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct StoredAsset {
    pub relative_path: String,
    pub size: u64,
    pub sha256: String,
}

const MAX_ZIP_ENTRIES: usize = 500;
const MAX_UNCOMPRESSED_BYTES: u64 = 50 * 1024 * 1024;

impl Storage {
    pub fn new(root: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 保存插件 ZIP。路径组件先经过安全校验，避免 `../` 落到数据目录之外。
    pub async fn save_plugin_zip(
        &self,
        owner: &str,
        slug: &str,
        version: &str,
        bytes: &[u8],
    ) -> Result<StoredAsset, String> {
        if !safe_component(owner) || !safe_component(slug) || !safe_component(version) {
            return Err("非法的 owner/slug/version 路径组件".to_string());
        }
        validate_zip(bytes)?;

        let dir = self.root.join(owner).join(slug);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| format!("创建资产目录失败: {e}"))?;
        let file = dir.join(format!("{version}.zip"));
        tokio::fs::write(&file, bytes)
            .await
            .map_err(|e| format!("写入资产失败: {e}"))?;

        let sha256 = hex::encode(Sha256::digest(bytes));
        Ok(StoredAsset {
            relative_path: format!("{owner}/{slug}/{version}.zip"),
            size: bytes.len() as u64,
            sha256,
        })
    }

    pub async fn read(&self, relative_path: &str) -> std::io::Result<Vec<u8>> {
        let path = self.resolve(relative_path)?;
        tokio::fs::read(path).await
    }

    /// 回滚用：DB 写入失败时把刚落盘的 ZIP 删掉，避免孤儿文件。
    pub async fn remove_asset(&self, relative_path: &str) {
        if let Ok(path) = self.resolve(relative_path) {
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    pub async fn remove_plugin_dir(&self, owner: &str, slug: &str) {
        if !safe_component(owner) || !safe_component(slug) {
            return;
        }
        let _ = tokio::fs::remove_dir_all(self.root.join(owner).join(slug)).await;
    }

    fn resolve(&self, relative_path: &str) -> std::io::Result<PathBuf> {
        let mut path = self.root.clone();
        for part in relative_path.split('/') {
            if part.is_empty() || part == "." || part == ".." || part.contains('\\') {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "非法资产路径",
                ));
            }
            path.push(part);
        }
        Ok(path)
    }
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && value != "."
        && value != ".."
}

/// 只做**校验**，不解压到磁盘：拒绝 zip-slip、超量条目、超大解压体积与符号链接。
pub fn validate_zip(bytes: &[u8]) -> Result<(), String> {
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("不是合法 ZIP: {e}"))?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(format!("ZIP 条目过多（上限 {MAX_ZIP_ENTRIES}）"));
    }
    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let file = archive
            .by_index(i)
            .map_err(|e| format!("读取 ZIP 条目失败: {e}"))?;
        let name = file.name();
        // ZIP 里的目录条目以 `/` 结尾；去掉尾部斜杠再按段校验，
        // 否则 `src/` 会被误判成空路径段。
        let normalized = name.trim_end_matches('/');
        if normalized.is_empty()
            || name.starts_with('/')
            || name.contains('\\')
            || name.contains(':')
            || normalized
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(format!("ZIP 含非法路径: {name}"));
        }
        if file
            .unix_mode()
            .map(|mode| mode & 0o170000 == 0o120000)
            .unwrap_or(false)
        {
            return Err(format!("ZIP 不允许符号链接: {name}"));
        }
        total = total.saturating_add(file.size());
        if total > MAX_UNCOMPRESSED_BYTES {
            return Err("ZIP 解压后体积超过上限（50 MB）".to_string());
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(path: &str) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            zip.start_file(path, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"export default {}").unwrap();
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn directory_entry_is_not_a_traversal() {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            zip.add_directory("src", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.start_file("src/index.ts", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"export default {}").unwrap();
            zip.finish().unwrap();
        }
        assert!(validate_zip(&buf.into_inner()).is_ok());
    }

    #[test]
    fn zip_slip_is_rejected() {
        assert!(validate_zip(&zip_with("../evil.ts")).is_err());
        assert!(validate_zip(&zip_with("/abs.ts")).is_err());
        assert!(validate_zip(&zip_with("a\\b.ts")).is_err());
    }
}
