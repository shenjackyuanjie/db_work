//! 网页会话超集：cookie 会话（`session_token`）而非 Django 的 Bearer。
//!
//! 负责的 `/web/*` 路径（外层已 `nest("/web")`，此处写相对路径）：
//!
//! | 路径 | 方法 | 旧路径 | 前端引用 |
//! |---|---|---|---|
//! | `/session/login` | POST | `/user/login` | `index.js:225` |
//! | `/session/register` | POST | `/user/register` | `index.js:260`（带审批流） |
//! | `/session/logout` | POST | `/user/logout` | 9 个文件 |
//! | `/session/validate` | POST | `/user/validate` | 9 个文件（返回 `valid` / `is_admin`） |
//! | `/session/me` | POST | `/user/me` | 无（与 validate 同族，一并迁移以免留孤儿） |
//!
//! 与契约层的关系（`W2_PLAN.md` §3）：登录/登出复用 `compat::auth` 的口令校验与
//! token 表读写，**新增职责**是下发 / 清除 HttpOnly cookie `session_token`。
//! 数据面从 `app_users` / `app_sessions` 改成 `"user"` / `auth_token`。
//!
//! 为什么必须留在 `/web/*` 而不是 `/api/*`：Django 的 `/api/me` 没有 `is_admin` 字段
//! （`is_admin` 是 `"user"` 的加法列，见 `bootstrap.rs`），两者契约不同，不能混。

use axum::Router;
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::compat::auth::{self, AuthUser};
use crate::server::AppState;

/// 与 `compat::auth` 的备用通道共用同一个 Cookie 名，否则它读不到这里下发的会话。
const COOKIE_NAME: &str = "session_token";
/// 对应 `models.default_auth_token_expiry` 的 `timedelta(days=30)`。
const SESSION_MAX_AGE_SECONDS: u64 = 30 * 24 * 60 * 60;

/// 网页注册默认落在果农侧：网页是果园运营端，`"user".role` 只有 `farmer`/`buyer` 两个取值，
/// 而注册表单只问「管理员 / 普通用户」，所以角色固定 `farmer`、管理员身份另记 `is_admin`。
const WEB_DEFAULT_ROLE: &str = "farmer";

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/session/login", post(login_handler))
        .route("/session/register", post(register_handler))
        .route("/session/logout", post(logout_handler))
        .route("/session/validate", post(validate_handler))
        .route("/session/me", post(me_handler))
}

// --------------------------------------------------------------------------------------
// 供 S3（admin / dashboard / store_admin）复用的公共能力
// --------------------------------------------------------------------------------------

/// 超集层的统一信封：`{code, message, data, timestamp}`。
///
/// 网页前端的 `postJson` 会先取 `resp.data`，再交给 `unwrapApiPayload` 解一层，
/// 所以这里必须保持这个形状（与旧 `user_routes` 的 `app_response` 一致）。
pub(crate) fn app_response(code: u16, message: impl Into<String>, data: Value) -> Value {
    json!({
        "code": code,
        "message": message.into(),
        "data": data,
        "timestamp": crate::server::now_millis()
    })
}

pub(crate) fn app_ok(message: impl Into<String>, data: Value) -> Response {
    (StatusCode::OK, Json(app_response(200, message, data))).into_response()
}

pub(crate) fn app_err(status: StatusCode, message: impl Into<String>, data: Value) -> Response {
    (status, Json(app_response(status.as_u16(), message, data))).into_response()
}

/// 401 + 超集信封，供 `/web/*` 的鉴权失败使用（不是契约层的 DRF 异常体）。
pub(crate) fn unauthorized(message: impl Into<String>) -> Response {
    app_err(StatusCode::UNAUTHORIZED, message, Value::Null)
}

pub(crate) fn forbidden(message: impl Into<String>) -> Response {
    app_err(StatusCode::FORBIDDEN, message, Value::Null)
}

