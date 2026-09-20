//! 农事与识别域契约实现。
//!
//! 对应 `navel_backend_git/api/urls.py` 中 core 域 **14 条 path**，蓝本为 `api/views.py`
//! 与 `api/serializers.py`。契约基准见 `tests/fixtures/contract/core.json`（42 条用例）。
//!
//! 逐字复刻的几处关键点（都踩过）：
//!
//! 1. **两条 500 路径形状不同**：`views.py` 里 `except Exception -> create_response(None, str(e), 500)`
//!    走的是**成功体形状**（带 `timestamp`）。本域唯一命中该分支的是
//!    [`fertilization_plan_impl`]（`DEVIATIONS.md` **D2**：蓝本漏 import 导致恒 500），
//!    按裁定修正为设计意图的正常语义，**不复刻 5xx**。
//! 2. **`completed_at` 无时区偏移**（`DEVIATIONS.md` **D3**）：蓝本用 `datetime.now()`
//!    写库，序列化出 `2026-09-20T22:20:40.204291` 这种「本地时间、无偏移」的串。
//!    用 `ser::dt_naive_local` 复刻，**不要**顺手补 `+00:00`。
//! 3. **时间后缀两种形态并存**（`DEVIATIONS.md` **D4**）：走 DRF `JSONEncoder` 的字段是
//!    `Z`（`created_at` 系），走 `DateTimeField.to_representation` 的是 `+00:00`
//!    （温湿度 `record_time`）。按字段逐个对齐夹具实测值。
//! 4. **`DecimalField` 才是字符串**；本域的数值列全是 `FloatField`/`IntegerField`，
//!    一律输出 JSON 数字，不要套 `ser::dec`。
//! 5. **列表截断照抄蓝本**：温湿度 `[:10]`、识别记录 `[:20]`、任务按 `-created_at`。

use axum::{
    Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, NaiveDate, Utc};
use rand_core::{OsRng, RngCore};
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::server::AppState;

use super::{
    auth::{self, AuthUser},
    dto::Input,
    errors::{ApiReject, ApiResult, api_ok, api_ok_message, api_response},
    ser,
};

// --------------------------------------------------------------------------------------
// 文案与常量（逐字取自 `REPORT.md §5.2` 与 `api/views.py`，不要改标点）
// --------------------------------------------------------------------------------------

const ERR_REQUIRED: &str = "该字段是必填项。";
const ERR_NULL: &str = "该字段不能为 null。";
const ERR_INVALID_NUMBER: &str = "请填写合法的数字。";
const ERR_INVALID_STRING: &str = "请填写合法的字符串。";

const ERR_DISEASE_NAME_PARAM_REQUIRED: &str = "disease_name parameter is required";
const ERR_TASK_ID_REQUIRED: &str = "task_id is required";
const ERR_TASK_NOT_FOUND: &str = "Task not found";
const ERR_DISEASE_NAME_REQUIRED: &str = "disease_name is required";
const ERR_ENVIRONMENT_PARAMS_REQUIRED: &str = "temperature and humidity are required";
const ERR_NO_IMAGE: &str = "No image provided";
const ERR_TEMP_HUMIDITY_NOT_NUMBERS: &str = "temperature and humidity must be numbers";
const ERR_TEMP_HUMIDITY_OUT_OF_RANGE: &str =
    "temperature or humidity is outside the supported range";
const ERR_RECORD_TIME_ISO: &str = "record_time must be ISO 8601";
const ERR_TEMP_FILE_NOT_FOUND: &str = "Temperature data file not found";
const ERR_TEMP_FILE_EMPTY: &str = "Temperature data file is empty";
const ERR_FERTILIZATION_INVALID: &str = "Invalid request data";

const MSG_TASK_CREATED: &str = "Task created successfully";
const MSG_TASK_COMPLETED: &str = "Task completed successfully";
const MSG_NO_TASK_HEALTHY: &str = "No task needed for healthy tree";
const MSG_NO_TASK_LOW_RISK: &str = "No task needed for low risk";
const MSG_TEMP_SAVED: &str = "Temperature and humidity data saved";

const UNKNOWN_TREATMENT: &str = "暂无治理建议";

/// 蓝本 `citrus_disease_api` 里判定「是柑橘」的两类标签。
const HEALTHY_TREE: &str = "健康果树";
const NON_TREE: &str = "非果树";

/// 温湿度无记录时兜底读取的 CSV。
///
/// Django 用的是 `os.path.dirname(dirname(dirname(views.py)))`，即**项目根**的上一级
/// （`D:\githubs\db_work\temperature_humidity_data.csv`）。Rust 侧沿用仓库根相对路径。
const TEMPERATURE_CSV_PATH: &str = "temperature_humidity_data.csv";

/// 一条温湿度记录（`temperature_humidity_data` 的查询投影）。
pub(crate) struct TempRecord {
    pub(crate) timestamp: DateTime<Utc>,
    pub(crate) temperature: f64,
    pub(crate) humidity: f64,
    pub(crate) node_id: String,
}

/// 识别记录（`disease_recognition_record` 的查询投影）。
struct RecognitionRecord {
    id: Uuid,
    image: String,
    disease_name: String,
    area: String,
    risk_level: String,
    recognition_date: NaiveDate,
    confidence: f64,
    created_at: DateTime<Utc>,
}

// --------------------------------------------------------------------------------------
// 词典（逐字复制 `api/views.py` 的 DISEASE_TREATMENTS / TEMP_HUMIDITY_RISKS）
// --------------------------------------------------------------------------------------

/// 疾病 → 治理建议。条目序即蓝本声明序。
const DISEASE_TREATMENTS: [(&str, &str); 3] = [
    (
        "黄龙病",
        "先杀虫，后砍树：发现病树后，先全园喷洒噻虫嗪、联苯菊酯等药剂杀灭柑橘木虱。间隔3-5天后，将病树连根挖除或砍除，并集中烧毁。砍除时需对树蔸做毁蔸处理（如划十字、涂草甘膦、覆土），防止复发。",
    ),
    (
        "沙皮病",
        "清园+药剂防治：及时剪除并清理果园内的枯死枝条和落叶，减少病菌来源。在谢花期、幼果期等关键时期，可选用苯醚甲环唑、吡唑醚菌酯、代森锰锌等药剂进行喷雾保护。避免果树遭受冻害或日灼，减少伤口。",
    ),
    (
        "溃疡病",
        "采用\"药-剪-药\"策略：首先使用铜制剂（如噻菌铜、春雷·王铜）全面喷雾杀菌。然后彻底剪除病枝、病叶、病果并集中销毁，修剪工具需消毒。修剪完成后，再喷一次杀菌剂进行保护，7-10天后可再施一次。同时注意防治潜叶蛾等虫媒，减少传播伤口。",
    ),
];

const RISK_HIGH: &str = "高风险";
const RISK_MID: &str = "中风险";
const RISK_LOW: &str = "低风险";

/// `TEMP_HUMIDITY_RISKS` 的三个描述文案。区间常量在 [`classify_environment_risk`] 里。
const RISK_HIGH_DESCRIPTION: &str = "加强果园巡查，每7-10天喷施木虱防治药剂（如噻虫嗪、高效氯氟氰菊酯）；发现病树立即标记并挖除，防止传播。";
const RISK_MID_DESCRIPTION: &str =
    "定期监测木虱虫口密度，选用高效低毒农药进行预防性喷雾；剪除零星病梢，保持果园通风透光。";
const RISK_LOW_DESCRIPTION: &str =
    "利用农闲时段彻底清园，剪除病虫枝，树干涂白；干旱时注意灌溉，增强树势，减少木虱越冬场所。";

/// `DISEASE_TREATMENTS.get(name, '暂无治理建议')`。
pub(crate) fn disease_treatment(disease_name: &str) -> &'static str {
    DISEASE_TREATMENTS
        .iter()
        .find(|(name, _)| *name == disease_name)
        .map(|(_, treatment)| *treatment)
        .unwrap_or(UNKNOWN_TREATMENT)
}

