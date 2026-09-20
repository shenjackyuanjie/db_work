//! 账号域契约实现：register / login / logout / me 及其 `/api/v1/auth/*` 与 `/api/user` 别名。
//!
//! 蓝本为 `navel_backend_git/api/auth_views.py`、`api/serializers.py`、`api/authentication.py`。
//! 契约基准见 `tests/fixtures/contract/auth.json`（27 条用例）。
//!
//! TODO(W0-b): 待实现。

use axum::Router;

use crate::server::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
}
