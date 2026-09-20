//! Bearer 鉴权、角色守卫与密码校验。
//!
//! 契约要点（来自 `tests/fixtures/contract/REPORT.md` 的文案字典）：
//! - 无 `Authorization` 头 → 401 `身份认证信息未提供。`
//! - 头格式不对 → 401 `Authorization 请求头格式无效`
//! - token 查不到 → 401 `登录凭证无效`
//! - token 过期 → 401 `登录已过期，请重新登录`（并删除该 token）
//! - 角色不符 → 403 `该接口仅限果农使用` / `该接口仅限购买者使用`
//!
//! 鉴权通道优先级由 `authentication.py::BearerTokenAuthentication` 与蓝本实际部署决定：
//! `Authorization` 头**存在且 trim 后非空**时严格按 Bearer 解析，不合格立即 401，**不回落**
//! 到其它通道；头缺失或 trim 后为空时再依次试 Cookie `session_token`、头 `X-Session-Token`。
//!
//! 密码三格式：
//! - `$argon2*`（历史自研写入）→ `argon2` crate 校验；
//! - `pbkdf2_sha256$<rounds>$<salt>$<b64>`（**Django 存盘格式，现在新建用户也写这个**）
//!   → `pbkdf2` + `hmac` + `sha2` 自行实现（`salt` 用 UTF-8 原始字节，
//!   Django 是 `force_bytes(salt)`，**不要** b64 解码）；
//! - 其它 → 旧 `blake3` 十六进制常量时间比对。

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordVerifier},
};
use axum::{
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rand_core::OsRng;
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::{errors::ApiReject, ser};

/// 新建用户的 Django 兼容口令参数（`django.conf.global_settings.PBKDF2_ITERATIONS`）。
const PBKDF2_ITERATIONS: u32 = 720_000;

/// 登录会话有效期，对应 `models.default_auth_token_expiry` 的 `timedelta(days=30)`。
const TOKEN_TTL_DAYS: i64 = 30;

/// 备用鉴权通道：与旧自研层共用的 Cookie 名与头名。
const COOKIE_TOKEN_NAME: &str = "session_token";
const FALLBACK_HEADER_NAME: &str = "x-session-token";

/// 错误文案字典（逐字取自 `REPORT.md §5.2`）。测试模块会逐条比对，故对 crate 可见。
pub(crate) const ERR_MISSING_CREDENTIALS: &str = "身份认证信息未提供。";
pub(crate) const ERR_MALFORMED_HEADER: &str = "Authorization 请求头格式无效";
pub(crate) const ERR_INVALID_TOKEN: &str = "登录凭证无效";
pub(crate) const ERR_EXPIRED_TOKEN: &str = "登录已过期，请重新登录";
pub(crate) const ERR_FARMER_ONLY: &str = "该接口仅限果农使用";
pub(crate) const ERR_BUYER_ONLY: &str = "该接口仅限购买者使用";
/// 通过鉴权的调用者：`UserSerializer` 需要的全部字段 + 当前 token（logout 要按它删行）。
#[derive(Debug, Clone)]
pub(crate) struct AuthUser {
    pub(crate) id: Uuid,
    pub(crate) username: String,
    pub(crate) email: Option<String>,
    pub(crate) role: String,
    pub(crate) orchard_address: Option<String>,
    pub(crate) latitude: Option<f64>,
    pub(crate) longitude: Option<f64>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) token_key: Uuid,
}

impl AuthUser {
    /// `UserSerializer` 的字段序：`id, username, email, role, orchard_address, latitude,
    /// longitude, created_at`。`created_at` 走 DRF `JSONEncoder` 的 `Z` 形态。
    pub(crate) fn payload(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "username": self.username,
            "email": self.email,
            "role": self.role,
            "orchard_address": self.orchard_address,
            "latitude": self.latitude,
            "longitude": self.longitude,
            "created_at": ser::dt_z(self.created_at),
        })
    }

    pub(crate) fn is_farmer(&self) -> bool {
        self.role == "farmer"
    }

    pub(crate) fn is_buyer(&self) -> bool {
        self.role == "buyer"
    }
}

// --------------------------------------------------------------------------------------
// 响应装配
// --------------------------------------------------------------------------------------

