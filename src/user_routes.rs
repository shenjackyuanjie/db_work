use axum::{
    Router,
    extract::{Json, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
    routing::post,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;
use uuid::Uuid;

use crate::models::RequestedRole;
use crate::server::AppState;

const SESSION_COOKIE_NAME: &str = "session_token";
const SESSION_HEADER_NAME: &str = "x-session-token";
const SESSION_MAX_AGE_SECONDS: u64 = 30 * 24 * 60 * 60;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub invitation_code: String,
    #[serde(default)]
    pub requested_role: RequestedRole,
}

#[derive(Debug, Deserialize)]
pub struct SetAdminRequest {
    pub target_username: String,
    pub make_admin: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateInvitationRequest {
    pub ttl_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ApprovePendingUserRequest {
    pub username: String,
}

#[derive(Debug, Deserialize)]
pub struct RejectPendingUserRequest {
    pub username: String,
}

#[derive(Debug, Serialize)]
pub struct PublicUser {
    pub username: String,
    pub is_admin: bool,
    pub created_at: u64,
}

#[derive(Debug, Serialize)]
pub struct PendingPublicUser {
    pub username: String,
    pub created_at: u64,
    pub requested_role: RequestedRole,
}

fn generate_token() -> String {
    Uuid::new_v4().to_string()
}

fn hash_password(pw: &str) -> String {
    blake3::hash(pw.as_bytes()).to_string()
}

fn verify_password(hash: &str, pw: &str) -> bool {
    hash == blake3::hash(pw.as_bytes()).to_string()
}

fn now_secs() -> u64 {
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

fn app_response(
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

fn requested_role_label(role: &RequestedRole) -> &'static str {
    if role.is_admin() { "admin" } else { "user" }
}

fn parse_requested_role(text: &str) -> RequestedRole {
    if text.eq_ignore_ascii_case("admin") {
        RequestedRole::Admin
    } else {
        RequestedRole::User
    }
}

fn user_payload(username: &str, created_at: u64) -> serde_json::Value {
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

fn build_login_cookie(token: &str) -> Result<HeaderValue, (StatusCode, serde_json::Value)> {
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

fn build_clear_cookie() -> Result<HeaderValue, (StatusCode, serde_json::Value)> {
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

pub fn extract_auth_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(SESSION_HEADER_NAME)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string)
        .or_else(|| token_from_cookie(headers))
}

pub async fn ensure_authenticated(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(String, String), (StatusCode, serde_json::Value)> {
    let token = match extract_auth_token(headers) {
        Some(t) => t,
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

    let username = match row.and_then(|r| r.try_get::<String, _>("username").ok()) {
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

pub async fn ensure_admin(
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
        .and_then(|r| r.try_get::<bool, _>("is_admin").ok())
        .unwrap_or(false);

    if !is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            json!({ "error": "User is not an admin" }),
        ));
    }
    Ok(admin_username)
}

pub async fn login_handler(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> impl IntoResponse {
    let username = payload.username.trim();
    let password = payload.password.trim();
    info!("用户请求登录: username={}", username);

    if username.is_empty() || password.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(app_response(
                400,
                "Username and password are required",
                serde_json::Value::Null,
            )),
        )
            .into_response();
    }

    let row = match sqlx::query(
        "SELECT password_hash, is_admin, created_at FROM app_users WHERE username = $1 LIMIT 1",
    )
    .bind(username)
    .fetch_optional(&state.db)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(app_response(
                    500,
                    format!("db error: {}", e),
                    serde_json::Value::Null,
                )),
            )
                .into_response();
        }
    };

    let Some(row) = row else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(app_response(
                401,
                "Invalid credentials",
                serde_json::Value::Null,
            )),
        )
            .into_response();
    };

    let password_hash: String = row.try_get("password_hash").unwrap_or_default();
    let is_admin: bool = row.try_get("is_admin").unwrap_or(false);
    let created_at: i64 = row.try_get("created_at").unwrap_or(0);

    if !verify_password(&password_hash, password) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(app_response(
                401,
                "Invalid credentials",
                serde_json::Value::Null,
            )),
        )
            .into_response();
    }

    let token = generate_token();
    let now = now_secs() as i64;

    if let Err(e) =
        sqlx::query("INSERT INTO app_sessions (token, username, created_at) VALUES ($1, $2, $3)")
            .bind(&token)
            .bind(username)
            .bind(now)
            .execute(&state.db)
            .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(app_response(
                500,
                format!("session save failed: {}", e),
                serde_json::Value::Null,
            )),
        )
            .into_response();
    }

    let _ = sqlx::query("UPDATE app_users SET session_token = $1 WHERE username = $2")
        .bind(&token)
        .bind(username)
        .execute(&state.db)
        .await;

    let cookie_header = match build_login_cookie(&token) {
        Ok(v) => v,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, cookie_header);

    (
        StatusCode::OK,
        headers,
        Json(app_response(
            200,
            "Login successful",
            json!({
                "id": username,
                "username": username,
                "email": null,
                "orchard_address": null,
                "latitude": null,
                "longitude": null,
                "created_at": created_at.max(0) as u64,
                "token": token,
                "is_admin": is_admin
            }),
        )),
    )
        .into_response()
}

