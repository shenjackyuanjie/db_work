use axum::{
    Json,
    extract::{Json as AxumJson, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use std::collections::HashSet;
use uuid::Uuid;

use crate::{
    server::{AppState, api_response, api_success, now_millis},
    system_settings::append_audit_log,
};

use super::auth::{ensure_admin, ensure_authenticated};

const BATCH_STATUSES: &[&str] = &[
    "draft",
    "preorder",
    "open",
    "closed",
    "harvesting",
    "shipping",
    "completed",
    "cancelled",
];
const ORDER_STATUSES: &[&str] = &[
    "pending_payment",
    "paid",
    "confirmed",
    "harvesting",
    "packing",
    "shipped",
    "completed",
    "cancelled",
    "refunded",
];
const PAYMENT_STATUSES: &[&str] = &["unpaid", "deposit_paid", "paid", "refunded"];

#[derive(Debug, Deserialize)]
pub(crate) struct CreateOrchardRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub farmer_name: String,
    pub cover_image: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateProductRequest {
    pub name: String,
    pub sku: String,
    pub unit_label: String,
    pub price_cents: i64,
    #[serde(default)]
    pub deposit_cents: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BatchProductRequest {
    pub product_id: i64,
    pub quota: i32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateBatchRequest {
    pub batch_code: String,
    pub title: String,
    pub orchard_id: i64,
    pub status: Option<String>,
    pub open_at: Option<i64>,
    pub close_at: Option<i64>,
    pub harvest_start_at: Option<i64>,
    pub harvest_end_at: Option<i64>,
    pub ship_at: Option<i64>,
    pub planned_quantity: i32,
    pub products: Vec<BatchProductRequest>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateOrderItemRequest {
    pub product_id: i64,
    pub quantity: i32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateOrderRequest {
    pub batch_id: String,
    pub recipient_name: String,
    pub recipient_phone: String,
    pub shipping_address: String,
    pub items: Vec<CreateOrderItemRequest>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateOrderStatusRequest {
    pub order_id: String,
    pub status: String,
    pub payment_status: Option<String>,
    #[serde(default)]
    pub note: String,
}

fn error_response(status: StatusCode, code: u16, message: &str) -> Response {
    api_response(status, code, message, json!({}))
}

fn valid_status(value: &str, allowed: &[&str]) -> bool {
    allowed.contains(&value)
}

fn order_payload(row: &PgRow, items: Vec<serde_json::Value>) -> serde_json::Value {
    json!({
        "id": row.try_get::<String, _>("id").unwrap_or_default(),
        "order_no": row.try_get::<String, _>("order_no").unwrap_or_default(),
        "username": row.try_get::<String, _>("username").unwrap_or_default(),
        "batch_id": row.try_get::<String, _>("batch_id").unwrap_or_default(),
        "recipient_name": row.try_get::<String, _>("recipient_name").unwrap_or_default(),
        "recipient_phone": row.try_get::<String, _>("recipient_phone").unwrap_or_default(),
        "shipping_address": row.try_get::<String, _>("shipping_address").unwrap_or_default(),
        "payment_status": row.try_get::<String, _>("payment_status").unwrap_or_default(),
        "status": row.try_get::<String, _>("status").unwrap_or_default(),
        "total_cents": row.try_get::<i64, _>("total_cents").unwrap_or_default(),
        "deposit_cents": row.try_get::<i64, _>("deposit_cents").unwrap_or_default(),
        "created_at": row.try_get::<i64, _>("created_at").unwrap_or_default(),
        "updated_at": row.try_get::<i64, _>("updated_at").unwrap_or_default(),
        "items": items
    })
}

async fn load_order_items(
    pool: &PgPool,
    order_id: &str,
) -> Result<Vec<serde_json::Value>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT product_id, product_name, unit_label, quantity, unit_price_cents, unit_deposit_cents FROM commerce_order_items WHERE order_id = $1 ORDER BY id ASC",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            json!({
                "product_id": row.try_get::<i64, _>("product_id").unwrap_or_default(),
                "product_name": row.try_get::<String, _>("product_name").unwrap_or_default(),
                "unit_label": row.try_get::<String, _>("unit_label").unwrap_or_default(),
                "quantity": row.try_get::<i32, _>("quantity").unwrap_or_default(),
                "unit_price_cents": row.try_get::<i64, _>("unit_price_cents").unwrap_or_default(),
                "unit_deposit_cents": row.try_get::<i64, _>("unit_deposit_cents").unwrap_or_default()
            })
        })
        .collect())
}

pub(crate) async fn create_orchard_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<CreateOrchardRequest>,
) -> Response {
    let admin = match ensure_admin(&state, &headers).await {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    let name = payload.name.trim();
    if name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, 400, "果园名称不能为空");
    }

    let now = now_millis() as i64;
    let row = match sqlx::query(
        "INSERT INTO commerce_orchards (name, description, location, farmer_name, cover_image, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $6) RETURNING id",
    )
    .bind(name)
    .bind(payload.description.trim())
    .bind(payload.location.trim())
    .bind(payload.farmer_name.trim())
    .bind(payload.cover_image.as_deref().map(str::trim).filter(|v| !v.is_empty()))
    .bind(now)
    .fetch_one(&state.db)
    .await
    {
        Ok(row) => row,
        Err(error) => {
            tracing::error!("创建合作果园失败: {}", error);
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "创建合作果园失败");
        }
    };

    let id = row.try_get::<i64, _>("id").unwrap_or_default();
    let _ = append_audit_log(
        &state.db,
        "commerce",
        Some(&admin),
        &format!("管理员 {} 创建合作果园 {}", admin, name),
    )
    .await;
    api_success(json!({ "id": id, "name": name }))
}

