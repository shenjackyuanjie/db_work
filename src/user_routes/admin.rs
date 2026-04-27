use axum::{
    Json,
    extract::{Json as AxumJson, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::{Duration, TimeZone, Utc};
use serde_json::json;
use sqlx::Row;
use tracing::info;
use uuid::Uuid;

use crate::server::AppState;
use crate::system_settings::{
    SystemSettings, append_audit_log, load_system_settings, prune_audit_logs,
    update_system_settings,
};

use super::*;

fn system_settings_payload(settings: &SystemSettings) -> serde_json::Value {
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

fn load_settings_or_error(err: anyhow::Error) -> impl IntoResponse {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": format!("Failed to load settings: {}", err) })),
    )
}

fn count_from_row(row: Option<sqlx::postgres::PgRow>, field: &str) -> i64 {
    row.and_then(|item| item.try_get::<i64, _>(field).ok())
        .unwrap_or(0)
}

fn orchard_status_from_snapshot(
    health_index: Option<f64>,
    predicted_class: Option<&str>,
) -> (&'static str, &'static str, &'static str) {
    if let Some(label) = predicted_class
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        match label {
            "黄龙病" => return ("critical", "黄龙病预警", "#ef4444"),
            "溃疡病" => return ("warning", "溃疡病预警", "#f59e0b"),
            "沙皮病" => return ("critical", "沙皮病预警", "#8b5cf6"),
            "待人工复核" => return ("attention", "待人工复核", "#38bdf8"),
            _ => {}
        }
    }

    match health_index {
        Some(value) if value >= 0.85 => ("healthy", "健康果树", "#10b981"),
        Some(value) if value >= 0.72 => ("attention", "轻度异常", "#f59e0b"),
        Some(value) if value >= 0.60 => ("warning", "中度异常", "#ef4444"),
        Some(_) => ("critical", "重度异常", "#8b5cf6"),
        None => ("offline", "暂无采样", "#64748b"),
    }
}

fn orchard_status_priority(level: &str) -> i32 {
    match level {
        "healthy" => 0,
        "attention" => 1,
        "warning" => 2,
        "critical" => 3,
        _ => 4,
    }
}

fn push_orchard_legend(
    legend: &mut Vec<(String, String, String, i64)>,
    label: &str,
    level: &str,
    color: &str,
) {
    if let Some(item) = legend.iter_mut().find(|item| item.0 == label) {
        item.3 += 1;
        return;
    }

    legend.push((label.to_string(), level.to_string(), color.to_string(), 1));
}

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
            let role_text = if payload.make_admin {
                "管理员"
            } else {
                "普通用户"
            };
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

pub async fn create_invitation_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<super::CreateInvitationRequest>,
) -> impl IntoResponse {
    let admin_username = match ensure_admin(&state, &headers).await {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    let settings = match load_system_settings(&state.db).await {
        Ok(settings) => settings,
        Err(err) => return load_settings_or_error(err).into_response(),
    };

    let ttl = payload
        .ttl_seconds
        .unwrap_or(settings.default_invite_ttl_seconds.max(3600) as u64);
    let code = Uuid::new_v4().to_string();
    let expires_at = if ttl == 0 {
        i64::MAX as u64
    } else {
        now_secs() + ttl
    };

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

    match sqlx::query(
        "SELECT username, is_admin, created_at FROM app_users ORDER BY created_at DESC",
    )
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

pub async fn get_system_settings_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
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
        Err(err) => load_settings_or_error(err).into_response(),
    }
}

