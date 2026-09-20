//! 农事与识别域契约实现。
//!
//! 对应 `navel_backend_git/api/urls.py` 中 core 域 **14 条 path**，蓝本为 `api/views.py`
//! 与 `api/serializers.py`。契约基准见 `tests/fixtures/contract/core.json`。
//!
//! TODO(W1-A): 待实现。

use axum::Router;

use crate::server::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
}