pub(crate) async fn list_orchards_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }
    let rows = match sqlx::query(
        "SELECT id, name, description, location, farmer_name, cover_image, is_active, created_at, updated_at FROM commerce_orchards ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!("查询合作果园失败: {}", error);
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "查询合作果园失败");
        }
    };
    let orchards = rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.try_get::<i64, _>("id").unwrap_or_default(),
                "name": row.try_get::<String, _>("name").unwrap_or_default(),
                "description": row.try_get::<String, _>("description").unwrap_or_default(),
                "location": row.try_get::<String, _>("location").unwrap_or_default(),
                "farmer_name": row.try_get::<String, _>("farmer_name").unwrap_or_default(),
                "cover_image": row.try_get::<Option<String>, _>("cover_image").unwrap_or(None),
                "is_active": row.try_get::<bool, _>("is_active").unwrap_or(false),
                "created_at": row.try_get::<i64, _>("created_at").unwrap_or_default(),
                "updated_at": row.try_get::<i64, _>("updated_at").unwrap_or_default()
            })
        })
        .collect::<Vec<_>>();
    api_success(json!({ "orchards": orchards }))
}

pub(crate) async fn create_product_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<CreateProductRequest>,
) -> Response {
    let admin = match ensure_admin(&state, &headers).await {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    let name = payload.name.trim();
    let sku = payload.sku.trim();
    let unit_label = payload.unit_label.trim();
    if name.is_empty()
        || sku.is_empty()
        || unit_label.is_empty()
        || payload.price_cents <= 0
        || payload.deposit_cents < 0
        || payload.deposit_cents > payload.price_cents
    {
        return error_response(StatusCode::BAD_REQUEST, 400, "商品信息或价格参数无效");
    }

    let now = now_millis() as i64;
    let row = match sqlx::query(
        "INSERT INTO commerce_products (name, sku, unit_label, price_cents, deposit_cents, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $6) RETURNING id",
    )
    .bind(name)
    .bind(sku)
    .bind(unit_label)
    .bind(payload.price_cents)
    .bind(payload.deposit_cents)
    .bind(now)
    .fetch_one(&state.db)
    .await
    {
        Ok(row) => row,
        Err(error) => {
            tracing::error!("创建商业商品失败: {}", error);
            return error_response(StatusCode::CONFLICT, 409, "商品 SKU 已存在或商品创建失败");
        }
    };

    let id = row.try_get::<i64, _>("id").unwrap_or_default();
    let _ = append_audit_log(
        &state.db,
        "commerce",
        Some(&admin),
        &format!("管理员 {} 创建商品 {}", admin, sku),
    )
    .await;
    api_success(json!({ "id": id, "sku": sku }))
}

pub(crate) async fn list_products_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }
    let rows = match sqlx::query(
        "SELECT id, name, sku, unit_label, price_cents, deposit_cents, is_active, created_at, updated_at FROM commerce_products ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!("查询商业商品失败: {}", error);
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "查询商业商品失败");
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
                "deposit_cents": row.try_get::<i64, _>("deposit_cents").unwrap_or_default(),
                "is_active": row.try_get::<bool, _>("is_active").unwrap_or(false),
                "created_at": row.try_get::<i64, _>("created_at").unwrap_or_default(),
                "updated_at": row.try_get::<i64, _>("updated_at").unwrap_or_default()
            })
        })
        .collect::<Vec<_>>();
    api_success(json!({ "products": products }))
}

