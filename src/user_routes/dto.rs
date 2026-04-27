use serde::{Deserialize, Serialize};

use crate::models::RequestedRole;

#[derive(Debug, Deserialize)]
pub(crate) struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RegisterRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub invitation_code: String,
    #[serde(default)]
    pub requested_role: RequestedRole,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SetAdminRequest {
    pub target_username: String,
    pub make_admin: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateInvitationRequest {
    pub ttl_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ApprovePendingUserRequest {
    pub username: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RejectPendingUserRequest {
    pub username: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateSystemSettingsRequest {
    pub open_registration: bool,
    pub invite_bypass_enabled: bool,
    pub maintenance_mode: bool,
    pub default_invite_ttl_seconds: i64,
    pub confidence_threshold: f64,
    pub log_retention_days: i32,
}

#[derive(Debug, Serialize)]
pub(crate) struct PublicUser {
    pub username: String,
    pub is_admin: bool,
    pub created_at: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct PendingPublicUser {
    pub username: String,
    pub created_at: u64,
    pub requested_role: RequestedRole,
}