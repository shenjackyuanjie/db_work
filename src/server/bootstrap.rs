use sqlx::{PgPool, Row};

use super::shared::now_millis;

pub(super) async fn init_database(pool: &PgPool) -> anyhow::Result<()> {
    let ddl = [
        r#"CREATE TABLE IF NOT EXISTS app_users (
            username TEXT PRIMARY KEY,
            password_hash TEXT NOT NULL,
            is_admin BOOLEAN NOT NULL DEFAULT FALSE,
            created_at BIGINT NOT NULL,
            session_token TEXT NULL,
            latitude DOUBLE PRECISION NULL,
            longitude DOUBLE PRECISION NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS app_sessions (
            token TEXT PRIMARY KEY,
            username TEXT NOT NULL,
            created_at BIGINT NOT NULL
        )"#,
        r#"CREATE INDEX IF NOT EXISTS idx_app_sessions_username ON app_sessions(username)"#,
        r#"CREATE TABLE IF NOT EXISTS app_invitations (
            code TEXT PRIMARY KEY,
            used BOOLEAN NOT NULL DEFAULT FALSE,
            expires_at BIGINT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS app_pending_users (
            username TEXT PRIMARY KEY,
            password_hash TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            requested_role TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS app_tasks (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL,
            title TEXT NOT NULL,
            description TEXT NOT NULL,
            risk_level TEXT NOT NULL,
            task_type TEXT NOT NULL,
            source TEXT NOT NULL,
            is_completed BOOLEAN NOT NULL DEFAULT FALSE,
            created_at BIGINT NOT NULL,
            completed_at BIGINT NULL
        )"#,
        r#"CREATE INDEX IF NOT EXISTS idx_app_tasks_username_created ON app_tasks(username, created_at DESC)"#,
        r#"CREATE TABLE IF NOT EXISTS app_temperature_humidity (
            id BIGSERIAL PRIMARY KEY,
            username TEXT NULL,
            timestamp BIGINT NOT NULL,
            temperature DOUBLE PRECISION NOT NULL,
            humidity DOUBLE PRECISION NOT NULL
        )"#,
        r#"CREATE INDEX IF NOT EXISTS idx_app_temp_humidity_user_time ON app_temperature_humidity(username, timestamp DESC)"#,
        r#"CREATE TABLE IF NOT EXISTS app_diagnosis_records (
            id TEXT PRIMARY KEY,
            timestamp BIGINT NOT NULL,
            predicted_class TEXT NOT NULL,
            confidence DOUBLE PRECISION NOT NULL,
            is_citrus_leaf BOOLEAN NOT NULL,
            citrus_type TEXT NOT NULL,
            is_healthy BOOLEAN NOT NULL,
            disease_name TEXT NOT NULL,
            severity TEXT NOT NULL,
            treatment_suggestion TEXT NOT NULL,
            preventive_measures TEXT NOT NULL,
            image_quality_warning TEXT NOT NULL,
            username TEXT NULL,
            area TEXT NULL,
            image_path TEXT,
            temp DOUBLE PRECISION NULL,
            humm DOUBLE PRECISION NULL
        )"#,
        r#"CREATE INDEX IF NOT EXISTS idx_app_diag_user_time ON app_diagnosis_records(username, timestamp DESC)"#,
        r#"CREATE TABLE IF NOT EXISTS app_system_settings (
            id SMALLINT PRIMARY KEY,
            open_registration BOOLEAN NOT NULL DEFAULT TRUE,
            invite_bypass_enabled BOOLEAN NOT NULL DEFAULT TRUE,
            maintenance_mode BOOLEAN NOT NULL DEFAULT FALSE,
            default_invite_ttl_seconds BIGINT NOT NULL DEFAULT 86400,
            confidence_threshold DOUBLE PRECISION NOT NULL DEFAULT 0.75,
            log_retention_days INTEGER NOT NULL DEFAULT 30,
            updated_at BIGINT NOT NULL,
            updated_by TEXT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS app_admin_audit_logs (
            id TEXT PRIMARY KEY,
            log_type TEXT NOT NULL,
            actor_username TEXT NULL,
            message TEXT NOT NULL,
            created_at BIGINT NOT NULL
        )"#,
        r#"CREATE INDEX IF NOT EXISTS idx_app_admin_audit_logs_created ON app_admin_audit_logs(created_at DESC)"#,
        r#"CREATE TABLE IF NOT EXISTS app_orchard_trees (
            id BIGSERIAL PRIMARY KEY,
            tree_code TEXT NOT NULL UNIQUE,
            tag_serial_number BIGINT NULL UNIQUE,
            pos_x DOUBLE PRECISION NOT NULL CHECK (pos_x >= 0 AND pos_x <= 500),
            pos_y DOUBLE PRECISION NOT NULL CHECK (pos_y >= 0 AND pos_y <= 500),
            terrain_height DOUBLE PRECISION NOT NULL DEFAULT 0,
            is_active BOOLEAN NOT NULL DEFAULT TRUE,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL
        )"#,
        r#"CREATE INDEX IF NOT EXISTS idx_app_orchard_trees_active ON app_orchard_trees(is_active)"#,
        r#"CREATE TABLE IF NOT EXISTS app_tree_sensor_records (
            id BIGSERIAL PRIMARY KEY,
            tag_serial_number BIGINT NOT NULL,
            sampled_at BIGINT NOT NULL,
            temperature DOUBLE PRECISION NOT NULL,
            humidity DOUBLE PRECISION NOT NULL,
            source TEXT NOT NULL DEFAULT 'sensor'
        )"#,
        r#"CREATE INDEX IF NOT EXISTS idx_app_tree_sensor_records_tag_sampled ON app_tree_sensor_records(tag_serial_number, sampled_at DESC)"#,
    ];

    for stmt in ddl {
        sqlx::query(stmt)
            .execute(pool)
            .await
            .map_err(|e| anyhow::anyhow!("初始化数据库表失败: {}", e))?;
    }

    crate::system_settings::ensure_default_settings(pool).await?;
    ensure_orchard_demo_data(pool).await?;

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
