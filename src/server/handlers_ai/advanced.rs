use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::models::DiagnosisRecord;

use super::super::{AppState, now_millis};
use super::{
    persistence::{create_disease_task_if_needed, save_record_image, store_diagnosis_record},
    request::extract_citrus_request,
    review::{apply_review_threshold_to_fields, confidence_threshold_percent},
};

pub(crate) async fn citrus_disease_advanced_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> Response {
    let payload =
        match extract_citrus_request(&state, &headers, request, "citrus_disease_advanced").await {
            Ok(payload) => payload,
            Err(response) => return response,
        };

    let temperature = payload.temperature;
    let humidity = payload.humidity;

    let gate = match state
        .inference
        .predict_fruit_tree(Some(payload.image_data.clone()))
        .await
    {
        Ok(gate) => gate,
        Err(err) => {
            tracing::error!("模型1果树判断失败: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": format!("识别失败: {}", err),
                    "data": null
                })),
            )
                .into_response();
        }
    };

    if !gate.is_fruit_tree {
        let timestamp = now_millis();
        let record_id = uuid::Uuid::new_v4().to_string();
        let record = DiagnosisRecord {
            id: record_id.clone(),
            timestamp,
            predicted_class: gate.predicted_class.clone(),
            confidence: gate.confidence,
            is_citrus_leaf: false,
            citrus_type: "非柑橘".to_string(),
            is_healthy: false,
            disease_name: String::new(),
            severity: "健康".to_string(),
            treatment_suggestion: "请上传清晰的果树叶片图片以便继续诊断".to_string(),
            preventive_measures: "确保拍摄主体为单片叶片，光线充足、无遮挡。".to_string(),
            image_quality_warning: String::new(),
            username: Some(payload.username),
            area: payload.area,
            temp: temperature,
            humm: humidity,
            image_path: save_record_image(&record_id, &payload.image_data),
        };

        let _ = store_diagnosis_record(&state, &record).await;

        return (
            StatusCode::OK,
            Json(json!({
                "code": 200,
                "message": "success",
                "data": {
                    "predicted_class": record.predicted_class,
                    "confidence": record.confidence,
                    "stage": "model_1",
                    "is_citrus_leaf": false,
                    "citrus_type": "非柑橘",
                    "is_healthy": false,
                    "disease_name": "",
                    "severity": "健康",
                    "treatment_suggestion": record.treatment_suggestion,
                    "preventive_measures": record.preventive_measures,
                    "image_quality_warning": ""
                },
                "timestamp": timestamp
            })),
        )
            .into_response();
    }

    tracing::info!(
        "model_1 判定为果树（置信度 {:.1}%），调用 OpenRouter 进行高级识别",
        gate.confidence
    );

    let analysis_response = match state
        .client
        .analyze_citrus(Some(payload.image_data.clone()))
        .await
    {
        Ok(response) => response,
        Err(err) => {
            tracing::error!("OpenRouter 高级识别失败: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": format!("高级识别失败: {}", err),
                    "data": null
                })),
            )
                .into_response();
        }
    };

    let analysis = analysis_response.data;
    let mut predicted_class = if !analysis.is_citrus_leaf {
        "非果树".to_string()
    } else if analysis.disease_analysis.is_healthy {
        "健康果树".to_string()
    } else {
        analysis.disease_analysis.disease_name.clone()
    };
    let confidence = analysis.disease_analysis.confidence * 100.0;
    let is_citrus_leaf = analysis.is_citrus_leaf;
    let citrus_type = format!("{:?}", analysis.citrus_type);
    let mut is_healthy = analysis.disease_analysis.is_healthy;
    let mut disease_name = analysis.disease_analysis.disease_name.clone();
    let mut severity = format!("{:?}", analysis.disease_analysis.severity);
    let mut treatment_suggestion = analysis.disease_analysis.treatment_suggestion.clone();
    let mut preventive_measures = analysis.disease_analysis.preventive_measures.clone();
    let mut image_quality_warning = analysis.image_quality_warning.clone();
    let review_required = apply_review_threshold_to_fields(
        &mut predicted_class,
        &mut is_healthy,
        &mut disease_name,
        &mut severity,
        &mut treatment_suggestion,
        &mut preventive_measures,
        &mut image_quality_warning,
        confidence,
        confidence_threshold_percent(&state).await,
    );

    let timestamp = now_millis();
    let record_id = uuid::Uuid::new_v4().to_string();
    let record = DiagnosisRecord {
        id: record_id.clone(),
        timestamp,
        predicted_class: predicted_class.clone(),
        confidence,
        is_citrus_leaf,
        citrus_type: citrus_type.clone(),
        is_healthy,
        disease_name: disease_name.clone(),
        severity: severity.clone(),
        treatment_suggestion: treatment_suggestion.clone(),
        preventive_measures: preventive_measures.clone(),
        image_quality_warning: image_quality_warning.clone(),
        username: Some(payload.username),
        area: payload.area,
        temp: temperature,
        humm: humidity,
        image_path: save_record_image(&record_id, &payload.image_data),
    };

    match store_diagnosis_record(&state, &record).await {
        Ok(()) => tracing::info!(
            "已记录高级识别结果: {} (置信度: {}%)",
            predicted_class,
            confidence
        ),
        Err(err) => tracing::error!("高级识别结果写入数据库失败: {}", err),
    }

    match create_disease_task_if_needed(&state, &record).await {
        Ok(Some(task_title)) => tracing::info!("已自动生成治理任务: {}", task_title),
        Ok(None) => {}
        Err(err) => tracing::error!("自动生成任务失败: {}", err),
    }

    (
        StatusCode::OK,
        Json(json!({
            "code": 200,
            "message": "success",
            "data": {
                "predicted_class": predicted_class,
                "confidence": confidence,
                "stage": "openrouter",
                "is_citrus_leaf": is_citrus_leaf,
                "citrus_type": citrus_type,
                "is_healthy": is_healthy,
                "disease_name": disease_name,
                "severity": severity,
                "treatment_suggestion": treatment_suggestion,
                "preventive_measures": preventive_measures,
                "image_quality_warning": image_quality_warning,
                "review_required": review_required
            },
            "timestamp": timestamp
        })),
    )
        .into_response()
}
