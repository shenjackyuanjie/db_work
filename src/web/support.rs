//! 网页客服会话超集（网页独有，Django 无对应模型）。
//!
//! 负责的 `/web/*` 路径（外层已 `nest("/web")`，此处写相对路径）：
//!
//! | 路径 | 方法 | 旧路径 | 前端引用 |
//! |---|---|---|---|
//! | `/support` | GET + POST | `/user/store/support` | `store-support.js:19`、经它被 `cart.js` 引用 |
//! | `/admin/support` | GET + POST | `/user/admin/store/support` | `store-admin.js:188,203,375`、`admin.js` |
//!
//! 表：保留自研的 `store_support_messages`（Django 从未建模）。
//!
//! # ⚠️ 本文件的响应是**裸 JSON**，不是超集信封
//!
//! `store-support.js:18-31` 的 `request()` 是 `return b`（**不做 `b.data ?? b`**），
//! 随后 `:40` 直接读 `b.messages`；错误分支 `:28` 读顶层 `b.message`。
//! 一旦套上 `{code,message,data,timestamp}` 信封，`b.messages` 就是 `undefined`，
//! 面板会直接 TypeError。所以这里**故意不用** `session::app_ok/app_err`。
//! （`store-admin.js:40-50` 是 `b.data ?? b`，两种都吃，但保持一致更重要。）

use axum::Router;
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::server::AppState;
use crate::web::session::{current_user, require_admin};

/// 单条会话最多回放的消息数，与旧实现一致。
const MESSAGE_LIMIT: i64 = 200;
/// 单条消息长度上限（`store-support.js` 的 textarea 也是 2000）。
const CONTENT_MAX_CHARS: usize = 2000;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/support", get(customer_messages).post(customer_send))
        .route("/admin/support", get(admin_messages).post(admin_send))
}

// --------------------------------------------------------------------------------------
// 裸 JSON 响应装配（见文件头注释：这里不能用信封）
// --------------------------------------------------------------------------------------

fn ok_json(value: Value) -> Response {
    (StatusCode::OK, Json(value)).into_response()
}

fn err_json(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "message": message.into() }))).into_response()
}

fn db_error(context: &str, error: &sqlx::Error) -> Response {
    tracing::error!("客服会话查询失败({context}): {error}");
    err_json(
        StatusCode::INTERNAL_SERVER_ERROR,
        "数据读取或保存失败，请稍后重试",
    )
}

/// 把 `json_agg(...)::text` 的结果解析回 JSON。
async fn rows(state: &AppState, sql: &str, username: &str) -> Result<Value, Response> {
    let text = sqlx::query_scalar::<_, String>(sql)
        .bind(username)
        .fetch_one(&state.db)
        .await
        .map_err(|err| db_error("rows", &err))?;

    serde_json::from_str(&text)
        .map_err(|_| err_json(StatusCode::INTERNAL_SERVER_ERROR, "数据格式错误"))
}

/// 某个用户的全部消息（按 id 升序，前端按时间顺序渲染）。
const MESSAGES_SQL: &str = "SELECT COALESCE(json_agg(t ORDER BY t.id), '[]'::json)::text \
     FROM (SELECT id, is_staff, content, created_at FROM store_support_messages \
     WHERE username=$1 ORDER BY id DESC LIMIT 200) t";

/// 管理端无 `username` 参数时的会话列表（每个用户只取最新一条）。
const CONVERSATIONS_SQL: &str = "SELECT COALESCE(json_agg(t ORDER BY t.id DESC), '[]'::json)::text \
     FROM (SELECT DISTINCT ON (username) username, id, content, is_staff, created_at \
     FROM store_support_messages WHERE $1='' ORDER BY username, id DESC) t";