/// 三通道取原始 token：`Authorization: Bearer`（若存在则严格解析）→ Cookie → `X-Session-Token`。
///
/// 直接复用 `compat::auth::bearer_token` 的解析规则，避免两套语义漂移。
pub(crate) fn extract_token(headers: &HeaderMap) -> Option<String> {
    match auth::bearer_token(headers) {
        Ok(Some(token)) => return Some(token),
        Err(()) => return None,
        Ok(None) => {}
    }

    if let Some(cookies) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) {
        let found = cookies.split(';').find_map(|part| {
            part.trim()
                .strip_prefix(COOKIE_NAME)
                .and_then(|rest| rest.strip_prefix('='))
                .map(str::to_string)
                .filter(|value| !value.is_empty())
        });
        if found.is_some() {
            return found;
        }
    }

    headers
        .get("x-session-token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// 软鉴权：拿得到就返回调用者，拿不到返回 `None`（不产生响应）。
pub(crate) async fn current_user(state: &AppState, headers: &HeaderMap) -> Option<AuthUser> {
    auth::authenticate(&state.db, headers).await.ok()
}

/// `is_admin` 不在 `AuthUser` 里（它是契约表的加法列），按主键单独查一次。
pub(crate) async fn lookup_is_admin(db: &PgPool, user_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(r#"SELECT is_admin FROM "user" WHERE id = $1"#)
        .bind(user_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

/// **S3 与后续超集接口统一用这个做后台鉴权。**
///
/// 签名（S3 请照此书写）：
/// ```ignore
/// pub(crate) async fn require_admin(
///     state: &AppState,
///     headers: &HeaderMap,
/// ) -> Result<AuthUser, Response>
/// ```
///
/// 成功返回调用者（`AuthUser.username` 就是后台接口要的 `username`），
/// 失败返回**已经渲染好**的 `Response`，调用方直接 `return` 即可：
///
/// ```ignore
/// let admin = match require_admin(&state, &headers).await {
///     Ok(user) => user,
///     Err(response) => return response,
/// };
/// ```
///
/// 失败体是超集信封：未登录 401 `请先登录`，非管理员 403 `当前账号无管理权限`。
pub(crate) async fn require_admin(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<AuthUser, Response> {
    let Some(user) = current_user(state, headers).await else {
        return Err(unauthorized("请先登录"));
    };

    if lookup_is_admin(&state.db, user.id).await {
        Ok(user)
    } else {
        Err(forbidden("当前账号无管理权限"))
    }
}

// --------------------------------------------------------------------------------------
// Cookie
// --------------------------------------------------------------------------------------

/// `session_token=<uuid>; Path=/; HttpOnly; SameSite=Lax; Max-Age=2592000[; Secure]`
pub(crate) fn build_login_cookie(token: &str, secure: bool) -> Option<HeaderValue> {
    let secure_flag = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_MAX_AGE_SECONDS}{secure_flag}"
    ))
    .ok()
}

pub(crate) fn build_clear_cookie() -> Option<HeaderValue> {
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"
    ))
    .ok()
}

// --------------------------------------------------------------------------------------
// 系统设置（维护模式 / 注册开关）
// --------------------------------------------------------------------------------------

/// 与旧 `user_routes::auth::current_system_settings` 行为一致：读失败回退默认值。
async fn system_settings(state: &AppState) -> crate::system_settings::SystemSettings {
    match crate::system_settings::load_system_settings(&state.db).await {
        Ok(settings) => settings,
        Err(err) => {
            tracing::error!("读取系统设置失败，回退默认值: {}", err);
            crate::system_settings::SystemSettings::default()
        }
    }
}

// --------------------------------------------------------------------------------------
// 请求体
// --------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct LoginRequest {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WebRegisterRequest {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// 网页只区分 `admin` / `user`（`index.js:252` 的 `register-role` 选择框）。
    #[serde(default)]
    pub requested_role: Option<String>,
    #[serde(default)]
    pub invitation_code: Option<String>,
}

/// 网页侧「申请管理员」的判定，与旧 `parse_requested_role` 同一规则。
fn wants_admin(requested_role: Option<&str>) -> bool {
    requested_role
        .map(str::trim)
        .is_some_and(|value| value.eq_ignore_ascii_case("admin"))
}

fn role_label(wants_admin: bool) -> &'static str {
    if wants_admin { "admin" } else { "user" }
}

/// 旧 `auth::user_payload` 的形状（`created_at` 是**秒**，不是毫秒）。
fn user_payload(username: &str, created_at_secs: u64) -> Value {
    json!({
        "id": username,
        "username": username,
        "email": null,
        "orchard_address": null,
        "latitude": null,
        "longitude": null,
        "created_at": created_at_secs
    })
}

fn pending_approval_response(wants_admin: bool, hint: Option<&str>) -> Response {
    (
        StatusCode::ACCEPTED,
        Json(app_response(
            202,
            "pending_approval",
            json!({
                "requested_role": role_label(wants_admin),
                "hint": hint
            }),
        )),
    )
        .into_response()
}

// --------------------------------------------------------------------------------------
// 处理器
// --------------------------------------------------------------------------------------

