//! AI 推理 handler。
//!
//! **可见性有两层，别只开一层**：`server.rs` 里的 `pub(crate) mod handlers_ai;` 只管到
//! 模块这一层；下面这些再导出如果是 `pub(super)`，就仍然只在 `server` 内可见，
//! 而网页超集层（`src/web/**`）是 `server` 的兄弟模块 → 会撞 `E0603`。
//! 同类问题已在 `compat::auth` / `handlers_ai` / `handlers_ai::advanced` 上各踩过一次，
//! 所以这里统一开到 `pub(crate)`，避免第四次。

mod advanced;
mod persistence;
mod request;
mod review;

/// 唯一被路由引用的 AI handler：`server.rs:141`（`/api/citrus-disease-v2`）
/// 与 `web/orchard.rs:58`（`/web/citrus-disease-v2`）两处都指向它。
pub(crate) use advanced::citrus_disease_advanced_handler;
