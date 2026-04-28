use axum::{Router, routing::post};

use crate::server::AppState;

mod admin;
mod auth;
mod dto;
mod registration;
mod session;

pub(crate) use auth::{ensure_admin, extract_auth_token, now_secs, parse_requested_role};
pub(crate) use dto::{
    ApprovePendingUserRequest, CreateInvitationRequest, OrchardOverviewRequest,
    PendingPublicUser, PublicUser, RejectPendingUserRequest, SetAdminRequest,
    UpdateSystemSettingsRequest,
};
pub(crate) use registration::register_handler;
pub(crate) use session::{login_handler, logout_handler, me_handler, validate_token_handler};

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/login", post(login_handler))
        .route("/register", post(register_handler))
        .route("/logout", post(logout_handler))
        .route("/validate", post(validate_token_handler))
        .route("/me", post(me_handler))
        .route("/admin/set_admin", post(admin::set_admin_handler))
        .route(
            "/admin/invitations/create",
            post(admin::create_invitation_handler),
        )
        .route(
            "/admin/invitations/list",
            post(admin::list_invitations_handler),
        )
        .route("/admin/users/list", post(admin::list_users_handler))
        .route(
            "/admin/settings/get",
            post(admin::get_system_settings_handler),
        )
        .route(
            "/admin/settings/update",
            post(admin::update_system_settings_handler),
        )
        .route(
            "/admin/orchard/overview",
            post(admin::orchard_overview_handler),
        )
        .route(
            "/admin/dashboard/stats",
            post(admin::dashboard_stats_handler),
        )
        .route("/admin/dashboard/logs", post(admin::dashboard_logs_handler))
        .route(
            "/admin/pending/list",
            post(admin::list_pending_users_handler),
        )
        .route(
            "/admin/pending/approve",
            post(admin::approve_pending_user_handler),
        )
        .route(
            "/admin/pending/reject",
            post(admin::reject_pending_user_handler),
        )
        .with_state(state)
}
