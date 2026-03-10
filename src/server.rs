use crate::models::{DiagnosisRecord, FertilizationPlanRequest};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};

use chrono::{SecondsFormat, TimeZone, Utc};
use serde::Deserialize;
use serde_json::json;
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing_subscriber::EnvFilter;

use crate::client::OpenRouterClient;
use crate::models::ChatApiRequest;
use base64::Engine;

#[derive(Clone)]
pub struct AppState {
    pub client: OpenRouterClient,
    pub inference: crate::inference::InferenceRuntime,
    pub db: PgPool,
}

#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub id: String,
    pub username: String,
    pub title: String,
    pub description: String,
    pub risk_level: String,
    pub task_type: String,
    pub source: String,
    pub is_completed: bool,
    pub created_at: u64,
    pub completed_at: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct TemperatureHumiditySample {
    pub username: Option<String>,
    pub timestamp: u64,
    pub temperature: f64,
    pub humidity: f64,
}

#[derive(Debug, Deserialize)]
pub struct UsernameQuery {
    pub username: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DiseaseTreatmentQuery {
    pub disease_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddTaskRequest {
    pub username: String,
    pub title: String,
    pub description: String,
    pub risk_level: Option<String>,
    pub task_type: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CompleteTaskRequest {
    pub task_id: String,
}

#[derive(Debug, Deserialize)]
pub struct GenerateDiseaseTaskRequest {
    pub disease_name: String,
    pub username: String,
}

#[derive(Debug, Deserialize)]
pub struct GenerateEnvironmentTaskRequest {
    pub username: String,
    pub temperature: f64,
    pub humidity: f64,
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn format_drf_datetime(timestamp_millis: i64) -> String {
    Utc.timestamp_millis_opt(timestamp_millis)
        .single()
        .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Millis, true))
        .unwrap_or_default()
}

const RECOGNITION_RECORDS_MEDIA_PREFIX: &str = "/media/recognition_records";
const RECOGNITION_RECORDS_UPLOAD_DIR: &str = "static/uploads";

fn recognition_record_public_path(file_name: &str) -> String {
    format!(
        "{}/{}",
        RECOGNITION_RECORDS_MEDIA_PREFIX,
        file_name.trim_start_matches('/')
    )
}

fn normalize_recognition_record_image_path(path: Option<&str>) -> Option<String> {
    let trimmed = path?.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Some(trimmed.to_string());
    }

    if trimmed.starts_with("/uploads/")
        || trimmed.starts_with("uploads/")
        || trimmed.starts_with("/media/recognition_records/")
        || trimmed.starts_with("media/recognition_records/")
    {
        return trimmed
            .rsplit('/')
            .next()
            .filter(|file_name| !file_name.is_empty())
            .map(recognition_record_public_path);
    }

    Some(trimmed.to_string())
}

fn save_recognition_record_image(record_id: &str, data_url: &str) -> anyhow::Result<String> {
    let b64 = if let Some(pos) = data_url.find(',') {
        &data_url[pos + 1..]
    } else {
        data_url
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| anyhow::anyhow!("base64解码失败: {}", e))?;
    let file_name = format!("{}.jpg", record_id);
    let full_path = format!("{}/{}", RECOGNITION_RECORDS_UPLOAD_DIR, file_name);

    std::fs::create_dir_all(RECOGNITION_RECORDS_UPLOAD_DIR)
        .map_err(|e| anyhow::anyhow!("创建目录失败: {}", e))?;
    std::fs::write(&full_path, &bytes).map_err(|e| anyhow::anyhow!("写入图片失败: {}", e))?;

    Ok(recognition_record_public_path(&file_name))
}

#[allow(clippy::too_many_arguments)]
fn task_payload(
    id: &str,
    title: &str,
    description: &str,
    risk_level: &str,
    task_type: &str,
    source: &str,
    is_completed: bool,
    created_at: i64,
    completed_at: Option<i64>,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "description": description,
        "risk_level": risk_level,
        "task_type": task_type,
        "source": source,
        "is_completed": is_completed,
        "created_at": format_drf_datetime(created_at),
        "completed_at": completed_at.map(format_drf_datetime)
    })
}