/// `POST /web/session/login`
///
/// 旧 `/user/login` 的等价物，但用户表换成契约表 `"user"`、会话表换成 `auth_token`，
/// 口令校验交给 `compat::auth::verify_password`（pbkdf2 / argon2 / blake3 三格式）。
pub(crate) async fn login_handler(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Response {
    let username = payload.username.trim();
    let password = payload.password.trim();

    if username.is_empty() || password.is_empty() {
        return app_err(
            StatusCode::BAD_REQUEST,
            "Username and password are required",
            Value::Null,
        );
    }

    let row = match sqlx::query(
        r#"SELECT id, password, is_admin, created_at, latitude, longitude
             FROM "user" WHERE username = $1 LIMIT 1"#,
    )
    .bind(username)
    .fetch_optional(&state.db)
    .await
    {
        Ok(row) => row,
        Err(err) => {
            tracing::error!(%err, username, "网页登录查询用户失败");
            return app_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {}", err),
                Value::Null,
            );
        }
    };

    let Some(row) = row else {
        return app_err(StatusCode::UNAUTHORIZED, "Invalid credentials", Value::Null);
    };

    let user_id: Uuid = row.try_get("id").unwrap_or_default();
    let password_hash: String = row.try_get("password").unwrap_or_default();
    let is_admin: bool = row.try_get("is_admin").unwrap_or(false);
    let created_at: DateTime<Utc> = row.try_get("created_at").unwrap_or_else(|_| Utc::now());
    let latitude: Option<f64> = row.try_get("latitude").unwrap_or(None);
    let longitude: Option<f64> = row.try_get("longitude").unwrap_or(None);

    if !auth::verify_password(&password_hash, password) {
        return app_err(StatusCode::UNAUTHORIZED, "Invalid credentials", Value::Null);
    }

    let settings = system_settings(&state).await;
    if settings.maintenance_mode && !is_admin {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(app_response(
                503,
                "系统维护中，仅管理员可登录",
                json!({ "maintenance_mode": true }),
            )),
        )
            .into_response();
    }

    // 历史哈希（blake3 / argon2）在首次成功登录后迁移成 Django 认可的 pbkdf2_sha256。
    if !password_hash.starts_with("pbkdf2_sha256$") {
        let rehashed = auth::hash_password(password);
        if let Err(err) = sqlx::query(r#"UPDATE "user" SET password = $1 WHERE id = $2"#)
            .bind(&rehashed)
            .bind(user_id)
            .execute(&state.db)
            .await
        {
            tracing::warn!(%err, %username, "旧密码哈希迁移失败，将在下次登录重试");
        }
    }

    let token = match auth::create_session(&state.db, user_id).await {
        Ok((token, _expires_at)) => token,
        Err(reject) => return reject.into_response(),
    };

    let Some(cookie) = build_login_cookie(&token.to_string(), state.secure_session_cookie) else {
        return app_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to build session cookie",
            Value::Null,
        );
    };

    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, cookie);

    (
        StatusCode::OK,
        headers,
        Json(app_response(
            200,
            "Login successful",
            json!({
                "id": username,
                "username": username,
                "email": null,
                "orchard_address": null,
                "latitude": latitude,
                "longitude": longitude,
                "created_at": created_at.timestamp_millis().max(0) as u64,
                "token": token.to_string(),
                "is_admin": is_admin
            }),
        )),
    )
        .into_response()
}

