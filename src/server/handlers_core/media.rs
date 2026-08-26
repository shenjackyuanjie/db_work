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

    let (_, username) = match crate::user_routes::ensure_authenticated(&state, &headers).await {
        Ok(value) => value,
        Err((code, body)) => return (code, axum::Json(body)).into_response(),
    };
    let media_path = format!("/media/recognition_records/{file_name}");
    let legacy_path = format!("/uploads/{file_name}");
    let owner_exists = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM app_diagnosis_records WHERE username = $1 AND (image_path = $2 OR image_path = $3 OR image_path LIKE '%' || $2) LIMIT 1",
    )
    .bind(username)
    .bind(&media_path)
    .bind(&legacy_path)
    .fetch_optional(&state.db)
    .await;

    match owner_exists {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
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
