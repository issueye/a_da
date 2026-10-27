//! SQLite 原生绑定：`node:sqlite` 的同步子集（`DatabaseSync` / `StatementSync`）。
//!
//! 为什么是同步的：Boa 的事件循环线程本身就是同步执行段，SQLite 的调用又是
//! 微秒到毫秒级；再包一层 Tokio 只会把"数据库已关闭"这类错误延迟成超时。
//!
//! 边界：
//! - 只有 `@ada-needs sqlite` 才注入（未声明时 `require('node:sqlite')` 会**说清原因**地报错）；
//! - 数据库文件必须落在工作区内（`:memory:` 例外）；`file:` URI 一律拒绝，
//!   否则 `file:../../x.db` 会绕过路径校验；
//! - `ATTACH DATABASE` / `VACUUM INTO` 也走同一套路径判定（authorizer），
//!   否则一条 SQL 就能打开工作区外的文件；
//! - 结果集有行数/字节上限，避免一条查询把宿主内存吃光；
//! - 不注册 SQLite 扩展加载入口（`load_extension` 未编译进来）。

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use boa_engine::{js_error, js_string, Context, JsValue, NativeFunction, Source};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, OpenFlags, Statement};
use serde_json::{json, Map, Value};

const MAX_SQL_BYTES: usize = 1024 * 1024;
const MAX_ROWS: usize = 10_000;
const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_BUSY_TIMEOUT_MS: u64 = 5_000;

