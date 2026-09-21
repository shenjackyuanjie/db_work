//! 网页商城运营超集：封面 multipart 上传 + 后台商城概览 / 订单管理 / 分析。
//!
//! 负责的 `/web/*` 路径（外层已 `nest("/web")`）：
//!
//! | 路径 | 方法 | 旧路径 | 前端引用 |
//! |---|---|---|---|
//! | `/admin/store/products/{id}/cover` | POST | `/user/admin/store/products/{id}/cover` | `admin.js:1650`、`store-admin.js:327` |
//! | `/admin/store/analytics` | GET | `/user/admin/store/analytics` | `store-admin.js:79` |
//! | `/admin/store/overview` | POST | `/user/admin/store/overview` | `admin.js:1505`、`store-admin.js` |
//! | `/admin/store/orders` | POST | `/user/admin/store/orders` | `admin.js:1726`、`store-admin.js:179` |
//! | `/admin/store/orders/status` | POST | `/user/admin/store/orders/status` | `admin.js:1733`、`store-admin.js:348` |
//!
//! ## 表：改读写 Django 契约表
//!
//! 数据源从自研的 `store_products` / `store_orders` / `store_order_items` 换成契约表
//! `citrus_product` / `"order"` / `order_item`（`W2_PLAN.md` §3、§4）。
//!
//! ## 响应形状：保留前端现有字段名（**有意的兼容层**）
//!
//! 前端读的是 `price_cents` / `stock_quantity` / `is_active` / `order_no` / `total_cents`
//! 这类**分制与自研命名**的字段（`admin.js:1700-1709`、`store-admin.js:79-96`）。
//! 任务书给的取向是「规划没写死就选前端少改」，所以本层**同时**输出：
//!
//! - **Django 原生字段**：`price`（Decimal 字符串）、`stock`、`status`、`order_number`、`total_amount`
//! - **派生兼容别名**：`price_cents`、`stock_quantity`、`is_active`、`order_no`、`total_cents`
//!
//! 别名一律由原生字段派生（只读视图，不存库），S5 收敛前端后可整体删除。
//! 这样 S4 不必为了后台五个页面同时改前端与后端，风险面小得多。
//!
//! ## 两个必须守住的约束
//!
//! 1. **封面上传写入 `citrus_product.cover_image_url`，值必须保持 `/store-images/<uuid>.<ext>` 前缀** ——
//!    前端 `coverImageSrc`（`store.js:61`）只认 `/store-images/`、`/uploads/`、`http(s)://`，
//!    而这三条是**隐式静态通道**（没有任何 `fetch`，靠数据里的路径串 + `<img src>` 生效），
//!    写错前缀 App 与网页会同时 404。
//! 2. **`"order"` 与 `"user"` 是 PG 保留字，SQL 里必须双引号。**