/// 鉴权失败的 DRF 异常体 + `WWW-Authenticate: Bearer`。
///
/// `authentication.py::authenticate_header` 返回 `'Bearer'`，DRF 把它挂到**所有**
/// 由该认证器引发的 401/403 上——所以这里不能在 `errors.rs` 里统一加（那是冻结文件，
/// 且其它域的匿名接口没有这个头）。
fn with_www_authenticate(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

/// 鉴权失败（401 / 403）：DRF 异常体，**无 `timestamp`**，带 `WWW-Authenticate`。
pub(crate) fn auth_error(status: StatusCode, message: impl Into<String>) -> Response {
    with_www_authenticate(ApiReject::new(status, message).into_response())
}

/// 鉴权失败（401 / 403），`message` 是对象（DRF 校验错误形状）。
pub(crate) fn auth_error_json(status: StatusCode, message: Value) -> Response {
    with_www_authenticate(ApiReject::with_json(status, message).into_response())
}

/// 405：`方法 “DELETE” 不被允许。`（DRF zh-hans 本地化，**全角引号 + 句末句号**）
///
/// 与 401/403 一样走 DRF 异常体（`{code, message, data}`，**无 timestamp**），
/// 且没有 `WWW-Authenticate`——405 由 `MethodNotAllowed` 抛出，不经过认证器。
pub(crate) fn render_method_not_allowed(method: &axum::http::Method) -> Response {
    let message = format!("方法 “{}” 不被允许。", method.as_str());
    ApiReject::new(StatusCode::METHOD_NOT_ALLOWED, message).into_response()
}

// --------------------------------------------------------------------------------------
// token 提取
// --------------------------------------------------------------------------------------

/// 从 `Authorization: Bearer <token>` 里取 token。
///
/// 蓝本用 `authorization.split()` 后判 `len(parts) != 2`，所以「多个空格」被容忍、
/// 「多一个词」被拒绝，且 keyword 大小写不敏感。
///
/// 返回 `Ok(None)` = 没有可用的 Authorization 头（可以走备用通道）；
/// 返回 `Err(())` = 头存在但格式不对（必须立即 401，不回落）。
pub(crate) fn bearer_token(headers: &HeaderMap) -> Result<Option<String>, ()> {
    let Some(raw) = headers.get(header::AUTHORIZATION) else {
        return Ok(None);
    };
    let text = raw.to_str().map_err(|_| ())?;
    if text.trim().is_empty() {
        return Ok(None);
    }

    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() != 2 || !parts[0].eq_ignore_ascii_case("Bearer") {
        return Err(());
    }
    Ok(Some(parts[1].to_string()))
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|part| {
        part.trim()
            .strip_prefix(COOKIE_TOKEN_NAME)
            .and_then(|rest| rest.strip_prefix('='))
            .map(str::to_string)
            .filter(|value| !value.is_empty())
    })
}

fn fallback_header_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(FALLBACK_HEADER_NAME)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

// --------------------------------------------------------------------------------------
// token 查库
// --------------------------------------------------------------------------------------

/// 按 `auth_token.key` 查调用者。
///
/// UUID 解析失败或无行 → `登录凭证无效`；命中但 `expires_at <= now` → **先删该 token**
/// 再报 `登录已过期，请重新登录`（蓝本 `token.delete()` 的顺序如此）。
///
/// `"user"` 是 PostgreSQL 保留字，必须双引号。
pub(crate) async fn lookup_token(pool: &PgPool, token: &str) -> Result<AuthUser, ApiReject> {
    let Ok(key) = Uuid::parse_str(token) else {
        return Err(ApiReject::unauthorized(ERR_INVALID_TOKEN));
    };

    let row = sqlx::query(
        r#"SELECT t.key        AS token_key,
                  t.expires_at AS expires_at,
                  u.id         AS user_id,
                  u.username   AS username,
                  u.email      AS email,
                  u.role       AS role,
                  u.orchard_address AS orchard_address,
                  u.latitude   AS latitude,
                  u.longitude  AS longitude,
                  u.created_at AS created_at
             FROM auth_token t
             JOIN "user" u ON u.id = t.user_id
            WHERE t.key = $1"#,
    )
    .bind(key)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    let Some(row) = row else {
        return Err(ApiReject::unauthorized(ERR_INVALID_TOKEN));
    };

    let expires_at: DateTime<Utc> = row.try_get("expires_at").map_err(internal_error)?;
    if expires_at <= Utc::now() {
        sqlx::query("DELETE FROM auth_token WHERE key = $1")
            .bind(key)
            .execute(pool)
            .await
            .map_err(internal_error)?;
        return Err(ApiReject::unauthorized(ERR_EXPIRED_TOKEN));
    }

    Ok(AuthUser {
        id: row.try_get("user_id").map_err(internal_error)?,
        username: row.try_get("username").map_err(internal_error)?,
        email: row.try_get("email").map_err(internal_error)?,
        role: row.try_get("role").map_err(internal_error)?,
        orchard_address: row.try_get("orchard_address").map_err(internal_error)?,
        latitude: row.try_get("latitude").map_err(internal_error)?,
        longitude: row.try_get("longitude").map_err(internal_error)?,
        created_at: row.try_get("created_at").map_err(internal_error)?,
        token_key: row.try_get("token_key").map_err(internal_error)?,
    })
}

