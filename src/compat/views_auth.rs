//! 账号域契约实现：register / login / logout / me 及其 `/api/v1/auth/*` 与 `/api/user` 别名。
//!
//! 蓝本为 `navel_backend_git/api/auth_views.py`、`api/serializers.py`、`api/authentication.py`。
//! 契约基准见 `tests/fixtures/contract/auth.json`（27 条用例）。
//!
//! 三条容易踩空的契约细节：
//!
//! 1. **两种错误形状不能混**：序列化器校验失败走 `errors::api_response(...)`（**成功体形状**，
//!    带 `timestamp`；register 是 400、login 是 401，后者连 `Invalid credentials` 也带
//!    `timestamp`，见 [`invalid_credentials`]）；鉴权失败走 `ApiReject`（DRF 异常体，
//!    无 `timestamp`，带 `WWW-Authenticate: Bearer`），405 也是异常体。
//! 2. **405 逐路由收口**：用 `MethodRouter::fallback` 在**本文件内部**挂，绝不放到
//!    `compat.rs`——`Router::merge` 遇到路径级 fallback 会 panic，且多个域会互相覆盖。
//! 3. **测试缝**：`AppState` 的构造会加载两个 ONNX 模型，单测里没法廉价构造。所以每个
//!    端点拆成「薄 axum 包装 + `*_impl(&PgPool, ...)`」，单测直接打 scratch schema。