/// `POST /web/session/register`
///
/// 逐条复刻旧 `/user/register` 的判定顺序与文案，只把落库目标从 `app_users` 换成 `"user"`。
/// `app_pending_users` / `app_invitations` 是保留表（`W2_PLAN.md` §4.2），语义不变。
pub(crate) async fn register_handler(
    State(state): State<AppState>,
    Json(payload): Json<WebRegisterRequest>,
) -> Response {
    let username = payload.username.trim().to_string();
    let password = payload.password.trim().to_string();
    let settings = system_settings(&state).await;
    let wants_admin = wants_admin(payload.requested_role.as_deref());

    if username.is_empty() || password.is_empty() {
        return app_err(
            StatusCode::BAD_REQUEST,
            "Username and password are required",
            Value::Null,
        );
    }

    if !settings.open_registration {
        return app_err(
            StatusCode::FORBIDDEN,
            "当前已关闭注册",
            json!({ "open_registration": false }),
        );
    }

    match sqlx::query_scalar::<_, i32>(r#"SELECT 1 FROM "user" WHERE username = $1 LIMIT 1"#)
        .bind(&username)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(_)) => {
            return app_err(StatusCode::CONFLICT, "Username already exists", Value::Null);
        }
        Ok(None) => {}
        Err(err) => {
            tracing::error!(%err, %username, "网页注册查询用户失败");
            return app_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {}", err),
                Value::Null,
            );
        }
    }

    match sqlx::query_scalar::<_, String>(
        "SELECT username FROM app_pending_users WHERE username = $1 LIMIT 1",
    )
    .bind(&username)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(_)) => {
            return app_err(
                StatusCode::CONFLICT,
                "Username already pending approval",
                Value::Null,
            );
        }
        Ok(None) => {}
        Err(err) => {
            tracing::error!(%err, %username, "网页注册查询待审批失败");
            return app_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {}", err),
                Value::Null,
            );
        }
    }

    let now = crate::server::now_millis() / 1000;
    let password_hash = auth::hash_password(&password);
    let invitation_code = payload
        .invitation_code
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_string();

    /// 待审批入队；四种「不直接放行」的分支共用。
    macro_rules! enqueue_pending {
        ($hint:expr) => {{
            let result = sqlx::query(
                "INSERT INTO app_pending_users (username, password_hash, created_at, requested_role) VALUES ($1, $2, $3, $4)",
            )
            .bind(&username)
            .bind(&password_hash)
            .bind(now as i64)
            .bind(role_label(wants_admin))
            .execute(&state.db)
            .await;

            return match result {
                Ok(_) => pending_approval_response(wants_admin, $hint),
                Err(err) => app_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", err),
                    Value::Null,
                ),
            };
        }};
    }

    if invitation_code.is_empty() {
        enqueue_pending!(None);
    }

    if !settings.invite_bypass_enabled {
        enqueue_pending!(Some("邀请码免审核当前已关闭，已进入审批队列"));
    }

    let invite_valid =
        match sqlx::query("SELECT used, expires_at FROM app_invitations WHERE code = $1 LIMIT 1")
            .bind(&invitation_code)
            .fetch_optional(&state.db)
            .await
        {
            Ok(Some(row)) => {
                let used: bool = row.try_get("used").unwrap_or(true);
                let expires_at: i64 = row.try_get("expires_at").unwrap_or(0);
                !used && expires_at > now as i64
            }
            Ok(None) => false,
            Err(err) => {
                tracing::error!(%err, "网页注册校验邀请码失败");
                return app_err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", err),
                    Value::Null,
                );
            }
        };

    if !invite_valid {
        enqueue_pending!(Some("邀请码无效或已过期，已进入审批队列"));
    }

    let _ = sqlx::query("UPDATE app_invitations SET used = TRUE WHERE code = $1")
        .bind(&invitation_code)
        .execute(&state.db)
        .await;

    // 契约表必填列：id / username / password / role / created_at / updated_at。
    // is_admin 是加法列，有 DEFAULT FALSE，这里显式写以便用户能申请管理员。
    let created_at = Utc::now();
    if let Err(err) = sqlx::query(
        r#"INSERT INTO "user" (id, username, password, role, is_admin, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $6)"#,
    )
    .bind(Uuid::new_v4())
    .bind(&username)
    .bind(&password_hash)
    .bind(WEB_DEFAULT_ROLE)
    .bind(wants_admin)
    .bind(created_at)
    .execute(&state.db)
    .await
    {
        tracing::error!(%err, %username, "网页注册写入用户失败");
        return app_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("db error: {}", err),
            Value::Null,
        );
    }

    app_ok("Registration successful", user_payload(&username, now))
}

/// `POST /web/session/logout`
///
/// 删掉当前 token 并清除 cookie。**无论是否真的删到了行都返回 200**：
/// 前端（`app-shell.js:170`）在 `!response.ok` 时会弹「退出失败，请重试。」，
/// 而「token 已过期后再点一次退出」是常见路径，不该报错。
pub(crate) async fn logout_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let deleted = match extract_token(&headers) {
        Some(token) => match auth::lookup_token(&state.db, &token).await {
            Ok(user) => sqlx::query("DELETE FROM auth_token WHERE key = $1")
                .bind(user.token_key)
                .execute(&state.db)
                .await
                .map(|result| result.rows_affected())
                .unwrap_or(0),
            // token 无效/已过期：`lookup_token` 对过期行已经删过了，这里无需再动。
            Err(_) => 0,
        },
        None => 0,
    };

    let mut response_headers = HeaderMap::new();
    if let Some(clear) = build_clear_cookie() {
        response_headers.insert(header::SET_COOKIE, clear);
    }

    (
        StatusCode::OK,
        response_headers,
        Json(json!({
            "status": "logged out",
            "deleted": deleted
        })),
    )
        .into_response()
}

