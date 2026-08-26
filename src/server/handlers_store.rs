use axum::{
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;
use sqlx::Row;

use crate::server::{AppState, api_response, api_success};

fn valid_cover_file_name(file_name: &str) -> bool {
    let extension = file_name.rsplit('.').next().unwrap_or("");
    matches!(extension, "jpg" | "jpeg" | "png" | "webp")
        && file_name.len() <= 64
        && file_name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

/// 公开访问商城商品封面图（商品数据本身对游客公开）。
pub(crate) async fn store_cover_image_handler(Path(file_name): Path<String>) -> Response {
    if !valid_cover_file_name(&file_name) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let storage_path = format!(
        "{}/{}",
        crate::server::shared::STORE_COVER_UPLOAD_DIR,
        file_name
    );
    let bytes = match tokio::fs::read(&storage_path).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let content_type = if file_name.ends_with(".png") {
        "image/png"
    } else if file_name.ends_with(".webp") {
        "image/webp"
    } else {
        "image/jpeg"
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        Body::from(bytes),
    )
        .into_response()
}

pub(crate) async fn storefront_handler(State(state): State<AppState>) -> Response {
    let rows = match sqlx::query(
        r#"
        SELECT id, name, sku, unit_label, price_cents, stock_quantity, description, cover_image
        FROM store_products
        WHERE is_active = TRUE
        ORDER BY id ASC
        "#,
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!("加载商城商品失败: {}", error);
            return api_response(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                500,
                "加载商品数据失败",
                json!({}),
            );
        }
    };

    let products = rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.try_get::<i64, _>("id").unwrap_or_default(),
                "name": row.try_get::<String, _>("name").unwrap_or_default(),
                "sku": row.try_get::<String, _>("sku").unwrap_or_default(),
                "unit_label": row.try_get::<String, _>("unit_label").unwrap_or_default(),
                "price_cents": row.try_get::<i64, _>("price_cents").unwrap_or_default(),
                "stock_quantity": row.try_get::<i32, _>("stock_quantity").unwrap_or_default(),
                "description": row.try_get::<String, _>("description").unwrap_or_default(),
                "cover_image": row.try_get::<Option<String>, _>("cover_image").unwrap_or(None)
            })
        })
        .collect::<Vec<_>>();

    api_success(json!({
        "products": products,
        "generated_at": crate::server::now_millis()
    }))
}
