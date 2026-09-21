//! 网页超集层：承载 **Django 从未建模**的运营能力 + 网页自己的 cookie 会话。
//!
//! 与 [`crate::compat`] 的分工（`W2_PLAN.md` §3）：
//!
//! | 层 | 路径 | 性质 | 谁用 |
//! |---|---|---|---|
//! | 契约层 | `/api/**`、`/api/v1/**` | 从 `/compat` 提升，逐字兼容 Django | App + 网页 |
//! | 超集层 | `/web/**`（本层） | Rust 独有，**不保证**逐字兼容 | 仅网页 |
//!
//! **原则**：任何 Django 已有等价契约的功能都走 `/api/**`；**绝不**为了超集去改 compat 的响应。
//! 这也是 `-v2` 识别、系统状态、注册审批、邀请码、客服、3D 沙盘、封面上传、后台仪表盘
//! 被放在 `/web/*` 而不是 `/api/*` 的原因。
//!
//! 各子模块只声明自己的路由（返回 `Router<AppState>`），状态由 `server.rs` 的最外层
//! `.with_state(...)` 统一注入一次。
//!
//! **要加路由就往自己负责的子文件里加，不要改本文件** —— 本文件是并行开发的分界，
//! 六个子模块各自独占，互不冲突。

mod admin;
mod dashboard;
mod orchard;
mod session;
mod store_admin;
mod support;

use axum::Router;

use crate::server::AppState;

/// 超集层路由总装配。挂载点见 `src/server.rs` 的 `.nest("/web", crate::web::router())`。
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .merge(session::router())
        .merge(admin::router())
        .merge(support::router())
        .merge(orchard::router())
        .merge(store_admin::router())
        .merge(dashboard::router())
}
