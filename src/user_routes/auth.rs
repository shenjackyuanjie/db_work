use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use axum::{
    Json,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use rand_core::OsRng;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::{models::RequestedRole, server::AppState, system_settings::SystemSettings};

const SESSION_COOKIE_NAME: &str = "session_token";
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

#[cfg(test)]
mod tests {
    use super::{build_login_cookie, hash_password, needs_password_rehash, verify_password};

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
}
