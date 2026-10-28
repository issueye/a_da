//! 插件包元数据：从 ZIP 包根的 `index.ts` / `index.js` **静态提取** `metadata`。
//!
//! 为什么是静态提取：插件库里存放的是**第三方上传的代码**，发布时不应该执行它
//! （哪怕跑在 Boa 沙箱里，也等于给了一个 CPU/内存消耗面）。而 a_da 的 metadata 契约
//! 本身是声明式的——`ada.metadata = { ... }` 或 `export default { metadata: { ... } }`，
//! 值由字符串/数字/布尔/数组/对象字面量组成，所以用一个小型字面量解析器就够了。
//!
//! 解析策略（按优先级）：
//! 1. `ada.metadata = <对象字面量>`；
//! 2. `ada.metadata = metadata`（再回溯 `const metadata = <对象字面量>`）；
//! 3. `api.metadata = ...`（兼容沙箱里的 api 别名）；
//! 4. `export default { metadata: <对象字面量> }`。
//!
//! 明确**不支持**复杂表达式（函数调用、模板插值、展开、计算属性）：遇到就跳过该键，
//! 必填字段因此缺失时会给出可操作的错误，而不是猜一个值。

use std::io::{Cursor, Read};

use serde_json::{Map, Number, Value};
use zip::ZipArchive;

/// 入口脚本读取上限：metadata 只占开头一小段，但入口文件本身可能带实现代码。
const MAX_ENTRY_BYTES: u64 = 4 * 1024 * 1024;
/// 元数据 JSON 落库上限，避免恶意包把整个对象字面量塞进 metadata。
const MAX_RAW_JSON_BYTES: usize = 16 * 1024;
/// README 读取上限：插件库只做展示，README 不该是几 MB 的二进制。
const MAX_README_BYTES: u64 = 512 * 1024;

/// 校验通过的插件包元数据。
#[derive(Debug, Clone)]
pub struct PackageMetadata {
    pub raw: Value,
    pub name: String,
    pub description: String,
    pub author: String,
    pub version: String,
}

impl PackageMetadata {
    /// 校验必填字段并归一化。错误信息直接展示给发布者。
    pub fn from_value(raw: Value) -> Result<Self, String> {
        let object = raw
            .as_object()
            .ok_or_else(|| "index.ts 的 metadata 必须是对象".to_string())?;
        let name = required_text(object, "name", 80)?;
        let description = required_text(object, "description", 300)?;
        let author = required_text(object, "author", 80)?;
        let version = required_text(object, "version", 32)?;
        if !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
        {
            return Err("metadata.version 只能包含字母、数字、点、加号和连字符".to_string());
        }
        let encoded =
            serde_json::to_string(&raw).map_err(|e| format!("metadata 序列化失败: {e}"))?;
        if encoded.len() > MAX_RAW_JSON_BYTES {
            return Err(format!(
                "metadata 过大（{} 字节，上限 {} 字节）",
                encoded.len(),
                MAX_RAW_JSON_BYTES
            ));
        }
        Ok(Self {
            raw,
            name,
            description,
            author,
            version,
        })
    }

    /// 表单版本与 metadata.version 的合并规则：
    /// - 表单留空 → 用 metadata 里的版本；
    /// - 两者都有 → 必须一致（否则"下载的包"与"页面上标的版本"会对不上）。
    pub fn effective_version(&self, requested: &str) -> Result<String, String> {
        let requested = requested.trim();
        if requested.is_empty() {
            return Ok(self.version.clone());
        }
        if requested != self.version {
            return Err(format!(
                "提交的版本号 {requested} 与 index.ts 的 metadata.version {} 不一致",
                self.version
            ));
        }
        Ok(self.version.clone())
    }

    /// 落库用的 JSON 字符串（只存归一化后的对象）。
    pub fn to_json_string(&self) -> String {
        serde_json::to_string(&self.raw).unwrap_or_else(|_| "{}".to_string())
    }
}