use axum::{
    Router,
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::server::AppState;

use super::{
    auth::{self, AuthUser},
    dto::{Input, UserLoginBody, UserRegistrationBody},
    errors::{ApiReject, ApiResult, api_ok, api_ok_message, api_response},
};

// --------------------------------------------------------------------------------------
// 文案字典（逐字取自 `REPORT.md §5.2`，不要改标点）
// --------------------------------------------------------------------------------------

const ERR_REQUIRED: &str = "该字段是必填项。";
const ERR_NULL: &str = "该字段不能为 null。";
const ERR_BLANK: &str = "该字段不能为空。";
const ERR_MIN_LENGTH_6: &str = "请确保这个字段至少包含 6 个字符。";
const ERR_INVALID_EMAIL: &str = "请输入合法的邮件地址。";
const ERR_INVALID_NUMBER: &str = "请填写合法的数字。";
const ERR_INVALID_CREDENTIALS: &str = "Invalid credentials";
const ERR_FARMER_NEEDS_ORCHARD: &str = "果农需要填写果园地址";

const MSG_REGISTRATION_SUCCESS: &str = "Registration successful";
const MSG_LOGIN_SUCCESS: &str = "Login successful";
const MSG_LOGOUT_SUCCESS: &str = "Logout successful";

/// Django `choices` 的两个合法 role。
const ROLE_FARMER: &str = "farmer";
const ROLE_BUYER: &str = "buyer";

/// `UserSerializer` 需要的那组列；三处查询共用，避免列清单漂移。
const USER_COLUMNS: &str = r#"u.id, u.username, u.email, u.role, u.orchard_address,
                              u.latitude, u.longitude, u.created_at"#;

// --------------------------------------------------------------------------------------
// 路由
// --------------------------------------------------------------------------------------

/// 账号域 9 条 path。外层已 `nest("/compat")`，所以这里写相对路径。
///
/// `/api/user` 与 `/api/me` 是**同一个** handler（蓝本 `get_user_api` 就是 `me_api`）。
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/register",
            post(register_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/login",
            post(login_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/logout",
            post(logout_handler).fallback(handle_unallowed_method),
        )
        .route("/api/me", get(me_handler).fallback(handle_unallowed_method))
        .route(
            "/api/user",
            get(me_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/auth/register",
            post(register_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/auth/login",
            post(login_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/auth/logout",
            post(logout_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/auth/me",
            get(me_handler).fallback(handle_unallowed_method),
        )
}

/// 405：DRF 异常体 + zh-hans 文案 `方法 “DELETE” 不被允许。`
async fn handle_unallowed_method(method: Method) -> Response {
    auth::render_method_not_allowed(&method)
}

// --------------------------------------------------------------------------------------
// 薄包装：只做「取 state / 取 body」，逻辑全在 `*_impl`
// --------------------------------------------------------------------------------------

async fn register_handler(State(state): State<AppState>, request: Request) -> Response {
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    match register_impl(&state.db, &body).await {
        Ok(response) => response,
        Err(reject) => reject.into_response(),
    }
}

async fn login_handler(State(state): State<AppState>, request: Request) -> Response {
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    match login_impl(&state.db, &body).await {
        Ok(response) => response,
        Err(reject) => reject.into_response(),
    }
}

async fn me_handler(State(state): State<AppState>, request: Request) -> Response {
    auth_result_response(me_impl(&state.db, request.headers()).await)
}

async fn logout_handler(State(state): State<AppState>, request: Request) -> Response {
    auth_result_response(logout_impl(&state.db, request.headers()).await)
}

fn auth_result_response(result: ApiResult) -> Response {
    match result {
        Ok(response) => response,
        Err(reject) => reject.into_response(),
    }
}

/// 手写 JSON 提取，而不是 `Json<Value>` extractor。
///
/// axum 的 `JsonRejection` 会渲染成 `text/plain` 的裸 400，形状与契约差得远；这里改成
/// 契约的形状（`{code, message, data}`）。**精确文案未实测**（本域夹具没有畸形 JSON 用例），
/// 取保守值：宁可形状对、文案待定，也不要用 axum 的默认。
async fn json_body(request: Request) -> Result<Value, Response> {
    let bytes = axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(|_| json_error("无效数据。请求体读取失败。"))?;

    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes).map_err(|_| json_error("JSON 解析错误。"))
}

/// DRF `ParseError` 的异常体形状（无 `timestamp`）。
fn json_error(message: &str) -> Response {
    ApiReject::bad_request(message).into_response()
}

// --------------------------------------------------------------------------------------
// POST /api/register 与 /api/v1/auth/register
// --------------------------------------------------------------------------------------

/// 校验失败**不是** `ApiReject`：蓝本走 `api_response(None, serializer.errors, 4xx)`，
/// 即**成功体形状**（`{code, message, data, timestamp}`），只是 HTTP 状态码是 4xx。
/// 所以这里返回 `Ok(Response)`，只有真正的鉴权/服务端错误才走 `Err`。
pub(crate) async fn register_impl(pool: &PgPool, body: &Value) -> ApiResult {
    let request = UserRegistrationBody::from_value(body);

    let mut errors = FieldErrors::default();
    let validated = match validate_registration(pool, &request, &mut errors).await {
        Some(validated) => validated,
        None => return Ok(errors.into_response(StatusCode::BAD_REQUEST)),
    };

    let mut tx = pool.begin().await.map_err(internal_error)?;
    let user = create_user(&mut tx, &validated).await?;
    ensure_draft_orchard(&mut tx, &user, &validated).await?;
    let (token, expires_at) = auth::create_session_tx(&mut tx, user.id).await?;
    tx.commit().await.map_err(internal_error)?;

    Ok(api_ok_message(
        MSG_REGISTRATION_SUCCESS,
        auth::session_payload(token, expires_at, &user),
    ))
}

async fn create_user(
    tx: &mut Transaction<'_, Postgres>,
    input: &ValidatedRegistration,
) -> Result<AuthUser, ApiReject> {
    let id = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query(
        r#"INSERT INTO "user"
               (id, username, password, email, role, orchard_address, latitude, longitude,
                created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9)"#,
    )
    .bind(id)
    .bind(&input.username)
    .bind(auth::hash_password(&input.password))
    .bind(&input.email)
    .bind(input.role)
    .bind(&input.orchard_address)
    .bind(input.latitude)
    .bind(input.longitude)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(internal_error)?;

    let row = sqlx::query(&format!(
        r#"SELECT {USER_COLUMNS} FROM "user" u WHERE u.id = $1"#
    ))
    .bind(id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_error)?;

    Ok(row_to_auth_user(&row, id)?)
}

/// 果农注册即自动建立合作果园（草稿，待运营审核完善）。
///
/// 蓝本用 `Orchard.objects.create(...)`；**只写必要列**，其余交给 DDL `DEFAULT`
/// （D8：我方保留 DEFAULT，且与 Django 的 Python 层默认值一致）。
async fn ensure_draft_orchard(
    tx: &mut Transaction<'_, Postgres>,
    user: &AuthUser,
    input: &ValidatedRegistration,
) -> Result<(), ApiReject> {
    if !user.is_farmer() {
        return Ok(());
    }

    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM orchard WHERE owner_id = $1)")
            .bind(user.id)
            .fetch_one(&mut **tx)
            .await
            .map_err(internal_error)?;
    if exists {
        return Ok(());
    }

    let now = Utc::now();
    sqlx::query(
        r#"INSERT INTO orchard
               (id, code, owner_id, name, grower_name, county, detail_address,
                latitude, longitude, status, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, '待补充', $6, $7, $8, 'draft', $9, $9)"#,
    )
    .bind(Uuid::new_v4())
    .bind(generate_orchard_code())
    .bind(user.id)
    .bind(format!("{}的果园", user.username))
    .bind(&user.username)
    .bind(input.orchard_address.clone().unwrap_or_default())
    .bind(input.latitude)
    .bind(input.longitude)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(internal_error)?;

    Ok(())
}

/// `models.default_orchard_code`：`GY-<8 位大写 hex>`。
fn generate_orchard_code() -> String {
    let hex = Uuid::new_v4().simple().to_string().to_uppercase();
    format!("GY-{}", &hex[..8])
}

// --------------------------------------------------------------------------------------
// POST /api/login 与 /api/v1/auth/login
// --------------------------------------------------------------------------------------

pub(crate) async fn login_impl(pool: &PgPool, body: &Value) -> ApiResult {
    let request = UserLoginBody::from_value(body);

    // DRF 会把两个字段的错误**一起**收集，所以不能遇到第一个就 return。
    let mut errors = FieldErrors::default();
    let username = required_char_field(&request.username, "username", &mut errors);
    let password = required_char_field(&request.password, "password", &mut errors);
    let (Ok(username), Ok(password)) = (username, password) else {
        return Ok(errors.into_response(StatusCode::UNAUTHORIZED));
    };

    let row = sqlx::query(&format!(
        r#"SELECT {USER_COLUMNS}, u.password AS password FROM "user" u WHERE u.username = $1"#
    ))
    .bind(&username)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    // 用户不存在与口令错误是**同一个**文案，蓝本如此。
    let Some(row) = row else {
        return Ok(invalid_credentials());
    };
    let stored: String = row.try_get("password").map_err(internal_error)?;
    if !auth::verify_password(&stored, &password) {
        return Ok(invalid_credentials());
    }

    let id: Uuid = row.try_get("id").map_err(internal_error)?;
    let user = row_to_auth_user(&row, id)?;
    let (token, expires_at) = auth::create_session(pool, user.id).await?;

    Ok(api_ok_message(
        MSG_LOGIN_SUCCESS,
        auth::session_payload(token, expires_at, &user),
    ))
}

/// 凭据错误的 401。
///
/// ⚠️ 蓝本这里写的是 `serializers.ValidationError('Invalid credentials')`，正常会被
/// DRF 渲染成**异常体**（无 `timestamp`），但实测录到的响应是
/// `{"code":401,"message":{"non_field_errors":["Invalid credentials"]},"data":null,"timestamp":…}`
/// —— 105 字节、`normalize_hits=['timestamp']`、键序 `code, message, data, timestamp`。
/// 也就是说它走的是**成功体形状**。夹具 > 蓝本源码，照实测复刻。
fn invalid_credentials() -> Response {
    api_response(
        StatusCode::UNAUTHORIZED,
        StatusCode::UNAUTHORIZED.as_u16(),
        json!({ "non_field_errors": [ERR_INVALID_CREDENTIALS] }),
        Value::Null,
    )
}

// --------------------------------------------------------------------------------------
// GET /api/me、/api/user、/api/v1/auth/me
// --------------------------------------------------------------------------------------

pub(crate) async fn me_impl(pool: &PgPool, headers: &axum::http::HeaderMap) -> ApiResult {
    let user = auth::authenticate(pool, headers).await?;
    Ok(api_ok(user.payload()))
}

// --------------------------------------------------------------------------------------
// POST /api/logout、/api/v1/auth/logout
// --------------------------------------------------------------------------------------

pub(crate) async fn logout_impl(pool: &PgPool, headers: &axum::http::HeaderMap) -> ApiResult {
    let user = auth::authenticate(pool, headers).await?;

    sqlx::query("DELETE FROM auth_token WHERE key = $1")
        .bind(user.token_key)
        .execute(pool)
        .await
        .map_err(internal_error)?;

    Ok(api_ok_message(MSG_LOGOUT_SUCCESS, Value::Null))
}

// --------------------------------------------------------------------------------------
// 序列化器校验（DRF 语义：逐字段收集，全通过才跑对象级 validate）
// --------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ValidatedRegistration {
    username: String,
    email: Option<String>,
    password: String,
    role: &'static str,
    orchard_address: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
}

/// DRF 的字段错误字典：`{"字段": ["文案"]}`，**插入序 = 字段声明序**。
///
/// `serde_json` 开了 `preserve_order`，所以这个序会原样落到响应体里。
#[derive(Debug, Default)]
struct FieldErrors(Map<String, Value>);

impl FieldErrors {
    fn push(&mut self, field: &str, message: &str) {
        self.0.insert(field.to_string(), json!([message]));
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 校验失败走**成功体形状**（带 `timestamp`），只是 HTTP 状态码是 4xx。
    fn into_response(self, status: StatusCode) -> Response {
        api_response(status, status.as_u16(), Value::Object(self.0), Value::Null)
    }
}

async fn validate_registration(
    pool: &PgPool,
    request: &UserRegistrationBody,
    errors: &mut FieldErrors,
) -> Option<ValidatedRegistration> {
    let username = match required_char_field(&request.username, "username", errors) {
        Ok(value) => value,
        Err(()) => return None,
    };

    let email = match optional_char_field(&request.email, "email", errors) {
        Ok(None) => None,
        Ok(Some(text)) if is_email_like(&text) => Some(text),
        Ok(Some(_)) => {
            errors.push("email", ERR_INVALID_EMAIL);
            return None;
        }
        Err(()) => return None,
    };

    let password = match required_char_field(&request.password, "password", errors) {
        Ok(value) if value.chars().count() >= 6 => value,
        Ok(_) => {
            errors.push("password", ERR_MIN_LENGTH_6);
            return None;
        }
        Err(()) => return None,
    };

    let role = match choice_field(&request.role, "role", errors) {
        Ok(value) => value,
        Err(()) => return None,
    };

    let mut orchard_address =
        match optional_char_field(&request.orchard_address, "orchard_address", errors) {
            Ok(value) => value,
            Err(()) => return None,
        };
    let mut latitude = match optional_float_field(&request.latitude, "latitude", errors) {
        Ok(value) => value,
        Err(()) => return None,
    };
    let mut longitude = match optional_float_field(&request.longitude, "longitude", errors) {
        Ok(value) => value,
        Err(()) => return None,
    };

    // `UniqueValidator` 挂在**字段级校验之后**（DRF 把它塞进 `field.validators`，
    // 但只在字段自身校验通过时才跑），所以这里等其它字段都收完错误再来查库。
    ensure_username_available(pool, &username, errors).await;
    if !errors.is_empty() {
        return None;
    }

    // 字段级全通过后才跑对象级 `validate()`。
    // `attrs.get('orchard_address')` 对缺失 / `null` / `''` 三个态都判为假。
    if role == ROLE_FARMER && orchard_address.as_deref().unwrap_or_default().is_empty() {
        errors.push("orchard_address", ERR_FARMER_NEEDS_ORCHARD);
        return None;
    }
    if role == ROLE_BUYER {
        // 购买者强制清空果园相关字段（蓝本 `attrs['x'] = None`）。
        orchard_address = None;
        latitude = None;
        longitude = None;
    }

    Some(ValidatedRegistration {
        username,
        email,
        password,
        role,
        orchard_address,
        latitude,
        longitude,
    })
}

/// `serializers.CharField(required=True)`：缺 → required；`null` → null；`""` → blank。
fn required_char_field(input: &Input, field: &str, errors: &mut FieldErrors) -> Result<String, ()> {
    match input {
        Input::Missing => {
            errors.push(field, ERR_REQUIRED);
            Err(())
        }
        Input::Null => {
            errors.push(field, ERR_NULL);
            Err(())
        }
        Input::Value(Value::String(text)) if text.is_empty() => {
            errors.push(field, ERR_BLANK);
            Err(())
        }
        Input::Value(Value::String(text)) => Ok(text.trim().to_string()),
        Input::Value(_) => {
            errors.push(field, ERR_BLANK);
            Err(())
        }
    }
}

/// 模型字段派生出来的 `CharField(blank=True, null=True)`：缺 / `null` / `""` 都是 `None`，
/// 不报错。注意 `''` 会被保留成 `Some("")`，因为对象级 `validate()` 要按「假值」处理它。
fn optional_char_field(
    input: &Input,
    field: &str,
    errors: &mut FieldErrors,
) -> Result<Option<String>, ()> {
    match input {
        Input::Missing | Input::Null => Ok(None),
        Input::Value(Value::String(text)) if text.is_empty() => Ok(Some(String::new())),
        Input::Value(Value::String(text)) => Ok(Some(text.trim().to_string())),
        Input::Value(_) => {
            errors.push(field, ERR_BLANK);
            Err(())
        }
    }
}

/// `serializers.FloatField(required=False, allow_null=True)`。
fn optional_float_field(
    input: &Input,
    field: &str,
    errors: &mut FieldErrors,
) -> Result<Option<f64>, ()> {
    match input {
        Input::Missing | Input::Null => Ok(None),
        other => match other.as_f64() {
            Some(value) => Ok(Some(value)),
            None => {
                errors.push(field, ERR_INVALID_NUMBER);
                Err(())
            }
        },
    }
}

/// `serializers.ChoiceField(choices=User.Role.choices, default='farmer')`。
///
/// 缺 / `null` / `''` 都落回默认值 `farmer`（DRF 把 `''` 视作「空值 → 用默认」）。
fn choice_field(input: &Input, field: &str, errors: &mut FieldErrors) -> Result<&'static str, ()> {
    let raw = match input {
        Input::Missing | Input::Null => return Ok(ROLE_FARMER),
        Input::Value(Value::String(text)) if text.is_empty() => return Ok(ROLE_FARMER),
        Input::Value(Value::String(text)) => text.as_str(),
        Input::Value(other) => {
            let rendered = render_choice(other);
            errors.push(field, &format!("“{rendered}” 不是合法选项。"));
            return Err(());
        }
    };

    match raw {
        ROLE_FARMER => Ok(ROLE_FARMER),
        ROLE_BUYER => Ok(ROLE_BUYER),
        other => {
            errors.push(field, &format!("“{other}” 不是合法选项。"));
            Err(())
        }
    }
}

fn render_choice(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `UniqueValidator` 的文案 `具有 username 的 User 已存在。`（Django 的 `unique` 模板）。
async fn ensure_username_available(pool: &PgPool, username: &str, errors: &mut FieldErrors) {
    match sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS (SELECT 1 FROM "user" WHERE username = $1)"#,
    )
    .bind(username)
    .fetch_one(pool)
    .await
    {
        Ok(true) => errors.push("username", "具有 username 的 User 已存在。"),
        Ok(false) => {}
        Err(err) => tracing::error!("compat 注册查重失败: {err}"),
    }
}

/// 够用的邮件形状检查：DRF `EmailField` 的完整正则过于宽松/复杂，这里只挡明显非邮件串。
fn is_email_like(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !value.contains(char::is_whitespace)
}

// --------------------------------------------------------------------------------------
// 行 → AuthUser
// --------------------------------------------------------------------------------------

fn row_to_auth_user(row: &sqlx::postgres::PgRow, id: Uuid) -> Result<AuthUser, ApiReject> {
    Ok(AuthUser {
        id,
        username: row.try_get("username").map_err(internal_error)?,
        email: row.try_get("email").map_err(internal_error)?,
        role: row.try_get("role").map_err(internal_error)?,
        orchard_address: row.try_get("orchard_address").map_err(internal_error)?,
        latitude: row.try_get("latitude").map_err(internal_error)?,
        longitude: row.try_get("longitude").map_err(internal_error)?,
        created_at: row.try_get("created_at").map_err(internal_error)?,
        // 注册/登录路径上还没有 token 行，调用方随后用 `create_session` 覆盖。
        token_key: id,
    })
}

fn internal_error(err: sqlx::Error) -> ApiReject {
    tracing::error!("compat 账号域写入/查询失败: {err}");
    ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, header};
    use chrono::Duration;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    // --------------------------------------------------------------------------------
    // scratch schema 连接（连不上就跳过：离线也不红）
    // --------------------------------------------------------------------------------

    fn schema() -> String {
        std::env::var("COMPAT_TEST_SCHEMA").unwrap_or_else(|_| "compat_test".to_string())
    }

    fn database_url() -> Option<String> {
        let configured = crate::config::AppConfig::load("config.toml")
            .ok()?
            .database
            .postgres_url;

        let schema = schema();
        if !schema.starts_with("compat_") {
            eprintln!("跳过：COMPAT_TEST_SCHEMA 必须是 compat_* scratch schema，实际 {schema}");
            return None;
        }

        let separator = if configured.contains('?') { '&' } else { '?' };
        Some(format!(
            "{configured}{separator}options=-csearch_path%3D{schema}"
        ))
    }

    async fn pool() -> Option<PgPool> {
        let url = database_url()?;
        match PgPoolOptions::new().max_connections(2).connect(&url).await {
            Ok(pool) => Some(pool),
            Err(err) => {
                eprintln!("跳过（连不上 scratch schema {url}）: {err}");
                None
            }
        }
    }

    macro_rules! pool_or_skip {
        () => {
            match pool().await {
                Some(pool) => pool,
                None => return,
            }
        };
    }

    /// 测试用口令（种子账号的口令，见 `index.json.seed_refs.auth`）。
    const SEED_FARMER: &str = "farmer_xinfeng";
    const SEED_FARMER_PASSWORD: &str = "farmer123";
    const SEED_BUYER: &str = "buyer_zhang";
    const SEED_BUYER_PASSWORD: &str = "buyer123";

    async fn body_json(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    fn auth_headers(token: Option<&str>) -> axum::http::HeaderMap {
        let mut headers = axum::http::HeaderMap::new();
        if let Some(token) = token {
            headers.insert(
                header::AUTHORIZATION,
                format!("Bearer {token}").parse().unwrap(),
            );
        }
        headers
    }

    /// 建一个只在本用例存活的一次性用户名（用完即删，`auth_token`/`orchard` 走级联）。
    fn unique_username(prefix: &str) -> String {
        format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
    }

    async fn drop_user(pool: &PgPool, username: &str) {
        let _ = sqlx::query(r#"DELETE FROM "user" WHERE username = $1"#)
            .bind(username)
            .execute(pool)
            .await;
    }

    // --------------------------------------------------------------------------------
    // 纯函数层
    // --------------------------------------------------------------------------------

    #[test]
    fn bearer_channel_prefers_authorization_and_rejects_malformed() {
        // 合法：大小写不敏感、多空格容忍。
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "bearer   abc".parse().unwrap());
        assert!(matches!(auth::bearer_token(&headers), Ok(Some(t)) if t == "abc"));

        // 多一个词 -> 格式无效。
        headers.insert(header::AUTHORIZATION, "Bearer a b".parse().unwrap());
        assert!(matches!(auth::bearer_token(&headers), Err(())));

        // 非 Bearer -> 格式无效。
        headers.insert(header::AUTHORIZATION, "Token not-bearer".parse().unwrap());
        assert!(matches!(auth::bearer_token(&headers), Err(())));

        // 完全没有头 -> 交给备用通道。
        let empty = axum::http::HeaderMap::new();
        assert!(matches!(auth::bearer_token(&empty), Ok(None)));
    }

    #[test]
    fn field_errors_keep_drf_declaration_order() {
        // 用户名字段声明在 password 之前，所以重复 + 短口令时两者都报，且顺序固定。
        let mut errors = FieldErrors::default();
        errors.push("username", "a");
        errors.push("email", "b");
        errors.push("password", "c");

        let rendered = serde_json::to_string(&Value::Object(errors.0)).unwrap();
        let username = rendered.find("username").unwrap();
        let email = rendered.find("email").unwrap();
        let password = rendered.find("password").unwrap();
        assert!(username < email && email < password, "{rendered}");
    }

    #[test]
    fn orchard_code_matches_django_default_shape() {
        let code = generate_orchard_code();
        assert!(code.starts_with("GY-"), "{code}");
        assert_eq!(code.len(), 11, "{code}");
        assert!(
            code[3..]
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
            "{code}"
        );
    }

    #[test]
    fn email_shape_check_accepts_qa_address_and_rejects_junk() {
        assert!(is_email_like("qa-register@example.com"));
        assert!(!is_email_like("zz"));
        assert!(!is_email_like("a@b"));
        assert!(!is_email_like("@example.com"));
    }

    // --------------------------------------------------------------------------------
    // 单测（打 scratch schema）
    // --------------------------------------------------------------------------------

    /// 注册 400：果农缺 orchard_address —— 与夹具 `register_farmer_missing_orchard_400` 同形。
    #[tokio::test]
    async fn register_farmer_without_orchard_reports_field_error() {
        let pool = pool_or_skip!();
        let username = unique_username("w0ba_farmer");

        let (status, body) = body_json(
            register_impl(
                &pool,
                &json!({"username": username, "password": "qa-pass-123456", "role": "farmer"}),
            )
            .await
            .expect("校验失败是 Ok 分支（成功体形状），不是 ApiReject"),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], 400);
        assert_eq!(
            body["message"]["orchard_address"][0],
            ERR_FARMER_NEEDS_ORCHARD
        );
        assert_eq!(body["data"], Value::Null);
        assert!(body["timestamp"].is_i64(), "{body}");

        drop_user(&pool, &username).await;
    }

    /// 注册 400：非法 role —— 文案含全角引号。
    #[tokio::test]
    async fn register_bad_role_reports_choice_error() {
        let pool = pool_or_skip!();

        let (status, body) = body_json(
            register_impl(
                &pool,
                &json!({"username": unique_username("w0ba_role"),
                        "password": "qa-pass-123456", "role": "operator"}),
            )
            .await
            .expect("校验失败是 Ok 分支（成功体形状），不是 ApiReject"),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["message"]["role"][0], "“operator” 不是合法选项。");
    }

    /// 注册 400：口令最短 6 位。
    #[tokio::test]
    async fn register_short_password_reports_min_length() {
        let pool = pool_or_skip!();

        let (status, body) = body_json(
            register_impl(
                &pool,
                &json!({"username": unique_username("w0ba_pw"), "password": "123"}),
            )
            .await
            .expect("校验失败是 Ok 分支（成功体形状），不是 ApiReject"),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["message"]["password"][0], ERR_MIN_LENGTH_6);
    }

    /// 注册 400：用户名重复。
    #[tokio::test]
    async fn register_duplicate_username_reports_unique_error() {
        let pool = pool_or_skip!();

        let (status, body) = body_json(
            register_impl(
                &pool,
                &json!({"username": SEED_BUYER, "password": "qa-pass-123456", "role": "buyer"}),
            )
            .await
            .expect("校验失败是 Ok 分支（成功体形状），不是 ApiReject"),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body["message"]["username"][0],
            "具有 username 的 User 已存在。"
        );
    }

    /// 注册 200：买家的果园字段被强制清空；口令按 Django 格式落库。
    #[tokio::test]
    async fn register_buyer_clears_orchard_fields_and_stores_pbkdf2() {
        let pool = pool_or_skip!();
        let username = unique_username("w0ba_buyer");

        let (status, body) = body_json(
            register_impl(
                &pool,
                &json!({"username": username, "password": "qa-pass-123456", "role": "buyer",
                        "email": "qa-register@example.com",
                        "orchard_address": "不该留下", "latitude": 25.1, "longitude": 115.1}),
            )
            .await
            .expect("校验失败是 Ok 分支（成功体形状），不是 ApiReject"),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["code"], 200);
        assert_eq!(body["message"], MSG_REGISTRATION_SUCCESS);

        let data = &body["data"];
        assert!(
            Uuid::parse_str(data["token"].as_str().unwrap()).is_ok(),
            "{body}"
        );
        // `expiresAt` 是 Django `isoformat()` 形态：`+00:00`，不是 `Z`。
        let expires = data["expiresAt"].as_str().unwrap();
        assert!(expires.ends_with("+00:00"), "{expires}");

        let user = &data["user"];
        assert_eq!(user["username"], username);
        assert_eq!(user["role"], "buyer");
        assert_eq!(user["orchard_address"], Value::Null);
        assert_eq!(user["latitude"], Value::Null);
        assert_eq!(user["longitude"], Value::Null);
        assert!(
            user["created_at"].as_str().unwrap().ends_with('Z'),
            "{body}"
        );

        // 键序必须与 Django 声明序一致（`preserve_order` 兜着）。
        let rendered = serde_json::to_string(&body).unwrap();
        let code = rendered.find(r#""code""#).unwrap();
        let message = rendered.find(r#""message""#).unwrap();
        let data_at = rendered.find(r#""data""#).unwrap();
        let timestamp = rendered.find(r#""timestamp""#).unwrap();
        assert!(
            code < message && message < data_at && data_at < timestamp,
            "{rendered}"
        );

        // `data` 内部：`token, expiresAt, user`；`user` 内部按 `UserSerializer` 字段序。
        let expires_at = rendered.find(r#""expiresAt""#).unwrap();
        let user_at = rendered.find(r#""user""#).unwrap();
        assert!(
            data_at < rendered.find(r#""token""#).unwrap()
                && rendered.find(r#""token""#).unwrap() < expires_at
                && expires_at < user_at,
            "{rendered}"
        );

        let mut last = user_at;
        for key in [
            "id",
            "username",
            "email",
            "role",
            "orchard_address",
            "latitude",
            "longitude",
            "created_at",
        ] {
            let at = rendered.find(&format!(r#""{key}""#)).unwrap_or_else(|| {
                panic!("user 里缺少 {key}：{rendered}");
            });
            assert!(last < at, "{key} 的位置不对：{rendered}");
            last = at;
        }

        let stored: String =
            sqlx::query_scalar(r#"SELECT password FROM "user" WHERE username = $1"#)
                .bind(&username)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(stored.starts_with("pbkdf2_sha256$720000$"), "{stored}");
        assert!(auth::verify_password(&stored, "qa-pass-123456"));
        assert!(!stored.starts_with("$argon2"));

        drop_user(&pool, &username).await;
    }

    /// 注册 200：果农自动拿到一个 draft 果园。
    #[tokio::test]
    async fn register_farmer_creates_draft_orchard() {
        let pool = pool_or_skip!();
        let username = unique_username("w0ba_orchard");

        let (status, body) = body_json(
            register_impl(
                &pool,
                &json!({"username": username, "password": "qa-pass-123456", "role": "farmer",
                        "orchard_address": "赣州市信丰县 QA 果园", "latitude": 25.1, "longitude": 115.1}),
            )
            .await
            .expect("校验失败是 Ok 分支（成功体形状），不是 ApiReject"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["data"]["user"]["orchard_address"],
            "赣州市信丰县 QA 果园"
        );

        let row = sqlx::query(
            r#"SELECT o.code, o.name, o.grower_name, o.county, o.status, o.detail_address,
                      o.latitude, o.longitude, o.province, o.main_variety, o.area_mu,
                      o.feature_tags::text AS feature_tags
                 FROM orchard o JOIN "user" u ON u.id = o.owner_id
                WHERE u.username = $1"#,
        )
        .bind(&username)
        .fetch_one(&pool)
        .await
        .unwrap();

        let code: String = row.get("code");
        assert_eq!(code.len(), 11, "{code}");
        assert_eq!(row.get::<String, _>("name"), format!("{username}的果园"));
        assert_eq!(row.get::<String, _>("grower_name"), username);
        assert_eq!(row.get::<String, _>("county"), "待补充");
        assert_eq!(row.get::<String, _>("status"), "draft");
        assert_eq!(
            row.get::<String, _>("detail_address"),
            "赣州市信丰县 QA 果园"
        );
        // 只写必要列，其余走 DDL DEFAULT。
        assert_eq!(row.get::<String, _>("province"), "江西省");
        assert_eq!(row.get::<String, _>("main_variety"), "纽荷尔脐橙");
        assert_eq!(row.get::<Option<rust_decimal::Decimal>, _>("area_mu"), None);
        assert_eq!(row.get::<String, _>("feature_tags"), "[]");

        drop_user(&pool, &username).await;
    }

    /// 登录 401：口令错 / 用户不存在都报 `non_field_errors`。
    #[tokio::test]
    async fn login_failure_uses_non_field_errors() {
        let pool = pool_or_skip!();

        let cases = [
            json!({"username": SEED_BUYER, "password": "wrong"}),
            json!({"username": "no_such_user_at_all", "password": "whatever"}),
        ];
        for payload in cases {
            let (status, body) = body_json(login_impl(&pool, &payload).await.unwrap()).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(
                body["message"]["non_field_errors"][0],
                ERR_INVALID_CREDENTIALS
            );
            assert_eq!(body["data"], Value::Null);
            // 凭据错误走**成功体形状**（带 timestamp）：蓝本源码看着该走异常体，
            // 但夹具实测是成功体形状（105 字节、normalize_hits=['timestamp']），以夹具为准。
            assert!(body["timestamp"].is_i64(), "{body}");
        }
    }

    /// 登录 401：缺字段走**校验**分支（不是 `Invalid credentials`）。
    #[tokio::test]
    async fn login_missing_fields_reports_required_errors() {
        let pool = pool_or_skip!();

        let (_, body) = body_json(
            login_impl(&pool, &json!({"username": SEED_FARMER}))
                .await
                .expect("校验失败是 Ok 分支（成功体形状），不是 ApiReject"),
        )
        .await;
        assert_eq!(body["message"]["password"][0], ERR_REQUIRED);
        assert!(body["message"].get("username").is_none(), "{body}");

        let (_, body) =
            body_json(login_impl(&pool, &json!({})).await.expect("空体是 Ok 分支")).await;
        assert_eq!(body["message"]["username"][0], ERR_REQUIRED);
        assert_eq!(body["message"]["password"][0], ERR_REQUIRED);
    }

    /// 登录 200：种子的 Django pbkdf2 串能验通，且响应形状与夹具一致。
    #[tokio::test]
    async fn login_with_seed_account_returns_session_payload() {
        let pool = pool_or_skip!();

        let (status, body) = body_json(
            login_impl(
                &pool,
                &json!({"username": SEED_FARMER, "password": SEED_FARMER_PASSWORD}),
            )
            .await
            .expect("校验失败是 Ok 分支（成功体形状），不是 ApiReject"),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["message"], MSG_LOGIN_SUCCESS);

        let data = &body["data"];
        assert!(
            Uuid::parse_str(data["token"].as_str().unwrap()).is_ok(),
            "{body}"
        );
        assert!(
            data["expiresAt"].as_str().unwrap().ends_with("+00:00"),
            "{body}"
        );
        assert_eq!(data["user"]["username"], SEED_FARMER);
        assert_eq!(data["user"]["role"], "farmer");

        // 蓝本 `_create_session`：先删该用户全部旧 token，只留新的一条。
        let token: Uuid = data["token"].as_str().unwrap().parse().unwrap();
        let count: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM auth_token t JOIN "user" u ON u.id = t.user_id
                WHERE u.username = $1"#,
        )
        .bind(SEED_FARMER)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "登录应清掉旧 token");

        // 拿这条 token 走完整鉴权链路。
        let user = auth::authenticate(&pool, &auth_headers(Some(&token.to_string())))
            .await
            .expect("新 token 必须能过鉴权");
        assert_eq!(user.username, SEED_FARMER);

        // 复原成"无 token"状态，免得影响其它用例/域。
        sqlx::query("DELETE FROM auth_token WHERE key = $1")
            .bind(token)
            .execute(&pool)
            .await
            .unwrap();
    }

    /// 鉴权三态：没有凭证 / 头格式错 / token 不存在。
    #[tokio::test]
    async fn missing_credentials_malformed_and_unknown_token() {
        let pool = pool_or_skip!();

        let cases = [
            (auth_headers(None), auth::ERR_MISSING_CREDENTIALS),
            (
                std::iter::once((header::AUTHORIZATION, "Token not-bearer".parse().unwrap()))
                    .collect(),
                auth::ERR_MALFORMED_HEADER,
            ),
            (
                auth_headers(Some("00000000-0000-4000-8000-000000000000")),
                auth::ERR_INVALID_TOKEN,
            ),
        ];

        for (headers, expected) in cases {
            let (status, body) =
                body_json(me_impl(&pool, &headers).await.unwrap_err().into_response()).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(body["message"], expected);
            assert_eq!(body["data"], Value::Null);
            assert!(body.get("timestamp").is_none(), "{body}");
        }
    }

    /// 401 必须带 `WWW-Authenticate: Bearer`（蓝本 `authenticate_header`）。
    #[tokio::test]
    async fn unauthorized_response_carries_www_authenticate() {
        let response = auth::auth_error(StatusCode::UNAUTHORIZED, "x");
        assert_eq!(
            response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer"
        );
    }

    /// 405：全角引号 + 句末句号 + 无 timestamp。
    #[tokio::test]
    async fn method_not_allowed_message_matches_drf_zh_hans() {
        let response = auth::render_method_not_allowed(&Method::DELETE);
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let (_, body) = body_json(response).await;
        assert_eq!(body["code"], 405);
        assert_eq!(body["message"], "方法 “DELETE” 不被允许。");
        assert_eq!(body["data"], Value::Null);
        assert!(body.get("timestamp").is_none(), "{body}");
    }

    /// 完整走一遍「注册 → me → logout → 再 logout」。
    #[tokio::test]
    async fn register_then_me_then_logout_round_trip() {
        let pool = pool_or_skip!();
        let username = unique_username("w0ba_flow");

        let (_, registered) = body_json(
            register_impl(
                &pool,
                &json!({"username": username, "password": "qa-pass-123456", "role": "buyer"}),
            )
            .await
            .expect("校验失败是 Ok 分支（成功体形状），不是 ApiReject"),
        )
        .await;
        let token = registered["data"]["token"].as_str().unwrap().to_string();

        let (status, body) =
            body_json(me_impl(&pool, &auth_headers(Some(&token))).await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["message"], "success");
        assert_eq!(body["data"]["username"], username);
        assert_eq!(body["data"]["role"], "buyer");

        let (status, body) = body_json(
            logout_impl(&pool, &auth_headers(Some(&token)))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["message"], MSG_LOGOUT_SUCCESS);
        assert_eq!(body["data"], Value::Null);

        // 同一条 token 再来一次：已经被删，转成 401 `登录凭证无效`。
        let (reject_status, reject_body) = body_json(
            logout_impl(&pool, &auth_headers(Some(&token)))
                .await
                .unwrap_err()
                .into_response(),
        )
        .await;
        assert_eq!(reject_status, StatusCode::UNAUTHORIZED);
        assert_eq!(reject_body["message"], auth::ERR_INVALID_TOKEN);

        drop_user(&pool, &username).await;
    }

    /// 过期 token：先删行，再报 `登录已过期，请重新登录`。
    #[tokio::test]
    async fn expired_token_is_deleted_then_reported() {
        let pool = pool_or_skip!();
        let username = unique_username("w0ba_expired");

        let (_, registered) = body_json(
            register_impl(
                &pool,
                &json!({"username": username, "password": "qa-pass-123456", "role": "buyer"}),
            )
            .await
            .expect("校验失败是 Ok 分支（成功体形状），不是 ApiReject"),
        )
        .await;
        let token: Uuid = registered["data"]["token"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        sqlx::query("UPDATE auth_token SET expires_at = $1 WHERE key = $2")
            .bind(Utc::now() - Duration::days(1))
            .bind(token)
            .execute(&pool)
            .await
            .unwrap();

        let err = auth::authenticate(&pool, &auth_headers(Some(&token.to_string())))
            .await
            .unwrap_err();
        let (status, body) = body_json(err.into_response()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["message"], auth::ERR_EXPIRED_TOKEN);

        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auth_token WHERE key = $1")
            .bind(token)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0, "过期 token 必须被删掉");

        drop_user(&pool, &username).await;
    }

    /// 角色守卫：买家过不了 `require_farmer`。
    #[tokio::test]
    async fn role_guards_reject_wrong_role() {
        let pool = pool_or_skip!();

        let (_, logged_in) = body_json(
            login_impl(
                &pool,
                &json!({"username": SEED_BUYER, "password": SEED_BUYER_PASSWORD}),
            )
            .await
            .expect("校验失败是 Ok 分支（成功体形状），不是 ApiReject"),
        )
        .await;
        let token = logged_in["data"]["token"].as_str().unwrap().to_string();
        let headers = auth_headers(Some(&token));

        let err = auth::require_farmer(&pool, &headers).await.unwrap_err();
        let (status, body) = body_json(err.into_response()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["message"], auth::ERR_FARMER_ONLY);

        assert!(auth::require_buyer(&pool, &headers).await.is_ok());

        sqlx::query(
            r#"DELETE FROM auth_token WHERE user_id = (SELECT id FROM "user" WHERE username = $1)"#,
        )
        .bind(SEED_BUYER)
        .execute(&pool)
        .await
        .unwrap();
    }

    /// 备用通道：没有 `Authorization` 时 Cookie / `X-Session-Token` 兜底。
    #[tokio::test]
    async fn fallback_channels_work_when_authorization_is_absent() {
        let pool = pool_or_skip!();

        let (_, logged_in) = body_json(
            login_impl(
                &pool,
                &json!({"username": SEED_BUYER, "password": SEED_BUYER_PASSWORD}),
            )
            .await
            .expect("校验失败是 Ok 分支（成功体形状），不是 ApiReject"),
        )
        .await;
        let token = logged_in["data"]["token"].as_str().unwrap().to_string();

        let mut cookie_headers = axum::http::HeaderMap::new();
        cookie_headers.insert(
            header::COOKIE,
            format!("other=1; session_token={token}").parse().unwrap(),
        );
        assert!(auth::authenticate(&pool, &cookie_headers).await.is_ok());

        let mut token_headers = axum::http::HeaderMap::new();
        token_headers.insert("x-session-token", token.parse().unwrap());
        assert!(auth::authenticate(&pool, &token_headers).await.is_ok());

        // `Authorization` 存在但格式无效 -> **不回落**，直接 401。
        let mut both = token_headers.clone();
        both.insert(header::AUTHORIZATION, "Token nope".parse().unwrap());
        let err = auth::authenticate(&pool, &both).await.unwrap_err();
        let (_, body) = body_json(err.into_response()).await;
        assert_eq!(body["message"], auth::ERR_MALFORMED_HEADER);

        sqlx::query(
            r#"DELETE FROM auth_token WHERE user_id = (SELECT id FROM "user" WHERE username = $1)"#,
        )
        .bind(SEED_BUYER)
        .execute(&pool)
        .await
        .unwrap();
    }

    /// router 能装配起来（不构造 `AppState`，只验证路由表不 panic）。
    #[test]
    fn router_builds_without_state() {
        let router = router();
        let _ = router;
    }

    /// 405 fallback 真的被 MethodRouter 接住（用 `Router<()>` 的等价装配验证）。
    #[tokio::test]
    async fn method_router_fallback_answers_405() {
        let app: Router = Router::new().route(
            "/api/me",
            get(|| async { "ok" }).fallback(handle_unallowed_method),
        );

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method(Method::DELETE)
                    .uri("/api/me")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let (_, body) = body_json(response).await;
        assert_eq!(body["message"], "方法 “DELETE” 不被允许。");
    }
}