pub(crate) async fn create_batch_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<CreateBatchRequest>,
) -> Response {
    let admin = match ensure_admin(&state, &headers).await {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    let batch_code = payload.batch_code.trim();
    let title = payload.title.trim();
    let status = payload.status.as_deref().unwrap_or("draft").trim();
    if batch_code.is_empty()
        || title.is_empty()
        || payload.planned_quantity <= 0
        || payload.products.is_empty()
        || !valid_status(status, BATCH_STATUSES)
    {
        return error_response(StatusCode::BAD_REQUEST, 400, "批次参数无效");
    }
    let mut product_ids = HashSet::new();
    if payload
        .products
        .iter()
        .any(|item| item.quota <= 0 || !product_ids.insert(item.product_id))
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            400,
            "批次商品不能重复且配额必须大于 0",
        );
    }

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!("创建批次事务失败: {}", error);
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "创建批次失败");
        }
    };
    let orchard_exists =
        match sqlx::query("SELECT 1 FROM commerce_orchards WHERE id = $1 AND is_active = TRUE")
            .bind(payload.orchard_id)
            .fetch_optional(&mut *tx)
            .await
        {
            Ok(row) => row.is_some(),
            Err(_) => false,
        };
    if !orchard_exists {
        return error_response(StatusCode::NOT_FOUND, 404, "合作果园不存在");
    }

    let batch_id = Uuid::new_v4().to_string();
    let now = now_millis() as i64;
    if sqlx::query(
        "INSERT INTO commerce_batches (id, batch_code, title, orchard_id, status, open_at, close_at, harvest_start_at, harvest_end_at, ship_at, planned_quantity, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12)",
    )
    .bind(&batch_id)
    .bind(batch_code)
    .bind(title)
    .bind(payload.orchard_id)
    .bind(status)
    .bind(payload.open_at)
    .bind(payload.close_at)
    .bind(payload.harvest_start_at)
    .bind(payload.harvest_end_at)
    .bind(payload.ship_at)
    .bind(payload.planned_quantity)
    .bind(now)
    .execute(&mut *tx)
    .await
    .is_err()
    {
        return error_response(StatusCode::CONFLICT, 409, "批次编号已存在或批次创建失败");
    }

    for item in &payload.products {
        let product_exists =
            match sqlx::query("SELECT 1 FROM commerce_products WHERE id = $1 AND is_active = TRUE")
                .bind(item.product_id)
                .fetch_optional(&mut *tx)
                .await
            {
                Ok(row) => row.is_some(),
                Err(_) => false,
            };
        if !product_exists || sqlx::query("INSERT INTO commerce_batch_products (batch_id, product_id, quota) VALUES ($1, $2, $3)")
            .bind(&batch_id)
            .bind(item.product_id)
            .bind(item.quota)
            .execute(&mut *tx)
            .await
            .is_err()
        {
            return error_response(StatusCode::BAD_REQUEST, 400, "批次包含不存在的商品");
        }
    }

    if tx.commit().await.is_err() {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "保存批次失败");
    }
    let _ = append_audit_log(
        &state.db,
        "commerce",
        Some(&admin),
        &format!("管理员 {} 创建销售批次 {}", admin, batch_code),
    )
    .await;
    api_success(json!({ "id": batch_id, "batch_code": batch_code, "status": status }))
}

