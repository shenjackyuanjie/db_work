use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
};
use serde_json::json;
use sqlx::Row;

use super::{
    AddTaskRequest, AppState, CompleteTaskRequest, DiseaseTreatmentQuery,
    GenerateDiseaseTaskRequest, GenerateEnvironmentTaskRequest, TaskRecord,
    TemperatureHumiditySample, UsernameQuery, api_response, api_success,
    build_temp_humidity_payload, default_temperature_samples,
    classify_environment_risk, disease_treatment_text, normalize_recognition_record_image_path,
    now_millis, risk_from_disease_name, task_payload, user_exists, username_by_token,
};

pub async fn health_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "service": "openrouter"
        })),
    )
}

async fn has_valid_session(state: &AppState, headers: &HeaderMap) -> bool {
    let token = match crate::user_routes::extract_auth_token(headers) {
        Some(t) => t,
        None => return false,
    };
    username_by_token(state, &token).await.is_some()
}

pub async fn admin_page_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !has_valid_session(&state, &headers).await {
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
    if !has_valid_session(&state, &headers).await {
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
    if has_valid_session(&state, &headers).await {
        return crate::user_routes::me_handler(State(state), headers)
            .await
            .into_response();
    }

    let row = sqlx::query("SELECT username, created_at FROM app_users ORDER BY created_at ASC LIMIT 1")
        .fetch_optional(&state.db)
        .await;

    match row {
        Ok(Some(user)) => api_success(serde_json::json!({
            "id": user.try_get::<String, _>("username").unwrap_or_default(),
            "username": user.try_get::<String, _>("username").unwrap_or_default(),
            "email": null,
            "orchard_address": null,
            "latitude": null,
            "longitude": null,
            "created_at": user.try_get::<i64, _>("created_at").unwrap_or(0)
        }))
        .into_response(),
        Ok(None) => api_response(
            StatusCode::NOT_FOUND,
            404,
            "User not found",
            serde_json::Value::Null,
        )
        .into_response(),
        Err(e) => api_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            500,
            format!("db error: {}", e),
            serde_json::Value::Null,
        )
        .into_response(),
    }
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
    let rows = if let Some(username) = query.username.as_ref().map(|x| x.trim()).filter(|x| !x.is_empty()) {
        sqlx::query(
            "SELECT username, timestamp, temperature, humidity FROM app_temperature_humidity WHERE username = $1 ORDER BY timestamp DESC LIMIT 10",
        )
        .bind(username)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query(
            "SELECT username, timestamp, temperature, humidity FROM app_temperature_humidity ORDER BY timestamp DESC LIMIT 10",
        )
        .fetch_all(&state.db)
        .await
    };

    let mut samples = match rows {
        Ok(rows) => rows
            .into_iter()
            .rev()
            .map(|item| TemperatureHumiditySample {
                username: item.try_get::<Option<String>, _>("username").unwrap_or(None),
                timestamp: item.try_get::<i64, _>("timestamp").unwrap_or(0).max(0) as u64,
                temperature: item.try_get::<f64, _>("temperature").unwrap_or(0.0),
                humidity: item.try_get::<f64, _>("humidity").unwrap_or(0.0),
            })
            .collect::<Vec<_>>(),
        Err(e) => {
            return api_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                500,
                format!("db error: {}", e),
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

    let rows = sqlx::query(
        "SELECT id, predicted_class, area, confidence, timestamp, image_path FROM app_diagnosis_records WHERE username = $1 ORDER BY timestamp DESC LIMIT 20",
    )
    .bind(username.unwrap_or_default())
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(rows) => {
            let mut list = rows
                .into_iter()
                .map(|record| {
                    let predicted_class = record.try_get::<String, _>("predicted_class").unwrap_or_default();
                    let image_path = record
                        .try_get::<Option<String>, _>("image_path")
                        .unwrap_or(None);
                    serde_json::json!({
                        "id": record.try_get::<String, _>("id").unwrap_or_default(),
                        "imagePath": normalize_recognition_record_image_path(image_path.as_deref()).unwrap_or_default(),
                        "diseaseName": predicted_class,
                        "area": record.try_get::<Option<String>, _>("area").unwrap_or(None).unwrap_or_else(|| "未指定区域".to_string()),
                        "riskLevel": risk_from_disease_name(&record.try_get::<String, _>("predicted_class").unwrap_or_default()),
                        "recognitionDate": "2026-03-04",
                        "confidence": record.try_get::<f64, _>("confidence").unwrap_or(0.0),
                        "created_at": record.try_get::<i64, _>("timestamp").unwrap_or(0)
                    })
                })
                .collect::<Vec<_>>();
            list.reverse();
            api_success(serde_json::json!({ "records": list }))
        }
        Err(e) => api_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            500,
            format!("db error: {}", e),
            serde_json::Value::Null,
        )
        .into_response(),
    }
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

    if !user_exists(&state, username).await {
        return api_response(
            StatusCode::NOT_FOUND,
            404,
            "User not found",
            serde_json::Value::Null,
        )
        .into_response();
    }

    let rows = sqlx::query(
        "SELECT id, title, description, risk_level, task_type, source, is_completed, created_at, completed_at FROM app_tasks WHERE username = $1 ORDER BY created_at DESC",
    )
    .bind(username)
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(rows) => {
            let list = rows
                .into_iter()
                .map(|x| {
                    task_payload(
                        &x.try_get::<String, _>("id").unwrap_or_default(),
                        &x.try_get::<String, _>("title").unwrap_or_default(),
                        &x.try_get::<String, _>("description").unwrap_or_default(),
                        &x.try_get::<String, _>("risk_level").unwrap_or_default(),
                        &x.try_get::<String, _>("task_type").unwrap_or_default(),
                        &x.try_get::<String, _>("source").unwrap_or_default(),
                        x.try_get::<bool, _>("is_completed").unwrap_or(false),
                        x.try_get::<i64, _>("created_at").unwrap_or(0),
                        x.try_get::<Option<i64>, _>("completed_at").unwrap_or(None),
                    )
                })
                .collect::<Vec<_>>();
            tracing::debug!("查询到 {} 的 {} 条任务记录", username, list.len());
            tracing::debug!("任务列表: {:#?}", list);
            api_success(serde_json::Value::Array(list))
        }
        Err(e) => api_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            500,
            format!("db error: {}", e),
            serde_json::Value::Null,
        )
        .into_response(),
    }
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
    if !user_exists(&state, username).await {
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

    let result = sqlx::query(
        "INSERT INTO app_tasks (id, username, title, description, risk_level, task_type, source, is_completed, created_at, completed_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(&task.id)
    .bind(&task.username)
    .bind(&task.title)
    .bind(&task.description)
    .bind(&task.risk_level)
    .bind(&task.task_type)
    .bind(&task.source)
    .bind(task.is_completed)
    .bind(task.created_at as i64)
    .bind(task.completed_at.map(|x| x as i64))
    .execute(&state.db)
    .await;

    if let Err(e) = result {
        return api_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            500,
            format!("db error: {}", e),
            serde_json::Value::Null,
        )
        .into_response();
    }

    api_response(
        StatusCode::OK,
        200,
        "Task created successfully",
        task_payload(
            &task.id,
            &task.title,
            &task.description,
            &task.risk_level,
            &task.task_type,
            &task.source,
            task.is_completed,
            task.created_at as i64,
            task.completed_at.map(|x| x as i64),
        ),
    )
}