/// 一次发布从 ZIP 里能拿到的展示内容。
#[derive(Debug, Clone)]
pub struct PackageInspection {
    pub metadata: PackageMetadata,
    /// 包根 `README.md`（大小写不敏感）；没有就是 `None`——发布时不会用它清空已有 README。
    pub readme: Option<String>,
}

/// 从 ZIP 中定位包根入口并提取元数据。ZIP 结构本身也会走一遍 `validate_zip`。
pub fn inspect_zip(bytes: &[u8]) -> Result<PackageMetadata, String> {
    inspect_package(bytes).map(|inspection| inspection.metadata)
}

/// 同时提取 metadata 与 README：发布时用它们同步插件仓库的简介/README。
pub fn inspect_package(bytes: &[u8]) -> Result<PackageInspection, String> {
    crate::storage::validate_zip(bytes)?;
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("不是合法 ZIP: {e}"))?;
    let entry_name = package_entry_name(&mut archive)?;

    let mut entry = archive
        .by_name(&entry_name)
        .map_err(|e| format!("读取 {entry_name} 失败: {e}"))?;
    if entry.size() > MAX_ENTRY_BYTES {
        return Err(format!(
            "{entry_name} 过大（上限 {} MB）",
            MAX_ENTRY_BYTES / 1024 / 1024
        ));
    }
    let mut source = String::new();
    entry
        .by_ref()
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_string(&mut source)
        .map_err(|e| format!("{entry_name} 不是 UTF-8 文本: {e}"))?;
    if source.len() as u64 > MAX_ENTRY_BYTES {
        return Err(format!("{entry_name} 过大（上限 4 MB）"));
    }

    let raw = parse_metadata_source(&source)?;
    let metadata = PackageMetadata::from_value(raw)?;
    if metadata.name.trim().is_empty() {
        return Err("metadata.name 不能为空".to_string());
    }
    drop(entry);
    let readme = package_readme(&mut archive, &entry_name);
    Ok(PackageInspection { metadata, readme })
}

/// 找与入口同层的 `README.md`（大小写不敏感）。
///
/// README 是**展示内容**，不是发布契约：读不出来（过大/非 UTF-8/损坏）时返回 `None`，
/// 保留仓库里已有的 README，绝不能因为一份 README 让版本发布失败。
fn package_readme(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    entry_name: &str,
) -> Option<String> {
    let prefix = entry_name
        .rsplit_once('/')
        .map(|(dir, _)| dir)
        .unwrap_or("");
    let mut target: Option<String> = None;
    for i in 0..archive.len() {
        let file = archive.by_index(i).ok()?;
        if file.is_dir() {
            continue;
        }
        let name = file.name();
        let (dir, base) = name.rsplit_once('/').unwrap_or(("", name));
        if !base.eq_ignore_ascii_case("README.md") || dir != prefix {
            continue;
        }
        target = Some(name.to_string());
        break;
    }
    let name = target?;
    let mut file = archive.by_name(&name).ok()?;
    if file.size() > MAX_README_BYTES {
        tracing::warn!(%name, "插件包 README 过大，已忽略");
        return None;
    }
    let mut text = String::new();
    if file
        .by_ref()
        .take(MAX_README_BYTES + 1)
        .read_to_string(&mut text)
        .is_err()
    {
        tracing::warn!(%name, "插件包 README 不是 UTF-8 文本，已忽略");
        return None;
    }
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// 包根入口：`index.ts|js`，允许整个包被一层目录包着（GitHub ZIP 常见形态）。
fn package_entry_name(archive: &mut ZipArchive<Cursor<&[u8]>>) -> Result<String, String> {
    let mut root: Option<String> = None;
    let mut wrapped: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        let file = archive
            .by_index(i)
            .map_err(|e| format!("读取 ZIP 条目失败: {e}"))?;
        if file.is_dir() {
            continue;
        }
        let name = file.name();
        let base = name.rsplit('/').next().unwrap_or(name);
        if base != "index.ts" && base != "index.js" {
            continue;
        }
        match name.matches('/').count() {
            0 => root = Some(name.to_string()),
            1 => wrapped.push(name.to_string()),
            _ => {}
        }
    }
    if let Some(root) = root {
        return Ok(root);
    }
    match wrapped.len() {
        1 => Ok(wrapped.remove(0)),
        0 => Err("ZIP 包根缺少 index.ts / index.js（a_da 插件的唯一入口）".to_string()),
        _ => Err("ZIP 里发现多个 index.ts / index.js，无法确定插件包根".to_string()),
    }
}