/// 未预期错误：蓝本会冒泡成 DRF 的 500 `Internal server error`（异常体，无 timestamp）。
fn internal_error(err: sqlx::Error) -> ApiReject {
    tracing::error!("compat 账号域查询失败: {err}");
    ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}

// --------------------------------------------------------------------------------------
// 鉴权入口与角色守卫
// --------------------------------------------------------------------------------------

/// `Authorization` 存在且非空 → 严格 Bearer，不合格立即 401 且**不回落**；
/// 否则依次试 Cookie、`X-Session-Token`；都没有 → 401 `身份认证信息未提供。`
pub(crate) async fn authenticate(
    pool: &PgPool,
    headers: &HeaderMap,
) -> Result<AuthUser, ApiReject> {
    let token = match bearer_token(headers) {
        Err(()) => return Err(ApiReject::unauthorized(ERR_MALFORMED_HEADER)),
        Ok(Some(token)) => token,
        Ok(None) => match cookie_token(headers).or_else(|| fallback_header_token(headers)) {
            Some(token) => token,
            None => return Err(ApiReject::unauthorized(ERR_MISSING_CREDENTIALS)),
        },
    };

    lookup_token(pool, &token).await
}

/// `IsFarmer`：非果农（含匿名）一律 403 `该接口仅限果农使用`。
pub(crate) async fn require_farmer(
    pool: &PgPool,
    headers: &HeaderMap,
) -> Result<AuthUser, ApiReject> {
    let user = authenticate(pool, headers).await?;
    if user.is_farmer() {
        Ok(user)
    } else {
        Err(ApiReject::forbidden(ERR_FARMER_ONLY))
    }
}

/// `IsBuyer`：非购买者一律 403 `该接口仅限购买者使用`。
pub(crate) async fn require_buyer(
    pool: &PgPool,
    headers: &HeaderMap,
) -> Result<AuthUser, ApiReject> {
    let user = authenticate(pool, headers).await?;
    if user.is_buyer() {
        Ok(user)
    } else {
        Err(ApiReject::forbidden(ERR_BUYER_ONLY))
    }
}

// --------------------------------------------------------------------------------------
// 会话（token）读写
// --------------------------------------------------------------------------------------