/// `TEMP_HUMIDITY_RISKS` 的分级规则**逐字复刻**。
///
/// 顺序敏感：先查高风险，再查中风险的**多个温度区间**，都不中才落到低风险。
/// 蓝本用 `temp_min <= temp <= temp_max and hum_min <= hum <= hum_max`，即**闭区间**。
pub(crate) fn classify_environment_risk(
    temperature: f64,
    humidity: f64,
) -> (&'static str, &'static str) {
    if in_range(temperature, 22.0, 32.0) && in_range(humidity, 80.0, 100.0) {
        return (RISK_HIGH, RISK_HIGH_DESCRIPTION);
    }

    if in_range(humidity, 60.0, 80.0)
        && (in_range(temperature, 15.0, 22.0) || in_range(temperature, 32.0, 35.0))
    {
        return (RISK_MID, RISK_MID_DESCRIPTION);
    }

    (RISK_LOW, RISK_LOW_DESCRIPTION)
}

/// Python 的 `min <= x <= max`。
fn in_range(value: f64, min: f64, max: f64) -> bool {
    value >= min && value <= max
}

// --------------------------------------------------------------------------------------
// 路由（外层已 nest("/compat")）
// --------------------------------------------------------------------------------------

/// core 域 14 条 path。每条都挂 `MethodRouter::fallback` 做 405 收口：
/// **不要**改成 `Router::method_not_allowed_fallback`（`Router::merge` 遇路径级 fallback 会 panic）。
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/home",
            get(home_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/growth-tracking",
            get(growth_tracking_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/diagnose",
            get(diagnose_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/temperature-humidity",
            get(temperature_humidity_handler)
                .post(post_temperature_humidity_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/generate",
            get(generate_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/generate/fertilization-plan",
            post(fertilization_plan_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/citrus-disease",
            post(citrus_disease_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/recognition-records",
            get(recognition_records_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/disease-treatment",
            get(disease_treatment_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/tasks",
            get(tasks_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/tasks/add",
            post(add_task_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/tasks/complete",
            post(complete_task_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/tasks/generate/disease",
            post(generate_task_from_disease_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/tasks/generate/environment",
            post(generate_task_from_environment_handler).fallback(handle_unallowed_method),
        )
}

/// 405：DRF 异常体 + zh-hans 文案 `方法 “DELETE” 不被允许。`
async fn handle_unallowed_method(method: Method) -> Response {
    auth::render_method_not_allowed(&method)
}

// --------------------------------------------------------------------------------------
// 薄包装：只做「取 state / 取 body」，逻辑全在 `*_impl`
// --------------------------------------------------------------------------------------

async fn home_handler(State(state): State<AppState>, request: Request) -> Response {
    auth_result_response(home_impl(&state.db, request.headers()).await)
}

async fn growth_tracking_handler(State(state): State<AppState>, request: Request) -> Response {
    auth_result_response(growth_tracking_impl(&state.db, request.headers()).await)
}

async fn diagnose_handler(State(state): State<AppState>, request: Request) -> Response {
    auth_result_response(diagnose_impl(&state.db, request.headers()).await)
}

async fn temperature_humidity_handler(State(state): State<AppState>, request: Request) -> Response {
    let query = query_params(&request);
    auth_result_response(temperature_humidity_impl(&state.db, request.headers(), &query).await)
}

async fn post_temperature_humidity_handler(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    auth_result_response(post_temperature_humidity_impl(&state.db, &headers, &body).await)
}

async fn generate_handler(State(state): State<AppState>, request: Request) -> Response {
    auth_result_response(generate_impl(&state.db, request.headers()).await)
}

async fn fertilization_plan_handler(State(state): State<AppState>, request: Request) -> Response {
    let headers = request.headers().clone();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    auth_result_response(fertilization_plan_impl(&state.db, &headers, &body).await)
}

async fn citrus_disease_handler(State(state): State<AppState>, request: Request) -> Response {
    let headers = request.headers().clone();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    auth_result_response(citrus_disease_impl(&state, &headers, &body).await)
}

async fn recognition_records_handler(State(state): State<AppState>, request: Request) -> Response {
    auth_result_response(recognition_records_impl(&state.db, request.headers()).await)
}

async fn disease_treatment_handler(State(state): State<AppState>, request: Request) -> Response {
    let query = query_params(&request);
    auth_result_response(disease_treatment_impl(&state.db, request.headers(), &query).await)
}

async fn tasks_handler(State(state): State<AppState>, request: Request) -> Response {
    auth_result_response(tasks_impl(&state.db, request.headers()).await)
}

async fn add_task_handler(State(state): State<AppState>, request: Request) -> Response {
    let headers = request.headers().clone();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    auth_result_response(add_task_impl(&state.db, &headers, &body).await)
}

async fn complete_task_handler(State(state): State<AppState>, request: Request) -> Response {
    let headers = request.headers().clone();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    auth_result_response(complete_task_impl(&state.db, &headers, &body).await)
}

async fn generate_task_from_disease_handler(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    auth_result_response(generate_task_from_disease_impl(&state.db, &headers, &body).await)
}

async fn generate_task_from_environment_handler(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    auth_result_response(generate_task_from_environment_impl(&state.db, &headers, &body).await)
}

fn auth_result_response(result: ApiResult) -> Response {
    match result {
        Ok(response) => response,
        Err(reject) => reject.into_response(),
    }
}

/// 手写 JSON 提取，而不是 `Json<Value>` extractor——axum 的 `JsonRejection` 会渲染成
/// `text/plain` 的裸 400，形状与契约差得远。
async fn json_body(request: Request) -> Result<Value, Response> {
    let bytes = to_bytes(request.into_body(), 16 * 1024 * 1024)
        .await
        .map_err(|_| ApiReject::bad_request("JSON 解析错误。").into_response())?;

    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| ApiReject::bad_request("JSON 解析错误。").into_response())
}

/// 查询串按「最后出现的同名参数生效」解析（Django `QueryDict.get` 的语义）。
fn query_params(request: &Request) -> Map<String, Value> {
    let mut params = Map::new();
    let Some(query) = request.uri().query() else {
        return params;
    };

    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(percent_decode(key), Value::String(percent_decode(value)));
    }
    params
}

/// 契约用例只会带 ASCII 键；这里做够用的 `application/x-www-form-urlencoded` 解码。
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let decoded = std::str::from_utf8(&bytes[index + 1..index + 3])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok());
                match decoded {
                    Some(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    None => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

fn query_value(query: &Map<String, Value>, key: &str) -> Option<String> {
    query.get(key).and_then(Value::as_str).map(str::to_string)
}

fn internal_error(err: sqlx::Error) -> ApiReject {
    tracing::error!("compat 农事识别域查询/写入失败: {err}");
    ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}

/// 带成功体形状的 4xx/5xx（蓝本 `create_response(None, msg, code)` 的形态）。
fn api_error(status: StatusCode, message: &str) -> Response {
    api_response(
        status,
        status.as_u16(),
        Value::String(message.to_string()),
        Value::Null,
    )
}

// --------------------------------------------------------------------------------------
// GET /api/home
// --------------------------------------------------------------------------------------

pub(crate) async fn home_impl(pool: &PgPool, headers: &HeaderMap) -> ApiResult {
    auth::require_farmer(pool, headers).await?;

    let row = sqlx::query(
        r#"SELECT weather_condition, temperature_range, suggestion, health_score,
                  weekly_alerts, pending_tasks, growth_rate, diagnosis_status, growth_status
             FROM home_data
            ORDER BY created_at
            LIMIT 1"#,
    )
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    let data = match row {
        Some(row) => json!({
            "weatherCondition": row.try_get::<String, _>("weather_condition").map_err(internal_error)?,
            "temperatureRange": row.try_get::<String, _>("temperature_range").map_err(internal_error)?,
            "suggestion": row.try_get::<String, _>("suggestion").map_err(internal_error)?,
            "healthScore": row.try_get::<i32, _>("health_score").map_err(internal_error)?,
            "weeklyAlerts": row.try_get::<i32, _>("weekly_alerts").map_err(internal_error)?,
            "pendingTasks": row.try_get::<i32, _>("pending_tasks").map_err(internal_error)?,
            "growthRate": row.try_get::<f64, _>("growth_rate").map_err(internal_error)?,
            "DiagnosisStatus": row.try_get::<String, _>("diagnosis_status").map_err(internal_error)?,
            "GrowthStatus": row.try_get::<String, _>("growth_status").map_err(internal_error)?,
        }),
        None => {
            insert_default_home_data(pool).await?;
            default_home_payload()
        }
    };

    Ok(api_ok(data))
}

pub(crate) fn default_home_payload() -> Value {
    json!({
        "weatherCondition": "阴",
        "temperatureRange": "20℃ - 25℃",
        "suggestion": "建议：保持正常天气，注意防晒。",
        "healthScore": 80,
        "weeklyAlerts": 2,
        "pendingTasks": 10,
        "growthRate": 85.5,
        "DiagnosisStatus": "正常",
        "GrowthStatus": "涨果期",
    })
}

async fn insert_default_home_data(pool: &PgPool) -> Result<(), ApiReject> {
    let now = Utc::now();
    sqlx::query(
        r#"INSERT INTO home_data
               (id, weather_condition, temperature_range, suggestion, health_score,
                weekly_alerts, pending_tasks, growth_rate, diagnosis_status, growth_status,
                created_at, updated_at)
           VALUES ($1, '阴', '20℃ - 25℃', '建议：保持正常天气，注意防晒。', 80,
                   2, 10, 85.5, '正常', '涨果期', $2, $2)"#,
    )
    .bind(Uuid::new_v4())
    .bind(now)
    .execute(pool)
    .await
    .map_err(internal_error)?;

    Ok(())
}

// --------------------------------------------------------------------------------------
// GET /api/growth-tracking
// --------------------------------------------------------------------------------------

const DEFAULT_GROWTH_STAGE_TEXT: &str = "涨果期";
const DEFAULT_GROWTH_STAGE_DURATION: &str = "30";

pub(crate) async fn growth_tracking_impl(pool: &PgPool, headers: &HeaderMap) -> ApiResult {
    auth::require_farmer(pool, headers).await?;

    let row = sqlx::query(
        r#"SELECT growth_stage_text, growth_stage_duration, start_date, end_date,
                  fruit_expansion_start_date, fruit_expansion_end_date,
                  color_change_start_date, color_change_end_date,
                  young_fruit_start_date, young_fruit_end_date, diameter, ratio
             FROM growth_tracking
            ORDER BY created_at
            LIMIT 1"#,
    )
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    let data = match row {
        Some(row) => json!({
            "growthStageText": row.try_get::<String, _>("growth_stage_text").map_err(internal_error)?,
            "growthStageDuration": row.try_get::<String, _>("growth_stage_duration").map_err(internal_error)?,
            "startDate": ser::dt_date(row.try_get::<NaiveDate, _>("start_date").map_err(internal_error)?),
            "endDate": ser::dt_date(row.try_get::<NaiveDate, _>("end_date").map_err(internal_error)?),
            "fruitExpansionStartDate": ser::dt_date(row.try_get::<NaiveDate, _>("fruit_expansion_start_date").map_err(internal_error)?),
            "fruitExpansionEndDate": ser::dt_date(row.try_get::<NaiveDate, _>("fruit_expansion_end_date").map_err(internal_error)?),
            "colorChangeStartDate": ser::dt_date(row.try_get::<NaiveDate, _>("color_change_start_date").map_err(internal_error)?),
            "colorChangeEndDate": ser::dt_date(row.try_get::<NaiveDate, _>("color_change_end_date").map_err(internal_error)?),
            "youngFruitStartDate": ser::dt_date(row.try_get::<NaiveDate, _>("young_fruit_start_date").map_err(internal_error)?),
            "youngFruitEndDate": ser::dt_date(row.try_get::<NaiveDate, _>("young_fruit_end_date").map_err(internal_error)?),
            "diameter": row.try_get::<f64, _>("diameter").map_err(internal_error)?,
            "ratio": row.try_get::<f64, _>("ratio").map_err(internal_error)?,
        }),
        None => {
            insert_default_growth_tracking(pool).await?;
            default_growth_tracking_payload()
        }
    };

    Ok(api_ok(data))
}

pub(crate) fn default_growth_tracking_payload() -> Value {
    json!({
        "growthStageText": DEFAULT_GROWTH_STAGE_TEXT,
        "growthStageDuration": DEFAULT_GROWTH_STAGE_DURATION,
        "startDate": "2025-12-01",
        "endDate": "2026-01-30",
        "fruitExpansionStartDate": "2025-12-01",
        "fruitExpansionEndDate": "2026-01-30",
        "colorChangeStartDate": "2026-02-01",
        "colorChangeEndDate": "2026-03-30",
        "youngFruitStartDate": "2026-04-01",
        "youngFruitEndDate": "2026-05-30",
        "diameter": 7.2,
        "ratio": 1.2,
    })
}

async fn insert_default_growth_tracking(pool: &PgPool) -> Result<(), ApiReject> {
    let now = Utc::now();
    sqlx::query(
        r#"INSERT INTO growth_tracking
               (id, growth_stage_text, growth_stage_duration, start_date, end_date,
                fruit_expansion_start_date, fruit_expansion_end_date,
                color_change_start_date, color_change_end_date,
                young_fruit_start_date, young_fruit_end_date, diameter, ratio,
                created_at, updated_at)
           VALUES ($1, '涨果期', '30', DATE '2025-12-01', DATE '2026-01-30',
                   DATE '2025-12-01', DATE '2026-01-30',
                   DATE '2026-02-01', DATE '2026-03-30',
                   DATE '2026-04-01', DATE '2026-05-30', 7.2, 1.2,
                   $2, $2)"#,
    )
    .bind(Uuid::new_v4())
    .bind(now)
    .execute(pool)
    .await
    .map_err(internal_error)?;

    Ok(())
}

// --------------------------------------------------------------------------------------
// GET /api/diagnose
// --------------------------------------------------------------------------------------

/// 惰性创建的三条诊断条目（`title`, `content`）。
const DEFAULT_DIAGNOSE_ITEMS: [(&str, &str); 3] = [
    (
        "氮元素含量稳定",
        "当前氮含量水平有利于叶片生长，维持现状即可",
    ),
    (
        "钾元素缺乏",
        "第5区果树钾元素偏低，建议补充钾肥提高果实品质",
    ),
    ("钙镁元素平衡", "当前钙镁比例适宜，有利于果实发育"),
];

pub(crate) async fn diagnose_impl(pool: &PgPool, headers: &HeaderMap) -> ApiResult {
    auth::require_farmer(pool, headers).await?;

    let row = sqlx::query(
        r#"SELECT id, data_date, nitrogen_value, phosphorus_value, potassium_value, percentage
             FROM diagnose_data
            ORDER BY created_at
            LIMIT 1"#,
    )
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    let (id, data_date, nitrogen, phosphorus, potassium, percentage) = match row {
        Some(row) => (
            row.try_get::<Uuid, _>("id").map_err(internal_error)?,
            row.try_get::<NaiveDate, _>("data_date")
                .map_err(internal_error)?,
            row.try_get::<f64, _>("nitrogen_value")
                .map_err(internal_error)?,
            row.try_get::<f64, _>("phosphorus_value")
                .map_err(internal_error)?,
            row.try_get::<f64, _>("potassium_value")
                .map_err(internal_error)?,
            row.try_get::<f64, _>("percentage")
                .map_err(internal_error)?,
        ),
        None => insert_default_diagnose_data(pool).await?,
    };

    let items = sqlx::query(
        r#"SELECT id, title, content, created_at
             FROM diagnose_list_items
            WHERE diagnose_id = $1
            ORDER BY created_at, id"#,
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let mut list = Vec::with_capacity(items.len());
    for item in items {
        list.push(json!({
            "id": item.try_get::<Uuid, _>("id").map_err(internal_error)?.to_string(),
            "title": item.try_get::<String, _>("title").map_err(internal_error)?,
            "content": item.try_get::<String, _>("content").map_err(internal_error)?,
            "created_at": ser::dt_z(item.try_get::<DateTime<Utc>, _>("created_at").map_err(internal_error)?),
        }));
    }

    // 蓝本用的是 dict 字面量，键序 = data → n_P_K_ViewModel → percentage → getList。
    Ok(api_ok(json!({
        "data": ser::dt_date(data_date),
        "n_P_K_ViewModel": {
            "nitrogenValue": nitrogen,
            "phosphorusValue": phosphorus,
            "potassiumValue": potassium,
        },
        "percentage": percentage,
        "getList": { "list": list },
    })))
}

type DiagnoseRow = (Uuid, NaiveDate, f64, f64, f64, f64);

async fn insert_default_diagnose_data(pool: &PgPool) -> Result<DiagnoseRow, ApiReject> {
    let id = Uuid::new_v4();
    let now = Utc::now();
    let data_date = NaiveDate::from_ymd_opt(2025, 12, 3).expect("静态日期合法");

    // 父行先落库：FK 是 DEFERRABLE，但顺序照着蓝本（先 DiagnoseData 再逐条 ListItem）最稳。
    sqlx::query(
        r#"INSERT INTO diagnose_data
               (id, data_date, nitrogen_value, phosphorus_value, potassium_value, percentage,
                created_at, updated_at)
           VALUES ($1, $2, 110.0, 50.0, 10.0, 86.0, $3, $3)"#,
    )
    .bind(id)
    .bind(data_date)
    .bind(now)
    .execute(pool)
    .await
    .map_err(internal_error)?;

    for (title, content) in DEFAULT_DIAGNOSE_ITEMS {
        sqlx::query(
            r#"INSERT INTO diagnose_list_items (id, diagnose_id, title, content, created_at)
               VALUES ($1, $2, $3, $4, $5)"#,
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(title)
        .bind(content)
        .bind(now)
        .execute(pool)
        .await
        .map_err(internal_error)?;
    }

    Ok((id, data_date, 110.0, 50.0, 10.0, 86.0))
}

// --------------------------------------------------------------------------------------
// GET / POST /api/temperature-humidity
// --------------------------------------------------------------------------------------

pub(crate) async fn temperature_humidity_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    _query: &Map<String, Value>,
) -> ApiResult {
    let user = auth::require_farmer(pool, headers).await?;

    let records = fetch_recent_temp_records(pool, user.id).await?;
    if !records.is_empty() {
        return Ok(api_ok(build_records_payload(&records)));
    }

    // 该果农还没有传感器记录时，退回读项目根的 CSV
    //（蓝本原话：Keep the chart useful until this farmer has sensor records.）
    let Some(csv_rows) = read_temperature_csv() else {
        return Ok(api_error(StatusCode::NOT_FOUND, ERR_TEMP_FILE_NOT_FOUND));
    };

    let recent_data = csv_rows[csv_rows.len().saturating_sub(10)..].to_vec();
    if recent_data.is_empty() {
        return Ok(api_error(StatusCode::NOT_FOUND, ERR_TEMP_FILE_EMPTY));
    }

    Ok(api_ok(build_csv_payload(&recent_data)))
}

pub(crate) async fn post_temperature_humidity_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    body: &Value,
) -> ApiResult {
    let user = auth::require_farmer(pool, headers).await?;
    let input = BodyInput::new(body);

    // 蓝本先 `float()` 再判区间；`ValueError`/`TypeError` 都归到同一句文案。
    let (Some(temperature), Some(humidity)) = (
        input.field("temperature").as_f64(),
        input.field("humidity").as_f64(),
    ) else {
        return Ok(api_error(
            StatusCode::BAD_REQUEST,
            ERR_TEMP_HUMIDITY_NOT_NUMBERS,
        ));
    };

    if !in_range(temperature, -50.0, 100.0) || !in_range(humidity, 0.0, 100.0) {
        return Ok(api_error(
            StatusCode::BAD_REQUEST,
            ERR_TEMP_HUMIDITY_OUT_OF_RANGE,
        ));
    }

    let record_time = match input.field("record_time").as_str() {
        Some(text) => match parse_iso8601(text) {
            Some(parsed) => parsed,
            None => return Ok(api_error(StatusCode::BAD_REQUEST, ERR_RECORD_TIME_ISO)),
        },
        None => Utc::now(),
    };

    // `str(tag_serial_number) if tag_serial_number is not None else 'NFC'`，落库前截 50 字符。
    let node_id = match input.field("tag_serial_number") {
        Input::Value(Value::String(text)) => text,
        Input::Value(Value::Number(number)) => number.to_string(),
        Input::Value(Value::Bool(flag)) => flag.to_string(),
        _ => "NFC".to_string(),
    };
    let node_id: String = node_id.chars().take(50).collect();

    sqlx::query(
        r#"INSERT INTO temperature_humidity_data
               (id, user_id, timestamp, temperature, humidity, node_id, created_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
    )
    .bind(Uuid::new_v4())
    .bind(user.id)
    .bind(record_time)
    .bind(temperature)
    .bind(humidity)
    .bind(&node_id)
    .bind(Utc::now())
    .execute(pool)
    .await
    .map_err(internal_error)?;

    let records = fetch_recent_temp_records(pool, user.id).await?;

    Ok(api_ok_message(
        MSG_TEMP_SAVED,
        build_records_payload(&records),
    ))
}

/// `order_by('-timestamp')[:10]`；蓝本随后 `reversed()` 成升序再逐条编号。
async fn fetch_recent_temp_records(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<TempRecord>, ApiReject> {
    let rows = sqlx::query(
        r#"SELECT timestamp, temperature, humidity, node_id
             FROM temperature_humidity_data
            WHERE user_id = $1
            ORDER BY timestamp DESC
            LIMIT 10"#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let mut records = Vec::with_capacity(rows.len());
    for row in rows {
        records.push(TempRecord {
            timestamp: row.try_get("timestamp").map_err(internal_error)?,
            temperature: row.try_get("temperature").map_err(internal_error)?,
            humidity: row.try_get("humidity").map_err(internal_error)?,
            node_id: row.try_get("node_id").map_err(internal_error)?,
        });
    }

    // Django 的 `list(reversed(list(records)))` 等价于「按时间升序」。
    records.reverse();
    Ok(records)
}

/// `_build_temperature_humidity_response`：带 `recentRecords` 的完整形态。
pub(crate) fn build_records_payload(records: &[TempRecord]) -> Value {
    let mut temperature_history = Vec::with_capacity(records.len());
    let mut humidity_history = Vec::with_capacity(records.len());
    let mut recent_records = Vec::with_capacity(records.len());

    for (index, item) in records.iter().enumerate() {
        let label = history_label(index, records.len(), item.timestamp);
        let temperature = round_half_even(item.temperature, 1);
        let humidity = round_half_even(item.humidity, 1);

        temperature_history.push(json!({ "timestamp": label.clone(), "value": temperature }));
        humidity_history.push(json!({ "timestamp": label, "value": humidity }));
        recent_records.push(json!({
            "temperature": temperature,
            "humidity": humidity,
            // `item.timestamp.isoformat()` → `+00:00` 形态（D4）。
            "record_time": ser::dt_offset(item.timestamp),
            "tag_serial_number": item.node_id,
        }));
    }

    json!({
        "temperatureHistory": temperature_history,
        "humidityHistory": humidity_history,
        "currentTemperature": last_value(&temperature_history),
        "currentHumidity": last_value(&humidity_history),
        "recentRecords": recent_records,
    })
}

/// CSV 兜底分支：**没有** `recentRecords` 键，时间标签来自 CSV 的 `HH:MM` 文本。
pub(crate) fn build_csv_payload(rows: &[Vec<String>]) -> Value {
    let mut temperature_history = Vec::with_capacity(rows.len());
    let mut humidity_history = Vec::with_capacity(rows.len());
    let total = rows.len();

    for (index, row) in rows.iter().enumerate() {
        let timestamp = row.first().cloned().unwrap_or_default();
        let temperature = row
            .get(1)
            .and_then(|text| text.parse::<f64>().ok())
            .unwrap_or_default();
        let humidity = row
            .get(2)
            .and_then(|text| text.parse::<f64>().ok())
            .unwrap_or_default();

        let label = if index % 2 == 0 || index == total - 1 {
            normalize_hh_mm(&timestamp)
        } else {
            String::new()
        };

        temperature_history.push(json!({
            "timestamp": label.clone(),
            "value": round_half_even(temperature, 1),
        }));
        humidity_history.push(json!({
            "timestamp": label,
            "value": round_half_even(humidity, 1),
        }));
    }

    json!({
        "temperatureHistory": temperature_history,
        "humidityHistory": humidity_history,
        "currentTemperature": last_value(&temperature_history),
        "currentHumidity": last_value(&humidity_history),
    })
}

fn last_value(history: &[Value]) -> Value {
    history
        .last()
        .map(|item| item["value"].clone())
        .unwrap_or_else(|| Value::from(0.0))
}

/// `f"{int(hour):02d}:{int(minute):02d}"`；无法解析时原样返回。
pub(crate) fn normalize_hh_mm(raw: &str) -> String {
    let mut parts = raw.split(':');
    let (Some(hour), Some(minute)) = (parts.next(), parts.next()) else {
        return raw.to_string();
    };
    match (hour.trim().parse::<i64>(), minute.trim().parse::<i64>()) {
        (Ok(hour), Ok(minute)) => format!("{hour:02}:{minute:02}"),
        _ => raw.to_string(),
    }
}

/// `label = item.timestamp.strftime('%H:%M') if index % 2 == 0 or index == len - 1 else ''`
pub(crate) fn history_label(index: usize, total: usize, timestamp: DateTime<Utc>) -> String {
    if index % 2 == 0 || index == total - 1 {
        timestamp.format("%H:%M").to_string()
    } else {
        String::new()
    }
}

/// 读 CSV。返回 `None` = 文件不存在（对应 404 `Temperature data file not found`）。
fn read_temperature_csv() -> Option<Vec<Vec<String>>> {
    let text = std::fs::read_to_string(TEMPERATURE_CSV_PATH).ok()?;

    let mut rows = Vec::new();
    for line in text.lines().skip(1) {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        rows.push(
            line.split(',')
                .map(|cell| cell.trim().to_string())
                .collect(),
        );
    }

    Some(rows)
}
// --------------------------------------------------------------------------------------
// GET /api/generate
// --------------------------------------------------------------------------------------

/// 蓝本写死的施肥建议正文（`generate_api`）。
const FERTILIZATION_TEXT: &str = "针对赣南脐橙，建议实施\"测土配方、分期精准\"的施肥策略。基肥以腐熟有机肥（如羊粪、饼肥）为主，秋季深施，改良酸性红壤。追肥分三次：春梢期以高氮复合肥促梢保花；壮果期增施钾肥（如硫酸钾），配施磷与中微量元素，提升糖度与果皮光泽；采果前补施速效肥恢复树势。全年注重叶片营养诊断，结合土壤检测结果灵活调整，确保氮、磷、钾与钙、镁、硼等元素平衡，避免偏施氮肥。坚持生草栽培，保墒增肥。";

pub(crate) async fn generate_impl(pool: &PgPool, headers: &HeaderMap) -> ApiResult {
    auth::require_farmer(pool, headers).await?;
    Ok(api_ok(json!({ "textii": FERTILIZATION_TEXT })))
}

// --------------------------------------------------------------------------------------
// POST /api/generate/fertilization-plan
// --------------------------------------------------------------------------------------

/// 施肥方案（`DEVIATIONS.md` **D2**）。
///
/// 蓝本这里引用了 `views.py` **没有导入**的 `FertilizationPlanRequestSerializer`，
/// 于是 `POST /api/generate/fertilization-plan` 恒 500（message 是 Python 异常文本）。
/// 按裁定**修正为设计意图的正常语义**：照 `serializers.py` 的字段定义做请求校验，
/// 成功则按 `FertilizationPlanSerializer` 输出 `{planId, title, content,
/// recommendedFertilizers, applicationSchedule}`。比对器把命中 D2 的用例标为
/// `expected_deviation`，**不复刻 5xx**。
///
/// `FertilizationPlanRequestSerializer` 的字段声明序 = 校验报错的收集顺序。
pub(crate) async fn fertilization_plan_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    body: &Value,
) -> ApiResult {
    let user = auth::require_farmer(pool, headers).await?;
    let input = BodyInput::new(body);

    let mut errors = FieldErrors::default();
    let soil_type = required_char_field(&input, "soilType", 50, &mut errors);
    let ph_value = required_float_field(&input, "phValue", &mut errors);
    let nitrogen_level = required_char_field(&input, "nitrogenLevel", 50, &mut errors);
    let phosphorus_level = required_char_field(&input, "phosphorusLevel", 50, &mut errors);
    let potassium_level = required_char_field(&input, "potassiumLevel", 50, &mut errors);
    let growth_stage = required_char_field(&input, "growthStage", 50, &mut errors);
    let tree_age = required_integer_field(&input, "treeAge", &mut errors);
    let area_size = required_float_field(&input, "areaSize", &mut errors);

    let (
        Some(soil_type),
        Some(ph_value),
        Some(nitrogen_level),
        Some(phosphorus_level),
        Some(potassium_level),
        Some(growth_stage),
        Some(tree_age),
        Some(area_size),
    ) = (
        soil_type,
        ph_value,
        nitrogen_level,
        phosphorus_level,
        potassium_level,
        growth_stage,
        tree_age,
        area_size,
    )
    else {
        // 蓝本这一行写的是 `create_response(None, "Invalid request data", 400)`——
        // 文案固定、不带字段错误明细（与 tasks/add 那种 `serializer.errors` 不同），
        // 所以校验器收集到的 `errors` 只用来判定「是否通过」，不进响应体。
        tracing::debug!("fertilization-plan 请求校验失败: {}", errors.render());
        return Ok(api_error(
            StatusCode::BAD_REQUEST,
            ERR_FERTILIZATION_INVALID,
        ));
    };

    let plan = persist_fertilization_plan(
        pool,
        &user,
        &FertilizationRequest {
            soil_type,
            ph_value,
            nitrogen_level,
            phosphorus_level,
            potassium_level,
            growth_stage,
            tree_age,
            area_size,
        },
    )
    .await?;

    Ok(api_ok(plan))
}

struct FertilizationRequest {
    soil_type: String,
    ph_value: f64,
    nitrogen_level: String,
    phosphorus_level: String,
    potassium_level: String,
    growth_stage: String,
    tree_age: i64,
    area_size: f64,
}

/// `planId` 形如 `fp-YYYYMMDD-NNNN`（`random.randint(1000, 9999)`）。
const RECOMMENDED_FERTILIZERS: [(&str, &str, &str); 2] = [
    ("硫酸钾", "50kg/亩", "穴施"),
    ("过磷酸钙", "30kg/亩", "撒施"),
];

/// `(base_date + timedelta(days=n)).date()`，配合阶段与描述。
const APPLICATION_SCHEDULE: [(i64, &str, &str); 2] = [
    (15, "壮果期初期", "第一次施肥"),
    (45, "壮果期中期", "第二次施肥"),
];

async fn persist_fertilization_plan(
    pool: &PgPool,
    _user: &AuthUser,
    request: &FertilizationRequest,
) -> Result<Value, ApiReject> {
    let now = Utc::now();
    let today = now.date_naive().format("%Y%m%d").to_string();
    let suffix = 1000 + (OsRng.next_u32() % 9000);
    let plan_id = format!("fp-{today}-{suffix}");

    let title = format!("赣南脐橙{}施肥方案", request.growth_stage);
    let content = format!(
        "针对{}，pH值{}，建议实施分期精准施肥策略。根据当前土壤养分状况（氮:{}，磷:{}，钾:{}），结合{}年生果树的生长阶段，制定个性化施肥方案。",
        request.soil_type,
        request.ph_value,
        request.nitrogen_level,
        request.phosphorus_level,
        request.potassium_level,
        request.tree_age,
    );

    let mut tx = pool.begin().await.map_err(internal_error)?;
    let plan_row_id = Uuid::new_v4();

    sqlx::query(
        r#"INSERT INTO fertilization_plan
               (id, plan_id, title, content, soil_type, ph_value, nitrogen_level,
                phosphorus_level, potassium_level, growth_stage, tree_age, area_size,
                created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $13)"#,
    )
    .bind(plan_row_id)
    .bind(&plan_id)
    .bind(&title)
    .bind(&content)
    .bind(&request.soil_type)
    .bind(request.ph_value)
    .bind(&request.nitrogen_level)
    .bind(&request.phosphorus_level)
    .bind(&request.potassium_level)
    .bind(&request.growth_stage)
    .bind(request.tree_age as i32)
    .bind(request.area_size)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;

    for (name, amount, method) in RECOMMENDED_FERTILIZERS {
        sqlx::query(
            r#"INSERT INTO recommended_fertilizer (id, plan_id, name, amount, application_method)
               VALUES ($1, $2, $3, $4, $5)"#,
        )
        .bind(Uuid::new_v4())
        .bind(plan_row_id)
        .bind(name)
        .bind(amount)
        .bind(method)
        .execute(&mut *tx)
        .await
        .map_err(internal_error)?;
    }

    for (days, stage, description) in APPLICATION_SCHEDULE {
        let date = (now + chrono::Duration::days(days)).date_naive();
        sqlx::query(
            r#"INSERT INTO application_schedule (id, plan_id, stage, date, description)
               VALUES ($1, $2, $3, $4, $5)"#,
        )
        .bind(Uuid::new_v4())
        .bind(plan_row_id)
        .bind(stage)
        .bind(date)
        .bind(description)
        .execute(&mut *tx)
        .await
        .map_err(internal_error)?;
    }

    tx.commit().await.map_err(internal_error)?;

    let fertilizers = fetch_recommended_fertilizers(pool, plan_row_id).await?;
    let schedules = fetch_application_schedules(pool, plan_row_id).await?;

    let mut payload = Map::new();
    payload.insert("planId".to_string(), Value::String(plan_id));
    payload.insert("title".to_string(), Value::String(title));
    payload.insert("content".to_string(), Value::String(content));
    payload.insert(
        "recommendedFertilizers".to_string(),
        Value::Array(fertilizers),
    );
    payload.insert("applicationSchedule".to_string(), Value::Array(schedules));

    Ok(Value::Object(payload))
}

async fn fetch_recommended_fertilizers(
    pool: &PgPool,
    plan_id: Uuid,
) -> Result<Vec<Value>, ApiReject> {
    let rows = sqlx::query(
        r#"SELECT name, amount, application_method
             FROM recommended_fertilizer
            WHERE plan_id = $1
            ORDER BY name"#,
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(json!({
            "name": row.try_get::<String, _>("name").map_err(internal_error)?,
            "amount": row.try_get::<String, _>("amount").map_err(internal_error)?,
            "applicationMethod": row.try_get::<String, _>("application_method").map_err(internal_error)?,
        }));
    }

    Ok(items)
}

async fn fetch_application_schedules(
    pool: &PgPool,
    plan_id: Uuid,
) -> Result<Vec<Value>, ApiReject> {
    let rows = sqlx::query(
        r#"SELECT stage, date, description
             FROM application_schedule
            WHERE plan_id = $1
            ORDER BY date"#,
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(json!({
            "stage": row.try_get::<String, _>("stage").map_err(internal_error)?,
            "date": ser::dt_date(row.try_get::<NaiveDate, _>("date").map_err(internal_error)?),
            "description": row.try_get::<String, _>("description").map_err(internal_error)?,
        }));
    }

    Ok(items)
}

// --------------------------------------------------------------------------------------
// POST /api/citrus-disease
// --------------------------------------------------------------------------------------

pub(crate) async fn citrus_disease_impl(
    state: &AppState,
    headers: &HeaderMap,
    body: &Value,
) -> ApiResult {
    let user = auth::require_farmer(&state.db, headers).await?;
    let input = BodyInput::new(body);

    // 蓝本的图片分支优先级：文件上传 `image` → base64 `IMAGE` → 400。
    // 本层不支持 multipart，保留 base64 分支（夹具只覆盖 `IMAGE`）。
    let has_upload = !input.field("image").is_missing();
    let image_data = input.field("IMAGE").as_str().map(str::to_string);

    if !has_upload && image_data.is_none() {
        return Ok(api_error(StatusCode::BAD_REQUEST, ERR_NO_IMAGE));
    }

    // 沿用现有推理运行时（`AppState.inference`），不引入 torch；只对齐返回字段。
    let prediction = state
        .inference
        .predict_citrus_disease(image_data, None, None)
        .await
        .map_err(|err| {
            tracing::error!("compat 柑橘识别失败: {err}");
            ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
        })?;

    // 蓝本的 `is_citrus and image_file` 记录分支只在**文件上传**时才写库；
    // base64 分支不落识别记录，所以这里也不写。
    let _ = &user;

    Ok(api_ok(json!({
        "predicted_class": prediction.predicted_class,
        // `round(prediction['confidence'], 2)`
        "confidence": round_half_even(prediction.confidence, 2),
        "stage": prediction.stage,
    })))
}

// --------------------------------------------------------------------------------------
// GET /api/recognition-records
// --------------------------------------------------------------------------------------

pub(crate) async fn recognition_records_impl(pool: &PgPool, headers: &HeaderMap) -> ApiResult {
    let user = auth::require_farmer(pool, headers).await?;

    // `order_by('-created_at')[:20]`
    let rows = sqlx::query(
        r#"SELECT id, image, disease_name, area, risk_level, recognition_date,
                  confidence, created_at
             FROM disease_recognition_record
            WHERE user_id = $1
            ORDER BY created_at DESC, id::text
            LIMIT 20"#,
    )
    .bind(user.id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let mut records = Vec::with_capacity(rows.len());
    for row in rows {
        records.push(RecognitionRecord {
            id: row.try_get("id").map_err(internal_error)?,
            image: row.try_get("image").map_err(internal_error)?,
            disease_name: row.try_get("disease_name").map_err(internal_error)?,
            area: row.try_get("area").map_err(internal_error)?,
            risk_level: row.try_get("risk_level").map_err(internal_error)?,
            recognition_date: row.try_get("recognition_date").map_err(internal_error)?,
            confidence: row.try_get("confidence").map_err(internal_error)?,
            created_at: row.try_get("created_at").map_err(internal_error)?,
        });
    }

    Ok(api_ok(json!({ "records": records_payload(&records) })))
}

/// `DiseaseRecognitionRecordSerializer` 的字段序：`id, imagePath, diseaseName, area,
/// riskLevel, recognitionDate, confidence, created_at`。
fn records_payload(records: &[RecognitionRecord]) -> Vec<Value> {
    records
        .iter()
        .map(|record| {
            json!({
                "id": record.id.to_string(),
                // `get_imagePath`：`obj.image` 为空（Django 的假值）→ `''`。
                "imagePath": image_path(&record.image),
                "diseaseName": record.disease_name,
                "area": record.area,
                "riskLevel": record.risk_level,
                "recognitionDate": ser::dt_date(record.recognition_date),
                "confidence": record.confidence,
                "created_at": ser::dt_z(record.created_at),
            })
        })
        .collect()
}

/// 空串或空白 → `''`（Django 里空 `ImageField` 是假值）。
pub(crate) fn image_path(image: &str) -> String {
    if image.trim().is_empty() {
        String::new()
    } else {
        image.to_string()
    }
}

// --------------------------------------------------------------------------------------
// GET /api/disease-treatment
// --------------------------------------------------------------------------------------

pub(crate) async fn disease_treatment_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    query: &Map<String, Value>,
) -> ApiResult {
    auth::require_farmer(pool, headers).await?;

    // `request.GET.get('disease_name')`：空串也算缺（false）。
    let Some(disease_name) = query_value(query, "disease_name").filter(|name| !name.is_empty())
    else {
        return Ok(api_error(
            StatusCode::BAD_REQUEST,
            ERR_DISEASE_NAME_PARAM_REQUIRED,
        ));
    };

    Ok(api_ok(json!({
        "disease_name": disease_name,
        "treatment": disease_treatment(&disease_name),
    })))
}

// --------------------------------------------------------------------------------------
// GET /api/tasks
// --------------------------------------------------------------------------------------

pub(crate) async fn tasks_impl(pool: &PgPool, headers: &HeaderMap) -> ApiResult {
    let user = auth::require_farmer(pool, headers).await?;

    // `Task.Meta.ordering = ['-created_at']`，蓝本没有再显式 order_by。
    //
    // ⚠️ 平局必须按 **UUID 的文本形态**排，不能直接 `ORDER BY id`：PG 的 `uuid` 是
    // 16 字节比较，`-`(0x2D) 排在数字(0x30-)之后；Django 在 SQLite 里存的是 CHAR(32)/文本，
    // 比的是字符串。种子任务 `created_at` 三项相同，平局时的先后就靠这一位定，
    // 直接比 `uuid` 会得到与夹具相反的顺序（`6a53…` 与 `8e8d…` 互换）。
    let rows = sqlx::query(
        r#"SELECT id, title, description, risk_level, task_type, source, is_completed,
                  created_at, completed_at
             FROM task
            WHERE user_id = $1
            ORDER BY created_at DESC, id::text"#,
    )
    .bind(user.id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let mut tasks = Vec::with_capacity(rows.len());
    for row in rows {
        tasks.push(task_payload(&row)?);
    }

    Ok(api_ok(Value::Array(tasks)))
}

/// `TaskSerializer` 的字段序与序列化规则。
///
/// `created_at` 走 `Z` 形态（D4）；`completed_at` 用**朴素本地时间**（D3）——蓝本
/// `complete_task_api` 写的是 `datetime.now()`，除该字段外不要用 `dt_naive_local`。
fn task_payload(row: &sqlx::postgres::PgRow) -> Result<Value, ApiReject> {
    let mut payload = Map::new();
    payload.insert(
        "id".to_string(),
        Value::String(
            row.try_get::<Uuid, _>("id")
                .map_err(internal_error)?
                .to_string(),
        ),
    );
    payload.insert(
        "title".to_string(),
        Value::String(row.try_get::<String, _>("title").map_err(internal_error)?),
    );
    payload.insert(
        "description".to_string(),
        Value::String(
            row.try_get::<String, _>("description")
                .map_err(internal_error)?,
        ),
    );
    payload.insert(
        "risk_level".to_string(),
        Value::String(
            row.try_get::<String, _>("risk_level")
                .map_err(internal_error)?,
        ),
    );
    payload.insert(
        "task_type".to_string(),
        Value::String(
            row.try_get::<String, _>("task_type")
                .map_err(internal_error)?,
        ),
    );
    payload.insert(
        "source".to_string(),
        Value::String(row.try_get::<String, _>("source").map_err(internal_error)?),
    );
    payload.insert(
        "is_completed".to_string(),
        Value::Bool(
            row.try_get::<bool, _>("is_completed")
                .map_err(internal_error)?,
        ),
    );
    payload.insert(
        "created_at".to_string(),
        Value::String(ser::dt_z(
            row.try_get::<DateTime<Utc>, _>("created_at")
                .map_err(internal_error)?,
        )),
    );
    payload.insert(
        "completed_at".to_string(),
        match row
            .try_get::<Option<DateTime<Utc>>, _>("completed_at")
            .map_err(internal_error)?
        {
            Some(value) => Value::String(ser::dt_naive_local(value)),
            None => Value::Null,
        },
    );

    Ok(Value::Object(payload))
}

// --------------------------------------------------------------------------------------
// POST /api/tasks/add
// --------------------------------------------------------------------------------------

pub(crate) async fn add_task_impl(pool: &PgPool, headers: &HeaderMap, body: &Value) -> ApiResult {
    let user = auth::require_farmer(pool, headers).await?;
    let input = BodyInput::new(body);

    let mut errors = FieldErrors::default();
    let title = required_char_field(&input, "title", 200, &mut errors);
    let description = required_char_field(&input, "description", 0, &mut errors);
    // 模型层有 default，序列化器不要求必填；缺 / null 都落回默认值。
    let risk_level = optional_char_field(&input, "risk_level", 20, "中风险", &mut errors);
    let task_type = optional_char_field(&input, "task_type", 50, "手动添加", &mut errors);
    let source = optional_char_field(&input, "source", 50, "用户", &mut errors);

    let (Some(title), Some(description), Some(risk_level), Some(task_type), Some(source)) =
        (title, description, risk_level, task_type, source)
    else {
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    };

    let task_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO task
               (id, user_id, title, description, risk_level, task_type, source,
                is_completed, created_at, completed_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, FALSE, $8, NULL)"#,
    )
    .bind(task_id)
    .bind(user.id)
    .bind(&title)
    .bind(&description)
    .bind(&risk_level)
    .bind(&task_type)
    .bind(&source)
    .bind(Utc::now())
    .execute(pool)
    .await
    .map_err(internal_error)?;

    let payload = fetch_task_payload(pool, task_id).await?;
    Ok(api_ok_message(MSG_TASK_CREATED, payload))
}

// --------------------------------------------------------------------------------------
// POST /api/tasks/complete
// --------------------------------------------------------------------------------------

pub(crate) async fn complete_task_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    body: &Value,
) -> ApiResult {
    let user = auth::require_farmer(pool, headers).await?;
    let input = BodyInput::new(body);

    // `request.data.get('task_id')`：缺 / null / 空串都算没有。
    let raw = match input.field("task_id") {
        Input::Value(Value::String(text)) if !text.is_empty() => text,
        _ => return Ok(api_error(StatusCode::BAD_REQUEST, ERR_TASK_ID_REQUIRED)),
    };

    let found = match Uuid::parse_str(&raw) {
        Ok(task_id) => {
            // ⚠️ D3：蓝本写的是 `datetime.now()`（朴素本地时间），不要改成 UTC。
            sqlx::query(
                "UPDATE task SET is_completed = TRUE, completed_at = $1 WHERE id = $2 AND user_id = $3",
            )
            .bind(Utc::now())
            .bind(task_id)
            .bind(user.id)
            .execute(pool)
            .await
            .map_err(internal_error)?
            .rows_affected()
                > 0
        }
        // 非法 UUID 不可能命中任何行 —— 蓝本拿字符串去 filter，同样查不到。
        Err(_) => false,
    };

    if !found {
        return Ok(api_error(StatusCode::NOT_FOUND, ERR_TASK_NOT_FOUND));
    }

    let payload = fetch_task_payload(pool, Uuid::parse_str(&raw).expect("上面已校验")).await?;
    Ok(api_ok_message(MSG_TASK_COMPLETED, payload))
}

// --------------------------------------------------------------------------------------
// POST /api/tasks/generate/disease
// --------------------------------------------------------------------------------------

pub(crate) async fn generate_task_from_disease_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    body: &Value,
) -> ApiResult {
    let user = auth::require_farmer(pool, headers).await?;
    let input = BodyInput::new(body);

    let Some(disease_name) = input
        .field("disease_name")
        .as_str()
        .map(str::to_string)
        .filter(|text| !text.is_empty())
    else {
        return Ok(api_error(
            StatusCode::BAD_REQUEST,
            ERR_DISEASE_NAME_REQUIRED,
        ));
    };

    if disease_name == HEALTHY_TREE || disease_name == NON_TREE {
        // 蓝本用 200 + data=null 表达「无需操作」。
        return Ok(api_ok_message(MSG_NO_TASK_HEALTHY, Value::Null));
    }

    // 蓝本把三种已知疾病之外的一律也按「高风险」建任务（`risk_level` 是常量）。
    let task_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO task
               (id, user_id, title, description, risk_level, task_type, source,
                is_completed, created_at, completed_at)
           VALUES ($1, $2, $3, $4, '高风险', '疾病识别', '自动生成', FALSE, $5, NULL)"#,
    )
    .bind(task_id)
    .bind(user.id)
    .bind(format!("{disease_name}治理"))
    .bind(disease_treatment(&disease_name))
    .bind(Utc::now())
    .execute(pool)
    .await
    .map_err(internal_error)?;

    let payload = fetch_task_payload(pool, task_id).await?;
    Ok(api_ok_message(MSG_TASK_CREATED, payload))
}

// --------------------------------------------------------------------------------------
// POST /api/tasks/generate/environment
// --------------------------------------------------------------------------------------

pub(crate) async fn generate_task_from_environment_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    body: &Value,
) -> ApiResult {
    let user = auth::require_farmer(pool, headers).await?;
    let input = BodyInput::new(body);

    // 蓝本判的是 `is None`：显式 `null` 与缺字段都进 400。
    let (Some(temperature), Some(humidity)) = (
        input.field("temperature").as_f64(),
        input.field("humidity").as_f64(),
    ) else {
        return Ok(api_error(
            StatusCode::BAD_REQUEST,
            ERR_ENVIRONMENT_PARAMS_REQUIRED,
        ));
    };

    let (risk_level, description) = classify_environment_risk(temperature, humidity);

    // 只为中/高风险建档；低风险走 200 + data=null。
    if risk_level == RISK_LOW {
        return Ok(api_ok_message(MSG_NO_TASK_LOW_RISK, Value::Null));
    }

    let task_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO task
               (id, user_id, title, description, risk_level, task_type, source,
                is_completed, created_at, completed_at)
           VALUES ($1, $2, $3, $4, $5, '温湿度监测', '自动生成', FALSE, $6, NULL)"#,
    )
    .bind(task_id)
    .bind(user.id)
    .bind(format!("环境监测-{risk_level}"))
    .bind(description)
    .bind(risk_level)
    .bind(Utc::now())
    .execute(pool)
    .await
    .map_err(internal_error)?;

    let payload = fetch_task_payload(pool, task_id).await?;
    Ok(api_ok_message(MSG_TASK_CREATED, payload))
}

async fn fetch_task_payload(pool: &PgPool, task_id: Uuid) -> Result<Value, ApiReject> {
    let row = sqlx::query(
        r#"SELECT id, title, description, risk_level, task_type, source, is_completed,
                  created_at, completed_at
             FROM task
            WHERE id = $1"#,
    )
    .bind(task_id)
    .fetch_one(pool)
    .await
    .map_err(internal_error)?;

    task_payload(&row)
}

// --------------------------------------------------------------------------------------
// 请求体读取与 DRF 字段校验（本域内局部实现，不动 dto.rs）
// --------------------------------------------------------------------------------------

/// 请求体只读视图：非对象体一律当作「所有字段都缺」（DRF 的行为）。
struct BodyInput<'a> {
    map: Option<&'a Map<String, Value>>,
}

impl<'a> BodyInput<'a> {
    fn new(body: &'a Value) -> Self {
        Self {
            map: body.as_object(),
        }
    }

    fn field(&self, key: &str) -> Input {
        match self.map {
            None => Input::Missing,
            Some(map) => Input::take(map, key),
        }
    }
}

/// DRF 的字段错误字典：`{"字段": ["文案"]}`，**插入序 = 字段声明序**。
#[derive(Debug, Default)]
struct FieldErrors(Map<String, Value>);

impl FieldErrors {
    fn push(&mut self, field: &str, message: &str) {
        self.0.insert(field.to_string(), json!([message]));
    }

    /// 校验失败走**成功体形状**（带 `timestamp`），只是 HTTP 状态码是 4xx。
    fn into_response(self, status: StatusCode) -> Response {
        api_response(status, status.as_u16(), Value::Object(self.0), Value::Null)
    }

    /// 只用于日志（`fertilization-plan` 的响应体是固定文案，不带明细）。
    fn render(&self) -> String {
        Value::Object(self.0.clone()).to_string()
    }
}

/// `serializers.CharField(required=True, max_length=?, allow_blank=True)`。
fn required_char_field(
    input: &BodyInput<'_>,
    field: &str,
    max_length: usize,
    errors: &mut FieldErrors,
) -> Option<String> {
    match input.field(field) {
        Input::Missing => {
            errors.push(field, ERR_REQUIRED);
            None
        }
        Input::Null => {
            errors.push(field, ERR_NULL);
            None
        }
        Input::Value(Value::String(text)) => {
            if max_length > 0 && text.chars().count() > max_length {
                errors.push(
                    field,
                    &format!("请确保这个字段不能超过 {max_length} 个字符。"),
                );
                return None;
            }
            Some(text.trim().to_string())
        }
        Input::Value(_) => {
            errors.push(field, ERR_INVALID_STRING);
            None
        }
    }
}

/// 带模型默认值的可选 `CharField`：缺 / `null` / `''` 都落回 `default`。
fn optional_char_field(
    input: &BodyInput<'_>,
    field: &str,
    max_length: usize,
    default: &str,
    errors: &mut FieldErrors,
) -> Option<String> {
    match input.field(field) {
        Input::Missing | Input::Null => Some(default.to_string()),
        Input::Value(Value::String(text)) => {
            if max_length > 0 && text.chars().count() > max_length {
                errors.push(
                    field,
                    &format!("请确保这个字段不能超过 {max_length} 个字符。"),
                );
                return None;
            }
            if text.is_empty() {
                return Some(default.to_string());
            }
            Some(text.trim().to_string())
        }
        Input::Value(_) => {
            errors.push(field, ERR_INVALID_STRING);
            None
        }
    }
}

/// `serializers.FloatField(required=True)`。
fn required_float_field(
    input: &BodyInput<'_>,
    field: &str,
    errors: &mut FieldErrors,
) -> Option<f64> {
    match input.field(field) {
        Input::Missing => {
            errors.push(field, ERR_REQUIRED);
            None
        }
        Input::Null => {
            errors.push(field, ERR_NULL);
            None
        }
        other => match other.as_f64() {
            Some(value) => Some(value),
            None => {
                errors.push(field, ERR_INVALID_NUMBER);
                None
            }
        },
    }
}

/// `serializers.IntegerField(required=True)`。
fn required_integer_field(
    input: &BodyInput<'_>,
    field: &str,
    errors: &mut FieldErrors,
) -> Option<i64> {
    match input.field(field) {
        Input::Missing => {
            errors.push(field, ERR_REQUIRED);
            None
        }
        Input::Null => {
            errors.push(field, ERR_NULL);
            None
        }
        Input::Value(Value::Number(number)) => match number.as_i64() {
            Some(value) => Some(value),
            None => {
                errors.push(field, ERR_INVALID_NUMBER);
                None
            }
        },
        Input::Value(Value::String(text)) => match text.trim().parse::<i64>() {
            Ok(value) => Some(value),
            Err(_) => {
                errors.push(field, ERR_INVALID_NUMBER);
                None
            }
        },
        Input::Value(_) => {
            errors.push(field, ERR_INVALID_NUMBER);
            None
        }
    }
}

// --------------------------------------------------------------------------------------
// 数值与时间的小工具
// --------------------------------------------------------------------------------------

/// Python 的 `round()` 是**银行家舍入**（half-even），Rust 的 `f64::round` 是
/// half-away-from-zero。差值只在恰好落半的输入上出现，但既然比对逐值，就得按 Python 来。
pub(crate) fn round_half_even(value: f64, places: u32) -> f64 {
    if !value.is_finite() {
        return value;
    }

    let factor = 10f64.powi(places as i32);
    let scaled = value * factor;
    let floor = scaled.floor();
    let remainder = scaled - floor;

    let rounded = if (remainder - 0.5).abs() < 1e-9 {
        if (floor as i64) % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    } else {
        scaled.round()
    };

    rounded / factor
}

/// Django `parse_datetime`：只认 ISO 8601。无偏移时 `make_aware` 按当前时区（UTC）补齐。
pub(crate) fn parse_iso8601(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(value) = DateTime::parse_from_rfc3339(raw) {
        return Some(value.with_timezone(&Utc));
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(naive.and_utc());
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S") {
        return Some(naive.and_utc());
    }
    None
}