pub(crate) async fn list_batches_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }
    let rows = match sqlx::query(
        "SELECT b.id, b.batch_code, b.title, b.status, b.planned_quantity, b.open_at, b.close_at, b.harvest_start_at, b.harvest_end_at, b.ship_at, b.created_at, o.id AS orchard_id, o.name AS orchard_name FROM commerce_batches b JOIN commerce_orchards o ON o.id = b.orchard_id ORDER BY b.created_at DESC",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!("查询销售批次失败: {}", error);
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "查询销售批次失败");
        }
    };
    let batches = rows.into_iter().map(|row| json!({
        "id": row.try_get::<String, _>("id").unwrap_or_default(),
        "batch_code": row.try_get::<String, _>("batch_code").unwrap_or_default(),
        "title": row.try_get::<String, _>("title").unwrap_or_default(),
        "status": row.try_get::<String, _>("status").unwrap_or_default(),
        "planned_quantity": row.try_get::<i32, _>("planned_quantity").unwrap_or_default(),
        "open_at": row.try_get::<Option<i64>, _>("open_at").unwrap_or(None),
        "close_at": row.try_get::<Option<i64>, _>("close_at").unwrap_or(None),
        "harvest_start_at": row.try_get::<Option<i64>, _>("harvest_start_at").unwrap_or(None),
        "harvest_end_at": row.try_get::<Option<i64>, _>("harvest_end_at").unwrap_or(None),
        "ship_at": row.try_get::<Option<i64>, _>("ship_at").unwrap_or(None),
        "created_at": row.try_get::<i64, _>("created_at").unwrap_or_default(),
        "orchard": {
            "id": row.try_get::<i64, _>("orchard_id").unwrap_or_default(),
            "name": row.try_get::<String, _>("orchard_name").unwrap_or_default()
        }
    })).collect::<Vec<_>>();
    api_success(json!({ "batches": batches }))
}

