// 历史表分组：本服务自有的会话/任务/农情/果园沙盘表，以及上一代商城（commerce_*）与
// 现货商城（store_*）表。这些表名与 Django 契约表无关，保留仅为兼容现有接口，
// 待契约层切换完成后统一退役。

pub(super) const DDL: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS store_support_messages (
            id BIGSERIAL PRIMARY KEY,
            username TEXT NOT NULL,
            is_staff BOOLEAN NOT NULL DEFAULT FALSE,
            actor TEXT NOT NULL,
            content TEXT NOT NULL CHECK (char_length(content) BETWEEN 1 AND 2000),
            created_at BIGINT NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_store_support_user_id ON store_support_messages(username, id DESC)"#,
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
            created_at BIGINT NOT NULL,
            expires_at BIGINT NOT NULL
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
    // Legacy batch-commerce tables remain part of the public API contract.
    r#"CREATE TABLE IF NOT EXISTS commerce_orchards (
            id BIGSERIAL PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            location TEXT NOT NULL DEFAULT '',
            farmer_name TEXT NOT NULL DEFAULT '',
            cover_image TEXT NULL,
            is_active BOOLEAN NOT NULL DEFAULT TRUE,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL
        )"#,
    r#"CREATE TABLE IF NOT EXISTS commerce_products (
            id BIGSERIAL PRIMARY KEY,
            name TEXT NOT NULL,
            sku TEXT NOT NULL UNIQUE,
            unit_label TEXT NOT NULL,
            price_cents BIGINT NOT NULL CHECK (price_cents > 0),
            deposit_cents BIGINT NOT NULL DEFAULT 0 CHECK (deposit_cents >= 0),
            is_active BOOLEAN NOT NULL DEFAULT TRUE,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL
        )"#,
    r#"CREATE TABLE IF NOT EXISTS commerce_batches (
            id TEXT PRIMARY KEY,
            batch_code TEXT NOT NULL UNIQUE,
            title TEXT NOT NULL,
            orchard_id BIGINT NOT NULL REFERENCES commerce_orchards(id),
            status TEXT NOT NULL DEFAULT 'draft',
            open_at BIGINT NULL,
            close_at BIGINT NULL,
            harvest_start_at BIGINT NULL,
            harvest_end_at BIGINT NULL,
            ship_at BIGINT NULL,
            planned_quantity INTEGER NOT NULL CHECK (planned_quantity > 0),
            is_active BOOLEAN NOT NULL DEFAULT TRUE,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_commerce_batches_status ON commerce_batches(status, is_active, close_at)"#,
    r#"CREATE TABLE IF NOT EXISTS commerce_batch_products (
            batch_id TEXT NOT NULL REFERENCES commerce_batches(id) ON DELETE CASCADE,
            product_id BIGINT NOT NULL REFERENCES commerce_products(id),
            quota INTEGER NOT NULL CHECK (quota > 0),
            sold_quantity INTEGER NOT NULL DEFAULT 0 CHECK (sold_quantity >= 0),
            PRIMARY KEY (batch_id, product_id)
        )"#,
    r#"CREATE TABLE IF NOT EXISTS commerce_orders (
            id TEXT PRIMARY KEY,
            order_no TEXT NOT NULL UNIQUE,
            username TEXT NOT NULL REFERENCES app_users(username),
            batch_id TEXT NOT NULL REFERENCES commerce_batches(id),
            recipient_name TEXT NOT NULL,
            recipient_phone TEXT NOT NULL,
            shipping_address TEXT NOT NULL,
            payment_status TEXT NOT NULL DEFAULT 'unpaid',
            status TEXT NOT NULL DEFAULT 'pending_payment',
            total_cents BIGINT NOT NULL CHECK (total_cents >= 0),
            deposit_cents BIGINT NOT NULL CHECK (deposit_cents >= 0),
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_commerce_orders_user_created ON commerce_orders(username, created_at DESC)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_commerce_orders_status ON commerce_orders(status, created_at DESC)"#,
    r#"CREATE TABLE IF NOT EXISTS commerce_order_items (
            id BIGSERIAL PRIMARY KEY,
            order_id TEXT NOT NULL REFERENCES commerce_orders(id) ON DELETE CASCADE,
            product_id BIGINT NOT NULL REFERENCES commerce_products(id),
            product_name TEXT NOT NULL,
            unit_label TEXT NOT NULL,
            quantity INTEGER NOT NULL CHECK (quantity > 0),
            unit_price_cents BIGINT NOT NULL CHECK (unit_price_cents > 0),
            unit_deposit_cents BIGINT NOT NULL CHECK (unit_deposit_cents >= 0)
        )"#,
    r#"CREATE TABLE IF NOT EXISTS commerce_order_status_logs (
            id BIGSERIAL PRIMARY KEY,
            order_id TEXT NOT NULL REFERENCES commerce_orders(id) ON DELETE CASCADE,
            status TEXT NOT NULL,
            note TEXT NOT NULL DEFAULT '',
            actor_username TEXT NULL,
            created_at BIGINT NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_commerce_order_status_logs_order ON commerce_order_status_logs(order_id, created_at DESC)"#,
    r#"CREATE TABLE IF NOT EXISTS store_products (
            id BIGSERIAL PRIMARY KEY,
            name TEXT NOT NULL,
            sku TEXT NOT NULL UNIQUE,
            unit_label TEXT NOT NULL,
            price_cents BIGINT NOT NULL CHECK (price_cents > 0),
            stock_quantity INTEGER NOT NULL DEFAULT 0 CHECK (stock_quantity >= 0),
            description TEXT NOT NULL DEFAULT '',
            cover_image TEXT NULL,
            is_active BOOLEAN NOT NULL DEFAULT TRUE,
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL
        )"#,
    r#"CREATE TABLE IF NOT EXISTS store_orders (
            id TEXT PRIMARY KEY,
            order_no TEXT NOT NULL UNIQUE,
            username TEXT NOT NULL REFERENCES app_users(username),
            recipient_name TEXT NOT NULL,
            recipient_phone TEXT NOT NULL,
            shipping_address TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending_payment',
            total_cents BIGINT NOT NULL CHECK (total_cents >= 0),
            created_at BIGINT NOT NULL,
            updated_at BIGINT NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_store_orders_user_created ON store_orders(username, created_at DESC)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_store_orders_status_created ON store_orders(status, created_at DESC)"#,
    r#"CREATE TABLE IF NOT EXISTS store_order_items (
            id BIGSERIAL PRIMARY KEY,
            order_id TEXT NOT NULL REFERENCES store_orders(id) ON DELETE CASCADE,
            product_id BIGINT NOT NULL REFERENCES store_products(id),
            product_name TEXT NOT NULL,
            unit_label TEXT NOT NULL,
            quantity INTEGER NOT NULL CHECK (quantity > 0),
            unit_price_cents BIGINT NOT NULL CHECK (unit_price_cents > 0)
        )"#,
];
