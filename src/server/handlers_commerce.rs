use axum::{
    extract::{Path, State},
    response::Response,
};
use serde_json::json;
use sqlx::Row;

use crate::server::{AppState, api_response, api_success};

async fn batch_products(
    state: &AppState,
    batch_id: &str,
) -> Result<Vec<serde_json::Value>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT
            p.id,
            p.name,
            p.sku,
            p.unit_label,
            p.price_cents,
            p.deposit_cents,
            bp.quota,
            bp.sold_quantity
        FROM commerce_batch_products bp
        JOIN commerce_products p ON p.id = bp.product_id
        WHERE bp.batch_id = $1 AND p.is_active = TRUE
        ORDER BY p.id ASC
        "#,
    )
    .bind(batch_id)
    .fetch_all(&state.db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let quota = row.try_get::<i32, _>("quota").unwrap_or(0);
            let sold = row.try_get::<i32, _>("sold_quantity").unwrap_or(0);
            json!({
                "id": row.try_get::<i64, _>("id").unwrap_or_default(),
                "name": row.try_get::<String, _>("name").unwrap_or_default(),
                "sku": row.try_get::<String, _>("sku").unwrap_or_default(),
                "unit_label": row.try_get::<String, _>("unit_label").unwrap_or_default(),
                "price_cents": row.try_get::<i64, _>("price_cents").unwrap_or_default(),
                "deposit_cents": row.try_get::<i64, _>("deposit_cents").unwrap_or_default(),
                "quota": quota,
                "sold_quantity": sold,
                "remaining_quantity": quota.saturating_sub(sold)
            })
        })
        .collect())
}

pub(crate) async fn storefront_handler(State(state): State<AppState>) -> Response {
    let rows = match sqlx::query(
        r#"
        SELECT
            b.id,
            b.batch_code,
            b.title,
            b.status,
            b.open_at,
            b.close_at,
            b.harvest_start_at,
            b.harvest_end_at,
            b.ship_at,
            b.planned_quantity,
            o.id AS orchard_id,
            o.name AS orchard_name,
            o.description AS orchard_description,
            o.location AS orchard_location,
            o.farmer_name,
            o.cover_image
        FROM commerce_batches b
        JOIN commerce_orchards o ON o.id = b.orchard_id
        WHERE b.is_active = TRUE
          AND o.is_active = TRUE
          AND b.status IN ('preorder', 'open')
        ORDER BY COALESCE(b.open_at, b.created_at) DESC
        "#,
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!("加载商业首页失败: {}", error);
            return api_response(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                500,
                "加载开团数据失败",
                json!({}),
            );
        }
    };

    let mut batches = Vec::with_capacity(rows.len());
    for row in rows {
        let batch_id = row.try_get::<String, _>("id").unwrap_or_default();
        let products = match batch_products(&state, &batch_id).await {
            Ok(products) => products,
            Err(error) => {
                tracing::error!("加载商业批次商品失败: {}", error);
                return api_response(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    500,
                    "加载开团商品失败",
                    json!({}),
                );
            }
        };

        batches.push(json!({
            "id": batch_id,
            "batch_code": row.try_get::<String, _>("batch_code").unwrap_or_default(),
            "title": row.try_get::<String, _>("title").unwrap_or_default(),
            "status": row.try_get::<String, _>("status").unwrap_or_default(),
            "open_at": row.try_get::<Option<i64>, _>("open_at").unwrap_or(None),
            "close_at": row.try_get::<Option<i64>, _>("close_at").unwrap_or(None),
            "harvest_start_at": row.try_get::<Option<i64>, _>("harvest_start_at").unwrap_or(None),
            "harvest_end_at": row.try_get::<Option<i64>, _>("harvest_end_at").unwrap_or(None),
            "ship_at": row.try_get::<Option<i64>, _>("ship_at").unwrap_or(None),
            "planned_quantity": row.try_get::<i32, _>("planned_quantity").unwrap_or_default(),
            "orchard": {
                "id": row.try_get::<i64, _>("orchard_id").unwrap_or_default(),
                "name": row.try_get::<String, _>("orchard_name").unwrap_or_default(),
                "description": row.try_get::<String, _>("orchard_description").unwrap_or_default(),
                "location": row.try_get::<String, _>("orchard_location").unwrap_or_default(),
                "farmer_name": row.try_get::<String, _>("farmer_name").unwrap_or_default(),
                "cover_image": row.try_get::<Option<String>, _>("cover_image").unwrap_or(None)
            },
            "products": products
        }));
    }

    api_success(json!({
        "batches": batches,
        "generated_at": crate::server::now_millis()
    }))
}

pub(crate) async fn batch_trace_handler(
    State(state): State<AppState>,
    Path(batch_id): Path<String>,
) -> Response {
    let row = match sqlx::query(
        r#"
        SELECT
            b.id,
            b.batch_code,
            b.title,
            b.status,
            b.open_at,
            b.close_at,
            b.harvest_start_at,
            b.harvest_end_at,
            b.ship_at,
            o.id AS orchard_id,
            o.name AS orchard_name,
            o.description AS orchard_description,
            o.location AS orchard_location,
            o.farmer_name,
            o.cover_image
        FROM commerce_batches b
        JOIN commerce_orchards o ON o.id = b.orchard_id
        WHERE b.id = $1
          AND b.is_active = TRUE
          AND b.status NOT IN ('draft', 'cancelled')
        LIMIT 1
        "#,
    )
    .bind(&batch_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            return api_response(
                axum::http::StatusCode::NOT_FOUND,
                404,
                "批次不存在",
                json!({}),
            );
        }
        Err(error) => {
            tracing::error!("加载批次追溯失败: {}", error);
            return api_response(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                500,
                "加载批次追溯失败",
                json!({}),
            );
        }
    };

    let products = match batch_products(&state, &batch_id).await {
        Ok(products) => products,
        Err(error) => {
            tracing::error!("加载追溯商品失败: {}", error);
            return api_response(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                500,
                "加载批次商品失败",
                json!({}),
            );
        }
    };

    api_success(json!({
        "batch": {
            "id": row.try_get::<String, _>("id").unwrap_or_default(),
            "batch_code": row.try_get::<String, _>("batch_code").unwrap_or_default(),
            "title": row.try_get::<String, _>("title").unwrap_or_default(),
            "status": row.try_get::<String, _>("status").unwrap_or_default(),
            "open_at": row.try_get::<Option<i64>, _>("open_at").unwrap_or(None),
            "close_at": row.try_get::<Option<i64>, _>("close_at").unwrap_or(None),
            "harvest_start_at": row.try_get::<Option<i64>, _>("harvest_start_at").unwrap_or(None),
            "harvest_end_at": row.try_get::<Option<i64>, _>("harvest_end_at").unwrap_or(None),
            "ship_at": row.try_get::<Option<i64>, _>("ship_at").unwrap_or(None),
            "orchard": {
                "id": row.try_get::<i64, _>("orchard_id").unwrap_or_default(),
                "name": row.try_get::<String, _>("orchard_name").unwrap_or_default(),
                "description": row.try_get::<String, _>("orchard_description").unwrap_or_default(),
                "location": row.try_get::<String, _>("orchard_location").unwrap_or_default(),
                "farmer_name": row.try_get::<String, _>("farmer_name").unwrap_or_default(),
                "cover_image": row.try_get::<Option<String>, _>("cover_image").unwrap_or(None)
            },
            "products": products,
            "traceability": {
                "source": "合作果园产地直发",
                "message": "该批次的采摘、分选、装箱和发货信息由运营人员在履约过程中补充。"
            }
        }
    }))
}
