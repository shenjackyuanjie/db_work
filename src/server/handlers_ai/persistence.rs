use crate::models::DiagnosisRecord;

use super::super::{AppState, disease_treatment_text, save_recognition_record_image};

/// 网页侧识别记录的**正主**：`web_diagnosis_records` 是自研 17 列表 `app_diagnosis_records`
/// 的改名后继（列完全一致，见 `src/server/bootstrap/web_tables.rs`）。
/// 网页仪表盘统计与 3D 沙盘的 `latest_diagnosis` 都读它。
const INSERT_WEB_DIAGNOSIS_SQL: &str = "INSERT INTO web_diagnosis_records (id, timestamp, predicted_class, confidence, is_citrus_leaf, citrus_type, is_healthy, disease_name, severity, treatment_suggestion, preventive_measures, image_quality_warning, username, area, temp, humm, image_path) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)";

/// 契约表（Django `DiseaseRecognitionRecord`，9 列），让 **App 的 `/api/recognition-records`**
/// 也能读到网页产生的识别记录。
///
/// 契约表里没有富字段（`is_citrus_leaf` / `severity` / `temp` / `humm` …），按设计丢弃；
/// `user_id` 由用户名反查 `"user"`（保留字必须双引号），查不到时为 NULL（列可空）；
/// `recognition_date` 是 **DATE**，按 `Asia/Shanghai` 取日——与 `store_workspace.rs` 的分桶口径一致。
const INSERT_CONTRACT_RECOGNITION_SQL: &str = r#"INSERT INTO disease_recognition_record
        (id, user_id, image, disease_name, area, risk_level, recognition_date, confidence, created_at)
     VALUES ($1,
             (SELECT id FROM "user" WHERE username = $2 LIMIT 1),
             $3, $4, $5, $6,
             (to_timestamp($7::double precision / 1000) AT TIME ZONE 'Asia/Shanghai')::date,
             $8, $9)"#;

/// 契约表全是 `VARCHAR(N) NOT NULL`，网页侧的值可能超长——超长会直接 INSERT 报错。
fn clamp_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

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

/// 一次识别**写两张表**（同一事务）：
/// 1. `web_diagnosis_records` —— 17 列富字段，供网页仪表盘与 3D 沙盘；
/// 2. `disease_recognition_record` —— 9 列契约表，供 App 的 `/api/recognition-records`。
///
/// **不再写** `app_diagnosis_records`（遗留表，S5 连同 DDL 一起退役）。
pub(super) async fn store_diagnosis_record(
    state: &AppState,
    record: &DiagnosisRecord,
) -> Result<(), sqlx::Error> {
    let mut tx = state.db.begin().await?;

    sqlx::query(INSERT_WEB_DIAGNOSIS_SQL)
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
        .execute(&mut *tx)
        .await?;

    // 富表主键是 TEXT，契约表是 UUID：能解析就沿用同一个 id，否则新生成一个。
    let record_uuid = uuid::Uuid::parse_str(&record.id).unwrap_or_else(|_| uuid::Uuid::new_v4());
    let area = match record.area.as_deref().map(str::trim).unwrap_or_default() {
        "" => "未指定区域".to_string(),
        value => clamp_chars(value, 50),
    };
    let risk_level = super::super::risk_from_disease_name(&record.disease_name);

    sqlx::query(INSERT_CONTRACT_RECOGNITION_SQL)
        .bind(record_uuid)
        .bind(record.username.as_deref())
        .bind(clamp_chars(
            record.image_path.as_deref().unwrap_or_default(),
            100,
        ))
        .bind(clamp_chars(&record.disease_name, 100))
        .bind(area)
        .bind(clamp_chars(risk_level, 20))
        .bind(record.timestamp as i64)
        .bind(record.confidence)
        .bind(chrono::Utc::now())
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(())
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
    let task_id = uuid::Uuid::new_v4();
    // `record.timestamp` 是 epoch 毫秒，而契约表 `task.created_at` 是 `TIMESTAMPTZ`。
    let created_at = chrono::DateTime::from_timestamp_millis(record.timestamp as i64)
        .unwrap_or_else(chrono::Utc::now);

    // **写契约表 `task`，不再写 `app_tasks`**（S5/G2 的 O5）：
    // `app_tasks` 是退役目标，而这条路径是**活的**（`-v2` 识别成功后会建治理任务）
    // —— 先删它的 DDL 会让这里在运行期报 `relation does not exist`（识别成功但 500）。
    //
    // 用 `INSERT ... SELECT` 从 `"user"` 解析 `user_id`：契约表的 `user_id` 是 UUID 外键，
    // 而调用方给的是 username。找不到用户时插 0 行（不报错），行为与旧版「写个悬空 username」等价但不留脏引用。
    sqlx::query(
        r#"INSERT INTO task
             (id, user_id, title, description, risk_level, task_type, source, is_completed,
              created_at, completed_at)
           SELECT $1, u.id, $2, $3, $4, $5, $6, FALSE, $7, NULL
           FROM "user" u WHERE u.username = $8"#,
    )
    .bind(task_id)
    .bind(&task_title)
    .bind(&task_description)
    .bind("高风险")
    .bind("疾病识别")
    .bind("自动生成")
    .bind(created_at)
    .bind(username)
    .execute(&state.db)
    .await?;

    Ok(Some(task_title))
}
