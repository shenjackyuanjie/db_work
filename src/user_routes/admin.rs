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
        (StatusCode::OK, Json(json!({ "status": "updated" }))).into_response()
    } else {
        warn!(
            "管理员修改用户权限失败: admin={}, target_username={}, reason=target_not_found",
            admin_username, payload.target_username
        );
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Target user not found" })),
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
pub async fn list_users_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
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