fn api_response(
    status: StatusCode,
    code: u16,
    message: impl Into<String>,
    data: serde_json::Value,
) -> Response {
    (
        status,
        Json(serde_json::json!({
            "code": code,
            "message": message.into(),
            "data": data,
            "timestamp": now_millis()
        })),
    )
        .into_response()
}

fn api_success(data: serde_json::Value) -> Response {
    api_response(StatusCode::OK, 200, "success", data)
}

async fn user_exists(state: &AppState, username: &str) -> bool {
    sqlx::query("SELECT 1 FROM app_users WHERE username = $1 LIMIT 1")
        .bind(username)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .is_some()
}

async fn username_by_token(state: &AppState, token: &str) -> Option<String> {
    sqlx::query("SELECT username FROM app_sessions WHERE token = $1 LIMIT 1")
        .bind(token)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.try_get::<String, _>("username").ok())
}

async fn init_database(pool: &PgPool) -> anyhow::Result<()> {
    let ddl = [
        r#"CREATE TABLE IF NOT EXISTS app_users (
            username TEXT PRIMARY KEY,
            password_hash TEXT NOT NULL,
            is_admin BOOLEAN NOT NULL DEFAULT FALSE,
            created_at BIGINT NOT NULL,
            session_token TEXT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS app_sessions (
            token TEXT PRIMARY KEY,
            username TEXT NOT NULL,
            created_at BIGINT NOT NULL
        )"#,
        r#"CREATE INDEX IF NOT EXISTS idx_app_sessions_username ON app_sessions(username)"#,
        r#"CREATE TABLE IF NOT EXISTS app_invitations (
            code TEXT PRIMARY KEY,
            used BOOLEAN NOT NULL DEFAULT FALSE,
            expires_at BIGINT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS app_pending_users (
            username TEXT PRIMARY KEY,
            password_hash TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            requested_role TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS app_tasks (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL,
            title TEXT NOT NULL,
            description TEXT NOT NULL,
            risk_level TEXT NOT NULL,
            task_type TEXT NOT NULL,
            source TEXT NOT NULL,
            is_completed BOOLEAN NOT NULL DEFAULT FALSE,
            created_at BIGINT NOT NULL,
            completed_at BIGINT NULL
        )"#,
        r#"CREATE INDEX IF NOT EXISTS idx_app_tasks_username_created ON app_tasks(username, created_at DESC)"#,
        r#"CREATE TABLE IF NOT EXISTS app_temperature_humidity (
            id BIGSERIAL PRIMARY KEY,
            username TEXT NULL,
            timestamp BIGINT NOT NULL,
            temperature DOUBLE PRECISION NOT NULL,
            humidity DOUBLE PRECISION NOT NULL
        )"#,
        r#"CREATE INDEX IF NOT EXISTS idx_app_temp_humidity_user_time ON app_temperature_humidity(username, timestamp DESC)"#,
        r#"CREATE TABLE IF NOT EXISTS app_diagnosis_records (
            id TEXT PRIMARY KEY,
            timestamp BIGINT NOT NULL,
            predicted_class TEXT NOT NULL,
            confidence DOUBLE PRECISION NOT NULL,
            is_citrus_leaf BOOLEAN NOT NULL,
            citrus_type TEXT NOT NULL,
            is_healthy BOOLEAN NOT NULL,
            disease_name TEXT NOT NULL,
            severity TEXT NOT NULL,
            treatment_suggestion TEXT NOT NULL,
            preventive_measures TEXT NOT NULL,
            image_quality_warning TEXT NOT NULL,
            username TEXT NULL,
            area TEXT NULL
        )"#,
        r#"CREATE INDEX IF NOT EXISTS idx_app_diag_user_time ON app_diagnosis_records(username, timestamp DESC)"#,
    ];

    for stmt in ddl {
        sqlx::query(stmt)
            .execute(pool)
            .await
            .map_err(|e| anyhow::anyhow!("初始化数据库表失败: {}", e))?;
    }

    // 迁移：为旧库补充 image_path 列
    let _ = sqlx::query(
        "ALTER TABLE app_diagnosis_records ADD COLUMN IF NOT EXISTS image_path TEXT NULL",
    )
    .execute(pool)
    .await;

    // sqlx::query(
    //     "INSERT INTO app_invitations (code, used, expires_at) VALUES ($1, $2, $3) ON CONFLICT (code) DO NOTHING",
    // )
    // .bind("1111")
    // .bind(false)
    // .bind(i64::MAX)
    // .execute(pool)
    // .await
    // .map_err(|e| anyhow::anyhow!("写入默认邀请码失败: {}", e))?;

    Ok(())
}