/// `POST /web/session/validate`
///
/// **必须保持扁平体 + 恒 200**：网页前端有两种解包习惯，且 `index.js:197-198`
/// 直接把整个 body 交给 `applySystemStatus` 并读 `data.valid`，套一层信封就会坏。
///
/// 形状与旧 `/user/validate` 逐字段一致：
/// `{valid, username?, is_admin?, maintenance_mode, open_registration}`
pub(crate) async fn validate_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let settings = system_settings(&state).await;

    let (valid, username, is_admin) = match current_user(&state, &headers).await {
        Some(user) => {
            let is_admin = lookup_is_admin(&state.db, user.id).await;
            (true, Some(user.username), Some(is_admin))
        }
        None => (false, None, None),
    };

    let mut body = serde_json::Map::new();
    body.insert("valid".to_string(), Value::Bool(valid));
    if let Some(username) = username {
        body.insert("username".to_string(), Value::String(username));
    }
    if let Some(is_admin) = is_admin {
        body.insert("is_admin".to_string(), Value::Bool(is_admin));
    }
    body.insert(
        "maintenance_mode".to_string(),
        Value::Bool(settings.maintenance_mode),
    );
    body.insert(
        "open_registration".to_string(),
        Value::Bool(settings.open_registration),
    );

    (StatusCode::OK, Json(Value::Object(body))).into_response()
}

/// `POST /web/session/me`
///
/// 前端未使用（旧 `/user/me` 也是孤儿），保留为超集自用；形状沿用旧实现。
pub(crate) async fn me_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(user) = current_user(&state, &headers).await else {
        return unauthorized("请先登录");
    };

    let is_admin = lookup_is_admin(&state.db, user.id).await;

    app_ok(
        "success",
        json!({
            "username": user.username,
            "is_admin": is_admin,
            "created_at": user.created_at.timestamp(),
            "latitude": user.latitude,
            "longitude": user.longitude
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_is_http_only_and_optionally_secure() {
        let local = build_login_cookie("abc", false).unwrap();
        let local = local.to_str().unwrap();
        assert!(local.starts_with("session_token=abc; Path=/; HttpOnly; SameSite=Lax"));
        assert!(!local.contains("; Secure"));
        assert!(local.contains("Max-Age=2592000"));

        let production = build_login_cookie("abc", true).unwrap();
        assert!(production.to_str().unwrap().ends_with("; Secure"));
    }

    #[test]
    fn clear_cookie_expires_immediately() {
        let clear = build_clear_cookie().unwrap();
        let clear = clear.to_str().unwrap();
        assert!(clear.starts_with("session_token=;"));
        assert!(clear.contains("Max-Age=0"));
    }

    /// compat::auth 读的就是这个名字与格式，两头必须对得上。
    #[test]
    fn cookie_name_matches_compat_reader() {
        let cookie = build_login_cookie("11111111-1111-4111-8111-111111111111", false).unwrap();
        let cookie = cookie.to_str().unwrap().to_string();
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_str(&cookie).unwrap());

        assert_eq!(
            extract_token(&headers).as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
    }

    #[test]
    fn bearer_header_takes_priority_over_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("session_token=from-cookie"),
        );
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer from-header"),
        );
        assert_eq!(extract_token(&headers).as_deref(), Some("from-header"));
    }

    #[test]
    fn malformed_authorization_does_not_fall_back_to_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("session_token=from-cookie"),
        );
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Token nope"),
        );
        assert_eq!(extract_token(&headers), None);
    }

    #[test]
    fn fallback_header_is_accepted_when_cookie_absent() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-session-token",
            HeaderValue::from_static("  from-header  "),
        );
        assert_eq!(extract_token(&headers).as_deref(), Some("from-header"));
    }

    #[test]
    fn requested_role_maps_to_admin_only_for_admin_literal() {
        assert!(wants_admin(Some("admin")));
        assert!(wants_admin(Some("ADMIN")));
        assert!(wants_admin(Some(" admin ")));
        assert!(!wants_admin(Some("user")));
        assert!(!wants_admin(Some("")));
        assert!(!wants_admin(None));
    }

    /// 旧 `app_response` 的键序是 code, message, data, timestamp（`preserve_order` 下必须一致）。
    #[test]
    fn superset_envelope_keeps_legacy_key_order() {
        let text = app_response(200, "ok", Value::Null).to_string();
        assert!(
            text.starts_with(r#"{"code":200,"message":"ok","data":null,"timestamp":"#),
            "{text}"
        );
    }
}
