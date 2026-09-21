//! 网页会话超集：cookie 会话（`session_token`）而非 Django 的 Bearer。
//!
//! 负责的 `/web/*` 路径（外层已 `nest("/web")`，此处写相对路径）：
//!
//! | 路径 | 方法 | 旧路径 | 前端引用 |
//! |---|---|---|---|
//! | `/session/login` | POST | `/user/login` | `index.js:225` |
//! | `/session/register` | POST | `/user/register` | `index.js:260`（带审批流） |
//! | `/session/logout` | POST | `/user/logout` | 9 个文件 |
//! | `/session/validate` | POST | `/user/validate` | 9 个文件（返回 `valid` / `is_admin`） |
//! | `/session/me` | POST | `/user/me` | 无（与 validate 同族，一并迁移以免留孤儿） |
//!
//! 与契约层的关系（`W2_PLAN.md` §3）：登录/登出可包一层 `compat::views_auth` 复用其校验，
//! 但**新增职责**是下发 / 清除 HttpOnly cookie `session_token`。数据面从
//! `app_users` / `app_sessions` 改成读 `"user"` / `auth_token`。
//!
//! 为什么必须留在 `/web/*` 而不是 `/api/*`：Django 的 `/api/me` 没有 `is_admin` 字段
//! （`is_admin` 是 `"user"` 的加法列，见 `bootstrap.rs`），两者契约不同，不能混。
//!
//! TODO(S2): 待实现。

use axum::Router;

use crate::server::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
}
