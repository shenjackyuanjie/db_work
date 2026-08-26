use axum::{
    Json,
    extract::{Json as AxumJson, Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use sqlx::{PgPool, Row, postgres::PgRow};
use std::collections::HashSet;
use uuid::Uuid;

use crate::{
    server::{AppState, api_response, api_success, now_millis, save_store_cover_image},
    system_settings::append_audit_log,
};

use super::auth::{ensure_admin, ensure_authenticated};

const STORE_ORDER_STATUSES: &[&str] = &[
    "pending_payment",
    "paid",
    "shipped",
    "completed",
    "cancelled",
    "refunded",
];

/// 商城商品封面上传大小与尺寸上限。
const MAX_STORE_COVER_BYTES: usize = 8 * 1024 * 1024;
const MAX_STORE_COVER_DIMENSION: u32 = 4096;

#[derive(Debug, Deserialize)]
pub(crate) struct CreateStoreProductRequest {
    pub name: String,
    pub sku: String,
    pub unit_label: String,
    pub price_cents: i64,
    pub stock_quantity: i32,
    #[serde(default)]
    pub description: String,
    pub cover_image: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateStoreProductRequest {
    pub name: String,
    pub sku: String,
    pub unit_label: String,
    pub price_cents: i64,
    pub stock_quantity: i32,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub cover_image: Option<String>,
    #[serde(default)]
    pub is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateStoreOrderItemRequest {
    pub product_id: i64,
    pub quantity: i32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateStoreOrderRequest {
    pub recipient_name: String,
    pub recipient_phone: String,
    pub shipping_address: String,
    pub items: Vec<CreateStoreOrderItemRequest>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateStoreOrderStatusRequest {
    pub order_id: String,
    pub status: String,
    #[serde(default)]
    pub note: String,
}

fn error_response(status: StatusCode, code: u16, message: &str) -> Response {
    api_response(status, code, message, json!({}))
}

fn valid_status(value: &str) -> bool {
    STORE_ORDER_STATUSES.contains(&value)
}

fn store_order_payload(row: &PgRow, items: Vec<serde_json::Value>) -> serde_json::Value {
    json!({
        "id": row.try_get::<String, _>("id").unwrap_or_default(),
        "order_no": row.try_get::<String, _>("order_no").unwrap_or_default(),
        "username": row.try_get::<String, _>("username").unwrap_or_default(),
        "recipient_name": row.try_get::<String, _>("recipient_name").unwrap_or_default(),
        "recipient_phone": row.try_get::<String, _>("recipient_phone").unwrap_or_default(),
        "shipping_address": row.try_get::<String, _>("shipping_address").unwrap_or_default(),
        "status": row.try_get::<String, _>("status").unwrap_or_default(),
        "total_cents": row.try_get::<i64, _>("total_cents").unwrap_or_default(),
        "created_at": row.try_get::<i64, _>("created_at").unwrap_or_default(),
        "updated_at": row.try_get::<i64, _>("updated_at").unwrap_or_default(),
        "items": items
    })
}

async fn load_store_order_items(
    pool: &PgPool,
    order_id: &str,
) -> Result<Vec<serde_json::Value>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT product_id, product_name, unit_label, quantity, unit_price_cents FROM store_order_items WHERE order_id = $1 ORDER BY id ASC",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let quantity = row.try_get::<i32, _>("quantity").unwrap_or_default();
            let unit_price = row
                .try_get::<i64, _>("unit_price_cents")
                .unwrap_or_default();
            json!({
                "product_id": row.try_get::<i64, _>("product_id").unwrap_or_default(),
                "product_name": row.try_get::<String, _>("product_name").unwrap_or_default(),
                "unit_label": row.try_get::<String, _>("unit_label").unwrap_or_default(),
                "quantity": quantity,
                "unit_price_cents": unit_price,
                "line_total_cents": quantity as i64 * unit_price
            })
        })
        .collect())
}

pub(crate) async fn create_store_product_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<CreateStoreProductRequest>,
) -> Response {
    let admin = match ensure_admin(&state, &headers).await {
        Ok(username) => username,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    if payload.name.trim().is_empty()
        || payload.sku.trim().is_empty()
        || payload.unit_label.trim().is_empty()
        || payload.price_cents <= 0
        || payload.stock_quantity < 0
    {
        return error_response(StatusCode::BAD_REQUEST, 400, "商品信息不完整");
    }

    let now = now_millis() as i64;
    let result = sqlx::query(
        "INSERT INTO store_products (name, sku, unit_label, price_cents, stock_quantity, description, cover_image, is_active, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, TRUE, $8, $8)
         RETURNING id",
    )
    .bind(payload.name.trim())
    .bind(payload.sku.trim())
    .bind(payload.unit_label.trim())
    .bind(payload.price_cents)
    .bind(payload.stock_quantity)
    .bind(payload.description.trim())
    .bind(payload.cover_image)
    .bind(now)
    .fetch_one(&state.db)
    .await;

    let product_id = match result {
        Ok(row) => row.try_get::<i64, _>("id").unwrap_or_default(),
        Err(_) => {
            return error_response(StatusCode::CONFLICT, 409, "SKU 已存在或商品创建失败");
        }
    };
    let _ = append_audit_log(
        &state.db,
        "action",
        Some(&admin),
        &format!(
            "创建商城商品: {} ({})",
            payload.name.trim(),
            payload.sku.trim()
        ),
    )
    .await;
    api_success(json!({ "id": product_id }))
}

pub(crate) async fn list_store_products_admin_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }
    let rows = match sqlx::query(
        "SELECT id, name, sku, unit_label, price_cents, stock_quantity, description, cover_image, is_active, created_at, updated_at FROM store_products ORDER BY id DESC",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "读取商品列表失败"),
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
                "cover_image": row.try_get::<Option<String>, _>("cover_image").unwrap_or(None),
                "is_active": row.try_get::<bool, _>("is_active").unwrap_or(false),
                "created_at": row.try_get::<i64, _>("created_at").unwrap_or_default()
            })
        })
        .collect::<Vec<_>>();
    api_success(json!({ "products": products }))
}

