//! 果园、批次与溯源域契约实现。
//!
//! 对应 `navel_backend_git/api/urls.py` 中 orchard_trace 域 **21 条 path**，蓝本为
//! `api/commerce_views.py`。契约基准见 `tests/fixtures/contract/orchard_trace.json`。
//!
//! 本域核心难点是 `trace_event` 的哈希链：`evidence_hash` 必须与 Django 逐字节相等。
//!
//! TODO(W1-B): 待实现。

use axum::Router;

use crate::server::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
}
