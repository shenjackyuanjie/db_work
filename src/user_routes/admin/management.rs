use axum::{
    Json,
    extract::{Json as AxumJson, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;
use sqlx::Row;
use tracing::info;
use uuid::Uuid;

use crate::{
    server::AppState,
    system_settings::{append_audit_log, load_system_settings},
};

use super::common::load_settings_or_error;
use super::super::{CreateInvitationRequest, PublicUser, SetAdminRequest, ensure_admin, now_secs};

pub(crate) async fn set_admin_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<SetAdminRequest>,
) -> Response {
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
            let role_text = if payload.make_admin { "管理员" } else { "普通用户" };
            let _ = append_audit_log(
                &state.db,
                "action",
                Some(&admin_username),
                &format!(
                    "管理员 {} 将用户 {} 调整为{}",
                    admin_username, payload.target_username, role_text
                ),
            )
            .await;
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

pub(crate) async fn create_invitation_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<CreateInvitationRequest>,
) -> Response {
    let admin_username = match ensure_admin(&state, &headers).await {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    let settings = match load_system_settings(&state.db).await {
        Ok(settings) => settings,
        Err(err) => return load_settings_or_error(err),
    };

    let ttl = payload
        .ttl_seconds
        .unwrap_or(settings.default_invite_ttl_seconds.max(3600) as u64);
    let code = Uuid::new_v4().to_string();
    let expires_at = if ttl == 0 { i64::MAX as u64 } else { now_secs() + ttl };

    info!(
        "管理员请求创建邀请码: admin={}, ttl_seconds={}",
        admin_username, ttl
    );

    match sqlx::query("INSERT INTO app_invitations (code, used, expires_at) VALUES ($1, $2, $3)")
        .bind(&code)
        .bind(false)
        .bind(expires_at as i64)
        .execute(&state.db)
        .await
    {
        Ok(_) => {
            let _ = append_audit_log(
                &state.db,
                "action",
                Some(&admin_username),
                &format!(
                    "管理员 {} 创建邀请码 {}，有效期 {} 秒",
                    admin_username, code, ttl
                ),
            )
            .await;
            (
                StatusCode::OK,
                Json(json!({
                    "code": code,
                    "expires_at": expires_at
                })),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Failed to create invitation"})),
        )
            .into_response(),
    }
}

pub(crate) async fn list_invitations_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
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
                .map(|row| {
                    json!({
                        "code": row.try_get::<String, _>("code").unwrap_or_default(),
                        "used": row.try_get::<bool, _>("used").unwrap_or(false),
                        "expires_at": row.try_get::<i64, _>("expires_at").unwrap_or(0)
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

pub(crate) async fn list_users_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
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
                .map(|row| PublicUser {
                    username: row.try_get("username").unwrap_or_default(),
                    is_admin: row.try_get("is_admin").unwrap_or(false),
                    created_at: row.try_get::<i64, _>("created_at").unwrap_or(0).max(0) as u64,
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