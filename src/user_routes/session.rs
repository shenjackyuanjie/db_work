use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;
use sqlx::Row;
use tracing::info;

use crate::server::AppState;

use super::{
    auth::{
        SESSION_MAX_AGE_SECONDS, app_response, build_clear_cookie, build_login_cookie,
        current_system_settings, ensure_authenticated, extract_auth_token, generate_token,
        hash_password, needs_password_rehash, now_secs, verify_password,
    },
    dto::LoginRequest,
};

pub(crate) async fn login_handler(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Response {
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
        "SELECT password_hash, is_admin, created_at, latitude, longitude FROM app_users WHERE username = $1 LIMIT 1",
    )
    .bind(username)
    .fetch_optional(&state.db)
    .await
    {
        Ok(row) => row,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(app_response(
                    500,
                    format!("db error: {}", err),
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
    let latitude: Option<f64> = row.try_get("latitude").unwrap_or(None);
    let longitude: Option<f64> = row.try_get("longitude").unwrap_or(None);

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

    let settings = current_system_settings(&state).await;
    if settings.maintenance_mode && !is_admin {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(app_response(
                503,
                "系统维护中，仅管理员可登录",
                json!({
                    "maintenance_mode": true
                }),
            )),
        )
            .into_response();
    }

    if needs_password_rehash(&password_hash) {
        match hash_password(password) {
            Ok(new_hash) => {
                if let Err(err) =
                    sqlx::query("UPDATE app_users SET password_hash = $1 WHERE username = $2")
                        .bind(new_hash)
                        .bind(username)
                        .execute(&state.db)
                        .await
                {
                    tracing::warn!(%err, username, "旧密码哈希迁移失败，将在下次登录重试");
                }
            }
            Err(err) => tracing::warn!(%err, username, "生成新的密码哈希失败，将在下次登录重试"),
        }
    }

    let token = generate_token();
    let now = now_secs() as i64;
    let expires_at = now + SESSION_MAX_AGE_SECONDS as i64;

    if let Err(err) =
        sqlx::query("INSERT INTO app_sessions (token, username, created_at, expires_at) VALUES ($1, $2, $3, $4)")
            .bind(&token)
            .bind(username)
            .bind(now)
            .bind(expires_at)
            .execute(&state.db)
            .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(app_response(
                500,
                format!("session save failed: {}", err),
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

    let cookie_header = match build_login_cookie(&token, state.secure_session_cookie) {
        Ok(value) => value,
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
                "latitude": latitude,
                "longitude": longitude,
                "created_at": created_at.max(0) as u64,
                "token": token,
                "is_admin": is_admin
            }),
        )),
    )
        .into_response()
}

pub(crate) async fn logout_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let token = match extract_auth_token(&headers) {
        Some(token) => token,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"Missing token"})),
            )
                .into_response();
        }
    };

    let clear_cookie = match build_clear_cookie() {
        Ok(value) => value,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    let mut response_headers = HeaderMap::new();
    response_headers.insert(header::SET_COOKIE, clear_cookie);

    let removed = sqlx::query("DELETE FROM app_sessions WHERE token = $1")
        .bind(&token)
        .execute(&state.db)
        .await;

    match removed {
        Ok(result) if result.rows_affected() > 0 => {
            let _ =
                sqlx::query("UPDATE app_users SET session_token = NULL WHERE session_token = $1")
                    .bind(&token)
                    .execute(&state.db)
                    .await;
            (
                StatusCode::OK,
                response_headers,
                Json(json!({"status":"logged out"})),
            )
                .into_response()
        }
        Ok(_) => (
            StatusCode::BAD_REQUEST,
            response_headers,
            Json(json!({"error":"Invalid token"})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            response_headers,
            Json(json!({"error":"Failed to logout"})),
        )
            .into_response(),
    }
}

pub(crate) async fn validate_token_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let settings = current_system_settings(&state).await;

    let token = match extract_auth_token(&headers) {
        Some(token) => token,
        None => {
            return (
                StatusCode::OK,
                Json(json!({
                    "valid": false,
                    "maintenance_mode": settings.maintenance_mode,
                    "open_registration": settings.open_registration
                })),
            )
                .into_response();
        }
    };

    let row = sqlx::query(
        "SELECT u.username, u.is_admin FROM app_sessions s JOIN app_users u ON u.username = s.username WHERE s.token = $1 AND s.expires_at > $2 LIMIT 1",
    )
    .bind(&token)
    .bind(now_secs() as i64)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(row)) => (
            StatusCode::OK,
            Json(json!({
                "valid": true,
                "username": row.try_get::<String, _>("username").unwrap_or_default(),
                "is_admin": row.try_get::<bool, _>("is_admin").unwrap_or(false),
                "maintenance_mode": settings.maintenance_mode,
                "open_registration": settings.open_registration
            })),
        )
            .into_response(),
        _ => (
            StatusCode::OK,
            Json(json!({
                "valid": false,
                "maintenance_mode": settings.maintenance_mode,
                "open_registration": settings.open_registration
            })),
        )
            .into_response(),
    }
}

pub(crate) async fn me_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let (_, username) = match ensure_authenticated(&state, &headers).await {
        Ok(value) => value,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    let row = sqlx::query(
        "SELECT username, is_admin, created_at, latitude, longitude FROM app_users WHERE username = $1 LIMIT 1",
    )
    .bind(&username)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(row)) => (
            StatusCode::OK,
            Json(json!({
                "username": row.try_get::<String, _>("username").unwrap_or(username),
                "is_admin": row.try_get::<bool, _>("is_admin").unwrap_or(false),
                "created_at": row.try_get::<i64, _>("created_at").unwrap_or(0),
                "latitude": row.try_get::<Option<f64>, _>("latitude").unwrap_or(None),
                "longitude": row.try_get::<Option<f64>, _>("longitude").unwrap_or(None)
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
