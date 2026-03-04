pub async fn health_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "service": "openrouter"
        })),
    )
}

fn has_valid_session(state: &AppState, headers: &HeaderMap) -> bool {
    let token = match crate::user_routes::extract_auth_token(headers) {
        Some(t) => t,
        None => return false,
    };
    let tokens = state.tokens.lock().unwrap();
    tokens.contains_key(&token)
}

pub async fn admin_page_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !has_valid_session(&state, &headers) {
        tracing::warn!("未登录或会话无效访问 /admin.html，重定向到 /index.html");
        return Redirect::temporary("/index.html").into_response();
    }

    match tokio::fs::read_to_string("static/admin.html").await {
        Ok(content) => Html(content).into_response(),
        Err(err) => {
            tracing::error!("读取 admin 页面失败: {}", err);
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to load page").into_response()
        }
    }
}

pub async fn analyze_page_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !has_valid_session(&state, &headers) {
        tracing::warn!("未登录或会话无效访问 /analyze.html，重定向到 /index.html");
        return Redirect::temporary("/index.html").into_response();
    }

    match tokio::fs::read_to_string("static/analyze.html").await {
        Ok(content) => Html(content).into_response(),
        Err(err) => {
            tracing::error!("读取 analyze 页面失败: {}", err);
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to load page").into_response()
        }
    }
}

pub async fn api_user_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if has_valid_session(&state, &headers) {
        return crate::user_routes::me_handler(State(state), headers)
            .await
            .into_response();
    }

    let users = state.users.lock().unwrap();
    if let Some(user) = users.values().next() {
        return api_success(serde_json::json!({
            "id": user.username,
            "username": user.username,
            "email": null,
            "orchard_address": null,
            "latitude": null,
            "longitude": null,
            "created_at": user.created_at
        }))
        .into_response();
    }

    api_response(
        StatusCode::NOT_FOUND,
        404,
        "User not found",
        serde_json::Value::Null,
    )
    .into_response()
}

pub async fn home_api_handler() -> impl IntoResponse {
    api_success(serde_json::json!({
        "weatherCondition": "阴",
        "temperatureRange": "20℃ - 25℃",
        "suggestion": "建议：保持正常天气，注意防晒。",
        "healthScore": 80,
        "weeklyAlerts": 2,
        "pendingTasks": 10,
        "growthRate": 85.5,
        "DiagnosisStatus": "正常",
        "GrowthStatus": "涨果期"
    }))
}

pub async fn growth_tracking_api_handler() -> impl IntoResponse {
    api_success(serde_json::json!({
        "growthStageText": "涨果期",
        "growthStageDuration": "30",
        "startDate": "2025-12-01",
        "endDate": "2026-01-30",
        "fruitExpansionStartDate": "2025-12-01",
        "fruitExpansionEndDate": "2026-01-30",
        "colorChangeStartDate": "2026-02-01",
        "colorChangeEndDate": "2026-03-30",
        "youngFruitStartDate": "2026-04-01",
        "youngFruitEndDate": "2026-05-30",
        "diameter": 7.2,
        "ratio": 1.2
    }))
}

pub async fn diagnose_api_handler() -> impl IntoResponse {
    api_success(serde_json::json!({
        "data": "2025-12-03",
        "n_P_K_ViewModel": {
            "nitrogenValue": 110.0,
            "phosphorusValue": 50.0,
            "potassiumValue": 10.0
        },
        "percentage": 86.0,
        "getList": {
            "list": [
                {
                    "title": "氮元素含量稳定",
                    "content": "当前氮含量水平有利于叶片生长，维持现状即可"
                },
                {
                    "title": "钾元素缺乏",
                    "content": "第5区果树钾元素偏低，建议补充钾肥提高果实品质"
                },
                {
                    "title": "钙镁元素平衡",
                    "content": "当前钙镁比例适宜，有利于果实发育"
                }
            ]
        }
    }))
}

pub async fn temperature_humidity_api_handler(
    State(state): State<AppState>,
    Query(query): Query<UsernameQuery>,
) -> impl IntoResponse {
    let mut samples = {
        let records = state.temperature_humidity_records.lock().unwrap();
        if let Some(username) = query.username.as_ref() {
            records
                .iter()
                .filter(|x| x.username.as_deref() == Some(username.as_str()))
                .cloned()
                .collect::<Vec<_>>()
        } else {
            records.clone()
        }
    };

    if samples.is_empty() {
        samples = default_temperature_samples();
    }

    if samples.len() > 10 {
        samples = samples[samples.len() - 10..].to_vec();
    }

    api_success(build_temp_humidity_payload(&samples))
}

