use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use base64::Engine;
use chrono::{SecondsFormat, TimeZone, Utc};
use serde::Deserialize;
use sqlx::{PgPool, Row};

use crate::client::OpenRouterClient;

pub(crate) const RECOGNITION_RECORDS_UPLOAD_DIR: &str = "static/uploads";
const RECOGNITION_RECORDS_MEDIA_PREFIX: &str = "/media/recognition_records";

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
    pub username: String,
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

#[derive(Debug, Deserialize)]
pub struct TagTemperatureHumidityRequest {
    pub username: String,
    pub temperature: f64,
    pub humidity: f64,
    pub tag_serial_number: i64,
    pub record_time: String,
}

pub(crate) fn now_millis() -> u64 {
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

fn recognition_record_public_path(file_name: &str) -> String {
    format!(
        "{}/{}",
        RECOGNITION_RECORDS_MEDIA_PREFIX,
        file_name.trim_start_matches('/')
    )
}

pub(crate) fn normalize_recognition_record_image_path(path: Option<&str>) -> Option<String> {
    let trimmed = path?.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Some(trimmed.to_string());
    }

    if trimmed.starts_with('/') {
        return Some(format!("http://shenjack.top:11000{}", trimmed));
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

pub(crate) fn save_recognition_record_image(
    record_id: &str,
    data_url: &str,
) -> anyhow::Result<String> {
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
pub(crate) fn task_payload(
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

pub(crate) fn api_response(
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

pub(crate) fn api_success(data: serde_json::Value) -> Response {
    api_response(StatusCode::OK, 200, "success", data)
}

pub(crate) async fn user_exists(state: &AppState, username: &str) -> bool {
    sqlx::query("SELECT 1 FROM app_users WHERE username = $1 LIMIT 1")
        .bind(username)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .is_some()
}

pub(crate) async fn username_by_token(state: &AppState, token: &str) -> Option<String> {
    sqlx::query("SELECT username FROM app_sessions WHERE token = $1 LIMIT 1")
        .bind(token)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.try_get::<String, _>("username").ok())
}

fn time_label_from_millis(millis: u64) -> String {
    let total_minutes = (millis / 60_000) % (24 * 60);
    let hour = total_minutes / 60;
    let minute = total_minutes % 60;
    format!("{:02}:{:02}", hour, minute)
}

pub(crate) fn build_temp_humidity_payload(
    samples: &[TemperatureHumiditySample],
) -> serde_json::Value {
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

pub(crate) fn default_temperature_samples() -> Vec<TemperatureHumiditySample> {
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

pub(crate) fn disease_treatment_text(disease: &str) -> &'static str {
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

pub(crate) fn risk_from_disease_name(disease_name: &str) -> &'static str {
    if disease_name == "健康果树" || disease_name == "非果树" {
        "正常"
    } else {
        "高风险"
    }
}

pub(crate) fn classify_environment_risk(
    temperature: f64,
    humidity: f64,
) -> (&'static str, &'static str) {
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