pub(crate) async fn create_order_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<CreateOrderRequest>,
) -> Response {
    let (_, username) = match ensure_authenticated(&state, &headers).await {
        Ok(value) => value,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    if payload.batch_id.trim().is_empty()
        || payload.recipient_name.trim().is_empty()
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
    let batch = match sqlx::query(
        "SELECT status, close_at, is_active FROM commerce_batches WHERE id = $1 FOR UPDATE",
    )
    .bind(payload.batch_id.trim())
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, 404, "销售批次不存在"),
        Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "读取销售批次失败");
        }
    };
    let batch_status = batch.try_get::<String, _>("status").unwrap_or_default();
    let close_at = batch.try_get::<Option<i64>, _>("close_at").unwrap_or(None);
    let is_active = batch.try_get::<bool, _>("is_active").unwrap_or(false);
    if !is_active
        || !matches!(batch_status.as_str(), "preorder" | "open")
        || close_at.is_some_and(|value| value < now_millis() as i64)
    {
        return error_response(StatusCode::CONFLICT, 409, "当前批次不在可下单时间内");
    }

    let mut total_cents = 0_i64;
    let mut deposit_cents = 0_i64;
    let mut priced_items = Vec::with_capacity(payload.items.len());
    for item in &payload.items {
        let row = match sqlx::query(
            "SELECT p.name, p.unit_label, p.price_cents, p.deposit_cents, bp.quota, bp.sold_quantity FROM commerce_batch_products bp JOIN commerce_products p ON p.id = bp.product_id WHERE bp.batch_id = $1 AND bp.product_id = $2 AND p.is_active = TRUE FOR UPDATE OF bp",
        )
        .bind(payload.batch_id.trim())
        .bind(item.product_id)
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(Some(row)) => row,
            Ok(None) => return error_response(StatusCode::BAD_REQUEST, 400, "订单包含不属于该批次的商品"),
            Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "读取商品库存失败"),
        };
        let quota = row.try_get::<i32, _>("quota").unwrap_or_default();
        let sold = row.try_get::<i32, _>("sold_quantity").unwrap_or_default();
        if item.quantity > quota.saturating_sub(sold) {
            return error_response(StatusCode::CONFLICT, 409, "商品库存不足");
        }
        let unit_price = row.try_get::<i64, _>("price_cents").unwrap_or_default();
        let unit_deposit = row.try_get::<i64, _>("deposit_cents").unwrap_or_default();
        total_cents = match total_cents.checked_add(unit_price.saturating_mul(item.quantity as i64))
        {
            Some(value) => value,
            None => return error_response(StatusCode::BAD_REQUEST, 400, "订单金额超出范围"),
        };
        deposit_cents =
            deposit_cents.saturating_add(unit_deposit.saturating_mul(item.quantity as i64));
        priced_items.push((
            item.product_id,
            item.quantity,
            row.try_get::<String, _>("name").unwrap_or_default(),
            row.try_get::<String, _>("unit_label").unwrap_or_default(),
            unit_price,
            unit_deposit,
        ));
    }

    let order_id = Uuid::new_v4().to_string();
    let order_no = format!("CG{}-{}", now_millis(), &order_id[..8]);
    let now = now_millis() as i64;
    if sqlx::query(
        "INSERT INTO commerce_orders (id, order_no, username, batch_id, recipient_name, recipient_phone, shipping_address, total_cents, deposit_cents, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)",
    )
    .bind(&order_id)
    .bind(&order_no)
    .bind(&username)
    .bind(payload.batch_id.trim())
    .bind(payload.recipient_name.trim())
    .bind(payload.recipient_phone.trim())
    .bind(payload.shipping_address.trim())
    .bind(total_cents)
    .bind(deposit_cents)
    .bind(now)
    .execute(&mut *tx)
    .await
    .is_err()
    {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "保存订单失败");
    }

    for (product_id, quantity, name, unit_label, unit_price, unit_deposit) in priced_items {
        if sqlx::query("INSERT INTO commerce_order_items (order_id, product_id, product_name, unit_label, quantity, unit_price_cents, unit_deposit_cents) VALUES ($1, $2, $3, $4, $5, $6, $7)")
            .bind(&order_id)
            .bind(product_id)
            .bind(name)
            .bind(unit_label)
            .bind(quantity)
            .bind(unit_price)
            .bind(unit_deposit)
            .execute(&mut *tx)
            .await
            .is_err()
            || sqlx::query("UPDATE commerce_batch_products SET sold_quantity = sold_quantity + $1 WHERE batch_id = $2 AND product_id = $3")
                .bind(quantity)
                .bind(payload.batch_id.trim())
                .bind(product_id)
                .execute(&mut *tx)
                .await
                .is_err()
        {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "保存订单商品失败");
        }
    }
    if sqlx::query("INSERT INTO commerce_order_status_logs (order_id, status, note, actor_username, created_at) VALUES ($1, 'pending_payment', '订单创建', $2, $3)")
        .bind(&order_id)
        .bind(&username)
        .bind(now)
        .execute(&mut *tx)
        .await
        .is_err()
        || tx.commit().await.is_err()
    {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "保存订单状态失败");
    }

    api_success(json!({
        "order_id": order_id,
        "order_no": order_no,
        "status": "pending_payment",
        "payment_status": "unpaid",
        "total_cents": total_cents,
        "deposit_cents": deposit_cents
    }))
}