use axum::{
    Json, Router,
    extract::{Multipart, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::server::{AppState, save_store_cover_image};
use crate::system_settings::append_audit_log;

use super::session;

/// 与 `user_routes/store.rs` 保持一致的上传上限（前端 `admin.js:1633-1646` 也各校验一次）。
const MAX_STORE_COVER_BYTES: usize = 8 * 1024 * 1024;
const MAX_STORE_COVER_DIMENSION: u32 = 4096;

/// Django `Order.Status` 的全部取值（`navel_backend_git/api/models.py`）。
/// 契约表的状态机是这 10 个，后台只能在这 10 个之间流转。
const ORDER_STATUSES: &[&str] = &[
    "pending_payment",
    "pending_deposit",
    "pending_balance",
    "paid",
    "picking",
    "packed",
    "shipped",
    "completed",
    "after_sale",
    "cancelled",
];

/// 「已收款」状态集合：成交额只统计这些。
const REVENUE_STATUSES: &[&str] = &["paid", "shipped", "completed"];

/// 「未完结」状态集合：后台概览的待处理订单数。
const PENDING_STATUSES: &[&str] = &[
    "pending_payment",
    "pending_deposit",
    "pending_balance",
    "paid",
];

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        // 商品 CRUD：**后台管理端口径**，跨 seller 读写 `citrus_product`。
        // 不能改打 `/api/v1/farmer/products` —— 那个是 `IsFarmer` 且按 `seller_id` 自我隔离，
        // 而 `is_admin` 与 `role` 正交（管理员可能是 buyer），详见执行记录里的裁定。
        .route(
            "/admin/store/products",
            get(products_list_api).post(product_create_api),
        )
        .route(
            "/admin/store/products/{product_id}",
            get(product_detail_api)
                .patch(product_update_api)
                .put(product_update_api)
                .delete(product_delete_api),
        )
        .route(
            "/admin/store/products/{product_id}/toggle",
            post(product_toggle_api),
        )
        .route(
            "/admin/store/products/{product_id}/cover",
            post(upload_cover_api),
        )
        .route("/admin/store/analytics", get(analytics_api))
        .route("/admin/store/overview", post(overview_api))
        .route("/admin/store/orders", post(orders_api))
        .route("/admin/store/orders/status", post(order_status_api))
}

// --------------------------------------------------------------------------------------
// 纯函数：金额与状态映射（可单测，不碰数据库）
// --------------------------------------------------------------------------------------

/// `NUMERIC(12,2)` → 前端使用的「分」。
///
/// `citrus_product.price` / `order.total_amount` / `order_item.unit_price` 都是
/// `NUMERIC`，而前端一律按「分」做算术与展示（`storeMoney`），所以必须在这里换算。
fn amount_to_cents(amount: Decimal) -> i64 {
    (amount * Decimal::from(100)).round().to_i64().unwrap_or(0)
}

/// `citrus_product.status` → 前端使用的 `is_active`（`draft` / `on_sale` / `off_sale`）。
///
/// 前端只认布尔量，所以商品载荷里把它派生成 `is_active`（只读、不存库）。
fn product_is_active(status: &str) -> bool {
    status == "on_sale"
}

fn valid_order_status(status: &str) -> bool {
    ORDER_STATUSES.contains(&status)
}

/// `sqlx` 对 `text[]` 参数只实现了 `Vec<String>` 的编码，`&[&str]` 传不进去，
/// 所以统一在这里转一次。
fn status_list(list: &[&str]) -> Vec<String> {
    list.iter().map(|value| (*value).to_string()).collect()
}

/// 失败体一律走超集信封（`session::app_*`），不要另造一套——
/// 网页前端 `postJson → resp.data → unwrapApiPayload` 依赖这个形状。
fn error_response(status: StatusCode, message: &str) -> Response {
    session::app_err(status, message, Value::Null)
}

fn internal_error(message: &str) -> Response {
    error_response(StatusCode::INTERNAL_SERVER_ERROR, message)
}

// --------------------------------------------------------------------------------------
// 封面 multipart 上传
// --------------------------------------------------------------------------------------

async fn upload_cover_api(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(product_id): axum::extract::Path<String>,
    multipart: Multipart,
) -> Response {
    let admin = match session::require_admin(&state, &headers).await {
        Ok(user) => user.username,
        Err(response) => return response,
    };

    let product_id = match Uuid::parse_str(product_id.trim()) {
        Ok(value) => value,
        // 契约表是 UUID 主键，非法 uuid 直接 404（不泄露「格式错」还是「不存在」）
        Err(_) => return error_response(StatusCode::NOT_FOUND, "商品不存在"),
    };

    match upload_cover_impl(&state.db, product_id, multipart).await {
        Ok(cover_image) => {
            let _ = append_audit_log(
                &state.db,
                "action",
                Some(&admin),
                &format!("更新商城商品封面: id={product_id}"),
            )
            .await;
            session::app_ok(
                "success",
                json!({
                    "id": product_id.to_string(),
                    "cover_image": cover_image,
                    "cover_image_url": cover_image,
                }),
            )
        }
        Err(reject) => reject.into_response(),
    }
}

/// 封面写入的失败形态：要么是客户端错误，要么是数据库错误，分开表达以便给准确状态码。
enum CoverReject {
    Client(StatusCode, String),
    Db(sqlx::Error),
}

impl IntoResponse for CoverReject {
    fn into_response(self) -> Response {
        match self {
            CoverReject::Client(status, message) => error_response(status, &message),
            // 具体 DB 错误只进日志，不回给前端（避免泄露表结构），但**不能**静默丢弃。
            CoverReject::Db(error) => {
                tracing::error!("更新商品封面失败: {error}");
                internal_error("更新商品封面失败")
            }
        }
    }
}

async fn upload_cover_impl(
    pool: &PgPool,
    product_id: Uuid,
    mut multipart: Multipart,
) -> Result<String, CoverReject> {
    let exists =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM citrus_product WHERE id = $1)")
            .bind(product_id)
            .fetch_one(pool)
            .await
            .map_err(CoverReject::Db)?;
    if !exists {
        return Err(CoverReject::Client(
            StatusCode::NOT_FOUND,
            "商品不存在".to_string(),
        ));
    }

    let mut cover_image: Option<String> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                return Err(CoverReject::Client(
                    StatusCode::BAD_REQUEST,
                    format!("解析上传失败: {error}"),
                ));
            }
        };
        if !field
            .name()
            .map(|name| matches!(name, "image" | "file"))
            .unwrap_or(false)
        {
            continue;
        }

        let mime_type = field
            .content_type()
            .map(str::to_string)
            .unwrap_or_else(|| "image/jpeg".to_string());
        let bytes = match field.bytes().await {
            Ok(bytes) => bytes,
            Err(_) => {
                return Err(CoverReject::Client(
                    StatusCode::BAD_REQUEST,
                    "读取图片失败".to_string(),
                ));
            }
        };
        if bytes.len() > MAX_STORE_COVER_BYTES {
            return Err(CoverReject::Client(
                StatusCode::BAD_REQUEST,
                "封面图片过大，最大 8MB".to_string(),
            ));
        }
        match image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|_| ())
            .and_then(|reader| reader.into_dimensions().map_err(|_| ()))
        {
            Ok((width, height)) => {
                if width > MAX_STORE_COVER_DIMENSION || height > MAX_STORE_COVER_DIMENSION {
                    return Err(CoverReject::Client(
                        StatusCode::BAD_REQUEST,
                        "封面图片尺寸过大，最长边不可超过 4096px".to_string(),
                    ));
                }
            }
            Err(_) => {
                return Err(CoverReject::Client(
                    StatusCode::BAD_REQUEST,
                    "封面图片无法解析，请上传有效的图片".to_string(),
                ));
            }
        }
        // 返回值形如 `/store-images/<uuid>.jpg`，必须原样落库（隐式静态通道）。
        cover_image =
            Some(save_store_cover_image(&mime_type, &bytes).map_err(|error| {
                CoverReject::Client(StatusCode::BAD_REQUEST, error.to_string())
            })?);
    }

    let cover_image = match cover_image {
        Some(path) => path,
        None => {
            return Err(CoverReject::Client(
                StatusCode::BAD_REQUEST,
                "缺少封面图片字段".to_string(),
            ));
        }
    };

    let affected = sqlx::query(
        "UPDATE citrus_product SET cover_image_url = $1, updated_at = $2 WHERE id = $3",
    )
    .bind(&cover_image)
    .bind(Utc::now())
    .bind(product_id)
    .execute(pool)
    .await
    .map_err(CoverReject::Db)?
    .rows_affected();
    if affected == 0 {
        return Err(CoverReject::Client(
            StatusCode::NOT_FOUND,
            "商品不存在".to_string(),
        ));
    }

    Ok(cover_image)
}

// --------------------------------------------------------------------------------------
// 订单列表 / 状态流转
// --------------------------------------------------------------------------------------

async fn orders_api(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = session::require_admin(&state, &headers).await {
        return response;
    }

    match orders_impl(&state.db).await {
        Ok(orders) => session::app_ok("success", json!({ "orders": orders })),
        Err(_) => internal_error("读取订单失败"),
    }
}

