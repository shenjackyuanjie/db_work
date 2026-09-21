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

pub(crate) mod bootstrap;
// 下面三个模块里的自研 `/api/*` handler 已在 S1 删掉路由（由契约层同名路径接管），
// 因此整体变成死代码。用模块级 `allow(dead_code)` 压住告警，避免 60+ 条噪音淹没
// 后续实施流的真实告警。
//
// **S5 删除这些代码时必须把下面三个 `#[allow(dead_code)]` 一起删掉**——
// 让它们留在此处会让这三个模块里日后的真死代码隐身。
#[allow(dead_code)]
mod handlers_ai;
mod handlers_commerce;
#[allow(dead_code)]
mod handlers_core;
mod handlers_store;
#[allow(dead_code)]
mod shared;

pub(crate) use shared::{
    AddTaskRequest, AppState, CompleteTaskRequest, DiseaseTreatmentQuery,
    GenerateDiseaseTaskRequest, GenerateEnvironmentTaskRequest, TagTemperatureHumidityRequest,
    TaskRecord, TemperatureHumiditySample, UsernameQuery, api_response, api_success,
    build_temp_humidity_payload, classify_environment_risk, default_temperature_samples,
    disease_treatment_text, normalize_recognition_record_image_path, now_millis,
    risk_from_disease_name, save_recognition_record_image, save_store_cover_image, task_payload,
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

pub fn create_router(config: &crate::config::AppConfig, db: PgPool) -> anyhow::Result<Router> {
    let api_key = config.ai.openrouter_api_key.clone();

    let client = OpenRouterClient::new(api_key);
    let inference = crate::inference::InferenceRuntime::new(&config.inference, client.clone())?;
    let state = AppState {
        client,
        inference,
        db,
        secure_session_cookie: config.server.secure_session_cookie,
    };

    Ok(Router::new()
        .route("/health", get(handlers_core::health_handler))
        .route("/", get(handlers_core::app_shell_handler))
        .route("/app-content/home", get(handlers_core::index_page_handler))
        .route("/app-content/store", get(handlers_core::store_page_handler))
        .route("/app-content/cart", get(handlers_core::cart_page_handler))
        .route("/app-content/admin", get(handlers_core::admin_page_handler))
        .route(
            "/app-content/analyze",
            get(handlers_core::analyze_page_handler),
        )
        .route(
            "/app-content/orchard-3d",
            get(handlers_core::orchard_3d_page_handler),
        )
        .route(
            "/app-content/store-admin",
            get(handlers_core::store_admin_page_handler),
        )
        .route(
            "/index.html",
            get(|| async { axum::response::Redirect::permanent("/") }),
        )
        // 会话类自研 `/api/*` 已删除：前端对它们**零引用**（`db/static/**` 的 9 个 js 与 8 个 html
        // 全无命中），且已被契约层同名路径取代。`/api/user` 的角色由契约层 `GET /api/me` 承担。
        // `/api/system-status` 保留到 S2 —— 它挂在网页登录页首屏，S2 会迁到 `/web/system-status`。
        .route(
            "/api/system-status",
            get(handlers_core::system_status_api_handler),
        )
        // `/api/home`、`/api/growth-tracking`、`/api/diagnose`、`/api/temperature-humidity`
        // 的自研实现已删除，由契约层同名路径接管。
        .route("/admin", get(handlers_core::app_shell_handler))
        .route("/store-admin", get(handlers_core::app_shell_handler))
        .route("/store-admin.html", get(handlers_core::app_shell_handler))
        .route(
            "/admin.html",
            get(|| async { axum::response::Redirect::permanent("/admin") }),
        )
        .route("/analyze", get(handlers_core::app_shell_handler))
        .route(
            "/analyze.html",
            get(|| async { axum::response::Redirect::permanent("/analyze") }),
        )
        // 已保留的历史入口：开团页面已合并到现货商城。
        .route(
            "/commerce",
            get(|| async { axum::response::Redirect::permanent("/store") }),
        )
        .route(
            "/commerce.html",
            get(|| async { axum::response::Redirect::permanent("/store") }),
        )
        .route("/store", get(handlers_core::app_shell_handler))
        .route(
            "/store.html",
            get(|| async { axum::response::Redirect::permanent("/store") }),
        )
        .route("/cart", get(handlers_core::app_shell_handler))
        .route(
            "/cart.html",
            get(|| async { axum::response::Redirect::permanent("/cart") }),
        )
        .route("/orchard-3d", get(handlers_core::app_shell_handler))
        .route(
            "/orchard-3d.html",
            get(|| async { axum::response::Redirect::permanent("/orchard-3d") }),
        )
        // 自研 `/citrus/analyze` 与 `/api/citrus-disease` 已删除，由契约层接管。
        // `-v2` 是**超集**（响应比 Django 多 6 个字段），暂留原路径，
        // S2 迁到 `/web/citrus-disease-v2` 以免超集污染 `/api/**` 的逐字兼容目标。
        .route(
            "/api/citrus-disease-v2",
            post(handlers_ai::citrus_disease_advanced_handler),
        )
        // `/api/recognition-records`、`/api/disease-treatment`、`/api/tasks*`、`/api/health-point`、
        // `/api/generate*` 的自研实现已删除，由契约层同名路径接管。
        //
        // `/api/commerce/*` 与 `/api/store/products` **暂留**：网页（含死代码 `commerce.js`）仍在用，
        // 等 S4 前端切完、S5 统一退役。这些路径与契约层不冲突。
        .route(
            "/api/commerce/storefront",
            get(handlers_commerce::storefront_handler),
        )
        .route(
            "/api/commerce/batches/{batch_id}/trace",
            get(handlers_commerce::batch_trace_handler),
        )
        .route(
            "/api/store/products",
            get(handlers_store::storefront_handler),
        )
        .route(
            "/store-images/{file_name}",
            get(handlers_store::store_cover_image_handler),
        )
        // ---- 契约层（Django 逐字兼容）----
        // 双挂载：根上 `merge` 让 Django 的 `/api/**` 与 `/api/v1/**` 成为**正式路径**（生产用）；
        // 同时保留 `/compat` 前缀，因为整套 L2 验证网
        // （`scripts/replay_diff.py --base-url .../compat`）依赖它，丢了等于把回归网拆了。
        // 两者路径不同，不会重复注册（axum 对同路径重复注册会直接 panic）。
        .merge(crate::compat::router())
        .nest("/compat", crate::compat::router())
        // ---- 超集层：Django 从未建模的运营能力 + 网页 cookie 会话（W2_PLAN §3）----
        .nest("/web", crate::web::router())
        // ---- 网页会话遗留路径：仍在用。等 S2 提供 `/web/session/*` 且 S4 切完前端后再删 ----
        .nest("/user", crate::user_routes::router(state.clone()))
        .route(
            "/media/recognition_records/{file_name}",
            get(handlers_core::recognition_image_handler),
        )
        .route(
            "/uploads/{file_name}",
            get(handlers_core::recognition_image_handler),
        )
        .fallback_service(ServeDir::new("static"))
        .layer(axum::middleware::from_fn(log_request_path))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state.clone()))
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
        .max_connections(config.server.database_max_connections)
        .connect(&config.database.postgres_url)
        .await
        .map_err(|e| anyhow::anyhow!("连接 PostgreSQL 失败: {}", e))?;
    bootstrap::init_database(&db, config.bootstrap.seed_demo_data).await?;

    let app = create_router(&config, db)?;

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
