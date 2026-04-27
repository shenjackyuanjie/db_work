use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::models::DiagnosisRecord;

use super::{
    persistence::{create_disease_task_if_needed, save_record_image, store_diagnosis_record},
    request::extract_citrus_request,
    review::{apply_review_threshold_to_prediction, confidence_threshold_percent},
};
use super::super::{AppState, now_millis};

pub(crate) async fn citrus_disease_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> Response {
    let payload = match extract_citrus_request(&state, &headers, request, "citrus_disease").await {
        Ok(payload) => payload,
        Err(response) => return response,
    };

    let temperature = payload.temperature;
    let humidity = payload.humidity;

    match state
        .inference
        .predict_citrus_disease(Some(payload.image_data.clone()), temperature, humidity)
        .await
    {
        Ok(mut prediction) => {
            let threshold_percent = confidence_threshold_percent(&state).await;
            let review_required =
                apply_review_threshold_to_prediction(&mut prediction, threshold_percent);
            let predicted_class = prediction.predicted_class;
            let confidence = prediction.confidence;
            let timestamp = now_millis();
            let record_id = uuid::Uuid::new_v4().to_string();

            let record = DiagnosisRecord {
                id: record_id.clone(),
                timestamp,
                predicted_class: predicted_class.clone(),
                confidence,
                is_citrus_leaf: prediction.is_citrus_leaf,
                citrus_type: prediction.citrus_type.clone(),
                is_healthy: prediction.is_healthy,
                disease_name: prediction.disease_name.clone(),
                severity: prediction.severity.clone(),
                treatment_suggestion: prediction.treatment_suggestion.clone(),
                preventive_measures: prediction.preventive_measures.clone(),
                image_quality_warning: prediction.image_quality_warning.clone(),
                username: payload.username,
                area: payload.area,
                temp: temperature,
                humm: humidity,
                image_path: save_record_image(&record_id, &payload.image_data),
            };

            if record.is_citrus_leaf {
                match store_diagnosis_record(&state, &record).await {
                    Ok(()) => tracing::info!(
                        "已记录识别结果: {} (置信度: {}%)",
                        predicted_class,
                        confidence
                    ),
                    Err(err) => tracing::error!("识别结果写入数据库失败: {}", err),
                }

                match create_disease_task_if_needed(&state, &record).await {
                    Ok(Some(task_title)) => tracing::info!("已自动生成治理任务: {}", task_title),
                    Ok(None) => {}
                    Err(err) => tracing::error!("自动生成任务失败: {}", err),
                }
            }

            (
                StatusCode::OK,
                Json(json!({
                    "code": 200,
                    "message": "success",
                    "data": {
                        "predicted_class": predicted_class,
                        "confidence": confidence,
                        "stage": prediction.stage,
                        "is_citrus_leaf": prediction.is_citrus_leaf,
                        "citrus_type": prediction.citrus_type,
                        "is_healthy": prediction.is_healthy,
                        "disease_name": prediction.disease_name,
                        "severity": prediction.severity,
                        "treatment_suggestion": prediction.treatment_suggestion,
                        "preventive_measures": prediction.preventive_measures,
                        "image_quality_warning": prediction.image_quality_warning,
                        "image_predicted_class": prediction.image_predicted_class,
                        "image_confidence": prediction.image_confidence,
                        "climate_validation": prediction.climate_validation,
                        "temperature": temperature,
                        "humidity": humidity,
                        "review_required": review_required
                    },
                    "timestamp": timestamp
                })),
            )
                .into_response()
        }
        Err(err) => {
            tracing::error!("柑橘病害识别失败: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": format!("识别失败: {}", err),
                    "data": null
                })),
            )
                .into_response()
        }
    }
}