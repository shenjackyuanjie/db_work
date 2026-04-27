use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use sqlx::Row;

use crate::system_settings::SystemSettings;

pub(super) fn system_settings_payload(settings: &SystemSettings) -> serde_json::Value {
    json!({
        "open_registration": settings.open_registration,
        "invite_bypass_enabled": settings.invite_bypass_enabled,
        "maintenance_mode": settings.maintenance_mode,
        "default_invite_ttl_seconds": settings.default_invite_ttl_seconds,
        "confidence_threshold": settings.confidence_threshold,
        "log_retention_days": settings.log_retention_days,
        "updated_at": settings.updated_at,
        "updated_by": settings.updated_by,
    })
}

pub(super) fn load_settings_or_error(err: anyhow::Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": format!("Failed to load settings: {}", err) })),
    )
        .into_response()
}

pub(super) fn count_from_row(row: Option<sqlx::postgres::PgRow>, field: &str) -> i64 {
    row.and_then(|item| item.try_get::<i64, _>(field).ok())
        .unwrap_or(0)
}
