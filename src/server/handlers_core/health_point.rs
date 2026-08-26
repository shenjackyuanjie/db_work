use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;
use sqlx::Row;

use super::super::{AppState, api_response, api_success, now_millis};
use crate::inference::is_climate_in_range;

pub(crate) async fn health_point_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, body)) = crate::user_routes::ensure_authenticated(&state, &headers).await {
        return (code, axum::Json(body)).into_response();
    }
    // 最近1天 = 86400秒 = 86_400_000 毫秒
    let one_day_ago = now_millis() as i64 - 86_400_000_i64;

    // 对每个 tag_serial_number 取最近1天内 sampled_at 最大的那条记录
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT ON (tag_serial_number)
            tag_serial_number,
            sampled_at,
            temperature,
            humidity
        FROM app_tree_sensor_records
        WHERE sampled_at >= $1
        ORDER BY tag_serial_number, sampled_at DESC
        "#,
    )
    .bind(one_day_ago)
    .fetch_all(&state.db)
    .await;

    let rows = match rows {
        Ok(r) => r,
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

    let total = rows.len();

    if total == 0 {
        tracing::warn!(
            "No sensor records found in the last 24 hours (since {})",
            one_day_ago
        );

        return api_success(json!({
            "total": 0,
            "normal_count": 0,
            "abnormal_count": 0,
            "normal_ratio": null,
            "abnormal_ratio": null,
            "temp_range": {
                "min": crate::inference::NORMAL_TEMP_RANGE.0,
                "max": crate::inference::NORMAL_TEMP_RANGE.1
            },
            "humidity_range": {
                "min": crate::inference::NORMAL_HUMIDITY_RANGE.0,
                "max": crate::inference::NORMAL_HUMIDITY_RANGE.1
            },
            "details": []
        }));
    }

    let mut abnormal_count: usize = 0;
    let mut details = Vec::with_capacity(total);

    for row in &rows {
        let tag_serial_number = row.try_get::<i64, _>("tag_serial_number").unwrap_or(0);
        let temperature = row.try_get::<f64, _>("temperature").unwrap_or(0.0);
        let humidity = row.try_get::<f64, _>("humidity").unwrap_or(0.0);
        let sampled_at = row.try_get::<i64, _>("sampled_at").unwrap_or(0);

        let is_normal = is_climate_in_range(temperature, humidity);
        if !is_normal {
            abnormal_count += 1;
        }

        details.push(json!({
            "tag_serial_number": tag_serial_number,
            "temperature": temperature,
            "humidity": humidity,
            "sampled_at": sampled_at,
            "is_normal": is_normal,
        }));
    }

    let normal_count = total - abnormal_count;
    let abnormal_ratio = round4(abnormal_count as f64 / total as f64);
    let normal_ratio = round4(normal_count as f64 / total as f64);

    api_success(json!({
        "total": total,
        "normal_count": normal_count,
        "abnormal_count": abnormal_count,
        "normal_ratio": normal_ratio,
        "abnormal_ratio": abnormal_ratio,
        "temp_range": {
            "min": crate::inference::NORMAL_TEMP_RANGE.0,
            "max": crate::inference::NORMAL_TEMP_RANGE.1
        },
        "humidity_range": {
            "min": crate::inference::NORMAL_HUMIDITY_RANGE.0,
            "max": crate::inference::NORMAL_HUMIDITY_RANGE.1
        },
        "details": details,
    }))
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}
