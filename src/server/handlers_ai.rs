use axum::{
    Json,
    extract::{Json as AxumJson, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use sqlx::Row;

use crate::models::{ChatApiRequest, DiagnosisRecord, FertilizationPlanRequest};
use crate::system_settings::load_system_settings;

use super::{
    AppState, disease_treatment_text, normalize_recognition_record_image_path, now_millis,
    save_recognition_record_image, username_by_token,
};

async fn confidence_threshold_percent(state: &AppState) -> f64 {
    load_system_settings(&state.db)
        .await
        .map(|settings| settings.confidence_threshold_percent())
        .unwrap_or(75.0)
}

fn apply_review_threshold_to_prediction(
    prediction: &mut crate::inference::types::DiseasePrediction,
    threshold_percent: f64,
) -> bool {
    if prediction.predicted_class == "非果树" || prediction.confidence >= threshold_percent {
        return false;
    }

    prediction.predicted_class = "待人工复核".to_string();
    prediction.is_healthy = false;
    prediction.disease_name = "待人工复核".to_string();
    prediction.severity = "待复核".to_string();
    prediction.treatment_suggestion =
        "当前识别结果低于管理员设定的置信度阈值，请重新拍摄或人工复核。".to_string();
    prediction.preventive_measures =
        "建议保证叶片主体清晰、居中、无遮挡，并在自然光下重新采集图片。".to_string();
    if prediction.image_quality_warning.is_empty() {
        prediction.image_quality_warning = "识别置信度较低，建议复核".to_string();
    }

    true
}

fn apply_review_threshold_to_fields(
    predicted_class: &mut String,
    is_healthy: &mut bool,
    disease_name: &mut String,
    severity: &mut String,
    treatment_suggestion: &mut String,
    preventive_measures: &mut String,
    image_quality_warning: &mut String,
    confidence: f64,
    threshold_percent: f64,
) -> bool {
    if predicted_class == "非果树" || confidence >= threshold_percent {
        return false;
    }

    *predicted_class = "待人工复核".to_string();
    *is_healthy = false;
    *disease_name = "待人工复核".to_string();
    *severity = "待复核".to_string();
    *treatment_suggestion =
        "当前识别结果低于管理员设定的置信度阈值，请重新拍摄或人工复核。".to_string();
    *preventive_measures =
        "建议保证叶片主体清晰、居中、无遮挡，并在自然光下重新采集图片。".to_string();
    if image_quality_warning.is_empty() {
        *image_quality_warning = "识别置信度较低，建议复核".to_string();
    }

    true
}

// #[axum::debug_handler]
pub async fn citrus_analyze_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(request): AxumJson<ChatApiRequest>,
) -> impl IntoResponse {
    let token = match crate::user_routes::extract_auth_token(&headers).or(request.token.clone()) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Missing token" })),
            )
                .into_response();
        }
    };

    let token_valid = { username_by_token(&state, &token).await.is_some() };
    if !token_valid {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid token" })),
        )
            .into_response();
    }

    println!(
        "处理请求 图像数据长度: {}",
        request.image.as_ref().map_or(0, |img| img.len())
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
        Err(e) => {
            println!("柑橘分析请求处理失败: {}", e);
            let error_response = json!({
                "success": false,
                "error": e.to_string(),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)).into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct CitrusDiseaseJsonRequest {
    #[serde(rename = "IMAGE")]
    image_upper: Option<String>,
    image: Option<String>,
    username: Option<String>,
    area: Option<String>,
    temperature: Option<Value>,
    humidity: Option<Value>,
}

fn payload_preview(input: &str) -> String {
    input.chars().take(48).collect()
}

fn parse_optional_json_f64(value: Option<Value>, field_name: &str) -> Result<Option<f64>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_f64()
            .map(Some)
            .ok_or_else(|| format!("{} 必须是合法数字", field_name)),
        Some(Value::String(text)) => parse_optional_text_f64(&text, field_name),
        Some(_) => Err(format!("{} 必须是数字或数字字符串", field_name)),
    }
}