pub async fn recognition_records_api_handler(
    State(state): State<AppState>,
    Query(query): Query<UsernameQuery>,
) -> impl IntoResponse {
    let username = query
        .username
        .as_ref()
        .map(|x| x.trim())
        .filter(|x| !x.is_empty());

    if username.is_none() {
        return api_success(serde_json::json!({ "records": [] }));
    }

    let records = state.diagnosis_records.lock().unwrap();
    let mut list = records
        .iter()
        .rev()
        .filter(|record| {
            if let Some(name) = username {
                record.username.as_deref() == Some(name)
            } else {
                true
            }
        })
        .take(20)
        .map(|record| {
            serde_json::json!({
                "id": record.id,
                "imagePath": "",
                "diseaseName": record.predicted_class,
                "area": record.area.clone().unwrap_or_else(|| "未指定区域".to_string()),
                "riskLevel": risk_from_disease_name(&record.predicted_class),
                "recognitionDate": "2026-03-04",
                "confidence": record.confidence,
                "created_at": record.timestamp
            })
        })
        .collect::<Vec<_>>();
    list.reverse();

    api_success(serde_json::json!({ "records": list }))
}

pub async fn disease_treatment_api_handler(
    Query(query): Query<DiseaseTreatmentQuery>,
) -> impl IntoResponse {
    let disease_name = match query
        .disease_name
        .as_ref()
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
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

    api_success(serde_json::json!({
        "disease_name": disease_name,
        "treatment": disease_treatment_text(disease_name)
    }))
}

pub async fn get_tasks_api_handler(
    State(state): State<AppState>,
    Query(query): Query<UsernameQuery>,
) -> impl IntoResponse {
    let username = match query
        .username
        .as_ref()
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
    {
        Some(name) => name,
        None => {
            return api_response(
                StatusCode::BAD_REQUEST,
                400,
                "username parameter is required",
                serde_json::Value::Null,
            )
            .into_response();
        }
    };

    if !user_exists(&state, username) {
        return api_response(
            StatusCode::NOT_FOUND,
            404,
            "User not found",
            serde_json::Value::Null,
        )
        .into_response();
    }

    let tasks = state.tasks.lock().unwrap();
    let mut filtered_tasks = tasks
        .iter()
        .filter(|x| x.username == username)
        .cloned()
        .collect::<Vec<_>>();
    filtered_tasks.sort_by_key(|x| std::cmp::Reverse(x.created_at));

    let list = filtered_tasks
        .into_iter()
        .map(|x| {
            serde_json::json!({
                "id": x.id,
                "title": x.title,
                "description": x.description,
                "risk_level": x.risk_level,
                "task_type": x.task_type,
                "source": x.source,
                "is_completed": x.is_completed,
                "created_at": x.created_at,
                "completed_at": x.completed_at
            })
        })
        .collect::<Vec<_>>();

    api_success(serde_json::Value::Array(list))
}

pub async fn add_task_api_handler(
    State(state): State<AppState>,
    Json(req): Json<AddTaskRequest>,
) -> impl IntoResponse {
    let username = req.username.trim();
    if username.is_empty() {
        return api_response(
            StatusCode::BAD_REQUEST,
            400,
            "username is required",
            serde_json::Value::Null,
        )
        .into_response();
    }
    if !user_exists(&state, username) {
        return api_response(
            StatusCode::NOT_FOUND,
            404,
            "User not found",
            serde_json::Value::Null,
        )
        .into_response();
    }

    let task = TaskRecord {
        id: uuid::Uuid::new_v4().to_string(),
        username: username.to_string(),
        title: req.title,
        description: req.description,
        risk_level: req.risk_level.unwrap_or_else(|| "中风险".to_string()),
        task_type: req.task_type.unwrap_or_else(|| "手动添加".to_string()),
        source: req.source.unwrap_or_else(|| "用户".to_string()),
        is_completed: false,
        created_at: now_millis(),
        completed_at: None,
    };

    state.tasks.lock().unwrap().push(task.clone());

    api_response(
        StatusCode::OK,
        200,
        "Task created successfully",
        serde_json::json!({
            "id": task.id,
            "title": task.title,
            "description": task.description,
            "risk_level": task.risk_level,
            "task_type": task.task_type,
            "source": task.source,
            "is_completed": task.is_completed,
            "created_at": task.created_at,
            "completed_at": task.completed_at
        }),
    )
}