pub async fn register_handler(
    State(state): State<AppState>,
    Json(payload): Json<RegisterRequest>,
) -> impl IntoResponse {
    let username = payload.username.trim().to_string();
    let password = payload.password.trim().to_string();

    if username.is_empty() || password.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(app_response(
                400,
                "Username and password are required",
                serde_json::Value::Null,
            )),
        )
            .into_response();
    }

    let exists = sqlx::query("SELECT 1 FROM app_users WHERE username = $1 LIMIT 1")
        .bind(&username)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .is_some();
    if exists {
        return (
            StatusCode::CONFLICT,
            Json(app_response(
                409,
                "Username already exists",
                serde_json::Value::Null,
            )),
        )
            .into_response();
    }

    let pending_exists = sqlx::query("SELECT 1 FROM app_pending_users WHERE username = $1 LIMIT 1")
        .bind(&username)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .is_some();
    if pending_exists {
        return (
            StatusCode::CONFLICT,
            Json(app_response(
                409,
                "Username already pending approval",
                serde_json::Value::Null,
            )),
        )
            .into_response();
    }

    let now = now_secs();
    let password_hash = hash_password(&password);
    let invitation_code = payload.invitation_code.trim();

    if invitation_code.is_empty() {
        if !payload.requested_role.is_admin() {
            let result = sqlx::query(
                "INSERT INTO app_users (username, password_hash, is_admin, created_at, session_token) VALUES ($1, $2, $3, $4, NULL)",
            )
            .bind(&username)
            .bind(&password_hash)
            .bind(false)
            .bind(now as i64)
            .execute(&state.db)
            .await;

            if let Err(e) = result {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(app_response(
                        500,
                        format!("db error: {}", e),
                        serde_json::Value::Null,
                    )),
                )
                    .into_response();
            }

            return (
                StatusCode::OK,
                Json(app_response(
                    200,
                    "Registration successful",
                    user_payload(&username, now),
                )),
            )
                .into_response();
        }

        let result = sqlx::query(
            "INSERT INTO app_pending_users (username, password_hash, created_at, requested_role) VALUES ($1, $2, $3, $4)",
        )
        .bind(&username)
        .bind(&password_hash)
        .bind(now as i64)
        .bind(requested_role_label(&payload.requested_role))
        .execute(&state.db)
        .await;

        if let Err(e) = result {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(app_response(
                    500,
                    format!("db error: {}", e),
                    serde_json::Value::Null,
                )),
            )
                .into_response();
        }

        return (
            StatusCode::ACCEPTED,
            Json(app_response(
                202,
                "pending_approval",
                json!({ "requested_role": payload.requested_role }),
            )),
        )
            .into_response();
    }

    let invite_row =
        match sqlx::query("SELECT used, expires_at FROM app_invitations WHERE code = $1 LIMIT 1")
            .bind(invitation_code)
            .fetch_optional(&state.db)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(app_response(
                        500,
                        format!("db error: {}", e),
                        serde_json::Value::Null,
                    )),
                )
                    .into_response();
            }
        };

    let Some(invite_row) = invite_row else {
        return (
            StatusCode::BAD_REQUEST,
            Json(app_response(
                400,
                "Invalid or expired invitation",
                json!({ "hint": "邀请码错误时可清空邀请码并提交审核申请" }),
            )),
        )
            .into_response();
    };

    let used: bool = invite_row.try_get("used").unwrap_or(true);
    let expires_at: i64 = invite_row.try_get("expires_at").unwrap_or(0);
    if used || expires_at <= now as i64 {
        return (
            StatusCode::BAD_REQUEST,
            Json(app_response(
                400,
                "Invalid or expired invitation",
                json!({ "hint": "邀请码错误时可清空邀请码并提交审核申请" }),
            )),
        )
            .into_response();
    }

    let _ = sqlx::query("UPDATE app_invitations SET used = TRUE WHERE code = $1")
        .bind(invitation_code)
        .execute(&state.db)
        .await;

    if let Err(e) = sqlx::query(
        "INSERT INTO app_users (username, password_hash, is_admin, created_at, session_token) VALUES ($1, $2, $3, $4, NULL)",
    )
    .bind(&username)
    .bind(&password_hash)
    .bind(payload.requested_role.is_admin())
    .bind(now as i64)
    .execute(&state.db)
    .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(app_response(500, format!("db error: {}", e), serde_json::Value::Null)),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(app_response(
            200,
            "Registration successful",
            user_payload(&username, now),
        )),
    )
        .into_response()
}

