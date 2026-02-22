use axum::{
    extract::{Json, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
    routing::post,
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use uuid::Uuid;

use crate::models::{Invitation, PendingUser, RequestedRole, User};
use crate::server::AppState;

const SESSION_COOKIE_NAME: &str = "session_token";
const SESSION_HEADER_NAME: &str = "x-session-token";
const SESSION_MAX_AGE_SECONDS: u64 = 30 * 24 * 60 * 60;

/// Request payload for login
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Response payload for login
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub is_admin: bool,
}

/// Request payload for registration
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub invitation_code: String,
    #[serde(default)]
    pub requested_role: RequestedRole,
}

/// Request payload for admin to set a user's admin flag
#[derive(Debug, Deserialize)]
pub struct SetAdminRequest {
    pub target_username: String,
    pub make_admin: bool,
}

/// Request payload for admin to create invitation
#[derive(Debug, Deserialize)]
pub struct CreateInvitationRequest {
    pub ttl_seconds: Option<u64>,
}

/// Request payload for admin to approve pending user
#[derive(Debug, Deserialize)]
pub struct ApprovePendingUserRequest {
    pub username: String,
}

/// Request payload for admin to reject pending user
#[derive(Debug, Deserialize)]
pub struct RejectPendingUserRequest {
    pub username: String,
}

#[derive(Debug, Serialize)]
pub struct PublicUser {
    pub username: String,
    pub is_admin: bool,
    pub created_at: u64,
}

#[derive(Debug, Serialize)]
pub struct PendingPublicUser {
    pub username: String,
    pub created_at: u64,
    pub requested_role: RequestedRole,
}

/// Generate a UUID v4 token
fn generate_token() -> String {
    Uuid::new_v4().to_string()
}

/// Password hashing using blake3
fn hash_password(pw: &str) -> String {
    blake3::hash(pw.as_bytes()).to_string()
}

/// Validate password
fn verify_password(hash: &str, pw: &str) -> bool {
    hash == blake3::hash(pw.as_bytes()).to_string()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn requested_role_label(role: &RequestedRole) -> &'static str {
    if role.is_admin() { "admin" } else { "user" }
}

fn build_login_cookie(token: &str) -> Result<HeaderValue, (StatusCode, serde_json::Value)> {
    let cookie = format!(
        "{SESSION_COOKIE_NAME}={token}; Path=/; SameSite=Lax; Max-Age={SESSION_MAX_AGE_SECONDS}"
    );
    HeaderValue::from_str(&cookie).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Failed to build session cookie" }),
        )
    })
}

fn build_clear_cookie() -> Result<HeaderValue, (StatusCode, serde_json::Value)> {
    let cookie = format!("{SESSION_COOKIE_NAME}=; Path=/; SameSite=Lax; Max-Age=0");
    HeaderValue::from_str(&cookie).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Failed to clear session cookie" }),
        )
    })
}

fn token_from_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie_header.split(';').find_map(|part| {
        let trimmed = part.trim();
        trimmed
            .strip_prefix("session_token=")
            .and_then(|value| (!value.is_empty()).then(|| value.to_string()))
    })
}

pub fn extract_auth_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(SESSION_HEADER_NAME)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string)
        .or_else(|| token_from_cookie(headers))
}