pub async fn complete_task_api_handler(
    State(state): State<AppState>,
    Json(req): Json<CompleteTaskRequest>,
) -> impl IntoResponse {
    let mut tasks = state.tasks.lock().unwrap();
    let task = match tasks.iter_mut().find(|x| x.id == req.task_id) {
        Some(task) => task,
        None => {
            return api_response(
                StatusCode::NOT_FOUND,
                404,
                "Task not found",
                serde_json::Value::Null,
            )
            .into_response();
        }
    };

    task.is_completed = true;
    task.completed_at = Some(now_millis());

    api_response(
        StatusCode::OK,
        200,
        "Task completed successfully",
        serde_json::json!({
            "id": task.id,
            "title": task.title,
            "description": task.description,
            "risk_level": task.risk_level,
            "task_type": task.task_type,
            "source": task.source,
            "is_completed": task.is_completed,
            "created_at": task.created_at,
            "completed_at": task.completed_at
        }),
    )
}

pub async fn generate_task_from_disease_api_handler(
    State(state): State<AppState>,
    Json(req): Json<GenerateDiseaseTaskRequest>,
) -> impl IntoResponse {
    let username = req.username.trim();
    let disease_name = req.disease_name.trim();
    if username.is_empty() || disease_name.is_empty() {
        return api_response(
            StatusCode::BAD_REQUEST,
            400,
            "disease_name and username are required",
            serde_json::Value::Null,
        )
        .into_response();
    }

    if !user_exists(&state, username) {
        return api_response(
            StatusCode::NOT_FOUND,
            404,
            "User not found",
            serde_json::Value::Null,
        )
        .into_response();
    }

    if disease_name == "健康果树" || disease_name == "非果树" {
        return api_response(
            StatusCode::OK,
            200,
            "No task needed for healthy tree",
            serde_json::Value::Null,
        )
        .into_response();
    }

    let task = TaskRecord {
        id: uuid::Uuid::new_v4().to_string(),
        username: username.to_string(),
        title: format!("{}治理", disease_name),
        description: disease_treatment_text(disease_name).to_string(),
        risk_level: "高风险".to_string(),
        task_type: "疾病识别".to_string(),
        source: "自动生成".to_string(),
        is_completed: false,
        created_at: now_millis(),
        completed_at: None,
    };

    state.tasks.lock().unwrap().push(task.clone());

    api_response(
        StatusCode::OK,
        200,
        "Task created successfully",
        serde_json::json!({
            "id": task.id,
            "title": task.title,
            "description": task.description,
            "risk_level": task.risk_level,
            "task_type": task.task_type,
            "source": task.source,
            "is_completed": task.is_completed,
            "created_at": task.created_at,
            "completed_at": task.completed_at
        }),
    )
}

pub async fn generate_task_from_environment_api_handler(
    State(state): State<AppState>,
    Json(req): Json<GenerateEnvironmentTaskRequest>,
) -> impl IntoResponse {
    let username = req.username.trim();
    if username.is_empty() {
        return api_response(
            StatusCode::BAD_REQUEST,
            400,
            "username, temperature and humidity are required",
            serde_json::Value::Null,
        )
        .into_response();
    }

    if !user_exists(&state, username) {
        return api_response(
            StatusCode::NOT_FOUND,
            404,
            "User not found",
            serde_json::Value::Null,
        )
        .into_response();
    }

    state
        .temperature_humidity_records
        .lock()
        .unwrap()
        .push(TemperatureHumiditySample {
            username: Some(username.to_string()),
            timestamp: now_millis(),
            temperature: req.temperature,
            humidity: req.humidity,
        });

    let (risk_level, description) = classify_environment_risk(req.temperature, req.humidity);
    if risk_level == "低风险" {
        return api_response(
            StatusCode::OK,
            200,
            "No task needed for low risk",
            serde_json::Value::Null,
        )
        .into_response();
    }

    let task = TaskRecord {
        id: uuid::Uuid::new_v4().to_string(),
        username: username.to_string(),
        title: format!("环境监测-{}", risk_level),
        description: description.to_string(),
        risk_level: risk_level.to_string(),
        task_type: "温湿度监测".to_string(),
        source: "自动生成".to_string(),
        is_completed: false,
        created_at: now_millis(),
        completed_at: None,
    };

    state.tasks.lock().unwrap().push(task.clone());

    api_response(
        StatusCode::OK,
        200,
        "Task created successfully",
        serde_json::json!({
            "id": task.id,
            "title": task.title,
            "description": task.description,
            "risk_level": task.risk_level,
            "task_type": task.task_type,
            "source": task.source,
            "is_completed": task.is_completed,
            "created_at": task.created_at,
            "completed_at": task.completed_at
        }),
    )
}