pub async fn logout_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = match extract_auth_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"Missing token"})),
            )
                .into_response();
        }
    };

    let clear_cookie = match build_clear_cookie() {
        Ok(v) => v,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    let mut res_headers = HeaderMap::new();
    res_headers.insert(header::SET_COOKIE, clear_cookie);

    let removed = sqlx::query("DELETE FROM app_sessions WHERE token = $1")
        .bind(&token)
        .execute(&state.db)
        .await;

    match removed {
        Ok(res) if res.rows_affected() > 0 => {
            let _ =
                sqlx::query("UPDATE app_users SET session_token = NULL WHERE session_token = $1")
                    .bind(&token)
                    .execute(&state.db)
                    .await;
            (
                StatusCode::OK,
                res_headers,
                Json(json!({"status":"logged out"})),
            )
                .into_response()
        }
        Ok(_) => (
            StatusCode::BAD_REQUEST,
            res_headers,
            Json(json!({"error":"Invalid token"})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            res_headers,
            Json(json!({"error":"Failed to logout"})),
        )
            .into_response(),
    }
}

pub async fn validate_token_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = match extract_auth_token(&headers) {
        Some(t) => t,
        None => return (StatusCode::OK, Json(json!({"valid": false}))).into_response(),
    };

    let row = sqlx::query(
        "SELECT u.username, u.is_admin FROM app_sessions s JOIN app_users u ON u.username = s.username WHERE s.token = $1 LIMIT 1",
    )
    .bind(&token)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(r)) => (
            StatusCode::OK,
            Json(json!({
                "valid": true,
                "username": r.try_get::<String, _>("username").unwrap_or_default(),
                "is_admin": r.try_get::<bool, _>("is_admin").unwrap_or(false)
            })),
        )
            .into_response(),
        _ => (StatusCode::OK, Json(json!({"valid": false}))).into_response(),
    }
}

pub async fn me_handler(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let (_, username) = match ensure_authenticated(&state, &headers).await {
        Ok(v) => v,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    let row = sqlx::query(
        "SELECT username, is_admin, created_at FROM app_users WHERE username = $1 LIMIT 1",
    )
    .bind(&username)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(r)) => (
            StatusCode::OK,
            Json(json!({
                "username": r.try_get::<String, _>("username").unwrap_or(username),
                "is_admin": r.try_get::<bool, _>("is_admin").unwrap_or(false),
                "created_at": r.try_get::<i64, _>("created_at").unwrap_or(0)
            })),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error":"User not found"})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Failed to get current user"})),
        )
            .into_response(),
    }
}

include!("user_routes/admin.rs");

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/login", post(login_handler))
        .route("/register", post(register_handler))
        .route("/logout", post(logout_handler))
        .route("/validate", post(validate_token_handler))
        .route("/me", post(me_handler))
        .route("/admin/set_admin", post(set_admin_handler))
        .route("/admin/invitations/create", post(create_invitation_handler))
        .route("/admin/invitations/list", post(list_invitations_handler))
        .route("/admin/users/list", post(list_users_handler))
        .route("/admin/pending/list", post(list_pending_users_handler))
        .route("/admin/pending/approve", post(approve_pending_user_handler))
        .route("/admin/pending/reject", post(reject_pending_user_handler))
        .with_state(state)
}