fn ensure_authenticated(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(String, String), (StatusCode, serde_json::Value)> {
    let token = match extract_auth_token(headers) {
        Some(t) => t,
        None => {
            return Err((
                StatusCode::UNAUTHORIZED,
                json!({ "error": "Missing session token" }),
            ));
        }
    };

    let tokens = state.tokens.lock().unwrap();
    let username = match tokens.get(&token) {
        Some(name) => name.clone(),
        None => {
            return Err((StatusCode::UNAUTHORIZED, json!({ "error": "Invalid token" })));
        }
    };

    Ok((token, username))
}

fn ensure_admin(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<String, (StatusCode, serde_json::Value)> {
    let (_, admin_username) = ensure_authenticated(state, headers)?;

    let users = state.users.lock().unwrap();
    let admin_user = match users.get(&admin_username) {
        Some(u) => u,
        None => {
            return Err((StatusCode::UNAUTHORIZED, json!({ "error": "Admin user not found" })));
        }
    };
    if !admin_user.is_admin {
        return Err((StatusCode::FORBIDDEN, json!({ "error": "User is not an admin" })));
    }
    Ok(admin_username)
}

/// Login handler
pub async fn login_handler(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> impl IntoResponse {
    let username = payload.username.trim();
    let password = payload.password.trim();
    info!("用户请求登录: username={}", username);

    if username.is_empty() || password.is_empty() {
        warn!("登录失败: username={}, reason=missing_username_or_password", username);
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"Username and password are required"})),
        )
            .into_response();
    }

    let mut users = state.users.lock().unwrap();
    let user = match users.get_mut(username) {
        Some(u) => u,
        None => {
            warn!("登录失败: username={}, reason=user_not_found", username);
            return (StatusCode::UNAUTHORIZED, Json(json!({"error":"Invalid credentials"})))
                .into_response();
        }
    };

    if !verify_password(&user.password_hash, password) {
        warn!("登录失败: username={}, reason=invalid_password", username);
        return (StatusCode::UNAUTHORIZED, Json(json!({"error":"Invalid credentials"})))
            .into_response();
    }

    let token = generate_token();
    user.session_token = Some(token.clone());
    let is_admin = user.is_admin;
    drop(users);

    state
        .tokens
        .lock()
        .unwrap()
        .insert(token.clone(), username.to_string());

    let cookie_header = match build_login_cookie(&token) {
        Ok(v) => v,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, cookie_header);

    info!("登录成功: username={}, is_admin={}", username, is_admin);

    let resp = LoginResponse { token, is_admin };
    (StatusCode::OK, headers, Json(resp)).into_response()
}

/// Register handler
pub async fn register_handler(
    State(state): State<AppState>,
    Json(payload): Json<RegisterRequest>,
) -> impl IntoResponse {
    let username = payload.username.trim().to_string();
    let password = payload.password.trim().to_string();
    let has_invitation = !payload.invitation_code.trim().is_empty();
    let requested_role = requested_role_label(&payload.requested_role);

    info!(
        "用户请求注册: username={}, requested_role={}, has_invitation={}",
        username, requested_role, has_invitation
    );

    if username.is_empty() || password.is_empty() {
        warn!(
            "注册失败: username={}, reason=missing_username_or_password",
            username
        );
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"Username and password are required"})),
        )
            .into_response();
    }

    {
        let users = state.users.lock().unwrap();
        if users.contains_key(&username) {
            warn!("注册失败: username={}, reason=username_exists", username);
            return (
                StatusCode::CONFLICT,
                Json(json!({"error":"Username already exists"})),
            )
                .into_response();
        }
    }

    {
        let pending = state.pending_users.lock().unwrap();
        if pending.contains_key(&username) {
            warn!("注册失败: username={}, reason=username_already_pending", username);
            return (
                StatusCode::CONFLICT,
                Json(json!({"error":"Username already pending approval"})),
            )
                .into_response();
        }
    }

    let invitation_code = payload.invitation_code.trim();
    if invitation_code.is_empty() {
        let pending_user = PendingUser {
            username: username.clone(),
            password_hash: hash_password(&password),
            created_at: now_secs(),
            requested_role: payload.requested_role.clone(),
        };
        state
            .pending_users
            .lock()
            .unwrap()
            .insert(username.clone(), pending_user);

        info!(
            "注册进入审核队列: username={}, requested_role={}",
            username, requested_role
        );

        return (
            StatusCode::ACCEPTED,
            Json(json!({
                "status":"pending_approval",
                "requested_role": payload.requested_role
            })),
        )
            .into_response();
    }

    let mut invites = state.invitations.lock().unwrap();
    match invites.get_mut(invitation_code) {
        Some(inv) if !inv.used && inv.expires_at > now_secs() => {
            inv.used = true;
        }
        _ => {
            warn!("注册失败: username={}, reason=invalid_or_expired_invitation", username);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error":"Invalid or expired invitation",
                    "hint":"邀请码错误时可清空邀请码并提交审核申请"
                })),
            )
                .into_response();
        }
    }
    drop(invites);

    let user = User {
        username: username.clone(),
        password_hash: hash_password(&password),
        is_admin: payload.requested_role.is_admin(),
        created_at: now_secs(),
        session_token: None,
    };
    state.users.lock().unwrap().insert(username.clone(), user);

    info!(
        "注册成功: username={}, is_admin={}, via_invitation=true",
        username,
        payload.requested_role.is_admin()
    );

    (
        StatusCode::CREATED,
        Json(json!({
            "status":"registered",
            "is_admin": payload.requested_role.is_admin()
        })),
    )
        .into_response()
}

