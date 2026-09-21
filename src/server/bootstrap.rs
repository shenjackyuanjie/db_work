mod agent_tables;
mod commerce_tables;
mod core_tables;
mod legacy_tables;
mod trace_tables;
mod web_tables;

use sqlx::{PgPool, Row};

use super::shared::now_millis;

/// 建库自举。契约层的集成测试会直接调用它，把 30 张契约表建到独立的 scratch schema 上。
pub(crate) async fn init_database(pool: &PgPool, seed_demo_data: bool) -> anyhow::Result<()> {
    for &stmt in legacy_tables::DDL {
        sqlx::query(stmt)
            .execute(pool)
            .await
            .map_err(|e| anyhow::anyhow!("初始化数据库表失败: {}", e))?;
    }

    // 契约表按外键依赖顺序执行，跨分组的交错顺序不能随意调整：
    // 1. core_tables      —— 建 `user` 及只依赖 `user` 的农事/识别表
    // 2. trace_tables     —— orchard -> fruit_tree_archive / sales_batch -> harvest_archive / trace_event
    // 3. commerce PRODUCT —— citrus_product 依赖 sales_batch，且被品质抽检表引用
    // 4. trace QUALITY    —— batch_quality_sample 依赖 citrus_product
    // 5. commerce ORDER   —— buyer_address / cart_item / "order" / order_item / payment_record /
    //                        trace_package / after_sale_request
    // 6. agent_tables     —— agent_approval / agent_feedback
    apply_contract_ddl(pool, core_tables::DDL, "core").await?;
    apply_contract_ddl(pool, trace_tables::DDL, "trace").await?;
    apply_contract_ddl(pool, commerce_tables::PRODUCT_DDL, "commerce.product").await?;
    apply_contract_ddl(pool, trace_tables::QUALITY_DDL, "trace.quality").await?;
    apply_contract_ddl(pool, commerce_tables::DDL, "commerce").await?;
    apply_contract_ddl(pool, agent_tables::DDL, "agent").await?;

    // `"user"` 的**加法列** `is_admin`：网页后台鉴权要用，App 的 `role` 契约里没有管理员概念
    // （`W2_PLAN.md` §3.1(a)）。
    //
    // 三条硬约束，改的时候别踩：
    // 1. 用 `ALTER ... ADD COLUMN IF NOT EXISTS`，保证可重复执行（bootstrap 每次启动都会跑）；
    // 2. **不要**把它挪进 `core_tables.rs` 的 `CREATE TABLE` —— 那张表要与 Django `db_table`
    //    逐字对齐，加了列就没法用 Django 的 `dumpdata` 直接把种子灌进来；
    // 3. 它不进任何 DRF 响应（`AuthUser::payload()` 不做改动），也**不要**引入第三个 `role` 值
    //    （会污染 App 依赖的 `role` 契约）。管理员用
    //    `UPDATE "user" SET is_admin = TRUE WHERE username = '<管理员账号>'` 标记。
    sqlx::query(
        r#"ALTER TABLE "user" ADD COLUMN IF NOT EXISTS is_admin BOOLEAN NOT NULL DEFAULT FALSE"#,
    )
    .execute(pool)
    .await
    .map_err(|e| anyhow::anyhow!("添加 user.is_admin 加法列失败: {}", e))?;

    // 网页超集专用表（不参与契约，故排在契约表之后）。
    apply_contract_ddl(pool, web_tables::DDL, "web").await?;

    crate::system_settings::ensure_default_settings(pool).await?;
    if seed_demo_data {
        ensure_orchard_demo_data(pool).await?;
    }

    Ok(())
}

async fn apply_contract_ddl(pool: &PgPool, statements: &[&str], group: &str) -> anyhow::Result<()> {
    for &stmt in statements {
        sqlx::query(stmt)
            .execute(pool)
            .await
            .map_err(|e| anyhow::anyhow!("初始化契约表失败({}): {}", group, e))?;
    }

    Ok(())
}