async fn orders_impl(pool: &PgPool) -> Result<Vec<Value>, sqlx::Error> {
    let rows = sqlx::query(
        r#"SELECT o.id, o.order_number, o.status, o.recipient_name, o.recipient_phone,
                  o.shipping_address, o.total_amount, o.created_at, u.username
           FROM "order" o
           LEFT JOIN "user" u ON u.id = o.buyer_id
           ORDER BY o.created_at DESC"#,
    )
    .fetch_all(pool)
    .await?;

    let mut orders = Vec::with_capacity(rows.len());
    for row in rows {
        let order_id = row.try_get::<Uuid, _>("id").unwrap_or_else(|_| Uuid::nil());
        let items = order_items_impl(pool, order_id).await?;
        let total_amount = row
            .try_get::<Decimal, _>("total_amount")
            .unwrap_or_else(|_| Decimal::ZERO);
        let created_at = row.try_get::<DateTime<Utc>, _>("created_at").ok();

        orders.push(json!({
            "id": order_id.to_string(),
            // Django 原生
            "order_number": row.try_get::<String, _>("order_number").unwrap_or_default(),
            "status": row.try_get::<String, _>("status").unwrap_or_default(),
            "total_amount": total_amount.to_string(),
            "created_at": created_at.map(|value| value.timestamp_millis()),
            // 前端兼容别名（派生，不存库）
            "order_no": row.try_get::<String, _>("order_number").unwrap_or_default(),
            "total_cents": amount_to_cents(total_amount),
            "username": row.try_get::<Option<String>, _>("username").ok().flatten(),
            "recipient_name": row.try_get::<String, _>("recipient_name").unwrap_or_default(),
            "recipient_phone": row.try_get::<String, _>("recipient_phone").unwrap_or_default(),
            "shipping_address": row.try_get::<String, _>("shipping_address").unwrap_or_default(),
            "items": items,
        }));
    }

    Ok(orders)
}

async fn order_items_impl(pool: &PgPool, order_id: Uuid) -> Result<Vec<Value>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT product_name, unit, quantity, unit_price FROM order_item WHERE order_id = $1 ORDER BY id ASC",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let quantity = row.try_get::<i32, _>("quantity").unwrap_or_default();
            let unit_price = row
                .try_get::<Decimal, _>("unit_price")
                .unwrap_or_else(|_| Decimal::ZERO);
            let unit_price_cents = amount_to_cents(unit_price);
            json!({
                "product_name": row.try_get::<String, _>("product_name").unwrap_or_default(),
                "quantity": quantity,
                // 前端读 unit_label / unit_price_cents / line_total_cents（admin.js:1703）
                "unit": row.try_get::<String, _>("unit").unwrap_or_default(),
                "unit_label": row.try_get::<String, _>("unit").unwrap_or_default(),
                "unit_price": unit_price.to_string(),
                "unit_price_cents": unit_price_cents,
                "line_total_cents": unit_price_cents * quantity as i64,
            })
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct UpdateOrderStatusRequest {
    order_id: Option<String>,
    status: Option<String>,
    note: Option<String>,
}

async fn order_status_api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<UpdateOrderStatusRequest>,
) -> Response {
    let admin = match session::require_admin(&state, &headers).await {
        Ok(user) => user.username,
        Err(response) => return response,
    };

    let raw_id = payload.order_id.unwrap_or_default();
    let status = payload.status.unwrap_or_default();
    if !valid_order_status(&status) {
        return error_response(StatusCode::BAD_REQUEST, "订单状态参数无效");
    }
    let order_id = match Uuid::parse_str(raw_id.trim()) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::NOT_FOUND, "订单不存在"),
    };
    let note = payload.note.unwrap_or_default();

    let changed = match update_order_status_impl(&state.db, order_id, &status).await {
        Ok(changed) => changed,
        Err(_) => return internal_error("更新订单状态失败"),
    };
    if !changed {
        return error_response(StatusCode::NOT_FOUND, "订单不存在");
    }

    let note_suffix = if note.trim().is_empty() {
        String::new()
    } else {
        format!("，备注：{}", note.trim())
    };
    let _ = append_audit_log(
        &state.db,
        "action",
        Some(&admin),
        &format!("更新商城订单 {order_id} 状态为 {status}{note_suffix}"),
    )
    .await;

    session::app_ok(
        "success",
        json!({
            "order_id": order_id.to_string(),
            "status": status,
        }),
    )
}

async fn update_order_status_impl(
    pool: &PgPool,
    order_id: Uuid,
    status: &str,
) -> Result<bool, sqlx::Error> {
    let affected = sqlx::query(r#"UPDATE "order" SET status = $1, updated_at = $2 WHERE id = $3"#)
        .bind(status)
        .bind(Utc::now())
        .bind(order_id)
        .execute(pool)
        .await?
        .rows_affected();

    Ok(affected > 0)
}

// --------------------------------------------------------------------------------------
// 商城概览
// --------------------------------------------------------------------------------------

async fn overview_api(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = session::require_admin(&state, &headers).await {
        return response;
    }

    match overview_impl(&state.db).await {
        Ok(payload) => session::app_ok("success", payload),
        Err(_) => internal_error("读取商城概览失败"),
    }
}

async fn overview_impl(pool: &PgPool) -> Result<Value, sqlx::Error> {
    let product_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM citrus_product WHERE status = 'on_sale'",
    )
    .fetch_one(pool)
    .await?;
    let order_count = sqlx::query_scalar::<_, i64>(r#"SELECT COUNT(*) FROM "order""#)
        .fetch_one(pool)
        .await?;
    let gross_amount_cents = sqlx::query_scalar::<_, i64>(
        r#"SELECT COALESCE(SUM(ROUND(total_amount * 100))::BIGINT, 0) FROM "order"
           WHERE status NOT IN ('cancelled', 'after_sale')"#,
    )
    .fetch_one(pool)
    .await?;
    let pending_order_count = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM "order"
           WHERE status = ANY($1::text[])"#,
    )
    .bind(status_list(PENDING_STATUSES))
    .fetch_one(pool)
    .await?;

    Ok(json!({
        "product_count": product_count,
        "order_count": order_count,
        "gross_amount_cents": gross_amount_cents,
        "pending_order_count": pending_order_count,
    }))
}

// --------------------------------------------------------------------------------------
// 商城分析
// --------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AnalyticsQuery {
    days: Option<i32>,
}

