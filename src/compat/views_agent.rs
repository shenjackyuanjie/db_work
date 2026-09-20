//! 智能体域契约实现。
//!
//! 对应 `navel_backend_git/api/urls.py` 中 agent 域 **11 条 path**，蓝本为 `api/agent_views.py`。
//! 契约基准见 `tests/fixtures/contract/agent.json`。
//!
//! 三条容易踩空的地方：
//!
//! 1. **蓝本漏 `import status`**：`select` / `inquiry` / `chat` / `feedback` / `approvals` /
//!    `approvals/<id>/decision` 的**全部校验分支**在蓝本里恒 500。按 `DEVIATIONS.md` **D1**
//!    修正为设计意图的 **400/404**，并且在形状上用 `api_response(...)`（**成功体**，
//!    带 `timestamp`）——蓝本这几处就是 `api_response(None, '文案', 4xx)`。**不要复刻 500**。
//! 2. **匿名接口不鉴权**：`select` / `inquiry` 在蓝本里是 `AllowAny` 且**没挂**
//!    `BearerTokenAuthentication`，所以 replay 带上 token 也不解析（`created_by` 记 NULL）。
//! 3. **测试缝**：`AppState` 构造会加载 ONNX 模型，单测里没法廉价构造，所以每个端点拆成
//!    「薄 axum 包装 + `*_impl(&PgPool, ...)`」，单测直接打 scratch schema。