pub(crate) async fn list_user_orders_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let (_, username) = match ensure_authenticated(&state, &headers).await {
        Ok(value) => value,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    let rows = match sqlx::query("SELECT id, order_no, username, batch_id, recipient_name, recipient_phone, shipping_address, payment_status, status, total_cents, deposit_cents, created_at, updated_at FROM commerce_orders WHERE username = $1 ORDER BY created_at DESC")
        .bind(&username)
        .fetch_all(&state.db)
        .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!("查询用户订单失败: {}", error);
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "查询订单失败");
        }
    };
    let mut orders = Vec::with_capacity(rows.len());
    for row in rows {
        let order_id = row.try_get::<String, _>("id").unwrap_or_default();
        let items = match load_order_items(&state.db, &order_id).await {
            Ok(items) => items,
            Err(_) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "查询订单商品失败");
            }
        };
        orders.push(order_payload(&row, items));
    }
    api_success(json!({ "orders": orders }))
}

pub(crate) async fn get_user_order_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
) -> Response {
    let (_, username) = match ensure_authenticated(&state, &headers).await {
        Ok(value) => value,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    let row = match sqlx::query("SELECT id, order_no, username, batch_id, recipient_name, recipient_phone, shipping_address, payment_status, status, total_cents, deposit_cents, created_at, updated_at FROM commerce_orders WHERE id = $1 AND username = $2 LIMIT 1")
        .bind(&order_id)
        .bind(&username)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, 404, "订单不存在"),
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "查询订单失败"),
    };
    let items = match load_order_items(&state.db, &order_id).await {
        Ok(items) => items,
        Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "查询订单商品失败");
        }
    };
    api_success(json!({ "order": order_payload(&row, items) }))
}

pub(crate) async fn list_admin_orders_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }
    let rows = match sqlx::query("SELECT id, order_no, username, batch_id, recipient_name, recipient_phone, shipping_address, payment_status, status, total_cents, deposit_cents, created_at, updated_at FROM commerce_orders ORDER BY created_at DESC")
        .fetch_all(&state.db)
        .await
    {
        Ok(rows) => rows,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "查询订单失败"),
    };
    let mut orders = Vec::with_capacity(rows.len());
    for row in rows {
        let order_id = row.try_get::<String, _>("id").unwrap_or_default();
        let items = match load_order_items(&state.db, &order_id).await {
            Ok(items) => items,
            Err(_) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "查询订单商品失败");
            }
        };
        orders.push(order_payload(&row, items));
    }
    api_success(json!({ "orders": orders }))
}