async fn analytics_api(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<AnalyticsQuery>,
) -> Response {
    if let Err(response) = session::require_admin(&state, &headers).await {
        return response;
    }

    let days = query.days.unwrap_or(7).clamp(1, 90);
    match analytics_impl(&state.db, days).await {
        // 旧实现在这里返回**裸 JSON**（`store_workspace::analytics` 直接 `Json(value)`），
        // 前端 `request()` 对裸体与信封都容忍，但 `/web/*` 统一成套信封更省心。
        Ok(payload) => session::app_ok("success", payload),
        Err(error) => {
            tracing::error!("读取商城统计失败: {error}");
            internal_error("读取商城统计失败")
        }
    }
}

/// 原实现是 `store_orders` 上的 epoch-millis 算术；契约表 `created_at` 已是 `TIMESTAMPTZ`，
/// 所以这里改成直接按日期切分，逻辑等价但少一层换算。
/// 金额仍输出「分」（前端 `storeMoney`）。
async fn analytics_impl(pool: &PgPool, days: i32) -> Result<Value, sqlx::Error> {
    let sql = r#"
WITH selected AS (
    SELECT o.id,
           o.status,
           o.total_amount,
           u.username,
           (o.created_at AT TIME ZONE 'Asia/Shanghai')::date AS day
    FROM "order" o
    LEFT JOIN "user" u ON u.id = o.buyer_id
    WHERE (o.created_at AT TIME ZONE 'Asia/Shanghai')::date
          >= (now() AT TIME ZONE 'Asia/Shanghai')::date - ($1::int - 1)
)
SELECT json_build_object(
    'summary', (
        SELECT json_build_object(
            'orders', count(*),
            'revenue', COALESCE(sum(ROUND(total_amount * 100)) FILTER (WHERE status = ANY($2::text[])), 0),
            'buyers', count(DISTINCT username),
            'pending', count(*) FILTER (WHERE status = ANY($3::text[]))
        ) FROM selected
    ),
    'trend', (
        SELECT json_agg(t ORDER BY t.day) FROM (
            SELECT d::date::text AS day,
                   COALESCE(sum(ROUND(s.total_amount * 100)) FILTER (WHERE s.status = ANY($2::text[])), 0) AS revenue,
                   count(s.id) AS orders
            FROM generate_series(
                     (now() AT TIME ZONE 'Asia/Shanghai')::date - ($1::int - 1),
                     (now() AT TIME ZONE 'Asia/Shanghai')::date,
                     interval '1 day'
                 ) d
            LEFT JOIN selected s ON s.day = d::date
            GROUP BY d
        ) t
    ),
    'statuses', (
        SELECT COALESCE(json_agg(t), '[]'::json) FROM (
            SELECT status, count(*) AS count FROM selected GROUP BY status
        ) t
    ),
    'products', (
        SELECT COALESCE(json_agg(t), '[]'::json) FROM (
            SELECT i.product_id, i.product_name,
                   sum(i.quantity) AS quantity,
                   sum(ROUND(i.unit_price * i.quantity * 100)) AS revenue
            FROM order_item i
            JOIN selected s ON s.id = i.order_id
            WHERE s.status = ANY($2::text[])
            GROUP BY i.product_id, i.product_name
            ORDER BY revenue DESC
            LIMIT 8
        ) t
    )
)::text"#;

    let raw = sqlx::query_scalar::<_, String>(sql)
        .bind(days)
        .bind(status_list(REVENUE_STATUSES))
        .bind(status_list(PENDING_STATUSES))
        .fetch_one(pool)
        .await?;

    serde_json::from_str(&raw).map_err(|error| sqlx::Error::Decode(Box::new(error)))
}

// --------------------------------------------------------------------------------------
// 商品 CRUD（后台管理端口径）
// --------------------------------------------------------------------------------------
//
// **为什么不能改打 `/api/v1/farmer/products`**（`W2_PLAN.md` §2.3 原方案，已被推翻）：
// 夹具 `farmer_products_buyer_403` / `farmer_product_put_buyer_403` 证明 `/api/v1/farmer/*`
// 是 `IsFarmer`（buyer 一律 403 `该接口仅限果农使用`），`farmer_product_patch_foreign_404`
// 又证明它按 `seller_id` 自我隔离（只能管自己的商品）。而 `is_admin` 是 `"user"` 的加法列、
// 与 `role` **正交**，管理员完全可能是 `role=buyer` —— 走农户端接口要么 403，要么只能看到
// 自己名下的商品，后台失去意义。所以这里保留管理端超集：**跨 seller 直接读写 `citrus_product`**。

const PRODUCT_STATUSES: &[&str] = &["draft", "on_sale", "off_sale"];

/// 新建商品的默认状态。
///
/// **有意偏离 Django 模型的默认值 `draft`**：旧后台的商品表单里没有状态字段，而旧模型
/// `store_products.is_active` 默认 TRUE（建了即上架）；且 `/api/products` 只返回 `on_sale`。
/// 若沿用 `draft`，管理员建完商品在商城页看不到，必然被当成 bug。要下架用 toggle 或显式传 `status`。
const DEFAULT_PRODUCT_STATUS: &str = "on_sale";

fn valid_product_status(status: &str) -> bool {
    PRODUCT_STATUSES.contains(&status)
}

fn product_status_display(status: &str) -> &'static str {
    match status {
        "on_sale" => "在售",
        "off_sale" => "已下架",
        _ => "待上架",
    }
}

