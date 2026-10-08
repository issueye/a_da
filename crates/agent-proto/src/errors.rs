use serde::{Deserialize, Serialize};
use thiserror::Error;

/// JSON-RPC 2.0 标准错误码
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcErrorCode {
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    InternalError = -32603,
}

impl RpcErrorCode {
    pub const fn code(&self) -> i32 {
        *self as i32
    }
}

/// 应用级错误码（-32000..-32099）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppErrorCode {
    ProtocolVersionMismatch = -32000,
    Unauthorized = -32001,
    NotFound = -32002,
    NotReady = -32003,
    Denied = -32004,
    Timeout = -32005,
    AlreadyAnswered = -32006,
    WorkspaceDenied = -32007,
    TooLarge = -32008,
    Cancelled = -32009,
    NeedResync = -32010,
    Busy = -32011,
    ConfigInvalid = -32012,
}

impl AppErrorCode {
    pub const fn code(&self) -> i32 {
        *self as i32
    }
}

/// 协议错误
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
#[error("{message} (code: {code})")]
pub struct ProtocolError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl ProtocolError {
    pub fn new(code: i32, message: impl Into<String>, data: Option<serde_json::Value>) -> Self {
        Self {
            code,
            message: message.into(),
            data,
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(AppErrorCode::Unauthorized.code(), message, None)
    }

    pub fn method_not_found(method: impl Into<String>) -> Self {
        Self::new(
            RpcErrorCode::MethodNotFound.code(),
            format!("方法不存在: {}", method.into()),
            None,
        )
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::InvalidParams.code(), message, None)
    }

    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::InternalError.code(), message, None)
    }
}
