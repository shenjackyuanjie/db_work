use axum::{
    extract::{Json, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::models::{Invitation, PendingUser, User};
use crate::server::AppState;

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

/// Request payload for registration (requires invitation code)
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    pub invitation_code: String,
}

/// Request payload for logout
#[derive(Debug, Deserialize)]
pub struct LogoutRequest {
    pub token: String,
}

/// Request payload for admin to set a user's admin flag
#[derive(Debug, Deserialize)]
pub struct SetAdminRequest {
    pub target_username: String,
    pub make_admin: bool,
    pub admin_token: String,
}

/// Request payload for admin to create invitation
#[derive(Debug, Deserialize)]
pub struct CreateInvitationRequest {
    pub admin_token: String,
    pub ttl_seconds: Option<u64>,
}

/// Request payload for admin to list invitations
#[derive(Debug, Deserialize)]
pub struct ListInvitationsRequest {
    pub admin_token: String,
}

/// Request payload for admin to list users
#[derive(Debug, Deserialize)]
pub struct ListUsersRequest {
    pub admin_token: String,
}

/// Request payload for admin to list pending users
#[derive(Debug, Deserialize)]
pub struct ListPendingUsersRequest {
    pub admin_token: String,
}

/// Request payload for admin to approve pending user
#[derive(Debug, Deserialize)]
pub struct ApprovePendingUserRequest {
    pub admin_token: String,
    pub username: String,
}

/// Request payload for admin to reject pending user
#[derive(Debug, Deserialize)]
pub struct RejectPendingUserRequest {
    pub admin_token: String,
    pub username: String,
}

/// Request payload for token validation
#[derive(Debug, Deserialize)]
pub struct ValidateTokenRequest {
    pub token: String,
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
}

/// Generate a UUID v4 token
fn generate_token(username: &str) -> String {
    let uuid = Uuid::new_v4();
    format!("{}-{}", username, uuid)
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
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn ensure_admin(state: &AppState, admin_token: &str) -> Result<String, (StatusCode, serde_json::Value)> {
    let tokens = state.tokens.lock().unwrap();
    let admin_username = match tokens.get(admin_token) {
        Some(name) => name.clone(),
        None => {
            return Err((StatusCode::UNAUTHORIZED, json!({ "error": "Invalid admin token" })));
        }
    };
    drop(tokens);

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
    let users = state.users.lock().unwrap();
    if let Some(user) = users.get(&payload.username).filter(|user| verify_password(&user.password_hash, &payload.password)) {
        let token = generate_token(&payload.username);
        state.tokens.lock().unwrap().insert(token.clone(), payload.username.clone());
        let resp = LoginResponse {
            token,
            is_admin: user.is_admin,
        };
        return (StatusCode::OK, Json(resp)).into_response();
    }
    (StatusCode::UNAUTHORIZED, Json(json!({"error":"Invalid credentials"}))).into_response()
}

/// Register handler – requires a valid invitation
pub async fn register_handler(
    State(state): State<AppState>,
    Json(payload): Json<RegisterRequest>,
) -> impl IntoResponse {
    if payload.invitation_code.trim().is_empty() {
        let users = state.users.lock().unwrap();
        if users.contains_key(&payload.username) {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error":"Username already exists"})),
            )
                .into_response();
        }
        drop(users);

        let mut pending = state.pending_users.lock().unwrap();
        if pending.contains_key(&payload.username) {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error":"Username already pending approval"})),
            )
                .into_response();
        }

        let pending_user = PendingUser {
            username: payload.username.clone(),
            password_hash: hash_password(&payload.password),
            created_at: now_secs(),
        };
        pending.insert(payload.username.clone(), pending_user);

        return (
            StatusCode::ACCEPTED,
            Json(json!({"status":"pending_approval"})),
        )
            .into_response();
    }

    let mut invites = state.invitations.lock().unwrap();
    match invites.get_mut(&payload.invitation_code) {
        Some(inv) if !inv.used && inv.expires_at > now_secs() => {
            inv.used = true;
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"Invalid or expired invitation"})),
            )
                .into_response()
        }
    }

    let mut users = state.users.lock().unwrap();
    if users.contains_key(&payload.username) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"Username already exists"})),
        )
            .into_response();
    }

    let user = User {
        username: payload.username.clone(),
        password_hash: hash_password(&payload.password),
        is_admin: false,
        created_at: now_secs(),
        session_token: None,
    };
    users.insert(payload.username.clone(), user);
    (StatusCode::CREATED, Json(json!({"status":"registered"}))).into_response()
}

/// Logout handler – removes token
pub async fn logout_handler(
    State(state): State<AppState>,
    Json(payload): Json<LogoutRequest>,
) -> impl IntoResponse {
    let mut tokens = state.tokens.lock().unwrap();
    if tokens.remove(&payload.token).is_some() {
        (StatusCode::OK, Json(json!({"status":"logged out"}))).into_response()
    } else {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"Invalid token"})),
        )
            .into_response()
    }
}

