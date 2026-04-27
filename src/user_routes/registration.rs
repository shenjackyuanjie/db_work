use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use sqlx::Row;

use crate::server::AppState;

use super::{
    auth::{
        app_response, current_system_settings, hash_password, now_secs, pending_approval_response,
        requested_role_label, user_payload,
    },
    dto::RegisterRequest,
};

pub(crate) async fn register_handler(
    State(state): State<AppState>,
    Json(payload): Json<RegisterRequest>,
) -> Response {
    let username = payload.username.trim().to_string();
    let password = payload.password.trim().to_string();
    let settings = current_system_settings(&state).await;

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

    if !settings.open_registration {
        return (
            StatusCode::FORBIDDEN,
            Json(app_response(
                403,
                "当前已关闭注册",
                json!({
                    "open_registration": false
                }),
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
        let result = sqlx::query(
            "INSERT INTO app_pending_users (username, password_hash, created_at, requested_role) VALUES ($1, $2, $3, $4)",
        )
        .bind(&username)
        .bind(&password_hash)
        .bind(now as i64)
        .bind(requested_role_label(&payload.requested_role))
        .execute(&state.db)
        .await;

        return match result {
            Ok(_) => pending_approval_response(&payload.requested_role, None),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(app_response(
                    500,
                    format!("db error: {}", err),
                    serde_json::Value::Null,
                )),
            )
                .into_response(),
        };
    }

    if !settings.invite_bypass_enabled {
        let result = sqlx::query(
            "INSERT INTO app_pending_users (username, password_hash, created_at, requested_role) VALUES ($1, $2, $3, $4)",
        )
        .bind(&username)
        .bind(&password_hash)
        .bind(now as i64)
        .bind(requested_role_label(&payload.requested_role))
        .execute(&state.db)
        .await;

        return match result {
            Ok(_) => pending_approval_response(
                &payload.requested_role,
                Some("邀请码免审核当前已关闭，已进入审批队列"),
            ),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(app_response(
                    500,
                    format!("db error: {}", err),
                    serde_json::Value::Null,
                )),
            )
                .into_response(),
        };
    }

    let invite_row =
        match sqlx::query("SELECT used, expires_at FROM app_invitations WHERE code = $1 LIMIT 1")
            .bind(invitation_code)
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

    let invite_valid = match invite_row {
        Some(ref row) => {
            let used: bool = row.try_get("used").unwrap_or(true);
            let expires_at: i64 = row.try_get("expires_at").unwrap_or(0);
            !used && expires_at > now as i64
        }
        None => false,
    };

    if !invite_valid {
        let result = sqlx::query(
            "INSERT INTO app_pending_users (username, password_hash, created_at, requested_role) VALUES ($1, $2, $3, $4)",
        )
        .bind(&username)
        .bind(&password_hash)
        .bind(now as i64)
        .bind(requested_role_label(&payload.requested_role))
        .execute(&state.db)
        .await;

        return match result {
            Ok(_) => pending_approval_response(
                &payload.requested_role,
                Some("邀请码无效或已过期，已进入审批队列"),
            ),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(app_response(
                    500,
                    format!("db error: {}", err),
                    serde_json::Value::Null,
                )),
            )
                .into_response(),
        };
    }

    let _ = sqlx::query("UPDATE app_invitations SET used = TRUE WHERE code = $1")
        .bind(invitation_code)
        .execute(&state.db)
        .await;

    if let Err(err) = sqlx::query(
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
            Json(app_response(
                500,
                format!("db error: {}", err),
                serde_json::Value::Null,
            )),
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
