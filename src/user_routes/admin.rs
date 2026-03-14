use axum::{
    Json,
    extract::{Json as AxumJson, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde_json::json;
use sqlx::Row;
use tracing::info;
use uuid::Uuid;

use crate::server::AppState;

use super::*;

pub async fn set_admin_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<super::SetAdminRequest>,
) -> impl IntoResponse {
    let admin_username = match ensure_admin(&state, &headers).await {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    info!(
        "管理员请求修改用户权限: admin={}, target_username={}, make_admin= {}",
        admin_username, payload.target_username, payload.make_admin
    );

    match sqlx::query("UPDATE app_users SET is_admin = $1 WHERE username = $2")
        .bind(payload.make_admin)
        .bind(&payload.target_username)
        .execute(&state.db)
        .await
    {
        Ok(result) if result.rows_affected() > 0 => {
            (StatusCode::OK, Json(json!({ "status": "updated" }))).into_response()
        }
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Target user not found" })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Failed to update user" })),
        )
            .into_response(),
    }
}

pub async fn create_invitation_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<super::CreateInvitationRequest>,
) -> impl IntoResponse {
    let admin_username = match ensure_admin(&state, &headers).await {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    let ttl = payload.ttl_seconds.unwrap_or(24 * 60 * 60);
    let code = Uuid::new_v4().to_string();
    let expires_at = now_secs() + ttl;

    info!(
        "管理员请求创建邀请码: admin={}, ttl_seconds={}",
        admin_username, ttl
    );

    match sqlx::query(
        "INSERT INTO app_invitations (code, used, expires_at) VALUES ($1, $2, $3)",
    )
    .bind(&code)
    .bind(false)
    .bind(expires_at as i64)
    .execute(&state.db)
    .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({
                "code": code,
                "expires_at": expires_at
            })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Failed to create invitation"})),
        )
            .into_response(),
    }
}

pub async fn list_invitations_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }

    match sqlx::query("SELECT code, used, expires_at FROM app_invitations ORDER BY expires_at DESC")
        .fetch_all(&state.db)
        .await
    {
        Ok(rows) => {
            let invitations = rows
                .into_iter()
                .map(|r| {
                    json!({
                        "code": r.try_get::<String, _>("code").unwrap_or_default(),
                        "used": r.try_get::<bool, _>("used").unwrap_or(false),
                        "expires_at": r.try_get::<i64, _>("expires_at").unwrap_or(0)
                    })
                })
                .collect::<Vec<_>>();
            (StatusCode::OK, Json(json!({ "invitations": invitations }))).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Failed to list invitations"})),
        )
            .into_response(),
    }
}

pub async fn list_users_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }

    match sqlx::query("SELECT username, is_admin, created_at FROM app_users ORDER BY created_at DESC")
        .fetch_all(&state.db)
        .await
    {
        Ok(rows) => {
            let users = rows
                .into_iter()
                .map(|r| PublicUser {
                    username: r.try_get("username").unwrap_or_default(),
                    is_admin: r.try_get("is_admin").unwrap_or(false),
                    created_at: r.try_get::<i64, _>("created_at").unwrap_or(0).max(0) as u64,
                })
                .collect::<Vec<_>>();
            (StatusCode::OK, Json(json!({ "users": users }))).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Failed to list users"})),
        )
            .into_response(),
    }
}

pub async fn list_pending_users_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
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
                .map(|r| PendingPublicUser {
                    username: r.try_get("username").unwrap_or_default(),
                    created_at: r.try_get::<i64, _>("created_at").unwrap_or(0).max(0) as u64,
                    requested_role: parse_requested_role(
                        &r.try_get::<String, _>("requested_role").unwrap_or_else(|_| "user".to_string()),
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

pub async fn approve_pending_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<super::ApprovePendingUserRequest>,
) -> impl IntoResponse {
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
        Ok(v) => v,
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
    let requested_role: String = row.try_get("requested_role").unwrap_or_else(|_| "user".to_string());

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

    (StatusCode::OK, Json(json!({ "status": "approved" }))).into_response()
}

pub async fn reject_pending_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<super::RejectPendingUserRequest>,
) -> impl IntoResponse {
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