fn time_label_from_millis(millis: u64) -> String {
    let total_minutes = (millis / 60_000) % (24 * 60);
    let hour = total_minutes / 60;
    let minute = total_minutes % 60;
    format!("{:02}:{:02}", hour, minute)
}

fn build_temp_humidity_payload(samples: &[TemperatureHumiditySample]) -> serde_json::Value {
    let mut temperature_history = Vec::new();
    let mut humidity_history = Vec::new();

    for (index, item) in samples.iter().enumerate() {
        let show_label = index % 2 == 0 || index == samples.len().saturating_sub(1);
        let label = if show_label {
            time_label_from_millis(item.timestamp)
        } else {
            String::new()
        };

        let temp_value = (item.temperature * 10.0).round() / 10.0;
        let humidity_value = (item.humidity * 10.0).round() / 10.0;

        temperature_history.push(serde_json::json!({
            "timestamp": label,
            "value": temp_value
        }));
        humidity_history.push(serde_json::json!({
            "timestamp": if show_label { time_label_from_millis(item.timestamp) } else { String::new() },
            "value": humidity_value
        }));
    }

    let current_temperature = samples
        .last()
        .map(|x| (x.temperature * 10.0).round() / 10.0)
        .unwrap_or(0.0);
    let current_humidity = samples
        .last()
        .map(|x| (x.humidity * 10.0).round() / 10.0)
        .unwrap_or(0.0);

    serde_json::json!({
        "temperatureHistory": temperature_history,
        "humidityHistory": humidity_history,
        "currentTemperature": current_temperature,
        "currentHumidity": current_humidity
    })
}

fn default_temperature_samples() -> Vec<TemperatureHumiditySample> {
    let now = now_millis();
    let temperatures = [24.0, 24.6, 25.1, 25.4, 25.0, 24.8, 24.9, 25.2, 25.5, 25.1];
    let humidities = [68.0, 67.3, 66.8, 67.1, 67.9, 68.2, 67.6, 66.9, 66.5, 66.8];

    temperatures
        .iter()
        .zip(humidities.iter())
        .enumerate()
        .map(|(index, (temp, humidity))| TemperatureHumiditySample {
            username: None,
            timestamp: now
                .saturating_sub(((temperatures.len() - index - 1) as u64) * 10 * 60 * 1000),
            temperature: *temp,
            humidity: *humidity,
        })
        .collect()
}

fn disease_treatment_text(disease: &str) -> &'static str {
    match disease {
        "黄龙病" => {
            "先杀虫，后砍树：发现病树后，先全园喷洒噻虫嗪、联苯菊酯等药剂杀灭柑橘木虱。间隔3-5天后，将病树连根挖除或砍除，并集中烧毁。砍除时需对树蔸做毁蔸处理（如划十字、涂草甘膦、覆土），防止复发。"
        }
        "沙皮病" => {
            "清园+药剂防治：及时剪除并清理果园内的枯死枝条和落叶，减少病菌来源。在谢花期、幼果期等关键时期，可选用苯醚甲环唑、吡唑醚菌酯、代森锰锌等药剂进行喷雾保护。避免果树遭受冻害或日灼，减少伤口。"
        }
        "溃疡病" => {
            "采用药-剪-药策略：首先使用铜制剂（如噻菌铜、春雷·王铜）全面喷雾杀菌。然后彻底剪除病枝、病叶、病果并集中销毁，修剪工具需消毒。修剪完成后，再喷一次杀菌剂进行保护，7-10天后可再施一次。同时注意防治潜叶蛾等虫媒，减少传播伤口。"
        }
        _ => "暂无治理建议",
    }
}

fn risk_from_disease_name(disease_name: &str) -> &'static str {
    if disease_name == "健康果树" || disease_name == "非果树" {
        "正常"
    } else {
        "高风险"
    }
}

