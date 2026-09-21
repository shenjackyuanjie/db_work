//! 网页果园超集：3D 沙盘几何数据 + 识别富字段。
//!
//! 负责的 `/web/*` 路径（外层已 `nest("/web")`，此处写相对路径）：
//!
//! | 路径 | 方法 | 旧路径 | 前端引用 |
//! |---|---|---|---|
//! | `/orchard/overview` | POST | `/user/orchard/overview` | `orchard-3d.js:839` |
//! | `/admin/orchard/overview` | POST | `/user/admin/orchard/overview` | `admin.js:887` |
//! | `/citrus-disease-v2` | POST | `/api/citrus-disease-v2` | `analyze.js:196` |
//!
//! 为什么这三个在最严重缺口里（`db/static/WEB_ENDPOINT_MAP.md`）：3D 沙盘要
//! `trees[].position{x,y}` / `terrain_height` / `tag_serial_number` / `latest_sensor` /
//! `latest_diagnosis` + `coordinate_range` + `weather.*`，而 Django 的 `fruit_tree_archive`
//! **无坐标、无地形高度、无 tag_serial_number，也没有传感器记录表**，天气字段零对应。
//!
//! 表：保留 `app_orchard_trees`、`app_tree_sensor_records`；识别富字段写
//! `web_diagnosis_records`（见 `bootstrap/web_tables.rs`），并**双写**契约表
//! `disease_recognition_record` 供 App 读。
//!
//! # 响应是**裸 JSON**
//!
//! `orchard-3d.js:109-110` 与 `admin.js:10-15` 的 `unwrapApiPayload` 是「有 `data` 就取
//! `data`，否则取整包」，两种都吃；但旧实现是裸的，这里**保持逐字一致**，
//! 免得给 3D 沙盘这种大 payload 引入无谓的包装差异。

use axum::Router;
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

use crate::server::AppState;
use crate::web::session::{current_user, require_admin};

/// 与旧实现一致的坐标范围（`orchard-3d.js:256-259` 用它做归一化）。
const COORDINATE_RANGE: f64 = 500.0;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/orchard/overview", post(public_overview_handler))
        .route("/admin/orchard/overview", post(admin_overview_handler))
}

#[derive(Debug, Deserialize)]
pub(crate) struct OrchardOverviewRequest {
    #[serde(default)]
    pub username: String,
}

fn ok_json(value: Value) -> Response {
    (StatusCode::OK, Json(value)).into_response()
}