/// `auth_views._create_session`：**先删该用户全部旧 token**，再建一条 30 天有效的新 token。
pub(crate) async fn create_session(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<(Uuid, DateTime<Utc>), ApiReject> {
    let mut tx = pool.begin().await.map_err(internal_error)?;
    let session = create_session_tx(&mut tx, user_id).await?;
    tx.commit().await.map_err(internal_error)?;
    Ok(session)
}

/// `create_session` 的事务版：注册要在一个事务里落 user + orchard + token。
pub(crate) async fn create_session_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> Result<(Uuid, DateTime<Utc>), ApiReject> {
    sqlx::query("DELETE FROM auth_token WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut **tx)
        .await
        .map_err(internal_error)?;

    let key = Uuid::new_v4();
    let now = Utc::now();
    let expires_at = now + Duration::days(TOKEN_TTL_DAYS);

    sqlx::query(
        "INSERT INTO auth_token (key, user_id, created_at, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(key)
    .bind(user_id)
    .bind(now)
    .bind(expires_at)
    .execute(&mut **tx)
    .await
    .map_err(internal_error)?;

    Ok((key, expires_at))
}

/// 登录/注册成功体：`{token, expiresAt, user}`，**键序即蓝本声明序**。
///
/// `expiresAt` 走 Django `isoformat()`（`+00:00`），不是 `Z`。
pub(crate) fn session_payload(token: Uuid, expires_at: DateTime<Utc>, user: &AuthUser) -> Value {
    let mut payload = Map::new();
    payload.insert("token".to_string(), Value::String(token.to_string()));
    payload.insert(
        "expiresAt".to_string(),
        Value::String(ser::dt_offset(expires_at)),
    );
    payload.insert("user".to_string(), user.payload());
    Value::Object(payload)
}

// --------------------------------------------------------------------------------------
// 口令
// --------------------------------------------------------------------------------------

const SALT_CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// Django `make_password` 的等价物：`pbkdf2_sha256$<rounds>$<salt>$<b64 digest>`。
///
/// 刻意**不用** Argon2 写库：Django 认不出 `$argon2*`，切回蓝本时这些用户会全部登不上。
pub(crate) fn hash_password(password: &str) -> String {
    let salt = random_salt();
    let digest = pbkdf2::pbkdf2_hmac_array::<sha2::Sha256, 32>(
        password.as_bytes(),
        salt.as_bytes(),
        PBKDF2_ITERATIONS,
    );
    format!(
        "pbkdf2_sha256${PBKDF2_ITERATIONS}${salt}${}",
        base64::engine::general_purpose::STANDARD.encode(digest)
    )
}

/// Django 的盐是 22 字符的 `RandomStringUtils` 风格串；这里用等价长度的 alnum 串，
/// 字符集是 `password_hash` 的 `Salt` 校验能接受的子集（不含 `.`、`/`、`+`）。
fn random_salt() -> String {
    use rand_core::RngCore;

    let mut bytes = [0u8; 22];
    OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|byte| SALT_CHARSET[(*byte as usize) % SALT_CHARSET.len()] as char)
        .collect()
}

/// `check_password` 的三格式分派。
pub(crate) fn verify_password(stored: &str, password: &str) -> bool {
    if stored.starts_with("$argon2") {
        return verify_argon2(stored, password);
    }
    if stored.starts_with("pbkdf2_sha256$") {
        return verify_pbkdf2_sha256(stored, password);
    }
    // 旧自研层：blake3 十六进制。
    stored == blake3::hash(password.as_bytes()).to_string()
}

fn verify_argon2(stored: &str, password: &str) -> bool {
    PasswordHash::new(stored)
        .ok()
        .and_then(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .ok()
        })
        .is_some()
}

fn verify_pbkdf2_sha256(stored: &str, password: &str) -> bool {
    // 注意 `str::split` 不会为首个 `$` 前产生空段，所以是 4 段而不是 5 段。
    let parts: Vec<&str> = stored.split('$').collect();
    let ["pbkdf2_sha256", rounds, salt, encoded] = parts.as_slice() else {
        return false;
    };

    let Ok(iterations) = rounds.parse::<u32>() else {
        return false;
    };
    let Ok(expected) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return false;
    };

    // Django 的 `force_bytes(salt)`：盐按 UTF-8 原始字节进 PBKDF2，**不要** b64 解码。
    let actual = pbkdf2::pbkdf2_hmac_array::<sha2::Sha256, 32>(
        password.as_bytes(),
        salt.as_bytes(),
        iterations,
    );
    constant_time_eq(&actual, &expected)
}

/// 常量时间比较，避免按字节短路泄漏口令信息。
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }

    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_hash_is_django_pbkdf2_and_verifies() {
        let hash = hash_password("qa-pass-123456");
        assert!(hash.starts_with("pbkdf2_sha256$720000$"), "{hash}");
        assert_eq!(hash.matches('$').count(), 3, "{hash}");
        assert!(verify_password(&hash, "qa-pass-123456"));
        assert!(!verify_password(&hash, "wrong"));
    }

    #[test]
    fn salt_is_re_randomized_per_hash() {
        assert_ne!(hash_password("same"), hash_password("same"));
    }

    /// 夹具 `seed.json` 里 farmer_xinfeng 的真实 pbkdf2 串（口令 `farmer123`）。
    #[test]
    fn seeded_django_hash_from_fixture_verifies() {
        let stored = "pbkdf2_sha256$720000$9v4pzPFovtucXyIxzowUwi$x8sLz0Z+Q4brqnHHJSZK+rJ826ILbbSfyPwPzjwjAy0=";
        assert!(verify_password(stored, "farmer123"));
        assert!(!verify_password(stored, "farmer124"));
    }

    #[test]
    fn legacy_blake3_hash_still_verifies() {
        let legacy = blake3::hash(b"old password").to_string();
        assert!(verify_password(&legacy, "old password"));
        assert!(!verify_password(&legacy, "new password"));
    }

    #[test]
    fn malformed_pbkdf2_never_panics() {
        for broken in [
            "pbkdf2_sha256$abc$x$y",
            "pbkdf2_sha256$720000$salt$not-base64!!",
            "pbkdf2_sha256$720000$salt",
        ] {
            assert!(!verify_password(broken, "whatever"), "{broken}");
        }
    }
}