fn required_text(
    object: &Map<String, Value>,
    key: &str,
    max_chars: usize,
) -> Result<String, String> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("metadata 缺少非空字符串 `{key}`"))?;
    if value.chars().count() > max_chars {
        return Err(format!("metadata.{key} 超过 {max_chars} 个字符"));
    }
    Ok(value.to_string())
}

// ─────────────────────────────────────────────────────────────────────────
// 以下为字面量解析（无第三方 JS 解析器）
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
enum ParseError {
    /// 不是可静态求值的字面量：跳过这个值，由必填校验给出明确错误。
    Unsupported,
    Invalid(String),
}

type ParseResult<T> = Result<T, ParseError>;

/// 从源码中找出 metadata 对象。
pub fn parse_metadata_source(source: &str) -> Result<Value, String> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);

    if let Some(pos) = find_metadata_assignment(source) {
        return parse_rhs(source, pos, 0).map_err(|e| {
            format!(
                "index.ts 的 metadata 无法静态解析：{e}\n\
                 请写成工厂函数里的 `ada.metadata = {{ name, description, author, version, ... }}`，\
                 值使用字符串/数字/布尔/数组/对象字面量。"
            )
        });
    }

    if let Some(pos) = find_export_default_object(source) {
        let mut reader = JsReader::new(source, pos);
        if let Ok(Value::Object(mut object)) = reader.parse_value() {
            if let Some(metadata) = object.remove("metadata") {
                match metadata {
                    Value::String(binding) => {
                        let bind = find_binding(source, &binding).ok_or_else(|| {
                            format!("找不到 metadata 变量 `{binding}` 的 `const/let/var` 定义")
                        })?;
                        return parse_rhs(source, bind, 0);
                    }
                    value => return Ok(value),
                }
            }
        }
    }

    Err(
        "index.ts 里没有找到 metadata：请在插件工厂里写 `ada.metadata = { name, description, author, version, ... }`"
            .to_string(),
    )
}

fn parse_rhs(source: &str, pos: usize, depth: usize) -> Result<Value, String> {
    if depth > 4 {
        return Err("metadata 变量引用层级过深".to_string());
    }
    let mut reader = JsReader::new(source, pos);
    reader.skip_ws();
    let save = reader.pos;

    // 形如 `ada.metadata = metadata;`：回溯常量定义。
    if let Some(name) = reader.take_ident() {
        let binding = find_binding(source, &name)
            .ok_or_else(|| format!("找不到 metadata 变量 `{name}` 的定义"))?;
        return parse_rhs(source, binding, depth + 1);
    }
    reader.pos = save;

    match reader.parse_value() {
        Ok(value) => Ok(value),
        Err(ParseError::Unsupported) => Err("metadata 必须是对象字面量".to_string()),
        Err(ParseError::Invalid(message)) => Err(message),
    }
}

