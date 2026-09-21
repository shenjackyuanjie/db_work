//! 网页客服会话超集（网页独有，Django 无对应模型）。
//!
//! 负责的 `/web/*` 路径（外层已 `nest("/web")`）：
//!
//! | 路径 | 方法 | 旧路径 | 前端引用 |
//! |---|---|---|---|
//! | `/support` | GET + POST | `/user/store/support` | `store-support.js:19`、经它被 `cart.js` 引用 |
//! | `/admin/support` | GET + POST | `/user/admin/store/support` | `store-admin.js:188,203,375`、`admin.js` |
//!
//! 表：保留自研的 `store_support_messages`（Django 从未建模）。
//!
//! TODO(S2): 待实现。

use axum::Router;

use crate::server::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
}