pub async fn update_system_settings_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<super::UpdateSystemSettingsRequest>,
) -> impl IntoResponse {
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

pub async fn dashboard_stats_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
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
        if let Some(dt) = Utc.timestamp_millis_opt(timestamp).single() {
            let diff_days = (today - dt.date_naive()).num_days();
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

pub async fn orchard_overview_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }

    let rows = match sqlx::query(
        r#"
        SELECT
            t.id,
            t.tree_code,
            t.pos_x,
            t.pos_y,
            t.terrain_height,

            latest_sensor.sampled_at,
            latest_sensor.temperature,
            latest_sensor.humidity,
            latest_sensor.nitrogen,
            latest_sensor.phosphorus,
            latest_sensor.potassium,
            latest_sensor.health_index,
            latest_diagnosis.predicted_class,
            latest_diagnosis.disease_name,
            latest_diagnosis.diagnosis_timestamp,
            latest_diagnosis.confidence AS diagnosis_confidence
        FROM app_orchard_trees t
        LEFT JOIN LATERAL (
            SELECT
                sampled_at,
                temperature,
                humidity,
                nitrogen,
                phosphorus,
                potassium,
                health_index
            FROM app_tree_sensor_records
            WHERE tree_id = t.id
            ORDER BY sampled_at DESC
            LIMIT 1
        ) latest_sensor ON TRUE
        LEFT JOIN LATERAL (
            SELECT
                predicted_class,
                disease_name,
                timestamp AS diagnosis_timestamp,
                confidence
            FROM app_diagnosis_records
            WHERE area = t.tree_code
            ORDER BY timestamp DESC
            LIMIT 1
        ) latest_diagnosis ON TRUE
        WHERE t.is_active = TRUE
        ORDER BY t.id ASC
        "#,
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to load orchard overview: {}", err) })),
            )
                .into_response();
        }
    };

    let mut trees = Vec::new();
    let mut legend = Vec::new();
    let mut online_trees = 0_i64;
    let mut health_sum = 0.0_f64;
    let mut health_count = 0_i64;
    let mut last_sampled_at = 0_i64;

    for row in rows {
        let tree_id = row.try_get::<i64, _>("id").unwrap_or_default();
        let tree_code = row.try_get::<String, _>("tree_code").unwrap_or_default();
        let pos_x = row.try_get::<f64, _>("pos_x").unwrap_or(0.0);
        let pos_y = row.try_get::<f64, _>("pos_y").unwrap_or(0.0);
        let terrain_height = row.try_get::<f64, _>("terrain_height").unwrap_or(0.0);

        let sampled_at = row.try_get::<Option<i64>, _>("sampled_at").unwrap_or(None);
        let health_index = row
            .try_get::<Option<f64>, _>("health_index")
            .unwrap_or(None);
        let predicted_class = row
            .try_get::<Option<String>, _>("predicted_class")
            .unwrap_or(None)
            .filter(|value| !value.trim().is_empty() && value != "健康果树" && value != "非果树");
        let disease_name = row
            .try_get::<Option<String>, _>("disease_name")
            .unwrap_or(None)
            .filter(|value| !value.trim().is_empty());
        let diagnosis_label = disease_name.as_deref().or(predicted_class.as_deref());
        let (status_level, status_label, status_color) =
            orchard_status_from_snapshot(health_index, diagnosis_label);

        push_orchard_legend(&mut legend, status_label, status_level, status_color);

        if let Some(sampled_at) = sampled_at {
            online_trees += 1;
            last_sampled_at = last_sampled_at.max(sampled_at);
        }
        if let Some(health_index) = health_index {
            health_sum += health_index;
            health_count += 1;
        }

        let latest_sensor = sampled_at.map(|sampled_at| {
            json!({
                "sampled_at": sampled_at,
                "temperature": row.try_get::<f64, _>("temperature").unwrap_or(0.0),
                "humidity": row.try_get::<f64, _>("humidity").unwrap_or(0.0),
                "nitrogen": row.try_get::<f64, _>("nitrogen").unwrap_or(0.0),
                "phosphorus": row.try_get::<f64, _>("phosphorus").unwrap_or(0.0),
                "potassium": row.try_get::<f64, _>("potassium").unwrap_or(0.0),
                "health_index": health_index.unwrap_or(0.0),
            })
        });

        let diagnosis_timestamp = row
            .try_get::<Option<i64>, _>("diagnosis_timestamp")
            .unwrap_or(None);
        let latest_diagnosis = diagnosis_timestamp.map(|timestamp| {
            json!({
                "timestamp": timestamp,
                "predicted_class": predicted_class,
                "disease_name": disease_name,
                "confidence": row.try_get::<Option<f64>, _>("diagnosis_confidence").unwrap_or(None),
            })
        });

        trees.push(json!({
            "id": tree_id,
            "tree_code": tree_code,
            "position": {
                "x": pos_x,
                "y": pos_y,
            },
            "terrain_height": terrain_height,

            "status": {
                "level": status_level,
                "label": status_label,
                "color": status_color,
            },
            "latest_sensor": latest_sensor,
            "latest_diagnosis": latest_diagnosis,
        }));
    }

    legend.sort_by(|left, right| {
        orchard_status_priority(&left.1)
            .cmp(&orchard_status_priority(&right.1))
            .then(left.0.cmp(&right.0))
    });

    let average_health_index = if health_count > 0 {
        (health_sum / health_count as f64 * 100.0).round() / 100.0
    } else {
        0.0
    };

    (
        StatusCode::OK,
        Json(json!({
            "coordinate_range": {
                "min_x": 0,
                "max_x": 500,
                "min_y": 0,
                "max_y": 500,
            },
            "summary": {
                "total_trees": trees.len(),
                "online_trees": online_trees,
                "average_health_index": average_health_index,
                "last_sampled_at": if last_sampled_at > 0 { Some(last_sampled_at) } else { None::<i64> },
            },
            "legend": legend.into_iter().map(|(label, level, color, count)| json!({
                "label": label,
                "level": level,
                "color": color,
                "count": count,
            })).collect::<Vec<_>>(),
            "trees": trees,
        })),
    )
        .into_response()
}

pub async fn dashboard_logs_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }

    let settings = match load_system_settings(&state.db).await {
        Ok(settings) => settings,
        Err(err) => return load_settings_or_error(err).into_response(),
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