/// Logout handler
pub async fn logout_handler(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let token = match extract_auth_token(&headers) {
        Some(t) => t,
        None => {
            warn!("登出失败: reason=missing_token");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"Missing token"})),
            )
                .into_response();
        }
    };

    let clear_cookie = match build_clear_cookie() {
        Ok(v) => v,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    let mut res_headers = HeaderMap::new();
    res_headers.insert(header::SET_COOKIE, clear_cookie);

    let removed_username = state.tokens.lock().unwrap().remove(&token);
    if let Some(username) = removed_username {
        info!("登出成功: username={}", username);
        (StatusCode::OK, res_headers, Json(json!({"status":"logged out"}))).into_response()
    } else {
        warn!("登出失败: reason=invalid_token");
        (
            StatusCode::BAD_REQUEST,
            res_headers,
            Json(json!({"error":"Invalid token"})),
        )
            .into_response()
    }
}

/// Validate token handler
pub async fn validate_token_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = match extract_auth_token(&headers) {
        Some(t) => t,
        None => return (StatusCode::OK, Json(json!({"valid": false}))).into_response(),
    };

    let tokens = state.tokens.lock().unwrap();
    if let Some(username) = tokens.get(&token) {
        let users = state.users.lock().unwrap();
        if let Some(user) = users.get(username) {
            return (
                StatusCode::OK,
                Json(json!({
                    "valid": true,
                    "username": user.username,
                    "is_admin": user.is_admin
                })),
            )
                .into_response();
        }
    }

    (StatusCode::OK, Json(json!({"valid": false}))).into_response()
}

/// Current user info
pub async fn me_handler(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let (_, username) = match ensure_authenticated(&state, &headers) {
        Ok(v) => v,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    let users = state.users.lock().unwrap();
    let user = users.get(&username).unwrap();
    (
        StatusCode::OK,
        Json(json!({
            "username": user.username,
            "is_admin": user.is_admin,
            "created_at": user.created_at
        })),
    )
        .into_response()
}

/// Admin handler to set admin flag for another user
pub async fn set_admin_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<SetAdminRequest>,
) -> impl IntoResponse {
    let admin_username = match ensure_admin(&state, &headers) {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    info!(
        "管理员请求修改用户权限: admin={}, target_username={}, make_admin={}",
        admin_username, payload.target_username, payload.make_admin
    );

    let mut users = state.users.lock().unwrap();
    if let Some(target) = users.get_mut(&payload.target_username) {
        target.is_admin = payload.make_admin;
        info!(
            "管理员修改用户权限成功: admin={}, target_username={}, make_admin={}",
            admin_username, payload.target_username, payload.make_admin
        );
        (StatusCode::OK, Json(json!({"status":"updated"}))).into_response()
    } else {
        warn!(
            "管理员修改用户权限失败: admin={}, target_username={}, reason=target_not_found",
            admin_username, payload.target_username
        );
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error":"Target user not found"})),
        )
            .into_response()
    }
}

/// Admin handler to create invitation
pub async fn create_invitation_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CreateInvitationRequest>,
) -> impl IntoResponse {
    let admin_username = match ensure_admin(&state, &headers) {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    let ttl = payload.ttl_seconds.unwrap_or(24 * 60 * 60);
    info!(
        "管理员请求创建邀请码: admin={}, ttl_seconds={}",
        admin_username, ttl
    );

    let code = Uuid::new_v4().to_string();
    let invitation = Invitation {
        code: code.clone(),
        used: false,
        expires_at: now_secs() + ttl,
    };

    let mut invites = state.invitations.lock().unwrap();
    invites.insert(code.clone(), invitation);

    info!("管理员创建邀请码成功: admin={}", admin_username);

    (
        StatusCode::OK,
        Json(json!({
            "code": code,
            "expires_at": now_secs() + ttl
        })),
    )
        .into_response()
}

/// Admin handler to list invitations
pub async fn list_invitations_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err((code, body)) = ensure_admin(&state, &headers) {
        return (code, Json(body)).into_response();
    }

    let invites = state.invitations.lock().unwrap();
    let list: Vec<Invitation> = invites.values().cloned().collect();
    (StatusCode::OK, Json(json!({ "invitations": list }))).into_response()
}

