use axum::{
    Json,
    extract::{Json as AxumJson, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::{
    server::AppState,
    system_settings::{
        SystemSettings, append_audit_log, load_system_settings, prune_audit_logs,
        update_system_settings,
    },
};

use super::super::{UpdateSystemSettingsRequest, ensure_admin};
use super::common::{load_settings_or_error, system_settings_payload};

pub(crate) async fn get_system_settings_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }

    match load_system_settings(&state.db).await {
        Ok(settings) => (
            StatusCode::OK,
            Json(json!({
                "settings": system_settings_payload(&settings)
            })),
        )
            .into_response(),
        Err(err) => load_settings_or_error(err),
    }
}

pub(crate) async fn update_system_settings_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<UpdateSystemSettingsRequest>,
) -> Response {
    let admin_username = match ensure_admin(&state, &headers).await {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    let settings = SystemSettings {
        open_registration: payload.open_registration,
        invite_bypass_enabled: payload.invite_bypass_enabled,
        maintenance_mode: payload.maintenance_mode,
        default_invite_ttl_seconds: payload.default_invite_ttl_seconds,
        confidence_threshold: payload.confidence_threshold,
        log_retention_days: payload.log_retention_days,
        updated_at: 0,
        updated_by: Some(admin_username.clone()),
    };

    let updated = match update_system_settings(&state.db, settings, Some(&admin_username)).await {
        Ok(settings) => settings,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to update settings: {}", err) })),
            )
                .into_response();
        }
    };

    let _ = append_audit_log(
        &state.db,
        "action",
        Some(&admin_username),
        &format!(
            "管理员 {} 更新系统设置：开放注册={}，邀请码免审={}，维护模式={}，默认邀请码TTL={}秒，置信度阈值={:.0}%，日志保留={}天",
            admin_username,
            updated.open_registration,
            updated.invite_bypass_enabled,
            updated.maintenance_mode,
            updated.default_invite_ttl_seconds,
            updated.confidence_threshold_percent(),
            updated.log_retention_days,
        ),
    )
    .await;

    let _ = prune_audit_logs(&state.db, updated.log_retention_days).await;

    (
        StatusCode::OK,
        Json(json!({
            "settings": system_settings_payload(&updated)
        })),
    )
        .into_response()
}