fn parse_optional_text_f64(value: &str, field_name: &str) -> Result<Option<f64>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    trimmed
        .parse::<f64>()
        .map(Some)
        .map_err(|_| format!("{} 必须是合法数字", field_name))
}

fn bad_request_response(message: String) -> axum::response::Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "code": 400,
            "message": message,
            "data": null
        })),
    )
        .into_response()
}

// #[axum::debug_handler]
pub async fn citrus_disease_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> impl IntoResponse {
    let mut image_data: Option<String> = None;
    let mut image_base64_text: Option<String> = None;
    let mut username: Option<String> = None;
    let mut area: Option<String> = None;
    let mut temperature: Option<f64> = None;
    let mut humidity: Option<f64> = None;

    let auth_username = match crate::user_routes::extract_auth_token(&headers) {
        Some(token) => match username_by_token(&state, &token).await {
            Some(name) => Some(name),
            None => {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "code": 401,
                        "message": "Invalid token",
                        "data": null
                    })),
                )
                    .into_response();
            }
        },
        None => None,
    };

    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    tracing::info!("citrus_disease 请求: content_type={}", content_type);

    let body_bytes = match axum::body::to_bytes(request.into_body(), 20 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("读取请求体失败: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "code": 400,
                    "message": format!("读取请求体失败: {}", e),
                    "data": null
                })),
            )
                .into_response();
        }
    };

    tracing::info!("citrus_disease 请求体大小: {} bytes", body_bytes.len());

    if content_type.starts_with("application/json") {
        let payload: CitrusDiseaseJsonRequest = match serde_json::from_slice(&body_bytes) {
            Ok(data) => data,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "code": 400,
                        "message": format!("JSON 请求体格式错误: {}", e),
                        "data": null
                    })),
                )
                    .into_response();
            }
        };

        tracing::info!(
            "citrus_disease JSON字段: has_IMAGE={} has_image={} has_username={} has_area={} has_temperature={} has_humidity={}",
            payload
                .image_upper
                .as_ref()
                .is_some_and(|x| !x.trim().is_empty()),
            payload.image.as_ref().is_some_and(|x| !x.trim().is_empty()),
            payload
                .username
                .as_ref()
                .is_some_and(|x| !x.trim().is_empty()),
            payload.area.as_ref().is_some_and(|x| !x.trim().is_empty()),
            payload.temperature.is_some(),
            payload.humidity.is_some()
        );

        image_base64_text = payload.image_upper.or(payload.image);
        username = payload
            .username
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty());
        area = payload
            .area
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty());
        temperature = match parse_optional_json_f64(payload.temperature, "temperature") {
            Ok(value) => value,
            Err(message) => return bad_request_response(message),
        };
        humidity = match parse_optional_json_f64(payload.humidity, "humidity") {
            Ok(value) => value,
            Err(message) => return bad_request_response(message),
        };
    } else if content_type.starts_with("multipart/form-data") {
        let boundary = match multer::parse_boundary(&content_type) {
            Ok(boundary) => boundary,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "code": 400,
                        "message": format!("multipart boundary 解析失败: {}", e),
                        "data": null
                    })),
                )
                    .into_response();
            }
        };

        let stream = futures_util::stream::once(async move {
            Ok::<axum::body::Bytes, std::io::Error>(body_bytes)
        });
        let mut multipart = multer::Multipart::new(stream, boundary);

        loop {
            match multipart.next_field().await {
                Ok(Some(field)) => {
                    let field_name = field.name().map(|x| x.to_string());
                    tracing::debug!("citrus_disease multipart 字段: {:?}", field_name);
                    match field_name.as_deref() {
                        Some("image") => {
                            let content_type = field
                                .content_type()
                                .map(|x| x.to_string())
                                .unwrap_or_else(|| "image/jpeg".to_string());
                            match field.bytes().await {
                                Ok(bytes) => {
                                    let b64 =
                                        base64::engine::general_purpose::STANDARD.encode(&bytes);
                                    image_data =
                                        Some(format!("data:{};base64,{}", content_type, b64));
                                    tracing::info!(
                                        "citrus_disease multipart image读取成功: mime={} size={} bytes",
                                        content_type,
                                        bytes.len()
                                    );
                                }
                                Err(e) => {
                                    tracing::error!("读取图片字段失败: {}", e);
                                    return (
                                        StatusCode::BAD_REQUEST,
                                        Json(serde_json::json!({
                                            "code": 400,
                                            "message": format!("读取图片数据失败: {}", e),
                                            "data": null
                                        })),
                                    )
                                        .into_response();
                                }
                            }
                        }
                        Some("IMAGE") => match field.text().await {
                            Ok(text) => {
                                tracing::info!(
                                    "citrus_disease multipart IMAGE读取成功: len={} preview={}...",
                                    text.len(),
                                    payload_preview(text.trim())
                                );
                                image_base64_text = Some(text)
                            }
                            Err(e) => {
                                tracing::error!("读取 IMAGE 字段失败: {}", e);
                                return (
                                    StatusCode::BAD_REQUEST,
                                    Json(serde_json::json!({
                                        "code": 400,
                                        "message": format!("读取IMAGE字段失败: {}", e),
                                        "data": null
                                    })),
                                )
                                    .into_response();
                            }
                        },
                        Some("username") => {
                            if let Ok(text) = field.text().await {
                                let trimmed = text.trim();
                                if !trimmed.is_empty() {
                                    username = Some(trimmed.to_string());
                                }
                            }
                        }
                        Some("area") => {
                            if let Ok(text) = field.text().await {
                                let trimmed = text.trim();
                                if !trimmed.is_empty() {
                                    area = Some(trimmed.to_string());
                                }
                            }
                        }
                        Some("temperature") => match field.text().await {
                            Ok(text) => match parse_optional_text_f64(&text, "temperature") {
                                Ok(value) => temperature = value,
                                Err(message) => return bad_request_response(message),
                            },
                            Err(e) => {
                                tracing::error!("读取 temperature 字段失败: {}", e);
                                return bad_request_response(format!(
                                    "读取temperature字段失败: {}",
                                    e
                                ));
                            }
                        },
                        Some("humidity") => match field.text().await {
                            Ok(text) => match parse_optional_text_f64(&text, "humidity") {
                                Ok(value) => humidity = value,
                                Err(message) => return bad_request_response(message),
                            },
                            Err(e) => {
                                tracing::error!("读取 humidity 字段失败: {}", e);
                                return bad_request_response(format!(
                                    "读取humidity字段失败: {}",
                                    e
                                ));
                            }
                        },
                        _ => {}
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::error!("解析 multipart 失败: {}", e);
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "code": 400,
                            "message": format!("解析请求失败: {}", e),
                            "data": null
                        })),
                    )
                        .into_response();
                }
            }
        }
    } else {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Json(serde_json::json!({
                "code": 415,
                "message": "仅支持 application/json 或 multipart/form-data",
                "data": null
            })),
        )
            .into_response();
    }

    if image_data.is_none()
        && let Some(mut base64_image) = image_base64_text
    {
        tracing::info!(
            "citrus_disease 原始IMAGE: len={} has_data_uri={} preview={}...",
            base64_image.len(),
            base64_image
                .trim()
                .to_ascii_lowercase()
                .starts_with("data:image/"),
            payload_preview(base64_image.trim())
        );
        if !base64_image.starts_with("data:image/") {
            base64_image = format!("data:image/jpeg;base64,{}", base64_image);
            tracing::info!(
                "citrus_disease IMAGE补齐data-uri后: len={} preview={}...",
                base64_image.len(),
                payload_preview(base64_image.trim())
            );
        }
        image_data = Some(base64_image);
    }

    if username.is_none() {
        username = auth_username;
    }

    if image_data.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "message": "请求中未找到 image 字段",
                "data": null
            })),
        )
            .into_response();
    }

    match state
        .inference
        .predict_citrus_disease(image_data.clone(), temperature, humidity)
        .await
    {
        Ok(prediction) => {
            let threshold_percent = confidence_threshold_percent(&state).await;
            let mut prediction = prediction;
            let review_required =
                apply_review_threshold_to_prediction(&mut prediction, threshold_percent);
            let predicted_class = prediction.predicted_class;
            let confidence = prediction.confidence;
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            let record_id = uuid::Uuid::new_v4().to_string();

            let saved_image_path: Option<String> = if let Some(ref data_url) = image_data {
                let save_result = save_recognition_record_image(&record_id, data_url);
                match save_result {
                    Ok(path) => {
                        tracing::info!("识别图片已保存: {}", path);
                        Some(path)
                    }
                    Err(e) => {
                        tracing::error!("识别图片保存失败: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            let record = DiagnosisRecord {
                id: record_id,
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
                username,
                area,
                temp: temperature,
                humm: humidity,
                image_path: saved_image_path,
            };

            // 只在是柑橘叶片时才插入数据库记录
            if record.is_citrus_leaf {
                match sqlx::query(
                    "INSERT INTO app_diagnosis_records (id, timestamp, predicted_class, confidence, is_citrus_leaf, citrus_type, is_healthy, disease_name, severity, treatment_suggestion, preventive_measures, image_quality_warning, username, area, temp, humm, image_path) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)",
                )
                .bind(&record.id)
                .bind(record.timestamp as i64)
                .bind(&record.predicted_class)
                .bind(record.confidence)
                .bind(record.is_citrus_leaf)
                .bind(&record.citrus_type)
                .bind(record.is_healthy)
                .bind(&record.disease_name)
                .bind(&record.severity)
                .bind(&record.treatment_suggestion)
                .bind(&record.preventive_measures)
                .bind(&record.image_quality_warning)
                .bind(&record.username)
                .bind(&record.area)
                .bind(record.temp)
                .bind(record.humm)
                .bind(&record.image_path)
                .execute(&state.db)
                .await {
                    Ok(_) => tracing::info!("已记录识别结果: {} (置信度: {}%)", predicted_class, confidence),
                    Err(e) => tracing::error!("识别结果写入数据库失败: {}", e),
                }

                // 若检测到病害且有用户名，自动生成治理任务
                if !record.is_healthy
                    && record.predicted_class != "非果树"
                    && record.predicted_class != "待人工复核"
                    && let Some(ref uname) = record.username
                {
                    let task_title = format!("{}治理", record.disease_name);
                    let task_description = disease_treatment_text(&record.disease_name).to_string();
                    let task_id = uuid::Uuid::new_v4().to_string();
                    match sqlx::query(
                            "INSERT INTO app_tasks (id, username, title, description, risk_level, task_type, source, is_completed, created_at, completed_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
                        )
                        .bind(&task_id)
                        .bind(uname)
                        .bind(&task_title)
                        .bind(&task_description)
                        .bind("高风险")
                        .bind("疾病识别")
                        .bind("自动生成")
                        .bind(false)
                        .bind(record.timestamp as i64)
                        .bind(None::<i64>)
                        .execute(&state.db)
                        .await {
                            Ok(_) => tracing::info!("已自动生成治理任务: {}", task_title),
                            Err(e) => tracing::error!("自动生成任务失败: {}", e),
                        }
                }
            }

            (
                StatusCode::OK,
                Json(serde_json::json!({
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
        Err(e) => {
            tracing::error!("柑橘病害识别失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "code": 500,
                    "message": format!("识别失败: {}", e),
                    "data": null
                })),
            )
                .into_response()
        }
    }
}

// #[axum::debug_handler]
pub async fn generate_handler(State(state): State<AppState>) -> impl IntoResponse {
    let rows = sqlx::query(
        "SELECT id, timestamp, predicted_class, confidence, is_citrus_leaf, citrus_type, is_healthy, disease_name, severity, treatment_suggestion, preventive_measures, image_quality_warning, username, area, temp, humm, image_path FROM app_diagnosis_records ORDER BY timestamp DESC LIMIT 200",
    )
    .fetch_all(&state.db)
    .await;

    let records = match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|r| {
                let image_path = r.try_get::<Option<String>, _>("image_path").unwrap_or(None);

                DiagnosisRecord {
                    id: r.try_get::<String, _>("id").unwrap_or_default(),
                    timestamp: r.try_get::<i64, _>("timestamp").unwrap_or(0).max(0) as u64,
                    predicted_class: r
                        .try_get::<String, _>("predicted_class")
                        .unwrap_or_default(),
                    confidence: r.try_get::<f64, _>("confidence").unwrap_or(0.0),
                    is_citrus_leaf: r.try_get::<bool, _>("is_citrus_leaf").unwrap_or(false),
                    citrus_type: r.try_get::<String, _>("citrus_type").unwrap_or_default(),
                    is_healthy: r.try_get::<bool, _>("is_healthy").unwrap_or(false),
                    disease_name: r.try_get::<String, _>("disease_name").unwrap_or_default(),
                    severity: r.try_get::<String, _>("severity").unwrap_or_default(),
                    treatment_suggestion: r
                        .try_get::<String, _>("treatment_suggestion")
                        .unwrap_or_default(),
                    preventive_measures: r
                        .try_get::<String, _>("preventive_measures")
                        .unwrap_or_default(),
                    image_quality_warning: r
                        .try_get::<String, _>("image_quality_warning")
                        .unwrap_or_default(),
                    username: r.try_get::<Option<String>, _>("username").unwrap_or(None),
                    area: r.try_get::<Option<String>, _>("area").unwrap_or(None),
                    temp: r.try_get::<Option<f64>, _>("temp").unwrap_or(None),
                    humm: r.try_get::<Option<f64>, _>("humm").unwrap_or(None),
                    image_path: normalize_recognition_record_image_path(image_path.as_deref()),
                }
            })
            .collect::<Vec<_>>(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "code": 500,
                    "message": format!("读取识别记录失败: {}", e),
                    "data": null
                })),
            )
                .into_response();
        }
    };

    match state.client.generate_fertilization_text(&records).await {
        Ok(text) => {
            let timestamp = now_millis();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "code": 200,
                    "message": "success",
                    "data": {
                        "textii": text
                    },
                    "timestamp": timestamp
                })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("生成施肥方案文本失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "code": 500,
                    "message": format!("生成失败: {}", e),
                    "data": null
                })),
            )
                .into_response()
        }
    }
}

