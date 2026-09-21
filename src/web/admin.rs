//! 网页后台管理超集：系统状态 / 系统设置 / 账号 / 邀请码 / 注册审批。
//!
//! 负责的 `/web/*` 路径（外层已 `nest("/web")`）：
//!
//! | 路径 | 方法 | 旧路径 | 前端引用 |
//! |---|---|---|---|
//! | `/system-status` | GET | `/api/system-status` | `index.js:143`（**登录页首屏**，不能留在 `/api/**`） |
//! | `/admin/settings/get` | POST | `/user/admin/settings/get` | `admin.js:1063` |
//! | `/admin/settings/update` | POST | `/user/admin/settings/update` | `admin.js:1079` |
//! | `/admin/set_admin` | POST | `/user/admin/set_admin` | `admin.js:165` |
//! | `/admin/users/list` | POST | `/user/admin/users/list` | `admin.js:277` |
//! | `/admin/invitations/create` | POST | `/user/admin/invitations/create` | `admin.js:146` |
//! | `/admin/invitations/list` | POST | `/user/admin/invitations/list` | `admin.js:266` |
//! | `/admin/pending/list` | POST | `/user/admin/pending/list` | `admin.js:255` |
//! | `/admin/pending/approve` | POST | `/user/admin/pending/approve` | `admin.js:288` |
//! | `/admin/pending/reject` | POST | `/user/admin/pending/reject` | `admin.js:300` |
//!
//! 表：保留 `app_system_settings`、`app_invitations`、`app_pending_users`；
//! 账号改为读写契约表 `"user"`，管理员标记用**加法列** `"user".is_admin`
//! （见 `src/server/bootstrap.rs`，不进 DRF 序列化）。
//!
//! TODO(S2): 待实现。

use axum::Router;

use crate::server::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
}