thread_local! {
    static SQLITE_WORKSPACE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static SQLITE_DBS: RefCell<HashMap<u64, Connection>> = RefCell::new(HashMap::new());
    static SQLITE_NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

/// 注册 SQLite 原生函数与 `node:sqlite` 封装层。
pub fn register_sqlite_native_and_script(
    ctx: &mut Context,
    workspace: Option<&Path>,
) -> Result<(), String> {
    SQLITE_WORKSPACE.with(|cell| {
        *cell.borrow_mut() = workspace.map(Path::to_path_buf);
    });

    let native_open = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let path = arg_string(args, 0, ctx);
        let options = arg_string(args, 1, ctx);
        let id = open_db(&path, &options).map_err(|e| js_error!("{}", e))?;
        Ok(JsValue::from(id as f64))
    });
    ctx.register_global_callable(js_string!("__native_sqlite_open"), 2, native_open)
        .ok();

    let native_close = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let id = arg_id(args, ctx).map_err(|e| js_error!("{}", e))?;
        close_db(id).map_err(|e| js_error!("{}", e))?;
        Ok(JsValue::undefined())
    });
    ctx.register_global_callable(js_string!("__native_sqlite_close"), 1, native_close)
        .ok();

    let native_exec = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let id = arg_id(args, ctx).map_err(|e| js_error!("{}", e))?;
        let sql = arg_string(args, 1, ctx);
        with_db(id, |conn| exec_sql(conn, &sql)).map_err(|e| js_error!("{}", e))?;
        Ok(JsValue::undefined())
    });
    ctx.register_global_callable(js_string!("__native_sqlite_exec"), 2, native_exec)
        .ok();

    let native_prepare = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let id = arg_id(args, ctx).map_err(|e| js_error!("{}", e))?;
        let sql = arg_string(args, 1, ctx);
        let columns =
            with_db(id, |conn| prepare_columns(conn, &sql)).map_err(|e| js_error!("{}", e))?;
        let out = json!({ "columns": columns }).to_string();
        Ok(JsValue::from(js_string!(out.as_str())))
    });
    ctx.register_global_callable(js_string!("__native_sqlite_prepare"), 2, native_prepare)
        .ok();

    let native_run = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let id = arg_id(args, ctx).map_err(|e| js_error!("{}", e))?;
        let sql = arg_string(args, 1, ctx);
        let params = arg_params(args, ctx);
        let out =
            with_db(id, |conn| run_sql(conn, &sql, &params)).map_err(|e| js_error!("{}", e))?;
        Ok(JsValue::from(js_string!(out.as_str())))
    });
    ctx.register_global_callable(js_string!("__native_sqlite_run"), 3, native_run)
        .ok();

    let native_get = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let id = arg_id(args, ctx).map_err(|e| js_error!("{}", e))?;
        let sql = arg_string(args, 1, ctx);
        let params = arg_params(args, ctx);
        let out = with_db(id, |conn| query_sql(conn, &sql, &params, true))
            .map_err(|e| js_error!("{}", e))?;
        Ok(JsValue::from(js_string!(out.as_str())))
    });
    ctx.register_global_callable(js_string!("__native_sqlite_get"), 3, native_get)
        .ok();

    let native_all = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let id = arg_id(args, ctx).map_err(|e| js_error!("{}", e))?;
        let sql = arg_string(args, 1, ctx);
        let params = arg_params(args, ctx);
        let out = with_db(id, |conn| query_sql(conn, &sql, &params, false))
            .map_err(|e| js_error!("{}", e))?;
        Ok(JsValue::from(js_string!(out.as_str())))
    });
    ctx.register_global_callable(js_string!("__native_sqlite_all"), 3, native_all)
        .ok();

    let wrapper = r#"
        (function() {
            function isBinary(value) {
                return !!value && typeof value === 'object' &&
                    (typeof Buffer !== 'undefined' && Buffer.isBuffer(value) || value instanceof Uint8Array);
            }

            function encodeValue(value) {
                if (value === undefined) return null;
                if (isBinary(value)) {
                    return { __ada_blob: Buffer.from(value).toString('base64') };
                }
                if (typeof value === 'bigint') {
                    return { __ada_bigint: value.toString() };
                }
                return value;
            }

            function encodeParams(args) {
                const first = args[0];
                if (args.length === 1 && first && typeof first === 'object' &&
                    !Array.isArray(first) && !isBinary(first)) {
                    const out = {};
                    const keys = Object.keys(first);
                    for (let i = 0; i < keys.length; i++) {
                        out[keys[i]] = encodeValue(first[keys[i]]);
                    }
                    return JSON.stringify(out);
                }
                const out = [];
                for (let i = 0; i < args.length; i++) out.push(encodeValue(args[i]));
                return JSON.stringify(out);
            }

            function decodeValue(value) {
                if (value && typeof value === 'object' && typeof value.__ada_blob === 'string') {
                    return Buffer.from(value.__ada_blob, 'base64');
                }
                return value;
            }

            function decodeRow(row) {
                if (!row) return row;
                const keys = Object.keys(row);
                for (let i = 0; i < keys.length; i++) {
                    row[keys[i]] = decodeValue(row[keys[i]]);
                }
                return row;
            }

            function parseJson(raw) {
                return typeof raw === 'string' ? JSON.parse(raw) : raw;
            }

            class StatementSync {
                constructor(db, sql, columns) {
                    this._db = db;
                    this._sql = sql;
                    this._columns = Array.isArray(columns) ? columns : [];
                }

                run(...args) {
                    const raw = __native_sqlite_run(
                        this._db._require(),
                        this._sql,
                        encodeParams(args),
                    );
                    const result = parseJson(raw);
                    return {
                        changes: result.changes,
                        lastInsertRowid: result.lastInsertRowid,
                    };
                }

                get(...args) {
                    const raw = __native_sqlite_get(
                        this._db._require(),
                        this._sql,
                        encodeParams(args),
                    );
                    const row = parseJson(raw);
                    return row === null ? undefined : decodeRow(row);
                }

                all(...args) {
                    const raw = __native_sqlite_all(
                        this._db._require(),
                        this._sql,
                        encodeParams(args),
                    );
                    const rows = parseJson(raw);
                    for (let i = 0; i < rows.length; i++) decodeRow(rows[i]);
                    return rows;
                }

                columns() {
                    const out = [];
                    for (let i = 0; i < this._columns.length; i++) {
                        out.push({ name: this._columns[i] });
                    }
                    return out;
                }

                /** Node 的开关；这里始终允许裸命名参数，保留入口只为兼容。 */
                setAllowBareNamedParameters(allowed) {
                    this._allowBareNamedParameters = allowed !== false;
                }
            }

            class DatabaseSync {
                constructor(path, options) {
                    this._path = path === undefined ? ':memory:' : String(path);
                    this._options = options || {};
                    this._id = 0;
                    if (this._options.open !== false) this.open();
                }

                open() {
                    if (this._id !== 0) return;
                    const options = Object.assign({}, this._options);
                    options.readOnly = options.readOnly === true;
                    this._id = Number(
                        __native_sqlite_open(this._path, JSON.stringify(options)),
                    );
                }

                close() {
                    if (this._id === 0) return;
                    __native_sqlite_close(this._id);
                    this._id = 0;
                }

                exec(sql) {
                    __native_sqlite_exec(this._require(), String(sql));
                }

                prepare(sql) {
                    const raw = __native_sqlite_prepare(this._require(), String(sql));
                    const info = parseJson(raw);
                    return new StatementSync(this, String(sql), info.columns || []);
                }

                location() {
                    return this._path;
                }

                get isOpen() {
                    return this._id !== 0;
                }

                _require() {
                    if (this._id === 0) throw new Error('SQLite 数据库未打开或已关闭');
                    return this._id;
                }
            }

            globalThis.sqlite = { DatabaseSync: DatabaseSync, StatementSync: StatementSync };
        })();
    "#;
    ctx.eval(Source::from_bytes(wrapper))
        .map_err(|e| format!("注入 node:sqlite 封装层失败: {e}"))?;
    Ok(())
}