fn classify_environment_risk(temperature: f64, humidity: f64) -> (&'static str, &'static str) {
    if (22.0..=32.0).contains(&temperature) && (80.0..=100.0).contains(&humidity) {
        (
            "高风险",
            "加强果园巡查，每7-10天喷施木虱防治药剂（如噻虫嗪、高效氯氟氰菊酯）；发现病树立即标记并挖除，防止传播。",
        )
    } else if ((15.0..=22.0).contains(&temperature) || (32.0..=35.0).contains(&temperature))
        && (60.0..=80.0).contains(&humidity)
    {
        (
            "中风险",
            "定期监测木虱虫口密度，选用高效低毒农药进行预防性喷雾；剪除零星病梢，保持果园通风透光。",
        )
    } else {
        (
            "低风险",
            "利用农闲时段彻底清园，剪除病虫枝，树干涂白；干旱时注意灌溉，增强树势，减少木虱越冬场所。",
        )
    }
}

include!("server/handlers_core.rs");
include!("server/handlers_ai.rs");

pub fn create_router(config: &crate::config::AppConfig, db: PgPool) -> Router {
    let api_key = config.ai.openrouter_api_key.clone();

    let client = OpenRouterClient::new(api_key);
    let inference = crate::inference::InferenceRuntime::new(&config.inference, client.clone());
    let state = AppState {
        client,
        inference,
        db,
    };

    Router::new()
        .route("/health", get(health_handler))
        .route(
            "/",
            get(|| async { axum::response::Redirect::temporary("/index.html") }),
        )
        .route("/api/register", post(crate::user_routes::register_handler))
        .route("/api/login", post(crate::user_routes::login_handler))
        .route("/api/logout", post(crate::user_routes::logout_handler))
        .route(
            "/api/validate",
            post(crate::user_routes::validate_token_handler),
        )
        .route("/api/user", get(api_user_handler))
        .route("/api/home", get(home_api_handler))
        .route("/api/growth-tracking", get(growth_tracking_api_handler))
        .route("/api/diagnose", get(diagnose_api_handler))
        .route(
            "/api/temperature-humidity",
            get(temperature_humidity_api_handler),
        )
        .route("/admin.html", get(admin_page_handler))
        .route("/analyze.html", get(analyze_page_handler))
        .route("/citrus/analyze", post(citrus_analyze_handler))
        .route("/api/citrus-disease", post(citrus_disease_handler))
        .route("/api/citrus-disease-v2", post(citrus_disease_advanced_handler))
        .route(
            "/api/recognition-records",
            get(recognition_records_api_handler),
        )
        .route("/api/disease-treatment", get(disease_treatment_api_handler))
        .route("/api/tasks", get(get_tasks_api_handler))
        .route("/api/tasks/add", post(add_task_api_handler))
        .route("/api/tasks/complete", post(complete_task_api_handler))
        .route(
            "/api/tasks/generate/disease",
            post(generate_task_from_disease_api_handler),
        )
        .route(
            "/api/tasks/generate/environment",
            post(generate_task_from_environment_api_handler),
        )
        .route("/api/generate", get(generate_handler))
        .route(
            "/api/generate/fertilization-plan",
            post(generate_fertilization_plan_handler),
        )
        .nest("/user", crate::user_routes::router(state.clone()))
        .nest_service(
            "/media/recognition_records",
            ServeDir::new(RECOGNITION_RECORDS_UPLOAD_DIR),
        )
        .fallback_service(ServeDir::new("static"))
        .with_state(state)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
}

pub fn init_tracing(log_level: &str) {
    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(log_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .try_init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!("监听 Ctrl+C 失败: {}", err);
        }
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(err) => {
                tracing::error!("监听 SIGTERM 失败: {}", err);
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("收到退出信号，正在关闭服务器...");
}

pub async fn run_server(config: crate::config::AppConfig) -> anyhow::Result<()> {
    let addr: SocketAddr = config
        .server
        .addr
        .parse()
        .map_err(|e| anyhow::anyhow!("解析 server.addr 失败: {}", e))?;
    let db = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database.postgres_url)
        .await
        .map_err(|e| anyhow::anyhow!("连接 PostgreSQL 失败: {}", e))?;
    init_database(&db).await?;

    let app = create_router(&config, db);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("无法绑定到地址");

    tracing::info!("AI 服务 服务器正在监听: {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| anyhow::anyhow!("服务器运行失败: {}", e))?;

    Ok(())
}
