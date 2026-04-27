use crate::system_settings::load_system_settings;
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
};
use serde_json::json;
use sqlx::Row;

use super::super::{AppState, api_response, api_success, username_by_token};

pub(crate) async fn health_handler() -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "service": "openrouter"
        })),
    )
        .into_response()
}

async fn has_valid_session(state: &AppState, headers: &HeaderMap) -> bool {
    let token = match crate::user_routes::extract_auth_token(headers) {
        Some(token) => token,
        None => return false,
    };

    username_by_token(state, &token).await.is_some()
}

async fn has_admin_session(state: &AppState, headers: &HeaderMap) -> bool {
    crate::user_routes::ensure_admin(state, headers)
        .await
        .is_ok()
}

pub(crate) async fn admin_page_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !has_admin_session(&state, &headers).await {
        tracing::warn!("非管理员访问 /admin.html，重定向到 /index.html");
        return Redirect::temporary("/index.html").into_response();
    }

    match tokio::fs::read_to_string("static/admin.html").await {
        Ok(content) => Html(content).into_response(),
        Err(err) => {
            tracing::error!("读取 admin 页面失败: {}", err);
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to load page").into_response()
        }
    }
}

pub(crate) async fn analyze_page_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !has_valid_session(&state, &headers).await {
        tracing::warn!("未登录或会话无效访问 /analyze.html，重定向到 /index.html");
        return Redirect::temporary("/index.html").into_response();
    }

    let settings = match load_system_settings(&state.db).await {
        Ok(settings) => settings,
        Err(err) => {
            tracing::error!("读取系统设置失败: {}", err);
            return Redirect::temporary("/index.html").into_response();
        }
    };

    if settings.maintenance_mode && !has_admin_session(&state, &headers).await {
        tracing::warn!("维护模式开启，普通用户访问 /analyze.html 被拒绝");
        return Redirect::temporary("/index.html").into_response();
    }

    match tokio::fs::read_to_string("static/analyze.html").await {
        Ok(content) => Html(content).into_response(),
        Err(err) => {
            tracing::error!("读取 analyze 页面失败: {}", err);
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to load page").into_response()
        }
    }
}

pub(crate) async fn system_status_api_handler(State(state): State<AppState>) -> Response {
    match load_system_settings(&state.db).await {
        Ok(settings) => api_success(json!({
            "open_registration": settings.open_registration,
            "invite_bypass_enabled": settings.invite_bypass_enabled,
            "maintenance_mode": settings.maintenance_mode,
            "default_invite_ttl_seconds": settings.default_invite_ttl_seconds,
            "confidence_threshold": settings.confidence_threshold,
            "log_retention_days": settings.log_retention_days,
        })),
        Err(err) => api_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            500,
            format!("failed to load system status: {}", err),
            serde_json::Value::Null,
        )
        .into_response(),
    }
}

pub(crate) async fn api_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if has_valid_session(&state, &headers).await {
        return crate::user_routes::me_handler(State(state), headers)
            .await
            .into_response();
    }

    let row = sqlx::query(
        "SELECT username, created_at, latitude, longitude FROM app_users ORDER BY created_at ASC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(user)) => api_success(json!({
            "id": user.try_get::<String, _>("username").unwrap_or_default(),
            "username": user.try_get::<String, _>("username").unwrap_or_default(),
            "email": null,
            "orchard_address": null,
            "latitude": user.try_get::<Option<f64>, _>("latitude").unwrap_or(None),
            "longitude": user.try_get::<Option<f64>, _>("longitude").unwrap_or(None),
            "created_at": user.try_get::<i64, _>("created_at").unwrap_or(0)
        })),
        Ok(None) => api_response(
            StatusCode::NOT_FOUND,
            404,
            "User not found",
            serde_json::Value::Null,
        )
        .into_response(),
        Err(err) => api_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            500,
            format!("db error: {}", err),
            serde_json::Value::Null,
        )
        .into_response(),
    }
}
