//! 插件市场客户端：配置、搜索与安装。
//!
//! 放在桥接面（而不是 `agent-node`）的理由：
//! - 网络 IO 属于宿主/平台适配，节点层保持"无协议管道、无网络"；
//! - `agent-rpc` 已经有 `reqwest`，市场只是把它用在一个新的只读 API 上；
//! - 安装后的重装配由 dispatch 层统一触发（与 `plugin.delete` / `plugin.createTemplate` 同一套）。
//!
//! 安全边界：
//! - Token 只存在 `~/.a-da/plugin_hub.json`（本地文件），界面只回显末 4 位；
//! - 下载包按 ZIP 结构白名单解压（拒绝 `..`、绝对路径、反斜杠、符号链接、超量条目/体积）；
//! - 安装目录固定为 `~/.a-da/agent-coding/extensions/<slug>`，目录名先做字符白名单校验。

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zip::ZipArchive;

/// 与 plugin-hub 的单包限制保持一致。
const MAX_ZIP_ENTRIES: usize = 500;
const MAX_UNCOMPRESSED_BYTES: u64 = 50 * 1024 * 1024;
const MAX_DOWNLOAD_BYTES: u64 = 50 * 1024 * 1024;

const CONFIG_FILE: &str = "plugin_hub.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketConfig {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_token: String,
}

impl MarketConfig {
    pub fn path(home: &Path) -> PathBuf {
        home.join(CONFIG_FILE)
    }

