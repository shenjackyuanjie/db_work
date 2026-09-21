//! 页面外壳与 legacy handler 的**会话鉴权**（S5/G1 从 `user_routes/auth.rs` 搬来）。
//!
//! ## 为什么要单独一个模块
//!
//! `handlers_core/{pages,media}.rs`、`handlers_ai/request.rs`、`server/shared.rs` 都**仍是活代码**
//! （页面外壳、`/media/recognition_records/*` 与 `/uploads/*` 两条隐式静态通道、`-v2` 识别链路），
//! 但它们原先依赖 `crate::user_routes::*`。而 `user_routes`（`/user/*`）整棵要随 S5 退役，
//! 所以这层依赖必须先搬走——**这是 G2 能删 `/user/*` 的前提**（见 `W2_S5_PLAN.md` 的 O3）。
//!
//! `user_routes/mod.rs` 原先用再导出指向本模块，因此 `/user/*` 内部那 10 处调用点不需要改；
//! G2 把 `/user/*` 整棵删掉之后，本模块就是**唯一**持有者。
//!
//! ## 会话来源只有一张表（G2 之后）
//!
//! `server::lookup_session_username` **只认 `auth_token`**：O4 把原先「先查 `auth_token`、
//! miss 再查 `app_sessions`」的双表桥第二段与 `app_sessions` 的 DDL 同一步删掉了。
//! 历史 `/user/*` 会话随之失效——但那条树本身也在同一步整棵删除，没有残留调用方。

use axum::http::{HeaderMap, StatusCode, header};
use serde_json::json;
use sqlx::Row;

use crate::server::AppState;

/// 兼容网页的备用通道（`X-Session-Token`）。Cookie 名与 `web::session` 共用 `session_token`。
const SESSION_HEADER_NAME: &str = "x-session-token";

fn token_from_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie_header.split(';').find_map(|part| {
        let trimmed = part.trim();
        trimmed
            .strip_prefix("session_token=")
            .and_then(|value| (!value.is_empty()).then(|| value.to_string()))
    })
}

/// 三通道取 token：Cookie `session_token` → `X-Session-Token`。
///
/// 注意本函数**不**读 `Authorization`；Bearer 头由 `compat::auth` / `web::session` 负责。
pub(crate) fn extract_auth_token(headers: &HeaderMap) -> Option<String> {
    token_from_cookie(headers).or_else(|| {
        headers
            .get(SESSION_HEADER_NAME)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

/// 校验会话，返回 `(token, username)`。失败体是 `{"error": ...}`（页面壳与 legacy handler 用的形状）。
pub(crate) async fn ensure_authenticated(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(String, String), (StatusCode, serde_json::Value)> {
    let token = match extract_auth_token(headers) {
        Some(token) => token,
        None => {
            return Err((
                StatusCode::UNAUTHORIZED,
                json!({ "error": "Missing session token" }),
            ));
        }
    };

    // 单表查询（原双表桥的第二段已随 `app_sessions` 的 DDL 一起删除）。
    // 理由见 `server/shared.rs::lookup_session_username` 的文档注释。
    let username = crate::server::lookup_session_username(&state.db, &token)
        .await
        .map_err(|err| {
            tracing::error!(%err, "会话查询失败");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Session lookup failed" }),
            )
        })?;

    let Some(username) = username else {
        return Err((
            StatusCode::UNAUTHORIZED,
            json!({ "error": "Invalid token" }),
        ));
    };

    Ok((token, username))
}

/// 库管理员校验。
///
/// **读的是契约表 `"user"` 的加法列 `is_admin`，不是旧的 `app_users`**（搬家时一并改的）：
/// `app_users` 是退役目标，而网页侧的 `web::session::{lookup_is_admin, require_admin}`
/// 已经统一读 `"user".is_admin` —— 两处读不同的表会导致「网页登录的管理员打不开后台页面」。
pub(crate) async fn ensure_admin(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<String, (StatusCode, serde_json::Value)> {
    let (_, admin_username) = ensure_authenticated(state, headers).await?;

    let row = sqlx::query(r#"SELECT is_admin FROM "user" WHERE username = $1 LIMIT 1"#)
        .bind(&admin_username)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Admin lookup failed" }),
            )
        })?;

    let is_admin = row
        .and_then(|row| row.try_get::<bool, _>("is_admin").ok())
        .unwrap_or(false);

    if !is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            json!({ "error": "User is not an admin" }),
        ));
    }

    Ok(admin_username)
}