/// Validate token handler
pub async fn validate_token_handler(
    State(state): State<AppState>,
    Json(payload): Json<ValidateTokenRequest>,
) -> impl IntoResponse {
    let tokens = state.tokens.lock().unwrap();
    if let Some(username) = tokens.get(&payload.token) {
        let users = state.users.lock().unwrap();
        let user = users.get(username).unwrap();
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
    (StatusCode::OK, Json(json!({"valid": false}))).into_response()
}

/// Current user info
pub async fn me_handler(
    State(state): State<AppState>,
    Json(payload): Json<ValidateTokenRequest>,
) -> impl IntoResponse {
    let tokens = state.tokens.lock().unwrap();
    let username = match tokens.get(&payload.token) {
        Some(name) => name.clone(),
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Invalid token" })),
            )
                .into_response()
        }
    };
    drop(tokens);

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
    Json(payload): Json<SetAdminRequest>,
) -> impl IntoResponse {
    if let Err((code, body)) = ensure_admin(&state, &payload.admin_token) {
        return (code, Json(body)).into_response();
    }

    let mut users = state.users.lock().unwrap();
    if let Some(target) = users.get_mut(&payload.target_username) {
        target.is_admin = payload.make_admin;
        (StatusCode::OK, Json(json!({"status":"updated"}))).into_response()
    } else {
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
    Json(payload): Json<CreateInvitationRequest>,
) -> impl IntoResponse {
    if let Err((code, body)) = ensure_admin(&state, &payload.admin_token) {
        return (code, Json(body)).into_response();
    }

    let ttl = payload.ttl_seconds.unwrap_or(24 * 60 * 60);
    let code = Uuid::new_v4().to_string();
    let invitation = Invitation {
        code: code.clone(),
        used: false,
        expires_at: now_secs() + ttl,
    };

    let mut invites = state.invitations.lock().unwrap();
    invites.insert(code.clone(), invitation);

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
    Json(payload): Json<ListInvitationsRequest>,
) -> impl IntoResponse {
    if let Err((code, body)) = ensure_admin(&state, &payload.admin_token) {
        return (code, Json(body)).into_response();
    }

    let invites = state.invitations.lock().unwrap();
    let list: Vec<Invitation> = invites.values().cloned().collect();
    (StatusCode::OK, Json(json!({ "invitations": list }))).into_response()
}

/// Admin handler to list users
pub async fn list_users_handler(
    State(state): State<AppState>,
    Json(payload): Json<ListUsersRequest>,
) -> impl IntoResponse {
    if let Err((code, body)) = ensure_admin(&state, &payload.admin_token) {
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
    Json(payload): Json<ListPendingUsersRequest>,
) -> impl IntoResponse {
    if let Err((code, body)) = ensure_admin(&state, &payload.admin_token) {
        return (code, Json(body)).into_response();
    }

    let pending = state.pending_users.lock().unwrap();
    let list: Vec<PendingPublicUser> = pending
        .values()
        .map(|u| PendingPublicUser {
            username: u.username.clone(),
            created_at: u.created_at,
        })
        .collect();

    (StatusCode::OK, Json(json!({ "pending_users": list }))).into_response()
}

/// Admin handler to approve pending user
pub async fn approve_pending_user_handler(
    State(state): State<AppState>,
    Json(payload): Json<ApprovePendingUserRequest>,
) -> impl IntoResponse {
    if let Err((code, body)) = ensure_admin(&state, &payload.admin_token) {
        return (code, Json(body)).into_response();
    }

    let mut pending = state.pending_users.lock().unwrap();
    let pending_user = match pending.remove(&payload.username) {
        Some(u) => u,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Pending user not found" })),
            )
                .into_response()
        }
    };
    drop(pending);

    let mut users = state.users.lock().unwrap();
    if users.contains_key(&pending_user.username) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "Username already exists" })),
        )
            .into_response();
    }

    let user = User {
        username: pending_user.username.clone(),
        password_hash: pending_user.password_hash,
        is_admin: false,
        created_at: pending_user.created_at,
        session_token: None,
    };
    users.insert(user.username.clone(), user);

    (StatusCode::OK, Json(json!({ "status": "approved" }))).into_response()
}

/// Admin handler to reject pending user
pub async fn reject_pending_user_handler(
    State(state): State<AppState>,
    Json(payload): Json<RejectPendingUserRequest>,
) -> impl IntoResponse {
    if let Err((code, body)) = ensure_admin(&state, &payload.admin_token) {
        return (code, Json(body)).into_response();
    }

    let mut pending = state.pending_users.lock().unwrap();
    if pending.remove(&payload.username).is_some() {
        (StatusCode::OK, Json(json!({ "status": "rejected" }))).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Pending user not found" })),
        )
            .into_response()
    }
}

/// Build the router for all user‑related endpoints
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
