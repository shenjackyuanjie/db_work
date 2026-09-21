//! 网页后台管理超集：系统状态 / 系统设置 / 账号 / 邀请码 / 注册审批。
//!
//! 负责的 `/web/*` 路径（外层已 `nest("/web")`）：
//!
//! | 路径 | 方法 | 旧路径 | 前端引用 |
//! |---|---|---|---|
//! | `/system-status` | GET | `/api/system-status` | `index.js:143`（**登录页首屏**，不能留在 `/api/**`） |
//! | `/admin/settings/get` | POST | `/user/admin/settings/get` | `admin.js:1063` |
//! | `/admin/settings/update` | POST | `/user/admin/settings/update` | `admin.js:1079` |
//! | `/admin/set_admin` | POST | `/user/admin/set_admin` | `admin.js:165` |
//! | `/admin/users/list` | POST | `/user/admin/users/list` | `admin.js:277` |
//! | `/admin/invitations/create` | POST | `/user/admin/invitations/create` | `admin.js:146` |
//! | `/admin/invitations/list` | POST | `/user/admin/invitations/list` | `admin.js:266` |
//! | `/admin/pending/list` | POST | `/user/admin/pending/list` | `admin.js:255` |
//! | `/admin/pending/approve` | POST | `/user/admin/pending/approve` | `admin.js:288` |
//! | `/admin/pending/reject` | POST | `/user/admin/pending/reject` | `admin.js:300` |
//!
//! 表：保留 `app_system_settings`、`app_invitations`、`app_pending_users`；
//! 账号改为读写契约表 `"user"`，管理员标记用**加法列** `"user".is_admin`
//! （见 `src/server/bootstrap.rs`，不进 DRF 序列化）。
//!
//! # 响应形状由消费方 reader 决定（搞反会在前端爆 TypeError）
//!
//! - `admin.js` 全部用 `postJson(...)` → `unwrapApiPayload(resp.data)`，其实现是
//!   「有 `data` 就取 `data`，否则取整包」。所以**成功响应的 `data` 不能是 `null`**，
//!   否则它会回落成整个信封，前端读 `payload.users` 之类全部拿不到。
//! - 错误体：`readErrorMessage` 是 `payload?.error || payload?.message || ...`，
//!   所以信封里的 `message` 能被正确读到，不需要额外造 `error` 键。
//! - `/system-status` 走 `index.js:143` → `unwrapApiPayload` → `applySystemStatus`，
//!   读 `maintenance_mode` / `open_registration` / `invite_bypass_enabled` 三个键。
//!
//! # 两个必须遵守的接口细节
//!
//! 1. **列表/读取端点不接收请求体**。前端写的是 `postJson(url)`——`admin.js:110-118`
//!    在 `body === undefined` 时**既不发 body 也不发 `Content-Type`**。若这里挂了
//!    `Json<T>` extractor，axum 会因为缺 `Content-Type` 直接拒绝请求。
//! 2. **时间一律输出「秒」**。`admin.js:73` 的 `formatDate(unixSeconds)` 是按
//!    `new Date(unixSeconds * 1000)` 实现的，且 `> 4102444800` 会渲染成「永久有效」
//!    （邀请码 `ttl=0` 的哨兵值依赖这一点）。契约表 `"user".created_at` 是
//!    `TIMESTAMPTZ`，必须显式转成秒。

use axum::Router;
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::server::AppState;
use crate::system_settings::{
    SystemSettings, append_audit_log, load_system_settings, prune_audit_logs,
    update_system_settings,
};
use crate::web::session::{app_err, app_ok, require_admin};

/// 网页注册/审批放行的用户一律落在果农侧，与 `web::session::WEB_DEFAULT_ROLE` 同一取值
/// （网页是果园运营端；`"user".role` 只有 `farmer`/`buyer`）。
const WEB_DEFAULT_ROLE: &str = "farmer";