pub(crate) async fn toggle_store_product_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(product_id): Path<i64>,
) -> Response {
    let admin = match ensure_admin(&state, &headers).await {
        Ok(username) => username,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    let now = now_millis() as i64;
    let affected = sqlx::query(
        "UPDATE store_products SET is_active = NOT is_active, updated_at = $1 WHERE id = $2",
    )
    .bind(now)
    .bind(product_id)
    .execute(&state.db)
    .await
    .map(|result| result.rows_affected())
    .unwrap_or(0);
    if affected == 0 {
        return error_response(StatusCode::NOT_FOUND, 404, "商品不存在");
    }
    let _ = append_audit_log(
        &state.db,
        "action",
        Some(&admin),
        &format!("切换商城商品上下架状态: id={}", product_id),
    )
    .await;
    api_success(json!({ "id": product_id }))
}

pub(crate) async fn update_store_product_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(product_id): Path<i64>,
    AxumJson(payload): AxumJson<UpdateStoreProductRequest>,
) -> Response {
    let admin = match ensure_admin(&state, &headers).await {
        Ok(username) => username,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    if payload.name.trim().is_empty()
        || payload.sku.trim().is_empty()
        || payload.unit_label.trim().is_empty()
        || payload.price_cents <= 0
        || payload.stock_quantity < 0
    {
        return error_response(StatusCode::BAD_REQUEST, 400, "商品信息不完整");
    }
    let cover_image = payload
        .cover_image
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let now = now_millis() as i64;
    let affected = sqlx::query(
        "UPDATE store_products SET name = $1, sku = $2, unit_label = $3, price_cents = $4, stock_quantity = $5, description = $6, cover_image = $7, is_active = $8, updated_at = $9 WHERE id = $10",
    )
    .bind(payload.name.trim())
    .bind(payload.sku.trim())
    .bind(payload.unit_label.trim())
    .bind(payload.price_cents)
    .bind(payload.stock_quantity)
    .bind(payload.description.trim())
    .bind(cover_image)
    .bind(payload.is_active.unwrap_or(true))
    .bind(now)
    .bind(product_id)
    .execute(&state.db)
    .await
    .map(|result| result.rows_affected())
    .unwrap_or(0);
    if affected == 0 {
        return error_response(StatusCode::NOT_FOUND, 404, "商品不存在");
    }
    let _ = append_audit_log(
        &state.db,
        "action",
        Some(&admin),
        &format!(
            "更新商城商品: {} ({})",
            payload.name.trim(),
            payload.sku.trim()
        ),
    )
    .await;
    api_success(json!({ "id": product_id }))
}

pub(crate) async fn upload_store_cover_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(product_id): Path<i64>,
    mut multipart: Multipart,
) -> Response {
    let admin = match ensure_admin(&state, &headers).await {
        Ok(username) => username,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    let exists = sqlx::query_scalar::<_, i64>("SELECT id FROM store_products WHERE id = $1")
        .bind(product_id)
        .fetch_optional(&state.db)
        .await
        .map(|row| row.is_some())
        .unwrap_or(false);
    if !exists {
        return error_response(StatusCode::NOT_FOUND, 404, "商品不存在");
    }

    let mut cover_image: Option<String> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                if field.name().map(|name| name == "image").unwrap_or(false) {
                    let mime_type = field
                        .content_type()
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "image/jpeg".to_string());
                    match field.bytes().await {
                        Ok(bytes) => {
                            if bytes.len() > MAX_STORE_COVER_BYTES {
                                return error_response(
                                    StatusCode::BAD_REQUEST,
                                    400,
                                    "封面图片过大，最大 8MB",
                                );
                            }
                            match image::ImageReader::new(std::io::Cursor::new(&bytes))
                                .with_guessed_format()
                                .map_err(|_| ())
                                .and_then(|reader| reader.into_dimensions().map_err(|_| ()))
                            {
                                Ok((width, height)) => {
                                    if width > MAX_STORE_COVER_DIMENSION
                                        || height > MAX_STORE_COVER_DIMENSION
                                    {
                                        return error_response(
                                            StatusCode::BAD_REQUEST,
                                            400,
                                            "封面图片尺寸过大，最长边不可超过 4096px",
                                        );
                                    }
                                }
                                Err(_) => {
                                    return error_response(
                                        StatusCode::BAD_REQUEST,
                                        400,
                                        "封面图片无法解析，请上传有效的图片",
                                    );
                                }
                            }
                            match save_store_cover_image(&mime_type, &bytes) {
                                Ok(path) => cover_image = Some(path),
                                Err(error) => {
                                    return error_response(
                                        StatusCode::BAD_REQUEST,
                                        400,
                                        &error.to_string(),
                                    );
                                }
                            }
                        }
                        Err(_) => {
                            return error_response(StatusCode::BAD_REQUEST, 400, "读取图片失败");
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(error) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    400,
                    &format!("解析上传失败: {}", error),
                );
            }
        }
    }
    let cover_image = match cover_image {
        Some(path) => path,
        None => return error_response(StatusCode::BAD_REQUEST, 400, "缺少 image 字段"),
    };

    let now = now_millis() as i64;
    let affected =
        sqlx::query("UPDATE store_products SET cover_image = $1, updated_at = $2 WHERE id = $3")
            .bind(&cover_image)
            .bind(now)
            .bind(product_id)
            .execute(&state.db)
            .await
            .map(|result| result.rows_affected())
            .unwrap_or(0);
    if affected == 0 {
        return error_response(StatusCode::NOT_FOUND, 404, "商品不存在");
    }
    let _ = append_audit_log(
        &state.db,
        "action",
        Some(&admin),
        &format!("更新商城商品封面: id={}", product_id),
    )
    .await;
    api_success(json!({ "id": product_id, "cover_image": cover_image }))
}

