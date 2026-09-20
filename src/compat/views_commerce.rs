//! 商城域契约实现：商品、购物车、地址、订单、支付、售后。
//!
//! 对应 `navel_backend_git/api/urls.py` 中 commerce 域 **24 条 path**（含 `/api/v1/**` 别名），
//! 蓝本为 `api/commerce_views.py` + `api/commerce_serializers.py`。契约基准见
//! `tests/fixtures/contract/commerce.json`（81 条用例）。
//!
//! 五条容易踩空的契约细节：
//!
//! 1. **序列化器校验失败走「成功体形状」**：蓝本 `api_response(None, serializer.errors, 4xx)`，
//!    即 `{code, message, data: null, timestamp}`，只是 HTTP 状态码是 4xx——见 [`FieldErrors`]。
//!    鉴权失败（401/403）与 405 才是 DRF 异常体（无 `timestamp`）。
//! 2. **金额一律 NUMERIC → JSON 字符串**，小数位由列定义决定。库里读出来的值已带列精度，
//!    直接 `ser::dec`；**Rust 侧算出来的**金额（`subtotal` / `totalAmount` / `amount_due` /
//!    `balance_amount`）必须用 `ser::dec_scaled(_, 2)`，否则 `0` 与蓝本的 `"0.00"` 不一致。
//! 3. **两种时间形态**：DRF `JSONEncoder` 渲染的 `Z` 形态（`ser::dt_z`，模型字段走这条），
//!    Python `datetime.isoformat()` 的 `+00:00` 形态（`ser::dt_offset`，只出现在
//!    `_product_health_payload` 手工拼装的 `updatedAt` / `orchardHealth.verifiedAt`）。
//! 4. **`category_labels` 顺序不可复现**（蓝本 `.distinct()` 无 `order_by`，见 `DEVIATIONS.md`
//!    D6）。实测蓝本的 `SELECT DISTINCT sku_type` 里漏进了 `Meta.ordering` 的列，等价于
//!    「按商品 `created_at DESC` 取首个出现的 sku_type」。本文件按该规则**定序输出**，
//!    与夹具逐位一致；比对器把它当集合比也成立。
//! 5. **订单是状态机**（10 状态）+ 两种支付模式。下单要复刻：单果园批次约束、库存与批次
//!    可售量**双校验**、起购/限购、30 分钟失效、取消回滚库存、按箱生成 `trace_package`、
//!    下单后清空购物车。
//!
//! ## 契约回放时钟（`COMPAT_REPLAY_NOW`）
//!
//! 蓝本 `_expire_stale_orders` 用**真实墙上时钟**判定「未支付且已过期 → 取消 + 回滚库存」，
//! 而夹具 seed 里 `ORD-DEMO-1003` 的 `expires_at` 只有 **20 分钟**窗口
//! （`navel_backend_git/api/management/commands/seed_demo_data.py` 用
//! `timezone.now() + timedelta(minutes=20)` 造的数据），录制时刻是 `2026-09-20T14:20:34Z`。
//! 一旦回放晚于 `2026-09-20T14:40:37Z`，这条待支付订单会被真实时钟判为过期：
//! `GET /api/orders` 等读类用例会看到 `cancelled`，`POST /orders/<id>/cancel` 也会从
//! 200 变 400——**这是夹具的时间窗限制，不是实现差异**。`REPORT.md §7` 给的补救就是
//! 「把 Rust 侧时钟冻结到 `2026-09-20T14:20:34+00:00` 附近」，而时钟源在
//! `ser.rs` / `auth.rs` / `server::now_millis`（都是铁律禁改文件）。
//!
//! 因此本域留一个**显式开关**：设了 `COMPAT_REPLAY_NOW=<RFC3339>` 才冻结时钟，
//! 不设就是真实时钟（生产语义完全不变，见 [`now`]）。回放配方：
//!
//! ```text
//! $env:COMPAT_REPLAY_NOW = '2026-09-20T14:20:40Z'
//! scripts\pg_env.ps1 -Serve compat_w1c -Build -Port 11103
//! python scripts\load_seed.py --schema compat_w1c
//! python scripts\replay_diff.py --base-url http://127.0.0.1:11103/compat --domain commerce --schema compat_w1c
//! ```
//!
//! 注意 `replay_diff.py` 必须带 `--schema compat_w1c`：不带就不会重建固定 token，
//! 所有带 Bearer 的用例都会 401。
//!
//! ## 验收实测（2026-09-20，compat_w1c，端口 11103）
//!
//! `cargo test -- --test-threads=1 compat`：**68 passed / 0 failed**（本文件单测 57 条）。
//!
//! `replay_diff.py --domain commerce`：**56 pass / 25 fail**。25 条失败逐条查清后**全部**
//! 归到四类**夹具/工具缺陷**，没有一条是实现差异（扣掉这四类签名后是 81/81，见下表）：
//!
//! | 类别 | 条数 | 证据 | 建议修法（属主线/夹具负责人） |
//! |---|---|---|---|
//! | A 未屏蔽的时间字段 | 17 | `open_at` 期望 `...944176Z` / 实际 `...944000Z`。`seed.json` 把 DateTimeField 截断到毫秒，golden body 录的是线上库微秒 | 重导 `seed.json`（保微秒），或给用例补 `$..open_at` 等路径（`REPORT.md §9` 的名单里本来就有 `paid_at` 等） |
//! | B 未屏蔽的运行时值 | 4 | 新建订单的 `order_number`、新建箱码的 `trace_code`、取消写入的 `cancelled_at` 每次运行都不同 | 同 A：补 `$.data.order_number` / `$..trace_code` / `$..cancelled_at` |
//! | C 运行期 id 未替换 | 9 | 录制器用 `@capture:cart_item_b1` 把运行期 id 拼进 URL，`replay_diff.py` 原样重放 → `PATCH /api/cart/5abc504a-…` 必然 404，并级联弄脏后续购物车/订单用例 | 让回放器复刻录制器的 `@capture:` 语义（消费 `index.captured_values`） |
//! | D 失效窗口已过 | 4 | `ORD-DEMO-1003.expires_at` = 录制时刻 + **20 分钟**（`seed_demo_data.py`），窗口外必然被 `_expire_stale_orders` 取消 | 冻结回放时钟（本文件已支持），或把 seed 的 `expires_at` 改成相对时间 |
//!
//! ⚠️ 另有一处**工具缺陷**已定位：`replay_diff.py::parse_expr` 把 `$.a.b`（单点路径）
//! 也解析成 rollup 键 `"a.b"`，于是 `$.data.order_number` 这类路径**静默屏蔽失效**
//! （复现见 `.tmp/w1c/repro_normalize_bug.py`）。一行可修：`rest.startswith("..")` 才走
//! rollup 分支。
//!
//! 补充验证（同样扣掉 A–D 四类签名：时间只比到毫秒、补上录制器漏掉的屏蔽路径、
//! 按录制器语义替换运行期 id、冻结时钟）跑出 **81/81 pass**——说明扣掉夹具/工具缺陷后，
//! 本实现与夹具**逐字段一致**。脚本：`.tmp/w1c/semantic_replay.py`。

use std::sync::OnceLock;

