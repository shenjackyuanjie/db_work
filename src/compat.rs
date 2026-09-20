//! Django（`navel_backend_git`）契约兼容层。
//!
//! 目标：让本服务成为 Django + DRF 后端的**契约等价替代**——同一请求、同一份数据下，
//! 响应 JSON 除 `timestamp` 外逐字节一致，App 只需改 baseUrl。
//!
//! 阶段说明：P0–P4 期间本层只挂在 `/compat` 影子路径上，用于与 Django 逐条做契约比对；
//! P5 再整体提升到 `/api/**` 与 `/api/v1/**`，同时删除旧自研 handler 与旧表。
//!
//! 模块职责：
//! - [`ser`] 时间 / 日期 / Decimal 的序列化对齐（**冻结接口**，不要就地改）
//! - [`errors`] 成功信封与 DRF 异常体（**冻结接口**）
//! - [`auth`] Bearer 鉴权、角色守卫、密码三格式校验
//! - [`dto`] 请求体（沿用 Django 原始字段名，不做 camelCase 转换）
//! - `views_*` 各域路由，按 `api/urls.py` 逐条对应

mod agent_service;
mod auth;
mod dto;
mod errors;
mod ser;
mod views_agent;
mod views_auth;
mod views_commerce;
mod views_core;
mod views_trace;

#[cfg(test)]
mod tests;

use axum::Router;

use crate::server::AppState;

/// 契约层路由总装配。
///
/// 各子模块只声明自己的路由（返回 `Router<AppState>`），状态由 `server.rs` 的最外层
/// `.with_state(...)` 统一注入一次，这样各域可以独立增删路由而互不冲突。
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .merge(views_auth::router())
        .merge(views_core::router())
        .merge(views_trace::router())
        .merge(views_commerce::router())
        .merge(views_agent::router())
}