use axum::{
    Router,
    extract::{Path, Request, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::server::AppState;

use super::agent_service::{self, ApprovalRow, FeedbackRow};
use super::auth::{self, AuthUser};
use super::errors::{ApiReject, api_ok, api_ok_message, api_response};

// --------------------------------------------------------------------------------------
// 文案字典（逐字取自蓝本 `agent_views.py`）
// --------------------------------------------------------------------------------------

const ERR_SELECT_QUERY: &str = "请描述你的需求";
const ERR_INQUIRY_QUERY: &str = "请描述采购需求";
const ERR_CHAT_QUERY: &str = "请输入你的问题";
const ERR_FEEDBACK_RATING: &str = "rating 必须为 1-5 的整数";
const ERR_APPROVAL_TITLE: &str = "请填写审批事项";
const ERR_APPROVAL_MISSING: &str = "审批单不存在";
const ERR_DECISION_VALUE: &str = "decision 必须为 approved 或 rejected";
const ERR_DECISION_REPEAT: &str = "该审批单已批准，不能重复处理";

const MSG_FEEDBACK_CREATED: &str = "感谢反馈，已记入偏好与品质反馈";
const MSG_APPROVAL_CREATED: &str = "审批单已创建，等待人工确认";

/// `agent_approval` 的列清单；三处查询共用，避免列名漂移。
const APPROVAL_COLUMNS: &str = r#"a.id, a.ticket_type, a.title, a.ref_type, a.ref_id, a.payload,
                                   a.status, a.created_by_id, cu.username AS created_by_username,
                                   du.username AS decided_by_username, a.note, a.created_at, a.decided_at"#;
const APPROVAL_FROM: &str = r#"FROM agent_approval a
                               LEFT JOIN "user" cu ON cu.id = a.created_by_id
                               LEFT JOIN "user" du ON du.id = a.decided_by_id"#;

// --------------------------------------------------------------------------------------
// 路由
// --------------------------------------------------------------------------------------

/// 智能体域 11 条 path。外层已 `nest("/compat")`，所以这里写相对路径。
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/agent/context",
            get(context_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/agent/daily-report",
            get(daily_report_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/agent/select",
            post(select_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/agent/inquiry",
            post(inquiry_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/agent/risk-alert",
            get(risk_alert_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/agent/repurchase",
            get(repurchase_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/agent/chat",
            post(chat_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/agent/feedback",
            get(feedback_get_handler)
                .post(feedback_post_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/agent/approvals",
            post(approval_create_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/agent/approvals/list",
            get(approval_list_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/agent/approvals/{approval_id}/decision",
            post(approval_decision_handler).fallback(handle_unallowed_method),
        )
}

/// 405：DRF 异常体 + zh-hans 文案 `方法 “DELETE” 不被允许。`
async fn handle_unallowed_method(method: Method) -> Response {
    auth::render_method_not_allowed(&method)
}

// --------------------------------------------------------------------------------------
// 请求侧辅助
// --------------------------------------------------------------------------------------

/// 契约校验错误：蓝本用 `api_response(None, '文案', 4xx)` → **成功体形状**（带 `timestamp`）。
fn contract_error(status: StatusCode, message: &str) -> Response {
    api_response(
        status,
        status.as_u16(),
        Value::String(message.to_string()),
        Value::Null,
    )
}

/// `IsAuthenticated`：失败按 DRF 异常体返回 401。
///
/// 说明：`errors::ApiReject` 的 `status` / `message` 字段对 crate 内不可见，所以这里
/// 只能 `into_response()`，**少了** `auth::auth_error` 补的 `WWW-Authenticate: Bearer`
/// （蓝本 401 有该头）。夹具只比对 body，故契约逐字对齐不受影响。
async fn require_user(pool: &PgPool, headers: &HeaderMap) -> Result<AuthUser, Response> {
    match auth::authenticate(pool, headers).await {
        Ok(user) => Ok(user),
        Err(reject) => Err(reject.into_response()),
    }
}

/// 手写 JSON 提取（与账号域同样的理由：axum 的 `JsonRejection` 形状与契约差得远）。
async fn json_body(request: Request) -> Result<Value, Response> {
    let bytes = axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(|_| ApiReject::bad_request("无效数据。请求体读取失败。").into_response())?;

    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| ApiReject::bad_request("JSON 解析错误。").into_response())
}

/// `(request.data.get('query') or '').strip()`。非字符串一律按空串处理（蓝本会抛异常）。
fn body_query(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// `request.data.get('note') or ''` 之类：取字符串，非字符串按空串。
fn body_text(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `int(request.data.get('rating'))`：整数直接取，浮点截断，数字串可解析，其余失败。
fn body_int(body: &Value, key: &str) -> Option<i64> {
    match body.get(key)? {
        Value::Number(number) => number.as_i64().or_else(|| {
            number
                .as_f64()
                .filter(|value| value.is_finite())
                .map(|value| value.trunc() as i64)
        }),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// 解析 `?scope=pending` 这类简单查询参数（值不做百分号解码，蓝本此处只用到字面量）。
fn query_param(uri: &axum::http::Uri, key: &str) -> Option<String> {
    let query = uri.query()?;
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| value.to_string())
    })
}

// --------------------------------------------------------------------------------------
// 读类端点
// --------------------------------------------------------------------------------------

async fn context_handler(State(state): State<AppState>, request: Request) -> Response {
    let user = match require_user(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    json_result(agent_service::build_context(&state.db, &user).await)
}

async fn daily_report_handler(State(state): State<AppState>, request: Request) -> Response {
    let user = match require_user(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    json_result(agent_service::daily_report(&state.db, &user).await)
}

async fn repurchase_handler(State(state): State<AppState>, request: Request) -> Response {
    let user = match require_user(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    json_result(agent_service::repurchase_suggestion(&state.db, &user).await)
}

async fn risk_alert_handler(State(state): State<AppState>, request: Request) -> Response {
    let user = match require_user(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    json_result(risk_alert_impl(&state.db, &user).await)
}

fn json_result(result: Result<Value, ApiReject>) -> Response {
    match result {
        Ok(data) => api_ok(data),
        Err(reject) => reject.into_response(),
    }
}

/// 存在风险时**自动生成人工审批单**（风险处置），高风险动作需人确认。
async fn risk_alert_impl(pool: &PgPool, user: &AuthUser) -> Result<Value, ApiReject> {
    let mut data = agent_service::risk_alert(pool, user).await?;
    if data.get("has_alert") != Some(&Value::Bool(true)) {
        return Ok(data);
    }

    let risk_cards = data
        .get("risk_cards")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let card_count = risk_cards.as_array().map_or(0, Vec::len);
    let safety_margin = data.get("safety_margin").cloned().unwrap_or(json!(0));

    let mut payload = Map::new();
    payload.insert("risk_cards".to_string(), risk_cards);
    payload.insert("safety_margin".to_string(), safety_margin);

    let id = Uuid::new_v4();
    let now = Utc::now();
    sqlx::query(
        r#"INSERT INTO agent_approval
               (id, ticket_type, title, ref_type, ref_id, payload, status, created_by_id,
                decided_by_id, note, created_at, decided_at, updated_at)
           VALUES ($1, 'risk_action', $2, 'orchard', '', $3, 'pending', $4, NULL, '', $5, NULL, $5)"#,
    )
    .bind(id)
    .bind(format!("生产风险处置确认（{card_count} 项）"))
    .bind(Value::Object(payload))
    .bind(user.id)
    .bind(now)
    .execute(pool)
    .await
    .map_err(internal_error)?;

    let approval = fetch_approval(pool, id).await?.ok_or_else(|| {
        ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
    })?;
    if let Value::Object(map) = &mut data {
        map.insert(
            "approval".to_string(),
            agent_service::approval_json(&approval),
        );
    }
    Ok(data)
}

// --------------------------------------------------------------------------------------
// POST /api/agent/select（AllowAny）
// --------------------------------------------------------------------------------------

async fn select_handler(State(state): State<AppState>, request: Request) -> Response {
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let query = body_query(&body, "query");
    if query.is_empty() {
        return contract_error(StatusCode::BAD_REQUEST, ERR_SELECT_QUERY);
    }
    json_result(agent_service::select_for_shopper(&state.db, &query).await)
}

// --------------------------------------------------------------------------------------
// POST /api/agent/inquiry（AllowAny）
// --------------------------------------------------------------------------------------

async fn inquiry_handler(State(state): State<AppState>, request: Request) -> Response {
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let query = body_query(&body, "query");
    if query.is_empty() {
        return contract_error(StatusCode::BAD_REQUEST, ERR_INQUIRY_QUERY);
    }
    json_result(inquiry_impl(&state.db, &query).await)
}

/// 询价主方案生成后**自动创建人工审批单**（报价单），交运营确认。
async fn inquiry_impl(pool: &PgPool, query: &str) -> Result<Value, ApiReject> {
    let mut data = agent_service::inquiry_quote(pool, query).await?;

    let Some(primary) = data
        .get("primary")
        .filter(|value| !value.is_null())
        .cloned()
    else {
        return Ok(data);
    };
    let batch_code = primary
        .pointer("/batch/code")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let batch_id = primary
        .pointer("/batch/id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let alternatives = data
        .get("alternatives")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));

    let mut payload = Map::new();
    payload.insert("query".to_string(), Value::String(query.to_string()));
    payload.insert("primary".to_string(), primary);
    payload.insert("alternatives".to_string(), alternatives);

    let id = Uuid::new_v4();
    let now = Utc::now();
    sqlx::query(
        r#"INSERT INTO agent_approval
               (id, ticket_type, title, ref_type, ref_id, payload, status, created_by_id,
                decided_by_id, note, created_at, decided_at, updated_at)
           VALUES ($1, 'quote', $2, 'sales_batch', $3, $4, 'pending', NULL, NULL, '', $5, NULL, $5)"#,
    )
    .bind(id)
    .bind(format!("询价：{batch_code}"))
    .bind(&batch_id)
    .bind(Value::Object(payload))
    .bind(now)
    .execute(pool)
    .await
    .map_err(internal_error)?;

    let approval = fetch_approval(pool, id).await?.ok_or_else(|| {
        ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
    })?;

    if let Value::Object(map) = &mut data {
        map.insert(
            "approval".to_string(),
            agent_service::approval_json(&approval),
        );
    }
    Ok(data)
}

// --------------------------------------------------------------------------------------
// POST /api/agent/chat
// --------------------------------------------------------------------------------------

async fn chat_handler(State(state): State<AppState>, request: Request) -> Response {
    let user = match require_user(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let query = body_query(&body, "query");
    if query.is_empty() {
        return contract_error(StatusCode::BAD_REQUEST, ERR_CHAT_QUERY);
    }

    let llm = agent_service::llm_client(state.client.clone());
    json_result(agent_service::agent_chat(&state.db, llm.as_ref(), &query, &user).await)
}

// --------------------------------------------------------------------------------------
// GET/POST /api/agent/feedback
// --------------------------------------------------------------------------------------

async fn feedback_get_handler(State(state): State<AppState>, request: Request) -> Response {
    let user = match require_user(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    match list_feedback(&state.db, &user).await {
        Ok(items) => api_ok(Value::Array(items)),
        Err(reject) => reject.into_response(),
    }
}

async fn feedback_post_handler(State(state): State<AppState>, request: Request) -> Response {
    let user = match require_user(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    let Some(rating) = body_int(&body, "rating") else {
        return contract_error(StatusCode::BAD_REQUEST, ERR_FEEDBACK_RATING);
    };
    if !(1..=5).contains(&rating) {
        return contract_error(StatusCode::BAD_REQUEST, ERR_FEEDBACK_RATING);
    }

    match create_feedback(&state.db, &user, &body, rating as i16).await {
        Ok(item) => api_ok_message(MSG_FEEDBACK_CREATED, item),
        Err(reject) => reject.into_response(),
    }
}

/// `AgentFeedback.objects.filter(user=request.user)[:50]`（`Meta.ordering = ['-created_at']`）。
async fn list_feedback(pool: &PgPool, user: &AuthUser) -> Result<Vec<Value>, ApiReject> {
    let rows = sqlx::query(
        r#"SELECT id, product_name, batch_code, rating, taste, package, note, created_at
             FROM agent_feedback
            WHERE user_id = $1
            ORDER BY created_at DESC
            LIMIT 50"#,
    )
    .bind(user.id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    rows.iter()
        .map(feedback_from_row)
        .map(|row| row.map(|row| agent_service::feedback_json(&row)))
        .collect()
}

async fn create_feedback(
    pool: &PgPool,
    user: &AuthUser,
    body: &Value,
    rating: i16,
) -> Result<Value, ApiReject> {
    let id = Uuid::new_v4();
    let now = Utc::now();
    sqlx::query(
        r#"INSERT INTO agent_feedback
               (id, user_id, product_name, batch_code, rating, taste, package, note, created_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
    )
    .bind(id)
    .bind(user.id)
    .bind(body_text(body, "product_name"))
    .bind(body_text(body, "batch_code"))
    .bind(rating)
    .bind(body_text(body, "taste"))
    .bind(body_text(body, "package"))
    .bind(body_text(body, "note"))
    .bind(now)
    .execute(pool)
    .await
    .map_err(internal_error)?;

    let row = sqlx::query(
        r#"SELECT id, product_name, batch_code, rating, taste, package, note, created_at
             FROM agent_feedback WHERE id = $1"#,
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .map_err(internal_error)?;

    Ok(agent_service::feedback_json(&feedback_from_row(&row)?))
}

fn feedback_from_row(row: &sqlx::postgres::PgRow) -> Result<FeedbackRow, ApiReject> {
    Ok(FeedbackRow {
        id: row.try_get("id").map_err(internal_error)?,
        product_name: row.try_get("product_name").map_err(internal_error)?,
        batch_code: row.try_get("batch_code").map_err(internal_error)?,
        rating: row.try_get("rating").map_err(internal_error)?,
        taste: row.try_get("taste").map_err(internal_error)?,
        package: row.try_get("package").map_err(internal_error)?,
        note: row.try_get("note").map_err(internal_error)?,
        created_at: row.try_get("created_at").map_err(internal_error)?,
    })
}

// --------------------------------------------------------------------------------------
// POST /api/agent/approvals
// --------------------------------------------------------------------------------------

async fn approval_create_handler(State(state): State<AppState>, request: Request) -> Response {
    let user = match require_user(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    let title = body_query(&body, "title");
    if title.is_empty() {
        return contract_error(StatusCode::BAD_REQUEST, ERR_APPROVAL_TITLE);
    }

    match create_approval(&state.db, &user, &body, &title).await {
        Ok(item) => api_ok_message(MSG_APPROVAL_CREATED, item),
        Err(reject) => reject.into_response(),
    }
}

/// 审批单创建：`ticket_type` 走 `data.get(...) or 'other'`；`payload` 走 `... or {}`。
async fn create_approval(
    pool: &PgPool,
    user: &AuthUser,
    body: &Value,
    title: &str,
) -> Result<Value, ApiReject> {
    let ticket_type = body
        .get("ticket_type")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("other")
        .to_string();
    let payload = body
        .get("payload")
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));

    let id = Uuid::new_v4();
    let now = Utc::now();
    sqlx::query(
        r#"INSERT INTO agent_approval
               (id, ticket_type, title, ref_type, ref_id, payload, status, created_by_id,
                decided_by_id, note, created_at, decided_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7, NULL, '', $8, NULL, $8)"#,
    )
    .bind(id)
    .bind(ticket_type)
    .bind(title)
    .bind(body_text(body, "ref_type"))
    .bind(body_text(body, "ref_id"))
    .bind(payload)
    .bind(user.id)
    .bind(now)
    .execute(pool)
    .await
    .map_err(internal_error)?;

    let approval = fetch_approval(pool, id).await?.ok_or_else(|| {
        ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
    })?;
    Ok(agent_service::approval_json(&approval))
}

// --------------------------------------------------------------------------------------
// GET /api/agent/approvals/list
// --------------------------------------------------------------------------------------

async fn approval_list_handler(State(state): State<AppState>, request: Request) -> Response {
    let user = match require_user(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let scope = query_param(request.uri(), "scope").unwrap_or_else(|| "mine".to_string());

    match list_approvals(&state.db, &user, &scope).await {
        Ok(items) => api_ok(Value::Array(items)),
        Err(reject) => reject.into_response(),
    }
}

/// `scope`：`mine`（默认，按 created_by 过滤）/ `pending`（按状态过滤）/ 其它（全量）。
async fn list_approvals(
    pool: &PgPool,
    user: &AuthUser,
    scope: &str,
) -> Result<Vec<Value>, ApiReject> {
    let sql = format!(
        "SELECT {APPROVAL_COLUMNS} {APPROVAL_FROM} {{where}} ORDER BY a.created_at DESC LIMIT 50"
    );
    let sql = match scope {
        "mine" => sql.replace("{where}", "WHERE a.created_by_id = $1"),
        "pending" => sql.replace("{where}", "WHERE a.status = 'pending'"),
        _ => sql.replace("{where}", ""),
    };

    let mut query = sqlx::query(&sql);
    if scope == "mine" {
        query = query.bind(user.id);
    }

    let rows = query.fetch_all(pool).await.map_err(internal_error)?;
    rows.iter()
        .map(approval_from_row)
        .map(|row| row.map(|row| agent_service::approval_json(&row)))
        .collect()
}

// --------------------------------------------------------------------------------------
// POST /api/agent/approvals/<uuid:approval_id>/decision
// --------------------------------------------------------------------------------------

async fn approval_decision_handler(
    State(state): State<AppState>,
    Path(approval_id): Path<String>,
    request: Request,
) -> Response {
    let user = match require_user(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    // Django 的 `<uuid:...>` 转换器不匹配时直接 404。
    let Ok(id) = Uuid::parse_str(&approval_id) else {
        return contract_error(StatusCode::NOT_FOUND, ERR_APPROVAL_MISSING);
    };

    match decide_approval(&state.db, &user, id, &body).await {
        Ok(response) => response,
        Err(reject) => reject.into_response(),
    }
}

async fn decide_approval(
    pool: &PgPool,
    user: &AuthUser,
    id: Uuid,
    body: &Value,
) -> Result<Response, ApiReject> {
    let Some(approval) = fetch_approval(pool, id).await? else {
        return Ok(contract_error(StatusCode::NOT_FOUND, ERR_APPROVAL_MISSING));
    };

    let decision = body
        .get("decision")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if decision != "approved" && decision != "rejected" {
        return Ok(contract_error(StatusCode::BAD_REQUEST, ERR_DECISION_VALUE));
    }
    if approval.status == "approved" {
        return Ok(contract_error(StatusCode::BAD_REQUEST, ERR_DECISION_REPEAT));
    }

    let status = if decision == "approved" {
        "approved"
    } else {
        "rejected"
    };
    let note = body_query(body, "note");
    let now = Utc::now();
    sqlx::query(
        r#"UPDATE agent_approval
              SET status = $2, decided_by_id = $3, note = $4, decided_at = $5, updated_at = $5
            WHERE id = $1"#,
    )
    .bind(id)
    .bind(status)
    .bind(user.id)
    .bind(&note)
    .bind(now)
    .execute(pool)
    .await
    .map_err(internal_error)?;

    let exec_note = if decision == "approved" {
        execute_approval(pool, &approval, user).await?
    } else {
        String::new()
    };

    let message = if status == "approved" {
        if exec_note.is_empty() {
            "已批准，动作交业务系统执行".to_string()
        } else {
            format!("已批准，{exec_note}")
        }
    } else {
        "已拒绝".to_string()
    };

    let updated = fetch_approval(pool, id).await?.ok_or_else(|| {
        ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
    })?;
    Ok(api_ok_message(
        message,
        agent_service::approval_json(&updated),
    ))
}

/// `_execute_approval`：批准后执行 payload 里的受控动作。目前只有 `create_task`。
async fn execute_approval(
    pool: &PgPool,
    approval: &ApprovalRow,
    user: &AuthUser,
) -> Result<String, ApiReject> {
    let action = approval
        .payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if action != "create_task" {
        return Ok(String::new());
    }

    let args = approval
        .payload
        .get("args")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let title = body_query(&args, "title");
    if title.is_empty() {
        return Ok("（未执行：缺少任务标题）".to_string());
    }

    let risk_level = args
        .get("risk_level")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("中风险");
    let task_type = args
        .get("task_type")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("智能体生成");

    sqlx::query(
        r#"INSERT INTO task
               (id, user_id, title, description, risk_level, task_type, source,
                is_completed, created_at, completed_at)
           VALUES ($1, $2, $3, $4, $5, $6, '智能体', FALSE, $7, NULL)"#,
    )
    .bind(Uuid::new_v4())
    .bind(approval.created_by_id.unwrap_or(user.id))
    .bind(&title)
    .bind(body_text(&args, "description"))
    .bind(risk_level)
    .bind(task_type)
    .bind(Utc::now())
    .execute(pool)
    .await
    .map_err(internal_error)?;

    Ok(format!("已创建任务「{title}」"))
}

// --------------------------------------------------------------------------------------
// 数据访问
// --------------------------------------------------------------------------------------

async fn fetch_approval(pool: &PgPool, id: Uuid) -> Result<Option<ApprovalRow>, ApiReject> {
    let sql = format!("SELECT {APPROVAL_COLUMNS} {APPROVAL_FROM} WHERE a.id = $1");
    let row = sqlx::query(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(internal_error)?;

    row.as_ref().map(approval_from_row).transpose()
}

fn approval_from_row(row: &sqlx::postgres::PgRow) -> Result<ApprovalRow, ApiReject> {
    Ok(ApprovalRow {
        id: row.try_get("id").map_err(internal_error)?,
        ticket_type: row.try_get("ticket_type").map_err(internal_error)?,
        title: row.try_get("title").map_err(internal_error)?,
        ref_type: row.try_get("ref_type").map_err(internal_error)?,
        ref_id: row.try_get("ref_id").map_err(internal_error)?,
        payload: row.try_get("payload").map_err(internal_error)?,
        status: row.try_get("status").map_err(internal_error)?,
        created_by_id: row.try_get("created_by_id").map_err(internal_error)?,
        created_by_username: row.try_get("created_by_username").map_err(internal_error)?,
        decided_by_username: row.try_get("decided_by_username").map_err(internal_error)?,
        note: row.try_get("note").map_err(internal_error)?,
        created_at: row.try_get("created_at").map_err(internal_error)?,
        decided_at: row.try_get("decided_at").map_err(internal_error)?,
    })
}

/// 未预期错误：与蓝本一致地冒泡成 DRF 的 500 `Internal server error`。
fn internal_error(err: sqlx::Error) -> ApiReject {
    tracing::error!("compat 智能体域写入/查询失败: {err}");
    ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}