fn arg_string(args: &[JsValue], index: usize, ctx: &mut Context) -> String {
    args.get(index)
        .map(|value| {
            value
                .to_string(ctx)
                .unwrap_or_default()
                .to_std_string_escaped()
        })
        .unwrap_or_default()
}

fn arg_id(args: &[JsValue], ctx: &mut Context) -> Result<u64, String> {
    let raw = args
        .get(0)
        .map(|value| value.to_number(ctx).unwrap_or(0.0))
        .unwrap_or(0.0);
    if raw < 1.0 || !raw.is_finite() {
        return Err("SQLite 数据库 id 不合法".to_string());
    }
    Ok(raw as u64)
}

fn arg_params(args: &[JsValue], ctx: &mut Context) -> Value {
    let raw = arg_string(args, 2, ctx);
    if raw.trim().is_empty() {
        return Value::Array(Vec::new());
    }
    serde_json::from_str(&raw).unwrap_or(Value::Array(Vec::new()))
}

fn resolve_db_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == ":memory:" {
        return Ok(PathBuf::from(":memory:"));
    }
    if trimmed.starts_with("file:") {
        return Err("SQLite 不接受 file: URI（无法通过工作区沙箱校验）".to_string());
    }
    let workspace = SQLITE_WORKSPACE.with(|cell| {
        cell.borrow()
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    });
    crate::sandbox::check_workspace_sandbox(&workspace, trimmed)
        .map_err(|error| format!("沙箱拦截: {error}"))
}

fn open_db(raw_path: &str, options_json: &str) -> Result<u64, String> {
    let path = resolve_db_path(raw_path)?;
    let options: Value = serde_json::from_str(options_json).unwrap_or_else(|_| json!({}));
    let read_only = options
        .get("readOnly")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let timeout_ms = options
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_BUSY_TIMEOUT_MS);
    let enable_foreign_keys = options
        .get("enableForeignKeyConstraints")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
    };
    let conn = Connection::open_with_flags(&path, flags)
        .map_err(|error| format!("打开 SQLite 失败（{}）：{error}", path.display()))?;
    conn.busy_timeout(Duration::from_millis(timeout_ms))
        .map_err(|error| format!("设置 SQLite busy_timeout 失败: {error}"))?;
    conn.pragma_update(
        None,
        "foreign_keys",
        if enable_foreign_keys { "ON" } else { "OFF" },
    )
    .map_err(|error| format!("设置 foreign_keys 失败: {error}"))?;
    // SQLite 自己的运行时限：先于查询发生，避免 randomblob/超长 SQL 在把结果
    // 交给宿主之前就把内存或调用栈吃光。结果集上限（行数/字节）在 query_sql 里。
    conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_RESULT_BYTES as i32);
    conn.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_SQL_BYTES as i32);
    conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 4);
    conn.set_limit(Limit::SQLITE_LIMIT_EXPR_DEPTH, 500);
    conn.set_limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 1_000);
    conn.set_limit(Limit::SQLITE_LIMIT_TRIGGER_DEPTH, 100);
    install_authorizer(&conn)?;

    let id = SQLITE_NEXT_ID.with(|cell| {
        let id = cell.get();
        cell.set(id + 1);
        id
    });
    SQLITE_DBS.with(|cell| {
        cell.borrow_mut().insert(id, conn);
    });
    Ok(id)
}

