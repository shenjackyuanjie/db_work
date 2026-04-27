use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{TimeZone, Utc};
use serde_json::json;
use sqlx::Row;

use super::super::{
    AppState, DiseaseTreatmentQuery, UsernameQuery, api_response, api_success,
    disease_treatment_text, normalize_recognition_record_image_path, risk_from_disease_name,
};

pub(crate) async fn recognition_records_api_handler(
    State(state): State<AppState>,
    Query(query): Query<UsernameQuery>,
) -> Response {
    let username = query.username;

    let rows = sqlx::query(
        "SELECT id, predicted_class, area, confidence, timestamp, image_path FROM app_diagnosis_records WHERE username = $1 ORDER BY timestamp ASC LIMIT 20",
    )
    .bind(&username)
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(rows) => {
            let mut list = rows
                .into_iter()
                .map(|record| {
                    let predicted_class = record
                        .try_get::<String, _>("predicted_class")
                        .unwrap_or_default();
                    let image_path = record
                        .try_get::<Option<String>, _>("image_path")
                        .unwrap_or(None);
                    let recognition_date = {
                        let timestamp = record.try_get::<i64, _>("timestamp").unwrap_or(0);
                        Utc.timestamp_millis_opt(timestamp)
                            .single()
                            .map(|datetime| datetime.format("%Y-%m-%d").to_string())
                            .unwrap_or_default()
                    };

                    json!({
                        "id": record.try_get::<String, _>("id").unwrap_or_default(),
                        "imagePath": normalize_recognition_record_image_path(image_path.as_deref()).unwrap_or_default(),
                        "diseaseName": predicted_class,
                        "area": record.try_get::<Option<String>, _>("area").unwrap_or(None).unwrap_or_else(|| "未指定区域".to_string()),
                        "riskLevel": risk_from_disease_name(&record.try_get::<String, _>("predicted_class").unwrap_or_default()),
                        "recognitionDate": recognition_date,
                        "confidence": record.try_get::<f64, _>("confidence").unwrap_or(0.0),
                        "created_at": record.try_get::<i64, _>("timestamp").unwrap_or(0)
                    })
                })
                .collect::<Vec<_>>();
            list.reverse();
            api_success(json!({ "records": list }))
        }
        Err(err) => api_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            500,
            format!("db error: {}", err),
            serde_json::Value::Null,
        )
        .into_response(),
    }
}

pub(crate) async fn disease_treatment_api_handler(
    Query(query): Query<DiseaseTreatmentQuery>,
) -> Response {
    let disease_name = match query
        .disease_name
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        Some(name) => name,
        None => {
            return api_response(
                StatusCode::BAD_REQUEST,
                400,
                "disease_name parameter is required",
                serde_json::Value::Null,
            )
            .into_response();
        }
    };

    api_success(json!({
        "disease_name": disease_name,
        "treatment": disease_treatment_text(disease_name)
    }))
}