#[derive(Debug, Deserialize)]
pub(crate) struct ConversationQuery {
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessageRequest {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub content: String,
}

/// 消息体校验；返回规范化后的内容。
fn validate_content(content: &str) -> Result<String, Response> {
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.chars().count() > CONTENT_MAX_CHARS {
        return Err(err_json(
            StatusCode::BAD_REQUEST,
            format!("消息需为 1 至 {CONTENT_MAX_CHARS} 字"),
        ));
    }
    Ok(trimmed.to_string())
}

async fn insert_message(
    state: &AppState,
    username: &str,
    actor: &str,
    is_staff: bool,
    content: &str,
) -> Result<(), Response> {
    sqlx::query(
        "INSERT INTO store_support_messages (username, actor, is_staff, content, created_at) \
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(username)
    .bind(actor)
    .bind(is_staff)
    .bind(content)
    .bind(crate::server::now_millis() as i64)
    .execute(&state.db)
    .await
    .map_err(|err| db_error("insert", &err))?;

    Ok(())
}

// --------------------------------------------------------------------------------------
// 处理器
// --------------------------------------------------------------------------------------

/// `GET /web/support`：当前登录用户自己的会话消息。
pub(crate) async fn customer_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = current_user(&state, &headers).await else {
        return err_json(StatusCode::UNAUTHORIZED, "请先登录");
    };

    match rows(&state, MESSAGES_SQL, &user.username).await {
        Ok(messages) => ok_json(json!({ "messages": messages })),
        Err(response) => response,
    }
}

/// `POST /web/support`：用户发言。
pub(crate) async fn customer_send(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<MessageRequest>,
) -> Response {
    let Some(user) = current_user(&state, &headers).await else {
        return err_json(StatusCode::UNAUTHORIZED, "请先登录");
    };

    let content = match validate_content(&body.content) {
        Ok(content) => content,
        Err(response) => return response,
    };

    match insert_message(&state, &user.username, &user.username, false, &content).await {
        Ok(()) => ok_json(json!({ "message": "发送成功" })),
        Err(response) => response,
    }
}

/// `GET /web/admin/support`：带 `?username=` 看某个用户的会话，否则看会话列表。
pub(crate) async fn admin_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ConversationQuery>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    if let Some(username) = query.username.as_deref().filter(|v| !v.trim().is_empty()) {
        return match rows(&state, MESSAGES_SQL, username.trim()).await {
            Ok(messages) => ok_json(json!({ "messages": messages })),
            Err(response) => response,
        };
    }

    match rows(&state, CONVERSATIONS_SQL, "").await {
        Ok(conversations) => ok_json(json!({ "conversations": conversations })),
        Err(response) => response,
    }
}

/// `POST /web/admin/support`：客服回复某个用户。
pub(crate) async fn admin_send(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<MessageRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(response) => return response,
    };

    let username = body.username.unwrap_or_default().trim().to_string();
    let content = match validate_content(&body.content) {
        Ok(content) => content,
        Err(response) => return response,
    };

    // 只能回复已存在的会话，避免凭空造出孤儿对话（与旧实现一致）。
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM store_support_messages WHERE username=$1)",
    )
    .bind(&username)
    .fetch_one(&state.db)
    .await;

    match exists {
        Ok(true) => {}
        Ok(false) => return err_json(StatusCode::NOT_FOUND, "会话不存在"),
        Err(err) => return db_error("exists", &err),
    }

    match insert_message(&state, &username, &admin.username, true, &content).await {
        Ok(()) => ok_json(json!({ "message": "发送成功" })),
        Err(response) => response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_must_be_one_to_two_thousand_chars() {
        assert!(validate_content("你好").is_ok());
        assert!(validate_content("  你好  ").is_ok());
        assert!(validate_content("").is_err());
        assert!(validate_content("   ").is_err());
    }

    /// 按**字符**而不是字节计数：2000 个汉字必须通过（`store-support.js` 的 maxlength 也是字符）。
    #[test]
    fn content_length_counts_characters_not_bytes() {
        let ok = "橙".repeat(CONTENT_MAX_CHARS);
        assert!(validate_content(&ok).is_ok());

        let too_long = "橙".repeat(CONTENT_MAX_CHARS + 1);
        assert!(validate_content(&too_long).is_err());
    }

    #[test]
    fn content_is_trimmed_before_length_check() {
        let padded = format!("  {}  ", "橙".repeat(CONTENT_MAX_CHARS));
        let content = validate_content(&padded).unwrap();
        assert_eq!(content.chars().count(), CONTENT_MAX_CHARS);
    }

    /// 裸 JSON 的错误体必须有顶层 `message`，`store-support.js:28` 就靠它显示。
    #[tokio::test]
    async fn error_body_is_bare_and_carries_message() {
        let response = err_json(StatusCode::BAD_REQUEST, "消息需为 1 至 2000 字");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["message"], "消息需为 1 至 2000 字");
        assert!(value.get("data").is_none(), "{value}");
        assert!(value.get("code").is_none(), "{value}");
    }

    /// 成功体也是裸的：`store-support.js:40` 直接读 `b.messages`。
    #[tokio::test]
    async fn success_body_is_bare_and_not_enveloped() {
        let response = ok_json(json!({ "messages": [] }));
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(value.get("messages").is_some(), "{value}");
        assert!(value.get("data").is_none(), "{value}");
    }
}
