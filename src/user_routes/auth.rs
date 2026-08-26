use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use axum::{
    Json,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use rand_core::OsRng;
use serde_json::json;
use sqlx::Row;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::{models::RequestedRole, server::AppState, system_settings::SystemSettings};

const SESSION_COOKIE_NAME: &str = "session_token";
const SESSION_HEADER_NAME: &str = "x-session-token";
pub(crate) const SESSION_MAX_AGE_SECONDS: u64 = 30 * 24 * 60 * 60;

pub(super) fn generate_token() -> String {
    Uuid::new_v4().to_string()
}

pub(super) fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| anyhow::anyhow!("密码哈希失败: {}", err))
}

pub(super) fn verify_password(hash: &str, password: &str) -> bool {
    if hash.starts_with("$argon2") {
        return PasswordHash::new(hash)
            .ok()
            .and_then(|parsed| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .ok()
            })
            .is_some();
    }

    // 兼容已有账户：首次成功登录后会迁移为 Argon2id。
    hash == blake3::hash(password.as_bytes()).to_string()
}

pub(super) fn needs_password_rehash(hash: &str) -> bool {
    !hash.starts_with("$argon2")
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(super) fn app_response(
    code: u16,
    message: impl Into<String>,
    data: serde_json::Value,
) -> serde_json::Value {
    json!({
        "code": code,
        "message": message.into(),
        "data": data,
        "timestamp": now_millis()
    })
}

pub(super) fn requested_role_label(role: &RequestedRole) -> &'static str {
    if role.is_admin() { "admin" } else { "user" }
}

pub(crate) fn parse_requested_role(text: &str) -> RequestedRole {
    if text.eq_ignore_ascii_case("admin") {
        RequestedRole::Admin
    } else {
        RequestedRole::User
    }
}

/// 历史 API 允许客户端携带 username；现在仅接受与当前会话一致的值。
pub(crate) fn username_matches_session(requested: Option<&str>, session_username: &str) -> bool {
    requested
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none_or(|value| value == session_username)
}

pub(super) fn user_payload(username: &str, created_at: u64) -> serde_json::Value {
    json!({
        "id": username,
        "username": username,
        "email": null,
        "orchard_address": null,
        "latitude": null,
        "longitude": null,
        "created_at": created_at
    })
}

pub(super) async fn current_system_settings(state: &AppState) -> SystemSettings {
    match crate::system_settings::load_system_settings(&state.db).await {
        Ok(settings) => settings,
        Err(err) => {
            tracing::error!("读取系统设置失败，回退默认值: {}", err);
            SystemSettings::default()
        }
    }
}

pub(super) fn pending_approval_response(
    requested_role: &RequestedRole,
    hint: Option<&str>,
) -> Response {
    (
        StatusCode::ACCEPTED,
        Json(app_response(
            202,
            "pending_approval",
            json!({
                "requested_role": requested_role,
                "hint": hint
            }),
        )),
    )
        .into_response()
}

pub(super) fn build_login_cookie(
    token: &str,
    secure: bool,
) -> Result<HeaderValue, (StatusCode, serde_json::Value)> {
    let secure_flag = if secure { "; Secure" } else { "" };
    let cookie = format!(
        "{SESSION_COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_MAX_AGE_SECONDS}{secure_flag}"
    );
    HeaderValue::from_str(&cookie).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Failed to build session cookie" }),
        )
    })
}

pub(super) fn build_clear_cookie() -> Result<HeaderValue, (StatusCode, serde_json::Value)> {
    let cookie = format!("{SESSION_COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    HeaderValue::from_str(&cookie).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Failed to clear session cookie" }),
        )
    })
}

fn token_from_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie_header.split(';').find_map(|part| {
        let trimmed = part.trim();
        trimmed
            .strip_prefix("session_token=")
            .and_then(|value| (!value.is_empty()).then(|| value.to_string()))
    })
}

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

    let now = now_secs() as i64;
    let row = sqlx::query(
        "SELECT username FROM app_sessions WHERE token = $1 AND expires_at > $2 LIMIT 1",
    )
    .bind(&token)
    .bind(now)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Session lookup failed" }),
        )
    })?;

    let username = match row.and_then(|row| row.try_get::<String, _>("username").ok()) {
        Some(name) => name,
        None => {
            return Err((
                StatusCode::UNAUTHORIZED,
                json!({ "error": "Invalid token" }),
            ));
        }
    };

    Ok((token, username))
}

pub(crate) async fn ensure_admin(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<String, (StatusCode, serde_json::Value)> {
    let (_, admin_username) = ensure_authenticated(state, headers).await?;

    let row = sqlx::query("SELECT is_admin FROM app_users WHERE username = $1 LIMIT 1")
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

#[cfg(test)]
mod tests {
    use super::{
        build_login_cookie, hash_password, needs_password_rehash, username_matches_session,
        verify_password,
    };

    #[test]
    fn new_password_hash_uses_argon2id_and_verifies() {
        let hash = hash_password("correct horse battery staple").expect("hash should be created");
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password(&hash, "correct horse battery staple"));
        assert!(!verify_password(&hash, "wrong password"));
        assert!(!needs_password_rehash(&hash));
    }

    #[test]
    fn legacy_blake3_hash_remains_login_compatible_for_migration() {
        let legacy = blake3::hash(b"old password").to_string();
        assert!(verify_password(&legacy, "old password"));
        assert!(needs_password_rehash(&legacy));
    }

    #[test]
    fn browser_cookie_is_http_only_and_optionally_secure() {
        let local = build_login_cookie("test-token", false)
            .expect("cookie should be created")
            .to_str()
            .expect("cookie header should be text")
            .to_string();
        assert!(local.contains("HttpOnly"));
        assert!(!local.contains("; Secure"));

        let production = build_login_cookie("test-token", true)
            .expect("cookie should be created")
            .to_str()
            .expect("cookie header should be text")
            .to_string();
        assert!(production.contains("; Secure"));
    }

    #[test]
    fn legacy_username_must_match_current_session() {
        assert!(username_matches_session(None, "alice"));
        assert!(username_matches_session(Some(""), "alice"));
        assert!(username_matches_session(Some(" alice "), "alice"));
        assert!(!username_matches_session(Some("bob"), "alice"));
    }
}
