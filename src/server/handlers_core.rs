//! 页面与公共数据 handler。
//!
//! 与 `handlers_ai.rs` 同理：再导出统一 `pub(crate)`，让网页超集层（`src/web/**`，
//! 它是 `server` 的兄弟模块）能复用。只开外层 `mod` 是不够的。
//!
//! `pages::*` 里只剩页面外壳（`/`、`/admin`、`/store`、`/analyze` 等 9 条页面路由）与
//! `media::recognition_image_handler`（`/media/recognition_records/*`、`/uploads/*`
//! 两条**隐式静态通道**）。原先同文件里的 `pages::system_status_api_handler` 已随
//! `/api/system-status` 路由在 G2 删除——`/web/system-status` 由 `web/admin.rs` 自己那份
//! 同形实现承担。

mod media;
mod pages;

pub(crate) use media::recognition_image_handler;
pub(crate) use pages::{
    admin_page_handler, analyze_page_handler, app_shell_handler, cart_page_handler, health_handler,
    index_page_handler, orchard_3d_page_handler, store_admin_page_handler, store_page_handler,
};
