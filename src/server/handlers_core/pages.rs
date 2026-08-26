use crate::system_settings::load_system_settings;
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
};
use serde_json::json;

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

pub(crate) async fn index_page_handler() -> Response {
    read_static_page("index.html", "首页").await
}

pub(crate) async fn orchard_3d_page_handler() -> Response {
    read_static_page("orchard-3d.html", "3D 沙盘").await
}

pub(crate) async fn store_page_handler() -> Response {
    read_static_page("store.html", "普通购买").await
}

pub(crate) async fn cart_page_handler() -> Response {
    read_static_page("cart.html", "购物车").await
}

async fn read_static_page(filename: &str, page_name: &str) -> Response {
    match tokio::fs::read_to_string(format!("static/{filename}")).await {
        Ok(content) => Html(content).into_response(),
        Err(err) => {
            tracing::error!(page = page_name, %err, "读取静态页面失败");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to load page").into_response()
        }
    }
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
        tracing::warn!("非管理员访问 /admin，重定向到 /");
        return Redirect::temporary("/").into_response();
    }

    read_static_page("admin.html", "管理后台").await
}

pub(crate) async fn analyze_page_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !has_valid_session(&state, &headers).await {
        tracing::warn!("未登录或会话无效访问 /analyze，重定向到 /");
        return Redirect::temporary("/").into_response();
    }

    let settings = match load_system_settings(&state.db).await {
        Ok(settings) => settings,
        Err(err) => {
            tracing::error!("读取系统设置失败: {}", err);
            return Redirect::temporary("/").into_response();
        }
    };

    if settings.maintenance_mode && !has_admin_session(&state, &headers).await {
        tracing::warn!("维护模式开启，普通用户访问 /analyze 被拒绝");
        return Redirect::temporary("/").into_response();
    }

    read_static_page("analyze.html", "病害识别").await
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

    api_response(
        StatusCode::UNAUTHORIZED,
        401,
        "authentication required",
        serde_json::Value::Null,
    )
    .into_response()
}