async fn ensure_orchard_demo_data(pool: &PgPool) -> anyhow::Result<()> {
    let tree_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM app_orchard_trees")
        .fetch_one(pool)
        .await
        .map_err(|e| anyhow::anyhow!("查询果树表失败: {}", e))?;

    if tree_count == 0 {
        let now = now_millis() as i64;
        let demo_trees = [
            ("NAVEL-001", 36.0, 58.0, 13.8),
            ("NAVEL-002", 74.0, 126.0, 13.5),
            ("NAVEL-003", 57.0, 214.0, 13.2),
            ("NAVEL-004", 98.0, 304.0, 12.9),
            ("NAVEL-005", 82.0, 404.0, 12.5),
            ("NAVEL-006", 144.0, 82.0, 13.0),
            ("NAVEL-007", 178.0, 168.0, 12.6),
            ("NAVEL-008", 161.0, 262.0, 12.1),
            ("NAVEL-009", 204.0, 356.0, 11.8),
            ("NAVEL-010", 188.0, 438.0, 11.4),
            ("NAVEL-011", 246.0, 118.0, 11.9),
            ("NAVEL-012", 274.0, 208.0, 11.5),
            ("NAVEL-013", 252.0, 296.0, 11.1),
            ("NAVEL-014", 289.0, 388.0, 10.8),
            ("NAVEL-015", 332.0, 72.0, 11.2),
            ("NAVEL-016", 368.0, 156.0, 10.9),
            ("NAVEL-017", 346.0, 248.0, 10.6),
            ("NAVEL-018", 391.0, 332.0, 10.2),
            ("NAVEL-019", 424.0, 118.0, 10.0),
            ("NAVEL-020", 452.0, 222.0, 9.7),
        ];

        for (index, (tree_code, pos_x, pos_y, terrain_height)) in demo_trees.iter().enumerate() {
            let created_at = now.saturating_sub(((demo_trees.len() - index) as i64) * 60_000);
            let tag_serial_number = 10_000_001_i64 + index as i64;
            sqlx::query(
                "INSERT INTO app_orchard_trees (tree_code, tag_serial_number, pos_x, pos_y, terrain_height, is_active, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, TRUE, $6, $7)",
            )
            .bind(*tree_code)
            .bind(tag_serial_number)
            .bind(*pos_x)
            .bind(*pos_y)
            .bind(*terrain_height)
            .bind(created_at)
            .bind(created_at)
            .execute(pool)
            .await
            .map_err(|e| anyhow::anyhow!("写入示例果树数据失败: {}", e))?;
        }
    }

    let sensor_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM app_tree_sensor_records")
        .fetch_one(pool)
        .await
        .map_err(|e| anyhow::anyhow!("查询果树传感器表失败: {}", e))?;

    if sensor_count == 0 {
        let now = now_millis() as i64;
        let tree_rows =
            sqlx::query("SELECT id, tag_serial_number FROM app_orchard_trees ORDER BY id ASC")
                .fetch_all(pool)
                .await
                .map_err(|e| anyhow::anyhow!("读取果树主数据失败: {}", e))?;
        let base_health = [
            0.98, 0.95, 0.93, 0.89, 0.86, 0.82, 0.79, 0.74, 0.69, 0.64, 0.58, 0.48,
        ];

        for (index, row) in tree_rows.iter().enumerate() {
            let tag_serial_number = row
                .try_get::<Option<i64>, _>("tag_serial_number")
                .ok()
                .flatten()
                .unwrap_or_else(|| row.try_get::<i64, _>("id").unwrap_or_default());
            let health_anchor = *base_health.get(index).unwrap_or(&0.82);

            for sample_index in 0..3 {
                let sampled_at = now.saturating_sub(((2 - sample_index) as i64) * 30 * 60 * 1000);
                let wave = sample_index as f64 - 1.0;
                let temperature = 23.6 + (index % 5) as f64 * 0.7 + wave * 0.35;
                let humidity = 58.0 + (index % 4) as f64 * 4.5 - wave * 1.6;
                let _ = health_anchor;

                sqlx::query(
                    "INSERT INTO app_tree_sensor_records (tag_serial_number, sampled_at, temperature, humidity, source) VALUES ($1, $2, $3, $4, 'seed')",
                )
                .bind(tag_serial_number)
                .bind(sampled_at)
                .bind(temperature)
                .bind(humidity)
                .execute(pool)
                .await
                .map_err(|e| anyhow::anyhow!("写入示例传感器数据失败: {}", e))?;
            }
        }
    }

    Ok(())
}