fn close_db(id: u64) -> Result<(), String> {
    let removed = SQLITE_DBS.with(|cell| cell.borrow_mut().remove(&id));
    if removed.is_none() {
        return Err(format!("SQLite 数据库未打开或已关闭（id={id}）"));
    }
    Ok(())
}

/// 主库路径校验只挡住 `new DatabaseSync(path)`；SQL 里的 `ATTACH` / `VACUUM INTO`
/// 仍然能带任意文件名。authorizer 是 SQLite 在 **prepare 阶段**给出的唯一完整视图，
/// 用它把这些"借 SQL 打开别的文件"的动作也按工作区沙箱判一次。
fn install_authorizer(conn: &Connection) -> Result<(), String> {
    let workspace = SQLITE_WORKSPACE.with(|cell| {
        cell.borrow()
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    });
    conn.authorizer(Some(move |ctx: AuthContext<'_>| match ctx.action {
        AuthAction::Attach { filename } => {
            if attached_database_allowed(&workspace, filename) {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }
        AuthAction::Function { function_name } => {
            if matches!(
                function_name.to_ascii_lowercase().as_str(),
                "load_extension" | "readfile" | "writefile"
            ) {
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }
        AuthAction::Pragma { pragma_name, .. } => {
            if matches!(
                pragma_name.to_ascii_lowercase().as_str(),
                "temp_store_directory" | "data_store_directory"
            ) {
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }
        _ => Authorization::Allow,
    }));
    Ok(())
}

fn attached_database_allowed(workspace: &Path, filename: &str) -> bool {
    let trimmed = filename.trim();
    if trimmed.is_empty() || trimmed == ":memory:" {
        return true;
    }
    if trimmed.starts_with("file:") {
        return false;
    }
    // 🔴 相对路径在 SQLite 里按**进程 CWD**解析，不是按工作区解析。我们无法
    // 在 authorizer 里改写 filename，所以只能要求绝对路径——否则
    // `ATTACH 'x.db'` 会落在宿主的工作目录，而不是插件工作区。
    let path = Path::new(trimmed);
    if !path.is_absolute() {
        return false;
    }
    crate::sandbox::check_workspace_sandbox(workspace, trimmed).is_ok()
}

fn with_db<R>(
    id: u64,
    run: impl FnOnce(&mut Connection) -> Result<R, String>,
) -> Result<R, String> {
    SQLITE_DBS.with(|cell| {
        let mut dbs = cell.borrow_mut();
        let conn = dbs
            .get_mut(&id)
            .ok_or_else(|| format!("SQLite 数据库未打开或已关闭（id={id}）"))?;
        run(conn)
    })
}

fn validate_sql(sql: &str) -> Result<(), String> {
    if sql.trim().is_empty() {
        return Err("SQL 不能为空".to_string());
    }
    if sql.len() > MAX_SQL_BYTES {
        return Err(format!("SQL 超过 {} KB 上限", MAX_SQL_BYTES / 1024));
    }
    Ok(())
}

fn exec_sql(conn: &mut Connection, sql: &str) -> Result<(), String> {
    validate_sql(sql)?;
    conn.execute_batch(sql)
        .map_err(|error| format!("执行 SQL 失败: {error}"))
}

fn prepare_columns(conn: &Connection, sql: &str) -> Result<Vec<String>, String> {
    validate_sql(sql)?;
    let stmt = conn
        .prepare(sql)
        .map_err(|error| format!("准备 SQL 失败: {error}"))?;
    Ok(stmt
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect())
}

fn run_sql(conn: &mut Connection, sql: &str, params: &Value) -> Result<String, String> {
    validate_sql(sql)?;
    let mut stmt = conn
        .prepare(sql)
        .map_err(|error| format!("准备 SQL 失败: {error}"))?;
    bind_params(&mut stmt, params)?;
    let changes = stmt
        .raw_execute()
        .map_err(|error| format!("执行 SQL 失败: {error}"))?;
    Ok(json!({
        "changes": changes,
        "lastInsertRowid": conn.last_insert_rowid(),
    })
    .to_string())
}

fn query_sql(
    conn: &mut Connection,
    sql: &str,
    params: &Value,
    first_only: bool,
) -> Result<String, String> {
    validate_sql(sql)?;
    let mut stmt = conn
        .prepare(sql)
        .map_err(|error| format!("准备 SQL 失败: {error}"))?;
    let columns: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    bind_params(&mut stmt, params)?;

    let mut rows = stmt.raw_query();
    let mut out = Vec::new();
    let mut bytes = 0usize;
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("读取结果失败: {error}"))?
    {
        if out.len() >= MAX_ROWS {
            return Err(format!("查询结果超过 {MAX_ROWS} 行上限"));
        }
        let (value, size) = row_to_json(row, &columns)?;
        bytes += size;
        if bytes > MAX_RESULT_BYTES {
            return Err(format!(
                "查询结果超过 {} MB 上限",
                MAX_RESULT_BYTES / 1024 / 1024
            ));
        }
        out.push(value);
        if first_only {
            break;
        }
    }
    if first_only {
        Ok(out.pop().unwrap_or(Value::Null).to_string())
    } else {
        Ok(Value::Array(out).to_string())
    }
}

fn bind_params(stmt: &mut Statement<'_>, params: &Value) -> Result<(), String> {
    match params {
        Value::Null => {
            if stmt.parameter_count() != 0 {
                return Err(format!(
                    "SQL 需要 {} 个参数，但调用没有提供",
                    stmt.parameter_count()
                ));
            }
        }
        Value::Array(values) => {
            let expected = stmt.parameter_count();
            if values.len() != expected {
                return Err(format!(
                    "位置参数数量不匹配：SQL 需要 {expected} 个，实际 {} 个",
                    values.len()
                ));
            }
            for (index, value) in values.iter().enumerate() {
                let value = json_to_sql(value)?;
                stmt.raw_bind_parameter(index + 1, value)
                    .map_err(|error| format!("绑定第 {} 个参数失败: {error}", index + 1))?;
            }
        }
        Value::Object(map) => {
            let mut supplied = HashSet::new();
            for (key, value) in map {
                supplied.insert(key.trim_start_matches([':', '$', '@']).to_string());
                let index = find_named_parameter(stmt, key)?;
                let value = json_to_sql(value)?;
                stmt.raw_bind_parameter(index, value)
                    .map_err(|error| format!("绑定命名参数 {key} 失败: {error}"))?;
            }
            for index in 1..=stmt.parameter_count() {
                let Some(name) = stmt.parameter_name(index) else {
                    return Err("位置参数不能用命名对象绑定".to_string());
                };
                let bare = name.trim_start_matches([':', '$', '@']);
                if !supplied.contains(bare) {
                    return Err(format!("缺少命名参数: {name}"));
                }
            }
        }
        other => {
            return Err(format!(
                "SQLite 参数必须是数组或对象，收到: {}",
                json_kind(other)
            ))
        }
    }
    Ok(())
}

fn find_named_parameter(stmt: &Statement<'_>, key: &str) -> Result<usize, String> {
    let trimmed = key.trim();
    let mut candidates = vec![trimmed.to_string()];
    if !trimmed.starts_with([':', '$', '@']) {
        candidates.push(format!(":{trimmed}"));
        candidates.push(format!("${trimmed}"));
        candidates.push(format!("@{trimmed}"));
    }
    for candidate in candidates {
        if let Some(index) = stmt
            .parameter_index(&candidate)
            .map_err(|error| format!("解析命名参数 {candidate} 失败: {error}"))?
        {
            return Ok(index);
        }
    }
    Err(format!("SQL 语句没有命名参数: {trimmed}"))
}

fn json_to_sql(value: &Value) -> Result<SqlValue, String> {
    match value {
        Value::Null => Ok(SqlValue::Null),
        Value::Bool(flag) => Ok(SqlValue::Integer(i64::from(*flag))),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                Ok(SqlValue::Integer(integer))
            } else if let Some(real) = number.as_f64() {
                Ok(SqlValue::Real(real))
            } else {
                Err(format!("不支持的 SQLite 数字: {number}"))
            }
        }
        Value::String(text) => Ok(SqlValue::Text(text.clone())),
        Value::Object(map) => {
            if let Some(encoded) = map.get("__ada_blob").and_then(Value::as_str) {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded.as_bytes())
                    .map_err(|error| format!("BLOB 参数不是合法 base64: {error}"))?;
                return Ok(SqlValue::Blob(bytes));
            }
            if let Some(raw) = map.get("__ada_bigint").and_then(Value::as_str) {
                let integer = raw
                    .parse::<i64>()
                    .map_err(|error| format!("BigInt 参数超出 SQLite INTEGER 范围: {error}"))?;
                return Ok(SqlValue::Integer(integer));
            }
            Err("不支持的 SQLite 参数类型：对象（BLOB 请传 Buffer/Uint8Array）".to_string())
        }
        Value::Array(_) => Err("不支持的 SQLite 参数类型：嵌套数组".to_string()),
    }
}