/// 查找 `ada.metadata =` / `api.metadata =`，返回值起始位置。
fn find_metadata_assignment(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => i = skip_line_comment(bytes, i),
            b'/' if bytes.get(i + 1) == Some(&b'*') => i = skip_block_comment(bytes, i),
            b'\'' | b'"' | b'`' => i = skip_js_string(bytes, i),
            byte if is_ident_start(byte) => {
                let start = i;
                let ident = read_ident_at(source, i).0;
                let end = start + ident.len();
                i = end;
                if ident != "ada" && ident != "api" {
                    continue;
                }
                let mut j = skip_trivia(bytes, end);
                if bytes.get(j) != Some(&b'.') {
                    continue;
                }
                j = skip_trivia(bytes, j + 1);
                let (field, field_end) = read_ident_at(source, j);
                if field != "metadata" {
                    continue;
                }
                j = skip_trivia(bytes, field_end);
                if bytes.get(j) == Some(&b'=')
                    && bytes.get(j + 1) != Some(&b'=')
                    && bytes.get(j + 1) != Some(&b'>')
                {
                    return Some(skip_trivia(bytes, j + 1));
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// 查找 `export default {`，返回 `{` 的位置。
fn find_export_default_object(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => i = skip_line_comment(bytes, i),
            b'/' if bytes.get(i + 1) == Some(&b'*') => i = skip_block_comment(bytes, i),
            b'\'' | b'"' | b'`' => i = skip_js_string(bytes, i),
            byte if is_ident_start(byte) => {
                let (ident, end) = read_ident_at(source, i);
                i = end;
                if ident != "export" {
                    continue;
                }
                let mut j = skip_trivia(bytes, end);
                let (keyword, keyword_end) = read_ident_at(source, j);
                if keyword != "default" {
                    continue;
                }
                j = skip_trivia(bytes, keyword_end);
                if bytes.get(j) == Some(&b'{') {
                    return Some(j);
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// 查找 `const|let|var <name> =`，返回值起始位置。
fn find_binding(source: &str, name: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => i = skip_line_comment(bytes, i),
            b'/' if bytes.get(i + 1) == Some(&b'*') => i = skip_block_comment(bytes, i),
            b'\'' | b'"' | b'`' => i = skip_js_string(bytes, i),
            byte if is_ident_start(byte) => {
                let (ident, end) = read_ident_at(source, i);
                i = end;
                if !matches!(ident.as_str(), "const" | "let" | "var") {
                    continue;
                }
                let mut j = skip_trivia(bytes, end);
                let (declared, declared_end) = read_ident_at(source, j);
                if declared != name {
                    continue;
                }
                j = skip_trivia(bytes, declared_end);
                // 容忍 TS 类型标注：`const meta: Foo = {...}`。
                if bytes.get(j) == Some(&b':') {
                    j = skip_type_annotation(bytes, j + 1);
                }
                if bytes.get(j) == Some(&b'=')
                    && bytes.get(j + 1) != Some(&b'=')
                    && bytes.get(j + 1) != Some(&b'>')
                {
                    return Some(skip_trivia(bytes, j + 1));
                }
            }
            _ => i += 1,
        }
    }
    None
}

fn skip_type_annotation(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    let mut depth = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => {
                if depth == 0 {
                    return i;
                }
                depth -= 1;
            }
            b'=' if depth == 0 && bytes.get(i + 1) != Some(&b'=') => return i,
            _ => {}
        }
        i += 1;
    }
    i
}

fn skip_trivia(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) == Some(&b'/') && bytes.get(i + 1) == Some(&b'/') {
            i = skip_line_comment(bytes, i);
            continue;
        }
        if bytes.get(i) == Some(&b'/') && bytes.get(i + 1) == Some(&b'*') {
            i = skip_block_comment(bytes, i);
            continue;
        }
        return i;
    }
}

fn skip_line_comment(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 2;
    while i < bytes.len() && bytes[i] != b'\n' {
        i += 1;
    }
    i
}

fn skip_block_comment(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 2;
    while i + 1 < bytes.len() {
        if bytes[i] == b'*' && bytes[i + 1] == b'/' {
            return i + 2;
        }
        i += 1;
    }
    bytes.len()
}

