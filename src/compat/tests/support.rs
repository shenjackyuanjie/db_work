//! 契约测试共享辅助：连接串、连接池、（幂等）自举建表。
//!
//! 铁律：只允许操作 `compat_*` scratch schema。现网 `public` 下的
//! `app_*` / `store_*` / `commerce_*` 有真实数据（6 用户 / 143 任务 / 20 棵树），
//! 任何测试都不得触碰。
//!
//! 用法：
//! ```ignore
//! #[tokio::test]
//! async fn login_ok() {
//!     let pool = super::support::pool().await;
//!     // ... 用 pool 查期望值，再打 /compat/api/login 比对
//! }
//! ```
//!
//! 换个 scratch schema（并行开发时避免互相踩）：
//! ```text
//! $env:COMPAT_TEST_SCHEMA='compat_w1a'
//! ```

use sqlx::{PgPool, postgres::PgPoolOptions};

/// 默认 scratch schema。其余 schema 用 `COMPAT_TEST_SCHEMA` 覆盖。
const DEFAULT_SCHEMA: &str = "compat_test";

/// 共享 PG 实例上给测试的池上限：别占兄弟项目的连接。
const TEST_MAX_CONNECTIONS: u32 = 2;

fn schema() -> String {
    std::env::var("COMPAT_TEST_SCHEMA").unwrap_or_else(|_| DEFAULT_SCHEMA.to_string())
}

/// 连接串优先级：`COMPAT_TEST_DATABASE_URL` > `config.toml` + `search_path=<schema>`。
///
/// 走 `search_path` 而不是新建库：`bootstrap` 的全部 DDL/DML 都是非限定表名，
/// 所以换 search_path 就完成了隔离，零代码改动。
pub(crate) fn database_url() -> String {
    if let Ok(url) = std::env::var("COMPAT_TEST_DATABASE_URL") {
        return url;
    }

    let configured = crate::config::AppConfig::load("config.toml")
        .expect("读取 config.toml 失败：契约测试需要在 db/ 目录下运行")
        .database
        .postgres_url;

    let schema = schema();
    assert!(
        schema.starts_with("compat_"),
        "COMPAT_TEST_SCHEMA 必须是 compat_* scratch schema，实际是 {schema}"
    );

    let separator = if configured.contains('?') { '&' } else { '?' };
    format!("{configured}{separator}options=-csearch_path%3D{schema}")
}

/// 建池 + 自举建表。`init_database` 全是 `CREATE TABLE IF NOT EXISTS`，可重复调用。
pub(crate) async fn pool() -> PgPool {
    let url = database_url();
    let pool = PgPoolOptions::new()
        .max_connections(TEST_MAX_CONNECTIONS)
        .connect(&url)
        .await
        .expect("连接 scratch schema 失败（检查 config.toml / COMPAT_TEST_DATABASE_URL）");

    let schema_ready: bool = sqlx::query_scalar("SELECT current_schema() IS NOT NULL")
        .fetch_one(&pool)
        .await
        .expect("查询 current_schema() 失败");
    assert!(
        schema_ready,
        "scratch schema 不存在。先跑：\n  db\\scripts\\pg_env.ps1 -Reset {}\n  db\\scripts\\pg_env.ps1 -Apply {}",
        schema(),
        schema()
    );

    crate::server::bootstrap::init_database(&pool, false)
        .await
        .expect("自举建表失败");
    pool
}
