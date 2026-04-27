use crate::models::DiagnosisRecord;

use super::super::{AppState, disease_treatment_text, save_recognition_record_image};

const INSERT_DIAGNOSIS_RECORD_SQL: &str = "INSERT INTO app_diagnosis_records (id, timestamp, predicted_class, confidence, is_citrus_leaf, citrus_type, is_healthy, disease_name, severity, treatment_suggestion, preventive_measures, image_quality_warning, username, area, temp, humm, image_path) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)";

pub(super) fn save_record_image(record_id: &str, image_data: &str) -> Option<String> {
    match save_recognition_record_image(record_id, image_data) {
        Ok(path) => {
            tracing::info!("识别图片已保存: {}", path);
            Some(path)
        }
        Err(err) => {
            tracing::error!("识别图片保存失败: {}", err);
            None
        }
    }
}

pub(super) async fn store_diagnosis_record(
    state: &AppState,
    record: &DiagnosisRecord,
) -> Result<(), sqlx::Error> {
    sqlx::query(INSERT_DIAGNOSIS_RECORD_SQL)
        .bind(&record.id)
        .bind(record.timestamp as i64)
        .bind(&record.predicted_class)
        .bind(record.confidence)
        .bind(record.is_citrus_leaf)
        .bind(&record.citrus_type)
        .bind(record.is_healthy)
        .bind(&record.disease_name)
        .bind(&record.severity)
        .bind(&record.treatment_suggestion)
        .bind(&record.preventive_measures)
        .bind(&record.image_quality_warning)
        .bind(&record.username)
        .bind(&record.area)
        .bind(record.temp)
        .bind(record.humm)
        .bind(&record.image_path)
        .execute(&state.db)
        .await
        .map(|_| ())
}

pub(super) async fn create_disease_task_if_needed(
    state: &AppState,
    record: &DiagnosisRecord,
) -> Result<Option<String>, sqlx::Error> {
    if record.is_healthy
        || record.predicted_class == "非果树"
        || record.predicted_class == "待人工复核"
    {
        return Ok(None);
    }

    let username = match record.username.as_deref() {
        Some(username) => username,
        None => return Ok(None),
    };

    let task_title = format!("{}治理", record.disease_name);
    let task_description = disease_treatment_text(&record.disease_name).to_string();
    let task_id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO app_tasks (id, username, title, description, risk_level, task_type, source, is_completed, created_at, completed_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(&task_id)
    .bind(username)
    .bind(&task_title)
    .bind(&task_description)
    .bind("高风险")
    .bind("疾病识别")
    .bind("自动生成")
    .bind(false)
    .bind(record.timestamp as i64)
    .bind(None::<i64>)
    .execute(&state.db)
    .await?;

    Ok(Some(task_title))
}