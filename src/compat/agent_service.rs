//! 智能体域规则引擎。
//!
//! 蓝本为 `navel_backend_git/api/agent_service.py`。该模块**以规则与 SQL 为主**，
//! LLM 只用于意图识别与摘要，且必须能在 `AGENT_LLM_API_KEY` 为空时走规则兜底
//! （契约基准即在该模式下录制）。
//!
//! TODO(W1-D): 待实现。