pub(crate) async fn update_order_status_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<UpdateOrderStatusRequest>,
) -> Response {
    let admin = match ensure_admin(&state, &headers).await {
        Ok(name) => name,
        Err((code, body)) => return (code, Json(body)).into_response(),
    };
    let status = payload.status.trim();
    if !valid_status(status, ORDER_STATUSES)
        || payload
            .payment_status
            .as_deref()
            .is_some_and(|value| !valid_status(value.trim(), PAYMENT_STATUSES))
    {
        return error_response(StatusCode::BAD_REQUEST, 400, "订单状态或收款状态无效");
    }
    let mut tx: Transaction<'_, Postgres> = match state.db.begin().await {
        Ok(tx) => tx,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "更新订单失败"),
    };
    let row = match sqlx::query(
        "SELECT status, payment_status, batch_id FROM commerce_orders WHERE id = $1 FOR UPDATE",
    )
    .bind(payload.order_id.trim())
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, 404, "订单不存在"),
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "读取订单失败"),
    };
    let old_status = row.try_get::<String, _>("status").unwrap_or_default();
    let old_payment = row
        .try_get::<String, _>("payment_status")
        .unwrap_or_default();
    let batch_id = row.try_get::<String, _>("batch_id").unwrap_or_default();
    if matches!(old_status.as_str(), "cancelled" | "refunded") && old_status != status {
        return error_response(StatusCode::CONFLICT, 409, "已取消或已退款订单不能恢复");
    }

    let old_closed = matches!(old_status.as_str(), "cancelled" | "refunded");
    let new_closed = matches!(status, "cancelled" | "refunded");
    if new_closed && !old_closed {
        let items = match sqlx::query(
            "SELECT product_id, quantity FROM commerce_order_items WHERE order_id = $1",
        )
        .bind(payload.order_id.trim())
        .fetch_all(&mut *tx)
        .await
        {
            Ok(items) => items,
            Err(_) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "读取订单商品失败");
            }
        };
        for item in items {
            if sqlx::query("UPDATE commerce_batch_products SET sold_quantity = GREATEST(0, sold_quantity - $1) WHERE batch_id = $2 AND product_id = $3")
                .bind(item.try_get::<i32, _>("quantity").unwrap_or_default())
                .bind(&batch_id)
                .bind(item.try_get::<i64, _>("product_id").unwrap_or_default())
                .execute(&mut *tx)
                .await
                .is_err()
            {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "释放批次库存失败");
            }
        }
    }

    let payment_status = payload.payment_status.as_deref().unwrap_or(&old_payment);
    let now = now_millis() as i64;
    if sqlx::query("UPDATE commerce_orders SET status = $1, payment_status = $2, updated_at = $3 WHERE id = $4")
        .bind(status)
        .bind(payment_status)
        .bind(now)
        .bind(payload.order_id.trim())
        .execute(&mut *tx)
        .await
        .is_err()
        || sqlx::query("INSERT INTO commerce_order_status_logs (order_id, status, note, actor_username, created_at) VALUES ($1, $2, $3, $4, $5)")
            .bind(payload.order_id.trim())
            .bind(status)
            .bind(payload.note.trim())
            .bind(&admin)
            .bind(now)
            .execute(&mut *tx)
            .await
            .is_err()
        || tx.commit().await.is_err()
    {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "保存订单状态失败");
    }
    let _ = append_audit_log(
        &state.db,
        "commerce",
        Some(&admin),
        &format!(
            "管理员 {} 将订单 {} 状态调整为 {}",
            admin, payload.order_id, status
        ),
    )
    .await;
    api_success(
        json!({ "order_id": payload.order_id, "status": status, "payment_status": payment_status }),
    )
}

pub(crate) async fn commerce_overview_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, body)) = ensure_admin(&state, &headers).await {
        return (code, Json(body)).into_response();
    }
    let row = match sqlx::query(
        r#"SELECT
            COUNT(*) AS order_count,
            COALESCE(SUM(total_cents), 0)::BIGINT AS gross_amount_cents,
            COUNT(*) FILTER (WHERE status IN ('pending_payment', 'paid', 'confirmed', 'harvesting', 'packing', 'shipped')) AS active_order_count,
            COUNT(*) FILTER (WHERE status = 'completed') AS completed_order_count,
            COUNT(*) FILTER (WHERE status IN ('cancelled', 'refunded')) AS cancelled_order_count
        FROM commerce_orders"#,
    )
    .fetch_one(&state.db)
    .await
    {
        Ok(row) => row,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, "查询商业统计失败"),
    };
    let batch_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM commerce_batches WHERE is_active = TRUE",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or_default();
    api_success(json!({
        "orders": {
            "count": row.try_get::<i64, _>("order_count").unwrap_or_default(),
            "gross_amount_cents": row.try_get::<i64, _>("gross_amount_cents").unwrap_or_default(),
            "active_count": row.try_get::<i64, _>("active_order_count").unwrap_or_default(),
            "completed_count": row.try_get::<i64, _>("completed_order_count").unwrap_or_default(),
            "cancelled_count": row.try_get::<i64, _>("cancelled_order_count").unwrap_or_default()
        },
        "active_batch_count": batch_count
    }))
}