/// `formatDate` 把它当作「永久有效」的阈值，邀请码 `ttl_seconds == 0` 时用它。
const PERMANENT_INVITE_EXPIRY: i64 = i64::MAX;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        // 匿名可达：登录页首屏就要用，加鉴权会让未登录用户看不到维护/注册开关状态。
        //
        // 形状与旧 `/api/system-status` **逐字段同形**（同一张表、同一个 loader、
        // 同一组 6 个键、同样是 api_success 信封），见 `system_status_handler` 的说明。
        .route("/system-status", get(system_status_handler))
        .route("/admin/settings/get", post(get_settings_handler))
        .route("/admin/settings/update", post(update_settings_handler))
        .route("/admin/set_admin", post(set_admin_handler))
        .route("/admin/users/list", post(list_users_handler))
        .route("/admin/invitations/create", post(create_invitation_handler))
        .route("/admin/invitations/list", post(list_invitations_handler))
        .route("/admin/pending/list", post(list_pending_handler))
        .route("/admin/pending/approve", post(approve_pending_handler))
        .route("/admin/pending/reject", post(reject_pending_handler))
}

// --------------------------------------------------------------------------------------
// 请求体（注意：只有带 body 的端点才用得上）
// --------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateSettingsRequest {
    pub open_registration: bool,
    pub invite_bypass_enabled: bool,
    pub maintenance_mode: bool,
    pub default_invite_ttl_seconds: i64,
    pub confidence_threshold: f64,
    pub log_retention_days: i32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SetAdminRequest {
    #[serde(default)]
    pub target_username: String,
    #[serde(default)]
    pub make_admin: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateInvitationRequest {
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UsernameRequest {
    #[serde(default)]
    pub username: String,
}

// --------------------------------------------------------------------------------------
// 纯函数：形状与单位转换
// --------------------------------------------------------------------------------------

/// 与旧 `user_routes::parse_requested_role` 同一规则（返回前端认识的 `admin` / `user`）。
fn normalize_requested_role(raw: &str) -> &'static str {
    if raw.trim().eq_ignore_ascii_case("admin") {
        "admin"
    } else {
        "user"
    }
}

/// 与旧 `user_routes::admin::common::system_settings_payload` 逐字段一致的形状。
fn settings_payload(settings: &SystemSettings) -> Value {
    json!({
        "open_registration": settings.open_registration,
        "invite_bypass_enabled": settings.invite_bypass_enabled,
        "maintenance_mode": settings.maintenance_mode,
        "default_invite_ttl_seconds": settings.default_invite_ttl_seconds,
        "confidence_threshold": settings.confidence_threshold,
        "log_retention_days": settings.log_retention_days,
        "updated_at": settings.updated_at,
        "updated_by": settings.updated_by,
    })
}

fn now_secs() -> i64 {
    (crate::server::now_millis() / 1000).max(0) as i64
}

fn database_error(context: &str, err: impl std::fmt::Display) -> Response {
    app_err(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("{context}: {err}"),
        Value::Null,
    )
}

/// 统一的「必须管理员」前置；失败时返回已渲染好的响应。
macro_rules! admin_or_return {
    ($state:expr, $headers:expr) => {
        match require_admin(&$state, &$headers).await {
            Ok(user) => user,
            Err(response) => return response,
        }
    };
}

// --------------------------------------------------------------------------------------
// 系统状态（匿名）
// --------------------------------------------------------------------------------------

/// `GET /web/system-status`
///
/// 与旧 `handlers_core::system_status_api_handler`（原在 `handlers_core/pages.rs:120`，
/// G2 已随 `/api/system-status` 路由一起删除）
/// **逐字段同形**：同一张 `app_system_settings`、同一个 `load_system_settings`、
/// 同一组 6 个键、同样是 `api_success` 信封、失败同样回信封。
///
/// 为什么这里复刻而不是直接挂载旧 handler：`server::handlers_core` 的**外层 `mod`
/// 仍是私有的**（只有 `handlers_ai` 被开到了 `pub(crate)`），而 `src/web/**` 是 `server`
/// 的兄弟模块，直接引用会 `E0603`。按纪律**不复制业务代码绕过可见性**，也不自改
/// `src/server.rs`——这里只是 10 行、读同一份 `SystemSettings`，不构成第二份业务逻辑。
/// 若主线之后把 `mod handlers_core` 也开成 `pub(crate)`，可以换成直接挂载。
async fn system_status_handler(State(state): State<AppState>) -> Response {
    match load_system_settings(&state.db).await {
        Ok(settings) => app_ok(
            "success",
            json!({
                "open_registration": settings.open_registration,
                "invite_bypass_enabled": settings.invite_bypass_enabled,
                "maintenance_mode": settings.maintenance_mode,
                "default_invite_ttl_seconds": settings.default_invite_ttl_seconds,
                "confidence_threshold": settings.confidence_threshold,
                "log_retention_days": settings.log_retention_days,
            }),
        ),
        Err(err) => app_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to load system status: {}", err),
            Value::Null,
        ),
    }
}