fn err_json(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

// --------------------------------------------------------------------------------------
// 天气（旧实现是静态桩，逐字保留）
// --------------------------------------------------------------------------------------

async fn fetch_weather_by_coordinates(latitude: f64, longitude: f64) -> Result<Value, String> {
    Ok(json!({
        "source": "static",
        "latitude": latitude,
        "longitude": longitude,
        "timezone": "Asia/Shanghai",
        "current": {
            "time": "2025-01-01T12:00",
            "temperature": 22.5,
            "temperature_unit": "°C",
            "humidity": 65.0,
            "humidity_unit": "%",
            "wind_speed": 3.2,
            "wind_speed_unit": "km/h",
            "weather_code": 0,
            "weather_text": "晴",
        }
    }))
}

#[derive(Debug)]
enum WeatherLookupError {
    UserNotFound,
    Database(sqlx::Error),
    Fetch(String),
}

/// 按用户名取坐标。**表已从 `app_users` 换成契约表 `"user"`**（`"user"` 是保留字，必须双引号）。
async fn fetch_weather_for_user(
    state: &AppState,
    username: &str,
) -> Result<Option<Value>, WeatherLookupError> {
    let row = sqlx::query(r#"SELECT latitude, longitude FROM "user" WHERE username = $1 LIMIT 1"#)
        .bind(username)
        .fetch_optional(&state.db)
        .await
        .map_err(WeatherLookupError::Database)?;

    let Some(row) = row else {
        return Err(WeatherLookupError::UserNotFound);
    };

    let latitude = row.try_get::<Option<f64>, _>("latitude").unwrap_or(None);
    let longitude = row.try_get::<Option<f64>, _>("longitude").unwrap_or(None);

    let (Some(latitude), Some(longitude)) = (latitude, longitude) else {
        return Ok(None);
    };

    fetch_weather_by_coordinates(latitude, longitude)
        .await
        .map(Some)
        .map_err(WeatherLookupError::Fetch)
}

// --------------------------------------------------------------------------------------
// 状态分级（逐字照抄旧实现，前端按 level/label/color 三键渲染图例）
// --------------------------------------------------------------------------------------

fn orchard_status_from_snapshot(
    health_index: Option<f64>,
    predicted_class: Option<&str>,
) -> (&'static str, &'static str, &'static str) {
    if let Some(label) = predicted_class
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        match label {
            "黄龙病" => return ("critical", "黄龙病预警", "#ef4444"),
            "溃疡病" => return ("warning", "溃疡病预警", "#f59e0b"),
            "沙皮病" => return ("critical", "沙皮病预警", "#8b5cf6"),
            "待人工复核" => return ("attention", "待人工复核", "#38bdf8"),
            _ => {}
        }
    }

    match health_index {
        Some(value) if value >= 0.85 => ("healthy", "健康果树", "#10b981"),
        Some(value) if value >= 0.72 => ("attention", "轻度异常", "#f59e0b"),
        Some(value) if value >= 0.60 => ("warning", "中度异常", "#ef4444"),
        Some(_) => ("critical", "重度异常", "#8b5cf6"),
        None => ("healthy", "健康果树", "#10b981"),
    }
}

fn orchard_status_priority(level: &str) -> i32 {
    match level {
        "healthy" => 0,
        "attention" => 1,
        "warning" => 2,
        "critical" => 3,
        _ => 4,
    }
}

type LegendEntry = (String, String, String, i64);

fn push_orchard_legend(legend: &mut Vec<LegendEntry>, label: &str, level: &str, color: &str) {
    if let Some(item) = legend.iter_mut().find(|item| item.0 == label) {
        item.3 += 1;
        return;
    }

    legend.push((label.to_string(), level.to_string(), color.to_string(), 1));
}

// --------------------------------------------------------------------------------------
// 共享的 payload 构造（旧实现把这段复制了两遍，这里合成一份）
// --------------------------------------------------------------------------------------

/// 每棵树的查询：几何 + 最新传感器读数 + 最新诊断。
///
/// `latest_diagnosis` 读 **`web_diagnosis_records`** —— 网页识别路径
/// （`/api/citrus-disease-v2` 的落库）写的正是这张表，见
/// `src/server/handlers_ai/persistence.rs::store_diagnosis_record` 的双写。
const TREES_SQL: &str = r#"
        SELECT
            t.id,
            t.tree_code,
            t.tag_serial_number,
            t.pos_x,
            t.pos_y,
            t.terrain_height,

            latest_sensor.sampled_at,
            latest_sensor.temperature,
            latest_sensor.humidity,

            latest_diagnosis.predicted_class,
            latest_diagnosis.disease_name,
            latest_diagnosis.diagnosis_timestamp,
            latest_diagnosis.confidence AS diagnosis_confidence
        FROM app_orchard_trees t
        LEFT JOIN LATERAL (
            SELECT sampled_at, temperature, humidity
            FROM app_tree_sensor_records
            WHERE tag_serial_number = t.tag_serial_number
            ORDER BY sampled_at DESC
            LIMIT 1
        ) latest_sensor ON TRUE
        LEFT JOIN LATERAL (
            SELECT predicted_class, disease_name, timestamp AS diagnosis_timestamp, confidence
            FROM web_diagnosis_records
            WHERE area = t.tree_code
            ORDER BY timestamp DESC
            LIMIT 1
        ) latest_diagnosis ON TRUE
        WHERE t.is_active = TRUE
        ORDER BY t.id ASC
        "#;

async fn build_overview(state: &AppState, username: &str) -> Result<Value, Response> {
    let (weather, weather_error) = match fetch_weather_for_user(state, username).await {
        Ok(weather) => (weather, None),
        Err(WeatherLookupError::UserNotFound) => {
            return Err(err_json(StatusCode::NOT_FOUND, "User not found"));
        }
        Err(WeatherLookupError::Database(err)) => {
            return Err(err_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load user coordinates: {err}"),
            ));
        }
        Err(WeatherLookupError::Fetch(err)) => (None, Some(err)),
    };

    let rows = sqlx::query(TREES_SQL)
        .fetch_all(&state.db)
        .await
        .map_err(|err| {
            err_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load orchard overview: {err}"),
            )
        })?;

    let mut trees = Vec::new();
    let mut legend: Vec<LegendEntry> = Vec::new();
    let mut online_trees = 0_i64;
    let mut last_sampled_at = 0_i64;

    for row in rows {
        let tree_id = row.try_get::<i64, _>("id").unwrap_or_default();
        let tree_code = row.try_get::<String, _>("tree_code").unwrap_or_default();
        let tag_serial_number = row
            .try_get::<Option<i64>, _>("tag_serial_number")
            .unwrap_or(None);
        let pos_x = row.try_get::<f64, _>("pos_x").unwrap_or(0.0);
        let pos_y = row.try_get::<f64, _>("pos_y").unwrap_or(0.0);
        let terrain_height = row.try_get::<f64, _>("terrain_height").unwrap_or(0.0);
        let sampled_at = row.try_get::<Option<i64>, _>("sampled_at").unwrap_or(None);

        let predicted_class = row
            .try_get::<Option<String>, _>("predicted_class")
            .unwrap_or(None)
            .filter(|value| !value.trim().is_empty() && value != "健康果树" && value != "非果树");
        let disease_name = row
            .try_get::<Option<String>, _>("disease_name")
            .unwrap_or(None)
            .filter(|value| !value.trim().is_empty());
        let diagnosis_label = disease_name.as_deref().or(predicted_class.as_deref());
        let (status_level, status_label, status_color) =
            orchard_status_from_snapshot(None, diagnosis_label);

        push_orchard_legend(&mut legend, status_label, status_level, status_color);

        if let Some(sampled_at) = sampled_at {
            online_trees += 1;
            last_sampled_at = last_sampled_at.max(sampled_at);
        }

        let latest_sensor = sampled_at.map(|sampled_at| {
            json!({
                "sampled_at": sampled_at,
                "temperature": row.try_get::<f64, _>("temperature").unwrap_or(0.0),
                "humidity": row.try_get::<f64, _>("humidity").unwrap_or(0.0),
            })
        });

        let latest_diagnosis = row
            .try_get::<Option<i64>, _>("diagnosis_timestamp")
            .unwrap_or(None)
            .map(|timestamp| {
                json!({
                    "timestamp": timestamp,
                    "predicted_class": predicted_class,
                    "disease_name": disease_name,
                    "confidence": row
                        .try_get::<Option<f64>, _>("diagnosis_confidence")
                        .unwrap_or(None),
                })
            });

        trees.push(json!({
            "id": tree_id,
            "tree_code": tree_code,
            "tag_serial_number": tag_serial_number,
            "position": { "x": pos_x, "y": pos_y },
            "terrain_height": terrain_height,
            "status": {
                "level": status_level,
                "label": status_label,
                "color": status_color,
            },
            "latest_sensor": latest_sensor,
            "latest_diagnosis": latest_diagnosis,
        }));
    }

    legend.sort_by(|left, right| {
        orchard_status_priority(&left.1)
            .cmp(&orchard_status_priority(&right.1))
            .then(left.0.cmp(&right.0))
    });

    Ok(json!({
        "requested_username": username,
        "coordinate_range": {
            "min_x": 0,
            "max_x": COORDINATE_RANGE,
            "min_y": 0,
            "max_y": COORDINATE_RANGE,
        },
        "summary": {
            "total_trees": trees.len(),
            "online_trees": online_trees,
            "last_sampled_at": if last_sampled_at > 0 { Some(last_sampled_at) } else { None::<i64> },
        },
        "legend": legend
            .into_iter()
            .map(|(label, level, color, count)| json!({
                "label": label,
                "level": level,
                "color": color,
                "count": count,
            }))
            .collect::<Vec<_>>(),
        "weather": weather,
        "weather_error": weather_error,
        "trees": trees,
    }))
}

