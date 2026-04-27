use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use sqlx::Row;

use super::super::{
    AddTaskRequest, AppState, CompleteTaskRequest, GenerateDiseaseTaskRequest,
    GenerateEnvironmentTaskRequest, TaskRecord, UsernameQuery, api_response, api_success,
    classify_environment_risk, disease_treatment_text, now_millis, task_payload, user_exists,
};

pub(crate) async fn get_tasks_api_handler(
    State(state): State<AppState>,
    Query(query): Query<UsernameQuery>,
) -> Response {
    let username = match Some(query.username.as_str())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
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
                .map(|row| {
                    task_payload(
                        &row.try_get::<String, _>("id").unwrap_or_default(),
                        &row.try_get::<String, _>("title").unwrap_or_default(),
                        &row.try_get::<String, _>("description").unwrap_or_default(),
                        &row.try_get::<String, _>("risk_level").unwrap_or_default(),
                        &row.try_get::<String, _>("task_type").unwrap_or_default(),
                        &row.try_get::<String, _>("source").unwrap_or_default(),
                        row.try_get::<bool, _>("is_completed").unwrap_or(false),
                        row.try_get::<i64, _>("created_at").unwrap_or(0),
                        row.try_get::<Option<i64>, _>("completed_at").unwrap_or(None),
                    )
                })
                .collect::<Vec<_>>();
            tracing::debug!("查询到 {} 的 {} 条任务记录", username, list.len());
            tracing::debug!("任务列表: {:#?}", list);
            api_success(serde_json::Value::Array(list))
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

pub(crate) async fn add_task_api_handler(
    State(state): State<AppState>,
    Json(request): Json<AddTaskRequest>,
) -> Response {
    let username = request.username.trim();
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
        title: request.title,
        description: request.description,
        risk_level: request.risk_level.unwrap_or_else(|| "中风险".to_string()),
        task_type: request.task_type.unwrap_or_else(|| "手动添加".to_string()),
        source: request.source.unwrap_or_else(|| "用户".to_string()),
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
    .bind(task.completed_at.map(|value| value as i64))
    .execute(&state.db)
    .await;

    if let Err(err) = result {
        return api_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            500,
            format!("db error: {}", err),
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
            task.completed_at.map(|value| value as i64),
        ),
    )
}

pub(crate) async fn complete_task_api_handler(
    State(state): State<AppState>,
    Json(request): Json<CompleteTaskRequest>,
) -> Response {
    let completed_at = now_millis() as i64;

    let updated = sqlx::query(
        "UPDATE app_tasks SET is_completed = TRUE, completed_at = $1 WHERE id = $2",
    )
    .bind(completed_at)
    .bind(&request.task_id)
    .execute(&state.db)
    .await;

    match updated {
        Ok(result) if result.rows_affected() > 0 => {
            let row = sqlx::query(
                "SELECT id, title, description, risk_level, task_type, source, is_completed, created_at, completed_at FROM app_tasks WHERE id = $1 LIMIT 1",
            )
            .bind(&request.task_id)
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
        Err(err) => api_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            500,
            format!("db error: {}", err),
            serde_json::Value::Null,
        )
        .into_response(),
    }
}

pub(crate) async fn generate_task_from_disease_api_handler(
    State(state): State<AppState>,
    Json(request): Json<GenerateDiseaseTaskRequest>,
) -> Response {
    let username = request.username.trim();
    let disease_name = request.disease_name.trim();
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
    .bind(task.completed_at.map(|value| value as i64))
    .execute(&state.db)
    .await;

    if let Err(err) = result {
        return api_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            500,
            format!("db error: {}", err),
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
            task.completed_at.map(|value| value as i64),
        ),
    )
}

pub(crate) async fn generate_task_from_environment_api_handler(
    State(state): State<AppState>,
    Json(request): Json<GenerateEnvironmentTaskRequest>,
) -> Response {
    let username = request.username.trim();
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
    .bind(request.temperature)
    .bind(request.humidity)
    .execute(&state.db)
    .await;

    let (risk_level, description) = classify_environment_risk(request.temperature, request.humidity);
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

    if let Err(err) = sqlx::query(
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
    .bind(task.completed_at.map(|value| value as i64))
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
            task.completed_at.map(|value| value as i64),
        ),
    )
}