// --------------------------------------------------------------------------------------
// 系统设置
// --------------------------------------------------------------------------------------

async fn get_settings_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let _admin = admin_or_return!(state, headers);

    match load_system_settings(&state.db).await {
        Ok(settings) => app_ok(
            "success",
            json!({ "settings": settings_payload(&settings) }),
        ),
        Err(err) => database_error("Failed to load settings", err),
    }
}

async fn update_settings_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<UpdateSettingsRequest>,
) -> Response {
    let admin = admin_or_return!(state, headers);

    let requested = SystemSettings {
        open_registration: payload.open_registration,
        invite_bypass_enabled: payload.invite_bypass_enabled,
        maintenance_mode: payload.maintenance_mode,
        default_invite_ttl_seconds: payload.default_invite_ttl_seconds,
        confidence_threshold: payload.confidence_threshold,
        log_retention_days: payload.log_retention_days,
        updated_at: 0,
        updated_by: Some(admin.username.clone()),
    };

    let updated = match update_system_settings(&state.db, requested, Some(&admin.username)).await {
        Ok(settings) => settings,
        Err(err) => return database_error("Failed to update settings", err),
    };

    let _ = append_audit_log(
        &state.db,
        "action",
        Some(&admin.username),
        &format!(
            "管理员 {} 更新系统设置：开放注册={}，邀请码免审={}，维护模式={}，默认邀请码TTL={}秒，置信度阈值={:.0}%，日志保留={}天",
            admin.username,
            updated.open_registration,
            updated.invite_bypass_enabled,
            updated.maintenance_mode,
            updated.default_invite_ttl_seconds,
            updated.confidence_threshold_percent(),
            updated.log_retention_days,
        ),
    )
    .await;

    // 与旧实现一致：保存设置时顺手按新的保留天数清理审计日志。
    let _ = prune_audit_logs(&state.db, updated.log_retention_days).await;

    app_ok("success", json!({ "settings": settings_payload(&updated) }))
}

// --------------------------------------------------------------------------------------
// 账号
// --------------------------------------------------------------------------------------

async fn set_admin_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<SetAdminRequest>,
) -> Response {
    let admin = admin_or_return!(state, headers);

    let target = payload.target_username.trim();
    if target.is_empty() {
        return app_err(
            StatusCode::BAD_REQUEST,
            "target_username is required",
            Value::Null,
        );
    }

    // `"user"` 是 PG 保留字，必须双引号。
    let result = match sqlx::query(r#"UPDATE "user" SET is_admin = $1 WHERE username = $2"#)
        .bind(payload.make_admin)
        .bind(target)
        .execute(&state.db)
        .await
    {
        Ok(result) => result,
        Err(err) => return database_error("Failed to update user", err),
    };

    if result.rows_affected() == 0 {
        return app_err(StatusCode::NOT_FOUND, "Target user not found", Value::Null);
    }

    let role_text = if payload.make_admin {
        "管理员"
    } else {
        "普通用户"
    };
    let _ = append_audit_log(
        &state.db,
        "action",
        Some(&admin.username),
        &format!(
            "管理员 {} 将用户 {} 调整为{}",
            admin.username, target, role_text
        ),
    )
    .await;

    app_ok("success", json!({ "status": "updated" }))
}

async fn list_users_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let _admin = admin_or_return!(state, headers);

    let rows = match sqlx::query(
        r#"SELECT username, is_admin, created_at FROM "user" ORDER BY created_at DESC"#,
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => return database_error("Failed to list users", err),
    };

    let users = rows
        .into_iter()
        .map(|row| {
            let created_at: DateTime<Utc> = row
                .try_get("created_at")
                .unwrap_or_else(|_| DateTime::<Utc>::UNIX_EPOCH);
            json!({
                "username": row.try_get::<String, _>("username").unwrap_or_default(),
                "is_admin": row.try_get::<bool, _>("is_admin").unwrap_or(false),
                "created_at": created_at.timestamp(),
            })
        })
        .collect::<Vec<_>>();

    app_ok("success", json!({ "users": users }))
}

// --------------------------------------------------------------------------------------
// 邀请码
// --------------------------------------------------------------------------------------