fn skip_js_string(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            byte if byte == quote => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$'
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn read_ident_at(source: &str, start: usize) -> (String, usize) {
    let bytes = source.as_bytes();
    let mut end = start;
    while end < bytes.len() && is_ident_continue(bytes[end]) {
        end += 1;
    }
    (source[start..end].to_string(), end)
}

struct JsReader<'a> {
    source: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsReader<'a> {
    fn new(source: &'a str, pos: usize) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            pos,
        }
    }

    fn skip_ws(&mut self) {
        self.pos = skip_trivia(self.bytes, self.pos);
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.pos += 1;
        Some(byte)
    }

    fn take_ident(&mut self) -> Option<String> {
        self.skip_ws();
        if !self.peek().is_some_and(is_ident_start) {
            return None;
        }
        let (ident, end) = read_ident_at(self.source, self.pos);
        self.pos = end;
        Some(ident)
    }

    fn expect(&mut self, byte: u8) -> ParseResult<()> {
        self.skip_ws();
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(ParseError::Invalid(format!(
                "期望 `{}`（位置 {}）",
                byte as char, self.pos
            )))
        }
    }

    fn parse_value(&mut self) -> ParseResult<Value> {
        self.skip_ws();
        match self.peek() {
            None => Err(ParseError::Invalid("metadata 对象不完整".to_string())),
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'\'') | Some(b'"') => self.parse_string().map(Value::String),
            Some(b'`') => self.parse_template(),
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(),
            Some(b'(') => {
                self.pos += 1;
                let value = self.parse_value()?;
                self.expect(b')')?;
                Ok(value)
            }
            Some(byte) if is_ident_start(byte) => {
                let start = self.pos;
                let (ident, end) = read_ident_at(self.source, self.pos);
                self.pos = end;
                match ident.as_str() {
                    "true" => Ok(Value::Bool(true)),
                    "false" => Ok(Value::Bool(false)),
                    "null" => Ok(Value::Null),
                    _ => {
                        self.pos = start;
                        self.skip_expression(&[b',', b'}', b']']);
                        Err(ParseError::Unsupported)
                    }
                }
            }
            Some(_) => {
                self.skip_expression(&[b',', b'}', b']']);
                Err(ParseError::Unsupported)
            }
        }
    }

    fn parse_object(&mut self) -> ParseResult<Value> {
        self.expect(b'{')?;
        let mut map = Map::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => return Err(ParseError::Invalid("metadata 对象没有闭合".to_string())),
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                Some(b',') => {
                    self.pos += 1;
                    continue;
                }
                Some(b'.') if self.source[self.pos..].starts_with("...") => {
                    self.pos += 3;
                    self.skip_expression(&[b',', b'}']);
                    continue;
                }
                _ => {}
            }

            let key = match self.parse_key() {
                Ok(key) => key,
                Err(ParseError::Unsupported) => {
                    self.skip_expression(&[b',', b'}']);
                    continue;
                }
                Err(error) => return Err(error),
            };
            self.skip_ws();
            if self.peek() != Some(b':') {
                self.skip_expression(&[b',', b'}']);
                continue;
            }
            self.pos += 1;

            match self.parse_value() {
                Ok(value) => {
                    map.insert(key, value);
                }
                Err(ParseError::Unsupported) => {}
                Err(error) => return Err(error),
            }

            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                Some(_) => {
                    self.skip_expression(&[b',', b'}']);
                    if self.peek() == Some(b',') {
                        self.pos += 1;
                    } else if self.peek() == Some(b'}') {
                        self.pos += 1;
                        break;
                    }
                }
                None => return Err(ParseError::Invalid("metadata 对象没有闭合".to_string())),
            }
        }
        Ok(Value::Object(map))
    }

    fn parse_array(&mut self) -> ParseResult<Value> {
        self.expect(b'[')?;
        let mut values = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => return Err(ParseError::Invalid("数组没有闭合".to_string())),
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                Some(b',') => {
                    self.pos += 1;
                    continue;
                }
                _ => {}
            }
            match self.parse_value() {
                Ok(value) => values.push(value),
                Err(ParseError::Unsupported) => {}
                Err(error) => return Err(error),
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                Some(_) => {
                    self.skip_expression(&[b',', b']']);
                    if self.peek() == Some(b',') {
                        self.pos += 1;
                    } else if self.peek() == Some(b']') {
                        self.pos += 1;
                        break;
                    }
                }
                None => return Err(ParseError::Invalid("数组没有闭合".to_string())),
            }
        }
        Ok(Value::Array(values))
    }

    fn parse_key(&mut self) -> ParseResult<String> {
        self.skip_ws();
        match self.peek() {
            Some(b'\'') | Some(b'"') => self.parse_string(),
            Some(byte) if is_ident_start(byte) => {
                let (ident, end) = read_ident_at(self.source, self.pos);
                self.pos = end;
                Ok(ident)
            }
            Some(byte) if byte.is_ascii_digit() => {
                let start = self.pos;
                while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                    self.pos += 1;
                }
                Ok(self.source[start..self.pos].to_string())
            }
            _ => Err(ParseError::Unsupported),
        }
    }

    fn parse_string(&mut self) -> ParseResult<String> {
        self.skip_ws();
        let quote = self
            .peek()
            .filter(|byte| *byte == b'\'' || *byte == b'"')
            .ok_or(ParseError::Unsupported)?;
        self.pos += 1;
        let mut out = String::new();
        while let Some(byte) = self.bump() {
            match byte {
                byte if byte == quote => return Ok(out),
                b'\\' => {
                    let escape = self
                        .bump()
                        .ok_or_else(|| ParseError::Invalid("字符串转义不完整".to_string()))?;
                    match escape {
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000c}'),
                        b'0' => out.push('\0'),
                        b'\'' => out.push('\''),
                        b'"' => out.push('"'),
                        b'`' => out.push('`'),
                        b'\\' => out.push('\\'),
                        b'u' => {
                            let code = self.take_hex4()?;
                            if (0xD800..=0xDBFF).contains(&code) {
                                // 代理对：`\uD83D\uDE00`
                                if self.bump() != Some(b'\\') || self.bump() != Some(b'u') {
                                    return Err(ParseError::Invalid(
                                        "字符串里的代理对不完整".to_string(),
                                    ));
                                }
                                let low = self.take_hex4()?;
                                let combined = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                                out.push(char::from_u32(combined).ok_or_else(|| {
                                    ParseError::Invalid("非法 Unicode 转义".to_string())
                                })?);
                            } else {
                                out.push(char::from_u32(code).ok_or_else(|| {
                                    ParseError::Invalid("非法 Unicode 转义".to_string())
                                })?);
                            }
                        }
                        b'\n' => {}
                        other => out.push(other as char),
                    }
                }
                byte if byte.is_ascii() => out.push(byte as char),
                _ => {
                    // UTF-8 多字节字符：`bump()` 只前进了一个字节，这里补上其余字节。
                    let ch = self.source[self.pos - 1..]
                        .chars()
                        .next()
                        .ok_or_else(|| ParseError::Invalid("非法 UTF-8".to_string()))?;
                    out.push(ch);
                    self.pos += ch.len_utf8() - 1;
                }
            }
        }
        Err(ParseError::Invalid("字符串没有闭合".to_string()))
    }

    fn take_hex4(&mut self) -> ParseResult<u32> {
        let start = self.pos;
        let end = (start + 4).min(self.bytes.len());
        let slice = &self.source[start..end];
        if slice.len() != 4 || !slice.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ParseError::Invalid("\\u 转义需要 4 位十六进制".to_string()));
        }
        self.pos = end;
        u32::from_str_radix(slice, 16).map_err(|_| ParseError::Invalid("\\u 转义非法".to_string()))
    }

    fn parse_template(&mut self) -> ParseResult<Value> {
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        while let Some(byte) = self.bump() {
            match byte {
                b'`' => return Ok(Value::String(out)),
                b'\\' => {
                    if let Some(escaped) = self.bump() {
                        out.push(escaped as char);
                    }
                }
                b'$' if self.peek() == Some(b'{') => {
                    self.pos = start;
                    self.skip_expression(&[b',', b'}', b']']);
                    return Err(ParseError::Unsupported);
                }
                byte if byte.is_ascii() => out.push(byte as char),
                _ => {
                    let ch = self.source[self.pos - 1..]
                        .chars()
                        .next()
                        .ok_or_else(|| ParseError::Invalid("非法 UTF-8".to_string()))?;
                    out.push(ch);
                    self.pos += ch.len_utf8() - 1;
                }
            }
        }
        Err(ParseError::Invalid("模板字符串没有闭合".to_string()))
    }

    fn parse_number(&mut self) -> ParseResult<Value> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let text = &self.source[start..self.pos];
        if let Ok(int) = text.parse::<i64>() {
            return Ok(Value::Number(Number::from(int)));
        }
        let float = text
            .parse::<f64>()
            .map_err(|_| ParseError::Invalid(format!("非法数字 `{text}`")))?;
        Number::from_f64(float)
            .map(Value::Number)
            .ok_or_else(|| ParseError::Invalid(format!("非法数字 `{text}`")))
    }

    /// 跳过一个无法静态求值的表达式：按括号配平，停在目标分隔符之前。
    fn skip_expression(&mut self, stops: &[u8]) {
        let mut depth = 0usize;
        while let Some(byte) = self.peek() {
            match byte {
                b'/' if self.bytes.get(self.pos + 1) == Some(&b'/') => {
                    self.pos = skip_line_comment(self.bytes, self.pos);
                    continue;
                }
                b'/' if self.bytes.get(self.pos + 1) == Some(&b'*') => {
                    self.pos = skip_block_comment(self.bytes, self.pos);
                    continue;
                }
                b'\'' | b'"' | b'`' => {
                    self.pos = skip_js_string(self.bytes, self.pos);
                    continue;
                }
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                b',' if depth == 0 && stops.contains(&b',') => return,
                _ => {}
            }
            self.pos += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn full_source(body: &str) -> String {
        format!(
            "const metadata = {{
              name: 'demo-plugin',
              description: '演示插件',
              author: 'alice',
              version: '1.2.3',
              tools: {{ count: 1, names: ['demo_tool'] }},
              systemPrompt: {{ count: 0, names: [] }},
              skills: {{ count: 0, names: [] }},
              events: {{ count: 0, names: [] }}
            }};
            export default function (ada) {{
              ada.metadata = metadata;
              {body}
            }}"
        )
    }

    #[test]
    fn extracts_const_binding() {
        let raw = parse_metadata_source(&full_source("")).expect("应解析成功");
        let meta = PackageMetadata::from_value(raw).expect("必填字段齐全");
        assert_eq!(meta.name, "demo-plugin");
        assert_eq!(meta.author, "alice");
        assert_eq!(meta.version, "1.2.3");
        assert_eq!(meta.raw["tools"]["count"], 1);
    }

    #[test]
    fn extracts_inline_assignment_with_comments_and_trailing_commas() {
        let source = r#"
          // ada.metadata = { name: '在注释里不算' };
          const s = "ada.metadata = { name: '在字符串里也不算' }";
          export default function (ada) {
            ada.metadata = {
              name: "inline", // 单行注释
              description: '带逗号, 也没关系',
              author: 'bob',
              version: '0.1.0',
              tools: { count: 0, names: [], },
            };
          }
        "#;
        let meta = PackageMetadata::from_value(parse_metadata_source(source).unwrap()).unwrap();
        assert_eq!(meta.name, "inline");
        assert_eq!(meta.description, "带逗号, 也没关系");
        assert_eq!(meta.author, "bob");
    }

    #[test]
    fn extracts_export_default_object_form() {
        let source = r#"
          export default {
            metadata: { name: 'legacy', description: '对象形态', author: 'carol', version: '2.0.0' },
            tools: []
          }
        "#;
        let meta = PackageMetadata::from_value(parse_metadata_source(source).unwrap()).unwrap();
        assert_eq!(meta.name, "legacy");
        assert_eq!(meta.version, "2.0.0");
    }

    #[test]
    fn ignores_non_literal_value_but_keeps_required_error() {
        let source = r#"
          const metadata = { name: 'x', description: 'd', author: String.fromCharCode(65), version: '1.0.0' };
          export default function (ada) { ada.metadata = metadata; }
        "#;
        let raw = parse_metadata_source(source).unwrap();
        let error = PackageMetadata::from_value(raw).unwrap_err();
        assert!(error.contains("author"), "{error}");
    }

    #[test]
    fn rejects_missing_field_and_bad_version() {
        let raw = parse_metadata_source(
            "export default function (ada) { ada.metadata = { name: 'x', description: 'd' }; }",
        )
        .unwrap();
        let error = PackageMetadata::from_value(raw).unwrap_err();
        assert!(error.contains("author"), "{error}");

        let raw = parse_metadata_source(
            "export default function (ada) { ada.metadata = { name: 'x', description: 'd', author: 'a', version: '1.0/0' }; }",
        )
        .unwrap();
        let error = PackageMetadata::from_value(raw).unwrap_err();
        assert!(error.contains("version"), "{error}");
    }

    #[test]
    fn effective_version_must_match() {
        let meta = PackageMetadata {
            raw: serde_json::json!({}),
            name: "x".into(),
            description: "d".into(),
            author: "a".into(),
            version: "1.0.0".into(),
        };
        assert_eq!(meta.effective_version("").unwrap(), "1.0.0");
        assert_eq!(meta.effective_version("1.0.0").unwrap(), "1.0.0");
        assert!(meta.effective_version("1.0.1").is_err());
    }

    #[test]
    fn zip_root_and_wrapped_package() {
        let source = full_source("");
        for prefix in ["", "demo/"] {
            let mut buf = Cursor::new(Vec::new());
            {
                let mut zip = zip::ZipWriter::new(&mut buf);
                zip.start_file(
                    format!("{prefix}index.ts"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
                zip.write_all(source.as_bytes()).unwrap();
                if !prefix.is_empty() {
                    zip.start_file("demo/README.md", zip::write::SimpleFileOptions::default())
                        .unwrap();
                    zip.write_all(b"# demo").unwrap();
                }
                zip.finish().unwrap();
            }
            let meta = inspect_zip(&buf.into_inner()).expect("应识别包根入口");
            assert_eq!(meta.name, "demo-plugin", "prefix={prefix}");
        }
    }

    #[test]
    fn extracts_readme_next_to_package_entry() {
        let source = full_source("");
        for (prefix, readme_name) in [("", "README.md"), ("demo/", "Readme.md")] {
            let mut buf = Cursor::new(Vec::new());
            {
                let mut zip = zip::ZipWriter::new(&mut buf);
                zip.start_file(
                    format!("{prefix}index.ts"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
                zip.write_all(source.as_bytes()).unwrap();
                zip.start_file(
                    format!("{prefix}{readme_name}"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
                zip.write_all(b"# Demo\n\nPackage README.").unwrap();
                zip.finish().unwrap();
            }
            let inspection = inspect_package(&buf.into_inner()).expect("应解析成功");
            assert_eq!(
                inspection.readme.as_deref(),
                Some("# Demo\n\nPackage README."),
                "prefix={prefix}"
            );
        }
    }

    #[test]
    fn missing_readme_is_none_not_an_error() {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            zip.start_file("index.ts", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(full_source("").as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        let inspection = inspect_package(&buf.into_inner()).expect("没有 README 也应成功");
        assert!(inspection.readme.is_none());
    }

    #[test]
    fn zip_without_entry_is_rejected() {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            zip.start_file("src/main.ts", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"export default {}").unwrap();
            zip.finish().unwrap();
        }
        let error = inspect_zip(&buf.into_inner()).unwrap_err();
        assert!(error.contains("index.ts"), "{error}");
    }
}
