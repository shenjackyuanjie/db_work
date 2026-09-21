// 历史表分组：本服务自有的会话/任务/农情/果园沙盘表与现货商城客服表。
// 这些表名与 Django 契约表无关；**留着只因为有活的读写方**，逐条理由见下。
//
// 仍在用：
//   - `app_system_settings` / `app_invitations` / `app_pending_users` / `app_admin_audit_logs`
//     —— 注册审批、后台设置、审计日志（`system_settings.rs`、`web/{admin,session,dashboard}.rs`）
//   - `app_orchard_trees` / `app_tree_sensor_records` —— 3D 沙盘（`web/orchard.rs`、`bootstrap.rs` 种子）
//   - `store_support_messages` —— 客服会话（`web/support.rs`）
//   - `app_diagnosis_records` —— **已无写入方，但仍有读取方**：S2 把识别写入口切到
//     `web_diagnosis_records`，G2 又把识别图静态通道（`handlers_core/media.rs`）也跟着切过去。
//     但历史行没搬：现网 `public.app_diagnosis_records` 有 **24 行**（都带
//     `/media/recognition_records/<uuid>.jpg`），而 `web_diagnosis_records` 在现网还不存在、
//     首次启动才建且为空 —— 所以 `media.rs` 现在**同时查两张表**（UNION 双表读桥）。
//     ⚠️ **本表的 DDL 因此不能单独删**：删它必须与 `media.rs` 去掉第二个 EXISTS **同一步**，
//     否则新库上的识别图通道会直接 `relation does not exist`。
//     另外它还端着现网的历史行，「历史数据要不要搬进 `web_diagnosis_records`」是 S5 的
//     `DROP` 脚本要连着裁定的问题。
//
// 已退役（S5/G2）：`app_sessions`（G2 step A）、`app_users`、`app_tasks`、
// `app_temperature_humidity`、`commerce_*`（7 张）、`store_products` / `store_orders` /
// `store_order_items`。它们的读取方只剩 `user_routes/**`、`handlers_commerce.rs` 与
// `handlers_store::storefront_handler`，三者都在 G2 一起删了。
//
// ⚠️ **上面这段里的表名只出现在注释里**。复核「是否真的退役」时别用裸名字 grep——
// 那样会命中这条注释，得出「还在用」的错误结论（`W2_S5_PLAN.md` §0.1 记的正是这个坑，
// 本项目已经栽过三次）。请用「真实 SQL 形态」查：
//   rg -n 'FROM app_sessions|INTO app_sessions|UPDATE app_sessions|FROM app_tasks|INTO app_tasks' src
//   （结果应为空；同理 `app_users` / `commerce_orchards` 等）
// 本文件的事实清单是下面这个数组：只有 8 张表，上面列的那些一个都不在。
//
// 注意：这里的 `CREATE TABLE IF NOT EXISTS` 只影响**新建库**，本文件**不写 `DROP TABLE`**；
// 现网 `public` 的旧表由 S5 的脚本在 `pg_dump` 之后手工执行。

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