/// 从 JSON 里按候选键取非空字符串（候选键用于兼容旧前端字段名）。
fn body_text(body: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| body.get(*key))
        .and_then(|value| match value {
            Value::String(text) => Some(text.trim().to_string()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
}

fn body_i64(body: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| body.get(*key))
        .and_then(|value| match value {
            Value::Number(number) => number
                .as_i64()
                .or_else(|| number.as_f64().map(|f| f as i64)),
            Value::String(text) => text.trim().parse::<i64>().ok(),
            _ => None,
        })
}

fn decimal_from_value(value: &Value) -> Option<Decimal> {
    match value {
        Value::String(text) => text.trim().parse::<Decimal>().ok(),
        Value::Number(number) => number.to_string().parse::<Decimal>().ok(),
        _ => None,
    }
}

/// 价格：优先 Django 原生 `price`，退化到旧前端的 `price_cents`（**分 → 元**）。
///
/// 旧前端两种提交形态都有：`admin.js:1578` / `store-admin.js:272` 传的是 `price_cents`（分）。
fn body_price(body: &Value) -> Option<Decimal> {
    if let Some(value) = body.get("price")
        && let Some(parsed) = decimal_from_value(value)
    {
        return Some(parsed);
    }
    body.get("price_cents")
        .and_then(decimal_from_value)
        .map(|cents| cents / Decimal::from(100))
}

/// 商品载荷：Django 原生字段 + **只读派生兼容别名**。
///
/// 别名存在的唯一理由：前端 `admin.js:1492` 与 `store-admin.js` 的 `money()` 是
/// `Number(x) / 100`（按「分」做算术），而契约的 `price` 是 `NUMERIC` **字符串**。
/// 不镜像的话，一个 168 元的商品会被显示成 **¥1.68**。
/// 别名一律由原生字段派生（**只读、不存库**），S5 收敛前端后可整体删除。
fn product_payload(row: &sqlx::postgres::PgRow) -> Value {
    let price = row
        .try_get::<Decimal, _>("price")
        .unwrap_or_else(|_| Decimal::ZERO);
    let stock = row.try_get::<i32, _>("stock").unwrap_or(0);
    let unit = row.try_get::<String, _>("unit").unwrap_or_default();
    let cover = row
        .try_get::<String, _>("cover_image_url")
        .unwrap_or_default();
    let status = row.try_get::<String, _>("status").unwrap_or_default();
    let created_at = row.try_get::<DateTime<Utc>, _>("created_at").ok();
    let updated_at = row.try_get::<DateTime<Utc>, _>("updated_at").ok();
    let harvest_date = row
        .try_get::<Option<chrono::NaiveDate>, _>("harvest_date")
        .ok()
        .flatten()
        .map(|value| value.format("%Y-%m-%d").to_string());

    json!({
        "id": row.try_get::<Uuid, _>("id").map(|value| value.to_string()).unwrap_or_default(),
        // ---- Django 原生 ----
        "name": row.try_get::<String, _>("name").unwrap_or_default(),
        "sku_type": row.try_get::<String, _>("sku_type").unwrap_or_default(),
        "fruit_type": row.try_get::<String, _>("fruit_type").unwrap_or_default(),
        "variety": row.try_get::<String, _>("variety").unwrap_or_default(),
        "origin": row.try_get::<String, _>("origin").unwrap_or_default(),
        "description": row.try_get::<String, _>("description").unwrap_or_default(),
        "price": price.to_string(),
        "unit": unit.clone(),
        "stock": stock,
        "sweetness": row.try_get::<Option<Decimal>, _>("sweetness").ok().flatten().map(|v| v.to_string()),
        "grade": row.try_get::<String, _>("grade").unwrap_or_default(),
        "harvest_date": harvest_date,
        "shipping_note": row.try_get::<String, _>("shipping_note").unwrap_or_default(),
        "cover_image_url": cover.clone(),
        "purchase_limit": row.try_get::<i32, _>("purchase_limit").unwrap_or(0),
        "minimum_order_quantity": row.try_get::<i32, _>("minimum_order_quantity").unwrap_or(1),
        "sort_order": row.try_get::<i32, _>("sort_order").unwrap_or(0),
        "status": status.clone(),
        "status_display": product_status_display(&status),
        "sales_batch_id": row.try_get::<Option<Uuid>, _>("sales_batch_id").ok().flatten().map(|v| v.to_string()),
        "sales_batch_code": row.try_get::<Option<String>, _>("batch_code").ok().flatten(),
        "sales_batch_title": row.try_get::<Option<String>, _>("batch_title").ok().flatten(),
        "seller_id": row.try_get::<Option<Uuid>, _>("seller_id").ok().flatten().map(|v| v.to_string()),
        "seller_name": row.try_get::<Option<String>, _>("seller_name").ok().flatten(),
        "created_at": created_at.map(|value| value.timestamp_millis()),
        "updated_at": updated_at.map(|value| value.timestamp_millis()),
        // ---- 只读派生兼容别名（S5 可删）----
        "price_cents": amount_to_cents(price),
        "stock_quantity": stock,
        "unit_label": unit,
        "cover_image": cover,
        "is_active": product_is_active(&status),
    })
}

/// 商品的统一查询：带上批次与人，供列表与详情共用。
const PRODUCT_SELECT: &str = r#"SELECT p.*, b.code AS batch_code, b.title AS batch_title,
                                        u.username AS seller_name
                                 FROM citrus_product p
                                 LEFT JOIN sales_batch b ON b.id = p.sales_batch_id
                                 LEFT JOIN "user" u ON u.id = p.seller_id"#;

/// 批次上下文：校验批次存在，并顺带推出 `seller_id`（果园 owner）与 `origin`。
struct BatchContext {
    seller_id: Option<Uuid>,
    origin: String,
}

/// `sales_batch_id` 是必填的 `RESTRICT` 外键：不存在时**必须给 400**，
/// 不能让外键违例冒成 500。
async fn lookup_batch(pool: &PgPool, batch_id: Uuid) -> Result<Option<BatchContext>, sqlx::Error> {
    let row = sqlx::query(
        r#"SELECT o.owner_id,
                  concat_ws('', o.province, o.city, o.county) AS origin
           FROM sales_batch b
           LEFT JOIN orchard o ON o.id = b.orchard_id
           WHERE b.id = $1"#,
    )
    .bind(batch_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| BatchContext {
        seller_id: row.try_get::<Option<Uuid>, _>("owner_id").ok().flatten(),
        origin: row
            .try_get::<String, _>("origin")
            .unwrap_or_else(|_| "江西省赣州市".to_string()),
    }))
}

async fn fetch_product(pool: &PgPool, product_id: Uuid) -> Result<Option<Value>, sqlx::Error> {
    let sql = format!("{PRODUCT_SELECT} WHERE p.id = $1");
    let row = sqlx::query(&sql)
        .bind(product_id)
        .fetch_optional(pool)
        .await?;

    Ok(row.as_ref().map(product_payload))
}

/// 商品 CRUD 的失败形态：客户端错误给准确状态码，DB 错误只进日志。
enum ProductReject {
    Client(StatusCode, String),
    Db(sqlx::Error),
}

