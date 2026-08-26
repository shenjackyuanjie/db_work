use axum::{
    Router,
    routing::{post, put},
};

use crate::server::AppState;

mod admin;
mod auth;
mod commerce;
mod dto;
mod registration;
mod session;
mod store;

pub(crate) use auth::{
    ensure_admin, ensure_authenticated, extract_auth_token, now_secs, parse_requested_role,
    username_matches_session,
};
pub(crate) use dto::{
    ApprovePendingUserRequest, CreateInvitationRequest, OrchardOverviewRequest, PendingPublicUser,
    PublicUser, RejectPendingUserRequest, SetAdminRequest, UpdateSystemSettingsRequest,
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
            "/orchard/overview",
            post(admin::orchard_overview_public_handler),
        )
        .route(
            "/commerce/orders",
            post(commerce::create_order_handler).get(commerce::list_user_orders_handler),
        )
        .route(
            "/commerce/orders/{order_id}",
            axum::routing::get(commerce::get_user_order_handler),
        )
        .route(
            "/admin/commerce/orchards",
            post(commerce::create_orchard_handler).get(commerce::list_orchards_handler),
        )
        .route(
            "/admin/commerce/products",
            post(commerce::create_product_handler).get(commerce::list_products_handler),
        )
        .route(
            "/admin/commerce/batches",
            post(commerce::create_batch_handler).get(commerce::list_batches_handler),
        )
        .route(
            "/admin/commerce/orders",
            post(commerce::list_admin_orders_handler),
        )
        .route(
            "/admin/commerce/orders/status",
            post(commerce::update_order_status_handler),
        )
        .route(
            "/admin/commerce/overview",
            post(commerce::commerce_overview_handler),
        )
        .route(
            "/store/orders",
            post(store::create_store_order_handler).get(store::list_user_store_orders_handler),
        )
        .route(
            "/admin/store/products",
            post(store::create_store_product_handler).get(store::list_store_products_admin_handler),
        )
        .route(
            "/admin/store/products/{product_id}/toggle",
            post(store::toggle_store_product_handler),
        )
        .route(
            "/admin/store/products/{product_id}",
            put(store::update_store_product_handler),
        )
        .route(
            "/admin/store/products/{product_id}/cover",
            post(store::upload_store_cover_handler),
        )
        .route(
            "/admin/store/orders",
            post(store::list_admin_store_orders_handler),
        )
        .route(
            "/admin/store/orders/status",
            post(store::update_store_order_status_handler),
        )
        .route("/admin/store/overview", post(store::store_overview_handler))
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