// --------------------------------------------------------------------------------------
// 处理器
// --------------------------------------------------------------------------------------

/// `POST /web/orchard/overview`：**任何已登录用户**都能看自己园区的沙盘
/// （旧 `/user/orchard/overview` 用 `ensure_authenticated`，不是管理员专属）。
pub(crate) async fn public_overview_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = current_user(&state, &headers).await else {
        return err_json(StatusCode::UNAUTHORIZED, "请先登录");
    };

    tracing::info!("网页用户 {} 请求园区 3D 沙盘数据", user.username);

    match build_overview(&state, &user.username).await {
        Ok(payload) => ok_json(payload),
        Err(response) => response,
    }
}

/// `POST /web/admin/orchard/overview`：管理员可**指定查看某个用户**的园区。
pub(crate) async fn admin_overview_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<OrchardOverviewRequest>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let username = payload.username.trim().to_string();
    if username.is_empty() {
        return err_json(StatusCode::BAD_REQUEST, "username is required");
    }

    match build_overview(&state, &username).await {
        Ok(payload) => ok_json(payload),
        Err(response) => response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_levels_match_legacy_labels_and_colors() {
        assert_eq!(
            orchard_status_from_snapshot(None, Some("黄龙病")),
            ("critical", "黄龙病预警", "#ef4444")
        );
        assert_eq!(
            orchard_status_from_snapshot(None, Some("溃疡病")),
            ("warning", "溃疡病预警", "#f59e0b")
        );
        assert_eq!(
            orchard_status_from_snapshot(None, None),
            ("healthy", "健康果树", "#10b981")
        );
    }

    /// 按健康度分档（前端图例顺序依赖 priority）。
    #[test]
    fn health_index_falls_into_buckets() {
        assert_eq!(orchard_status_from_snapshot(Some(0.9), None).0, "healthy");
        assert_eq!(orchard_status_from_snapshot(Some(0.8), None).0, "attention");
        assert_eq!(orchard_status_from_snapshot(Some(0.65), None).0, "warning");
        assert_eq!(orchard_status_from_snapshot(Some(0.4), None).0, "critical");
    }

    #[test]
    fn legend_priority_sorts_healthy_first() {
        let mut levels = ["critical", "healthy", "warning", "attention"];
        levels.sort_by_key(|level| orchard_status_priority(level));
        assert_eq!(levels, ["healthy", "attention", "warning", "critical"]);
    }

    #[test]
    fn legend_merges_same_label_and_counts() {
        let mut legend: Vec<LegendEntry> = Vec::new();
        push_orchard_legend(&mut legend, "健康果树", "healthy", "#10b981");
        push_orchard_legend(&mut legend, "健康果树", "healthy", "#10b981");
        push_orchard_legend(&mut legend, "溃疡病预警", "warning", "#f59e0b");

        assert_eq!(legend.len(), 2);
        assert_eq!(legend[0].3, 2);
        assert_eq!(legend[1].3, 1);
    }

    /// 错误体是裸 JSON 且用 `error` 键（`orchard-3d.js:842` 读 `response.data?.error`）。
    #[tokio::test]
    async fn error_body_uses_bare_error_key() {
        let response = err_json(StatusCode::NOT_FOUND, "User not found");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["error"], "User not found");
        assert!(value.get("data").is_none(), "{value}");
    }

    /// 坐标范围与旧的 0..500 一致（前端用它缩放地形）。
    #[test]
    fn coordinate_range_is_five_hundred() {
        assert_eq!(COORDINATE_RANGE, 500.0);
    }
}
