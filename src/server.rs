use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::post,
};
use serde_json::json;
use std::env;
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};

use crate::client::OpenRouterClient;
use crate::models::ChatApiRequest;

#[derive(Clone)]
pub struct AppState {
    pub client: OpenRouterClient,
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
pub async fn citrus_analyze_handler(
    State(state): State<AppState>,
    Json(request): Json<ChatApiRequest>,
) -> impl IntoResponse {
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
        }
        Err(e) => {
            println!("柑橘分析请求处理失败: {}", e);
            let error_response = json!({
                "success": false,
                "error": e.to_string(),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

pub fn create_router() -> Router {
    let api_key = env::var("OPENROUTER_API_KEY").expect("请设置环境变量 OPENROUTER_API_KEY");

    let client = OpenRouterClient::new(api_key);
    let state = AppState { client };

    Router::new()
        .route("/health", axum::routing::get(health_handler))
        .route("/chat", post(chat_handler))
        .route("/citrus/analyze", post(citrus_analyze_handler))
        .with_state(state)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
}

pub async fn run_server(addr: SocketAddr) {
    tracing_subscriber::fmt::init();

    let app = create_router();

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("无法绑定到地址");

    tracing::info!("GLM API 服务器正在监听: {}", addr);

    axum::serve(listener, app).await.expect("服务器运行失败");
}
