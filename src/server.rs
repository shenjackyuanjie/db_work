use axum::{
    Router,
    body::Body,
    http::Request,
    middleware::Next,
    response::Response,
    routing::{get, post},
};

use sqlx::{PgPool, postgres::PgPoolOptions};
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing_subscriber::EnvFilter;

use crate::client::OpenRouterClient;

mod bootstrap;
mod handlers_ai;
mod handlers_commerce;
mod handlers_core;
mod shared;

pub(crate) use shared::{
    AddTaskRequest, AppState, CompleteTaskRequest, DiseaseTreatmentQuery,
    GenerateDiseaseTaskRequest, GenerateEnvironmentTaskRequest, RECOGNITION_RECORDS_UPLOAD_DIR,
    TagTemperatureHumidityRequest, TaskRecord, TemperatureHumiditySample, UsernameQuery,
    api_response, api_success, build_temp_humidity_payload, classify_environment_risk,
    default_temperature_samples, disease_treatment_text, normalize_recognition_record_image_path,
    now_millis, risk_from_disease_name, save_recognition_record_image, task_payload, user_exists,
    username_by_token,
};

async fn log_request_path(req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();

    const DO_LOG: bool = false;
    if DO_LOG {
        println!("[REQUEST] {} {}", method, uri);
    }

    next.run(req).await
}

pub fn create_router(config: &crate::config::AppConfig, db: PgPool) -> Router {
    let api_key = config.ai.openrouter_api_key.clone();

    let client = OpenRouterClient::new(api_key);
    let inference = crate::inference::InferenceRuntime::new(&config.inference, client.clone());
    let state = AppState {
        client,
        inference,
        db,
    };

    Router::new()
        .route("/health", get(handlers_core::health_handler))
        .route(
            "/",
            get(handlers_core::index_page_handler),
        )
        .route(
            "/index.html",
            get(|| async { axum::response::Redirect::permanent("/") }),
        )
        .route("/api/register", post(crate::user_routes::register_handler))
        .route("/api/login", post(crate::user_routes::login_handler))
        .route("/api/logout", post(crate::user_routes::logout_handler))
        .route(
            "/api/validate",
            post(crate::user_routes::validate_token_handler),
        )
        .route("/api/user", get(handlers_core::api_user_handler))
        .route(
            "/api/system-status",
            get(handlers_core::system_status_api_handler),
        )
        .route("/api/home", get(handlers_core::home_api_handler))
        .route(
            "/api/growth-tracking",
            get(handlers_core::growth_tracking_api_handler),
        )
        .route("/api/diagnose", get(handlers_core::diagnose_api_handler))
        .route(
            "/api/temperature-humidity",
            get(handlers_core::temperature_humidity_api_handler)
                .post(handlers_core::post_temperature_humidity_handler),
        )
        .route("/admin", get(handlers_core::admin_page_handler))
        .route(
            "/admin.html",
            get(|| async { axum::response::Redirect::permanent("/admin") }),
        )
        .route("/analyze", get(handlers_core::analyze_page_handler))
        .route(
            "/analyze.html",
            get(|| async { axum::response::Redirect::permanent("/analyze") }),
        )
        .route("/commerce", get(handlers_core::commerce_page_handler))
        .route(
            "/commerce.html",
            get(|| async { axum::response::Redirect::permanent("/commerce") }),
        )
        .route(
            "/orchard-3d",
            get(handlers_core::orchard_3d_page_handler),
        )
        .route(
            "/orchard-3d.html",
            get(|| async { axum::response::Redirect::permanent("/orchard-3d") }),
        )
        .route("/citrus/analyze", post(handlers_ai::citrus_analyze_handler))
        .route(
            "/api/citrus-disease",
            post(handlers_ai::citrus_disease_handler),
        )
        .route(
            "/api/citrus-disease-v2",
            post(handlers_ai::citrus_disease_advanced_handler),
        )
        .route(
            "/api/recognition-records",
            get(handlers_core::recognition_records_api_handler),
        )
        .route(
            "/api/disease-treatment",
            get(handlers_core::disease_treatment_api_handler),
        )
        .route("/api/tasks", get(handlers_core::get_tasks_api_handler))
        .route("/api/tasks/add", post(handlers_core::add_task_api_handler))
        .route(
            "/api/tasks/complete",
            post(handlers_core::complete_task_api_handler),
        )
        .route(
            "/api/tasks/generate/disease",
            post(handlers_core::generate_task_from_disease_api_handler),
        )
        .route(
            "/api/tasks/generate/environment",
            post(handlers_core::generate_task_from_environment_api_handler),
        )
        .route(
            "/api/health-point",
            get(handlers_core::health_point_handler),
        )
        .route("/api/generate", get(handlers_ai::generate_handler))
        .route(
            "/api/generate/fertilization-plan",
            post(handlers_ai::generate_fertilization_plan_handler),
        )
        .route(
            "/api/commerce/storefront",
            get(handlers_commerce::storefront_handler),
        )
        .route(
            "/api/commerce/batches/{batch_id}/trace",
            get(handlers_commerce::batch_trace_handler),
        )
        .nest("/user", crate::user_routes::router(state.clone()))
        .nest_service(
            "/media/recognition_records",
            ServeDir::new(RECOGNITION_RECORDS_UPLOAD_DIR),
        )
        .fallback_service(ServeDir::new("static"))
        .layer(axum::middleware::from_fn(log_request_path))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state.clone())
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

pub async fn run_server(config: crate::config::AppConfig) -> anyhow::Result<()> {
    let addr: SocketAddr = config
        .server
        .addr
        .parse()
        .map_err(|e| anyhow::anyhow!("解析 server.addr 失败: {}", e))?;
    let db = PgPoolOptions::new()
        .max_connections(3)
        .connect(&config.database.postgres_url)
        .await
        .map_err(|e| anyhow::anyhow!("连接 PostgreSQL 失败: {}", e))?;
    bootstrap::init_database(&db).await?;

    let app = create_router(&config, db);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("无法绑定到地址");

    tracing::info!("AI 服务 服务器正在监听: {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| anyhow::anyhow!("服务器运行失败: {}", e))?;

    Ok(())
}