impl ProductReject {
    fn bad_request(message: impl Into<String>) -> Self {
        Self::Client(StatusCode::BAD_REQUEST, message.into())
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::Client(StatusCode::NOT_FOUND, message.into())
    }
}

impl IntoResponse for ProductReject {
    fn into_response(self) -> Response {
        match self {
            ProductReject::Client(status, message) => error_response(status, &message),
            ProductReject::Db(error) => {
                tracing::error!("商品操作失败: {error}");
                internal_error("商品操作失败")
            }
        }
    }
}

// ---- 处理器：薄壳（鉴权 + 取参），业务在 *_impl 里（便于单测） ----

async fn products_list_api(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = session::require_admin(&state, &headers).await {
        return response;
    }
    match products_list_impl(&state.db).await {
        Ok(products) => session::app_ok("success", json!({ "products": products })),
        Err(error) => {
            tracing::error!("读取商品列表失败: {error}");
            internal_error("读取商品列表失败")
        }
    }
}

async fn product_detail_api(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(product_id): axum::extract::Path<String>,
) -> Response {
    if let Err(response) = session::require_admin(&state, &headers).await {
        return response;
    }
    let Ok(product_id) = Uuid::parse_str(product_id.trim()) else {
        return error_response(StatusCode::NOT_FOUND, "商品不存在");
    };
    match fetch_product(&state.db, product_id).await {
        Ok(Some(product)) => session::app_ok("success", product),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "商品不存在"),
        Err(error) => {
            tracing::error!("读取商品详情失败: {error}");
            internal_error("读取商品详情失败")
        }
    }
}

async fn product_create_api(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(response) = session::require_admin(&state, &headers).await {
        return response;
    }
    match product_create_impl(&state.db, &body).await {
        Ok(product) => {
            let name = product.get("name").and_then(Value::as_str).unwrap_or("");
            if let Err(error) =
                append_audit_log(&state.db, "action", None, &format!("新增商城商品: {name}")).await
            {
                tracing::warn!("写审计日志失败: {error}");
            }
            session::app_ok("商品已创建", product)
        }
        Err(reject) => reject.into_response(),
    }
}

async fn product_update_api(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(product_id): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Response {
    if let Err(response) = session::require_admin(&state, &headers).await {
        return response;
    }
    let Ok(product_id) = Uuid::parse_str(product_id.trim()) else {
        return error_response(StatusCode::NOT_FOUND, "商品不存在");
    };
    match product_update_impl(&state.db, product_id, &body).await {
        Ok(product) => session::app_ok("商品已更新", product),
        Err(reject) => reject.into_response(),
    }
}

async fn product_delete_api(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(product_id): axum::extract::Path<String>,
) -> Response {
    if let Err(response) = session::require_admin(&state, &headers).await {
        return response;
    }
    let Ok(product_id) = Uuid::parse_str(product_id.trim()) else {
        return error_response(StatusCode::NOT_FOUND, "商品不存在");
    };
    match product_delete_impl(&state.db, product_id).await {
        Ok(true) => session::app_ok("商品已删除", json!({ "id": product_id.to_string() })),
        Ok(false) => error_response(StatusCode::NOT_FOUND, "商品不存在"),
        Err(error) => {
            tracing::error!("删除商品失败: {error}");
            internal_error("删除商品失败")
        }
    }
}

async fn product_toggle_api(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(product_id): axum::extract::Path<String>,
) -> Response {
    if let Err(response) = session::require_admin(&state, &headers).await {
        return response;
    }
    let Ok(product_id) = Uuid::parse_str(product_id.trim()) else {
        return error_response(StatusCode::NOT_FOUND, "商品不存在");
    };
    match product_toggle_impl(&state.db, product_id).await {
        Ok(Some(product)) => session::app_ok("商品状态已更新", product),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "商品不存在"),
        Err(error) => {
            tracing::error!("切换商品状态失败: {error}");
            internal_error("切换商品状态失败")
        }
    }
}

// ---- 业务实现 ----

async fn products_list_impl(pool: &PgPool) -> Result<Vec<Value>, sqlx::Error> {
    // 排序照抄 Django `citrus_product.Meta.ordering = ['sort_order', '-created_at']`。
    let sql = format!("{PRODUCT_SELECT} ORDER BY p.sort_order ASC, p.created_at DESC");
    let rows = sqlx::query(&sql).fetch_all(pool).await?;

    Ok(rows.iter().map(product_payload).collect())
}

