use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use sqlx::Row;

use crate::server::AppState;

use super::super::{
    OrchardOverviewRequest, ensure_admin, ensure_authenticated,
};

#[derive(Debug)]
enum WeatherLookupError {
    UserNotFound,
    Database(sqlx::Error),
    Fetch(String),
}

async fn fetch_weather_by_coordinates(latitude: f64, longitude: f64) -> Result<Value, String> {
    Ok(json!({
        "source": "static",
        "latitude": latitude,
        "longitude": longitude,
        "timezone": "Asia/Shanghai",
        "current": {
            "time": "2025-01-01T12:00",
            "temperature": 22.5,
            "temperature_unit": "°C",
            "humidity": 65.0,
            "humidity_unit": "%",
            "wind_speed": 3.2,
            "wind_speed_unit": "km/h",
            "weather_code": 0,
            "weather_text": "晴",
        }
    }))
}


async fn fetch_weather_for_user(state: &AppState, username: &str) -> Result<Option<Value>, WeatherLookupError> {
    let row = sqlx::query(
        "SELECT latitude, longitude FROM app_users WHERE username = $1 LIMIT 1",
    )
    .bind(username)
    .fetch_optional(&state.db)
    .await
    .map_err(WeatherLookupError::Database)?;

    let Some(row) = row else {
        return Err(WeatherLookupError::UserNotFound);
    };

    let latitude = row.try_get::<Option<f64>, _>("latitude").unwrap_or(None);
    let longitude = row.try_get::<Option<f64>, _>("longitude").unwrap_or(None);

    let (Some(latitude), Some(longitude)) = (latitude, longitude) else {
        return Ok(None);
    };

    fetch_weather_by_coordinates(latitude, longitude)
        .await
        .map(Some)
        .map_err(WeatherLookupError::Fetch)
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
        None => ("healthy", "健康果树", "#10b981"),
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

pub(crate) async fn orchard_overview_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<OrchardOverviewRequest>,
) -> Response {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }

    let username = payload.username.trim().to_string();
    if username.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "username is required" })),
        )
            .into_response();
    }

    let (weather, weather_error) = match fetch_weather_for_user(&state, &username).await {
        Ok(weather) => (weather, None),
        Err(WeatherLookupError::UserNotFound) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "User not found" })),
            )
                .into_response();
        }
        Err(WeatherLookupError::Database(err)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("Failed to load user coordinates: {}", err)
                })),
            )
                .into_response();
        }
        Err(WeatherLookupError::Fetch(err)) => (None, Some(err)),
    };

    let rows = match sqlx::query(
        r#"
        SELECT
            t.id,
            t.tree_code,
            t.tag_serial_number,
            t.pos_x,
            t.pos_y,
            t.terrain_height,

            latest_sensor.sampled_at,
            latest_sensor.temperature,
            latest_sensor.humidity,

            latest_diagnosis.predicted_class,
            latest_diagnosis.disease_name,
            latest_diagnosis.diagnosis_timestamp,
            latest_diagnosis.confidence AS diagnosis_confidence
        FROM app_orchard_trees t
        LEFT JOIN LATERAL (
            SELECT
                sampled_at,
                temperature,
                humidity
            FROM app_tree_sensor_records
            WHERE tag_serial_number = t.tag_serial_number
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
    let mut last_sampled_at = 0_i64;

    for row in rows {
        let tree_id = row.try_get::<i64, _>("id").unwrap_or_default();
        let tree_code = row.try_get::<String, _>("tree_code").unwrap_or_default();
        let tag_serial_number = row
            .try_get::<Option<i64>, _>("tag_serial_number")
            .unwrap_or(None);
        let pos_x = row.try_get::<f64, _>("pos_x").unwrap_or(0.0);
        let pos_y = row.try_get::<f64, _>("pos_y").unwrap_or(0.0);
        let terrain_height = row.try_get::<f64, _>("terrain_height").unwrap_or(0.0);

        let sampled_at = row.try_get::<Option<i64>, _>("sampled_at").unwrap_or(None);

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
            orchard_status_from_snapshot(None, diagnosis_label);

        push_orchard_legend(&mut legend, status_label, status_level, status_color);

        if let Some(sampled_at) = sampled_at {
            online_trees += 1;
            last_sampled_at = last_sampled_at.max(sampled_at);
        }

        let latest_sensor = sampled_at.map(|sampled_at| {
            json!({
                "sampled_at": sampled_at,
                "temperature": row.try_get::<f64, _>("temperature").unwrap_or(0.0),
                "humidity": row.try_get::<f64, _>("humidity").unwrap_or(0.0),
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
            "tag_serial_number": tag_serial_number,
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

    (
        StatusCode::OK,
        Json(json!({
            "requested_username": username,
            "coordinate_range": {
                "min_x": 0,
                "max_x": 500,
                "min_y": 0,
                "max_y": 500,
            },
            "summary": {
                "total_trees": trees.len(),
                "online_trees": online_trees,
                "last_sampled_at": if last_sampled_at > 0 { Some(last_sampled_at) } else { None::<i64> },
            },
            "legend": legend.into_iter().map(|(label, level, color, count)| json!({
                "label": label,
                "level": level,
                "color": color,
                "count": count,
            })).collect::<Vec<_>>(),
            "weather": weather,
            "weather_error": weather_error,
            "trees": trees,
        })),
    )
        .into_response()
}

/// 普通用户（无需管理员权限）的园区总览接口
pub(crate) async fn orchard_overview_public_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let (_, username) = match ensure_authenticated(&state, &headers).await {
        Ok(value) => value,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    tracing::info!("普通用户 {} 请求园区3D沙盘数据", username);

    let (weather, weather_error) = match fetch_weather_for_user(&state, &username).await {
        Ok(weather) => (weather, None),
        Err(WeatherLookupError::UserNotFound) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "User not found" })),
            )
                .into_response();
        }
        Err(WeatherLookupError::Database(err)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("Failed to load user coordinates: {}", err)
                })),
            )
                .into_response();
        }
        Err(WeatherLookupError::Fetch(err)) => (None, Some(err)),
    };

    let rows = match sqlx::query(
        r#"
        SELECT
            t.id,
            t.tree_code,
            t.tag_serial_number,
            t.pos_x,
            t.pos_y,
            t.terrain_height,

            latest_sensor.sampled_at,
            latest_sensor.temperature,
            latest_sensor.humidity,

            latest_diagnosis.predicted_class,
            latest_diagnosis.disease_name,
            latest_diagnosis.diagnosis_timestamp,
            latest_diagnosis.confidence AS diagnosis_confidence
        FROM app_orchard_trees t
        LEFT JOIN LATERAL (
            SELECT
                sampled_at,
                temperature,
                humidity
            FROM app_tree_sensor_records
            WHERE tag_serial_number = t.tag_serial_number
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
    let mut last_sampled_at = 0_i64;

    for row in rows {
        let tree_id = row.try_get::<i64, _>("id").unwrap_or_default();
        let tree_code = row.try_get::<String, _>("tree_code").unwrap_or_default();
        let tag_serial_number = row
            .try_get::<Option<i64>, _>("tag_serial_number")
            .unwrap_or(None);
        let pos_x = row.try_get::<f64, _>("pos_x").unwrap_or(0.0);
        let pos_y = row.try_get::<f64, _>("pos_y").unwrap_or(0.0);
        let terrain_height = row.try_get::<f64, _>("terrain_height").unwrap_or(0.0);

        let sampled_at = row.try_get::<Option<i64>, _>("sampled_at").unwrap_or(None);

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
            orchard_status_from_snapshot(None, diagnosis_label);

        push_orchard_legend(&mut legend, status_label, status_level, status_color);

        if let Some(sampled_at) = sampled_at {
            online_trees += 1;
            last_sampled_at = last_sampled_at.max(sampled_at);
        }

        let latest_sensor = sampled_at.map(|sampled_at| {
            json!({
                "sampled_at": sampled_at,
                "temperature": row.try_get::<f64, _>("temperature").unwrap_or(0.0),
                "humidity": row.try_get::<f64, _>("humidity").unwrap_or(0.0),
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
            "tag_serial_number": tag_serial_number,
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

    (
        StatusCode::OK,
        Json(json!({
            "requested_username": username,
            "coordinate_range": {
                "min_x": 0,
                "max_x": 500,
                "min_y": 0,
                "max_y": 500,
            },
            "summary": {
                "total_trees": trees.len(),
                "online_trees": online_trees,
                "last_sampled_at": if last_sampled_at > 0 { Some(last_sampled_at) } else { None::<i64> },
            },
            "legend": legend.into_iter().map(|(label, level, color, count)| json!({
                "label": label,
                "level": level,
                "color": color,
                "count": count,
            })).collect::<Vec<_>>(),
            "weather": weather,
            "weather_error": weather_error,
            "trees": trees,
        })),
    )
        .into_response()
}
