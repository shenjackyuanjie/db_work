use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};

use super::super::{AppState, shared::RECOGNITION_RECORDS_UPLOAD_DIR};

fn valid_file_name(file_name: &str) -> bool {
    file_name.ends_with(".jpg")
        && file_name.len() <= 64
        && file_name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

pub(crate) async fn recognition_image_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(file_name): Path<String>,
) -> Response {
    if !valid_file_name(&file_name) {
        return StatusCode::NOT_FOUND.into_response();
    }

    let (_, username) = match crate::auth::ensure_authenticated(&state, &headers).await {
        Ok(value) => value,
        Err((code, body)) => return (code, axum::Json(body)).into_response(),
    };
    let media_path = format!("/media/recognition_records/{file_name}");
    let legacy_path = format!("/uploads/{file_name}");
    // **双表读桥（过渡态，与 S5 的 DROP 同一步收尾）**：识别记录的**写口**在 S2 从
    // `app_diagnosis_records` 搬到了 `web_diagnosis_records`，但**历史行没搬**。
    // 只查新表会让老图全部 404（实测现网 `public.app_diagnosis_records` 有 24 行、
    // 24 行都带 `/media/recognition_records/<uuid>.jpg`，而 `web_diagnosis_records`
    // 在现网**还不存在**，首次启动才建且是空的）；只查旧表则新图全 404（写口已经不往那里写了）。
    // 所以两张都要查。
    //
    // ⚠️ 这条 UNION 是 `app_diagnosis_records` 的 DDL **必须留着**的原因之一：
    // S5 执行 DROP 时，必须**同一步**删掉这里的第二个 EXISTS，否则新库直接报 `relation does not exist`。
    // ⚠️ 这里必须用 `EXISTS(...)` + `bool`，**不要**写 `SELECT 1 ...` + `query_scalar::<_, i64>`：
    // `SELECT 1` 在 PG 里是 **INT4**，而 `query_scalar::<_, i64>` 要的是 INT8，sqlx 会在解码时报
    // `mismatched types; Rust type i64 (as SQL type INT8) is not compatible with SQL type INT4`
    // → 该端点对**任何**合法会话都返回 **500**（实测：合法 token 500、无 token 401、他人 token 404）。
    // 这个写法是从旧版沿袭下来的（旧版单查 `app_diagnosis_records` 时就已经 500），
    // 不是 UNION 引入的；改用 `EXISTS` 后类型是 `bool`，不存在这个歧义。
    let owner_exists = sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS (
               SELECT 1 FROM web_diagnosis_records
                WHERE username = $1 AND (image_path = $2 OR image_path = $3 OR image_path LIKE '%' || $2)
               UNION ALL
               SELECT 1 FROM app_diagnosis_records
                WHERE username = $1 AND (image_path = $2 OR image_path = $3 OR image_path LIKE '%' || $2)
           )"#,
    )
    .bind(username)
    .bind(&media_path)
    .bind(&legacy_path)
    .fetch_one(&state.db)
    .await;

    match owner_exists {
        Ok(true) => {}
        Ok(false) => return StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::error!(%err, "校验识别图片归属失败");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    let storage_path = format!("{RECOGNITION_RECORDS_UPLOAD_DIR}/{file_name}");
    let legacy_storage_path = format!("static/uploads/{file_name}");
    let bytes = match tokio::fs::read(&storage_path).await {
        Ok(bytes) => Ok(bytes),
        Err(_) => tokio::fs::read(&legacy_storage_path).await,
    };

    match bytes {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                (header::CACHE_CONTROL, "private, max-age=3600"),
            ],
            Body::from(bytes),
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
