// 契约表分组：账号与农事识别。
// 对应 Django navel_backend_git/api/models.py 中的 User / AuthToken / HomeData /
// GrowthTracking / DiagnoseData / DiagnoseListItem / FertilizationPlan /
// RecommendedFertilizer / ApplicationSchedule / DiseasePrediction /
// DiseaseRecognitionRecord / TemperatureHumidityData / Task 共 13 个模型。
// 表名取 Meta.db_table，列名取字段名（ForeignKey 追加 _id 后缀），与 Django 逐字对齐。
// 注意：`user` 是 PostgreSQL 保留字，建表与引用都必须双引号包裹。
// 执行顺序要求：`user` 必须最先建，其余表均只依赖 `user`。

pub(super) const DDL: &[&str] = &[
    // User -> user
    r#"CREATE TABLE IF NOT EXISTS "user" (
            id UUID PRIMARY KEY,
            username VARCHAR(150) NOT NULL UNIQUE,
            password VARCHAR(128) NOT NULL,
            email VARCHAR(254) NULL,
            role VARCHAR(20) NOT NULL DEFAULT 'farmer' CHECK (role IN ('farmer', 'buyer')),
            orchard_address VARCHAR(255) NULL,
            latitude DOUBLE PRECISION NULL,
            longitude DOUBLE PRECISION NULL,
            created_at TIMESTAMPTZ NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL
        )"#,
    // AuthToken -> auth_token（主键列名是 key，不是 id）
    r#"CREATE TABLE IF NOT EXISTS auth_token (
            key UUID PRIMARY KEY,
            user_id UUID NOT NULL REFERENCES "user"(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            created_at TIMESTAMPTZ NOT NULL,
            expires_at TIMESTAMPTZ NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_auth_token_user_id ON auth_token(user_id)"#,
    // HomeData -> home_data
    r#"CREATE TABLE IF NOT EXISTS home_data (
            id UUID PRIMARY KEY,
            weather_condition VARCHAR(50) NOT NULL,
            temperature_range VARCHAR(50) NOT NULL,
            suggestion TEXT NOT NULL,
            health_score INTEGER NOT NULL,
            weekly_alerts INTEGER NOT NULL DEFAULT 0,
            pending_tasks INTEGER NOT NULL DEFAULT 0,
            growth_rate DOUBLE PRECISION NOT NULL,
            diagnosis_status VARCHAR(50) NOT NULL,
            growth_status VARCHAR(50) NOT NULL,
            created_at TIMESTAMPTZ NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL
        )"#,
    // GrowthTracking -> growth_tracking
    r#"CREATE TABLE IF NOT EXISTS growth_tracking (
            id UUID PRIMARY KEY,
            growth_stage_text VARCHAR(50) NOT NULL,
            growth_stage_duration VARCHAR(50) NOT NULL,
            start_date DATE NOT NULL,
            end_date DATE NOT NULL,
            fruit_expansion_start_date DATE NOT NULL,
            fruit_expansion_end_date DATE NOT NULL,
            color_change_start_date DATE NOT NULL,
            color_change_end_date DATE NOT NULL,
            young_fruit_start_date DATE NOT NULL,
            young_fruit_end_date DATE NOT NULL,
            diameter DOUBLE PRECISION NOT NULL,
            ratio DOUBLE PRECISION NOT NULL,
            created_at TIMESTAMPTZ NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL
        )"#,
    // DiagnoseData -> diagnose_data
    r#"CREATE TABLE IF NOT EXISTS diagnose_data (
            id UUID PRIMARY KEY,
            data_date DATE NOT NULL,
            nitrogen_value DOUBLE PRECISION NOT NULL,
            phosphorus_value DOUBLE PRECISION NOT NULL,
            potassium_value DOUBLE PRECISION NOT NULL,
            percentage DOUBLE PRECISION NOT NULL,
            created_at TIMESTAMPTZ NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL
        )"#,
    // DiagnoseListItem -> diagnose_list_items
    r#"CREATE TABLE IF NOT EXISTS diagnose_list_items (
            id UUID PRIMARY KEY,
            diagnose_id UUID NOT NULL REFERENCES diagnose_data(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            title VARCHAR(200) NOT NULL,
            content TEXT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_diagnose_list_items_diagnose_id ON diagnose_list_items(diagnose_id)"#,
    // FertilizationPlan -> fertilization_plan
    r#"CREATE TABLE IF NOT EXISTS fertilization_plan (
            id UUID PRIMARY KEY,
            plan_id VARCHAR(100) NOT NULL UNIQUE,
            title VARCHAR(200) NOT NULL,
            content TEXT NOT NULL,
            soil_type VARCHAR(50) NOT NULL,
            ph_value DOUBLE PRECISION NOT NULL,
            nitrogen_level VARCHAR(50) NOT NULL,
            phosphorus_level VARCHAR(50) NOT NULL,
            potassium_level VARCHAR(50) NOT NULL,
            growth_stage VARCHAR(50) NOT NULL,
            tree_age INTEGER NOT NULL,
            area_size DOUBLE PRECISION NOT NULL,
            created_at TIMESTAMPTZ NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL
        )"#,
    // RecommendedFertilizer -> recommended_fertilizer（无 created_at）
    r#"CREATE TABLE IF NOT EXISTS recommended_fertilizer (
            id UUID PRIMARY KEY,
            plan_id UUID NOT NULL REFERENCES fertilization_plan(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            name VARCHAR(100) NOT NULL,
            amount VARCHAR(50) NOT NULL,
            application_method VARCHAR(100) NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_recommended_fertilizer_plan_id ON recommended_fertilizer(plan_id)"#,
    // ApplicationSchedule -> application_schedule（无 created_at）
    r#"CREATE TABLE IF NOT EXISTS application_schedule (
            id UUID PRIMARY KEY,
            plan_id UUID NOT NULL REFERENCES fertilization_plan(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            stage VARCHAR(100) NOT NULL,
            date DATE NOT NULL,
            description TEXT NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_application_schedule_plan_id ON application_schedule(plan_id)"#,
    // DiseasePrediction -> disease_prediction
    r#"CREATE TABLE IF NOT EXISTS disease_prediction (
            id UUID PRIMARY KEY,
            image VARCHAR(100) NOT NULL,
            predicted_class VARCHAR(100) NOT NULL,
            confidence DOUBLE PRECISION NOT NULL,
            stage VARCHAR(50) NOT NULL,
            created_at TIMESTAMPTZ NOT NULL
        )"#,
    // DiseaseRecognitionRecord -> disease_recognition_record
    r#"CREATE TABLE IF NOT EXISTS disease_recognition_record (
            id UUID PRIMARY KEY,
            user_id UUID NULL REFERENCES "user"(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            image VARCHAR(100) NOT NULL,
            disease_name VARCHAR(100) NOT NULL,
            area VARCHAR(50) NOT NULL DEFAULT '未指定区域',
            risk_level VARCHAR(20) NOT NULL,
            recognition_date DATE NOT NULL,
            confidence DOUBLE PRECISION NOT NULL,
            created_at TIMESTAMPTZ NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_disease_recognition_record_user_id ON disease_recognition_record(user_id)"#,
    // TemperatureHumidityData -> temperature_humidity_data
    r#"CREATE TABLE IF NOT EXISTS temperature_humidity_data (
            id UUID PRIMARY KEY,
            user_id UUID NULL REFERENCES "user"(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            timestamp TIMESTAMPTZ NOT NULL,
            temperature DOUBLE PRECISION NOT NULL,
            humidity DOUBLE PRECISION NOT NULL,
            node_id VARCHAR(50) NOT NULL DEFAULT '0x4a80',
            created_at TIMESTAMPTZ NOT NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_temperature_humidity_data_user_id ON temperature_humidity_data(user_id)"#,
    // Task -> task
    r#"CREATE TABLE IF NOT EXISTS task (
            id UUID PRIMARY KEY,
            user_id UUID NULL REFERENCES "user"(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            title VARCHAR(200) NOT NULL,
            description TEXT NOT NULL,
            risk_level VARCHAR(20) NOT NULL DEFAULT '中风险',
            task_type VARCHAR(50) NOT NULL DEFAULT '手动添加',
            source VARCHAR(50) NOT NULL DEFAULT '用户',
            is_completed BOOLEAN NOT NULL DEFAULT FALSE,
            created_at TIMESTAMPTZ NOT NULL,
            completed_at TIMESTAMPTZ NULL
        )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_task_user_id ON task(user_id)"#,
];
