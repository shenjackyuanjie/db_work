use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;
use sqlx::Row;

use crate::models::{ChatApiRequest, DiagnosisRecord, FertilizationPlanRequest};

use super::super::{
    AppState, normalize_recognition_record_image_path, now_millis, username_by_token,
};

pub(crate) async fn citrus_analyze_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChatApiRequest>,
) -> Response {
    let token = match crate::user_routes::extract_auth_token(&headers).or(request.token.clone()) {
        Some(token) => token,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Missing token" })),
            )
                .into_response();
        }
    };

    if username_by_token(&state, &token).await.is_none() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid token" })),
        )
            .into_response();
    }

    println!(
        "处理请求 图像数据长度: {}",
        request.image.as_ref().map_or(0, |image| image.len())
    );

    match state.client.analyze_citrus(request.image).await {
        Ok(response) => {
            println!("柑橘分析请求处理成功 usage: {:?}", response.usage);
            (
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "data": response.data,
                    "usage": response.usage,
                    "metrics": response.metrics
                })),
            )
                .into_response()
        }
        Err(err) => {
            println!("柑橘分析请求处理失败: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "success": false,
                    "error": err.to_string(),
                })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn generate_handler(State(state): State<AppState>) -> Response {
    let rows = sqlx::query(
        "SELECT id, timestamp, predicted_class, confidence, is_citrus_leaf, citrus_type, is_healthy, disease_name, severity, treatment_suggestion, preventive_measures, image_quality_warning, username, area, temp, humm, image_path FROM app_diagnosis_records ORDER BY timestamp DESC LIMIT 200",
    )
    .fetch_all(&state.db)
    .await;

    let records = match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|row| {
                let image_path = row.try_get::<Option<String>, _>("image_path").unwrap_or(None);

                DiagnosisRecord {
                    id: row.try_get::<String, _>("id").unwrap_or_default(),
                    timestamp: row.try_get::<i64, _>("timestamp").unwrap_or(0).max(0) as u64,
                    predicted_class: row
                        .try_get::<String, _>("predicted_class")
                        .unwrap_or_default(),
                    confidence: row.try_get::<f64, _>("confidence").unwrap_or(0.0),
                    is_citrus_leaf: row.try_get::<bool, _>("is_citrus_leaf").unwrap_or(false),
                    citrus_type: row
                        .try_get::<String, _>("citrus_type")
                        .unwrap_or_default(),
                    is_healthy: row.try_get::<bool, _>("is_healthy").unwrap_or(false),
                    disease_name: row.try_get::<String, _>("disease_name").unwrap_or_default(),
                    severity: row.try_get::<String, _>("severity").unwrap_or_default(),
                    treatment_suggestion: row
                        .try_get::<String, _>("treatment_suggestion")
                        .unwrap_or_default(),
                    preventive_measures: row
                        .try_get::<String, _>("preventive_measures")
                        .unwrap_or_default(),
                    image_quality_warning: row
                        .try_get::<String, _>("image_quality_warning")
                        .unwrap_or_default(),
                    username: row.try_get::<Option<String>, _>("username").unwrap_or(None),
                    area: row.try_get::<Option<String>, _>("area").unwrap_or(None),
                    temp: row.try_get::<Option<f64>, _>("temp").unwrap_or(None),
                    humm: row.try_get::<Option<f64>, _>("humm").unwrap_or(None),
                    image_path: normalize_recognition_record_image_path(image_path.as_deref()),
                }
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": format!("读取识别记录失败: {}", err),
                    "data": null
                })),
            )
                .into_response();
        }
    };

    match state.client.generate_fertilization_text(&records).await {
        Ok(text) => (
            StatusCode::OK,
            Json(json!({
                "code": 200,
                "message": "success",
                "data": {
                    "textii": text
                },
                "timestamp": now_millis()
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!("生成施肥方案文本失败: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": format!("生成失败: {}", err),
                    "data": null
                })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn generate_fertilization_plan_handler(
    State(state): State<AppState>,
    Json(request): Json<FertilizationPlanRequest>,
) -> Response {
    match state.client.generate_fertilization_plan(&request).await {
        Ok(plan) => (
            StatusCode::OK,
            Json(json!({
                "code": 200,
                "message": "success",
                "data": {
                    "planId": plan.plan_id,
                    "title": plan.title,
                    "content": plan.content,
                    "recommendedFertilizers": plan.recommended_fertilizers.iter().map(|item| json!({
                        "name": item.name,
                        "amount": item.amount,
                        "applicationMethod": item.application_method
                    })).collect::<Vec<_>>(),
                    "applicationSchedule": plan.application_schedule.iter().map(|item| json!({
                        "stage": item.stage,
                        "date": item.date,
                        "description": item.description
                    })).collect::<Vec<_>>()
                },
                "timestamp": now_millis()
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!("生成施肥方案失败: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": format!("生成失败: {}", err),
                    "data": null
                })),
            )
                .into_response()
        }
    }
}