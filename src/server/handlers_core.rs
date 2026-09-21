//! 页面与公共数据 handler。
//!
//! 与 `handlers_ai.rs` 同理：再导出统一 `pub(crate)`，让网页超集层（`src/web/**`，
//! 它是 `server` 的兄弟模块）能复用，例如 `/web/system-status` 要包
//! `system_status_api_handler`。只开外层 `mod` 是不够的。

mod media;
mod pages;

pub(crate) use media::recognition_image_handler;
pub(crate) use pages::{
    admin_page_handler, analyze_page_handler, app_shell_handler, cart_page_handler, health_handler,
    index_page_handler, orchard_3d_page_handler, store_admin_page_handler, store_page_handler,
    system_status_api_handler,
};