    /// 配置损坏时退回默认值（不让一个坏 JSON 把整个插件市场锁死）。
    pub fn load(home: &Path) -> Self {
        let path = Self::path(home);
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub async fn save(&self, home: &Path) -> Result<()> {
        tokio::fs::create_dir_all(home)
            .await
            .with_context(|| format!("创建配置目录失败: {}", home.display()))?;
        let path = Self::path(home);
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(self)?;
        tokio::fs::write(&tmp, body)
            .await
            .with_context(|| format!("写入配置失败: {}", tmp.display()))?;
        tokio::fs::rename(&tmp, &path)
            .await
            .with_context(|| format!("替换配置失败: {}", path.display()))?;
        Ok(())
    }

    pub fn has_token(&self) -> bool {
        !self.api_token.trim().is_empty()
    }

    /// 只回显末 4 位：设置页可以显示"已配置"，但不会把 token 再吐回界面/日志。
    pub fn token_hint(&self) -> String {
        let chars: Vec<char> = self.api_token.trim().chars().collect();
        if chars.is_empty() {
            return String::new();
        }
        let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();
        format!("…{tail}")
    }

    pub fn normalized_base(&self) -> Result<String> {
        normalize_base_url(&self.base_url)
    }
}

fn normalize_base_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        bail!("请先配置插件库地址（例如 http://127.0.0.1:8787）");
    }
    let url = reqwest::Url::parse(trimmed).map_err(|e| anyhow!("插件库地址不合法: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("插件库地址只支持 http:// 或 https://");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("插件库地址不能带查询串或锚点");
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketConfigView {
    pub base_url: String,
    pub has_token: bool,
    pub token_hint: String,
}

impl From<&MarketConfig> for MarketConfigView {
    fn from(config: &MarketConfig) -> Self {
        Self {
            base_url: config.base_url.clone(),
            has_token: config.has_token(),
            token_hint: config.token_hint(),
        }
    }
}

/// 保存配置。`api_token` 留空表示"保持原 Token"；`clear_token` 用于显式清除。
pub async fn update_config(
    home: &Path,
    base_url: &str,
    api_token: Option<&str>,
    clear_token: bool,
) -> Result<MarketConfig> {
    let mut config = MarketConfig::load(home);
    config.base_url = base_url.trim().trim_end_matches('/').to_string();
    if let Some(token) = api_token.map(str::trim).filter(|token| !token.is_empty()) {
        config.api_token = token.to_string();
    }
    if clear_token {
        config.api_token.clear();
    }
    if !config.base_url.is_empty() {
        normalize_base_url(&config.base_url)?;
    }
    config.save(home).await?;
    Ok(config)
}

// ─────────────────────────────────────────────────────────────────────────
// 市场 API
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketPlugin {
    pub owner: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub version: Option<String>,
    pub has_release: bool,
    pub tags: Vec<String>,
    pub downloads: i64,
    pub stars: i64,
    pub updated_at: i64,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketSearchResult {
    pub base_url: String,
    pub plugins: Vec<MarketPlugin>,
    pub total: i64,
}

#[derive(Debug, Deserialize)]
struct HubMarketResponse {
    #[serde(default)]
    plugins: Vec<HubMarketPlugin>,
    #[serde(default)]
    total: i64,
}

#[derive(Debug, Deserialize)]
struct HubMarketPlugin {
    owner: String,
    slug: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    has_release: bool,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    downloads: i64,
    #[serde(default)]
    stars: i64,
    #[serde(default)]
    updated_at: i64,
    #[serde(default)]
    metadata: Value,
}

impl From<HubMarketPlugin> for MarketPlugin {
    fn from(raw: HubMarketPlugin) -> Self {
        let author = if raw.author.trim().is_empty() {
            raw.metadata
                .get("author")
                .and_then(Value::as_str)
                .unwrap_or(&raw.owner)
                .to_string()
        } else {
            raw.author
        };
        Self {
            owner: raw.owner,
            slug: raw.slug,
            name: raw.name,
            description: raw.description,
            author,
            version: raw.version.clone(),
            has_release: raw.has_release || raw.version.is_some(),
            tags: raw.tags,
            downloads: raw.downloads,
            stars: raw.stars,
            updated_at: raw.updated_at,
            metadata: raw.metadata,
        }
    }
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(90))
        .user_agent(concat!("a-da-market/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("创建 HTTP 客户端失败")
}

fn market_error(status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    match status.as_u16() {
        401 | 403 => anyhow!(
            "插件库拒绝了请求：API Token 无效，或缺少所需作用域（浏览需 read，发布需 write）"
        ),
        404 => anyhow!("插件库地址不对：这个地址上没有市场接口（需要新版 plugin-hub）"),
        _ => {
            let detail: String = body.chars().take(200).collect();
            anyhow!("插件库返回 HTTP {status}: {detail}")
        }
    }
}

async fn send_checked(request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    let response = request
        .send()
        .await
        .map_err(|e| anyhow!("无法连接插件库: {e}"))?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    Err(market_error(status, &body))
}

pub async fn search(
    home: &Path,
    query: &str,
    limit: i64,
    offset: i64,
) -> Result<MarketSearchResult> {
    let config = MarketConfig::load(home);
    let base = config.normalized_base()?;
    if !config.has_token() {
        bail!("请先在插件市场设置里填写 API Token（plugin-hub 的 /settings/tokens 可创建）");
    }
    let limit = limit.clamp(1, 100);
    let offset = offset.max(0);

    let response = send_checked(
        http_client()?
            .get(format!("{base}/api/v1/market/plugins"))
            .bearer_auth(config.api_token.trim())
            .query(&[
                ("q", query.trim().to_string()),
                ("limit", limit.to_string()),
                ("offset", offset.to_string()),
            ]),
    )
    .await?;
    let body: HubMarketResponse = response
        .json()
        .await
        .map_err(|e| anyhow!("插件库返回的市场数据格式不对: {e}"))?;

    Ok(MarketSearchResult {
        base_url: base,
        plugins: body.plugins.into_iter().map(MarketPlugin::from).collect(),
        total: body.total,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPlugin {
    pub plugin_id: String,
    pub version: String,
    pub index_path: String,
    pub replaced: bool,
}

/// 从插件库下载指定版本并安装到全局扩展目录（`~/.a-da/agent-coding/extensions/<slug>`）。
pub async fn install(
    home: &Path,
    owner: &str,
    slug: &str,
    version: &str,
    overwrite: bool,
) -> Result<InstalledPlugin> {
    let owner = validate_component(owner, "owner")?;
    let slug = validate_component(slug, "slug")?.to_ascii_lowercase();
    let version = validate_component(version, "version")?;
    let config = MarketConfig::load(home);
    let base = config.normalized_base()?;
    if !config.has_token() {
        bail!("请先在插件市场设置里填写 API Token");
    }

    let url = format!("{base}/plugins/{owner}/{slug}/releases/{version}/download");
    let response =
        send_checked(http_client()?.get(url).bearer_auth(config.api_token.trim())).await?;
    let bytes = read_limited(response, MAX_DOWNLOAD_BYTES).await?;

    let extensions = home.join("agent-coding").join("extensions");
    let target = extensions.join(&slug);
    let replaced = target.exists();
    // 解压是同步文件 IO，放到阻塞线程池：WS 读取循环是逐条 await 的，
    // 一个几十 MB 的包不应该把界面后续请求一起堵住。
    let slug_for_install = slug.clone();
    let extensions_for_install = extensions.clone();
    let index_path = tokio::task::spawn_blocking(move || {
        install_zip(
            &bytes,
            &extensions_for_install,
            &slug_for_install,
            overwrite,
        )
    })
    .await
    .map_err(|e| anyhow!("安装任务异常退出: {e}"))??;
    Ok(InstalledPlugin {
        plugin_id: slug,
        version: version.to_string(),
        index_path: index_path.to_string_lossy().to_string(),
        replaced,
    })
}

// ─────────────────────────────────────────────────────────────────────────
// 上传/发布到 plugin-hub
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PublishRequest<'a> {
    /// 插件包目录（含包根 `index.ts` / `index.js`）
    pub package_dir: &'a Path,
    pub slug: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub tags: &'a str,
    pub license: &'a str,
    pub readme: &'a str,
    /// 留空 = 采用包内 metadata.version（hub 会校验并回填）
    pub version: &'a str,
    pub notes: &'a str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishedPlugin {
    pub owner: String,
    pub slug: String,
    pub version: String,
    pub url: String,
    /// false = 新建了插件仓库；true = 仓库已存在、本次只更新元数据并发布新版本
    pub existing: bool,
}

#[derive(Debug, Deserialize)]
struct HubEnsurePluginResponse {
    owner: String,
    slug: String,
    #[serde(default)]
    existing: bool,
}

#[derive(Debug, Deserialize)]
struct HubReleaseResponse {
    version: String,
}

/// 把一个本地插件包目录打包上传到 plugin-hub，并发布一个版本。
///
/// 先 `POST /api/v1/plugins?upsert=1`（创建或更新仓库元数据），
/// 再 `POST .../releases`（multipart ZIP）。Token 需要 **write** 作用域。
pub async fn publish(home: &Path, request: PublishRequest<'_>) -> Result<PublishedPlugin> {
    let slug = request.slug.trim().to_ascii_lowercase();
    if slug.len() < 2
        || slug.len() > 64
        || !slug
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        bail!("slug 只能包含小写字母、数字与连字符（2–64 位）");
    }
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        bail!("插件名称需在 1–80 个字符之间");
    }
    let config = MarketConfig::load(home);
    let base = config.normalized_base()?;
    if !config.has_token() {
        bail!("请先在插件市场设置里填写 API Token（发布需要 write 作用域）");
    }
    let token = config.api_token.trim().to_string();
    let client = http_client()?;

    // 打包（同步文件 IO）放到阻塞线程池
    let package_dir = request.package_dir.to_path_buf();
    let zip_bytes = tokio::task::spawn_blocking(move || zip_package(&package_dir))
        .await
        .map_err(|e| anyhow!("打包任务异常退出: {e}"))??;

    let create_body = serde_json::json!({
        "slug": slug,
        "name": name,
        "description": request.description.trim(),
        "readme": request.readme,
        "homepage": "",
        "license": request.license.trim(),
        "tags": request.tags,
    });
    let ensure = send_checked(
        client
            .post(format!("{base}/api/v1/plugins?upsert=1"))
            .bearer_auth(&token)
            .json(&create_body),
    )
    .await?;
    let ensured: HubEnsurePluginResponse = ensure
        .json()
        .await
        .map_err(|e| anyhow!("插件库返回的仓库信息格式不对: {e}"))?;

    let part = reqwest::multipart::Part::bytes(zip_bytes)
        .file_name(format!("{slug}.zip"))
        .mime_str("application/zip")
        .map_err(|e| anyhow!("构造上传内容失败: {e}"))?;
    let form = reqwest::multipart::Form::new()
        .text("version", request.version.trim().to_string())
        .text("notes", request.notes.trim().to_string())
        .part("asset", part);
    let release = send_checked(
        client
            .post(format!(
                "{base}/api/v1/plugins/{}/{}/releases",
                ensured.owner, ensured.slug
            ))
            .bearer_auth(&token)
            .multipart(form),
    )
    .await?;
    let released: HubReleaseResponse = release
        .json()
        .await
        .map_err(|e| anyhow!("插件库返回的版本信息格式不对: {e}"))?;

    Ok(PublishedPlugin {
        url: format!("{base}/plugins/{}/{}", ensured.owner, ensured.slug),
        owner: ensured.owner,
        slug: ensured.slug,
        version: released.version,
        existing: ensured.existing,
    })
}

/// 把插件包目录打成 ZIP（包根保持 `index.ts` 在顶层）。
///
/// 跳过隐藏文件/目录与常见构建垃圾（`node_modules` / `target` / `.git`），
/// 避免把 `.env`、依赖或产物上传到公共插件库。
fn zip_package(dir: &Path) -> Result<Vec<u8>> {
    if !dir.is_dir() {
        bail!("插件包目录不存在: {}", dir.display());
    }
    if !dir.join("index.ts").is_file() && !dir.join("index.js").is_file() {
        bail!("插件包根缺少 index.ts / index.js");
    }

    let mut files: Vec<PathBuf> = Vec::new();
    collect_package_files(dir, dir, &mut files)?;
    if files.len() > MAX_ZIP_ENTRIES {
        bail!("插件包文件过多（上限 {MAX_ZIP_ENTRIES} 个）");
    }

    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let mut total: u64 = 0;
        for path in files {
            let relative = path
                .strip_prefix(dir)
                .map_err(|_| anyhow!("无法计算包内相对路径: {}", path.display()))?;
            let name = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            let metadata = std::fs::symlink_metadata(&path)
                .with_context(|| format!("读取失败: {}", path.display()))?;
            if metadata.file_type().is_symlink() {
                bail!("插件包不允许符号链接: {name}");
            }
            if !metadata.is_file() {
                continue;
            }
            total = total.saturating_add(metadata.len());
            if total > MAX_UNCOMPRESSED_BYTES {
                bail!("插件包解压后超过 50 MB 上限");
            }
            writer
                .start_file(name.clone(), options)
                .with_context(|| format!("写入 ZIP 失败: {name}"))?;
            let mut file = std::fs::File::open(&path)
                .with_context(|| format!("读取失败: {}", path.display()))?;
            std::io::copy(&mut file, &mut writer)
                .with_context(|| format!("写入 ZIP 失败: {name}"))?;
        }
        writer.finish().context("完成 ZIP 失败")?;
    }
    Ok(buffer.into_inner())
}

fn collect_package_files(root: &Path, current: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(current)
        .with_context(|| format!("读取目录失败: {}", current.display()))?
    {
        let entry = entry.map_err(|e| anyhow!("读取目录项失败: {e}"))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || matches!(name.as_str(), "node_modules" | "target") {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("读取失败: {}", path.display()))?;
        if metadata.file_type().is_symlink() {
            bail!(
                "插件包不允许符号链接: {}",
                path.strip_prefix(root).unwrap_or(&path).display()
            );
        }
        if metadata.is_dir() {
            collect_package_files(root, &path, out)?;
        } else if metadata.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

async fn read_limited(mut response: reqwest::Response, limit: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| anyhow!("下载插件包失败: {e}"))?
    {
        if out.len() as u64 + chunk.len() as u64 > limit {
            bail!("插件包超过大小上限（{} MB）", limit / 1024 / 1024);
        }
        out.extend_from_slice(&chunk);
    }
    if out.is_empty() {
        bail!("插件包为空");
    }
    Ok(out)
}

fn validate_component<'a>(value: &'a str, label: &str) -> Result<&'a str> {
    let value = value.trim();
    let valid = !value.is_empty()
        && value.len() <= 80
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
    if !valid {
        bail!("非法的 {label}");
    }
    Ok(value)
}

/// 把 ZIP 包解压到 `extensions/<plugin_id>`，允许包外再套一层目录（GitHub ZIP 形态）。
///
/// 返回安装后的目录。`overwrite = false` 时目标已存在会直接拒绝——
/// 用户必须明确表达"覆盖已安装的插件"。
pub fn install_zip(
    bytes: &[u8],
    extensions: &Path,
    plugin_id: &str,
    overwrite: bool,
) -> Result<PathBuf> {
    let plugin_id = validate_component(plugin_id, "plugin_id")?.to_ascii_lowercase();
    std::fs::create_dir_all(extensions)
        .with_context(|| format!("创建扩展目录失败: {}", extensions.display()))?;

    let target = extensions.join(&plugin_id);
    if target.exists() && !overwrite {
        bail!("插件 {plugin_id} 已安装；如需覆盖，请确认后重新安装");
    }

    let staging = extensions.join(format!(".install-{plugin_id}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("创建临时目录失败: {}", staging.display()))?;

    let extracted = extract_archive(bytes, &staging).and_then(|_| {
        let entry = ["index.ts", "index.js"]
            .into_iter()
            .map(|name| staging.join(name))
            .find(|path| path.is_file())
            .ok_or_else(|| anyhow!("插件包根缺少 index.ts / index.js"))?;
        Ok(entry)
    });

    let entry = match extracted {
        Ok(entry) => entry,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    };

    if target.exists() {
        let backup = extensions.join(format!(".{plugin_id}.old-{}", uuid::Uuid::new_v4()));
        std::fs::rename(&target, &backup).with_context(|| {
            format!(
                "替换已安装插件失败（文件可能正被占用）: {}",
                target.display()
            )
        })?;
        if let Err(error) = std::fs::rename(&staging, &target) {
            let _ = std::fs::rename(&backup, &target);
            let _ = std::fs::remove_dir_all(&staging);
            return Err(anyhow!("安装插件失败: {error}"));
        }
        let _ = std::fs::remove_dir_all(&backup);
    } else {
        std::fs::rename(&staging, &target)
            .with_context(|| format!("安装插件失败: {}", target.display()))?;
    }

    let relative = entry.strip_prefix(&staging).unwrap_or(&entry).to_path_buf();
    Ok(target.join(relative))
}

fn extract_archive(bytes: &[u8], staging: &Path) -> Result<()> {
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|e| anyhow!("不是合法 ZIP: {e}"))?;
    if archive.len() > MAX_ZIP_ENTRIES {
        bail!("ZIP 条目过多（上限 {MAX_ZIP_ENTRIES}）");
    }

    let mut total: u64 = 0;
    let mut names: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        let file = archive
            .by_index(i)
            .map_err(|e| anyhow!("读取 ZIP 条目失败: {e}"))?;
        let name = file.name().to_string();
        validate_zip_path(&name)?;
        if file
            .unix_mode()
            .map(|mode| mode & 0o170000 == 0o120000)
            .unwrap_or(false)
        {
            bail!("ZIP 不允许符号链接: {name}");
        }
        if !file.is_dir() {
            total = total.saturating_add(file.size());
            if total > MAX_UNCOMPRESSED_BYTES {
                bail!("ZIP 解压后体积超过上限（50 MB）");
            }
            names.push(name);
        }
    }

    let prefix = common_package_prefix(&names)?;
    let index_in_prefix = names.iter().any(|name| {
        let stripped = strip_prefix(name, prefix.as_deref());
        matches!(stripped, Some("index.ts") | Some("index.js"))
    });
    if !index_in_prefix {
        bail!("插件包根缺少 index.ts / index.js（a_da 插件的唯一入口）");
    }

    let mut written: u64 = 0;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| anyhow!("读取 ZIP 条目失败: {e}"))?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_string();
        let Some(relative) = strip_prefix(&name, prefix.as_deref()) else {
            continue;
        };
        let mut destination = staging.to_path_buf();
        for part in relative.split('/') {
            if part.is_empty() || part == "." || part == ".." {
                bail!("ZIP 含非法路径: {name}");
            }
            destination.push(part);
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建目录失败: {}", parent.display()))?;
        }
        let mut out = std::fs::File::create(&destination)
            .with_context(|| format!("写入失败: {}", destination.display()))?;
        // 不信 ZIP 头里声明的大小：按**实际写出**的字节数再卡一次上限（zip bomb）。
        let remaining = MAX_UNCOMPRESSED_BYTES.saturating_sub(written) + 1;
        let copied = std::io::copy(&mut file.by_ref().take(remaining), &mut out)
            .with_context(|| format!("解压失败: {}", destination.display()))?;
        written = written.saturating_add(copied);
        if written > MAX_UNCOMPRESSED_BYTES {
            bail!("ZIP 解压后体积超过上限（50 MB）");
        }
        out.flush().ok();
    }
    Ok(())
}