pub(crate) async fn create_store_order_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<CreateStoreOrderRequest>,
) -> Response {
    let (_, username) = match ensure_authenticated(&state, &headers).await {
        Ok(value) => value,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    if payload.recipient_name.trim().is_empty()
        || payload.recipient_phone.trim().is_empty()
        || payload.shipping_address.trim().is_empty()
        || payload.items.is_empty()
    {
        return error_response(StatusCode::BAD_REQUEST, 400, "订单信息不完整");
    }
    let mut product_ids = HashSet::new();
    if payload
        .items
        .iter()
        .any(|item| item.quantity <= 0 || !product_ids.insert(item.product_id))
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            400,
            "订单商品不能重复且数量必须大于 0",
        );
    }

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "创建订单失败"),
    };

    let mut total_cents = 0_i64;
    let mut priced_items = Vec::with_capacity(payload.items.len());
    for item in &payload.items {
        let row = match sqlx::query(
            "SELECT name, unit_label, price_cents, stock_quantity FROM store_products WHERE id = $1 AND is_active = TRUE FOR UPDATE",
        )
        .bind(item.product_id)
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(Some(row)) => row,
            Ok(None) => return error_response(StatusCode::BAD_REQUEST, 400, "商品不存在或已下架"),
            Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "读取商品库存失败"),
        };
        let stock = row.try_get::<i32, _>("stock_quantity").unwrap_or_default();
        if item.quantity > stock {
            return error_response(StatusCode::CONFLICT, 409, "商品库存不足");
        }
        let unit_price = row.try_get::<i64, _>("price_cents").unwrap_or_default();
        total_cents = match total_cents.checked_add(unit_price.saturating_mul(item.quantity as i64))
        {
            Some(value) => value,
            None => return error_response(StatusCode::BAD_REQUEST, 400, "订单金额超出范围"),
        };
        priced_items.push((
            item.product_id,
            item.quantity,
            row.try_get::<String, _>("name").unwrap_or_default(),
            row.try_get::<String, _>("unit_label").unwrap_or_default(),
            unit_price,
        ));
    }

    let order_id = Uuid::new_v4().to_string();
    let order_no = format!("PT{}-{}", now_millis(), &order_id[..8]);
    let now = now_millis() as i64;
    if sqlx::query(
        "INSERT INTO store_orders (id, order_no, username, recipient_name, recipient_phone, shipping_address, status, total_cents, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, 'pending_payment', $7, $8, $8)",
    )
    .bind(&order_id)
    .bind(&order_no)
    .bind(&username)
    .bind(payload.recipient_name.trim())
    .bind(payload.recipient_phone.trim())
    .bind(payload.shipping_address.trim())
    .bind(total_cents)
    .bind(now)
    .execute(&mut *tx)
    .await
    .is_err()
    {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "保存订单失败");
    }

    for (product_id, quantity, name, unit_label, unit_price) in priced_items {
        if sqlx::query(
            "INSERT INTO store_order_items (order_id, product_id, product_name, unit_label, quantity, unit_price_cents) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&order_id)
        .bind(product_id)
        .bind(name)
        .bind(unit_label)
        .bind(quantity)
        .bind(unit_price)
        .execute(&mut *tx)
        .await
        .is_err()
            || sqlx::query(
                "UPDATE store_products SET stock_quantity = stock_quantity - $1, updated_at = $2 WHERE id = $3",
            )
            .bind(quantity)
            .bind(now)
            .bind(product_id)
            .execute(&mut *tx)
            .await
            .is_err()
        {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "保存订单商品失败");
        }
    }
    if tx.commit().await.is_err() {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "创建订单失败");
    }

    api_success(json!({
        "order_id": order_id,
        "order_no": order_no,
        "status": "pending_payment",
        "total_cents": total_cents
    }))
}