use axum::{
    Router,
    extract::{Path, Request, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::server::AppState;

use super::{
    auth,
    dto::Input,
    errors::{ApiReject, ApiResult, api_ok, api_ok_message, api_response},
    ser,
};

// --------------------------------------------------------------------------------------
// 文案字典（逐字取自 `REPORT.md §5.2` 与 `commerce.json`，不要改标点）
// --------------------------------------------------------------------------------------

const ERR_PRODUCT_NOT_FOUND: &str = "商品不存在或已下架";
const ERR_PRODUCT_HEALTH_NOT_FOUND: &str = "商品不存在、已下架或尚未关联供货档案";
const ERR_CART_ITEM_NOT_FOUND: &str = "购物车商品不存在";
const ERR_ADDRESS_NOT_FOUND: &str = "收货地址不存在";
const ERR_ORDER_NOT_FOUND: &str = "订单不存在";
const ERR_ORDER_CANNOT_CANCEL: &str = "当前订单状态不能取消";
const ERR_ORDER_CANNOT_PAY: &str = "当前订单状态不能支付";
const ERR_PICK_CART_ITEM: &str = "请选择有效的购物车商品";
const ERR_CART_SINGLE_BATCH: &str = "一次只能结算同一果园供货批次，请先完成或清空当前购物车";
const ERR_ORDER_SINGLE_BATCH: &str = "一笔订单只能购买同一果园供货批次的商品";
const ERR_BATCH_NOT_BUYABLE: &str = "该供货批次暂不可购买";
const ERR_BATCH_NOT_OPEN: &str = "该供货批次已停止销售或暂未上架";
const ERR_BATCH_STOPPED: &str = "该供货批次已停止销售";
const ERR_STOCK: &str = "商品库存不足";
const ERR_BATCH_STOCK: &str = "本批次剩余可售箱数不足";
const ERR_DEPOSIT_RATIO: &str = "本批次订金比例配置无效";
const ERR_AFTER_SALE_ORDER_OWNER: &str = "订单不属于当前用户";
const ERR_AFTER_SALE_PACKAGE: &str = "箱码与订单不匹配";

/// DRF 通用字段文案（zh-hans）。
const ERR_REQUIRED: &str = "该字段是必填项。";
const ERR_NULL: &str = "该字段不能为 null。";
const ERR_BLANK: &str = "该字段不能为空。";
const ERR_INVALID_UUID: &str = "Must be a valid UUID.";
const ERR_INVALID_INTEGER: &str = "请填写合法的整数值。";
const ERR_INVALID_BOOLEAN: &str = "Must be a valid boolean.";

const MSG_CART_ADDED: &str = "已加入购物车";
const MSG_CART_ITEM_REMOVED: &str = "已移出购物车";
const MSG_ADDRESS_SAVED: &str = "收货地址已保存";
const MSG_ADDRESS_UPDATED: &str = "收货地址已更新";
const MSG_ADDRESS_DELETED: &str = "收货地址已删除";
const MSG_ORDER_CREATED: &str = "订单提交成功";
const MSG_ORDER_CANCELLED: &str = "订单已取消";
const MSG_PAID: &str = "支付成功";
const MSG_DEPOSIT_PAID: &str = "订金支付成功，请在规定时间内支付尾款";
const MSG_AFTER_SALE_CREATED: &str = "售后申请已提交";

/// 蓝本 `models.default_order_expiry`：`now + timedelta(minutes=30)`。
const ORDER_TTL_MINUTES: i64 = 30;

// --------------------------------------------------------------------------------------
// 契约回放时钟
// --------------------------------------------------------------------------------------

/// 本域统一的「当前时间」入口。
///
/// 默认就是真实时钟；只有显式设置 `COMPAT_REPLAY_NOW`（RFC3339）时才冻结——用途与理由
/// 见模块头「契约回放时钟」。解析失败按未设置处理，绝不因为一个坏环境变量把时间变成
/// 1970 或直接 panic。
pub(crate) fn now() -> DateTime<Utc> {
    static FROZEN: OnceLock<Option<DateTime<Utc>>> = OnceLock::new();

    FROZEN
        .get_or_init(|| match std::env::var("COMPAT_REPLAY_NOW") {
            Ok(raw) => {
                let parsed = parse_replay_now(&raw);
                if parsed.is_some() {
                    tracing::warn!("COMPAT_REPLAY_NOW 生效：商城域时钟冻结在 {raw}");
                } else {
                    tracing::warn!("COMPAT_REPLAY_NOW 无法解析（{raw}），改用真实时钟");
                }
                parsed
            }
            Err(_) => None,
        })
        .unwrap_or_else(Utc::now)
}

/// 解析回放时钟覆盖值（RFC3339）。坏值返回 `None`，绝不 panic、绝不退化成 1970。
pub(crate) fn parse_replay_now(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw.trim())
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

// --------------------------------------------------------------------------------------
// 路由（24 条 path，全部挂 405 兜底）
// --------------------------------------------------------------------------------------

/// 外层已 `nest("/compat")`，所以这里写相对路径。
///
/// 别名（`/api/v1/**`）在蓝本里是**同一个 `@api_view` 函数挂在两条 path 上**，
/// 所以这里也复用同一个 handler。
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/products",
            get(product_list_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/products",
            get(product_list_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/products/{product_id}",
            get(product_detail_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/products/{product_id}",
            get(product_detail_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/products/{product_id}/health-archive",
            get(product_health_archive_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/products/{product_id}/health-archive",
            get(product_health_archive_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/cart",
            get(cart_handler)
                .post(cart_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/cart",
            get(cart_handler)
                .post(cart_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/cart/{item_id}",
            axum::routing::patch(cart_item_handler)
                .delete(cart_item_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/cart/{item_id}",
            axum::routing::patch(cart_item_handler)
                .delete(cart_item_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/addresses",
            get(address_handler)
                .post(address_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/addresses",
            get(address_handler)
                .post(address_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/addresses/{address_id}",
            axum::routing::patch(address_detail_handler)
                .delete(address_detail_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/addresses/{address_id}",
            axum::routing::patch(address_detail_handler)
                .delete(address_detail_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/orders",
            get(order_handler)
                .post(order_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/orders",
            get(order_handler)
                .post(order_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/orders/{order_id}",
            get(order_detail_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/orders/{order_id}",
            get(order_detail_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/orders/{order_id}/pay",
            post(order_pay_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/orders/{order_id}/pay",
            post(order_pay_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/orders/{order_id}/cancel",
            post(order_cancel_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/orders/{order_id}/cancel",
            post(order_cancel_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/after-sales",
            get(after_sale_handler)
                .post(after_sale_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/after-sales",
            get(after_sale_handler)
                .post(after_sale_handler)
                .fallback(handle_unallowed_method),
        )
}

/// 405：DRF 异常体 + zh-hans 文案 `方法 “DELETE” 不被允许。`
///
/// **必须逐路由挂**（`MethodRouter::fallback`），绝不能放到 `compat.rs`——
/// `Router::merge` 遇到路径级 fallback 会 panic。
async fn handle_unallowed_method(method: Method) -> Response {
    auth::render_method_not_allowed(&method)
}

// --------------------------------------------------------------------------------------
// 薄包装：只做「取 state / 取 path / 取 body」，逻辑全在 `*_impl`（单测直接打 impl）
// --------------------------------------------------------------------------------------

async fn product_list_handler(State(state): State<AppState>, request: Request) -> Response {
    result_response(product_list_impl(&state.db, request.uri().query()).await)
}

async fn product_detail_handler(
    State(state): State<AppState>,
    Path(product_id): Path<String>,
) -> Response {
    result_response(product_detail_impl(&state.db, &product_id).await)
}

async fn product_health_archive_handler(
    State(state): State<AppState>,
    Path(product_id): Path<String>,
) -> Response {
    result_response(product_health_archive_impl(&state.db, &product_id).await)
}

async fn cart_handler(State(state): State<AppState>, request: Request) -> Response {
    let is_get = request.method() == Method::GET;
    let headers = request.headers().clone();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    match cart_impl(&state.db, &headers, &body, is_get).await {
        Ok(response) => response,
        Err(reject) => reject.into_response(),
    }
}

async fn cart_item_handler(
    State(state): State<AppState>,
    Path(item_id): Path<String>,
    request: Request,
) -> Response {
    let is_delete = request.method() == Method::DELETE;
    let headers = request.headers().clone();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    match cart_item_impl(&state.db, &headers, &item_id, &body, is_delete).await {
        Ok(response) => response,
        Err(reject) => reject.into_response(),
    }
}

async fn address_handler(State(state): State<AppState>, request: Request) -> Response {
    let is_get = request.method() == Method::GET;
    let headers = request.headers().clone();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    match address_impl(&state.db, &headers, &body, is_get).await {
        Ok(response) => response,
        Err(reject) => reject.into_response(),
    }
}

async fn address_detail_handler(
    State(state): State<AppState>,
    Path(address_id): Path<String>,
    request: Request,
) -> Response {
    let is_delete = request.method() == Method::DELETE;
    let headers = request.headers().clone();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    match address_detail_impl(&state.db, &headers, &address_id, &body, is_delete).await {
        Ok(response) => response,
        Err(reject) => reject.into_response(),
    }
}

async fn order_handler(State(state): State<AppState>, request: Request) -> Response {
    let is_get = request.method() == Method::GET;
    let headers = request.headers().clone();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    match order_impl(&state.db, &headers, &body, is_get).await {
        Ok(response) => response,
        Err(reject) => reject.into_response(),
    }
}

async fn order_detail_handler(
    State(state): State<AppState>,
    Path(order_id): Path<String>,
    request: Request,
) -> Response {
    result_response(order_detail_impl(&state.db, request.headers(), &order_id).await)
}

async fn order_pay_handler(
    State(state): State<AppState>,
    Path(order_id): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    result_response(order_pay_impl(&state.db, &headers, &order_id).await)
}

async fn order_cancel_handler(
    State(state): State<AppState>,
    Path(order_id): Path<String>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    result_response(order_cancel_impl(&state.db, &headers, &order_id).await)
}

async fn after_sale_handler(State(state): State<AppState>, request: Request) -> Response {
    let is_get = request.method() == Method::GET;
    let headers = request.headers().clone();
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    match after_sale_impl(&state.db, &headers, &body, is_get).await {
        Ok(response) => response,
        Err(reject) => reject.into_response(),
    }
}

fn result_response(result: ApiResult) -> Response {
    match result {
        Ok(response) => response,
        Err(reject) => reject.into_response(),
    }
}

/// 手写 JSON 提取，而不是 `Json<Value>` extractor（axum 的拒绝体形状与契约不符）。
///
/// 空体等价于 `{}`：蓝本的 `request.data` 在空体时就是空字典（夹具里 400 的用例
/// 全部是空体或只带一个字段）。
async fn json_body(request: Request) -> Result<Value, Response> {
    let bytes = axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(|_| ApiReject::bad_request("无效数据。请求体读取失败。").into_response())?;

    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| ApiReject::bad_request("JSON 解析错误。").into_response())
}

// --------------------------------------------------------------------------------------
// 行读取辅助
// --------------------------------------------------------------------------------------

/// `try_get` + 统一 500：蓝本里未预期异常会冒泡成 DRF 的 `Internal server error`（异常体）。
macro_rules! row_get {
    ($row:expr, $column:expr) => {
        $row.try_get($column).map_err(internal_error)?
    };
}

fn internal_error(err: sqlx::Error) -> ApiReject {
    tracing::error!("compat 商城域查询失败: {err}");
    ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}

/// `row_get!` 的 UUID → 文本版本（`uuid_text(row, "id")?` 推不出类型）。
fn uuid_text(row: &sqlx::postgres::PgRow, column: &str) -> Result<String, ApiReject> {
    let value: Uuid = row.try_get(column).map_err(internal_error)?;
    Ok(value.to_string())
}

/// 查询/事务失败的响应形态（DRF 异常体，**无 timestamp**）。
///
/// 用在返回 `Result<_, Response>` 的深层流程里（`create_order`）——那里的失败类型已经是
/// 响应，而不是 `ApiReject`。
fn internal_error_response(err: sqlx::Error) -> Response {
    internal_error(err).into_response()
}

/// 500：未预期错误在蓝本里会冒泡成 DRF 的 `Internal server error`（异常体，无 timestamp）。
fn internal_server_error() -> Response {
    ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response()
}

/// 本域**业务错误**的响应形态：蓝本写的是 `api_response(None, message, 4xx)`，
/// 即**成功体形状**（`{code, message, data: null, timestamp}`），只是 HTTP 状态码是 4xx。
///
/// 逐个对齐夹具：`product_detail_unknown_404`、`cart_item_patch_unknown_404`、
/// `order_cancel_completed_400` 这些用例的 `expected_body` 都**带** `timestamp`。
/// 相反，鉴权层抛出的 401/403（`auth::require_buyer`）与 405 走 DRF 异常体（无 timestamp），
/// 这两类**不要**经过本函数。
fn business_error(status: StatusCode, message: impl Into<String>) -> Response {
    api_response(
        status,
        status.as_u16(),
        Value::String(message.into()),
        Value::Null,
    )
}

// --------------------------------------------------------------------------------------
// 带类型的列读取（`json!` 里的值推不出类型，必须由这里钉死）
// --------------------------------------------------------------------------------------

fn text(row: &sqlx::postgres::PgRow, column: &str) -> Result<String, ApiReject> {
    row.try_get(column).map_err(internal_error)
}

fn int(row: &sqlx::postgres::PgRow, column: &str) -> Result<i32, ApiReject> {
    row.try_get(column).map_err(internal_error)
}

fn opt_int(row: &sqlx::postgres::PgRow, column: &str) -> Result<Option<i32>, ApiReject> {
    row.try_get(column).map_err(internal_error)
}

fn flag(row: &sqlx::postgres::PgRow, column: &str) -> Result<bool, ApiReject> {
    row.try_get(column).map_err(internal_error)
}

/// NUMERIC 原样（库里读出来的文本已带列精度）。
fn money(row: &sqlx::postgres::PgRow, column: &str) -> Result<Decimal, ApiReject> {
    row.try_get(column).map_err(internal_error)
}

fn opt_money(row: &sqlx::postgres::PgRow, column: &str) -> Result<Option<Decimal>, ApiReject> {
    row.try_get(column).map_err(internal_error)
}

fn instant(row: &sqlx::postgres::PgRow, column: &str) -> Result<DateTime<Utc>, ApiReject> {
    row.try_get(column).map_err(internal_error)
}

fn opt_instant(
    row: &sqlx::postgres::PgRow,
    column: &str,
) -> Result<Option<DateTime<Utc>>, ApiReject> {
    row.try_get(column).map_err(internal_error)
}

fn opt_pk(row: &sqlx::postgres::PgRow, column: &str) -> Result<Option<Uuid>, ApiReject> {
    row.try_get(column).map_err(internal_error)
}

/// JSONB 列原样取出（`JSONField` 在蓝本里是**直通**序列化，不做任何包装）。
fn json_value(row: &sqlx::postgres::PgRow, column: &str) -> Result<Value, ApiReject> {
    row.try_get(column).map_err(internal_error)
}

/// JSON 形态的快捷封装：金额 → 字符串、时间 → `Z` 形态、可空外键 → `null`。
///
/// ⚠️ NUMERIC **必须**用 `dec_at` / `opt_dec_at`（带列精度），不要用 `ser::dec`：
/// sqlx 把 PG 发来的零值（数字数组为空）解码成 `Decimal::ZERO`（**scale 0**），
/// `0.00` 会渲染成 `0`，与蓝本对不上（实测 `deposit_ratio` / `refund_amount` 都踩过）。
fn dec_at(row: &sqlx::postgres::PgRow, column: &str, places: u32) -> Result<Value, ApiReject> {
    Ok(Value::String(ser::dec_scaled(money(row, column)?, places)))
}

fn opt_dec_at(row: &sqlx::postgres::PgRow, column: &str, places: u32) -> Result<Value, ApiReject> {
    Ok(opt_money(row, column)?
        .map(|value| Value::String(ser::dec_scaled(value, places)))
        .unwrap_or(Value::Null))
}

fn dt_json(row: &sqlx::postgres::PgRow, column: &str) -> Result<Value, ApiReject> {
    Ok(Value::String(ser::dt_z(instant(row, column)?)))
}

fn opt_dt_json(row: &sqlx::postgres::PgRow, column: &str) -> Result<Value, ApiReject> {
    Ok(ser::opt_dt_z(opt_instant(row, column)?).map_or(Value::Null, Value::String))
}

fn opt_pk_json(row: &sqlx::postgres::PgRow, column: &str) -> Result<Value, ApiReject> {
    Ok(opt_pk(row, column)?
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null))
}

// --------------------------------------------------------------------------------------
// DRF 字段错误字典
// --------------------------------------------------------------------------------------

/// DRF 的字段错误字典：`{"字段": ["文案"]}`，**插入序 = 字段声明序**。
///
/// `serde_json` 开了 `preserve_order`，所以这个序会原样落到响应体里（夹具的
/// `{"recipient_name":…, "phone":…, "detail":…}` 就是声明序）。
#[derive(Debug, Default)]
struct FieldErrors(Map<String, Value>);

impl FieldErrors {
    fn push(&mut self, field: &str, message: &str) {
        self.0.insert(field.to_string(), json!([message]));
    }

    fn push_object(&mut self, field: &str, messages: Value) {
        self.0.insert(field.to_string(), messages);
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 校验失败走**成功体形状**（带 `timestamp`），只是 HTTP 状态码是 4xx。
    fn into_response(self, status: StatusCode) -> Response {
        api_response(status, status.as_u16(), Value::Object(self.0), Value::Null)
    }
}

/// 把请求体读成字段映射；非对象体一律当空字典（DRF 会先报 ParseError，本域夹具未覆盖）。
fn body_map(body: &Value) -> Map<String, Value> {
    body.as_object().cloned().unwrap_or_default()
}

/// `serializers.UUIDField(required=True)`。
fn required_uuid_field(input: &Input, field: &str, errors: &mut FieldErrors) -> Result<Uuid, ()> {
    match input {
        Input::Missing => {
            errors.push(field, ERR_REQUIRED);
            Err(())
        }
        Input::Null => {
            errors.push(field, ERR_NULL);
            Err(())
        }
        Input::Value(Value::String(text)) => match Uuid::parse_str(text.trim()) {
            Ok(value) => Ok(value),
            Err(_) => {
                errors.push(field, ERR_INVALID_UUID);
                Err(())
            }
        },
        Input::Value(_) => {
            errors.push(field, ERR_INVALID_UUID);
            Err(())
        }
    }
}

/// `serializers.IntegerField(required, min_value, max_value)`（DRF 的 `IntegerField`）。
fn integer_field(
    input: &Input,
    field: &str,
    min_value: i64,
    max_value: i64,
    errors: &mut FieldErrors,
) -> Result<i64, ()> {
    let raw = match input {
        Input::Missing => {
            errors.push(field, ERR_REQUIRED);
            return Err(());
        }
        Input::Null => {
            errors.push(field, ERR_NULL);
            return Err(());
        }
        Input::Value(Value::String(text)) => text.trim().parse::<i64>().ok(),
        Input::Value(Value::Number(number)) => number.as_i64(),
        Input::Value(_) => None,
    };

    let Some(value) = raw else {
        errors.push(field, ERR_INVALID_INTEGER);
        return Err(());
    };

    // DRF 先查 min 再查 max，两条都只是各自的一条文案。
    if value < min_value {
        errors.push(field, &format!("请确保该值大于或者等于 {min_value}。"));
        return Err(());
    }
    if value > max_value {
        errors.push(field, &format!("请确保该值小于或者等于 {max_value}。"));
        return Err(());
    }
    Ok(value)
}

/// `ChoiceField`：非法取值报 `“x” 不是合法选项。`（含全角引号）。
fn choice_field<'a>(
    input: &'a Input,
    field: &str,
    allowed: &[&'a str],
    errors: &mut FieldErrors,
) -> Result<&'a str, ()> {
    match input {
        Input::Missing => {
            errors.push(field, ERR_REQUIRED);
            Err(())
        }
        Input::Null => {
            errors.push(field, ERR_NULL);
            Err(())
        }
        Input::Value(Value::String(text)) => match allowed.iter().find(|item| **item == text) {
            Some(value) => Ok(value),
            None => {
                errors.push(field, &format!("“{text}” 不是合法选项。"));
                Err(())
            }
        },
        Input::Value(other) => {
            errors.push(field, &format!("“{}” 不是合法选项。", render_scalar(other)));
            Err(())
        }
    }
}

fn render_scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `CharField`（`required=True, allow_blank=False`）：缺 / `null` / `""` 三态各一条文案。
fn required_char_field(input: &Input, field: &str, errors: &mut FieldErrors) -> Result<String, ()> {
    match input {
        Input::Missing => {
            errors.push(field, ERR_REQUIRED);
            Err(())
        }
        Input::Null => {
            errors.push(field, ERR_NULL);
            Err(())
        }
        Input::Value(Value::String(text)) if text.trim().is_empty() => {
            errors.push(field, ERR_BLANK);
            Err(())
        }
        Input::Value(Value::String(text)) => Ok(text.trim().to_string()),
        Input::Value(_) => {
            errors.push(field, ERR_BLANK);
            Err(())
        }
    }
}

/// `BooleanField(required=False)`：缺 / `null` → 未提供；其它值按 DRF 的宽松转换。
fn optional_bool_field(
    input: &Input,
    field: &str,
    errors: &mut FieldErrors,
) -> Result<Option<bool>, ()> {
    match input {
        Input::Missing | Input::Null => Ok(None),
        Input::Value(Value::Bool(value)) => Ok(Some(*value)),
        Input::Value(Value::String(text)) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(Some(true)),
            "false" | "0" => Ok(Some(false)),
            _ => {
                errors.push(field, ERR_INVALID_BOOLEAN);
                Err(())
            }
        },
        Input::Value(_) => {
            errors.push(field, ERR_INVALID_BOOLEAN);
            Err(())
        }
    }
}

// --------------------------------------------------------------------------------------
// 枚举 → 展示文案（`models.py` 的 TextChoices，逐字对照夹具）
// --------------------------------------------------------------------------------------

/// 所有 `get_FOO_display()` 等价物：命中返回中文，未命中返回空串。
///
/// 蓝本对非法取值会返回原始值（`choices` 只在 Python 层校验，D7），列上还有 CHECK 约束，
/// 所以未命中分支实际到不了；这里不为它引入 `String` 返回值。
fn display(map: &[(&str, &'static str)], value: &str) -> &'static str {
    map.iter()
        .find(|(key, _)| *key == value)
        .map(|(_, label)| *label)
        .unwrap_or("")
}

fn sku_type_display(value: &str) -> &'static str {
    display(
        &[
            ("trial", "试吃装"),
            ("family", "家庭装"),
            ("gift", "礼赠装"),
            ("juice", "榨汁装"),
            ("enterprise", "企业装"),
            ("specialty", "特色果品"),
        ],
        value,
    )
}

fn orchard_status_display(value: &str) -> &'static str {
    display(
        &[
            ("draft", "待审核"),
            ("verified", "已认证"),
            ("inactive", "已停用"),
        ],
        value,
    )
}

fn tree_health_display(value: &str) -> &'static str {
    display(
        &[
            ("healthy", "生长良好"),
            ("watch", "持续观察"),
            ("maintenance", "养护中"),
        ],
        value,
    )
}

fn batch_status_display(value: &str) -> &'static str {
    display(
        &[
            ("draft", "筹备中"),
            ("warming", "即将上架"),
            ("open", "在售"),
            ("closed", "已停止销售"),
            ("harvesting", "采摘中"),
            ("fulfilling", "履约中"),
            ("completed", "已完成"),
            ("cancelled", "已取消"),
        ],
        value,
    )
}

fn payment_mode_display(value: &str) -> &'static str {
    display(
        &[("full", "全款购买"), ("deposit_balance", "订金加尾款")],
        value,
    )
}

fn fruit_condition_display(value: &str) -> &'static str {
    display(
        &[
            ("good", "状态良好"),
            ("watch", "需要复检"),
            ("rejected", "不进入销售"),
        ],
        value,
    )
}

fn inspection_stage_display(value: &str) -> &'static str {
    display(
        &[
            ("growing", "生长期巡检"),
            ("pre_harvest", "采摘前检查"),
            ("at_harvest", "采摘时检查"),
            ("post_sorting", "分选后检查"),
        ],
        value,
    )
}

fn sample_health_display(value: &str) -> &'static str {
    display(
        &[
            ("qualified", "健康达标"),
            ("watch", "持续观察"),
            ("rejected", "不合格"),
        ],
        value,
    )
}

fn event_type_display(value: &str) -> &'static str {
    display(
        &[
            ("orchard", "果园建档"),
            ("environment", "环境记录"),
            ("quality", "品质抽检"),
            ("harvest", "成熟采摘"),
            ("sorting", "分选称重"),
            ("packing", "装箱赋码"),
            ("shipping", "产地发货"),
            ("aftersale", "售后记录"),
        ],
        value,
    )
}

fn source_type_display(value: &str) -> &'static str {
    display(
        &[
            ("operator", "运营记录"),
            ("farmer", "果农记录"),
            ("sensor", "设备采集"),
            ("quality", "质检记录"),
            ("logistics", "物流记录"),
        ],
        value,
    )
}

fn order_status_display(value: &str) -> &'static str {
    display(
        &[
            ("pending_payment", "待支付"),
            ("pending_deposit", "待付订金"),
            ("pending_balance", "待付尾款"),
            ("paid", "待发货"),
            ("picking", "采摘分选中"),
            ("packed", "已装箱"),
            ("shipped", "待收货"),
            ("completed", "已完成"),
            ("after_sale", "售后处理中"),
            ("cancelled", "已取消"),
        ],
        value,
    )
}

fn package_status_display(value: &str) -> &'static str {
    display(
        &[
            ("created", "已赋码"),
            ("packed", "已装箱"),
            ("shipped", "运输中"),
            ("signed", "已签收"),
            ("after_sale", "售后处理中"),
        ],
        value,
    )
}

fn payment_stage_display(value: &str) -> &'static str {
    display(
        &[
            ("full", "全款"),
            ("deposit", "订金"),
            ("balance", "尾款"),
            ("refund", "退款"),
        ],
        value,
    )
}

fn payment_status_display(value: &str) -> &'static str {
    display(
        &[
            ("pending", "处理中"),
            ("succeeded", "支付成功"),
            ("failed", "支付失败"),
            ("refunded", "已退款"),
        ],
        value,
    )
}

fn after_sale_issue_display(value: &str) -> &'static str {
    display(
        &[
            ("damaged", "运输破损"),
            ("spoiled", "坏果腐烂"),
            ("weight", "重量争议"),
            ("logistics", "物流异常"),
            ("other", "其他问题"),
        ],
        value,
    )
}

fn after_sale_status_display(value: &str) -> &'static str {
    display(
        &[
            ("submitted", "已提交"),
            ("reviewing", "处理中"),
            ("resolved", "已解决"),
            ("rejected", "未通过"),
        ],
        value,
    )
}

/// Python `round()`（银行家舍入）的等价物：`.5` 时向偶数取整。
///
/// 蓝本 `progress_percent` 走 `round(sold * 100 / planned)`，而 Rust 的 `f64::round`
/// 是「四舍五入远离零」，两者在 `x.5` 上不同。
pub(crate) fn python_round(value: f64) -> i64 {
    let floor = value.floor();
    let fraction = value - floor;
    if (fraction - 0.5).abs() < 1e-9 {
        let even = floor as i64;
        return if even % 2 == 0 { even } else { even + 1 };
    }
    value.round() as i64
}

// --------------------------------------------------------------------------------------
// 查询串解析（`urlencode` 用的 `+` 与 `%XX` 都要还原）
// --------------------------------------------------------------------------------------

/// 解析 `?q=%E8%84%90%E6%A9%99&sku_type=family`；未知键忽略。
pub(crate) struct ListQuery {
    pub(crate) keyword: Option<String>,
    pub(crate) orchard_id: Option<String>,
    pub(crate) sku_type: Option<String>,
}

pub(crate) fn parse_list_query(raw: Option<&str>) -> ListQuery {
    let mut query = ListQuery {
        keyword: None,
        orchard_id: None,
        sku_type: None,
    };

    for pair in raw.unwrap_or_default().split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (key, value),
            None => (pair, ""),
        };
        let value = percent_decode(value).trim().to_string();
        if value.is_empty() {
            continue;
        }
        match key {
            "q" => query.keyword = Some(value),
            "orchard_id" => query.orchard_id = Some(value),
            "sku_type" => query.sku_type = Some(value),
            _ => {}
        }
    }

    query
}

/// `%XX` 与 `+`（表单式空格的 `quote_plus`）还原，非法转义原样保留。
pub(crate) fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                match hex.and_then(|text| u8::from_str_radix(text, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    None => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

// --------------------------------------------------------------------------------------
// 记录与序列化：Orchard（`OrchardPublicSerializer`）
// --------------------------------------------------------------------------------------

struct OrchardRecord {
    id: Uuid,
    code: String,
    name: String,
    grower_name: String,
    province: String,
    city: String,
    county: String,
    area_mu: Option<Decimal>,
    main_variety: String,
    tagline: String,
    feature_tags: Value,
    story: String,
    cover_image_url: String,
    gallery_urls: Value,
    video_urls: Value,
    certifications: Value,
    status: String,
    verification_note: String,
    verified_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
}

const ORCHARD_COLUMNS: &str = "o.id, o.code, o.name, o.grower_name, o.province, o.city, o.county, \
     o.area_mu, o.main_variety, o.tagline, o.feature_tags, o.story, o.cover_image_url, \
     o.gallery_urls, o.video_urls, o.certifications, o.status, o.verification_note, \
     o.verified_at, o.updated_at";

fn orchard_from_row(row: &sqlx::postgres::PgRow) -> Result<OrchardRecord, ApiReject> {
    Ok(OrchardRecord {
        id: row_get!(row, "id"),
        code: row_get!(row, "code"),
        name: row_get!(row, "name"),
        grower_name: row_get!(row, "grower_name"),
        province: row_get!(row, "province"),
        city: row_get!(row, "city"),
        county: row_get!(row, "county"),
        area_mu: row_get!(row, "area_mu"),
        main_variety: row_get!(row, "main_variety"),
        tagline: row_get!(row, "tagline"),
        feature_tags: row_get!(row, "feature_tags"),
        story: row_get!(row, "story"),
        cover_image_url: row_get!(row, "cover_image_url"),
        gallery_urls: row_get!(row, "gallery_urls"),
        video_urls: row_get!(row, "video_urls"),
        certifications: row_get!(row, "certifications"),
        status: row_get!(row, "status"),
        verification_note: row_get!(row, "verification_note"),
        verified_at: row_get!(row, "verified_at"),
        updated_at: row_get!(row, "updated_at"),
    })
}

impl OrchardRecord {
    /// `Orchard.origin_text` = `province + city + county`（**无分隔符**）。
    fn origin_text(&self) -> String {
        format!("{}{}{}", self.province, self.city, self.county)
    }
}

async fn load_orchard(pool: &PgPool, orchard_id: Uuid) -> Result<Option<OrchardRecord>, ApiReject> {
    let row = sqlx::query(&format!(
        "SELECT {ORCHARD_COLUMNS} FROM orchard o WHERE o.id = $1"
    ))
    .bind(orchard_id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    row.as_ref().map(orchard_from_row).transpose()
}

/// `OrchardPublicSerializer`：字段序即蓝本声明序。
async fn orchard_json(pool: &PgPool, orchard: &OrchardRecord) -> Result<Value, ApiReject> {
    let product_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM citrus_product p JOIN sales_batch b ON b.id = p.sales_batch_id \
         WHERE b.orchard_id = $1 AND p.status = 'on_sale'",
    )
    .bind(orchard.id)
    .fetch_one(pool)
    .await
    .map_err(internal_error)?;

    let tree_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fruit_tree_archive WHERE orchard_id = $1")
            .bind(orchard.id)
            .fetch_one(pool)
            .await
            .map_err(internal_error)?;

    // 蓝本 `values_list('sku_type').distinct()` 无 `order_by`，但 `Meta.ordering`
    // （`sort_order, -created_at`）会漏进 SQL，实测等价于「按 created_at DESC 取首个
    // 出现的 sku_type」。这里显式按同序去重（D6：我方定序输出）。
    let sku_rows = sqlx::query(
        "SELECT p.sku_type FROM citrus_product p JOIN sales_batch b ON b.id = p.sales_batch_id \
         WHERE b.orchard_id = $1 AND p.status = 'on_sale' ORDER BY p.created_at DESC, p.id ASC",
    )
    .bind(orchard.id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let mut labels: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for row in &sku_rows {
        let sku_type: String = row_get!(row, "sku_type");
        if seen.contains(&sku_type) {
            continue;
        }
        let label = sku_type_display(&sku_type);
        seen.push(sku_type);
        labels.push(Value::String(label.to_string()));
    }

    let primary_trace_code: Option<String> = sqlx::query_scalar(
        "SELECT trace_code FROM sales_batch WHERE orchard_id = $1 \
         AND status NOT IN ('cancelled', 'draft') \
         ORDER BY is_featured DESC, open_at DESC, created_at DESC LIMIT 1",
    )
    .bind(orchard.id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    let mut payload = Map::new();
    payload.insert("id".into(), Value::String(orchard.id.to_string()));
    payload.insert("code".into(), Value::String(orchard.code.clone()));
    payload.insert("name".into(), Value::String(orchard.name.clone()));
    payload.insert(
        "grower_name".into(),
        Value::String(orchard.grower_name.clone()),
    );
    payload.insert("origin".into(), Value::String(orchard.origin_text()));
    payload.insert("county".into(), Value::String(orchard.county.clone()));
    payload.insert(
        "area_mu".into(),
        ser::opt_dec_scaled(orchard.area_mu, 2).map_or(Value::Null, Value::String),
    );
    payload.insert(
        "main_variety".into(),
        Value::String(orchard.main_variety.clone()),
    );
    payload.insert("tagline".into(), Value::String(orchard.tagline.clone()));
    payload.insert("feature_tags".into(), orchard.feature_tags.clone());
    payload.insert("story".into(), Value::String(orchard.story.clone()));
    payload.insert(
        "cover_image_url".into(),
        Value::String(orchard.cover_image_url.clone()),
    );
    payload.insert("gallery_urls".into(), orchard.gallery_urls.clone());
    payload.insert("video_urls".into(), orchard.video_urls.clone());
    payload.insert("certifications".into(), orchard.certifications.clone());
    payload.insert("status".into(), Value::String(orchard.status.clone()));
    payload.insert(
        "status_display".into(),
        Value::String(orchard_status_display(&orchard.status).to_string()),
    );
    payload.insert(
        "is_verified".into(),
        Value::Bool(orchard.status == "verified"),
    );
    payload.insert(
        "verification_note".into(),
        Value::String(orchard.verification_note.clone()),
    );
    payload.insert(
        "verified_at".into(),
        ser::opt_dt_z(orchard.verified_at).map_or(Value::Null, Value::String),
    );
    payload.insert("product_count".into(), json!(product_count));
    payload.insert("tree_count".into(), json!(tree_count));
    payload.insert("category_labels".into(), Value::Array(labels));
    payload.insert(
        "primary_trace_code".into(),
        Value::String(primary_trace_code.unwrap_or_default()),
    );

    Ok(Value::Object(payload))
}

// --------------------------------------------------------------------------------------
// 记录与序列化：SalesBatch（`SalesBatchSummarySerializer`）
// --------------------------------------------------------------------------------------

struct BatchRecord {
    id: Uuid,
    code: String,
    trace_code: String,
    title: String,
    subtitle: String,
    status: String,
    planned_quantity: i32,
    sold_quantity: i32,
    open_at: Option<DateTime<Utc>>,
    close_at: Option<DateTime<Utc>>,
    expected_harvest_start: Option<NaiveDate>,
    expected_harvest_end: Option<NaiveDate>,
    expected_ship_start: Option<NaiveDate>,
    expected_ship_end: Option<NaiveDate>,
    maturity_standard: String,
    quality_commitment: String,
    natural_variation_note: String,
    aftersale_policy: String,
    cover_image_url: String,
    live_image_urls: Value,
    environment_summary: Value,
    payment_mode: String,
    deposit_ratio: Decimal,
    orchard_id: Uuid,
    orchard: Value,
    updated_at: DateTime<Utc>,
}

const BATCH_COLUMNS: &str = "b.id, b.code, b.trace_code, b.title, b.subtitle, b.status, \
     b.planned_quantity, b.sold_quantity, b.open_at, b.close_at, b.expected_harvest_start, \
     b.expected_harvest_end, b.expected_ship_start, b.expected_ship_end, b.maturity_standard, \
     b.quality_commitment, b.natural_variation_note, b.aftersale_policy, b.cover_image_url, \
     b.live_image_urls, b.environment_summary, b.payment_mode, b.deposit_ratio, b.orchard_id, \
     b.updated_at";

fn batch_from_row(row: &sqlx::postgres::PgRow, orchard: Value) -> Result<BatchRecord, ApiReject> {
    Ok(BatchRecord {
        id: row_get!(row, "id"),
        code: row_get!(row, "code"),
        trace_code: row_get!(row, "trace_code"),
        title: row_get!(row, "title"),
        subtitle: row_get!(row, "subtitle"),
        status: row_get!(row, "status"),
        planned_quantity: row_get!(row, "planned_quantity"),
        sold_quantity: row_get!(row, "sold_quantity"),
        open_at: row_get!(row, "open_at"),
        close_at: row_get!(row, "close_at"),
        expected_harvest_start: row_get!(row, "expected_harvest_start"),
        expected_harvest_end: row_get!(row, "expected_harvest_end"),
        expected_ship_start: row_get!(row, "expected_ship_start"),
        expected_ship_end: row_get!(row, "expected_ship_end"),
        maturity_standard: row_get!(row, "maturity_standard"),
        quality_commitment: row_get!(row, "quality_commitment"),
        natural_variation_note: row_get!(row, "natural_variation_note"),
        aftersale_policy: row_get!(row, "aftersale_policy"),
        cover_image_url: row_get!(row, "cover_image_url"),
        live_image_urls: row_get!(row, "live_image_urls"),
        environment_summary: row_get!(row, "environment_summary"),
        payment_mode: row_get!(row, "payment_mode"),
        deposit_ratio: row_get!(row, "deposit_ratio"),
        orchard_id: row_get!(row, "orchard_id"),
        orchard,
        updated_at: row_get!(row, "updated_at"),
    })
}

impl BatchRecord {
    /// `SalesBatch.available_quantity` = `max(planned - sold, 0)`。
    fn available_quantity(&self) -> i32 {
        (self.planned_quantity - self.sold_quantity).max(0)
    }

    /// `SalesBatch.is_open`：在售 + 已到上架时间 + 未过停售时间 + 还有可售量。
    fn is_open(&self, now: DateTime<Utc>) -> bool {
        self.status == "open"
            && self.open_at.is_none_or(|value| value <= now)
            && self.close_at.is_none_or(|value| value > now)
            && self.available_quantity() > 0
    }

    fn payload(&self, now: DateTime<Utc>) -> Value {
        let mut payload = Map::new();
        payload.insert("id".into(), Value::String(self.id.to_string()));
        payload.insert("code".into(), Value::String(self.code.clone()));
        payload.insert("trace_code".into(), Value::String(self.trace_code.clone()));
        payload.insert("title".into(), Value::String(self.title.clone()));
        payload.insert("subtitle".into(), Value::String(self.subtitle.clone()));
        payload.insert("status".into(), Value::String(self.status.clone()));
        payload.insert(
            "status_display".into(),
            Value::String(batch_status_display(&self.status).to_string()),
        );
        payload.insert("planned_quantity".into(), json!(self.planned_quantity));
        payload.insert("sold_quantity".into(), json!(self.sold_quantity));
        payload.insert(
            "available_quantity".into(),
            json!(self.available_quantity()),
        );
        payload.insert("progress_percent".into(), json!(progress_percent(self)));
        payload.insert("is_open".into(), Value::Bool(self.is_open(now)));
        payload.insert(
            "open_at".into(),
            ser::opt_dt_z(self.open_at).map_or(Value::Null, Value::String),
        );
        payload.insert(
            "close_at".into(),
            ser::opt_dt_z(self.close_at).map_or(Value::Null, Value::String),
        );
        payload.insert(
            "expected_harvest_start".into(),
            ser::opt_date(self.expected_harvest_start).map_or(Value::Null, Value::String),
        );
        payload.insert(
            "expected_harvest_end".into(),
            ser::opt_date(self.expected_harvest_end).map_or(Value::Null, Value::String),
        );
        payload.insert(
            "expected_ship_start".into(),
            ser::opt_date(self.expected_ship_start).map_or(Value::Null, Value::String),
        );
        payload.insert(
            "expected_ship_end".into(),
            ser::opt_date(self.expected_ship_end).map_or(Value::Null, Value::String),
        );
        payload.insert(
            "maturity_standard".into(),
            Value::String(self.maturity_standard.clone()),
        );
        payload.insert(
            "quality_commitment".into(),
            Value::String(self.quality_commitment.clone()),
        );
        payload.insert(
            "natural_variation_note".into(),
            Value::String(self.natural_variation_note.clone()),
        );
        payload.insert(
            "aftersale_policy".into(),
            Value::String(self.aftersale_policy.clone()),
        );
        payload.insert(
            "cover_image_url".into(),
            Value::String(self.cover_image_url.clone()),
        );
        payload.insert("live_image_urls".into(), self.live_image_urls.clone());
        payload.insert(
            "environment_summary".into(),
            self.environment_summary.clone(),
        );
        payload.insert(
            "payment_mode".into(),
            Value::String(self.payment_mode.clone()),
        );
        payload.insert(
            "payment_mode_display".into(),
            Value::String(payment_mode_display(&self.payment_mode).to_string()),
        );
        payload.insert(
            "deposit_ratio".into(),
            Value::String(ser::dec_scaled(self.deposit_ratio, 2)),
        );
        payload.insert("orchard".into(), self.orchard.clone());

        Value::Object(payload)
    }
}

/// `SalesBatchSummarySerializer.get_progress_percent`：`min(round(...), 100)`。
fn progress_percent(batch: &BatchRecord) -> i64 {
    if batch.planned_quantity <= 0 {
        return 0;
    }
    let ratio = f64::from(batch.sold_quantity) * 100.0 / f64::from(batch.planned_quantity);
    python_round(ratio).min(100)
}

async fn load_batch(pool: &PgPool, batch_id: Uuid) -> Result<Option<BatchRecord>, ApiReject> {
    let row = sqlx::query(&format!(
        "SELECT {BATCH_COLUMNS} FROM sales_batch b WHERE b.id = $1"
    ))
    .bind(batch_id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    let Some(row) = row else {
        return Ok(None);
    };
    let orchard_id: Uuid = row_get!(&row, "orchard_id");
    let orchard = match load_orchard(pool, orchard_id).await? {
        Some(orchard) => orchard_json(pool, &orchard).await?,
        // 蓝本外键必填，正常查不到；真查不到也不能让整个响应炸掉。
        None => Value::Null,
    };

    Ok(Some(batch_from_row(&row, orchard)?))
}

// --------------------------------------------------------------------------------------
// 记录与序列化：CitrusProduct（`CitrusProductSerializer`）
// --------------------------------------------------------------------------------------

struct ProductRecord {
    id: Uuid,
    sales_batch_id: Option<Uuid>,
    name: String,
    sku_type: String,
    fruit_type: String,
    variety: String,
    origin: String,
    description: String,
    price: Decimal,
    unit: String,
    stock: i32,
    sweetness: Option<Decimal>,
    grade: String,
    harvest_date: Option<NaiveDate>,
    shipping_note: String,
    cover_image_url: String,
    purchase_limit: i32,
    minimum_order_quantity: i32,
    status: String,
    seller_name: String,
}

/// 商品列清单。**不含** `seller_name`：下单流程要给商品行加 `FOR UPDATE`，而
/// `FOR UPDATE` 不能落在外连接的可空侧，所以 `seller_name` 由各查询自己补。
const PRODUCT_ROW_COLUMNS: &str = "p.id, p.sales_batch_id, p.name, p.sku_type, p.fruit_type, \
     p.variety, p.origin, p.description, p.price, p.unit, p.stock, p.sweetness, p.grade, \
     p.harvest_date, p.shipping_note, p.cover_image_url, p.purchase_limit, \
     p.minimum_order_quantity, p.status";

/// `CitrusProductSerializer.seller_name`（`source='seller.username'`，无卖家时空串）。
const SELLER_NAME_COLUMN: &str = "COALESCE(u.username, '') AS seller_name";

struct ImageRecord {
    id: Uuid,
    image_url: String,
    sort_order: i32,
}

fn product_from_row(row: &sqlx::postgres::PgRow) -> Result<ProductRecord, ApiReject> {
    Ok(ProductRecord {
        id: row_get!(row, "id"),
        sales_batch_id: row_get!(row, "sales_batch_id"),
        name: row_get!(row, "name"),
        sku_type: row_get!(row, "sku_type"),
        fruit_type: row_get!(row, "fruit_type"),
        variety: row_get!(row, "variety"),
        origin: row_get!(row, "origin"),
        description: row_get!(row, "description"),
        price: row_get!(row, "price"),
        unit: row_get!(row, "unit"),
        stock: row_get!(row, "stock"),
        sweetness: row_get!(row, "sweetness"),
        grade: row_get!(row, "grade"),
        harvest_date: row_get!(row, "harvest_date"),
        shipping_note: row_get!(row, "shipping_note"),
        cover_image_url: row_get!(row, "cover_image_url"),
        purchase_limit: row_get!(row, "purchase_limit"),
        minimum_order_quantity: row_get!(row, "minimum_order_quantity"),
        status: row_get!(row, "status"),
        seller_name: row_get!(row, "seller_name"),
    })
}

async fn load_product(pool: &PgPool, product_id: Uuid) -> Result<Option<ProductRecord>, ApiReject> {
    let row = sqlx::query(&format!(
        "SELECT {PRODUCT_ROW_COLUMNS}, {SELLER_NAME_COLUMN} FROM citrus_product p \
         LEFT JOIN \"user\" u ON u.id = p.seller_id WHERE p.id = $1"
    ))
    .bind(product_id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    row.as_ref().map(product_from_row).transpose()
}

async fn load_images(pool: &PgPool, product_id: Uuid) -> Result<Vec<ImageRecord>, ApiReject> {
    let rows = sqlx::query(
        "SELECT id, image_url, sort_order FROM product_image WHERE product_id = $1 \
         ORDER BY sort_order ASC, id ASC",
    )
    .bind(product_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    rows.iter()
        .map(|row| {
            Ok(ImageRecord {
                id: row_get!(row, "id"),
                image_url: row_get!(row, "image_url"),
                sort_order: row_get!(row, "sort_order"),
            })
        })
        .collect()
}

/// `CitrusProductSerializer`。`sales_batch` 由调用方按需装配（订单里的 `batch_code` 等
/// 快照字段不需要它）。
async fn product_json(pool: &PgPool, product: &ProductRecord) -> Result<Value, ApiReject> {
    let batch = match product.sales_batch_id {
        Some(batch_id) => load_batch(pool, batch_id).await?,
        None => None,
    };
    let images = load_images(pool, product.id).await?;
    Ok(product_payload(product, &images, batch.as_ref()))
}

fn product_payload(
    product: &ProductRecord,
    images: &[ImageRecord],
    batch: Option<&BatchRecord>,
) -> Value {
    let now = now();
    let is_available = product.status == "on_sale"
        && product.stock > 0
        && batch.is_none_or(|batch| batch.is_open(now));

    let mut payload = Map::new();
    payload.insert("id".into(), Value::String(product.id.to_string()));
    payload.insert("name".into(), Value::String(product.name.clone()));
    payload.insert("sku_type".into(), Value::String(product.sku_type.clone()));
    payload.insert(
        "sku_type_display".into(),
        Value::String(sku_type_display(&product.sku_type).to_string()),
    );
    payload.insert(
        "fruit_type".into(),
        Value::String(product.fruit_type.clone()),
    );
    payload.insert("variety".into(), Value::String(product.variety.clone()));
    payload.insert("origin".into(), Value::String(product.origin.clone()));
    payload.insert(
        "description".into(),
        Value::String(product.description.clone()),
    );
    payload.insert(
        "price".into(),
        Value::String(ser::dec_scaled(product.price, 2)),
    );
    payload.insert("unit".into(), Value::String(product.unit.clone()));
    payload.insert("stock".into(), json!(product.stock));
    payload.insert(
        "sweetness".into(),
        ser::opt_dec_scaled(product.sweetness, 1).map_or(Value::Null, Value::String),
    );
    payload.insert("grade".into(), Value::String(product.grade.clone()));
    payload.insert(
        "harvest_date".into(),
        ser::opt_date(product.harvest_date).map_or(Value::Null, Value::String),
    );
    payload.insert(
        "shipping_note".into(),
        Value::String(product.shipping_note.clone()),
    );
    payload.insert(
        "cover_image_url".into(),
        Value::String(product.cover_image_url.clone()),
    );
    payload.insert("purchase_limit".into(), json!(product.purchase_limit));
    payload.insert(
        "minimum_order_quantity".into(),
        json!(product.minimum_order_quantity),
    );
    payload.insert("status".into(), Value::String(product.status.clone()));
    payload.insert(
        "seller_name".into(),
        Value::String(product.seller_name.clone()),
    );
    payload.insert(
        "sales_batch".into(),
        batch.map_or(Value::Null, |batch| batch.payload(now)),
    );
    payload.insert(
        "images".into(),
        Value::Array(
            images
                .iter()
                .map(|image| {
                    json!({
                        "id": image.id.to_string(),
                        "image_url": image.image_url,
                        "sort_order": image.sort_order,
                    })
                })
                .collect(),
        ),
    );
    payload.insert("is_available".into(), Value::Bool(is_available));

    Value::Object(payload)
}

// --------------------------------------------------------------------------------------
// GET /api/products 与 /api/product/<uuid>、/health-archive
// --------------------------------------------------------------------------------------

pub(crate) async fn product_list_impl(pool: &PgPool, query: Option<&str>) -> ApiResult {
    let filter = parse_list_query(query);
    let products = load_product_list(pool, &filter).await?;

    let mut items = Vec::with_capacity(products.len());
    for product in &products {
        items.push(product_json(pool, product).await?);
    }

    let count = items.len();
    Ok(api_ok(json!({"items": items, "count": count})))
}

async fn load_product_list(
    pool: &PgPool,
    filter: &ListQuery,
) -> Result<Vec<ProductRecord>, ApiReject> {
    let mut builder: QueryBuilder<Postgres> = QueryBuilder::new(format!(
        "SELECT {PRODUCT_ROW_COLUMNS}, {SELLER_NAME_COLUMN} FROM citrus_product p \
         LEFT JOIN \"user\" u ON u.id = p.seller_id \
         LEFT JOIN sales_batch b ON b.id = p.sales_batch_id \
         LEFT JOIN orchard o ON o.id = b.orchard_id \
         WHERE p.status = 'on_sale' \
         AND (p.sales_batch_id IS NULL OR b.status IN ('warming', 'open'))"
    ));

    if let Some(orchard_id) = &filter.orchard_id {
        // 蓝本会把非 UUID 直接抛给数据库 -> ValidationError 冒泡成 500。
        let parsed = Uuid::parse_str(orchard_id).map_err(|_| {
            ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
        })?;
        builder.push(" AND b.orchard_id = ").push_bind(parsed);
    }
    if let Some(sku_type) = &filter.sku_type {
        builder
            .push(" AND p.sku_type = ")
            .push_bind(sku_type.clone());
    }
    if let Some(keyword) = &filter.keyword {
        let pattern = format!("%{keyword}%");
        builder
            .push(" AND (p.name ILIKE ")
            .push_bind(pattern.clone());
        builder
            .push(" OR p.variety ILIKE ")
            .push_bind(pattern.clone());
        builder
            .push(" OR p.origin ILIKE ")
            .push_bind(pattern.clone());
        builder
            .push(" OR b.title ILIKE ")
            .push_bind(pattern.clone());
        builder
            .push(" OR o.name ILIKE ")
            .push_bind(pattern)
            .push(")");
    }

    builder.push(" ORDER BY p.sort_order ASC, p.created_at DESC, p.id ASC");

    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(internal_error)?;
    rows.iter().map(product_from_row).collect()
}

pub(crate) async fn product_detail_impl(pool: &PgPool, product_id: &str) -> ApiResult {
    let product = match load_on_sale_product(pool, product_id).await? {
        Some(product) => product,
        None => return Ok(business_error(StatusCode::NOT_FOUND, ERR_PRODUCT_NOT_FOUND)),
    };

    Ok(api_ok(product_json(pool, &product).await?))
}

/// `product_detail_api` / `product_health_archive_api` 共用的取数：必须 `on_sale`。
async fn load_on_sale_product(
    pool: &PgPool,
    product_id: &str,
) -> Result<Option<ProductRecord>, ApiReject> {
    let Ok(product_id) = Uuid::parse_str(product_id.trim()) else {
        return Ok(None);
    };

    let product = load_product(pool, product_id).await?;
    Ok(product.filter(|product| product.status == "on_sale"))
}

pub(crate) async fn product_health_archive_impl(pool: &PgPool, product_id: &str) -> ApiResult {
    let Ok(product_id) = Uuid::parse_str(product_id.trim()) else {
        return Ok(business_error(
            StatusCode::NOT_FOUND,
            ERR_PRODUCT_HEALTH_NOT_FOUND,
        ));
    };

    let product = load_product(pool, product_id).await?;
    let Some(product) =
        product.filter(|product| product.status == "on_sale" && product.sales_batch_id.is_some())
    else {
        return Ok(business_error(
            StatusCode::NOT_FOUND,
            ERR_PRODUCT_HEALTH_NOT_FOUND,
        ));
    };

    Ok(api_ok(health_payload(pool, &product).await?))
}

/// `_product_health_payload`：128 行聚合逻辑，逐字段对齐夹具。
///
/// 顺序敏感的三处查询：
/// - 采摘档案 `-harvested_at, -created_at`；
/// - 品质抽检 `-sampled_at, -created_at`，且**只看本商品或未挂商品的记录**；
/// - 果树按 `Meta.ordering = ['-is_featured', 'tree_number']`。
///
/// 时间形态：`updatedAt` / `orchardHealth.verifiedAt` 是手工拼装的 Python `isoformat()`
/// → `+00:00`；其余 `DateTimeField` 走 DRF `JSONEncoder` → `Z`。
async fn health_payload(pool: &PgPool, product: &ProductRecord) -> Result<Value, ApiReject> {
    let batch = match product.sales_batch_id {
        Some(batch_id) => load_batch(pool, batch_id).await?,
        None => None,
    };
    let batch = batch.ok_or_else(|| ApiReject::not_found(ERR_PRODUCT_HEALTH_NOT_FOUND))?;
    let orchard = load_orchard(pool, batch.orchard_id)
        .await?
        .ok_or_else(|| ApiReject::not_found(ERR_PRODUCT_HEALTH_NOT_FOUND))?;

    let harvest_rows = sqlx::query(
        "SELECT h.id, h.harvest_code, h.harvested_at, h.picker, h.plot_name, h.harvest_method, \
                h.quantity_kg, h.maturity_brix, h.grade, h.pre_harvest_status, h.harvest_weather, \
                h.fruit_condition, h.appearance_note, h.pest_status, h.damage_rate_percent, \
                h.summary, h.image_urls, h.video_urls, h.tree_id, h.created_at, h.updated_at, \
                COALESCE(t.tree_number, '') AS tree_number, \
                COALESCE(t.trace_code, '') AS tree_trace_code \
           FROM harvest_archive h \
           LEFT JOIN fruit_tree_archive t ON t.id = h.tree_id \
          WHERE h.batch_id = $1 \
          ORDER BY h.harvested_at DESC, h.created_at DESC, h.id ASC",
    )
    .bind(batch.id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let quality_rows = sqlx::query(
        "SELECT s.id, s.product_id, COALESCE(p.name, '') AS product_name, s.harvest_archive_id, \
                COALESCE(h.harvest_code, '') AS harvest_code, s.sampled_at, s.inspection_stage, \
                s.health_status, s.sample_size, s.sweetness_brix, s.acidity, \
                s.average_weight_grams, s.diameter_mm, s.grade, s.result, s.appearance_status, \
                s.pest_status, s.pesticide_residue_result, s.inspector, s.report_image_url, s.note \
           FROM batch_quality_sample s \
           LEFT JOIN citrus_product p ON p.id = s.product_id \
           LEFT JOIN harvest_archive h ON h.id = s.harvest_archive_id \
          WHERE s.batch_id = $1 AND (s.product_id = $2 OR s.product_id IS NULL) \
          ORDER BY s.sampled_at DESC, s.created_at DESC, s.id ASC",
    )
    .bind(batch.id)
    .bind(product.id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let tree_rows = sqlx::query(
        "SELECT id, trace_code, tree_number, plot_name, variety, planted_year, growth_stage, \
                health_status, growth_summary, latest_temperature, latest_humidity, \
                last_observed_at, cover_image_url, image_urls, video_urls, is_featured, updated_at \
           FROM fruit_tree_archive WHERE orchard_id = $1 \
          ORDER BY is_featured DESC, tree_number ASC, id ASC",
    )
    .bind(orchard.id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let event_rows = sqlx::query(
        "SELECT id, event_type, source_type, occurred_at, title, description, location, actor, \
                source_reference, image_urls, data, previous_hash, evidence_hash, recorded_at \
           FROM trace_event \
          WHERE batch_id = $1 \
            AND event_type IN ('orchard', 'environment', 'quality', 'harvest', 'sorting') \
          ORDER BY occurred_at ASC, recorded_at ASC, id ASC",
    )
    .bind(batch.id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    // ---- 归档状态：不合格 > 待复检 > 已核验 > 待检查 --------------------------------
    let mut has_rejected = false;
    let mut needs_watch = false;
    for row in &quality_rows {
        let health_status: String = row_get!(row, "health_status");
        has_rejected |= health_status == "rejected";
        needs_watch |= health_status == "watch";
    }
    for row in &harvest_rows {
        let fruit_condition: String = row_get!(row, "fruit_condition");
        has_rejected |= fruit_condition == "rejected";
        needs_watch |= fruit_condition == "watch";
    }

    let (archive_status, archive_status_display) = if has_rejected {
        ("rejected", "存在不合格记录")
    } else if needs_watch {
        ("watch", "存在待复检记录")
    } else if !quality_rows.is_empty() {
        ("qualified", "健康档案已核验")
    } else {
        ("pending", "等待商品健康检查")
    };

    let (harvest_status, harvest_status_display) = if !harvest_rows.is_empty() {
        ("recorded", "已采摘并完成状态记录")
    } else if batch.status == "harvesting" {
        ("harvesting", "采摘进行中")
    } else {
        ("planned", "等待达到采摘标准")
    };

    let healthy_tree_count = tree_rows
        .iter()
        .filter(|row| {
            row.try_get::<String, _>("health_status")
                .map(|value| value == "healthy")
                .unwrap_or(false)
        })
        .count();

    // `max(updated_candidates)`：商品 / 果园 / 批次 / 每棵树 / 每份采摘档案 / 每条抽检采样时刻。
    let mut updated_candidates = vec![
        product_updated_at(pool, product.id).await?,
        orchard.updated_at,
        batch.updated_at,
    ];
    for row in &tree_rows {
        updated_candidates.push(row_get!(row, "updated_at"));
    }
    for row in &harvest_rows {
        updated_candidates.push(row_get!(row, "updated_at"));
    }
    for row in &quality_rows {
        updated_candidates.push(row_get!(row, "sampled_at"));
    }
    let updated_at = updated_candidates.into_iter().max().unwrap_or_else(now);

    let latest_observed_at: Option<DateTime<Utc>> = tree_rows
        .iter()
        .filter_map(|row| {
            row.try_get::<Option<DateTime<Utc>>, _>("last_observed_at")
                .ok()
                .flatten()
        })
        .max();

    let orchard_verified = orchard.status == "verified";
    let fruit_trees: Vec<Value> = tree_rows.iter().map(tree_json).collect::<Result<_, _>>()?;
    let harvest_archives: Vec<Value> = harvest_rows
        .iter()
        .map(harvest_archive_json)
        .collect::<Result<_, _>>()?;
    let quality_records: Vec<Value> = quality_rows
        .iter()
        .map(quality_sample_json)
        .collect::<Result<_, _>>()?;
    let health_events: Vec<Value> = event_rows
        .iter()
        .map(trace_event_json)
        .collect::<Result<_, _>>()?;

    let mut payload = Map::new();
    payload.insert(
        "archiveCode".into(),
        Value::String(format!(
            "CGJ-HEALTH-{}",
            product.id.simple().to_string()[..10].to_uppercase()
        )),
    );
    payload.insert("status".into(), Value::String(archive_status.into()));
    payload.insert(
        "statusDisplay".into(),
        Value::String(archive_status_display.into()),
    );
    payload.insert(
        "updatedAt".into(),
        Value::String(ser::dt_offset(updated_at)),
    );
    payload.insert("product".into(), product_json(pool, product).await?);
    payload.insert("orchard".into(), orchard_json(pool, &orchard).await?);
    payload.insert("batch".into(), batch.payload(now()));
    payload.insert(
        "orchardHealth".into(),
        json!({
            "status": if orchard_verified { "verified" } else { "pending" },
            "statusDisplay": if orchard_verified { "果园主体与产地档案已核验" } else { "果园档案待核验" },
            "archiveCode": orchard.code,
            "verifiedAt": ser::opt_dt_offset(orchard.verified_at).map_or(Value::Null, Value::String),
            "verificationNote": orchard.verification_note,
            "environment": batch.environment_summary,
            "certifications": orchard.certifications,
        }),
    );
    payload.insert(
        "treeHealthSummary".into(),
        json!({
            "total": tree_rows.len(),
            "healthy": healthy_tree_count,
            "needsAttention": tree_rows.len() - healthy_tree_count,
            "latestObservedAt": ser::opt_dt_z(latest_observed_at).map_or(Value::Null, Value::String),
        }),
    );
    payload.insert(
        "harvestStatus".into(),
        json!({
            "status": harvest_status,
            "statusDisplay": harvest_status_display,
            "recordCount": harvest_rows.len(),
            "expectedWindow": {
                "start": ser::opt_date(batch.expected_harvest_start).map_or(Value::Null, Value::String),
                "end": ser::opt_date(batch.expected_harvest_end).map_or(Value::Null, Value::String),
            },
            "latestRecord": harvest_archives.first().cloned().unwrap_or(Value::Null),
        }),
    );
    payload.insert(
        "qualitySummary".into(),
        json!({
            "recordCount": quality_rows.len(),
            "latestRecord": quality_records.first().cloned().unwrap_or(Value::Null),
        }),
    );
    payload.insert("fruitTrees".into(), Value::Array(fruit_trees));
    payload.insert("harvestArchives".into(), Value::Array(harvest_archives));
    payload.insert("qualityRecords".into(), Value::Array(quality_records));
    payload.insert("healthEvents".into(), Value::Array(health_events));

    Ok(Value::Object(payload))
}

async fn product_updated_at(pool: &PgPool, product_id: Uuid) -> Result<DateTime<Utc>, ApiReject> {
    let value: DateTime<Utc> =
        sqlx::query_scalar("SELECT updated_at FROM citrus_product WHERE id = $1")
            .bind(product_id)
            .fetch_one(pool)
            .await
            .map_err(internal_error)?;

    Ok(value)
}

fn harvest_archive_json(row: &sqlx::postgres::PgRow) -> Result<Value, ApiReject> {
    let fruit_condition = text(row, "fruit_condition")?;

    Ok(json!({
        "id": uuid_text(row, "id")?,
        "harvest_code": text(row, "harvest_code")?,
        "harvested_at": dt_json(row, "harvested_at")?,
        "picker": text(row, "picker")?,
        "plot_name": text(row, "plot_name")?,
        "harvest_method": text(row, "harvest_method")?,
        "quantity_kg": dec_at(row, "quantity_kg", 2)?,
        "maturity_brix": opt_dec_at(row, "maturity_brix", 1)?,
        "grade": text(row, "grade")?,
        "pre_harvest_status": text(row, "pre_harvest_status")?,
        "harvest_weather": text(row, "harvest_weather")?,
        "fruit_condition": fruit_condition,
        "fruit_condition_display": fruit_condition_display(&fruit_condition),
        "appearance_note": text(row, "appearance_note")?,
        "pest_status": text(row, "pest_status")?,
        "damage_rate_percent": opt_dec_at(row, "damage_rate_percent", 2)?,
        "summary": text(row, "summary")?,
        "image_urls": json_value(row, "image_urls")?,
        "video_urls": json_value(row, "video_urls")?,
        "tree": opt_pk_json(row, "tree_id")?,
        "tree_number": text(row, "tree_number")?,
        "tree_trace_code": text(row, "tree_trace_code")?,
        "created_at": dt_json(row, "created_at")?,
    }))
}

fn quality_sample_json(row: &sqlx::postgres::PgRow) -> Result<Value, ApiReject> {
    let inspection_stage = text(row, "inspection_stage")?;
    let health_status = text(row, "health_status")?;

    Ok(json!({
        "id": uuid_text(row, "id")?,
        "product": opt_pk_json(row, "product_id")?,
        "product_name": text(row, "product_name")?,
        "harvest_archive": opt_pk_json(row, "harvest_archive_id")?,
        "harvest_code": text(row, "harvest_code")?,
        "sampled_at": dt_json(row, "sampled_at")?,
        "inspection_stage": inspection_stage,
        "inspection_stage_display": inspection_stage_display(&inspection_stage),
        "health_status": health_status,
        "health_status_display": sample_health_display(&health_status),
        "sample_size": int(row, "sample_size")?,
        "sweetness_brix": opt_dec_at(row, "sweetness_brix", 1)?,
        "acidity": opt_dec_at(row, "acidity", 2)?,
        "average_weight_grams": opt_int(row, "average_weight_grams")?,
        "diameter_mm": opt_dec_at(row, "diameter_mm", 1)?,
        "grade": text(row, "grade")?,
        "result": text(row, "result")?,
        "appearance_status": text(row, "appearance_status")?,
        "pest_status": text(row, "pest_status")?,
        "pesticide_residue_result": text(row, "pesticide_residue_result")?,
        "inspector": text(row, "inspector")?,
        "report_image_url": text(row, "report_image_url")?,
        "note": text(row, "note")?,
    }))
}

fn tree_json(row: &sqlx::postgres::PgRow) -> Result<Value, ApiReject> {
    let health_status = text(row, "health_status")?;

    Ok(json!({
        "id": uuid_text(row, "id")?,
        "trace_code": text(row, "trace_code")?,
        "tree_number": text(row, "tree_number")?,
        "plot_name": text(row, "plot_name")?,
        "variety": text(row, "variety")?,
        "planted_year": opt_int(row, "planted_year")?,
        "growth_stage": text(row, "growth_stage")?,
        "health_status": health_status,
        "health_status_display": tree_health_display(&health_status),
        "growth_summary": text(row, "growth_summary")?,
        "latest_temperature": opt_dec_at(row, "latest_temperature", 1)?,
        "latest_humidity": opt_dec_at(row, "latest_humidity", 1)?,
        "last_observed_at": opt_dt_json(row, "last_observed_at")?,
        "cover_image_url": text(row, "cover_image_url")?,
        "image_urls": json_value(row, "image_urls")?,
        "video_urls": json_value(row, "video_urls")?,
        "is_featured": flag(row, "is_featured")?,
        "updated_at": dt_json(row, "updated_at")?,
    }))
}

fn trace_event_json(row: &sqlx::postgres::PgRow) -> Result<Value, ApiReject> {
    let event_type = text(row, "event_type")?;
    let source_type = text(row, "source_type")?;
    let evidence_hash = text(row, "evidence_hash")?;

    Ok(json!({
        "id": uuid_text(row, "id")?,
        "event_type": event_type,
        "event_type_display": event_type_display(&event_type),
        "source_type": source_type,
        "source_type_display": source_type_display(&source_type),
        "occurred_at": dt_json(row, "occurred_at")?,
        "title": text(row, "title")?,
        "description": text(row, "description")?,
        "location": text(row, "location")?,
        "actor": text(row, "actor")?,
        "source_reference": text(row, "source_reference")?,
        "image_urls": json_value(row, "image_urls")?,
        "data": json_value(row, "data")?,
        "previous_hash": text(row, "previous_hash")?,
        "evidence_hash": evidence_hash,
        "hash_short": evidence_hash.chars().take(12).collect::<String>().to_uppercase(),
        "recorded_at": dt_json(row, "recorded_at")?,
    }))
}

// --------------------------------------------------------------------------------------
// 购物车（`cart_api` / `cart_item_api`）
// --------------------------------------------------------------------------------------

struct CartItemRecord {
    id: Uuid,
    product_id: Uuid,
    quantity: i32,
    updated_at: DateTime<Utc>,
}

fn cart_item_from_row(row: &sqlx::postgres::PgRow) -> Result<CartItemRecord, ApiReject> {
    Ok(CartItemRecord {
        id: row_get!(row, "id"),
        product_id: row_get!(row, "product_id"),
        quantity: row_get!(row, "quantity"),
        updated_at: row_get!(row, "updated_at"),
    })
}

const CART_ITEM_COLUMNS: &str = "id, product_id, quantity, updated_at";

/// `CartItem.Meta.ordering = ['-updated_at']`。seed 的两条同秒，蓝本的同序不可复现，
/// 所以补一个 `id ASC` 定序（D6 同理：我方定序输出，夹具顺序与之一致）。
async fn load_cart_items(pool: &PgPool, buyer_id: Uuid) -> Result<Vec<CartItemRecord>, ApiReject> {
    let rows = sqlx::query(&format!(
        "SELECT {CART_ITEM_COLUMNS} FROM cart_item WHERE buyer_id = $1 \
         ORDER BY updated_at DESC, id ASC"
    ))
    .bind(buyer_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    rows.iter().map(cart_item_from_row).collect()
}

async fn cart_item_json(
    pool: &PgPool,
    item: &CartItemRecord,
    product: &ProductRecord,
) -> Result<Value, ApiReject> {
    let product_image = load_images(pool, product.id).await?;
    let batch = match product.sales_batch_id {
        Some(batch_id) => load_batch(pool, batch_id).await?,
        None => None,
    };

    let mut payload = Map::new();
    payload.insert("id".into(), Value::String(item.id.to_string()));
    payload.insert(
        "product".into(),
        product_payload(product, &product_image, batch.as_ref()),
    );
    payload.insert("quantity".into(), json!(item.quantity));
    payload.insert(
        "subtotal".into(),
        Value::String(ser::dec_scaled(
            product.price * Decimal::from(item.quantity),
            2,
        )),
    );
    payload.insert(
        "updated_at".into(),
        Value::String(ser::dt_z(item.updated_at)),
    );

    Ok(Value::Object(payload))
}

/// `GET` / `POST /api/cart`（含 `/api/v1/cart` 别名）。
pub(crate) async fn cart_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    body: &Value,
    is_get: bool,
) -> ApiResult {
    let user = auth::require_buyer(pool, headers).await?;

    if is_get {
        expire_stale_orders(pool, Some(user.id)).await?;
        return cart_snapshot(pool, user.id).await;
    }

    // 蓝本 POST 分支传 `None`：会把**所有**买家的过期待支付订单一起清掉。
    expire_stale_orders(pool, None).await?;

    let map = body_map(body);
    let mut errors = FieldErrors::default();
    let product_id =
        required_uuid_field(&Input::take(&map, "product_id"), "product_id", &mut errors);
    let quantity = integer_field_with_default(
        &Input::take(&map, "quantity"),
        "quantity",
        1,
        1,
        999,
        &mut errors,
    );
    let (Ok(product_id), Ok(quantity)) = (product_id, quantity) else {
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    };

    let product = load_product(pool, product_id).await?;
    let Some(product) = product.filter(|product| product.status == "on_sale") else {
        return Ok(business_error(StatusCode::NOT_FOUND, ERR_PRODUCT_NOT_FOUND));
    };

    if let Some(batch_id) = product.sales_batch_id {
        let batch = load_batch(pool, batch_id)
            .await?
            .ok_or_else(|| ApiReject::not_found(ERR_PRODUCT_NOT_FOUND))?;
        if !batch.is_open(now()) {
            return Ok(business_error(
                StatusCode::BAD_REQUEST,
                ERR_BATCH_NOT_BUYABLE,
            ));
        }
    }

    // 「车中除目标商品外其它商品的批次集合必须为空或恰好等于目标批次」——实测语义。
    let other_batches: Vec<Option<Uuid>> = sqlx::query_scalar(
        "SELECT p.sales_batch_id FROM cart_item c JOIN citrus_product p ON p.id = c.product_id \
         WHERE c.buyer_id = $1 AND c.product_id <> $2",
    )
    .bind(user.id)
    .bind(product.id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    if !other_batches.is_empty() {
        let same_batch = other_batches
            .iter()
            .all(|value| *value == product.sales_batch_id);
        if !same_batch {
            return Ok(business_error(
                StatusCode::BAD_REQUEST,
                ERR_CART_SINGLE_BATCH,
            ));
        }
    }

    let quantity = quantity as i32;
    if quantity < product.minimum_order_quantity {
        return Ok(business_error(
            StatusCode::BAD_REQUEST,
            format!("该商品 {} 箱起购", product.minimum_order_quantity),
        ));
    }

    let existing = sqlx::query(&format!(
        "SELECT {CART_ITEM_COLUMNS} FROM cart_item WHERE buyer_id = $1 AND product_id = $2"
    ))
    .bind(user.id)
    .bind(product.id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    let created = existing.is_none();
    let now = now();
    let item = match existing.as_ref().map(cart_item_from_row).transpose()? {
        Some(item) => item,
        None => {
            let item = CartItemRecord {
                id: Uuid::new_v4(),
                product_id: product.id,
                quantity,
                updated_at: now,
            };
            sqlx::query(
                "INSERT INTO cart_item (id, buyer_id, product_id, quantity, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $5)",
            )
            .bind(item.id)
            .bind(user.id)
            .bind(item.product_id)
            .bind(item.quantity)
            .bind(now)
            .execute(pool)
            .await
            .map_err(internal_error)?;
            item
        }
    };

    let target_quantity = if created {
        item.quantity
    } else {
        item.quantity + quantity
    };

    if target_quantity > product.purchase_limit {
        rollback_created_cart_item(pool, created, item.id).await?;
        return Ok(business_error(
            StatusCode::BAD_REQUEST,
            format!("该商品每人限购 {} 箱", product.purchase_limit),
        ));
    }
    if target_quantity > product.stock {
        rollback_created_cart_item(pool, created, item.id).await?;
        return Ok(business_error(StatusCode::BAD_REQUEST, ERR_STOCK));
    }

    let saved = if created {
        item
    } else {
        sqlx::query("UPDATE cart_item SET quantity = $2, updated_at = $3 WHERE id = $1")
            .bind(item.id)
            .bind(target_quantity)
            .bind(now)
            .execute(pool)
            .await
            .map_err(internal_error)?;

        CartItemRecord {
            quantity: target_quantity,
            updated_at: now,
            ..item
        }
    };

    Ok(api_ok_message(
        MSG_CART_ADDED,
        cart_item_json(pool, &saved, &product).await?,
    ))
}

/// `get_or_create` 失败时的补偿：只有**新建**的那条才删（蓝本如此）。
async fn rollback_created_cart_item(
    pool: &PgPool,
    created: bool,
    item_id: Uuid,
) -> Result<(), ApiReject> {
    if !created {
        return Ok(());
    }

    sqlx::query("DELETE FROM cart_item WHERE id = $1")
        .bind(item_id)
        .execute(pool)
        .await
        .map_err(internal_error)?;

    Ok(())
}

async fn cart_snapshot(pool: &PgPool, buyer_id: Uuid) -> ApiResult {
    let items = load_cart_items(pool, buyer_id).await?;
    let mut payload_items = Vec::with_capacity(items.len());
    let mut total = Decimal::ZERO;

    for item in &items {
        let product = load_product(pool, item.product_id).await?.ok_or_else(|| {
            ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
        })?;
        total += product.price * Decimal::from(item.quantity);
        payload_items.push(cart_item_json(pool, item, &product).await?);
    }

    Ok(api_ok(json!({
        "items": payload_items,
        "totalAmount": ser::dec_scaled(total, 2),
    })))
}

/// `PATCH` / `DELETE /api/cart/<item_id>`（两个别名同一实现）。
pub(crate) async fn cart_item_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    item_id: &str,
    body: &Value,
    is_delete: bool,
) -> ApiResult {
    let user = auth::require_buyer(pool, headers).await?;

    let item = load_buyer_cart_item(pool, user.id, item_id).await?;
    let Some(item) = item else {
        return Ok(business_error(
            StatusCode::NOT_FOUND,
            ERR_CART_ITEM_NOT_FOUND,
        ));
    };

    if is_delete {
        sqlx::query("DELETE FROM cart_item WHERE id = $1")
            .bind(item.id)
            .execute(pool)
            .await
            .map_err(internal_error)?;

        return Ok(api_ok_message(MSG_CART_ITEM_REMOVED, Value::Null));
    }

    let map = body_map(body);
    let mut errors = FieldErrors::default();
    let quantity = integer_field(
        &Input::take(&map, "quantity"),
        "quantity",
        1,
        999,
        &mut errors,
    );
    let Ok(quantity) = quantity else {
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    };

    let product = load_product(pool, item.product_id)
        .await?
        .ok_or_else(|| ApiReject::not_found(ERR_CART_ITEM_NOT_FOUND))?;

    let quantity = quantity as i32;
    if quantity < product.minimum_order_quantity {
        return Ok(business_error(
            StatusCode::BAD_REQUEST,
            format!("该商品 {} 箱起购", product.minimum_order_quantity),
        ));
    }
    if quantity > product.purchase_limit {
        return Ok(business_error(
            StatusCode::BAD_REQUEST,
            format!("该商品每人限购 {} 箱", product.purchase_limit),
        ));
    }
    if let Some(batch_id) = product.sales_batch_id {
        let batch = load_batch(pool, batch_id)
            .await?
            .ok_or_else(|| ApiReject::not_found(ERR_CART_ITEM_NOT_FOUND))?;
        if !batch.is_open(now()) {
            return Ok(business_error(StatusCode::BAD_REQUEST, ERR_BATCH_STOPPED));
        }
    }
    if quantity > product.stock {
        return Ok(business_error(StatusCode::BAD_REQUEST, ERR_STOCK));
    }

    let now = now();
    sqlx::query("UPDATE cart_item SET quantity = $2, updated_at = $3 WHERE id = $1")
        .bind(item.id)
        .bind(quantity)
        .bind(now)
        .execute(pool)
        .await
        .map_err(internal_error)?;

    let updated = CartItemRecord {
        quantity,
        updated_at: now,
        ..item
    };

    Ok(api_ok(cart_item_json(pool, &updated, &product).await?))
}

async fn load_buyer_cart_item(
    pool: &PgPool,
    buyer_id: Uuid,
    item_id: &str,
) -> Result<Option<CartItemRecord>, ApiReject> {
    let Ok(item_id) = Uuid::parse_str(item_id.trim()) else {
        return Ok(None);
    };

    let row = sqlx::query(&format!(
        "SELECT {CART_ITEM_COLUMNS} FROM cart_item WHERE id = $1 AND buyer_id = $2"
    ))
    .bind(item_id)
    .bind(buyer_id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    row.as_ref().map(cart_item_from_row).transpose()
}

/// `IntegerField(required=False, default=...)`：缺字段用默认值，显式 `null` 仍报错。
fn integer_field_with_default(
    input: &Input,
    field: &str,
    default: i64,
    min_value: i64,
    max_value: i64,
    errors: &mut FieldErrors,
) -> Result<i64, ()> {
    if input.is_missing() {
        return Ok(default);
    }
    integer_field(input, field, min_value, max_value, errors)
}

/// `ListField(child=UUIDField(), required=False, allow_empty=False)`。
///
/// DRF 对子项错误会给出**按下标分组的对象**（`{"0": ["Must be a valid UUID."]}`），
/// 本域夹具未覆盖该分支，这里退化成扁平数组并把每个坏值都报出来。
fn optional_uuid_list(
    input: &Input,
    field: &str,
    errors: &mut FieldErrors,
) -> Result<Option<Vec<Uuid>>, ()> {
    match input {
        Input::Missing | Input::Null => Ok(None),
        Input::Value(Value::Array(values)) => {
            if values.is_empty() {
                errors.push(field, "该列表不能为空。");
                return Err(());
            }

            let mut parsed = Vec::with_capacity(values.len());
            let mut messages = Vec::new();
            for value in values {
                match value
                    .as_str()
                    .and_then(|text| Uuid::parse_str(text.trim()).ok())
                {
                    Some(id) => parsed.push(id),
                    None => messages.push(Value::String(ERR_INVALID_UUID.to_string())),
                }
            }
            if !messages.is_empty() {
                errors.push_object(field, Value::Array(messages));
                return Err(());
            }
            Ok(Some(parsed))
        }
        Input::Value(_) => {
            errors.push(field, "期待为列表类型。");
            Err(())
        }
    }
}

/// DRF `TextChoices` 之外的默认值兜底：`CharField(required=False, default='')`。
fn note_field(input: &Input, field: &str, errors: &mut FieldErrors) -> Result<String, ()> {
    match input {
        Input::Missing => Ok(String::new()),
        Input::Null => {
            errors.push(field, ERR_NULL);
            Err(())
        }
        Input::Value(Value::String(text)) => Ok(text.trim().to_string()),
        Input::Value(_) => {
            errors.push(field, ERR_BLANK);
            Err(())
        }
    }
}

// --------------------------------------------------------------------------------------
// 收货地址（`address_api` / `address_detail_api`）
// --------------------------------------------------------------------------------------

struct AddressRecord {
    id: Uuid,
    recipient_name: String,
    phone: String,
    province: String,
    city: String,
    district: String,
    detail: String,
    is_default: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const ADDRESS_COLUMNS: &str = "id, recipient_name, phone, province, city, district, detail, \
     is_default, created_at, updated_at";

fn address_from_row(row: &sqlx::postgres::PgRow) -> Result<AddressRecord, ApiReject> {
    Ok(AddressRecord {
        id: row_get!(row, "id"),
        recipient_name: row_get!(row, "recipient_name"),
        phone: row_get!(row, "phone"),
        province: row_get!(row, "province"),
        city: row_get!(row, "city"),
        district: row_get!(row, "district"),
        detail: row_get!(row, "detail"),
        is_default: row_get!(row, "is_default"),
        created_at: row_get!(row, "created_at"),
        updated_at: row_get!(row, "updated_at"),
    })
}

fn address_payload(address: &AddressRecord) -> Value {
    json!({
        "id": address.id.to_string(),
        "recipient_name": address.recipient_name,
        "phone": address.phone,
        "province": address.province,
        "city": address.city,
        "district": address.district,
        "detail": address.detail,
        "is_default": address.is_default,
        "created_at": ser::dt_z(address.created_at),
        "updated_at": ser::dt_z(address.updated_at),
    })
}

/// `BuyerAddress.Meta.ordering = ['-is_default', '-updated_at']`（seed 两条同秒 → 补 id）。
async fn load_addresses(pool: &PgPool, buyer_id: Uuid) -> Result<Vec<AddressRecord>, ApiReject> {
    let rows = sqlx::query(&format!(
        "SELECT {ADDRESS_COLUMNS} FROM buyer_address WHERE buyer_id = $1 \
         ORDER BY is_default DESC, updated_at DESC, id ASC"
    ))
    .bind(buyer_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    rows.iter().map(address_from_row).collect()
}

#[derive(Debug, Clone, Default)]
struct AddressInput {
    recipient_name: Option<String>,
    phone: Option<String>,
    province: Option<String>,
    city: Option<String>,
    district: Option<String>,
    detail: Option<String>,
    is_default: Option<bool>,
}

/// `BuyerAddressSerializer` 的字段级校验。
///
/// `recipient_name` / `phone` / `detail` 是 `null=False, blank=False` → 缺失（新建时）、
/// `null`、空串各报一条；部分更新（`PATCH`）时缺省合法，但给了 `null`/空串仍报错。
/// `province` / `city` / `district` 是 `blank=True` → 允许缺省与空串，`null` 仍报错。
/// 字段序即 `Meta.fields` 序：recipient_name, phone, province, city, district, detail,
/// is_default。
fn validate_address(body: &Value, partial: bool, errors: &mut FieldErrors) -> Option<AddressInput> {
    let map = body_map(body);
    let mut input = AddressInput::default();
    let mut failed = false;

    for (field, target) in [
        ("recipient_name", &mut input.recipient_name),
        ("phone", &mut input.phone),
    ] {
        let result = if partial {
            partial_required_char_field(&Input::take(&map, field), field, errors)
        } else {
            required_char_field(&Input::take(&map, field), field, errors).map(Some)
        };
        match result {
            Ok(value) => *target = value,
            Err(()) => failed = true,
        }
    }

    for (field, target) in [
        ("province", &mut input.province),
        ("city", &mut input.city),
        ("district", &mut input.district),
    ] {
        match blank_char_field(&Input::take(&map, field), field, errors) {
            Ok(value) => *target = value,
            Err(()) => failed = true,
        }
    }

    let detail = if partial {
        partial_required_char_field(&Input::take(&map, "detail"), "detail", errors)
    } else {
        required_char_field(&Input::take(&map, "detail"), "detail", errors).map(Some)
    };
    match detail {
        Ok(value) => input.detail = value,
        Err(()) => failed = true,
    }

    match optional_bool_field(&Input::take(&map, "is_default"), "is_default", errors) {
        Ok(value) => input.is_default = value,
        Err(()) => failed = true,
    }

    if failed { None } else { Some(input) }
}

/// `CharField(blank=True, null=False)`：缺省/空串合法，`null` 报错。
fn blank_char_field(
    input: &Input,
    field: &str,
    errors: &mut FieldErrors,
) -> Result<Option<String>, ()> {
    match input {
        Input::Missing => Ok(None),
        Input::Null => {
            errors.push(field, ERR_NULL);
            Err(())
        }
        Input::Value(Value::String(text)) => Ok(Some(text.trim().to_string())),
        Input::Value(_) => {
            errors.push(field, ERR_BLANK);
            Err(())
        }
    }
}

/// 部分更新（`partial=True`）时，必填字段缺省不报错；提供空串仍然报错。
fn partial_required_char_field(
    input: &Input,
    field: &str,
    errors: &mut FieldErrors,
) -> Result<Option<String>, ()> {
    if input.is_missing() {
        return Ok(None);
    }
    required_char_field(input, field, errors).map(Some)
}

pub(crate) async fn address_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    body: &Value,
    is_get: bool,
) -> ApiResult {
    let user = auth::require_buyer(pool, headers).await?;

    if is_get {
        let addresses = load_addresses(pool, user.id).await?;
        let payload: Vec<Value> = addresses.iter().map(address_payload).collect();
        return Ok(api_ok(Value::Array(payload)));
    }

    let mut errors = FieldErrors::default();
    let Some(input) = validate_address(body, false, &mut errors) else {
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    };

    let now = now();
    let existing: i64 =
        sqlx::query_scalar("SELECT count(*) FROM buyer_address WHERE buyer_id = $1")
            .bind(user.id)
            .fetch_one(pool)
            .await
            .map_err(internal_error)?;

    // 第一条地址强制设为默认。
    let make_default = if existing == 0 {
        true
    } else {
        input.is_default.unwrap_or(false)
    };

    if make_default {
        // 蓝本用 `QuerySet.update()`：**不触发** `auto_now`，其它地址的 `updated_at` 不变。
        sqlx::query(
            "UPDATE buyer_address SET is_default = FALSE WHERE buyer_id = $1 AND is_default = TRUE",
        )
        .bind(user.id)
        .execute(pool)
        .await
        .map_err(internal_error)?;
    }

    let address = AddressRecord {
        id: Uuid::new_v4(),
        recipient_name: input.recipient_name.unwrap_or_default(),
        phone: input.phone.unwrap_or_default(),
        province: input.province.unwrap_or_default(),
        city: input.city.unwrap_or_default(),
        district: input.district.unwrap_or_default(),
        detail: input.detail.unwrap_or_default(),
        is_default: make_default,
        created_at: now,
        updated_at: now,
    };

    sqlx::query(
        "INSERT INTO buyer_address (id, buyer_id, recipient_name, phone, province, city, district, \
                                    detail, is_default, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)",
    )
    .bind(address.id)
    .bind(user.id)
    .bind(&address.recipient_name)
    .bind(&address.phone)
    .bind(&address.province)
    .bind(&address.city)
    .bind(&address.district)
    .bind(&address.detail)
    .bind(address.is_default)
    .bind(now)
    .execute(pool)
    .await
    .map_err(internal_error)?;

    Ok(api_ok_message(MSG_ADDRESS_SAVED, address_payload(&address)))
}

pub(crate) async fn address_detail_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    address_id: &str,
    body: &Value,
    is_delete: bool,
) -> ApiResult {
    let user = auth::require_buyer(pool, headers).await?;

    let address = load_buyer_address(pool, user.id, address_id).await?;
    let Some(address) = address else {
        return Ok(business_error(StatusCode::NOT_FOUND, ERR_ADDRESS_NOT_FOUND));
    };

    if is_delete {
        sqlx::query("DELETE FROM buyer_address WHERE id = $1")
            .bind(address.id)
            .execute(pool)
            .await
            .map_err(internal_error)?;

        if address.is_default {
            // 删掉默认地址后，把剩下的第一条（`-is_default, -updated_at`）置为默认。
            if let Some(replacement) = load_addresses(pool, user.id).await?.first() {
                sqlx::query(
                    "UPDATE buyer_address SET is_default = TRUE, updated_at = $2 WHERE id = $1",
                )
                .bind(replacement.id)
                .bind(now())
                .execute(pool)
                .await
                .map_err(internal_error)?;
            }
        }

        return Ok(api_ok_message(MSG_ADDRESS_DELETED, Value::Null));
    }

    let mut errors = FieldErrors::default();
    let Some(patch) = validate_address(body, true, &mut errors) else {
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    };

    let now = now();
    let updated = AddressRecord {
        recipient_name: patch
            .recipient_name
            .unwrap_or_else(|| address.recipient_name.clone()),
        phone: patch.phone.unwrap_or_else(|| address.phone.clone()),
        province: patch.province.unwrap_or_else(|| address.province.clone()),
        city: patch.city.unwrap_or_else(|| address.city.clone()),
        district: patch.district.unwrap_or_else(|| address.district.clone()),
        detail: patch.detail.unwrap_or_else(|| address.detail.clone()),
        is_default: patch.is_default.unwrap_or(address.is_default),
        updated_at: now,
        ..address
    };

    if patch.is_default == Some(true) {
        sqlx::query(
            "UPDATE buyer_address SET is_default = FALSE \
             WHERE buyer_id = $1 AND is_default = TRUE AND id <> $2",
        )
        .bind(user.id)
        .bind(updated.id)
        .execute(pool)
        .await
        .map_err(internal_error)?;
    }

    sqlx::query(
        "UPDATE buyer_address SET recipient_name = $2, phone = $3, province = $4, city = $5, \
                                  district = $6, detail = $7, is_default = $8, updated_at = $9 \
         WHERE id = $1",
    )
    .bind(updated.id)
    .bind(&updated.recipient_name)
    .bind(&updated.phone)
    .bind(&updated.province)
    .bind(&updated.city)
    .bind(&updated.district)
    .bind(&updated.detail)
    .bind(updated.is_default)
    .bind(now)
    .execute(pool)
    .await
    .map_err(internal_error)?;

    Ok(api_ok_message(
        MSG_ADDRESS_UPDATED,
        address_payload(&updated),
    ))
}

async fn load_buyer_address(
    pool: &PgPool,
    buyer_id: Uuid,
    address_id: &str,
) -> Result<Option<AddressRecord>, ApiReject> {
    let Ok(address_id) = Uuid::parse_str(address_id.trim()) else {
        return Ok(None);
    };

    let row = sqlx::query(&format!(
        "SELECT {ADDRESS_COLUMNS} FROM buyer_address WHERE id = $1 AND buyer_id = $2"
    ))
    .bind(address_id)
    .bind(buyer_id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    row.as_ref().map(address_from_row).transpose()
}

// --------------------------------------------------------------------------------------
// 订单（`order_api` / `order_detail_api` / `order_pay_api` / `order_cancel_api`）
// --------------------------------------------------------------------------------------

const ORDER_COLUMNS: &str = "o.id, o.order_number, o.status, o.recipient_name, o.recipient_phone, \
     o.shipping_address, o.total_amount, o.note, o.payment_mode, o.deposit_amount, \
     o.balance_amount, o.paid_amount, o.expires_at, o.balance_due_at, o.paid_at, o.cancelled_at, \
     o.created_at, o.updated_at, o.sales_batch_id";

/// `Order.Meta.ordering = ['-created_at']`（seed 两条同秒 → 补 id）。
async fn load_orders(
    pool: &PgPool,
    buyer_id: Uuid,
) -> Result<Vec<sqlx::postgres::PgRow>, ApiReject> {
    sqlx::query(&format!(
        "SELECT {ORDER_COLUMNS} FROM \"order\" o WHERE o.buyer_id = $1 \
         ORDER BY o.created_at DESC, o.id ASC"
    ))
    .bind(buyer_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)
}

async fn load_order(
    pool: &PgPool,
    buyer_id: Uuid,
    order_id: Uuid,
) -> Result<Option<sqlx::postgres::PgRow>, ApiReject> {
    sqlx::query(&format!(
        "SELECT {ORDER_COLUMNS} FROM \"order\" o WHERE o.id = $1 AND o.buyer_id = $2"
    ))
    .bind(order_id)
    .bind(buyer_id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)
}

/// `OrderSerializer`：字段序即蓝本声明序，`sales_batch` / `trace_packages` / `payments` /
/// `items` 分别按各自 `Meta.ordering` 取。
async fn order_json(pool: &PgPool, row: &sqlx::postgres::PgRow) -> Result<Value, ApiReject> {
    let now = now();
    let status: String = row_get!(row, "status");
    let payment_mode: String = row_get!(row, "payment_mode");
    let total_amount: Decimal = row_get!(row, "total_amount");
    let deposit_amount: Decimal = row_get!(row, "deposit_amount");
    let balance_amount: Decimal = row_get!(row, "balance_amount");
    let sales_batch_id: Option<Uuid> = row_get!(row, "sales_batch_id");
    let order_id: Uuid = row_get!(row, "id");

    let amount_due = match status.as_str() {
        "pending_deposit" => ser::dec_scaled(deposit_amount, 2),
        "pending_balance" => ser::dec_scaled(balance_amount, 2),
        "pending_payment" => ser::dec_scaled(total_amount, 2),
        _ => "0.00".to_string(),
    };
    let payment_action_label = match status.as_str() {
        "pending_deposit" => "支付订金",
        "pending_balance" => "支付尾款",
        "pending_payment" => "支付全款",
        _ => "",
    };

    let batch = match sales_batch_id {
        Some(batch_id) => load_batch(pool, batch_id).await?,
        None => None,
    };

    // `TracePackage.Meta.ordering = ['order', 'sequence']`。
    let package_rows = sqlx::query(
        "SELECT id, trace_code, sequence, box_spec, status, carrier, tracking_number, packed_at, \
                shipped_at, signed_at, created_at \
           FROM trace_package WHERE order_id = $1 ORDER BY sequence ASC, id ASC",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let mut trace_packages = Vec::with_capacity(package_rows.len());
    for package in &package_rows {
        let package_status = text(package, "status")?;
        trace_packages.push(json!({
            "id": uuid_text(package, "id")?,
            "trace_code": text(package, "trace_code")?,
            "sequence": int(package, "sequence")?,
            "box_spec": text(package, "box_spec")?,
            "status": package_status,
            "status_display": package_status_display(&package_status),
            "carrier": text(package, "carrier")?,
            "tracking_number": text(package, "tracking_number")?,
            "packed_at": opt_dt_json(package, "packed_at")?,
            "shipped_at": opt_dt_json(package, "shipped_at")?,
            "signed_at": opt_dt_json(package, "signed_at")?,
            "created_at": dt_json(package, "created_at")?,
        }));
    }

    // `PaymentRecord.Meta.ordering = ['-created_at']`。
    let payment_rows = sqlx::query(
        "SELECT payment_number, stage, provider, amount, status, paid_at, created_at \
           FROM payment_record WHERE order_id = $1 ORDER BY created_at DESC, id ASC",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let mut payments = Vec::with_capacity(payment_rows.len());
    for payment in &payment_rows {
        let stage = text(payment, "stage")?;
        let payment_status = text(payment, "status")?;
        payments.push(json!({
            "payment_number": text(payment, "payment_number")?,
            "stage": stage,
            "stage_display": payment_stage_display(&stage),
            "provider": text(payment, "provider")?,
            "amount": dec_at(payment, "amount", 2)?,
            "status": payment_status,
            "status_display": payment_status_display(&payment_status),
            "paid_at": opt_dt_json(payment, "paid_at")?,
            "created_at": dt_json(payment, "created_at")?,
        }));
    }

    // `OrderItem` 没有 `Meta.ordering`：蓝本按 DB 返回序（= 插入序），这里也不加 ORDER BY。
    let item_rows = sqlx::query(
        "SELECT id, product_id, product_name, product_image_url, unit_price, batch_code, \
                batch_title, sku_type, unit, quantity, subtotal \
           FROM order_item WHERE order_id = $1",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let mut items = Vec::with_capacity(item_rows.len());
    for item in &item_rows {
        items.push(json!({
            "id": uuid_text(item, "id")?,
            "product": opt_pk_json(item, "product_id")?,
            "product_name": text(item, "product_name")?,
            "product_image_url": text(item, "product_image_url")?,
            "unit_price": dec_at(item, "unit_price", 2)?,
            "batch_code": text(item, "batch_code")?,
            "batch_title": text(item, "batch_title")?,
            "sku_type": text(item, "sku_type")?,
            "unit": text(item, "unit")?,
            "quantity": int(item, "quantity")?,
            "subtotal": dec_at(item, "subtotal", 2)?,
        }));
    }

    Ok(json!({
        "id": order_id.to_string(),
        "order_number": text(row, "order_number")?,
        "status": status,
        "status_display": order_status_display(&status),
        "recipient_name": text(row, "recipient_name")?,
        "recipient_phone": text(row, "recipient_phone")?,
        "shipping_address": text(row, "shipping_address")?,
        "total_amount": ser::dec_scaled(total_amount, 2),
        "note": text(row, "note")?,
        "payment_mode": payment_mode,
        "payment_mode_display": payment_mode_display(&payment_mode),
        "deposit_amount": ser::dec_scaled(deposit_amount, 2),
        "balance_amount": ser::dec_scaled(balance_amount, 2),
        "paid_amount": dec_at(row, "paid_amount", 2)?,
        "amount_due": amount_due,
        "payment_action_label": payment_action_label,
        "expires_at": dt_json(row, "expires_at")?,
        "balance_due_at": opt_dt_json(row, "balance_due_at")?,
        "paid_at": opt_dt_json(row, "paid_at")?,
        "cancelled_at": opt_dt_json(row, "cancelled_at")?,
        "created_at": dt_json(row, "created_at")?,
        "updated_at": dt_json(row, "updated_at")?,
        "sales_batch": batch.map_or(Value::Null, |batch| batch.payload(now)),
        "trace_packages": trace_packages,
        "payments": payments,
        "items": items,
    }))
}

/// `GET` / `POST /api/orders`（含 `/api/v1/orders` 别名）。
pub(crate) async fn order_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    body: &Value,
    is_get: bool,
) -> ApiResult {
    let user = auth::require_buyer(pool, headers).await?;

    if is_get {
        expire_stale_orders(pool, Some(user.id)).await?;

        let rows = load_orders(pool, user.id).await?;
        let mut orders = Vec::with_capacity(rows.len());
        for row in &rows {
            orders.push(order_json(pool, row).await?);
        }
        return Ok(api_ok(Value::Array(orders)));
    }

    // 蓝本 POST 分支传 `None`：把所有买家的过期待支付订单一起清掉。
    expire_stale_orders(pool, None).await?;

    let map = body_map(body);
    let mut errors = FieldErrors::default();
    let address_id =
        required_uuid_field(&Input::take(&map, "address_id"), "address_id", &mut errors);
    let requested_ids = optional_uuid_list(
        &Input::take(&map, "cart_item_ids"),
        "cart_item_ids",
        &mut errors,
    );
    let note = note_field(&Input::take(&map, "note"), "note", &mut errors);
    let (Ok(address_id), Ok(requested_ids), Ok(note)) = (address_id, requested_ids, note) else {
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    };

    let address = load_address(pool, user.id, address_id).await?;
    let Some(address) = address else {
        return Ok(business_error(StatusCode::NOT_FOUND, ERR_ADDRESS_NOT_FOUND));
    };

    // `create_order` 的失败类型是响应（业务错误要走成功体形状，`ApiReject` 表达不了）。
    let order_id = match create_order(pool, user.id, &address, requested_ids, &note).await {
        Ok(order_id) => order_id,
        Err(response) => return Ok(response),
    };
    let row = load_order(pool, user.id, order_id)
        .await?
        .ok_or_else(|| ApiReject::not_found(ERR_ORDER_NOT_FOUND))?;

    Ok(api_ok_message(
        MSG_ORDER_CREATED,
        order_json(pool, &row).await?,
    ))
}

async fn load_address(
    pool: &PgPool,
    buyer_id: Uuid,
    address_id: Uuid,
) -> Result<Option<AddressRecord>, ApiReject> {
    let row = sqlx::query(&format!(
        "SELECT {ADDRESS_COLUMNS} FROM buyer_address WHERE id = $1 AND buyer_id = $2"
    ))
    .bind(address_id)
    .bind(buyer_id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    row.as_ref().map(address_from_row).transpose()
}

/// 下单主流程（蓝本 `order_api` 的 POST 分支，整体在一个事务里）。
async fn create_order(
    pool: &PgPool,
    buyer_id: Uuid,
    address: &AddressRecord,
    requested_ids: Option<Vec<Uuid>>,
    note: &str,
) -> Result<Uuid, Response> {
    let now = now();
    let mut tx = pool
        .begin()
        .await
        .map_err(|err| internal_error(err).into_response())?;

    let mut cart_query: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT id, product_id, quantity, updated_at FROM cart_item WHERE buyer_id = ",
    );
    cart_query.push_bind(buyer_id);
    if let Some(ids) = &requested_ids {
        cart_query
            .push(" AND id = ANY(")
            .push_bind(ids.clone())
            .push(")");
    }
    cart_query.push(" ORDER BY updated_at DESC, id ASC FOR UPDATE");

    let cart_rows = cart_query
        .build()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| internal_error(err).into_response())?;
    let cart_items: Vec<CartItemRecord> = cart_rows
        .iter()
        .map(cart_item_from_row)
        .collect::<Result<_, _>>()
        .map_err(ApiReject::into_response)?;

    if cart_items.is_empty() {
        return Err(business_error(StatusCode::BAD_REQUEST, ERR_PICK_CART_ITEM));
    }
    if let Some(ids) = &requested_ids {
        let distinct: std::collections::HashSet<Uuid> = ids.iter().copied().collect();
        if cart_items.len() != distinct.len() {
            return Err(business_error(StatusCode::BAD_REQUEST, ERR_PICK_CART_ITEM));
        }
    }

    // 锁住商品（蓝本 `select_for_update()`）；`FOR UPDATE` 不能落在外连接上，所以这里不 join。
    let product_ids: Vec<Uuid> = cart_items.iter().map(|item| item.product_id).collect();
    let product_rows = sqlx::query(&format!(
        "SELECT {PRODUCT_ROW_COLUMNS}, '' AS seller_name FROM citrus_product p \
         WHERE p.id = ANY($1) FOR UPDATE"
    ))
    .bind(&product_ids)
    .fetch_all(&mut *tx)
    .await
    .map_err(internal_error_response)?;

    let products: Vec<ProductRecord> = product_rows
        .iter()
        .map(product_from_row)
        .collect::<Result<_, _>>()
        .map_err(ApiReject::into_response)?;

    let batch_ids: std::collections::HashSet<Option<Uuid>> = products
        .iter()
        .map(|product| product.sales_batch_id)
        .collect();
    if batch_ids.len() > 1 {
        return Err(business_error(
            StatusCode::BAD_REQUEST,
            ERR_ORDER_SINGLE_BATCH,
        ));
    }

    let batch_id = batch_ids.iter().next().copied().flatten();
    let batch = match batch_id {
        Some(batch_id) => {
            let row = sqlx::query(&format!(
                "SELECT {BATCH_COLUMNS} FROM sales_batch b WHERE b.id = $1 FOR UPDATE"
            ))
            .bind(batch_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal_error_response)?;

            match row {
                Some(row) => {
                    Some(batch_from_row(&row, Value::Null).map_err(ApiReject::into_response)?)
                }
                None => return Err(business_error(StatusCode::BAD_REQUEST, ERR_BATCH_NOT_OPEN)),
            }
        }
        None => None,
    };

    if let Some(batch) = &batch {
        if batch.status != "open"
            || batch.open_at.is_some_and(|value| value > now)
            || batch.close_at.is_some_and(|value| value <= now)
            || batch.available_quantity() <= 0
        {
            return Err(business_error(StatusCode::BAD_REQUEST, ERR_BATCH_NOT_OPEN));
        }
    }

    let mut total = Decimal::ZERO;
    let mut total_quantity: i64 = 0;
    for item in &cart_items {
        let product = products
            .iter()
            .find(|product| product.id == item.product_id)
            .ok_or_else(internal_server_error)?;

        if product.status != "on_sale" {
            return Err(business_error(
                StatusCode::BAD_REQUEST,
                format!("{} 已下架", product.name),
            ));
        }
        if item.quantity > product.stock {
            return Err(business_error(
                StatusCode::BAD_REQUEST,
                format!("{} 库存不足", product.name),
            ));
        }
        if item.quantity < product.minimum_order_quantity {
            return Err(business_error(
                StatusCode::BAD_REQUEST,
                format!("{} {} 箱起购", product.name, product.minimum_order_quantity),
            ));
        }
        if item.quantity > product.purchase_limit {
            return Err(business_error(
                StatusCode::BAD_REQUEST,
                format!("{} 每人限购 {} 箱", product.name, product.purchase_limit),
            ));
        }

        total += product.price * Decimal::from(item.quantity);
        total_quantity += i64::from(item.quantity);
    }

    // 批次可售量校验：可售量 = available_quantity - 在途（待支付/待订金/待尾款）已占用量。
    if let Some(batch) = &batch {
        let reserved: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(i.quantity), 0) FROM order_item i JOIN \"order\" o ON o.id = i.order_id \
             WHERE o.sales_batch_id = $1 \
               AND o.status IN ('pending_payment', 'pending_deposit', 'pending_balance')",
        )
        .bind(batch.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_error_response)?;

        if total_quantity > i64::from((batch.available_quantity() - reserved as i32).max(0)) {
            return Err(business_error(StatusCode::BAD_REQUEST, ERR_BATCH_STOCK));
        }
    }

    let address_text = [
        address.province.as_str(),
        address.city.as_str(),
        address.district.as_str(),
        address.detail.as_str(),
    ]
    .iter()
    .filter(|part| !part.is_empty())
    .copied()
    .collect::<Vec<_>>()
    .join(" ");

    let payment_mode = batch
        .as_ref()
        .map(|batch| batch.payment_mode.clone())
        .unwrap_or_else(|| "full".to_string());

    let mut order_status = "pending_payment".to_string();
    let mut deposit_amount = Decimal::ZERO;
    let mut balance_amount = total;
    let mut balance_due_at: Option<DateTime<Utc>> = None;

    if payment_mode == "deposit_balance" {
        let ratio = batch
            .as_ref()
            .map(|batch| batch.deposit_ratio)
            .unwrap_or(Decimal::ZERO);
        let deposit = (total * ratio / Decimal::from(100))
            .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);

        if deposit <= Decimal::ZERO || deposit >= total {
            return Err(business_error(StatusCode::BAD_REQUEST, ERR_DEPOSIT_RATIO));
        }

        deposit_amount = deposit;
        balance_amount = total - deposit;
        order_status = "pending_deposit".to_string();
        balance_due_at = batch.as_ref().and_then(|batch| batch.close_at);
    }

    let order_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO \"order\" (id, order_number, buyer_id, sales_batch_id, status, payment_mode, \
                                recipient_name, recipient_phone, shipping_address, total_amount, \
                                deposit_amount, balance_amount, paid_amount, note, expires_at, \
                                balance_due_at, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 0, $13, $14, $15, $16, $16)",
    )
    .bind(order_id)
    .bind(generate_order_number(now))
    .bind(buyer_id)
    .bind(batch.as_ref().map(|batch| batch.id))
    .bind(&order_status)
    .bind(&payment_mode)
    .bind(&address.recipient_name)
    .bind(&address.phone)
    .bind(&address_text)
    .bind(total)
    .bind(deposit_amount)
    .bind(balance_amount)
    .bind(note)
    .bind(now + Duration::minutes(ORDER_TTL_MINUTES))
    .bind(balance_due_at)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(internal_error_response)?;

    let mut sequence: i32 = 1;
    for item in &cart_items {
        let product = products
            .iter()
            .find(|product| product.id == item.product_id)
            .ok_or_else(internal_server_error)?;

        let order_item_id = Uuid::new_v4();
        let subtotal = product.price * Decimal::from(item.quantity);
        sqlx::query(
            "INSERT INTO order_item (id, order_id, product_id, product_name, product_image_url, \
                                     batch_code, batch_title, sku_type, unit_price, unit, quantity, \
                                     subtotal) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(order_item_id)
        .bind(order_id)
        .bind(product.id)
        .bind(&product.name)
        .bind(&product.cover_image_url)
        .bind(batch.as_ref().map(|batch| batch.code.clone()).unwrap_or_default())
        .bind(batch.as_ref().map(|batch| batch.title.clone()).unwrap_or_default())
        .bind(&product.sku_type)
        .bind(product.price)
        .bind(&product.unit)
        .bind(item.quantity)
        .bind(subtotal)
        .execute(&mut *tx)
        .await
        .map_err(internal_error_response)?;

        sqlx::query("UPDATE citrus_product SET stock = stock - $2, updated_at = $3 WHERE id = $1")
            .bind(product.id)
            .bind(item.quantity)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(internal_error_response)?;

        // 按箱生成追溯箱码（`sequence` 从 1 连号，`box_spec` 取商品单位）。
        if let Some(batch) = &batch {
            for _ in 0..item.quantity {
                sqlx::query(
                    "INSERT INTO trace_package (id, trace_code, order_id, order_item_id, batch_id, \
                                                sequence, box_spec, status, created_at, updated_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, 'created', $8, $8)",
                )
                .bind(Uuid::new_v4())
                .bind(generate_trace_code())
                .bind(order_id)
                .bind(order_item_id)
                .bind(batch.id)
                .bind(sequence)
                .bind(&product.unit)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(internal_error_response)?;
                sequence += 1;
            }
        }
    }

    let cart_item_ids: Vec<Uuid> = cart_items.iter().map(|item| item.id).collect();
    sqlx::query("DELETE FROM cart_item WHERE id = ANY($1)")
        .bind(&cart_item_ids)
        .execute(&mut *tx)
        .await
        .map_err(internal_error_response)?;

    tx.commit().await.map_err(internal_error_response)?;
    Ok(order_id)
}

/// `_order_number`：`NO<本地时间 %Y%m%d%H%M%S><uuid4 hex 前 6 位大写>`。
fn generate_order_number(now: DateTime<Utc>) -> String {
    let hex = Uuid::new_v4().simple().to_string().to_uppercase();
    format!("NO{}{}", now.format("%Y%m%d%H%M%S"), &hex[..6])
}

/// `models.default_trace_code`：`CGJ-<12 位大写 hex>`。
fn generate_trace_code() -> String {
    let hex = Uuid::new_v4().simple().to_string().to_uppercase();
    format!("CGJ-{}", &hex[..12])
}

/// `models.default_payment_number`：`PAY<20 位大写 hex>`。
fn generate_payment_number() -> String {
    let hex = Uuid::new_v4().simple().to_string().to_uppercase();
    format!("PAY{}", &hex[..20])
}

pub(crate) async fn order_detail_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    order_id: &str,
) -> ApiResult {
    let user = auth::require_buyer(pool, headers).await?;
    let Ok(order_id) = Uuid::parse_str(order_id.trim()) else {
        return Ok(business_error(StatusCode::NOT_FOUND, ERR_ORDER_NOT_FOUND));
    };

    let row = load_order(pool, user.id, order_id).await?;
    let Some(row) = row else {
        return Ok(business_error(StatusCode::NOT_FOUND, ERR_ORDER_NOT_FOUND));
    };

    Ok(api_ok(order_json(pool, &row).await?))
}

/// `POST /api/orders/<id>/pay`（全款 / 订金 / 尾款三态）。
pub(crate) async fn order_pay_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    order_id: &str,
) -> ApiResult {
    let user = auth::require_buyer(pool, headers).await?;
    expire_stale_orders(pool, Some(user.id)).await?;

    let Ok(order_id) = Uuid::parse_str(order_id.trim()) else {
        return Ok(business_error(StatusCode::NOT_FOUND, ERR_ORDER_NOT_FOUND));
    };

    let now = now();
    let mut tx = pool.begin().await.map_err(internal_error)?;

    let row = sqlx::query(&format!(
        "SELECT {ORDER_COLUMNS} FROM \"order\" o WHERE o.id = $1 AND o.buyer_id = $2 FOR UPDATE"
    ))
    .bind(order_id)
    .bind(user.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal_error)?;

    let Some(row) = row else {
        return Ok(business_error(StatusCode::NOT_FOUND, ERR_ORDER_NOT_FOUND));
    };

    let status: String = row_get!(&row, "status");
    let total_amount: Decimal = row_get!(&row, "total_amount");
    let deposit_amount: Decimal = row_get!(&row, "deposit_amount");
    let balance_amount: Decimal = row_get!(&row, "balance_amount");
    let paid_amount: Decimal = row_get!(&row, "paid_amount");
    let sales_batch_id: Option<Uuid> = row_get!(&row, "sales_batch_id");
    let balance_due_at: Option<DateTime<Utc>> = row_get!(&row, "balance_due_at");

    let (stage, payment_amount, next_status) = match status.as_str() {
        "pending_payment" => ("full", total_amount, "paid"),
        "pending_deposit" => ("deposit", deposit_amount, "pending_balance"),
        "pending_balance" => ("balance", balance_amount, "paid"),
        _ => {
            return Ok(business_error(
                StatusCode::BAD_REQUEST,
                ERR_ORDER_CANNOT_PAY,
            ));
        }
    };

    sqlx::query(
        "INSERT INTO payment_record (id, payment_number, order_id, stage, provider, amount, \
                                     status, provider_transaction_id, paid_at, raw_callback, \
                                     created_at, updated_at) \
         VALUES ($1, $2, $3, $4, 'mock', $5, 'succeeded', $6, $7, $8::jsonb, $7, $7)",
    )
    .bind(Uuid::new_v4())
    .bind(generate_payment_number())
    .bind(order_id)
    .bind(stage)
    .bind(payment_amount)
    .bind(format!(
        "MOCK-{}",
        Uuid::new_v4().simple().to_string().to_uppercase()
    ))
    .bind(now)
    .bind(json!({"mode": "local-development"}).to_string())
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;

    let new_paid_amount = paid_amount + payment_amount;

    if next_status == "pending_balance" {
        let expires_at = balance_due_at.unwrap_or(now + Duration::days(7));
        sqlx::query(
            "UPDATE \"order\" SET paid_amount = $2, status = $3, expires_at = $4, updated_at = $5 \
             WHERE id = $1",
        )
        .bind(order_id)
        .bind(new_paid_amount)
        .bind(next_status)
        .bind(expires_at)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(internal_error)?;
    } else {
        sqlx::query(
            "UPDATE \"order\" SET paid_amount = $2, status = $3, paid_at = $4, updated_at = $4 \
             WHERE id = $1",
        )
        .bind(order_id)
        .bind(new_paid_amount)
        .bind(next_status)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(internal_error)?;

        if let Some(batch_id) = sales_batch_id {
            let quantity: i64 = sqlx::query_scalar(
                "SELECT COALESCE(SUM(quantity), 0) FROM order_item WHERE order_id = $1",
            )
            .bind(order_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_error)?;

            sqlx::query(
                "UPDATE sales_batch SET sold_quantity = sold_quantity + $2, updated_at = $3 WHERE id = $1",
            )
            .bind(batch_id)
            .bind(quantity as i32)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(internal_error)?;
        }
    }

    tx.commit().await.map_err(internal_error)?;

    let row = load_order(pool, user.id, order_id)
        .await?
        .ok_or_else(|| ApiReject::not_found(ERR_ORDER_NOT_FOUND))?;
    let message = if next_status == "pending_balance" {
        MSG_DEPOSIT_PAID
    } else {
        MSG_PAID
    };

    Ok(api_ok_message(message, order_json(pool, &row).await?))
}

/// `POST /api/orders/<id>/cancel`：回滚库存 + 已付金额记一笔退款。
pub(crate) async fn order_cancel_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    order_id: &str,
) -> ApiResult {
    let user = auth::require_buyer(pool, headers).await?;

    let Ok(order_id) = Uuid::parse_str(order_id.trim()) else {
        return Ok(business_error(StatusCode::NOT_FOUND, ERR_ORDER_NOT_FOUND));
    };

    let now = now();
    let mut tx = pool.begin().await.map_err(internal_error)?;

    let row = sqlx::query(&format!(
        "SELECT {ORDER_COLUMNS} FROM \"order\" o WHERE o.id = $1 AND o.buyer_id = $2 FOR UPDATE"
    ))
    .bind(order_id)
    .bind(user.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal_error)?;

    let Some(row) = row else {
        return Ok(business_error(StatusCode::NOT_FOUND, ERR_ORDER_NOT_FOUND));
    };

    let status: String = row_get!(&row, "status");
    if !matches!(
        status.as_str(),
        "pending_payment" | "pending_deposit" | "pending_balance"
    ) {
        return Ok(business_error(
            StatusCode::BAD_REQUEST,
            ERR_ORDER_CANNOT_CANCEL,
        ));
    }

    restore_order_stock(&mut tx, order_id, now).await?;

    let paid_amount: Decimal = row_get!(&row, "paid_amount");
    let cancelled_paid_amount = if paid_amount > Decimal::ZERO {
        sqlx::query(
            "INSERT INTO payment_record (id, payment_number, order_id, stage, provider, amount, \
                                         status, provider_transaction_id, paid_at, raw_callback, \
                                         created_at, updated_at) \
             VALUES ($1, $2, $3, 'refund', 'mock', $4, 'refunded', $5, $6, $7::jsonb, $6, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(generate_payment_number())
        .bind(order_id)
        .bind(paid_amount)
        .bind(format!(
            "MOCK-REFUND-{}",
            Uuid::new_v4().simple().to_string().to_uppercase()
        ))
        .bind(now)
        .bind(json!({"mode": "local-development"}).to_string())
        .execute(&mut *tx)
        .await
        .map_err(internal_error)?;

        Decimal::ZERO
    } else {
        paid_amount
    };

    sqlx::query(
        "UPDATE \"order\" SET status = 'cancelled', paid_amount = $2, cancelled_at = $3, \
                              updated_at = $3 \
         WHERE id = $1",
    )
    .bind(order_id)
    .bind(cancelled_paid_amount)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;

    tx.commit().await.map_err(internal_error)?;

    let row = load_order(pool, user.id, order_id)
        .await?
        .ok_or_else(|| ApiReject::not_found(ERR_ORDER_NOT_FOUND))?;

    Ok(api_ok_message(
        MSG_ORDER_CANCELLED,
        order_json(pool, &row).await?,
    ))
}

/// `_restore_order_stock`：把订单里每个商品的库存加回去。
async fn restore_order_stock(
    tx: &mut Transaction<'_, Postgres>,
    order_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), ApiReject> {
    let rows =
        sqlx::query("SELECT product_id, quantity FROM order_item WHERE order_id = $1 FOR UPDATE")
            .bind(order_id)
            .fetch_all(&mut **tx)
            .await
            .map_err(internal_error)?;

    for row in &rows {
        let product_id: Option<Uuid> = row_get!(row, "product_id");
        let quantity: i32 = row_get!(row, "quantity");
        let Some(product_id) = product_id else {
            continue;
        };

        sqlx::query("UPDATE citrus_product SET stock = stock + $2, updated_at = $3 WHERE id = $1")
            .bind(product_id)
            .bind(quantity)
            .bind(now)
            .execute(&mut **tx)
            .await
            .map_err(internal_error)?;
    }

    Ok(())
}

/// `_expire_stale_orders`：未支付且已过期的订单 → 回滚库存 + 置为已取消。
///
/// `buyer` 为 `None` 时清**所有**买家（蓝本的 POST 分支就是这样）。
pub(crate) async fn expire_stale_orders(
    pool: &PgPool,
    buyer: Option<Uuid>,
) -> Result<(), ApiReject> {
    let now = now();
    let mut tx = pool.begin().await.map_err(internal_error)?;

    let stale_ids: Vec<Uuid> = match buyer {
        Some(buyer_id) => sqlx::query_scalar(
            "SELECT id FROM \"order\" \
             WHERE status IN ('pending_payment', 'pending_deposit') AND expires_at <= $1 \
               AND buyer_id = $2 FOR UPDATE",
        )
        .bind(now)
        .bind(buyer_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal_error)?,
        None => sqlx::query_scalar(
            "SELECT id FROM \"order\" \
             WHERE status IN ('pending_payment', 'pending_deposit') AND expires_at <= $1 FOR UPDATE",
        )
        .bind(now)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal_error)?,
    };

    for order_id in stale_ids {
        restore_order_stock(&mut tx, order_id, now).await?;

        sqlx::query(
            "UPDATE \"order\" SET status = 'cancelled', cancelled_at = $2, updated_at = $2 WHERE id = $1",
        )
        .bind(order_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(internal_error)?;
    }

    tx.commit().await.map_err(internal_error)?;
    Ok(())
}

// --------------------------------------------------------------------------------------
// 售后（`after_sale_api`）
// --------------------------------------------------------------------------------------

const AFTER_SALE_COLUMNS: &str = "id, order_id, package_id, issue_type, description, \
     evidence_urls, status, resolution, refund_amount, created_at, updated_at";

fn after_sale_payload(row: &sqlx::postgres::PgRow) -> Result<Value, ApiReject> {
    let issue_type = text(row, "issue_type")?;
    let status = text(row, "status")?;

    Ok(json!({
        "id": uuid_text(row, "id")?,
        "order": uuid_text(row, "order_id")?,
        "package": opt_pk_json(row, "package_id")?,
        "issue_type": issue_type,
        "issue_type_display": after_sale_issue_display(&issue_type),
        "description": text(row, "description")?,
        "evidence_urls": json_value(row, "evidence_urls")?,
        "status": status,
        "status_display": after_sale_status_display(&status),
        "resolution": text(row, "resolution")?,
        "refund_amount": dec_at(row, "refund_amount", 2)?,
        "created_at": dt_json(row, "created_at")?,
        "updated_at": dt_json(row, "updated_at")?,
    }))
}

/// `GET` / `POST /api/after-sales`（含 `/api/v1/after-sales` 别名）。
///
/// 视图层**不校验订单状态**：已取消订单也能提售后，提交后订单直接变 `after_sale`。
pub(crate) async fn after_sale_impl(
    pool: &PgPool,
    headers: &HeaderMap,
    body: &Value,
    is_get: bool,
) -> ApiResult {
    let user = auth::require_buyer(pool, headers).await?;

    if is_get {
        let rows = sqlx::query(&format!(
            "SELECT {AFTER_SALE_COLUMNS} FROM after_sale_request WHERE buyer_id = $1 \
             ORDER BY created_at DESC, id ASC"
        ))
        .bind(user.id)
        .fetch_all(pool)
        .await
        .map_err(internal_error)?;

        let payload: Vec<Value> = rows
            .iter()
            .map(after_sale_payload)
            .collect::<Result<_, _>>()?;
        return Ok(api_ok(Value::Array(payload)));
    }

    let map = body_map(body);
    let mut errors = FieldErrors::default();

    // 字段序 = `AfterSaleRequestSerializer.Meta.fields`（order, package, issue_type,
    // description, evidence_urls），错误字典的插入序就是它。
    let order = related_field(
        pool,
        &Input::take(&map, "order"),
        "order",
        true,
        RelatedKind::Order,
        &mut errors,
    )
    .await?;
    let package = related_field(
        pool,
        &Input::take(&map, "package"),
        "package",
        false,
        RelatedKind::Package,
        &mut errors,
    )
    .await?;
    // `choice_field` 的返回值借自入参，所以入参必须先落到局部变量（临时值会在语句末被丢弃）。
    let issue_input = Input::take(&map, "issue_type");
    let issue_type = choice_field(
        &issue_input,
        "issue_type",
        &["damaged", "spoiled", "weight", "logistics", "other"],
        &mut errors,
    )
    .unwrap_or_default();
    let description = required_char_field(
        &Input::take(&map, "description"),
        "description",
        &mut errors,
    )
    .unwrap_or_default();
    let evidence_urls = optional_json_list(
        &Input::take(&map, "evidence_urls"),
        "evidence_urls",
        &mut errors,
    )
    .unwrap_or_default();

    if !errors.is_empty() {
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    }

    // 校验全过：`order` 是必填字段，一定拿到了行。
    let Some(order) = order else {
        return Err(ApiReject::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal server error",
        ));
    };

    // 对象级 `validate()`：跨字段校验，错误落到 `non_field_errors`。
    let order_buyer_id: Uuid = row_get!(&order.row, "buyer_id");
    if order_buyer_id != user.id {
        errors.push("non_field_errors", ERR_AFTER_SALE_ORDER_OWNER);
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    }
    if let Some(package) = &package {
        let package_order_id: Uuid = row_get!(&package.row, "order_id");
        let order_row_id: Uuid = row_get!(&order.row, "id");
        if package_order_id != order_row_id {
            errors.push("non_field_errors", ERR_AFTER_SALE_PACKAGE);
            return Ok(errors.into_response(StatusCode::BAD_REQUEST));
        }
    }

    let now = now();
    let after_sale_id = Uuid::new_v4();
    let order_id: Uuid = row_get!(&order.row, "id");
    let package_id = package.as_ref().map(|package| package.id);

    sqlx::query(
        "INSERT INTO after_sale_request (id, order_id, package_id, buyer_id, issue_type, \
                                         description, evidence_urls, status, resolution, \
                                         refund_amount, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, 'submitted', '', 0, $8, $8)",
    )
    .bind(after_sale_id)
    .bind(order_id)
    .bind(package_id)
    .bind(user.id)
    .bind(issue_type)
    .bind(&description)
    .bind(evidence_urls.to_string())
    .bind(now)
    .execute(pool)
    .await
    .map_err(internal_error)?;

    sqlx::query("UPDATE \"order\" SET status = 'after_sale', updated_at = $2 WHERE id = $1")
        .bind(order_id)
        .bind(now)
        .execute(pool)
        .await
        .map_err(internal_error)?;

    if let Some(package_id) = package_id {
        sqlx::query(
            "UPDATE trace_package SET status = 'after_sale', updated_at = $2 WHERE id = $1",
        )
        .bind(package_id)
        .bind(now)
        .execute(pool)
        .await
        .map_err(internal_error)?;
    }

    let row = sqlx::query(&format!(
        "SELECT {AFTER_SALE_COLUMNS} FROM after_sale_request WHERE id = $1"
    ))
    .bind(after_sale_id)
    .fetch_one(pool)
    .await
    .map_err(internal_error)?;

    Ok(api_ok_message(
        MSG_AFTER_SALE_CREATED,
        after_sale_payload(&row)?,
    ))
}

/// `PrimaryKeyRelatedField` 的查库结果（连同原始行，供对象级校验用）。
struct RelatedRow {
    id: Uuid,
    row: sqlx::postgres::PgRow,
}

#[derive(Clone, Copy)]
enum RelatedKind {
    Order,
    Package,
}

/// `PrimaryKeyRelatedField(queryset=...)`：先当 UUID 解析（`pk_field.to_internal_value`），
/// 再查库；查不到报 `无效主键 “x” － 对象不存在。`。
///
/// 校验失败把文案写进 `errors` 并返回 `Ok(None)`；只有数据库故障才是 `Err`（蓝本会 500）。
async fn related_field(
    pool: &PgPool,
    input: &Input,
    field: &str,
    required: bool,
    kind: RelatedKind,
    errors: &mut FieldErrors,
) -> Result<Option<RelatedRow>, ApiReject> {
    let raw = match input {
        Input::Missing => {
            if required {
                errors.push(field, ERR_REQUIRED);
            }
            return Ok(None);
        }
        Input::Null => {
            if required {
                errors.push(field, ERR_NULL);
            }
            return Ok(None);
        }
        Input::Value(Value::String(text)) => text.trim().to_string(),
        Input::Value(_) => {
            errors.push(field, ERR_INVALID_UUID);
            return Ok(None);
        }
    };

    let Ok(id) = Uuid::parse_str(&raw) else {
        errors.push(field, ERR_INVALID_UUID);
        return Ok(None);
    };

    let (table, columns) = match kind {
        RelatedKind::Order => ("\"order\"", "id, buyer_id"),
        RelatedKind::Package => ("trace_package", "id, order_id"),
    };
    let row = sqlx::query(&format!("SELECT {columns} FROM {table} WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(internal_error)?;

    match row {
        Some(row) => Ok(Some(RelatedRow { id, row })),
        None => {
            errors.push(field, &invalid_pk_message(&id.to_string()));
            Ok(None)
        }
    }
}

/// `无效主键 “x” － 对象不存在。`
///
/// 标点用转义写成常量：全角左/右双引号 U+201C/U+201D、**全角连字符** U+FF0D（不是 `-`）、
/// 句末全角句号 U+3002。夹具里比对的就是这几个码位。
pub(crate) fn invalid_pk_message(pk: &str) -> String {
    format!("无效主键 \u{201c}{pk}\u{201d} \u{ff0d} 对象不存在\u{3002}")
}

fn optional_json_list(input: &Input, field: &str, errors: &mut FieldErrors) -> Result<Value, ()> {
    match input {
        Input::Missing | Input::Null => Ok(json!([])),
        Input::Value(Value::Array(values)) => Ok(Value::Array(values.clone())),
        Input::Value(_) => {
            errors.push(field, "期待为列表类型。");
            Err(())
        }
    }
}