async fn create_invitation_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CreateInvitationRequest>,
) -> Response {
    let admin = admin_or_return!(state, headers);

    let settings = match load_system_settings(&state.db).await {
        Ok(settings) => settings,
        Err(err) => return database_error("Failed to load settings", err),
    };

    let ttl = payload
        .ttl_seconds
        .unwrap_or_else(|| settings.default_invite_ttl_seconds.max(3600) as u64);
    let expires_at = if ttl == 0 {
        PERMANENT_INVITE_EXPIRY
    } else {
        now_secs() + ttl as i64
    };

    let code = Uuid::new_v4().to_string();

    if let Err(err) =
        sqlx::query("INSERT INTO app_invitations (code, used, expires_at) VALUES ($1, $2, $3)")
            .bind(&code)
            .bind(false)
            .bind(expires_at)
            .execute(&state.db)
            .await
    {
        return database_error("Failed to create invitation", err);
    }

    let _ = append_audit_log(
        &state.db,
        "action",
        Some(&admin.username),
        &format!(
            "管理员 {} 创建邀请码 {}，有效期 {} 秒",
            admin.username, code, ttl
        ),
    )
    .await;

    app_ok("success", json!({ "code": code, "expires_at": expires_at }))
}

async fn list_invitations_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let _admin = admin_or_return!(state, headers);

    let rows = match sqlx::query(
        "SELECT code, used, expires_at FROM app_invitations ORDER BY expires_at DESC",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => return database_error("Failed to list invitations", err),
    };

    let invitations = rows
        .into_iter()
        .map(|row| {
            json!({
                "code": row.try_get::<String, _>("code").unwrap_or_default(),
                "used": row.try_get::<bool, _>("used").unwrap_or(false),
                "expires_at": row.try_get::<i64, _>("expires_at").unwrap_or(0),
            })
        })
        .collect::<Vec<_>>();

    app_ok("success", json!({ "invitations": invitations }))
}

// --------------------------------------------------------------------------------------
// 注册审批
// --------------------------------------------------------------------------------------

async fn list_pending_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let _admin = admin_or_return!(state, headers);

    let rows = match sqlx::query(
        "SELECT username, created_at, requested_role FROM app_pending_users ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => return database_error("Failed to list pending users", err),
    };

    let pending_users = rows
        .into_iter()
        .map(|row| {
            json!({
                "username": row.try_get::<String, _>("username").unwrap_or_default(),
                "created_at": row.try_get::<i64, _>("created_at").unwrap_or(0),
                "requested_role": normalize_requested_role(
                    &row.try_get::<String, _>("requested_role")
                        .unwrap_or_else(|_| "user".to_string()),
                ),
            })
        })
        .collect::<Vec<_>>();

    app_ok("success", json!({ "pending_users": pending_users }))
}

