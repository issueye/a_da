mod pages;

pub use pages::*;

use askama::Template;
use axum::response::Html;
use chrono::{DateTime, Utc};

use crate::auth::AuthCtx;
use crate::error::AppError;

#[derive(Debug, Clone)]
pub struct UserView {
    pub username: String,
}

#[derive(Debug, Clone)]
pub struct Flash {
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Common {
    pub current_user: Option<UserView>,
    pub csrf: String,
    pub flash: Option<Flash>,
    pub query: String,
}

impl Common {
    pub fn anonymous() -> Self {
        Self {
            current_user: None,
            csrf: String::new(),
            flash: None,
            query: String::new(),
        }
    }

    pub fn from_auth(auth: &AuthCtx, flash: Option<Flash>, query: &str) -> Self {
        Self {
            current_user: auth.user.as_ref().map(|u| UserView {
                username: u.username.clone(),
            }),
            csrf: auth.csrf().to_string(),
            flash,
            query: query.to_string(),
        }
    }
}

impl Flash {
    pub fn from_query(ok: Option<&str>, err: Option<&str>) -> Option<Self> {
        if let Some(code) = err {
            let message = match code {
                "bad_request" => "请求参数不完整或格式不正确",
                "duplicate" => "名称/版本已存在，请换一个",
                "forbidden" => "没有权限执行这个操作",
                "upload" => "上传失败，请检查 ZIP 后重试",
                "no_release" => "这个插件还没有发布版本，无法从插件包同步",
                _ => "操作失败，请稍后重试",
            };
            return Some(Self {
                kind: "error".to_string(),
                message: message.to_string(),
            });
        }
        let code = ok?;
        let message = match code {
            "registered" => "注册成功，欢迎加入",
            "logged_in" => "已登录",
            "logged_out" => "已退出登录",
            "created" => "插件创建成功",
            "updated" => "插件已更新",
            "deleted" => "插件已删除",
            "released" => "新版本已发布",
            "starred" => "已 Star",
            "unstarred" => "已取消 Star",
            "member_added" => "成员已加入组织",
            "member_removed" => "成员已移出组织",
            "team_created" => "团队已创建",
            "team_member_added" => "成员已加入团队",
            "token_revoked" => "API Token 已撤销",
            "webhook_created" => "Webhook 已创建",
            "webhook_deleted" => "Webhook 已删除",
            "synced" => "已从最新版本插件包同步简介与 README",
            _ => return None,
        };
        Some(Self {
            kind: "success".to_string(),
            message: message.to_string(),
        })
    }
}

pub fn render<T: Template>(tpl: &T) -> Result<Html<String>, AppError> {
    tpl.render()
        .map(Html)
        .map_err(|e| AppError::Internal(format!("模板渲染失败: {e}")))
}

pub fn now_ts() -> i64 {
    Utc::now().timestamp()
}

pub fn format_ts(ts: i64) -> String {
    // 存的是 Unix 秒（UTC），**展示按本机时区**——本地优先的平台把 10:24 显示成 02:24
    // 只会让人以为发布记录错了。
    DateTime::<Utc>::from_timestamp(ts, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "-".to_string())
}
