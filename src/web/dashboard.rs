//! 网页后台仪表盘与审计日志超集。
//!
//! 负责的 `/web/*` 路径（外层已 `nest("/web")`）：
//!
//! | 路径 | 方法 | 旧路径 | 前端引用 |
//! |---|---|---|---|
//! | `/admin/dashboard/stats` | POST | `/user/admin/dashboard/stats` | `admin.js:929` |
//! | `/admin/dashboard/logs` | POST | `/user/admin/dashboard/logs` | `admin.js:1011` |
//!
//! 表：保留 `app_admin_audit_logs`。统计口径要读识别富字段
//! （`is_citrus_leaf` / `is_healthy` / `predicted_class` / `severity`），
//! 因此读 `web_diagnosis_records`（见 `bootstrap/web_tables.rs`）而不是契约表；
//! 温度湿度改读契约表 `temperature_humidity_data`（`W2_PLAN.md` §3.1(c)）。
//!
//! TODO(S2): 待实现。

use axum::Router;

use crate::server::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
}