/// Admin handler to list users
pub async fn list_users_handler(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if let Err((code, body)) = ensure_admin(&state, &headers) {
        return (code, Json(body)).into_response();
    }

    let users = state.users.lock().unwrap();
    let list: Vec<PublicUser> = users
        .values()
        .map(|u| PublicUser {
            username: u.username.clone(),
            is_admin: u.is_admin,
            created_at: u.created_at,
        })
        .collect();

    (StatusCode::OK, Json(json!({ "users": list }))).into_response()
}

/// Admin handler to list pending users
pub async fn list_pending_users_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err((code, body)) = ensure_admin(&state, &headers) {
        return (code, Json(body)).into_response();
    }

    let pending = state.pending_users.lock().unwrap();
    let list: Vec<PendingPublicUser> = pending
        .values()
        .map(|u| PendingPublicUser {
            username: u.username.clone(),
            created_at: u.created_at,
            requested_role: u.requested_role.clone(),
        })
        .collect();

    (StatusCode::OK, Json(json!({ "pending_users": list }))).into_response()
}

/// Admin handler to approve pending user
pub async fn approve_pending_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<ApprovePendingUserRequest>,
) -> impl IntoResponse {
    let admin_username = match ensure_admin(&state, &headers) {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    info!(
        "管理员请求通过审核: admin={}, username={}",
        admin_username, payload.username
    );

    let mut pending = state.pending_users.lock().unwrap();
    let pending_user = match pending.remove(&payload.username) {
        Some(u) => u,
        None => {
            warn!(
                "管理员通过审核失败: admin={}, username={}, reason=pending_user_not_found",
                admin_username, payload.username
            );
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Pending user not found" })),
            )
                .into_response();
        }
    };
    drop(pending);

    let mut users = state.users.lock().unwrap();
    if users.contains_key(&pending_user.username) {
        warn!(
            "管理员通过审核失败: admin={}, username={}, reason=username_exists",
            admin_username, pending_user.username
        );
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "Username already exists" })),
        )
            .into_response();
    }

    let user = User {
        username: pending_user.username.clone(),
        password_hash: pending_user.password_hash,
        is_admin: pending_user.requested_role.is_admin(),
        created_at: pending_user.created_at,
        session_token: None,
    };
    users.insert(user.username.clone(), user);

    info!(
        "管理员通过审核成功: admin={}, username={}",
        admin_username, payload.username
    );

    (StatusCode::OK, Json(json!({ "status": "approved" }))).into_response()
}

/// Admin handler to reject pending user
pub async fn reject_pending_user_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<RejectPendingUserRequest>,
) -> impl IntoResponse {
    let admin_username = match ensure_admin(&state, &headers) {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };

    info!(
        "管理员请求拒绝审核: admin={}, username={}",
        admin_username, payload.username
    );

    let mut pending = state.pending_users.lock().unwrap();
    if pending.remove(&payload.username).is_some() {
        info!(
            "管理员拒绝审核成功: admin={}, username={}",
            admin_username, payload.username
        );
        (StatusCode::OK, Json(json!({ "status": "rejected" }))).into_response()
    } else {
        warn!(
            "管理员拒绝审核失败: admin={}, username={}, reason=pending_user_not_found",
            admin_username, payload.username
        );
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Pending user not found" })),
        )
            .into_response()
    }
}

/// Build the router for all user-related endpoints
pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/login", post(login_handler))
        .route("/register", post(register_handler))
        .route("/logout", post(logout_handler))
        .route("/validate", post(validate_token_handler))
        .route("/me", post(me_handler))
        .route("/admin/set_admin", post(set_admin_handler))
        .route("/admin/invitations/create", post(create_invitation_handler))
        .route("/admin/invitations/list", post(list_invitations_handler))
        .route("/admin/users/list", post(list_users_handler))
        .route("/admin/pending/list", post(list_pending_users_handler))
        .route("/admin/pending/approve", post(approve_pending_user_handler))
        .route("/admin/pending/reject", post(reject_pending_user_handler))
        .with_state(state)
}
