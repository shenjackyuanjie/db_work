use axum::{extract::State, response::Response};
use serde_json::json;
use sqlx::Row;

use crate::server::{AppState, api_response, api_success};

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
