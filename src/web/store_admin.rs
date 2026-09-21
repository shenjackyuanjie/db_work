//! 网页商城运营超集：封面 multipart 上传 + 后台商城概览 / 订单管理 / 分析。
//!
//! 负责的 `/web/*` 路径（外层已 `nest("/web")`）：
//!
//! | 路径 | 方法 | 旧路径 | 前端引用 |
//! |---|---|---|---|
//! | `/admin/store/products/{id}/cover` | POST | `/user/admin/store/products/{id}/cover` | `admin.js:1650`、`store-admin.js:327` |
//! | `/admin/store/analytics` | GET | `/user/admin/store/analytics` | `store-admin.js:79` |
//! | `/admin/store/overview` | POST | `/user/admin/store/overview` | `admin.js:1505`、`store-admin.js` |
//! | `/admin/store/orders` | POST | `/user/admin/store/orders` | `admin.js:1726`、`store-admin.js:179` |
//! | `/admin/store/orders/status` | POST | `/user/admin/store/orders/status` | `admin.js:1733`、`store-admin.js:348` |
//!
//! 表（`W2_PLAN.md` §3、§4）：**改读写 Django 契约表** `"order"` / `order_item` /
//! `citrus_product`，替代自研的 `store_orders` / `store_order_items` / `store_products`。
//! 封面上传写入 `citrus_product.cover_image_url`，值仍是 `/store-images/<uuid>.jpg`，
//! 因为前端只认 `/store-images/`、`/uploads/`、`http(s)://` 三种前缀
//! （`store.js:61`，见 `W2_PLAN.md` §2.4 的隐式静态通道）。
//!
//! 为什么这些必须在 `/web/*`：Django 没有「管理端订单状态流转」这一契约，
//! 塞进 `/api/**` 会污染逐字兼容目标。
//!
//! TODO(S2): 待实现。

use axum::Router;

use crate::server::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
}
