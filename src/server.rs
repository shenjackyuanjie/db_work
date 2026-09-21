use axum::{Router, body::Body, http::Request, middleware::Next, response::Response, routing::get};

use sqlx::{PgPool, postgres::PgPoolOptions};
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing_subscriber::EnvFilter;

use crate::client::OpenRouterClient;

pub(crate) mod bootstrap;
// 下面三个模块曾各带一个 `#[allow(dead_code)]`，依据是「自研 `/api/*` handler 已在 S1 删掉路由，
// 因此整体变成死代码」。**那个判断是错的**（S5/G1 实测，见 `W2_S5_PLAN.md` §4）：
//   - `handlers_ai::citrus_disease_advanced_handler` 仍被 `web/orchard.rs` 路由
//     （G2 删掉了 `/api/citrus-disease-v2` 那条，超集出口只剩 `/web/citrus-disease-v2`）；
//   - `handlers_core` 的 `media::recognition_image_handler` 与 `pages::*` 都被路由
//     （前者正是 `/media/recognition_records/*`、`/uploads/*` 两条**隐式静态通道**）；
//   - `shared` 的 `AppState` / `api_response` / `now_millis` 等被 `compat/**`、`web/**` 使用。
//
// 属性已撤销；三个模块内的**真**死代码已随子模块一起删除（G1 记录见 `W2_S5_PLAN.md`）。
// 保留属性会让这三个模块里日后的真死代码隐身——不要再加回来。
//
// `handlers_commerce` 与 `handlers_store::storefront_handler` 不在上面这一列：G2 删掉
// `/api/commerce/*` 与 `/api/store/products` 后它们零调用者，前者整文件退役，后者只留
// `/store-images/*` 那条**隐式静态通道**。
pub(crate) mod handlers_ai;
pub(crate) mod handlers_core;
mod handlers_store;
mod shared;

// 再导出列表已按「谁真的在用」裁到最小：原先 24 项里有 14 项只被 G1 删掉的死模块引用，
// `api_response` / `api_success` 两项则只被 G2 删掉的 `handlers_store::storefront_handler` 用。
pub(crate) use shared::{
    AppState, disease_treatment_text, lookup_session_username, now_millis, risk_from_disease_name,
    save_recognition_record_image, save_store_cover_image, username_by_token,
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
        // `/api/system-status`（S2 已迁 `/web/system-status`）与 `/api/citrus-disease-v2`
        // （S2 已迁 `/web/citrus-disease-v2`）在 G2 一并删除：`db/static/**` 对这两条旧路径
        // 只剩注释级引用，活调用方全在 `/web/*` 上。另外 `/api/commerce/*`、`/api/store/products`
        // 与整棵 `/user/*` 树也在 G2 删除，清单与 grep 证据见 `W2_S5_PLAN.md` §1.1、§2。
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
        // `-v2` 是**超集**（响应比 Django 多 6 个字段），S2 已迁到 `/web/citrus-disease-v2`
        // 以免超集污染 `/api/**` 的逐字兼容目标，G2 删掉这条旧路径。
        // `/api/recognition-records`、`/api/disease-treatment`、`/api/tasks*`、`/api/health-point`、
        // `/api/generate*` 的自研实现已删除，由契约层同名路径接管。
        //
        // `/api/commerce/*` 与 `/api/store/products` 曾以「网页还在用」为由暂留；S4 把前端全切到
        // 契约表（`/api/products`、`/api/orders`）后它们在 `db/static/**` 只剩注释级引用，G2 删除。
        // `/api/commerce/batches/{batch_id}/trace` 的键语义本来就与契约 `/api/traces/{trace_code}`
        // 不同（batch_id ≠ trace_code），不是替代关系，删掉不丢契约能力。
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
        // `/user/*` 那棵遗留树（会话 / 商城 / 后台 / 团购 / 3D）已在 S5/G2 整棵删除：
        // S2 提供 `/web/session/*` 等替代面、S4 切完前端调用方之后，它的外部引用面只剩这一行。
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
