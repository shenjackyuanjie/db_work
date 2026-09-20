//! 商城域契约实现：商品、购物车、地址、订单、支付、售后。
//!
//! 对应 `navel_backend_git/api/urls.py` 中 commerce 域 **24 条 path**（含 `/api/v1/**` 别名），
//! 蓝本为 `api/commerce_views.py`。契约基准见 `tests/fixtures/contract/commerce.json`。
//!
//! TODO(W1-C): 待实现。

use axum::Router;

use crate::server::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
}
