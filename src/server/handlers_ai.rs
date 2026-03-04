#[axum::debug_handler]
pub async fn citrus_analyze_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChatApiRequest>,
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

    let token_valid = {
        let tokens = state.tokens.lock().unwrap();
        tokens.contains_key(&token)
    };
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
}

#[axum::debug_handler]
pub async fn citrus_disease_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> impl IntoResponse {
    let mut image_data: Option<String> = None;
    let mut image_base64_text: Option<String> = None;
    let mut username: Option<String> = None;
    let mut area: Option<String> = None;

    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

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
                    if field.name() == Some("image") {
                        let content_type = field
                            .content_type()
                            .map(|x| x.to_string())
                            .unwrap_or_else(|| "image/jpeg".to_string());
                        match field.bytes().await {
                            Ok(bytes) => {
                                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                                image_data = Some(format!("data:{};base64,{}", content_type, b64));
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
                    } else if field.name() == Some("IMAGE") {
                        match field.text().await {
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
                        }
                    } else if field.name() == Some("username") {
                        if let Ok(text) = field.text().await {
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                username = Some(trimmed.to_string());
                            }
                        }
                    } else if field.name() == Some("area") && let Ok(text) = field.text().await {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            area = Some(trimmed.to_string());
                        }
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

    if image_data.is_none() && let Some(mut base64_image) = image_base64_text {
        if !base64_image.starts_with("data:image/") {
            base64_image = format!("data:image/jpeg;base64,{}", base64_image);
        }
        image_data = Some(base64_image);
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

    match state.inference.predict_citrus_disease(image_data).await {
        Ok(prediction) => {
            let predicted_class = prediction.predicted_class;
            let confidence = prediction.confidence;
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            let record = DiagnosisRecord {
                id: uuid::Uuid::new_v4().to_string(),
                timestamp,
                predicted_class: predicted_class.clone(),
                confidence,
                is_citrus_leaf: prediction.is_citrus_leaf,
                citrus_type: prediction.citrus_type,
                is_healthy: prediction.is_healthy,
                disease_name: prediction.disease_name,
                severity: prediction.severity,
                treatment_suggestion: prediction.treatment_suggestion,
                preventive_measures: prediction.preventive_measures,
                image_quality_warning: prediction.image_quality_warning,
                username,
                area,
            };
            state.diagnosis_records.lock().unwrap().push(record);
            tracing::info!("已记录识别结果: {} (置信度: {}%)", predicted_class, confidence);

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "code": 200,
                    "message": "success",
                    "data": {
                        "predicted_class": predicted_class,
                        "confidence": confidence,
                        "stage": prediction.stage
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

#[axum::debug_handler]
pub async fn generate_handler(State(state): State<AppState>) -> impl IntoResponse {
    let records = state.diagnosis_records.lock().unwrap().clone();

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
