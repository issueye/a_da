//! a_da 插件库平台（Plugin Hub）。
//!
//! 结构按 MVC 拆分：
//! - `models/`：SQLx 行类型与查询
//! - `controllers/`：HTTP 处理器（薄，负责校验/编排）
//! - `views/` + `templates/`：Askama SSR
//! - `auth/`：密码、签名 Cookie 会话、CSRF
//! - `storage.rs`：插件 ZIP 资产落盘与安全校验

pub mod app;
pub mod auth;
pub mod config;
pub mod controllers;
pub mod db;
pub mod error;
pub mod markdown;
pub mod models;
pub mod storage;
pub mod views;

pub use app::{AppState, app};