async fn approve_pending_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<UsernameRequest>,
) -> Response {
    let admin = admin_or_return!(state, headers);

    let username = payload.username.trim();
    if username.is_empty() {
        return app_err(StatusCode::BAD_REQUEST, "username is required", Value::Null);
    }

    let pending = match sqlx::query(
        "SELECT username, password_hash, created_at, requested_role FROM app_pending_users WHERE username = $1 LIMIT 1",
    )
    .bind(username)
    .fetch_optional(&state.db)
    .await
    {
        Ok(row) => row,
        Err(err) => return database_error("Failed to query pending user", err),
    };

    let Some(row) = pending else {
        return app_err(StatusCode::NOT_FOUND, "Pending user not found", Value::Null);
    };

    let password_hash: String = row.try_get("password_hash").unwrap_or_default();
    let created_at_secs: i64 = row.try_get("created_at").unwrap_or(0);
    let requested_role: String = row
        .try_get("requested_role")
        .unwrap_or_else(|_| "user".to_string());

    let exists =
        sqlx::query_scalar::<_, i32>(r#"SELECT 1 FROM "user" WHERE username = $1 LIMIT 1"#)
            .bind(username)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .is_some();
    if exists {
        return app_err(StatusCode::CONFLICT, "Username already exists", Value::Null);
    }

    // 申请时间在 `app_pending_users` 里是 epoch 秒，而契约表是 TIMESTAMPTZ，需要显式转换。
    let created_at = DateTime::<Utc>::from_timestamp(created_at_secs, 0).unwrap_or_else(Utc::now);

    let insert = sqlx::query(
        r#"INSERT INTO "user" (id, username, password, role, is_admin, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $6)"#,
    )
    .bind(Uuid::new_v4())
    .bind(username)
    .bind(&password_hash)
    .bind(WEB_DEFAULT_ROLE)
    .bind(requested_role.trim().eq_ignore_ascii_case("admin"))
    .bind(created_at)
    .execute(&state.db)
    .await;

    if let Err(err) = insert {
        return database_error("Failed to approve pending user", err);
    }

    let _ = sqlx::query("DELETE FROM app_pending_users WHERE username = $1")
        .bind(username)
        .execute(&state.db)
        .await;

    let _ = append_audit_log(
        &state.db,
        "action",
        Some(&admin.username),
        &format!(
            "管理员 {} 通过了用户 {} 的注册申请",
            admin.username, username
        ),
    )
    .await;

    app_ok("success", json!({ "status": "approved" }))
}

async fn reject_pending_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<UsernameRequest>,
) -> Response {
    let admin = admin_or_return!(state, headers);

    let username = payload.username.trim();
    if username.is_empty() {
        return app_err(StatusCode::BAD_REQUEST, "username is required", Value::Null);
    }

    let result = match sqlx::query("DELETE FROM app_pending_users WHERE username = $1")
        .bind(username)
        .execute(&state.db)
        .await
    {
        Ok(result) => result,
        Err(err) => return database_error("Failed to reject pending user", err),
    };

    if result.rows_affected() == 0 {
        return app_err(StatusCode::NOT_FOUND, "Pending user not found", Value::Null);
    }

    let _ = append_audit_log(
        &state.db,
        "action",
        Some(&admin.username),
        &format!(
            "管理员 {} 拒绝了用户 {} 的注册申请",
            admin.username, username
        ),
    )
    .await;

    app_ok("success", json!({ "status": "rejected" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_value(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn requested_role_normalizes_like_legacy() {
        assert_eq!(normalize_requested_role("admin"), "admin");
        assert_eq!(normalize_requested_role("ADMIN"), "admin");
        assert_eq!(normalize_requested_role(" admin "), "admin");
        assert_eq!(normalize_requested_role("user"), "user");
        assert_eq!(normalize_requested_role(""), "user");
    }

    /// `admin.js:73` 的 `formatDate(unixSeconds)` 按秒渲染，且 `i64::MAX` 会显示「永久有效」。
    #[test]
    fn permanent_invite_sentinel_renders_as_permanent() {
        const FORMAT_DATE_PERMANENT_THRESHOLD: i64 = 4_102_444_800;
        assert!(PERMANENT_INVITE_EXPIRY > FORMAT_DATE_PERMANENT_THRESHOLD);
    }

    /// 键集必须与旧 `system_settings_payload` 一致：`admin.js:1051-1059` 逐字段读它们。
    #[test]
    fn settings_payload_keeps_legacy_keys() {
        let value = settings_payload(&SystemSettings::default());
        for key in [
            "open_registration",
            "invite_bypass_enabled",
            "maintenance_mode",
            "default_invite_ttl_seconds",
            "confidence_threshold",
            "log_retention_days",
            "updated_at",
            "updated_by",
        ] {
            assert!(value.get(key).is_some(), "缺少 {key}: {value}");
        }
    }

    /// `unwrapApiPayload` 只在 `data != null` 时取 `data`，所以成功体不能给 `data: null`。
    #[tokio::test]
    async fn success_envelope_has_non_null_data() {
        let value = body_value(app_ok("success", json!({ "users": [] }))).await;
        assert_eq!(value["code"], 200);
        assert_eq!(value["message"], "success");
        assert!(value.get("timestamp").is_some(), "{value}");
        assert!(
            !value["data"].is_null(),
            "unwrapApiPayload 会因此回落成整个信封: {value}"
        );
    }

    /// `readErrorMessage` 读 `error` → `message`，所以错误文案必须落在 `message` 上。
    #[tokio::test]
    async fn error_envelope_message_is_readable_by_frontend() {
        let response = app_err(StatusCode::NOT_FOUND, "Pending user not found", Value::Null);
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let value = body_value(response).await;
        assert_eq!(value["message"], "Pending user not found");
        assert!(value.get("timestamp").is_some(), "{value}");
    }

    /// 路由必须能构建（同路径重复注册会让 `Router::merge` 直接 panic，症状是服务起不来）。
    #[test]
    fn router_builds_without_path_conflicts() {
        let _ = router();
    }
}
