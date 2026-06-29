//! 修改网站后台登录密码(nginx Basic Auth 的 admin 密码)。
//!
//! 设计:app 自己管理 htpasswd 文件(默认 /etc/new-alpha-trade/htpasswd,由 app 用户拥有),
//! 验证旧密码 → 写入新 bcrypt($2y$)哈希。nginx 每次请求重读该文件 → 新密码立即生效。
//! 全程不碰 root、不调 shell(避免命令注入 / 密码进 ps),用 bcrypt 库直接算哈希原地写回。

use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/change-password", post(change_password))
}

#[derive(Deserialize)]
struct ChangePwReq {
    old_password: String,
    new_password: String,
}

fn htpasswd_path() -> String {
    // 默认放 APP_DIR/data 下(引擎可写,不受 systemd ProtectSystem=full 的 /etc 只读限制)
    std::env::var("HTPASSWD_PATH").unwrap_or_else(|_| "/opt/new-alpha-trade/data/htpasswd".into())
}
fn basic_user() -> String {
    std::env::var("BASIC_USER").unwrap_or_else(|_| "admin".into())
}
fn err(status: StatusCode, msg: impl ToString) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg.to_string() })))
}

async fn change_password(
    Json(req): Json<ChangePwReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if req.new_password.chars().count() < 6 {
        return Err(err(StatusCode::BAD_REQUEST, "新密码至少 6 位"));
    }
    if req.new_password == req.old_password {
        return Err(err(StatusCode::BAD_REQUEST, "新密码不能和旧密码相同"));
    }
    let path = htpasswd_path();
    let user = basic_user();

    let content = std::fs::read_to_string(&path).map_err(|e| {
        err(StatusCode::INTERNAL_SERVER_ERROR, format!("读取密码文件失败({path}): {e}"))
    })?;
    let prefix = format!("{user}:");
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let idx = lines
        .iter()
        .position(|l| l.starts_with(&prefix))
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, format!("密码文件里找不到用户 {user}")))?;

    // 1) 验证旧密码(防别人借已登录会话偷改)
    let cur_hash = lines[idx][prefix.len()..].trim();
    if !bcrypt::verify(&req.old_password, cur_hash).unwrap_or(false) {
        return Err(err(StatusCode::UNAUTHORIZED, "旧密码不正确"));
    }

    // 2) 算新 bcrypt 哈希,转 $2y$ 前缀(nginx/htpasswd 标准兼容)
    let new_hash = bcrypt::hash(&req.new_password, 10)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("生成哈希失败: {e}")))?
        .replacen("$2b$", "$2y$", 1);
    lines[idx] = format!("{user}:{new_hash}");

    // 3) 原地写回(文件由 app 用户拥有,644 → nginx 可读、app 可写,无需 root)
    std::fs::write(&path, lines.join("\n") + "\n")
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("写入密码文件失败: {e}")))?;

    tracing::info!(%user, "web panel password changed");
    Ok(Json(json!({ "ok": true })))
}