fn row_to_json(row: &rusqlite::Row<'_>, columns: &[String]) -> Result<(Value, usize), String> {
    let mut map = Map::new();
    let mut size = 0usize;
    for (index, name) in columns.iter().enumerate() {
        let value = row
            .get_ref(index)
            .map_err(|error| format!("读取列 {name} 失败: {error}"))?;
        let (value, value_size) = value_ref_to_json(value);
        size += name.len() + value_size;
        map.insert(name.clone(), value);
    }
    Ok((Value::Object(map), size))
}

fn value_ref_to_json(value: ValueRef<'_>) -> (Value, usize) {
    match value {
        ValueRef::Null => (Value::Null, 4),
        ValueRef::Integer(integer) => {
            let value = json!(integer);
            let size = value.to_string().len();
            (value, size)
        }
        ValueRef::Real(real) => {
            if real.is_finite() {
                let value = json!(real);
                let size = value.to_string().len();
                (value, size)
            } else {
                (Value::Null, 4)
            }
        }
        ValueRef::Text(bytes) => {
            let text = String::from_utf8_lossy(bytes).to_string();
            let size = text.len();
            (Value::String(text), size)
        }
        ValueRef::Blob(bytes) => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
            let size = encoded.len() + 16;
            (json!({ "__ada_blob": encoded }), size)
        }
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_value_round_trip_keeps_integers_reals_text_and_blobs() {
        assert!(matches!(json_to_sql(&json!(null)).unwrap(), SqlValue::Null));
        assert!(matches!(
            json_to_sql(&json!(3)).unwrap(),
            SqlValue::Integer(3)
        ));
        assert!(matches!(
            json_to_sql(&json!(3.5)).unwrap(),
            SqlValue::Real(_)
        ));
        assert!(matches!(
            json_to_sql(&json!("x")).unwrap(),
            SqlValue::Text(ref text) if text == "x"
        ));
        assert!(matches!(
            json_to_sql(&json!({"__ada_blob": "aGk="})).unwrap(),
            SqlValue::Blob(ref bytes) if bytes == b"hi"
        ));
    }

    #[test]
    fn file_uri_is_rejected_before_the_sandbox() {
        assert!(resolve_db_path("file:../../outside.db").is_err());
        assert_eq!(
            resolve_db_path(":memory:").unwrap(),
            PathBuf::from(":memory:")
        );
    }
}
