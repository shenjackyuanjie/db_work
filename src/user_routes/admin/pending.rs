use axum::{
    Json,
    extract::{Json as AxumJson, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;
use sqlx::Row;
use tracing::info;

use crate::{server::AppState, system_settings::append_audit_log};

use super::super::{
    ApprovePendingUserRequest, PendingPublicUser, RejectPendingUserRequest, ensure_admin,
    parse_requested_role,
};

pub(crate) async fn list_pending_users_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }

    match sqlx::query(
        "SELECT username, created_at, requested_role FROM app_pending_users ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => {
            let pending_users = rows
                .into_iter()
                .map(|row| PendingPublicUser {
                    username: row.try_get("username").unwrap_or_default(),
                    created_at: row.try_get::<i64, _>("created_at").unwrap_or(0).max(0) as u64,
                    requested_role: parse_requested_role(
                        &row.try_get::<String, _>("requested_role")
                            .unwrap_or_else(|_| "user".to_string()),
                    ),
                })
                .collect::<Vec<_>>();
            (StatusCode::OK, Json(json!({ "pending_users": pending_users }))).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Failed to list pending users"})),
        )
            .into_response(),
    }
}

pub(crate) async fn approve_pending_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<ApprovePendingUserRequest>,
) -> Response {
    let admin_username = match ensure_admin(&state, &headers).await {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    info!(
        "管理员请求通过审核: admin={}, username={}",
        admin_username, payload.username
    );

    let pending = match sqlx::query(
        "SELECT username, password_hash, created_at, requested_role FROM app_pending_users WHERE username = $1 LIMIT 1",
    )
    .bind(&payload.username)
    .fetch_optional(&state.db)
    .await
    {
        Ok(row) => row,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"Failed to query pending user"})),
            )
                .into_response();
        }
    };

    let Some(row) = pending else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Pending user not found" })),
        )
            .into_response();
    };

    let username: String = row.try_get("username").unwrap_or_default();
    let password_hash: String = row.try_get("password_hash").unwrap_or_default();
    let created_at: i64 = row.try_get("created_at").unwrap_or(0);
    let requested_role: String = row
        .try_get("requested_role")
        .unwrap_or_else(|_| "user".to_string());

    let user_exists = sqlx::query("SELECT 1 FROM app_users WHERE username = $1 LIMIT 1")
        .bind(&username)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .is_some();
    if user_exists {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "Username already exists" })),
        )
            .into_response();
    }

    let insert_user = sqlx::query(
        "INSERT INTO app_users (username, password_hash, is_admin, created_at, session_token) VALUES ($1, $2, $3, $4, NULL)",
    )
    .bind(&username)
    .bind(&password_hash)
    .bind(requested_role.eq_ignore_ascii_case("admin"))
    .bind(created_at)
    .execute(&state.db)
    .await;

    if insert_user.is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Failed to approve pending user"})),
        )
            .into_response();
    }

    let _ = sqlx::query("DELETE FROM app_pending_users WHERE username = $1")
        .bind(&payload.username)
        .execute(&state.db)
        .await;

    let _ = append_audit_log(
        &state.db,
        "action",
        Some(&admin_username),
        &format!(
            "管理员 {} 通过了用户 {} 的注册申请",
            admin_username, payload.username
        ),
    )
    .await;

    (StatusCode::OK, Json(json!({ "status": "approved" }))).into_response()
}

pub(crate) async fn reject_pending_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<RejectPendingUserRequest>,
) -> Response {
    let admin_username = match ensure_admin(&state, &headers).await {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    info!(
        "管理员请求拒绝审核: admin={}, username={}",
        admin_username, payload.username
    );

    match sqlx::query("DELETE FROM app_pending_users WHERE username = $1")
        .bind(&payload.username)
        .execute(&state.db)
        .await
    {
        Ok(result) if result.rows_affected() > 0 => {
            let _ = append_audit_log(
                &state.db,
                "action",
                Some(&admin_username),
                &format!(
                    "管理员 {} 拒绝了用户 {} 的注册申请",
                    admin_username, payload.username
                ),
            )
            .await;
            (StatusCode::OK, Json(json!({ "status": "rejected" }))).into_response()
        }
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Pending user not found" })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Failed to reject pending user"})),
        )
            .into_response(),
    }
}