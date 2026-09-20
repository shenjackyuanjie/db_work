//! 契约层测试入口。
//!
//! 每个域的实现者只填自己那份 `*_tests.rs`；比对用的 golden fixtures 在
//! `tests/fixtures/contract/`，回放语义见该目录的 `REPORT.md`（加载 seed → 按 seq 升序回放）。

mod agent_tests;
mod commerce_tests;
mod core_tests;
mod trace_tests;