fn validate_zip_path(name: &str) -> Result<()> {
    let normalized = name.trim_end_matches('/');
    let invalid = normalized.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.contains(':')
        || normalized
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..");
    if invalid {
        bail!("ZIP 含非法路径: {name}");
    }
    Ok(())
}

/// 如果整个包都套在同一个顶层目录里，返回该目录名。
fn common_package_prefix(names: &[String]) -> Result<Option<String>> {
    let mut prefix: Option<String> = None;
    let mut root_file = false;
    for name in names {
        let mut parts = name.split('/');
        let Some(top) = parts.next() else { continue };
        if parts.next().is_none() {
            root_file = true;
            continue;
        }
        match &prefix {
            None => prefix = Some(top.to_string()),
            Some(current) if current == top => {}
            Some(_) => {
                // 多个顶层目录：按"包根就是 ZIP 根"处理，后面缺 index 会明确报错。
                return Ok(None);
            }
        }
    }
    if root_file {
        return Ok(None);
    }
    Ok(prefix)
}

fn strip_prefix<'a>(name: &'a str, prefix: Option<&str>) -> Option<&'a str> {
    match prefix {
        Some(prefix) => name
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix('/')),
        None => Some(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ada_market_{tag}_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn zip_with(files: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            for (name, content) in files {
                zip.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(content.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn installs_flat_and_wrapped_packages() {
        for prefix in ["", "demo/"] {
            let dir = temp_dir("install");
            let bytes = zip_with(&[
                (&format!("{prefix}index.ts"), "export default {}"),
                (&format!("{prefix}skills/a/SKILL.md"), "# a"),
            ]);
            let entry = install_zip(&bytes, &dir, "demo", false).expect("安装应成功");
            assert!(entry.is_file(), "{}", entry.display());
            assert!(dir.join("demo/index.ts").is_file());
            assert!(dir.join("demo/skills/a/SKILL.md").is_file());
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn rejects_traversal_and_missing_entry() {
        let dir = temp_dir("reject");
        let bytes = zip_with(&[("../evil.ts", "x"), ("index.ts", "export default {}")]);
        assert!(install_zip(&bytes, &dir, "evil", false).is_err());
        let bytes = zip_with(&[("src/nested/index.ts", "export default {}")]);
        assert!(install_zip(&bytes, &dir, "nested", false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_overwrite_without_flag_and_replaces_with_it() {
        let dir = temp_dir("overwrite");
        let first = zip_with(&[("index.ts", "// v1")]);
        install_zip(&first, &dir, "demo", false).unwrap();

        let second = zip_with(&[("index.ts", "// v2")]);
        assert!(install_zip(&second, &dir, "demo", false).is_err());
        assert!(
            std::fs::read_to_string(dir.join("demo/index.ts"))
                .unwrap()
                .contains("v1")
        );

        install_zip(&second, &dir, "demo", true).unwrap();
        assert!(
            std::fs::read_to_string(dir.join("demo/index.ts"))
                .unwrap()
                .contains("v2")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zip_package_keeps_plugin_files_and_skips_secrets() {
        let dir = temp_dir("package");
        std::fs::write(dir.join("index.ts"), "export default {}").unwrap();
        std::fs::write(dir.join("README.md"), "# demo").unwrap();
        std::fs::create_dir_all(dir.join("skills").join("deploy")).unwrap();
        std::fs::write(dir.join("skills/deploy/SKILL.md"), "---\nname: deploy\n---").unwrap();
        std::fs::write(dir.join(".env"), "SECRET=1").unwrap();
        std::fs::create_dir_all(dir.join("node_modules").join("x")).unwrap();
        std::fs::write(dir.join("node_modules/x/index.js"), "x").unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("target/junk"), "junk").unwrap();

        let bytes = zip_package(&dir).expect("打包应成功");
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut names = Vec::new();
        for i in 0..archive.len() {
            names.push(archive.by_index(i).unwrap().name().to_string());
        }
        assert!(names.contains(&"index.ts".to_string()), "{names:?}");
        assert!(
            names.contains(&"skills/deploy/SKILL.md".to_string()),
            "{names:?}"
        );
        assert!(names.contains(&"README.md".to_string()), "{names:?}");
        assert!(!names.iter().any(|name| name.starts_with('.')), "{names:?}");
        assert!(
            !names.iter().any(|name| name.starts_with("node_modules")),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|name| name.starts_with("target")),
            "{names:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_roundtrip_and_token_hint() {
        let dir = temp_dir("config");
        let config = MarketConfig {
            base_url: "http://127.0.0.1:8787".into(),
            api_token: "ada_pat_abcdefgh".into(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(config.save(&dir)).unwrap();
        let loaded = MarketConfig::load(&dir);
        assert_eq!(loaded.base_url, "http://127.0.0.1:8787");
        assert!(loaded.has_token());
        assert_eq!(loaded.token_hint(), "…efgh");
        assert!(!loaded.token_hint().contains("ada_pat_"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn base_url_is_normalized_and_validated() {
        assert_eq!(
            normalize_base_url(" http://127.0.0.1:8787/ ").unwrap(),
            "http://127.0.0.1:8787"
        );
        assert!(normalize_base_url("ftp://example.com").is_err());
        assert!(normalize_base_url("").is_err());
        assert!(normalize_base_url("http://example.com/?a=1").is_err());
    }
}
