use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::post,
    Router,
};
use serde_json::json;
use std::env;
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};


use crate::client::GlmClient;
use crate::models::{ChatApiRequest, ChatRequest, ChatMessage};

#[derive(Clone)]
pub struct AppState {
    pub client: GlmClient,
}

pub async fn chat_handler(
    State(state): State<AppState>,
    Json(request): Json<ChatApiRequest>,
) -> impl IntoResponse {
    let content = crate::utils::build_message_content(&request.message, request.image.as_ref());

    let chat_request = ChatRequest {
        model: "glm-4.6v-flash".to_string(),
        messages: vec![
            ChatMessage {
                role: "user".to_string(),
                content,
            },
        ],
        temperature: request.temperature,
        top_p: request.top_p,
        max_tokens: request.max_tokens,
    };

    match state.client.chat_completions(&chat_request).await {
        Ok(response) => {
            let assistant_reply = response.choices
                .first()
                .map(|choice| {
                    match &choice.message.content {
                        serde_json::Value::String(s) => s.clone(),
                        _ => choice.message.content.to_string(),
                    }
                })
                .unwrap_or_else(|| "无法解析响应".to_string());

            let result = json!({
                "id": response.id,
                "model": response.model,
                "message": assistant_reply,
                "usage": response.usage,
            });

            (StatusCode::OK, Json(result))
        }
        Err(e) => {
            let error_response = json!({
                "error": e.to_string(),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

pub async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({
        "status": "ok",
        "service": "glm-api-server"
    })))
}

pub fn create_router() -> Router {
    let api_key = env::var("GLM_API_KEY")
        .expect("请设置环境变量 GLM_API_KEY");

    let client = GlmClient::new(api_key);
    let state = AppState { client };

    Router::new()
        .route("/health", axum::routing::get(health_handler))
        .route("/chat", post(chat_handler))
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

    axum::serve(listener, app)
        .await
        .expect("服务器运行失败");
}