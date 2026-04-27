use axum::{
    Json,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;
use sqlx::Row;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::{models::RequestedRole, server::AppState, system_settings::SystemSettings};

const SESSION_COOKIE_NAME: &str = "session_token";
const SESSION_HEADER_NAME: &str = "x-session-token";
const SESSION_MAX_AGE_SECONDS: u64 = 30 * 24 * 60 * 60;

pub(super) fn generate_token() -> String {
    Uuid::new_v4().to_string()
}

pub(super) fn hash_password(password: &str) -> String {
    blake3::hash(password.as_bytes()).to_string()
}

pub(super) fn verify_password(hash: &str, password: &str) -> bool {
    hash == blake3::hash(password.as_bytes()).to_string()
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
) -> Result<HeaderValue, (StatusCode, serde_json::Value)> {
    let cookie = format!(
        "{SESSION_COOKIE_NAME}={token}; Path=/; SameSite=Lax; Max-Age={SESSION_MAX_AGE_SECONDS}"
    );
    HeaderValue::from_str(&cookie).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Failed to build session cookie" }),
        )
    })
}

pub(super) fn build_clear_cookie() -> Result<HeaderValue, (StatusCode, serde_json::Value)> {
    let cookie = format!("{SESSION_COOKIE_NAME}=; Path=/; SameSite=Lax; Max-Age=0");
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
    headers
        .get(SESSION_HEADER_NAME)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| token_from_cookie(headers))
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

    let row = sqlx::query("SELECT username FROM app_sessions WHERE token = $1 LIMIT 1")
        .bind(&token)
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