async fn product_create_impl(pool: &PgPool, body: &Value) -> Result<Value, ProductReject> {
    let name =
        body_text(body, &["name"]).ok_or_else(|| ProductReject::bad_request("商品名称不能为空"))?;
    let price = body_price(body).ok_or_else(|| ProductReject::bad_request("商品价格无效"))?;
    if price < Decimal::from(1) {
        return Err(ProductReject::bad_request("商品价格必须大于 0"));
    }

    let batch_raw = body_text(body, &["sales_batch_id", "batch_id"])
        .ok_or_else(|| ProductReject::bad_request("必须选择销售批次（sales_batch_id）"))?;
    let batch_id =
        Uuid::parse_str(&batch_raw).map_err(|_| ProductReject::bad_request("销售批次不存在"))?;
    let batch = lookup_batch(pool, batch_id)
        .await
        .map_err(ProductReject::Db)?
        .ok_or_else(|| ProductReject::bad_request("销售批次不存在"))?;

    let status = body_text(body, &["status"]).unwrap_or_else(|| DEFAULT_PRODUCT_STATUS.to_string());
    if !valid_product_status(&status) {
        return Err(ProductReject::bad_request("商品状态无效"));
    }

    // `origin` 在契约表里是 NOT NULL，而旧后台表单没有这个字段 → 从批次所属果园推出。
    let origin = body_text(body, &["origin"]).unwrap_or(batch.origin);
    let unit = body_text(body, &["unit", "unit_label"]).unwrap_or_else(|| "5斤/箱".to_string());
    let product_id = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query(
        r#"INSERT INTO citrus_product
             (id, seller_id, sales_batch_id, name, sku_type, fruit_type, variety, origin,
              description, price, unit, stock, grade, shipping_note, cover_image_url,
              purchase_limit, minimum_order_quantity, sort_order, status, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                   $16, $17, $18, $19, $20, $20)"#,
    )
    .bind(product_id)
    .bind(batch.seller_id)
    .bind(batch_id)
    .bind(&name)
    .bind(body_text(body, &["sku_type"]).unwrap_or_else(|| "family".to_string()))
    .bind(body_text(body, &["fruit_type"]).unwrap_or_else(|| "脐橙".to_string()))
    .bind(body_text(body, &["variety"]).unwrap_or_else(|| "纽荷尔脐橙".to_string()))
    .bind(&origin)
    .bind(body_text(body, &["description"]).unwrap_or_default())
    .bind(price)
    .bind(&unit)
    .bind(body_i64(body, &["stock", "stock_quantity"]).unwrap_or(0) as i32)
    .bind(body_text(body, &["grade"]).unwrap_or_default())
    .bind(body_text(body, &["shipping_note"]).unwrap_or_default())
    .bind(body_text(body, &["cover_image_url", "cover_image"]).unwrap_or_default())
    .bind(body_i64(body, &["purchase_limit"]).unwrap_or(20) as i32)
    .bind(body_i64(body, &["minimum_order_quantity"]).unwrap_or(1) as i32)
    .bind(body_i64(body, &["sort_order"]).unwrap_or(0) as i32)
    .bind(&status)
    .bind(now)
    .execute(pool)
    .await
    .map_err(ProductReject::Db)?;

    fetch_product(pool, product_id)
        .await
        .map_err(ProductReject::Db)?
        .ok_or_else(|| ProductReject::bad_request("商品创建失败"))
}

async fn product_update_impl(
    pool: &PgPool,
    product_id: Uuid,
    body: &Value,
) -> Result<Value, ProductReject> {
    let current = sqlx::query(
        r#"SELECT name, description, unit, price, stock, grade, shipping_note, cover_image_url,
                  purchase_limit, minimum_order_quantity, sort_order, status, sales_batch_id,
                  sku_type, fruit_type, variety, origin
           FROM citrus_product WHERE id = $1"#,
    )
    .bind(product_id)
    .fetch_optional(pool)
    .await
    .map_err(ProductReject::Db)?;
    let Some(current) = current else {
        return Err(ProductReject::not_found("商品不存在"));
    };

    let name = body_text(body, &["name"])
        .unwrap_or_else(|| current.try_get::<String, _>("name").unwrap_or_default());
    let price = body_price(body).unwrap_or_else(|| {
        current
            .try_get::<Decimal, _>("price")
            .unwrap_or_else(|_| Decimal::ZERO)
    });
    if price < Decimal::from(1) {
        return Err(ProductReject::bad_request("商品价格必须大于 0"));
    }
    let status = match body_text(body, &["status"]) {
        Some(status) => {
            if !valid_product_status(&status) {
                return Err(ProductReject::bad_request("商品状态无效"));
            }
            status
        }
        None => current
            .try_get::<String, _>("status")
            .unwrap_or_else(|_| DEFAULT_PRODUCT_STATUS.to_string()),
    };

    // 换批次时要重新校验并重算 seller/origin（批次是 RESTRICT 外键，不存在必须 400）。
    let (batch_id, seller_id, origin) = match body_text(body, &["sales_batch_id", "batch_id"]) {
        Some(raw) => {
            let batch_id =
                Uuid::parse_str(&raw).map_err(|_| ProductReject::bad_request("销售批次不存在"))?;
            let batch = lookup_batch(pool, batch_id)
                .await
                .map_err(ProductReject::Db)?
                .ok_or_else(|| ProductReject::bad_request("销售批次不存在"))?;
            (batch_id, batch.seller_id, batch.origin)
        }
        None => (
            current
                .try_get::<Option<Uuid>, _>("sales_batch_id")
                .ok()
                .flatten()
                .unwrap_or_else(Uuid::nil),
            None,
            current
                .try_get::<String, _>("origin")
                .unwrap_or_else(|_| "江西省赣州市".to_string()),
        ),
    };

    let now = Utc::now();
    let affected = sqlx::query(
        r#"UPDATE citrus_product SET
             name = $2, description = $3, unit = $4, price = $5, stock = $6, grade = $7,
             shipping_note = $8, cover_image_url = $9, purchase_limit = $10,
             minimum_order_quantity = $11, sort_order = $12, status = $13, sales_batch_id = $14,
             seller_id = COALESCE($15, seller_id), origin = $16, updated_at = $17
           WHERE id = $1"#,
    )
    .bind(product_id)
    .bind(&name)
    .bind(body_text(body, &["description"]).unwrap_or_else(|| {
        current
            .try_get::<String, _>("description")
            .unwrap_or_default()
    }))
    .bind(
        body_text(body, &["unit", "unit_label"])
            .unwrap_or_else(|| current.try_get::<String, _>("unit").unwrap_or_default()),
    )
    .bind(price)
    .bind(
        body_i64(body, &["stock", "stock_quantity"])
            .map(|value| value as i32)
            .unwrap_or_else(|| current.try_get::<i32, _>("stock").unwrap_or(0)),
    )
    .bind(
        body_text(body, &["grade"])
            .unwrap_or_else(|| current.try_get::<String, _>("grade").unwrap_or_default()),
    )
    .bind(body_text(body, &["shipping_note"]).unwrap_or_else(|| {
        current
            .try_get::<String, _>("shipping_note")
            .unwrap_or_default()
    }))
    .bind(
        body_text(body, &["cover_image_url", "cover_image"]).unwrap_or_else(|| {
            current
                .try_get::<String, _>("cover_image_url")
                .unwrap_or_default()
        }),
    )
    .bind(
        body_i64(body, &["purchase_limit"])
            .map(|value| value as i32)
            .unwrap_or_else(|| current.try_get::<i32, _>("purchase_limit").unwrap_or(0)),
    )
    .bind(
        body_i64(body, &["minimum_order_quantity"])
            .map(|value| value as i32)
            .unwrap_or_else(|| {
                current
                    .try_get::<i32, _>("minimum_order_quantity")
                    .unwrap_or(1)
            }),
    )
    .bind(
        body_i64(body, &["sort_order"])
            .map(|value| value as i32)
            .unwrap_or_else(|| current.try_get::<i32, _>("sort_order").unwrap_or(0)),
    )
    .bind(&status)
    .bind(batch_id)
    .bind(seller_id)
    .bind(&origin)
    .bind(now)
    .execute(pool)
    .await
    .map_err(ProductReject::Db)?
    .rows_affected();
    if affected == 0 {
        return Err(ProductReject::not_found("商品不存在"));
    }

    fetch_product(pool, product_id)
        .await
        .map_err(ProductReject::Db)?
        .ok_or_else(|| ProductReject::not_found("商品不存在"))
}