#[axum::debug_handler]
pub async fn generate_fertilization_plan_handler(
    State(state): State<AppState>,
    Json(req): Json<FertilizationPlanRequest>,
) -> impl IntoResponse {
    match state.client.generate_fertilization_plan(&req).await {
        Ok(plan) => {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "code": 200,
                    "message": "success",
                    "data": {
                        "planId": plan.plan_id,
                        "title": plan.title,
                        "content": plan.content,
                        "recommendedFertilizers": plan.recommended_fertilizers.iter().map(|f| serde_json::json!({
                            "name": f.name,
                            "amount": f.amount,
                            "applicationMethod": f.application_method
                        })).collect::<Vec<_>>(),
                        "applicationSchedule": plan.application_schedule.iter().map(|s| serde_json::json!({
                            "stage": s.stage,
                            "date": s.date,
                            "description": s.description
                        })).collect::<Vec<_>>()
                    },
                    "timestamp": timestamp
                })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("生成施肥方案失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "code": 500,
                    "message": format!("生成失败: {}", e),
                    "data": null
                })),
            )
                .into_response()
        }
    }
}

/// 混合推理接口：先用 model_1 判断是否为果树，再调用 OpenRouter 进行高级识别
/// 接口格式与 /api/citrus-disease 完全相同，路径为 /api/citrus-disease-v2
// #[axum::debug_handler]
pub async fn citrus_disease_advanced_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> impl IntoResponse {
    let mut image_data: Option<String> = None;
    let mut image_base64_text: Option<String> = None;
    let mut username: Option<String> = None;
    let mut area: Option<String> = None;
    let mut temperature: Option<f64> = None;
    let mut humidity: Option<f64> = None;

    let auth_username = match crate::user_routes::extract_auth_token(&headers) {
        Some(token) => match username_by_token(&state, &token).await {
            Some(name) => Some(name),
            None => {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "code": 401,
                        "message": "Invalid token",
                        "data": null
                    })),
                )
                    .into_response();
            }
        },
        None => None,
    };

    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    tracing::info!(
        "citrus_disease_advanced 请求: content_type={}",
        content_type
    );

    let body_bytes = match axum::body::to_bytes(request.into_body(), 20 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("读取请求体失败: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "code": 400,
                    "message": format!("读取请求体失败: {}", e),
                    "data": null
                })),
            )
                .into_response();
        }
    };

    tracing::info!(
        "citrus_disease_advanced 请求体大小: {} bytes",
        body_bytes.len()
    );

    if content_type.starts_with("application/json") {
        let payload: CitrusDiseaseJsonRequest = match serde_json::from_slice(&body_bytes) {
            Ok(data) => data,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "code": 400,
                        "message": format!("JSON 请求体格式错误: {}", e),
                        "data": null
                    })),
                )
                    .into_response();
            }
        };

        image_base64_text = payload.image_upper.or(payload.image);
        username = payload
            .username
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty());
        area = payload
            .area
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty());
        temperature = match parse_optional_json_f64(payload.temperature, "temperature") {
            Ok(value) => value,
            Err(message) => return bad_request_response(message),
        };
        humidity = match parse_optional_json_f64(payload.humidity, "humidity") {
            Ok(value) => value,
            Err(message) => return bad_request_response(message),
        };
    } else if content_type.starts_with("multipart/form-data") {
        let boundary = match multer::parse_boundary(&content_type) {
            Ok(boundary) => boundary,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "code": 400,
                        "message": format!("multipart boundary 解析失败: {}", e),
                        "data": null
                    })),
                )
                    .into_response();
            }
        };

        let stream = futures_util::stream::once(async move {
            Ok::<axum::body::Bytes, std::io::Error>(body_bytes)
        });
        let mut multipart = multer::Multipart::new(stream, boundary);

        loop {
            match multipart.next_field().await {
                Ok(Some(field)) => {
                    let field_name = field.name().map(|x| x.to_string());
                    tracing::debug!("citrus_disease_advanced multipart 字段: {:?}", field_name);
                    match field_name.as_deref() {
                        Some("image") => {
                            let content_type = field
                                .content_type()
                                .map(|x| x.to_string())
                                .unwrap_or_else(|| "image/jpeg".to_string());
                            match field.bytes().await {
                                Ok(bytes) => {
                                    let b64 =
                                        base64::engine::general_purpose::STANDARD.encode(&bytes);
                                    image_data =
                                        Some(format!("data:{};base64,{}", content_type, b64));
                                }
                                Err(e) => {
                                    tracing::error!("读取图片字段失败: {}", e);
                                    return (
                                        StatusCode::BAD_REQUEST,
                                        Json(serde_json::json!({
                                            "code": 400,
                                            "message": format!("读取图片数据失败: {}", e),
                                            "data": null
                                        })),
                                    )
                                        .into_response();
                                }
                            }
                        }
                        Some("IMAGE") => match field.text().await {
                            Ok(text) => image_base64_text = Some(text),
                            Err(e) => {
                                tracing::error!("读取 IMAGE 字段失败: {}", e);
                                return (
                                    StatusCode::BAD_REQUEST,
                                    Json(serde_json::json!({
                                        "code": 400,
                                        "message": format!("读取IMAGE字段失败: {}", e),
                                        "data": null
                                    })),
                                )
                                    .into_response();
                            }
                        },
                        Some("username") => {
                            if let Ok(text) = field.text().await {
                                let trimmed = text.trim();
                                if !trimmed.is_empty() {
                                    username = Some(trimmed.to_string());
                                }
                            }
                        }
                        Some("area") => {
                            if let Ok(text) = field.text().await {
                                let trimmed = text.trim();
                                if !trimmed.is_empty() {
                                    area = Some(trimmed.to_string());
                                }
                            }
                        }
                        Some("temperature") => match field.text().await {
                            Ok(text) => match parse_optional_text_f64(&text, "temperature") {
                                Ok(value) => temperature = value,
                                Err(message) => return bad_request_response(message),
                            },
                            Err(e) => {
                                tracing::error!("读取 temperature 字段失败: {}", e);
                                return bad_request_response(format!(
                                    "读取temperature字段失败: {}",
                                    e
                                ));
                            }
                        },
                        Some("humidity") => match field.text().await {
                            Ok(text) => match parse_optional_text_f64(&text, "humidity") {
                                Ok(value) => humidity = value,
                                Err(message) => return bad_request_response(message),
                            },
                            Err(e) => {
                                tracing::error!("读取 humidity 字段失败: {}", e);
                                return bad_request_response(format!(
                                    "读取humidity字段失败: {}",
                                    e
                                ));
                            }
                        },
                        _ => {}
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::error!("解析 multipart 失败: {}", e);
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "code": 400,
                            "message": format!("解析请求失败: {}", e),
                            "data": null
                        })),
                    )
                        .into_response();
                }
            }
        }
    } else {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Json(serde_json::json!({
                "code": 415,
                "message": "仅支持 application/json 或 multipart/form-data",
                "data": null
            })),
        )
            .into_response();
    }

    if image_data.is_none()
        && let Some(mut base64_image) = image_base64_text
    {
        if !base64_image.starts_with("data:image/") {
            base64_image = format!("data:image/jpeg;base64,{}", base64_image);
        }
        image_data = Some(base64_image);
    }

    if username.is_none() {
        username = auth_username;
    }

    if image_data.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "message": "请求中未找到 image 字段",
                "data": null
            })),
        )
            .into_response();
    }

    // ---- 第一步：model_1 判断是否为果树 ----
    let gate = match state.inference.predict_fruit_tree(image_data.clone()).await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("模型1果树判断失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "code": 500,
                    "message": format!("识别失败: {}", e),
                    "data": null
                })),
            )
                .into_response();
        }
    };

    // 非果树直接返回，不再调用 OpenRouter
    if !gate.is_fruit_tree {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let record_id = uuid::Uuid::new_v4().to_string();

        let saved_image_path: Option<String> = if let Some(ref data_url) = image_data {
            let save_result = save_recognition_record_image(&record_id, data_url);
            match save_result {
                Ok(path) => Some(path),
                Err(e) => {
                    tracing::error!("识别图片保存失败: {}", e);
                    None
                }
            }
        } else {
            None
        };

        let record = DiagnosisRecord {
            id: record_id,
            timestamp,
            predicted_class: gate.predicted_class.clone(),
            confidence: gate.confidence,
            is_citrus_leaf: false,
            citrus_type: "非柑橘".to_string(),
            is_healthy: false,
            disease_name: "".to_string(),
            severity: "健康".to_string(),
            treatment_suggestion: "请上传清晰的果树叶片图片以便继续诊断".to_string(),
            preventive_measures: "确保拍摄主体为单片叶片，光线充足、无遮挡。".to_string(),
            image_quality_warning: "".to_string(),
            username,
            area,
            temp: temperature,
            humm: humidity,
            image_path: saved_image_path,
        };

        let _ = sqlx::query(
            "INSERT INTO app_diagnosis_records (id, timestamp, predicted_class, confidence, is_citrus_leaf, citrus_type, is_healthy, disease_name, severity, treatment_suggestion, preventive_measures, image_quality_warning, username, area, temp, humm, image_path) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)",
        )
        .bind(&record.id)
        .bind(record.timestamp as i64)
        .bind(&record.predicted_class)
        .bind(record.confidence)
        .bind(record.is_citrus_leaf)
        .bind(&record.citrus_type)
        .bind(record.is_healthy)
        .bind(&record.disease_name)
        .bind(&record.severity)
        .bind(&record.treatment_suggestion)
        .bind(&record.preventive_measures)
        .bind(&record.image_quality_warning)
        .bind(&record.username)
        .bind(&record.area)
        .bind(record.temp)
        .bind(record.humm)
        .bind(&record.image_path)
        .execute(&state.db)
        .await;

        return (
            StatusCode::OK,
            Json(serde_json::json!({
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

    // ---- 第二步：调用 OpenRouter 进行高级识别 ----
    tracing::info!(
        "model_1 判定为果树（置信度 {:.1}%），调用 OpenRouter 进行高级识别",
        gate.confidence
    );

    let analysis_response = match state.client.analyze_citrus(image_data.clone()).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("OpenRouter 高级识别失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "code": 500,
                    "message": format!("高级识别失败: {}", e),
                    "data": null
                })),
            )
                .into_response();
        }
    };

    let analysis = analysis_response.data;

    let predicted_class = if !analysis.is_citrus_leaf {
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
    let mut predicted_class = predicted_class;
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

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let record_id = uuid::Uuid::new_v4().to_string();

    let saved_image_path: Option<String> = if let Some(ref data_url) = image_data {
        let save_result = save_recognition_record_image(&record_id, data_url);
        match save_result {
            Ok(path) => {
                tracing::info!("识别图片已保存: {}", path);
                Some(path)
            }
            Err(e) => {
                tracing::error!("识别图片保存失败: {}", e);
                None
            }
        }
    } else {
        None
    };

    let record = DiagnosisRecord {
        id: record_id,
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
        username,
        area,
        temp: temperature,
        humm: humidity,
        image_path: saved_image_path,
    };

    match sqlx::query(
        "INSERT INTO app_diagnosis_records (id, timestamp, predicted_class, confidence, is_citrus_leaf, citrus_type, is_healthy, disease_name, severity, treatment_suggestion, preventive_measures, image_quality_warning, username, area, temp, humm, image_path) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)",
    )
    .bind(&record.id)
    .bind(record.timestamp as i64)
    .bind(&record.predicted_class)
    .bind(record.confidence)
    .bind(record.is_citrus_leaf)
    .bind(&record.citrus_type)
    .bind(record.is_healthy)
    .bind(&record.disease_name)
    .bind(&record.severity)
    .bind(&record.treatment_suggestion)
    .bind(&record.preventive_measures)
    .bind(&record.image_quality_warning)
    .bind(&record.username)
    .bind(&record.area)
    .bind(record.temp)
    .bind(record.humm)
    .bind(&record.image_path)
    .execute(&state.db)
    .await {
        Ok(_) => tracing::info!("已记录高级识别结果: {} (置信度: {}%)", predicted_class, confidence),
        Err(e) => tracing::error!("高级识别结果写入数据库失败: {}", e),
    }

    // 若检测到病害且有用户名，自动生成治理任务
    if !record.is_healthy
        && record.predicted_class != "非果树"
        && record.predicted_class != "待人工复核"
        && let Some(ref uname) = record.username
    {
        let task_title = format!("{}治理", record.disease_name);
        let task_description = disease_treatment_text(&record.disease_name).to_string();
        let task_id = uuid::Uuid::new_v4().to_string();
        match sqlx::query(
                "INSERT INTO app_tasks (id, username, title, description, risk_level, task_type, source, is_completed, created_at, completed_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(&task_id)
            .bind(uname)
            .bind(&task_title)
            .bind(&task_description)
            .bind("高风险")
            .bind("疾病识别")
            .bind("自动生成")
            .bind(false)
            .bind(record.timestamp as i64)
            .bind(None::<i64>)
            .execute(&state.db)
            .await {
                Ok(_) => tracing::info!("已自动生成治理任务: {}", task_title),
                Err(e) => tracing::error!("自动生成任务失败: {}", e),
            }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
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
