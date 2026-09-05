use super::auth::{ensure_admin, ensure_authenticated};
use crate::server::{AppState, now_millis};
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;
fn auth_error((status, body): (StatusCode, Value)) -> (StatusCode, Json<Value>) {
    (status, Json(body))
}
fn db_error(error: sqlx::Error) -> (StatusCode, Json<Value>) {
    tracing::error!("Store workspace query failed: {}", error);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"message":"数据读取或保存失败，请稍后重试"})),
    )
}
async fn rows(
    state: &AppState,
    sql: &str,
    username: &str,
) -> Result<Value, (StatusCode, Json<Value>)> {
    let text = sqlx::query_scalar::<_, String>(sql)
        .bind(username)
        .fetch_one(&state.db)
        .await
        .map_err(db_error)?;
    serde_json::from_str(&text).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"message":"数据格式错误"})),
        )
    })
}
const MESSAGES: &str = "SELECT COALESCE(json_agg(t ORDER BY t.id), '[]'::json)::text FROM (SELECT id, is_staff, content, created_at FROM store_support_messages WHERE username=$1 ORDER BY id DESC LIMIT 200) t";
#[derive(Deserialize)]
pub(super) struct Conversation {
    username: Option<String>,
}
#[derive(Deserialize)]
pub(super) struct Message {
    username: Option<String>,
    content: String,
}

pub(super) async fn customer_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult {
    let (_, username) = ensure_authenticated(&state, &headers)
        .await
        .map_err(auth_error)?;
    Ok(Json(
        json!({"messages": rows(&state, MESSAGES, &username).await?}),
    ))
}
pub(super) async fn admin_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<Conversation>,
) -> ApiResult {
    ensure_admin(&state, &headers).await.map_err(auth_error)?;
    if let Some(username) = query.username {
        return Ok(Json(
            json!({"messages": rows(&state, MESSAGES, &username).await?}),
        ));
    }
    let conversations = rows(&state, "SELECT COALESCE(json_agg(t ORDER BY t.id DESC), '[]'::json)::text FROM (SELECT DISTINCT ON (username) username, id, content, is_staff, created_at FROM store_support_messages WHERE $1='' ORDER BY username, id DESC) t", "").await?;
    Ok(Json(json!({"conversations":conversations})))
}
async fn send(
    state: &AppState,
    username: &str,
    actor: &str,
    staff: bool,
    content: &str,
) -> ApiResult {
    let content = content.trim();
    if content.is_empty() || content.chars().count() > 2000 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"message":"消息需为 1 至 2000 字"})),
        ));
    }
    sqlx::query("INSERT INTO store_support_messages (username, actor, is_staff, content, created_at) VALUES ($1,$2,$3,$4,$5)")
        .bind(username).bind(actor).bind(staff).bind(content).bind(now_millis() as i64)
        .execute(&state.db).await.map_err(db_error)?;
    Ok(Json(json!({"message":"发送成功"})))
}
pub(super) async fn customer_send(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Message>,
) -> ApiResult {
    let (_, username) = ensure_authenticated(&state, &headers)
        .await
        .map_err(auth_error)?;
    send(&state, &username, &username, false, &body.content).await
}
pub(super) async fn admin_send(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Message>,
) -> ApiResult {
    let actor = ensure_admin(&state, &headers).await.map_err(auth_error)?;
    let username = body.username.unwrap_or_default();
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM store_support_messages WHERE username=$1)",
    )
    .bind(&username)
    .fetch_one(&state.db)
    .await
    .map_err(db_error)?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, Json(json!({"message":"会话不存在"}))));
    }
    send(&state, &username, &actor, true, &body.content).await
}
#[derive(Deserialize)]
pub(super) struct Period {
    days: Option<i32>,
}
pub(super) async fn analytics(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(period): Query<Period>,
) -> ApiResult {
    ensure_admin(&state, &headers).await.map_err(auth_error)?;
    let days = period.days.unwrap_or(7).clamp(1, 90);
    let sql = r#"WITH selected AS (
        SELECT *, (to_timestamp(created_at / 1000.0) AT TIME ZONE 'Asia/Shanghai')::date AS day FROM store_orders
        WHERE created_at >= (extract(epoch FROM (((now() AT TIME ZONE 'Asia/Shanghai')::date - ($1::int - 1))::timestamp AT TIME ZONE 'Asia/Shanghai')) * 1000)::bigint
    ) SELECT json_build_object(
      'summary', (SELECT json_build_object('orders',count(*),'revenue',COALESCE(sum(total_cents) FILTER (WHERE status IN ('paid','shipped','completed')),0),'buyers',count(DISTINCT username),'pending',count(*) FILTER(WHERE status='paid')) FROM selected),
      'trend', (SELECT json_agg(t ORDER BY t.day) FROM (SELECT d::date::text AS day, COALESCE(sum(s.total_cents) FILTER(WHERE s.status IN ('paid','shipped','completed')),0) AS revenue, count(s.id) AS orders FROM generate_series((now() AT TIME ZONE 'Asia/Shanghai')::date - ($1::int - 1), (now() AT TIME ZONE 'Asia/Shanghai')::date, interval '1 day') d LEFT JOIN selected s ON s.day=d::date GROUP BY d) t),
      'statuses', (SELECT COALESCE(json_agg(t),'[]'::json) FROM (SELECT status,count(*) AS count FROM selected GROUP BY status) t),
      'products', (SELECT COALESCE(json_agg(t),'[]'::json) FROM (SELECT i.product_id,i.product_name,sum(i.quantity) AS quantity,sum(i.quantity*i.unit_price_cents) AS revenue FROM store_order_items i JOIN selected s ON s.id=i.order_id WHERE s.status IN ('paid','shipped','completed') GROUP BY i.product_id,i.product_name ORDER BY revenue DESC LIMIT 8) t)
    )::text"#;
    let result = sqlx::query_scalar::<_, String>(sql)
        .bind(days)
        .fetch_one(&state.db)
        .await
        .map_err(db_error)?;
    let value: Value = serde_json::from_str(&result).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"message":"统计数据格式错误"})),
        )
    })?;
    Ok(Json(value))
}
