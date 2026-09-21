//! 网页超集专用表（**不参与 Django 契约**）。
//!
//! 网页后台仪表盘的统计口径依赖自研识别记录的**富字段**：`is_citrus_leaf`、`is_healthy`、
//! `predicted_class`、`severity`、`treatment_suggestion`、`image_quality_warning`、`temp`、`humm`
//! 等，共 17 列；而 Django 的契约表 `disease_recognition_record` 只有 9 列
//! （`id` / `user_id` / `image` / `disease_name` / `area` / `risk_level` / `recognition_date` /
//! `confidence` / `created_at`）。
//!
//! 处置（`W2_PLAN.md` §3.1(b)）：把原 `app_diagnosis_records` 的列结构**原样**搬成
//! `web_diagnosis_records`，作为网页专表。
//! - App 读契约表 `disease_recognition_record`；
//! - 网页读 `web_diagnosis_records`；
//! - 识别时**双写两张表**（写逻辑由 S2/S3 负责，本文件只建表）。
//!
//! 列名与类型与原表**逐字一致**——仪表盘统计直接依赖这些列名，改一个就统计不出来。
//! 原表 `app_diagnosis_records` 保留到 S5 退役，此处不是它的替代，而是它的承接者。

pub(super) const DDL: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS web_diagnosis_records (
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
    r#"CREATE INDEX IF NOT EXISTS idx_web_diag_user_time ON web_diagnosis_records(username, timestamp DESC)"#,
];