async fn product_delete_impl(pool: &PgPool, product_id: Uuid) -> Result<bool, sqlx::Error> {
    let affected = sqlx::query("DELETE FROM citrus_product WHERE id = $1")
        .bind(product_id)
        .execute(pool)
        .await?
        .rows_affected();

    Ok(affected > 0)
}

async fn product_toggle_impl(
    pool: &PgPool,
    product_id: Uuid,
) -> Result<Option<Value>, sqlx::Error> {
    // `draft` 也视为「未上架」→ 第一次 toggle 直接上架。
    let affected = sqlx::query(
        r#"UPDATE citrus_product
           SET status = CASE WHEN status = 'on_sale' THEN 'off_sale' ELSE 'on_sale' END,
               updated_at = $2
           WHERE id = $1"#,
    )
    .bind(product_id)
    .bind(Utc::now())
    .execute(pool)
    .await?
    .rows_affected();
    if affected == 0 {
        return Ok(None);
    }

    fetch_product(pool, product_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_conversion_matches_frontend_cents() {
        // 前端按「分」做算术与展示（storeMoney），小数与四舍五入都要对。
        assert_eq!(amount_to_cents("19.90".parse::<Decimal>().unwrap()), 1990);
        assert_eq!(amount_to_cents("0".parse::<Decimal>().unwrap()), 0);
        assert_eq!(amount_to_cents("129.99".parse::<Decimal>().unwrap()), 12999);
        assert_eq!(amount_to_cents("0.005".parse::<Decimal>().unwrap()), 1);
    }

    #[test]
    fn only_on_sale_products_are_active() {
        assert!(product_is_active("on_sale"));
        assert!(!product_is_active("draft"));
        assert!(!product_is_active("off_sale"));
    }

    #[test]
    fn order_status_validation_covers_django_state_machine() {
        for status in ORDER_STATUSES {
            assert!(valid_order_status(status), "{status}");
        }
        // 自研老状态与错拼都不许通过
        assert!(!valid_order_status("refunded"));
        assert!(!valid_order_status("shipping"));
        assert!(!valid_order_status(""));
    }

    #[test]
    fn revenue_and_pending_sets_are_disjoint_enough_to_be_meaningful() {
        // 成交额只算已收款；待完结含未付款。两者语义不同，不能混用同一集合。
        assert!(REVENUE_STATUSES.contains(&"completed"));
        assert!(!REVENUE_STATUSES.contains(&"pending_payment"));
        assert!(PENDING_STATUSES.contains(&"pending_payment"));
        assert!(!PENDING_STATUSES.contains(&"cancelled"));
    }

    // ---- 商品 CRUD 的纯函数 ----

    /// 旧前端两种提交形态都必须吃：`admin.js:1578` / `store-admin.js:272` 传的是**分**。
    #[test]
    fn product_price_accepts_both_decimal_and_cents() {
        let native = json!({ "price": "168.00" });
        assert_eq!(body_price(&native).unwrap().to_string(), "168.00");

        let numeric = json!({ "price": 168.0 });
        assert_eq!(body_price(&numeric).unwrap().to_string(), "168.0");

        // price_cents 是「分」→ 必须除以 100，否则会 100 倍误差
        let legacy = json!({ "price_cents": 16800 });
        assert_eq!(body_price(&legacy).unwrap().to_string(), "168");

        let legacy_odd = json!({ "price_cents": 3999 });
        assert_eq!(body_price(&legacy_odd).unwrap().to_string(), "39.99");

        // 原生字段优先，两个都给时以 price 为准
        let both = json!({ "price": "10.00", "price_cents": 99999 });
        assert_eq!(body_price(&both).unwrap().to_string(), "10.00");

        assert!(body_price(&json!({})).is_none());
        assert!(body_price(&json!({ "price": "abc" })).is_none());
    }

    #[test]
    fn product_text_fields_trim_and_reject_blank() {
        assert_eq!(
            body_text(&json!({"name": "  脐橙  "}), &["name"]).as_deref(),
            Some("脐橙")
        );
        assert!(body_text(&json!({"name": "   "}), &["name"]).is_none());
        assert!(body_text(&json!({}), &["name"]).is_none());
        // 候选键按顺序取，兼容旧名
        assert_eq!(
            body_text(&json!({"unit_label": "5斤/箱"}), &["unit", "unit_label"]).as_deref(),
            Some("5斤/箱")
        );
    }

    #[test]
    fn product_status_validation_and_display() {
        for status in PRODUCT_STATUSES {
            assert!(valid_product_status(status), "{status}");
        }
        // 订单状态不能用来当商品状态
        assert!(!valid_product_status("paid"));
        assert!(!valid_product_status(""));
        assert_eq!(product_status_display("on_sale"), "在售");
        assert_eq!(product_status_display("off_sale"), "已下架");
        assert_eq!(product_status_display("draft"), "待上架");
    }

    /// 别名必须是**派生**的：`price_cents` 与 `is_active` 都由原生字段算出来。
    #[test]
    fn derived_aliases_are_consistent_with_native_fields() {
        assert_eq!(amount_to_cents("168.00".parse::<Decimal>().unwrap()), 16800);
        assert!(product_is_active("on_sale"));
        assert!(!product_is_active("draft"));
        assert!(!product_is_active("off_sale"));
        // 默认状态必须是 on_sale：旧后台表单没有状态字段，用 draft 会导致「建完看不见」
        assert_eq!(DEFAULT_PRODUCT_STATUS, "on_sale");
    }
}