pub async fn complete_task_api_handler(
    State(state): State<AppState>,
    Json(req): Json<CompleteTaskRequest>,
) -> impl IntoResponse {
    let completed_at = now_millis() as i64;

    let updated = sqlx::query(
        "UPDATE app_tasks SET is_completed = TRUE, completed_at = $1 WHERE id = $2",
    )
    .bind(completed_at)
    .bind(&req.task_id)
    .execute(&state.db)
    .await;

    match updated {
        Ok(result) if result.rows_affected() > 0 => {
            let row = sqlx::query(
                "SELECT id, title, description, risk_level, task_type, source, is_completed, created_at, completed_at FROM app_tasks WHERE id = $1 LIMIT 1",
            )
            .bind(&req.task_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

            if let Some(task) = row {
                return api_response(
                    StatusCode::OK,
                    200,
                    "Task completed successfully",
                    task_payload(
                        &task.try_get::<String, _>("id").unwrap_or_default(),
                        &task.try_get::<String, _>("title").unwrap_or_default(),
                        &task.try_get::<String, _>("description").unwrap_or_default(),
                        &task.try_get::<String, _>("risk_level").unwrap_or_default(),
                        &task.try_get::<String, _>("task_type").unwrap_or_default(),
                        &task.try_get::<String, _>("source").unwrap_or_default(),
                        task.try_get::<bool, _>("is_completed").unwrap_or(true),
                        task.try_get::<i64, _>("created_at").unwrap_or(0),
                        task.try_get::<Option<i64>, _>("completed_at").unwrap_or(None),
                    ),
                )
                .into_response();
            }

            api_response(
                StatusCode::OK,
                200,
                "Task completed successfully",
                serde_json::Value::Null,
            )
            .into_response()
        }
        Ok(_) => api_response(
            StatusCode::NOT_FOUND,
            404,
            "Task not found",
            serde_json::Value::Null,
        )
        .into_response(),
        Err(e) => api_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            500,
            format!("db error: {}", e),
            serde_json::Value::Null,
        )
        .into_response(),
    }
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

    if !user_exists(&state, username).await {
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

    let result = sqlx::query(
        "INSERT INTO app_tasks (id, username, title, description, risk_level, task_type, source, is_completed, created_at, completed_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(&task.id)
    .bind(&task.username)
    .bind(&task.title)
    .bind(&task.description)
    .bind(&task.risk_level)
    .bind(&task.task_type)
    .bind(&task.source)
    .bind(task.is_completed)
    .bind(task.created_at as i64)
    .bind(task.completed_at.map(|x| x as i64))
    .execute(&state.db)
    .await;

    if let Err(e) = result {
        return api_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            500,
            format!("db error: {}", e),
            serde_json::Value::Null,
        )
        .into_response();
    }

    api_response(
        StatusCode::OK,
        200,
        "Task created successfully",
        task_payload(
            &task.id,
            &task.title,
            &task.description,
            &task.risk_level,
            &task.task_type,
            &task.source,
            task.is_completed,
            task.created_at as i64,
            task.completed_at.map(|x| x as i64),
        ),
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

    if !user_exists(&state, username).await {
        return api_response(
            StatusCode::NOT_FOUND,
            404,
            "User not found",
            serde_json::Value::Null,
        )
        .into_response();
    }

    let _ = sqlx::query(
        "INSERT INTO app_temperature_humidity (username, timestamp, temperature, humidity) VALUES ($1, $2, $3, $4)",
    )
    .bind(username)
    .bind(now_millis() as i64)
    .bind(req.temperature)
    .bind(req.humidity)
    .execute(&state.db)
    .await;

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

    if let Err(e) = sqlx::query(
        "INSERT INTO app_tasks (id, username, title, description, risk_level, task_type, source, is_completed, created_at, completed_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(&task.id)
    .bind(&task.username)
    .bind(&task.title)
    .bind(&task.description)
    .bind(&task.risk_level)
    .bind(&task.task_type)
    .bind(&task.source)
    .bind(task.is_completed)
    .bind(task.created_at as i64)
    .bind(task.completed_at.map(|x| x as i64))
    .execute(&state.db)
    .await
    {
        return api_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            500,
            format!("db error: {}", e),
            serde_json::Value::Null,
        )
        .into_response();
    }

    api_response(
        StatusCode::OK,
        200,
        "Task created successfully",
        task_payload(
            &task.id,
            &task.title,
            &task.description,
            &task.risk_level,
            &task.task_type,
            &task.source,
            task.is_completed,
            task.created_at as i64,
            task.completed_at.map(|x| x as i64),
        ),
    )
}
