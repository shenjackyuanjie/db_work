//! 智能体域契约实现。
//!
//! 对应 `navel_backend_git/api/urls.py` 中 agent 域 **11 条 path**，蓝本为 `api/agent_views.py`。
//! 契约基准见 `tests/fixtures/contract/agent.json`。
//!
//! 注意：蓝本 `agent_views.py` 漏 `import status`，导致 6 个接口的校验分支恒返回 500。
//! 已裁定**修正为设计意图的 400/404**，并在偏差清单中登记（不要复刻 500）。
//!
//! TODO(W1-D): 待实现。

use axum::Router;

use crate::server::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
}
