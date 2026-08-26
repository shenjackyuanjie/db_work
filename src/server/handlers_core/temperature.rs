use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{NaiveDateTime, TimeZone, Utc};
use serde_json::json;
use sqlx::Row;

use super::super::{
    AppState, TagTemperatureHumidityRequest, TemperatureHumiditySample, UsernameQuery,
    api_response, api_success, build_temp_humidity_payload, default_temperature_samples,
    now_millis,
};

pub(crate) async fn temperature_humidity_api_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(_legacy_query): Query<UsernameQuery>,
) -> Response {
    let (_, username) = match crate::user_routes::ensure_authenticated(&state, &headers).await {
        Ok(value) => value,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    if !crate::user_routes::username_matches_session(_legacy_query.username.as_deref(), &username) {
        return api_response(
            StatusCode::FORBIDDEN,
            403,
            "username does not match the authenticated session",
            serde_json::Value::Null,
        )
        .into_response();
    }
    let rows = sqlx::query(
        "SELECT timestamp, temperature, humidity FROM app_temperature_humidity WHERE username = $1 ORDER BY timestamp DESC LIMIT 10",
    )
    .bind(username)
    .fetch_all(&state.db)
    .await;

    let mut samples = match rows {
        Ok(rows) => rows
            .into_iter()
            .rev()
            .map(|item| TemperatureHumiditySample {
                timestamp: item.try_get::<i64, _>("timestamp").unwrap_or(0).max(0) as u64,
                temperature: item.try_get::<f64, _>("temperature").unwrap_or(0.0),
                humidity: item.try_get::<f64, _>("humidity").unwrap_or(0.0),
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            return api_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                500,
                format!("db error: {}", err),
                serde_json::Value::Null,
            )
            .into_response();
        }
    };

    if samples.is_empty() {
        samples = default_temperature_samples();
    }

    api_success(build_temp_humidity_payload(&samples))
}

pub(crate) async fn post_temperature_humidity_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<TagTemperatureHumidityRequest>,
) -> Response {
    let (_, username) = match crate::user_routes::ensure_authenticated(&state, &headers).await {
        Ok(value) => value,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    if !crate::user_routes::username_matches_session(Some(&request.username), &username) {
        return api_response(
            StatusCode::FORBIDDEN,
            403,
            "username does not match the authenticated session",
            serde_json::Value::Null,
        )
        .into_response();
    }
    let sampled_at = NaiveDateTime::parse_from_str(&request.record_time, "%Y-%m-%dT%H:%M:%S%.3f")
        .map(|datetime| datetime.and_utc().timestamp_millis())
        .unwrap_or_else(|_| now_millis() as i64);

    let inserted_id = match sqlx::query_scalar::<_, i64>(
        "INSERT INTO app_tree_sensor_records (tag_serial_number, sampled_at, temperature, humidity, source) \
         VALUES ($1, $2, $3, $4, 'sensor') RETURNING id",
    )
    .bind(request.tag_serial_number)
    .bind(sampled_at)
    .bind(request.temperature)
    .bind(request.humidity)
    .fetch_one(&state.db)
    .await
    {
        Ok(id) => id,
        Err(err) => {
            return api_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                500,
                format!("db error: {}", err),
                serde_json::Value::Null,
            )
            .into_response();
        }
    };

    if let Err(err) = sqlx::query(
        "INSERT INTO app_temperature_humidity (username, timestamp, temperature, humidity) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(&username)
    .bind(sampled_at)
    .bind(request.temperature)
    .bind(request.humidity)
    .execute(&state.db)
    .await
    {
        return api_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            500,
            format!("db error: {}", err),
            serde_json::Value::Null,
        )
        .into_response();
    }

    let rows = sqlx::query(
        "SELECT tag_serial_number, sampled_at, temperature, humidity \
         FROM app_tree_sensor_records \
         WHERE tag_serial_number = $1 AND id != $2 \
         ORDER BY sampled_at DESC LIMIT 9",
    )
    .bind(request.tag_serial_number)
    .bind(inserted_id)
    .fetch_all(&state.db)
    .await;

    let recent_records = match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|row| {
                let millis = row.try_get::<i64, _>("sampled_at").unwrap_or(0);
                let record_time = Utc
                    .timestamp_millis_opt(millis)
                    .single()
                    .map(|datetime| datetime.format("%Y-%m-%dT%H:%M:%S%.3f").to_string())
                    .unwrap_or_default();

                json!({
                    "temperature": row.try_get::<f64, _>("temperature").unwrap_or(0.0),
                    "humidity": row.try_get::<f64, _>("humidity").unwrap_or(0.0),
                    "tag_serial_number": row.try_get::<i64, _>("tag_serial_number").unwrap_or(0),
                    "record_time": record_time,
                })
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            return api_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                500,
                format!("db error: {}", err),
                serde_json::Value::Null,
            )
            .into_response();
        }
    };

    api_success(json!({
        "recentRecords": recent_records,
    }))
}
