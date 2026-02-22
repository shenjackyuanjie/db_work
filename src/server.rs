use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};

use serde_json::json;
use std::env;
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing_subscriber::EnvFilter;

use crate::client::OpenRouterClient;
use crate::models::ChatApiRequest;

#[derive(Clone)]
pub struct AppState {
    pub client: OpenRouterClient,
    pub users: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, crate::models::User>>>,
    pub invitations: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, crate::models::Invitation>>>,
    pub pending_users: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, crate::models::PendingUser>>>,
    pub tokens: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>, // token -> username
}



pub async fn chat_handler(
    State(state): State<AppState>,
    Json(mut request): Json<ChatApiRequest>,
) -> impl IntoResponse {
    // 如果没有提供 message，使用默认提示词
    if request.message.is_none() {
        request.message = Some("请分析这张图片。".to_string());
    }

    // 使用统一的客户端执行方法，将构建/调用逻辑下沉到 client 中
    // 期望 client 提供 `exec_chat_api` 方法，返回 `Result<serde_json::Value, _>`，
    // 包含与之前一致的字段：id, model, message, usage
    match state.client.exec_chat_api(request).await {
        Ok(json_val) => (StatusCode::OK, Json(json_val)),
        Err(e) => {
            let error_response = json!({
                "error": e.to_string(),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

pub async fn health_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "service": "openrouter"
        })),
    )
}
#[axum::debug_handler]
pub async fn citrus_analyze_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChatApiRequest>,
) -> impl IntoResponse {
    let token = match crate::user_routes::extract_auth_token(&headers).or(request.token.clone()) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Missing token" })),
            )
                .into_response()
        }
    };

    let token_valid = {
        let tokens = state.tokens.lock().unwrap();
        tokens.contains_key(&token)
    };
    if !token_valid {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid token" })),
        )
            .into_response();
    }

    println!(
        "处理请求 图像数据长度: {}",
        request.image.as_ref().map_or(0, |img| img.len())
    );

    // Delegate to unified analyze method on client which returns CitrusAnalysisResponse
    // 用户消息只包含图片，所有指令都在 system prompt 中
    match state.client.analyze_citrus(request.image).await {
        Ok(response) => {
            println!("柑橘分析请求处理成功 usage: {:?}", response.usage);
            (StatusCode::OK, Json(json!({
                "success": true,
                "data": response.data,
                "usage": response.usage,
                "metrics": response.metrics
            })))
                .into_response()
        }
        Err(e) => {
            println!("柑橘分析请求处理失败: {}", e);
            let error_response = json!({
                "success": false,
                "error": e.to_string(),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)).into_response()
        }
    }
}

pub fn create_router() -> Router {
    let api_key = env::var("OPENROUTER_API_KEY").expect("请设置环境变量 OPENROUTER_API_KEY");

    let client = OpenRouterClient::new(api_key);
    let state = AppState {
        client,
        users: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        invitations: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        pending_users: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        tokens: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };

    {
        let mut invites = state.invitations.lock().unwrap();
        invites.insert(
            "1111".to_string(),
            crate::models::Invitation {
                code: "1111".to_string(),
                used: false,
                expires_at: u64::MAX,
            },
        );
    }

    Router::new()
        .route("/health", axum::routing::get(health_handler))
        .route("/", axum::routing::get(|| async { axum::response::Redirect::temporary("/index.html") }))
        .route("/chat", post(chat_handler))
        .route("/citrus/analyze", post(citrus_analyze_handler))
        .nest("/user", crate::user_routes::router(state.clone()))
        .fallback_service(ServeDir::new("static"))
        .with_state(state)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
}

pub fn init_tracing(log_level: &str) {
    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(log_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .try_init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!("监听 Ctrl+C 失败: {}", err);
        }
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(err) => {
                tracing::error!("监听 SIGTERM 失败: {}", err);
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("收到退出信号，正在关闭服务器...");
}

pub async fn run_server(addr: SocketAddr) {
    let app = create_router();

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("无法绑定到地址");

    tracing::info!("AI 服务 服务器正在监听: {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("服务器运行失败");
}