pub(crate) async fn list_user_store_orders_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let (_, username) = match ensure_authenticated(&state, &headers).await {
        Ok(value) => value,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    let rows = match sqlx::query(
        "SELECT id, order_no, username, recipient_name, recipient_phone, shipping_address, status, total_cents, created_at, updated_at FROM store_orders WHERE username = $1 ORDER BY created_at DESC",
    )
    .bind(&username)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "读取订单失败"),
    };
    let mut orders = Vec::with_capacity(rows.len());
    for row in rows {
        let order_id = row.try_get::<String, _>("id").unwrap_or_default();
        let items = match load_store_order_items(&state.db, &order_id).await {
            Ok(items) => items,
            Err(_) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "读取订单商品失败");
            }
        };
        orders.push(store_order_payload(&row, items));
    }
    api_success(json!({ "orders": orders }))
}

pub(crate) async fn list_admin_store_orders_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }
    let rows = match sqlx::query(
        "SELECT id, order_no, username, recipient_name, recipient_phone, shipping_address, status, total_cents, created_at, updated_at FROM store_orders ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "读取订单失败"),
    };
    let mut orders = Vec::with_capacity(rows.len());
    for row in rows {
        let order_id = row.try_get::<String, _>("id").unwrap_or_default();
        let items = match load_store_order_items(&state.db, &order_id).await {
            Ok(items) => items,
            Err(_) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "读取订单商品失败");
            }
        };
        orders.push(store_order_payload(&row, items));
    }
    api_success(json!({ "orders": orders }))
}

pub(crate) async fn update_store_order_status_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<UpdateStoreOrderStatusRequest>,
) -> Response {
    let admin = match ensure_admin(&state, &headers).await {
        Ok(username) => username,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    if payload.order_id.trim().is_empty() || !valid_status(&payload.status) {
        return error_response(StatusCode::BAD_REQUEST, 400, "订单状态参数无效");
    }
    let now = now_millis() as i64;
    let affected =
        sqlx::query("UPDATE store_orders SET status = $1, updated_at = $2 WHERE id = $3")
            .bind(&payload.status)
            .bind(now)
            .bind(payload.order_id.trim())
            .execute(&state.db)
            .await
            .map(|result| result.rows_affected())
            .unwrap_or(0);
    if affected == 0 {
        return error_response(StatusCode::NOT_FOUND, 404, "订单不存在");
    }
    let note = if payload.note.trim().is_empty() {
        String::new()
    } else {
        format!("，备注：{}", payload.note.trim())
    };
    let _ = append_audit_log(
        &state.db,
        "action",
        Some(&admin),
        &format!(
            "更新商城订单 {} 状态为 {}{}",
            payload.order_id.trim(),
            payload.status,
            note
        ),
    )
    .await;
    api_success(json!({ "order_id": payload.order_id.trim(), "status": payload.status }))
}

pub(crate) async fn store_overview_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }
    let product_count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM store_products WHERE is_active = TRUE")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    let order_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM store_orders")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);
    let gross_amount_cents = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(total_cents)::BIGINT, 0) FROM store_orders WHERE status NOT IN ('cancelled', 'refunded')",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    let pending_order_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM store_orders WHERE status IN ('pending_payment', 'paid')",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    api_success(json!({
        "product_count": product_count,
        "order_count": order_count,
        "gross_amount_cents": gross_amount_cents,
        "pending_order_count": pending_order_count
    }))
}
