use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{Duration, TimeZone, Utc};
use serde_json::json;
use sqlx::Row;

use crate::{server::AppState, system_settings::load_system_settings};

use super::super::ensure_admin;
use super::common::{count_from_row, load_settings_or_error};

pub(crate) async fn dashboard_stats_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }

    let total_detections = count_from_row(
        sqlx::query("SELECT COUNT(*) AS count FROM app_diagnosis_records")
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten(),
        "count",
    );
    let healthy_count = count_from_row(
        sqlx::query("SELECT COUNT(*) AS count FROM app_diagnosis_records WHERE is_healthy = TRUE")
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten(),
        "count",
    );
    let diseased_count = count_from_row(
        sqlx::query(
            "SELECT COUNT(*) AS count FROM app_diagnosis_records WHERE is_citrus_leaf = TRUE AND is_healthy = FALSE AND predicted_class <> '非果树'",
        )
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten(),
        "count",
    );
    let user_count = count_from_row(
        sqlx::query("SELECT COUNT(*) AS count FROM app_users")
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten(),
        "count",
    );
    let admin_count = count_from_row(
        sqlx::query("SELECT COUNT(*) AS count FROM app_users WHERE is_admin = TRUE")
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten(),
        "count",
    );
    let pending_count = count_from_row(
        sqlx::query("SELECT COUNT(*) AS count FROM app_pending_users")
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten(),
        "count",
    );

    let detection_rows = sqlx::query(
        "SELECT timestamp FROM app_diagnosis_records ORDER BY timestamp DESC LIMIT 500",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let today = Utc::now().date_naive();
    let mut series = (0..7)
        .map(|offset| {
            let date = today - Duration::days((6 - offset) as i64);
            (date.format("%Y-%m-%d").to_string(), 0_i64)
        })
        .collect::<Vec<_>>();

    for row in detection_rows {
        let timestamp = row.try_get::<i64, _>("timestamp").unwrap_or(0);
        if let Some(datetime) = Utc.timestamp_millis_opt(timestamp).single() {
            let diff_days = (today - datetime.date_naive()).num_days();
            if (0..7).contains(&diff_days) {
                let index = 6 - diff_days as usize;
                if let Some((_, count)) = series.get_mut(index) {
                    *count += 1;
                }
            }
        }
    }

    let latest_env = sqlx::query(
        "SELECT temperature, humidity FROM app_temperature_humidity ORDER BY timestamp DESC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let current_temperature = latest_env
        .as_ref()
        .and_then(|row| row.try_get::<f64, _>("temperature").ok());
    let current_humidity = latest_env
        .as_ref()
        .and_then(|row| row.try_get::<f64, _>("humidity").ok());
    let healthy_rate = if total_detections > 0 {
        ((healthy_count as f64 / total_detections as f64) * 100.0).round() as i64
    } else {
        100
    };

    (
        StatusCode::OK,
        Json(json!({
            "totals": {
                "detections": total_detections,
                "healthy_count": healthy_count,
                "diseased_count": diseased_count,
                "users": user_count,
                "admins": admin_count,
                "pending_users": pending_count,
                "healthy_rate": healthy_rate
            },
            "daily_counts": series.into_iter().map(|(date, count)| json!({
                "date": date,
                "count": count
            })).collect::<Vec<_>>(),
            "environment": {
                "current_temperature": current_temperature,
                "current_humidity": current_humidity,
                "health_score": healthy_rate,
                "active_alerts": diseased_count + pending_count
            }
        })),
    )
        .into_response()
}

pub(crate) async fn dashboard_logs_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }

    let settings = match load_system_settings(&state.db).await {
        Ok(settings) => settings,
        Err(err) => return load_settings_or_error(err),
    };
    let cutoff = Utc::now() - Duration::days(settings.log_retention_days as i64);
    let cutoff_millis = cutoff.timestamp_millis();

    let audit_rows = sqlx::query(
        "SELECT log_type, actor_username, message, created_at FROM app_admin_audit_logs WHERE created_at >= $1 ORDER BY created_at DESC LIMIT 120",
    )
    .bind(cutoff_millis)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let diagnosis_rows = sqlx::query(
        "SELECT username, predicted_class, confidence, timestamp, is_healthy, is_citrus_leaf FROM app_diagnosis_records WHERE timestamp >= $1 ORDER BY timestamp DESC LIMIT 120",
    )
    .bind(cutoff_millis)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let pending_rows = sqlx::query(
        "SELECT username, requested_role, created_at FROM app_pending_users WHERE created_at >= $1 ORDER BY created_at DESC LIMIT 60",
    )
    .bind(cutoff_millis / 1000)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let user_rows = sqlx::query(
        "SELECT username, is_admin, created_at FROM app_users WHERE created_at >= $1 ORDER BY created_at DESC LIMIT 60",
    )
    .bind(cutoff_millis / 1000)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut logs = Vec::new();

    for row in audit_rows {
        logs.push(json!({
            "type": row.try_get::<String, _>("log_type").unwrap_or_else(|_| "action".to_string()),
            "message": row.try_get::<String, _>("message").unwrap_or_default(),
            "created_at": row.try_get::<i64, _>("created_at").unwrap_or(0),
            "actor": row.try_get::<Option<String>, _>("actor_username").unwrap_or(None),
        }));
    }

    for row in diagnosis_rows {
        let predicted_class = row
            .try_get::<String, _>("predicted_class")
            .unwrap_or_default();
        let confidence = row.try_get::<f64, _>("confidence").unwrap_or(0.0);
        let username = row
            .try_get::<Option<String>, _>("username")
            .unwrap_or(None)
            .unwrap_or_else(|| "匿名用户".to_string());
        let is_healthy = row.try_get::<bool, _>("is_healthy").unwrap_or(false);
        let is_citrus_leaf = row.try_get::<bool, _>("is_citrus_leaf").unwrap_or(false);
        let log_type = if is_citrus_leaf && !is_healthy && predicted_class != "非果树" {
            "warning"
        } else {
            "detect"
        };
        logs.push(json!({
            "type": log_type,
            "message": format!("{} 完成病害识别，结果：{}（{:.1}%）", username, predicted_class, confidence),
            "created_at": row.try_get::<i64, _>("timestamp").unwrap_or(0),
            "actor": username,
        }));
    }

    for row in pending_rows {
        let username = row.try_get::<String, _>("username").unwrap_or_default();
        let requested_role = row
            .try_get::<String, _>("requested_role")
            .unwrap_or_else(|_| "user".to_string());
        let role_text = if requested_role.eq_ignore_ascii_case("admin") {
            "管理员"
        } else {
            "用户"
        };
        logs.push(json!({
            "type": "warning",
            "message": format!("{} 提交了{}注册申请，等待审核", username, role_text),
            "created_at": row.try_get::<i64, _>("created_at").unwrap_or(0) * 1000,
            "actor": username,
        }));
    }

    for row in user_rows {
        let username = row.try_get::<String, _>("username").unwrap_or_default();
        let is_admin = row.try_get::<bool, _>("is_admin").unwrap_or(false);
        logs.push(json!({
            "type": "action",
            "message": format!("用户 {} 注册成功{}", username, if is_admin { "（管理员）" } else { "" }),
            "created_at": row.try_get::<i64, _>("created_at").unwrap_or(0) * 1000,
            "actor": username,
        }));
    }

    logs.sort_by(|left, right| {
        let right_ts = right
            .get("created_at")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        let left_ts = left
            .get("created_at")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        right_ts.cmp(&left_ts)
    });

    (
        StatusCode::OK,
        Json(json!({
            "logs": logs.into_iter().take(150).collect::<Vec<_>>()
        })),
    )
        .into_response()
}
