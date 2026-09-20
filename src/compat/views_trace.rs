//! 果园、批次、溯源与果农端契约实现。
//!
//! 对应 `navel_backend_git/api/urls.py` 中 orchard_trace 域 **21 条 path**，蓝本为
//! `api/commerce_views.py`（序列化器在 `api/commerce_serializers.py`，字段与 Meta.ordering 在
//! `api/models.py`）。契约基准见 `tests/fixtures/contract/orchard_trace.json`（65 条用例）。
//!
//! 四处最容易踩空的地方：
//!
//! 1. **哈希链逐字节对齐**（本域最高风险）。`TraceEvent._hash_value()` 的输入是
//!    `json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(',', ':'))`；
//!    键名是 camelCase（`eventType`/`sourceType`/`occurredAt`/`sourceReference`/`imageUrls`/
//!    `previousHash`），`occurredAt` 取 `datetime.isoformat()`（UTC → `+00:00`，微秒为 0 时
//!    **整体省略**小数部分）。见 [`canonical_payload`] / [`chain_is_valid`]。
//!    已落库的事件**必须原样回显库里的 `previous_hash`/`evidence_hash`**：夹具里这两个字段
//!    没有被 `normalize` 屏蔽，是逐字比对的；只有新写入的事件才现算（且那两个键在夹具里
//!    被 `$.data.previous_hash` / `$.data.evidence_hash` 屏蔽）。
//! 2. **jsonb 往返结果不参与哈希**：新事件的 `data`/`imageUrls` 直接用请求里的 JSON 值算，
//!    不要「写库再读回再算」——jsonb 会归一化数字（`1e2`→`100`）并重排键，哈希必然漂移。
//! 3. **`<uuid:...>` 转换失败是 404 + `text/html`**（Django URL 解析失败在进视图之前），
//!    不是 400 JSON；反过来，序列化器校验失败走的是**成功体形状**（`{code,message,data,timestamp}`）。
//! 4. **唯一的 5xx 是蓝本缺陷**：`farmer_trees_create_duplicate_500` 在蓝本是 DB 唯一约束
//!    `IntegrityError` 冒泡成 500，按 `DEVIATIONS.md` 的 **D5** 修正为 400（夹具把它标了
//!    `server_error`，比对器归 `expected_deviation`，不计失败）。
//! 5. **链校验的结果与后端相关**（蓝本自身瑕疵，逐字复刻）：`_hash_value()` 用**写库前**的
//!    Python dict 算哈希，`verify_chain` 却用**读回来**的 JSONField 重算；PG 的 jsonb 会把
//!    对象的键按「长度 + 字节序」重排，所以 `data` 是多键对象时两次 `json.dumps` 不一致，
//!    链恒判 false（`data={}` 或单键对象则不受影响）。见 `tests/trace_tests.rs` 的同名断言。
//!
//! 排序：一律照抄蓝本模型的 `Meta.ordering`；只有商品列表（`sort_order, -created_at`）额外加了
//! `id ASC` 作为**确定性兜底**——Django 没有 tiebreaker，而夹具录制的库里 `created_at` 带微秒
//! 所以不撞；从 seed 灌库后微秒被丢掉（见交付说明），撞上就必须有确定序才能对齐（DEVIATIONS D6 同类处理）。

use axum::{
    Router,
    extract::{FromRequest, Multipart, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::str::FromStr;
use uuid::Uuid;

use crate::server::AppState;

use super::{
    auth::{self, AuthUser},
    errors::{ApiReject, api_ok, api_ok_message, api_response},
    ser,
};

// --------------------------------------------------------------------------------------
// 文案字典（逐字取自蓝本 `commerce_views.py` 与 fixtures 的实测响应）
// --------------------------------------------------------------------------------------

/// DRF 校验失败的字段文案（与账号域同源）。
const ERR_REQUIRED: &str = "该字段是必填项。";
const ERR_NULL: &str = "该字段不能为 null。";
const ERR_BLANK: &str = "该字段不能为空。";
const ERR_INVALID_NUMBER: &str = "请填写合法的数字。";
const ERR_INVALID_DATETIME: &str = "日期时间格式错误。";

const ERR_ORCHARD_NOT_FOUND_OR_FORBIDDEN: &str = "果园不存在或无权操作";
const ERR_ORCHARD_NOT_PUBLIC: &str = "果园不存在或尚未通过认证";
const ERR_BATCH_NOT_FOUND: &str = "供货批次不存在";
const ERR_BATCH_NOT_FOUND_OR_FORBIDDEN: &str = "供货批次不存在或无权操作";
const ERR_BATCH_NOT_FOUND_OR_FORBIDDEN_SHORT: &str = "批次不存在或无权操作";
const ERR_TRACE_NOT_FOUND: &str = "未查询到对应的来源追溯记录";
const ERR_TREE_NO_PUBLIC_BATCH: &str = "该果树尚未关联可公开的供货批次";
const ERR_PRODUCT_NOT_FOUND_OR_FORBIDDEN: &str = "商品不存在或无权操作";
const ERR_PRODUCT_NEEDS_BATCH: &str = "上架商品必须关联一个供货批次";
const ERR_BATCH_NOT_YOURS: &str = "该供货批次不属于你的果园";
const ERR_TREE_NOT_IN_ORCHARD: &str = "果树不属于该批次果园";
const ERR_SAMPLE_PRODUCT_MISMATCH: &str = "商品不属于该供货批次";
const ERR_SAMPLE_ARCHIVE_MISMATCH: &str = "采摘档案不属于该供货批次";
const ERR_IMAGE_MISSING: &str = "未提供图片文件";
const ERR_IMAGE_TYPE: &str = "仅支持图片文件";
const ERR_IMAGE_TOO_LARGE: &str = "图片大小不能超过 5MB";
/// D5：蓝本靠 DB 唯一约束冒泡成 500，我方按设计意图返回 400。
const ERR_TREE_NUMBER_TAKEN: &str = "该果园下已存在相同编号的果树";

const MSG_TREE_CREATED: &str = "果树档案已建立";
const MSG_BATCH_CREATED: &str = "供货批次已提交，待运营审核";
const MSG_PRODUCT_SAVED: &str = "商品已保存";
const MSG_PRODUCT_UPDATED: &str = "商品已更新";
const MSG_HARVEST_SAVED: &str = "采摘档案已保存并写入追溯链";
const MSG_EVENT_APPENDED: &str = "溯源记录已追加";
const MSG_SAMPLE_SAVED: &str = "商品健康档案已保存并写入追溯链";
const MSG_IMAGE_UPLOADED: &str = "图片上传成功";

const INTEGRITY_STATEMENT: &str = "事件采用追加式哈希链校验，用于发现记录被意外修改的情况";
const PRIVACY_STATEMENT: &str = "公开溯源信息不包含购买者姓名、电话、地址和完整订单号";

/// 图片落盘目录（蓝本 `MEDIA_ROOT/products/`，公开前缀 `MEDIA_URL` = `/media/`）。
const MEDIA_UPLOAD_DIR: &str = "storage/compat_media/products";
const MEDIA_PUBLIC_PREFIX: &str = "/media/products";

/// 上传大小上限，对应蓝本的 `5 * 1024 * 1024`。
const MAX_UPLOAD_BYTES: usize = 5 * 1024 * 1024;

// --------------------------------------------------------------------------------------
// 路由
// --------------------------------------------------------------------------------------

/// 本域 21 条 path（外层已 `nest("/compat")`，故此处是相对路径）。
///
/// `/api/*` 与 `/api/v1/*` 是**同一批 handler**（蓝本里就是同一个 `@api_view` 挂两条 path），
/// `/api/v1/farmer/batches/<id>/health-records` 也复用 quality-samples 的实现。
/// 每条路由都挂 `MethodRouter::fallback` 收口 405（绝不能用 `Router::method_not_allowed_fallback`）。
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/orchards",
            get(orchard_list_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/orchards/{orchard_id}",
            get(orchard_detail_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/supply-batches",
            get(supply_batch_list_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/supply-batches/{batch_id}",
            get(supply_batch_detail_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/traces/{trace_code}",
            get(trace_lookup_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/orchards",
            get(orchard_list_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/orchards/{orchard_id}",
            get(orchard_detail_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/supply-batches",
            get(supply_batch_list_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/supply-batches/{batch_id}",
            get(supply_batch_detail_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/traces/{trace_code}",
            get(trace_lookup_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/farmer/orchards",
            get(farmer_orchards_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/farmer/orchards/{orchard_id}/trees",
            get(farmer_trees_handler)
                .post(farmer_tree_create_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/farmer/batches",
            get(farmer_batches_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/farmer/batches/create",
            post(farmer_batch_create_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/farmer/upload-image",
            post(farmer_upload_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/farmer/products",
            get(farmer_products_handler)
                .post(farmer_product_create_handler)
                .fallback(handle_unallowed_method),
        )
        // 蓝本只挂 PATCH/PUT；GET 落到 405 兜底。
        .route(
            "/api/v1/farmer/products/{product_id}",
            put(farmer_product_update_handler)
                .patch(farmer_product_update_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/farmer/batches/{batch_id}/harvest-archives",
            get(farmer_harvest_list_handler)
                .post(farmer_harvest_create_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/farmer/batches/{batch_id}/trace-events",
            post(farmer_trace_event_handler).fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/farmer/batches/{batch_id}/quality-samples",
            get(farmer_quality_list_handler)
                .post(farmer_quality_create_handler)
                .fallback(handle_unallowed_method),
        )
        .route(
            "/api/v1/farmer/batches/{batch_id}/health-records",
            get(farmer_quality_list_handler)
                .post(farmer_quality_create_handler)
                .fallback(handle_unallowed_method),
        )
}

/// 405：DRF 异常体 + zh-hans 文案 `方法 “DELETE” 不被允许。`
async fn handle_unallowed_method(method: Method) -> Response {
    auth::render_method_not_allowed(&method)
}

// --------------------------------------------------------------------------------------
// 包装层：取 state / path / query / body，逻辑全在 `*_impl`（便于单测直接打 scratch schema）
// --------------------------------------------------------------------------------------

/// `IsAuthenticated + IsFarmer`：与 [`auth::require_farmer`] 同语义，
/// 但把 401/403 渲染成**带 `WWW-Authenticate: Bearer`** 的形状（蓝本由认证器挂，
/// `ApiReject` 里拿不到响应头，所以只能在这里补）。
async fn authed_farmer(pool: &PgPool, headers: &HeaderMap) -> Result<AuthUser, Response> {
    auth::require_farmer(pool, headers)
        .await
        .map_err(|reject| bearer_header(reject.into_response()))
}

/// 401 / 403 补 `WWW-Authenticate: Bearer`；其它状态原样返回。
fn bearer_header(mut response: Response) -> Response {
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    }
    response
}

/// 渲染本域 `*_impl` 的结果：错误侧已经是渲染好的响应（见 [`TraceResult`]）。
fn render(result: TraceResult) -> Response {
    result.unwrap_or_else(|response| response)
}

/// `<uuid:...>` 转换失败：Django 在 URL 解析阶段就 404，且返回 HTML 调试页。
///
/// 夹具 `orchard_detail_bad_uuid_404` 记的就是 `404 + text/html; charset=utf-8`，
/// **body 是 `null`（不参与比对）**，所以只需保证状态码与 Content-Type。
fn uuid_path_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        "<html><body>404 Not Found</body></html>".to_string(),
    )
        .into_response()
}

/// 解析路径里的 UUID；失败即 Django 风格的 404（HTML）。
fn parse_uuid_path(raw: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(raw).map_err(|_| uuid_path_not_found())
}

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

fn query_text(query: &std::collections::HashMap<String, String>, key: &str) -> Option<String> {
    query
        .get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

// --- 公开读接口 ---

async fn orchard_list_handler(
    State(state): State<AppState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    render(orchard_list_impl(&state.db, query_text(&query, "q").as_deref()).await)
}

async fn orchard_detail_handler(
    State(state): State<AppState>,
    Path(orchard_id): Path<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let id = match parse_uuid_path(&orchard_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    render(orchard_detail_impl(&state.db, id, query_text(&query, "sku_type").as_deref()).await)
}

async fn supply_batch_list_handler(State(state): State<AppState>) -> Response {
    render(supply_batch_list_impl(&state.db).await)
}

async fn supply_batch_detail_handler(
    State(state): State<AppState>,
    Path(batch_id): Path<String>,
) -> Response {
    let id = match parse_uuid_path(&batch_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    render(supply_batch_detail_impl(&state.db, id).await)
}

async fn trace_lookup_handler(State(state): State<AppState>, Path(trace_code): Path<String>) -> Response {
    render(trace_lookup_impl(&state.db, &trace_code).await)
}

// --- 果农端 ---

async fn farmer_orchards_handler(State(state): State<AppState>, request: Request) -> Response {
    let user = match authed_farmer(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    render(farmer_orchards_impl(&state.db, &user).await)
}

async fn farmer_trees_handler(
    State(state): State<AppState>,
    Path(orchard_id): Path<String>,
    request: Request,
) -> Response {
    let id = match parse_uuid_path(&orchard_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let user = match authed_farmer(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    render(farmer_trees_impl(&state.db, &user, id).await)
}

async fn farmer_tree_create_handler(
    State(state): State<AppState>,
    Path(orchard_id): Path<String>,
    request: Request,
) -> Response {
    let id = match parse_uuid_path(&orchard_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let user = match authed_farmer(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    render(farmer_tree_create_impl(&state.db, &user, id, &body).await)
}

async fn farmer_batches_handler(State(state): State<AppState>, request: Request) -> Response {
    let user = match authed_farmer(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    render(farmer_batches_impl(&state.db, &user).await)
}

async fn farmer_batch_create_handler(State(state): State<AppState>, request: Request) -> Response {
    let user = match authed_farmer(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    render(farmer_batch_create_impl(&state.db, &user, &body).await)
}

async fn farmer_upload_handler(State(state): State<AppState>, request: Request) -> Response {
    let headers = request.headers().clone();
    // 上传接口只做 `IsFarmer` 门禁，不记录调用者（蓝本同样不用 request.user）。
    let _user = match authed_farmer(&state.db, &headers).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let upload = match read_image_upload(request).await {
        Ok(upload) => upload,
        Err(response) => return response,
    };
    render(farmer_upload_impl(&headers, upload).await)
}

async fn farmer_products_handler(State(state): State<AppState>, request: Request) -> Response {
    let user = match authed_farmer(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    render(farmer_products_impl(&state.db, &user).await)
}

async fn farmer_product_create_handler(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let user = match authed_farmer(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    render(farmer_product_create_impl(&state.db, &user, &body).await)
}

/// `PUT` 与 `PATCH` 共用同一实现（蓝本对两者都传 `partial=True`）。
async fn farmer_product_update_handler(
    State(state): State<AppState>,
    Path(product_id): Path<String>,
    request: Request,
) -> Response {
    let id = match parse_uuid_path(&product_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let user = match authed_farmer(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    render(farmer_product_update_impl(&state.db, &user, id, &body).await)
}

async fn farmer_harvest_list_handler(
    State(state): State<AppState>,
    Path(batch_id): Path<String>,
    request: Request,
) -> Response {
    let id = match parse_uuid_path(&batch_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let user = match authed_farmer(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    render(farmer_harvest_list_impl(&state.db, &user, id).await)
}

async fn farmer_harvest_create_handler(
    State(state): State<AppState>,
    Path(batch_id): Path<String>,
    request: Request,
) -> Response {
    let id = match parse_uuid_path(&batch_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let user = match authed_farmer(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    render(farmer_harvest_create_impl(&state.db, &user, id, &body).await)
}

async fn farmer_trace_event_handler(
    State(state): State<AppState>,
    Path(batch_id): Path<String>,
    request: Request,
) -> Response {
    let id = match parse_uuid_path(&batch_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let user = match authed_farmer(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    render(farmer_trace_event_impl(&state.db, &user, id, &body).await)
}

async fn farmer_quality_list_handler(
    State(state): State<AppState>,
    Path(batch_id): Path<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    request: Request,
) -> Response {
    let id = match parse_uuid_path(&batch_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let user = match authed_farmer(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let product_id = query_text(&query, "product_id").and_then(|raw| Uuid::parse_str(&raw).ok());
    render(farmer_quality_list_impl(&state.db, &user, id, product_id).await)
}

async fn farmer_quality_create_handler(
    State(state): State<AppState>,
    Path(batch_id): Path<String>,
    request: Request,
) -> Response {
    let id = match parse_uuid_path(&batch_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let user = match authed_farmer(&state.db, request.headers()).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    render(farmer_quality_create_impl(&state.db, &user, id, &body).await)
}

// --------------------------------------------------------------------------------------
// 小工具
// --------------------------------------------------------------------------------------

fn internal_error(err: sqlx::Error) -> Response {
    tracing::error!("compat 果园区查询/写入失败: {err}");
    ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response()
}

// --------------------------------------------------------------------------------------
// 两类错误，不能混（蓝本自己就是这么分裂的）
// --------------------------------------------------------------------------------------
//
// | 来源 | 形状 | 例 |
// |---|---|---|
// | 视图层 `api_response(None, msg, 4xx)` | **成功体**（带 `timestamp`） | `果园不存在或尚未通过认证`、`未提供图片文件` |
// | DRF 认证/权限/方法不允许/未捕获异常 | 异常体（**无** `timestamp`） | `身份认证信息未提供。`、`该接口仅限果农使用`、405、500 |
//
// 所以本域的错误类型直接用渲染好的 `Response`：两种形状都能表达，且 `?` 传播不需要包装。

/// 视图层错误的返回类型（`Ok` 是响应，`Err` 也是响应）。
pub(crate) type TraceResult = Result<Response, Response>;

/// 视图层 4xx：蓝本 `api_response(None, message, status)` → **成功体形状**（带 `timestamp`）。
fn view_error(status: StatusCode, message: &str) -> Response {
    api_response(status, status.as_u16(), Value::String(message.to_string()), Value::Null)
}

/// DRF 异常体（鉴权/权限/405）。
fn drf_error(reject: ApiReject) -> Response {
    reject.into_response()
}

/// DRF 的字段错误字典：`{"字段": ["文案"]}`，**插入序 = 字段声明序**。
#[derive(Debug, Default)]
struct FieldErrors(Map<String, Value>);

impl FieldErrors {
    fn push(&mut self, field: &str, message: &str) {
        self.0.insert(field.to_string(), json!([message]));
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 序列化器校验失败走**成功体形状**（带 `timestamp`），只是 HTTP 状态码是 4xx。
    fn into_response(self, status: StatusCode) -> Response {
        api_response(status, status.as_u16(), Value::Object(self.0), Value::Null)
    }
}

fn json_col(row: &sqlx::postgres::PgRow, name: &str) -> Result<Value, Response> {
    let mut value: Value = row.try_get(name).map_err(internal_error)?;
    // jsonb 的 `null` 与 SQL NULL 都可能出现；蓝本默认值是 `[]` / `{}`。
    if value.is_null() {
        value = Value::Null;
    }
    Ok(value)
}

fn text_col(row: &sqlx::postgres::PgRow, name: &str) -> Result<String, Response> {
    let value: Option<String> = row.try_get(name).map_err(internal_error)?;
    Ok(value.unwrap_or_default())
}

fn opt_text_col(row: &sqlx::postgres::PgRow, name: &str) -> Result<Option<String>, Response> {
    row.try_get(name).map_err(internal_error)
}

/// Python `round()`（银行家舍入）→ 整数。
fn python_round(value: f64) -> i64 {
    value.round_ties_even() as i64
}

/// `label[...]`：不认识的取值原样回显（等价 Django `get_FOO_display()`）。
fn label(pairs: &[(&str, &str)], value: &str) -> String {
    pairs
        .iter()
        .find(|(key, _)| *key == value)
        .map(|(_, text)| (*text).to_string())
        .unwrap_or_else(|| value.to_string())
}

const ORCHARD_STATUS: &[(&str, &str)] = &[
    ("draft", "待审核"),
    ("verified", "已认证"),
    ("inactive", "已停用"),
];
const TREE_HEALTH: &[(&str, &str)] = &[
    ("healthy", "生长良好"),
    ("watch", "持续观察"),
    ("maintenance", "养护中"),
];
const BATCH_STATUS: &[(&str, &str)] = &[
    ("draft", "筹备中"),
    ("warming", "即将上架"),
    ("open", "在售"),
    ("closed", "已停止销售"),
    ("harvesting", "采摘中"),
    ("fulfilling", "履约中"),
    ("completed", "已完成"),
    ("cancelled", "已取消"),
];
const PAYMENT_MODE: &[(&str, &str)] = &[("full", "全款购买"), ("deposit_balance", "订金加尾款")];
const FRUIT_CONDITION: &[(&str, &str)] = &[
    ("good", "状态良好"),
    ("watch", "需要复检"),
    ("rejected", "不进入销售"),
];
const INSPECTION_STAGE: &[(&str, &str)] = &[
    ("growing", "生长期巡检"),
    ("pre_harvest", "采摘前检查"),
    ("at_harvest", "采摘时检查"),
    ("post_sorting", "分选后检查"),
];
const SAMPLE_HEALTH: &[(&str, &str)] = &[
    ("qualified", "健康达标"),
    ("watch", "持续观察"),
    ("rejected", "不合格"),
];
const EVENT_TYPE: &[(&str, &str)] = &[
    ("orchard", "果园建档"),
    ("environment", "环境记录"),
    ("quality", "品质抽检"),
    ("harvest", "成熟采摘"),
    ("sorting", "分选称重"),
    ("packing", "装箱赋码"),
    ("shipping", "产地发货"),
    ("aftersale", "售后记录"),
];
const SOURCE_TYPE: &[(&str, &str)] = &[
    ("operator", "运营记录"),
    ("farmer", "果农记录"),
    ("sensor", "设备采集"),
    ("quality", "质检记录"),
    ("logistics", "物流记录"),
];
const PACKAGE_STATUS: &[(&str, &str)] = &[
    ("created", "已赋码"),
    ("packed", "已装箱"),
    ("shipped", "运输中"),
    ("signed", "已签收"),
    ("after_sale", "售后处理中"),
];
const SKU_TYPE: &[(&str, &str)] = &[
    ("trial", "试吃装"),
    ("family", "家庭装"),
    ("gift", "礼赠装"),
    ("juice", "榨汁装"),
    ("enterprise", "企业装"),
    ("specialty", "特色果品"),
];

/// 蓝本 `_mask_tracking_number`：`SF1234567890` → `SF1******890`。
fn mask_tracking_number(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 6 {
        return value.to_string();
    }
    format!(
        "{}{}{}",
        chars[..3].iter().collect::<String>(),
        "*".repeat(chars.len() - 6),
        chars[chars.len() - 3..].iter().collect::<String>()
    )
}

fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

/// 随机编码：`GY-` + 8 位、`CGJ-TREE-` + 10 位、`CGJ-HV-` + 10 位、`CGJ-` + 12 位大写 hex
/// （蓝本 `models.default_*_code` 用 `uuid4().hex[:n].upper()`）。
fn random_code(prefix: &str, hex_len: usize) -> String {
    let hex = Uuid::new_v4().simple().to_string().to_uppercase();
    format!("{prefix}{}", &hex[..hex_len])
}

// --------------------------------------------------------------------------------------
// 行结构与序列化器
// --------------------------------------------------------------------------------------

struct OrchardRow {
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
}

const ORCHARD_COLUMNS: &str = r#"o.id, o.code, o.name, o.grower_name, o.province, o.city, o.county,
        o.area_mu, o.main_variety, o.tagline, o.feature_tags, o.story, o.cover_image_url,
        o.gallery_urls, o.video_urls, o.certifications, o.status, o.verification_note, o.verified_at"#;

impl OrchardRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, Response> {
        Ok(Self {
            id: row.try_get("id").map_err(internal_error)?,
            code: row.try_get("code").map_err(internal_error)?,
            name: row.try_get("name").map_err(internal_error)?,
            grower_name: row.try_get("grower_name").map_err(internal_error)?,
            province: row.try_get("province").map_err(internal_error)?,
            city: row.try_get("city").map_err(internal_error)?,
            county: row.try_get("county").map_err(internal_error)?,
            area_mu: row.try_get("area_mu").map_err(internal_error)?,
            main_variety: row.try_get("main_variety").map_err(internal_error)?,
            tagline: row.try_get("tagline").map_err(internal_error)?,
            feature_tags: json_col(row, "feature_tags")?,
            story: row.try_get("story").map_err(internal_error)?,
            cover_image_url: row.try_get("cover_image_url").map_err(internal_error)?,
            gallery_urls: json_col(row, "gallery_urls")?,
            video_urls: json_col(row, "video_urls")?,
            certifications: json_col(row, "certifications")?,
            status: row.try_get("status").map_err(internal_error)?,
            verification_note: row.try_get("verification_note").map_err(internal_error)?,
            verified_at: row.try_get("verified_at").map_err(internal_error)?,
        })
    }

    /// 蓝本 `origin_text`：`''.join(part for part in [province, city, county] if part)`
    fn origin_text(&self) -> String {
        [&self.province, &self.city, &self.county]
            .iter()
            .filter(|part| !part.is_empty())
            .map(|part| part.as_str())
            .collect::<Vec<_>>()
            .join("")
    }
}

async fn load_orchard(pool: &PgPool, id: Uuid) -> Result<Option<OrchardRow>, Response> {
    let row = sqlx::query(&format!(
        "SELECT {ORCHARD_COLUMNS} FROM orchard o WHERE o.id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    row.as_ref().map(OrchardRow::from_row).transpose()
}

/// `OrchardPublicSerializer`（含 4 个 SerializerMethodField）。
async fn orchard_public_json(pool: &PgPool, orchard: &OrchardRow) -> Result<Value, Response> {
    let product_count: i64 = sqlx::query_scalar(
        r#"SELECT count(*) FROM citrus_product p JOIN sales_batch b ON b.id = p.sales_batch_id
            WHERE b.orchard_id = $1 AND p.status = 'on_sale'"#,
    )
    .bind(orchard.id)
    .fetch_one(pool)
    .await
    .map_err(internal_error)?;

    let tree_count: i64 = sqlx::query_scalar("SELECT count(*) FROM fruit_tree_archive WHERE orchard_id = $1")
        .bind(orchard.id)
        .fetch_one(pool)
        .await
        .map_err(internal_error)?;

    // `values_list('sku_type', flat=True).distinct()`：Django 把 Meta.ordering 的列也塞进
    // SELECT，所以 DISTINCT 拦不住「同一 sku_type 出现两次」（夹具里就有 4 条含重复标签）。
    let sku_types: Vec<String> = sqlx::query_scalar(
        r#"SELECT p.sku_type FROM citrus_product p JOIN sales_batch b ON b.id = p.sales_batch_id
            WHERE b.orchard_id = $1 AND p.status = 'on_sale'
            ORDER BY p.sort_order ASC, p.created_at DESC, p.id ASC"#,
    )
    .bind(orchard.id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;
    let category_labels: Vec<Value> = sku_types
        .iter()
        .map(|value| Value::String(label(SKU_TYPE, value)))
        .collect();

    let primary_trace_code: Option<String> = sqlx::query_scalar(
        r#"SELECT b.trace_code FROM sales_batch b
            WHERE b.orchard_id = $1 AND b.status NOT IN ('cancelled', 'draft')
            ORDER BY b.is_featured DESC, b.open_at DESC, b.created_at DESC
            LIMIT 1"#,
    )
    .bind(orchard.id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    Ok(json!({
        "id": orchard.id.to_string(),
        "code": orchard.code,
        "name": orchard.name,
        "grower_name": orchard.grower_name,
        "origin": orchard.origin_text(),
        "county": orchard.county,
        "area_mu": ser::opt_dec_scaled(orchard.area_mu, 2),
        "main_variety": orchard.main_variety,
        "tagline": orchard.tagline,
        "feature_tags": orchard.feature_tags,
        "story": orchard.story,
        "cover_image_url": orchard.cover_image_url,
        "gallery_urls": orchard.gallery_urls,
        "video_urls": orchard.video_urls,
        "certifications": orchard.certifications,
        "status": orchard.status,
        "status_display": label(ORCHARD_STATUS, &orchard.status),
        "is_verified": orchard.status == "verified",
        "verification_note": orchard.verification_note,
        "verified_at": ser::opt_dt_z(orchard.verified_at),
        "product_count": product_count,
        "tree_count": tree_count,
        "category_labels": category_labels,
        "primary_trace_code": primary_trace_code.unwrap_or_default(),
    }))
}

struct BatchRow {
    id: Uuid,
    orchard_id: Uuid,
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
}

const BATCH_COLUMNS: &str = r#"b.id, b.orchard_id, b.code, b.trace_code, b.title, b.subtitle, b.status,
        b.planned_quantity, b.sold_quantity, b.open_at, b.close_at, b.expected_harvest_start,
        b.expected_harvest_end, b.expected_ship_start, b.expected_ship_end, b.maturity_standard,
        b.quality_commitment, b.natural_variation_note, b.aftersale_policy, b.cover_image_url,
        b.live_image_urls, b.environment_summary, b.payment_mode, b.deposit_ratio"#;

impl BatchRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, Response> {
        Ok(Self {
            id: row.try_get("id").map_err(internal_error)?,
            orchard_id: row.try_get("orchard_id").map_err(internal_error)?,
            code: row.try_get("code").map_err(internal_error)?,
            trace_code: row.try_get("trace_code").map_err(internal_error)?,
            title: row.try_get("title").map_err(internal_error)?,
            subtitle: row.try_get("subtitle").map_err(internal_error)?,
            status: row.try_get("status").map_err(internal_error)?,
            planned_quantity: row.try_get("planned_quantity").map_err(internal_error)?,
            sold_quantity: row.try_get("sold_quantity").map_err(internal_error)?,
            open_at: row.try_get("open_at").map_err(internal_error)?,
            close_at: row.try_get("close_at").map_err(internal_error)?,
            expected_harvest_start: row.try_get("expected_harvest_start").map_err(internal_error)?,
            expected_harvest_end: row.try_get("expected_harvest_end").map_err(internal_error)?,
            expected_ship_start: row.try_get("expected_ship_start").map_err(internal_error)?,
            expected_ship_end: row.try_get("expected_ship_end").map_err(internal_error)?,
            maturity_standard: row.try_get("maturity_standard").map_err(internal_error)?,
            quality_commitment: row.try_get("quality_commitment").map_err(internal_error)?,
            natural_variation_note: row.try_get("natural_variation_note").map_err(internal_error)?,
            aftersale_policy: row.try_get("aftersale_policy").map_err(internal_error)?,
            cover_image_url: row.try_get("cover_image_url").map_err(internal_error)?,
            live_image_urls: json_col(row, "live_image_urls")?,
            environment_summary: json_col(row, "environment_summary")?,
            payment_mode: row.try_get("payment_mode").map_err(internal_error)?,
            deposit_ratio: row.try_get("deposit_ratio").map_err(internal_error)?,
        })
    }

    /// 蓝本 `available_quantity`：`max(planned_quantity - sold_quantity, 0)`
    fn available_quantity(&self) -> i64 {
        i64::from(self.planned_quantity - self.sold_quantity).max(0)
    }

    /// 蓝本 `is_open`：状态 open + 在开放窗口内 + 还有可售量。
    fn is_open(&self, now: DateTime<Utc>) -> bool {
        self.status == "open"
            && self.open_at.is_none_or(|value| value <= now)
            && self.close_at.is_none_or(|value| value > now)
            && self.available_quantity() > 0
    }

    /// 蓝本 `get_progress_percent`：`min(round(sold * 100 / planned), 100)`（Python 银行家舍入）。
    fn progress_percent(&self) -> i64 {
        if self.planned_quantity <= 0 {
            return 0;
        }
        let value = f64::from(self.sold_quantity) * 100.0 / f64::from(self.planned_quantity);
        python_round(value).min(100)
    }
}

async fn load_batch(pool: &PgPool, id: Uuid) -> Result<Option<BatchRow>, Response> {
    let row = sqlx::query(&format!(
        "SELECT {BATCH_COLUMNS} FROM sales_batch b WHERE b.id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    row.as_ref().map(BatchRow::from_row).transpose()
}

/// `SalesBatchSummarySerializer`（嵌套 `OrchardPublicSerializer`）。
async fn batch_summary_json(pool: &PgPool, batch: &BatchRow) -> Result<Value, Response> {
    let orchard = load_orchard(pool, batch.orchard_id)
        .await?
        .ok_or_else(|| view_error(StatusCode::NOT_FOUND, ERR_ORCHARD_NOT_PUBLIC))?;
    let orchard_json = orchard_public_json(pool, &orchard).await?;
    let now = Utc::now();

    Ok(json!({
        "id": batch.id.to_string(),
        "code": batch.code,
        "trace_code": batch.trace_code,
        "title": batch.title,
        "subtitle": batch.subtitle,
        "status": batch.status,
        "status_display": label(BATCH_STATUS, &batch.status),
        "planned_quantity": batch.planned_quantity,
        "sold_quantity": batch.sold_quantity,
        "available_quantity": batch.available_quantity(),
        "progress_percent": batch.progress_percent(),
        "is_open": batch.is_open(now),
        "open_at": ser::opt_dt_z(batch.open_at),
        "close_at": ser::opt_dt_z(batch.close_at),
        "expected_harvest_start": ser::opt_date(batch.expected_harvest_start),
        "expected_harvest_end": ser::opt_date(batch.expected_harvest_end),
        "expected_ship_start": ser::opt_date(batch.expected_ship_start),
        "expected_ship_end": ser::opt_date(batch.expected_ship_end),
        "maturity_standard": batch.maturity_standard,
        "quality_commitment": batch.quality_commitment,
        "natural_variation_note": batch.natural_variation_note,
        "aftersale_policy": batch.aftersale_policy,
        "cover_image_url": batch.cover_image_url,
        "live_image_urls": batch.live_image_urls,
        "environment_summary": batch.environment_summary,
        "payment_mode": batch.payment_mode,
        "payment_mode_display": label(PAYMENT_MODE, &batch.payment_mode),
        "deposit_ratio": ser::dec_scaled(batch.deposit_ratio, 2),
        "orchard": orchard_json,
    }))
}

/// 商品列表的排序（`CitrusProduct.Meta.ordering = ['sort_order', '-created_at']`）。
///
/// 末尾的 `id ASC` 是**确定性兜底**：夹具录制时 `created_at` 带微秒，从 seed 灌库后被截到毫秒
/// 会出现并列（如 `纽荷尔企业装`/`纽荷尔试吃装` 同毫秒），必须用确定序才能与夹具逐条对齐。
const PRODUCT_ORDER: &str = "ORDER BY p.sort_order ASC, p.created_at DESC, p.id ASC";

struct ProductRow {
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
    seller_name: Option<String>,
}

const PRODUCT_COLUMNS: &str = r#"p.id, p.sales_batch_id, p.name, p.sku_type, p.fruit_type, p.variety,
        p.origin, p.description, p.price, p.unit, p.stock, p.sweetness, p.grade, p.harvest_date,
        p.shipping_note, p.cover_image_url, p.purchase_limit, p.minimum_order_quantity, p.status,
        u.username AS seller_name"#;

impl ProductRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, Response> {
        Ok(Self {
            id: row.try_get("id").map_err(internal_error)?,
            sales_batch_id: row.try_get("sales_batch_id").map_err(internal_error)?,
            name: row.try_get("name").map_err(internal_error)?,
            sku_type: row.try_get("sku_type").map_err(internal_error)?,
            fruit_type: row.try_get("fruit_type").map_err(internal_error)?,
            variety: row.try_get("variety").map_err(internal_error)?,
            origin: row.try_get("origin").map_err(internal_error)?,
            description: row.try_get("description").map_err(internal_error)?,
            price: row.try_get("price").map_err(internal_error)?,
            unit: row.try_get("unit").map_err(internal_error)?,
            stock: row.try_get("stock").map_err(internal_error)?,
            sweetness: row.try_get("sweetness").map_err(internal_error)?,
            grade: row.try_get("grade").map_err(internal_error)?,
            harvest_date: row.try_get("harvest_date").map_err(internal_error)?,
            shipping_note: row.try_get("shipping_note").map_err(internal_error)?,
            cover_image_url: row.try_get("cover_image_url").map_err(internal_error)?,
            purchase_limit: row.try_get("purchase_limit").map_err(internal_error)?,
            minimum_order_quantity: row.try_get("minimum_order_quantity").map_err(internal_error)?,
            status: row.try_get("status").map_err(internal_error)?,
            seller_name: opt_text_col(row, "seller_name")?,
        })
    }
}

async fn load_product(pool: &PgPool, id: Uuid) -> Result<Option<ProductRow>, Response> {
    let row = sqlx::query(&format!(
        r#"SELECT {PRODUCT_COLUMNS} FROM citrus_product p
             LEFT JOIN "user" u ON u.id = p.seller_id
            WHERE p.id = $1"#
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    row.as_ref().map(ProductRow::from_row).transpose()
}

/// `CitrusProductSerializer`。
async fn product_json(pool: &PgPool, product: &ProductRow) -> Result<Value, Response> {
    let images: Vec<Value> = sqlx::query(
        "SELECT id, image_url, sort_order FROM product_image WHERE product_id = $1 ORDER BY sort_order ASC, id ASC",
    )
    .bind(product.id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?
    .iter()
    .map(|row| {
        json!({
            "id": row.try_get::<Uuid, _>("id").map(|v| v.to_string()).unwrap_or_default(),
            "image_url": row.try_get::<String, _>("image_url").unwrap_or_default(),
            "sort_order": row.try_get::<i32, _>("sort_order").unwrap_or_default(),
        })
    })
    .collect();

    let (sales_batch, batch_open) = match product.sales_batch_id {
        Some(batch_id) => match load_batch(pool, batch_id).await? {
            Some(batch) => {
                let summary = batch_summary_json(pool, &batch).await?;
                let open = batch.is_open(Utc::now());
                (Value::Object(summary.as_object().cloned().unwrap_or_default()), open)
            }
            None => (Value::Null, true),
        },
        None => (Value::Null, true),
    };

    let is_available = product.status == "on_sale" && product.stock > 0 && batch_open;

    Ok(json!({
        "id": product.id.to_string(),
        "name": product.name,
        "sku_type": product.sku_type,
        "sku_type_display": label(SKU_TYPE, &product.sku_type),
        "fruit_type": product.fruit_type,
        "variety": product.variety,
        "origin": product.origin,
        "description": product.description,
        "price": ser::dec_scaled(product.price, 2),
        "unit": product.unit,
        "stock": product.stock,
        "sweetness": ser::opt_dec_scaled(product.sweetness, 1),
        "grade": product.grade,
        "harvest_date": ser::opt_date(product.harvest_date),
        "shipping_note": product.shipping_note,
        "cover_image_url": product.cover_image_url,
        "purchase_limit": product.purchase_limit,
        "minimum_order_quantity": product.minimum_order_quantity,
        "status": product.status,
        "seller_name": product.seller_name.clone().unwrap_or_default(),
        "sales_batch": sales_batch,
        "images": images,
        "is_available": is_available,
    }))
}

struct TreeRow {
    id: Uuid,
    trace_code: String,
    tree_number: String,
    plot_name: String,
    variety: String,
    planted_year: Option<i32>,
    growth_stage: String,
    health_status: String,
    growth_summary: String,
    latest_temperature: Option<Decimal>,
    latest_humidity: Option<Decimal>,
    last_observed_at: Option<DateTime<Utc>>,
    cover_image_url: String,
    image_urls: Value,
    video_urls: Value,
    is_featured: bool,
    updated_at: DateTime<Utc>,
}

const TREE_COLUMNS: &str = r#"t.id, t.trace_code, t.tree_number, t.plot_name, t.variety, t.planted_year,
        t.growth_stage, t.health_status, t.growth_summary, t.latest_temperature, t.latest_humidity,
        t.last_observed_at, t.cover_image_url, t.image_urls, t.video_urls, t.is_featured, t.updated_at"#;

impl TreeRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, Response> {
        Ok(Self {
            id: row.try_get("id").map_err(internal_error)?,
            trace_code: row.try_get("trace_code").map_err(internal_error)?,
            tree_number: row.try_get("tree_number").map_err(internal_error)?,
            plot_name: row.try_get("plot_name").map_err(internal_error)?,
            variety: row.try_get("variety").map_err(internal_error)?,
            planted_year: row.try_get("planted_year").map_err(internal_error)?,
            growth_stage: row.try_get("growth_stage").map_err(internal_error)?,
            health_status: row.try_get("health_status").map_err(internal_error)?,
            growth_summary: row.try_get("growth_summary").map_err(internal_error)?,
            latest_temperature: row.try_get("latest_temperature").map_err(internal_error)?,
            latest_humidity: row.try_get("latest_humidity").map_err(internal_error)?,
            last_observed_at: row.try_get("last_observed_at").map_err(internal_error)?,
            cover_image_url: row.try_get("cover_image_url").map_err(internal_error)?,
            image_urls: json_col(row, "image_urls")?,
            video_urls: json_col(row, "video_urls")?,
            is_featured: row.try_get("is_featured").map_err(internal_error)?,
            updated_at: row.try_get("updated_at").map_err(internal_error)?,
        })
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "trace_code": self.trace_code,
            "tree_number": self.tree_number,
            "plot_name": self.plot_name,
            "variety": self.variety,
            "planted_year": self.planted_year,
            "growth_stage": self.growth_stage,
            "health_status": self.health_status,
            "health_status_display": label(TREE_HEALTH, &self.health_status),
            "growth_summary": self.growth_summary,
            "latest_temperature": ser::opt_dec_scaled(self.latest_temperature, 1),
            "latest_humidity": ser::opt_dec_scaled(self.latest_humidity, 1),
            "last_observed_at": ser::opt_dt_z(self.last_observed_at),
            "cover_image_url": self.cover_image_url,
            "image_urls": self.image_urls,
            "video_urls": self.video_urls,
            "is_featured": self.is_featured,
            "updated_at": ser::dt_z(self.updated_at),
        })
    }
}

/// `Meta.ordering = ['-is_featured', 'tree_number']`。
async fn load_trees(pool: &PgPool, orchard_id: Uuid) -> Result<Vec<TreeRow>, Response> {
    sqlx::query(&format!(
        r#"SELECT {TREE_COLUMNS} FROM fruit_tree_archive t
            WHERE t.orchard_id = $1
            ORDER BY t.is_featured DESC, t.tree_number ASC"#
    ))
    .bind(orchard_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?
    .iter()
    .map(TreeRow::from_row)
    .collect()
}

struct HarvestRow {
    id: Uuid,
    harvest_code: String,
    harvested_at: DateTime<Utc>,
    picker: String,
    plot_name: String,
    harvest_method: String,
    quantity_kg: Decimal,
    maturity_brix: Option<Decimal>,
    grade: String,
    pre_harvest_status: String,
    harvest_weather: String,
    fruit_condition: String,
    appearance_note: String,
    pest_status: String,
    damage_rate_percent: Option<Decimal>,
    summary: String,
    image_urls: Value,
    video_urls: Value,
    tree: Option<Uuid>,
    tree_number: String,
    tree_trace_code: String,
    created_at: DateTime<Utc>,
}

const HARVEST_COLUMNS: &str = r#"h.id, h.harvest_code, h.harvested_at, h.picker, h.plot_name,
        h.harvest_method, h.quantity_kg, h.maturity_brix, h.grade, h.pre_harvest_status,
        h.harvest_weather, h.fruit_condition, h.appearance_note, h.pest_status,
        h.damage_rate_percent, h.summary, h.image_urls, h.video_urls, h.tree_id AS tree,
        COALESCE(t.tree_number, '') AS tree_number,
        COALESCE(t.trace_code, '') AS tree_trace_code, h.created_at"#;

impl HarvestRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, Response> {
        Ok(Self {
            id: row.try_get("id").map_err(internal_error)?,
            harvest_code: row.try_get("harvest_code").map_err(internal_error)?,
            harvested_at: row.try_get("harvested_at").map_err(internal_error)?,
            picker: row.try_get("picker").map_err(internal_error)?,
            plot_name: row.try_get("plot_name").map_err(internal_error)?,
            harvest_method: row.try_get("harvest_method").map_err(internal_error)?,
            quantity_kg: row.try_get("quantity_kg").map_err(internal_error)?,
            maturity_brix: row.try_get("maturity_brix").map_err(internal_error)?,
            grade: row.try_get("grade").map_err(internal_error)?,
            pre_harvest_status: row.try_get("pre_harvest_status").map_err(internal_error)?,
            harvest_weather: row.try_get("harvest_weather").map_err(internal_error)?,
            fruit_condition: row.try_get("fruit_condition").map_err(internal_error)?,
            appearance_note: row.try_get("appearance_note").map_err(internal_error)?,
            pest_status: row.try_get("pest_status").map_err(internal_error)?,
            damage_rate_percent: row.try_get("damage_rate_percent").map_err(internal_error)?,
            summary: row.try_get("summary").map_err(internal_error)?,
            image_urls: json_col(row, "image_urls")?,
            video_urls: json_col(row, "video_urls")?,
            tree: row.try_get("tree").map_err(internal_error)?,
            tree_number: text_col(row, "tree_number")?,
            tree_trace_code: text_col(row, "tree_trace_code")?,
            created_at: row.try_get("created_at").map_err(internal_error)?,
        })
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "harvest_code": self.harvest_code,
            "harvested_at": ser::dt_z(self.harvested_at),
            "picker": self.picker,
            "plot_name": self.plot_name,
            "harvest_method": self.harvest_method,
            "quantity_kg": ser::dec_scaled(self.quantity_kg, 2),
            "maturity_brix": ser::opt_dec_scaled(self.maturity_brix, 1),
            "grade": self.grade,
            "pre_harvest_status": self.pre_harvest_status,
            "harvest_weather": self.harvest_weather,
            "fruit_condition": self.fruit_condition,
            "fruit_condition_display": label(FRUIT_CONDITION, &self.fruit_condition),
            "appearance_note": self.appearance_note,
            "pest_status": self.pest_status,
            "damage_rate_percent": ser::opt_dec_scaled(self.damage_rate_percent, 2),
            "summary": self.summary,
            "image_urls": self.image_urls,
            "video_urls": self.video_urls,
            "tree": self.tree.map(|value| value.to_string()),
            "tree_number": self.tree_number,
            "tree_trace_code": self.tree_trace_code,
            "created_at": ser::dt_z(self.created_at),
        })
    }
}

/// `Meta.ordering = ['-harvested_at', '-created_at']`。
async fn load_harvest_archives(pool: &PgPool, batch_id: Uuid) -> Result<Vec<HarvestRow>, Response> {
    sqlx::query(&format!(
        r#"SELECT {HARVEST_COLUMNS} FROM harvest_archive h
             LEFT JOIN fruit_tree_archive t ON t.id = h.tree_id
            WHERE h.batch_id = $1
            ORDER BY h.harvested_at DESC, h.created_at DESC, h.id ASC"#
    ))
    .bind(batch_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?
    .iter()
    .map(HarvestRow::from_row)
    .collect()
}

struct SampleRow {
    id: Uuid,
    product: Option<Uuid>,
    product_name: String,
    harvest_archive: Option<Uuid>,
    harvest_code: String,
    sampled_at: DateTime<Utc>,
    inspection_stage: String,
    health_status: String,
    sample_size: i32,
    sweetness_brix: Option<Decimal>,
    acidity: Option<Decimal>,
    average_weight_grams: Option<i32>,
    diameter_mm: Option<Decimal>,
    grade: String,
    result: String,
    appearance_status: String,
    pest_status: String,
    pesticide_residue_result: String,
    inspector: String,
    report_image_url: String,
    note: String,
}

const SAMPLE_COLUMNS: &str = r#"s.id, s.product_id AS product, COALESCE(p.name, '') AS product_name,
        s.harvest_archive_id AS harvest_archive, COALESCE(a.harvest_code, '') AS harvest_code,
        s.sampled_at, s.inspection_stage, s.health_status, s.sample_size, s.sweetness_brix,
        s.acidity, s.average_weight_grams, s.diameter_mm, s.grade, s.result, s.appearance_status,
        s.pest_status, s.pesticide_residue_result, s.inspector, s.report_image_url, s.note"#;

impl SampleRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, Response> {
        Ok(Self {
            id: row.try_get("id").map_err(internal_error)?,
            product: row.try_get("product").map_err(internal_error)?,
            product_name: text_col(row, "product_name")?,
            harvest_archive: row.try_get("harvest_archive").map_err(internal_error)?,
            harvest_code: text_col(row, "harvest_code")?,
            sampled_at: row.try_get("sampled_at").map_err(internal_error)?,
            inspection_stage: row.try_get("inspection_stage").map_err(internal_error)?,
            health_status: row.try_get("health_status").map_err(internal_error)?,
            sample_size: row.try_get("sample_size").map_err(internal_error)?,
            sweetness_brix: row.try_get("sweetness_brix").map_err(internal_error)?,
            acidity: row.try_get("acidity").map_err(internal_error)?,
            average_weight_grams: row.try_get("average_weight_grams").map_err(internal_error)?,
            diameter_mm: row.try_get("diameter_mm").map_err(internal_error)?,
            grade: row.try_get("grade").map_err(internal_error)?,
            result: row.try_get("result").map_err(internal_error)?,
            appearance_status: row.try_get("appearance_status").map_err(internal_error)?,
            pest_status: row.try_get("pest_status").map_err(internal_error)?,
            pesticide_residue_result: row
                .try_get("pesticide_residue_result")
                .map_err(internal_error)?,
            inspector: row.try_get("inspector").map_err(internal_error)?,
            report_image_url: row.try_get("report_image_url").map_err(internal_error)?,
            note: row.try_get("note").map_err(internal_error)?,
        })
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "product": self.product.map(|value| value.to_string()),
            "product_name": self.product_name,
            "harvest_archive": self.harvest_archive.map(|value| value.to_string()),
            "harvest_code": self.harvest_code,
            "sampled_at": ser::dt_z(self.sampled_at),
            "inspection_stage": self.inspection_stage,
            "inspection_stage_display": label(INSPECTION_STAGE, &self.inspection_stage),
            "health_status": self.health_status,
            "health_status_display": label(SAMPLE_HEALTH, &self.health_status),
            "sample_size": self.sample_size,
            "sweetness_brix": ser::opt_dec_scaled(self.sweetness_brix, 1),
            "acidity": ser::opt_dec_scaled(self.acidity, 2),
            "average_weight_grams": self.average_weight_grams,
            "diameter_mm": ser::opt_dec_scaled(self.diameter_mm, 1),
            "grade": self.grade,
            "result": self.result,
            "appearance_status": self.appearance_status,
            "pest_status": self.pest_status,
            "pesticide_residue_result": self.pesticide_residue_result,
            "inspector": self.inspector,
            "report_image_url": self.report_image_url,
            "note": self.note,
        })
    }
}

/// `Meta.ordering = ['-sampled_at', '-created_at']`，可选 `product_id` 过滤。
async fn load_samples(
    pool: &PgPool,
    batch_id: Uuid,
    product_id: Option<Uuid>,
) -> Result<Vec<SampleRow>, Response> {
    sqlx::query(&format!(
        r#"SELECT {SAMPLE_COLUMNS} FROM batch_quality_sample s
             LEFT JOIN citrus_product p ON p.id = s.product_id
             LEFT JOIN harvest_archive a ON a.id = s.harvest_archive_id
            WHERE s.batch_id = $1 AND ($2::uuid IS NULL OR s.product_id = $2)
            ORDER BY s.sampled_at DESC, s.created_at DESC, s.id ASC"#
    ))
    .bind(batch_id)
    .bind(product_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?
    .iter()
    .map(SampleRow::from_row)
    .collect()
}

struct EventRow {
    id: Uuid,
    event_type: String,
    source_type: String,
    occurred_at: DateTime<Utc>,
    title: String,
    description: String,
    location: String,
    actor: String,
    source_reference: String,
    image_urls: Value,
    data: Value,
    previous_hash: String,
    evidence_hash: String,
    recorded_at: DateTime<Utc>,
}

const EVENT_COLUMNS: &str = r#"e.id, e.event_type, e.source_type, e.occurred_at, e.title,
        e.description, e.location, e.actor, e.source_reference, e.image_urls, e.data,
        e.previous_hash, e.evidence_hash, e.recorded_at"#;

impl EventRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, Response> {
        Ok(Self {
            id: row.try_get("id").map_err(internal_error)?,
            event_type: row.try_get("event_type").map_err(internal_error)?,
            source_type: row.try_get("source_type").map_err(internal_error)?,
            occurred_at: row.try_get("occurred_at").map_err(internal_error)?,
            title: row.try_get("title").map_err(internal_error)?,
            description: row.try_get("description").map_err(internal_error)?,
            location: row.try_get("location").map_err(internal_error)?,
            actor: row.try_get("actor").map_err(internal_error)?,
            source_reference: row.try_get("source_reference").map_err(internal_error)?,
            image_urls: json_col(row, "image_urls")?,
            data: json_col(row, "data")?,
            previous_hash: row.try_get("previous_hash").map_err(internal_error)?,
            evidence_hash: row.try_get("evidence_hash").map_err(internal_error)?,
            recorded_at: row.try_get("recorded_at").map_err(internal_error)?,
        })
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "event_type": self.event_type,
            "event_type_display": label(EVENT_TYPE, &self.event_type),
            "source_type": self.source_type,
            "source_type_display": label(SOURCE_TYPE, &self.source_type),
            "occurred_at": ser::dt_z(self.occurred_at),
            "title": self.title,
            "description": self.description,
            "location": self.location,
            "actor": self.actor,
            "source_reference": self.source_reference,
            "image_urls": self.image_urls,
            "data": self.data,
            "previous_hash": self.previous_hash,
            "evidence_hash": self.evidence_hash,
            "hash_short": self.evidence_hash.chars().take(12).collect::<String>().to_uppercase(),
            "recorded_at": ser::dt_z(self.recorded_at),
        })
    }
}

/// `Meta.ordering = ['occurred_at', 'recorded_at', 'id']`（链校验用的是 `recorded_at, id`）。
async fn load_events(pool: &PgPool, batch_id: Uuid) -> Result<Vec<EventRow>, Response> {
    sqlx::query(&format!(
        r#"SELECT {EVENT_COLUMNS} FROM trace_event e
            WHERE e.batch_id = $1
            ORDER BY e.occurred_at ASC, e.recorded_at ASC, e.id ASC"#
    ))
    .bind(batch_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?
    .iter()
    .map(EventRow::from_row)
    .collect()
}

/// 链校验顺序：`recorded_at, id`（蓝本 `verify_chain` 的 `order_by('recorded_at', 'id')`）。
async fn load_events_for_chain(pool: &PgPool, batch_id: Uuid) -> Result<Vec<EventRow>, Response> {
    sqlx::query(&format!(
        r#"SELECT {EVENT_COLUMNS} FROM trace_event e
            WHERE e.batch_id = $1
            ORDER BY e.recorded_at ASC, e.id ASC"#
    ))
    .bind(batch_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?
    .iter()
    .map(EventRow::from_row)
    .collect()
}

struct PackageRow {
    id: Uuid,
    trace_code: String,
    sequence: i32,
    box_spec: String,
    status: String,
    carrier: String,
    tracking_number: String,
    packed_at: Option<DateTime<Utc>>,
    shipped_at: Option<DateTime<Utc>>,
    signed_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

impl PackageRow {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "trace_code": self.trace_code,
            "sequence": self.sequence,
            "box_spec": self.box_spec,
            "status": self.status,
            "status_display": label(PACKAGE_STATUS, &self.status),
            "carrier": self.carrier,
            "tracking_number": mask_tracking_number(&self.tracking_number),
            "packed_at": ser::opt_dt_z(self.packed_at),
            "shipped_at": ser::opt_dt_z(self.shipped_at),
            "signed_at": ser::opt_dt_z(self.signed_at),
            "created_at": ser::dt_z(self.created_at),
        })
    }
}

async fn load_package(pool: &PgPool, id: Uuid) -> Result<Option<PackageRow>, Response> {
    let row = sqlx::query(
        r#"SELECT p.id, p.trace_code, p.sequence, p.box_spec, p.status, p.carrier,
                  p.tracking_number, p.packed_at, p.shipped_at, p.signed_at, p.created_at
             FROM trace_package p WHERE p.id = $1"#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    let Some(row) = row else {
        return Ok(None);
    };

    Ok(Some(PackageRow {
        id: row.try_get("id").map_err(internal_error)?,
        trace_code: row.try_get("trace_code").map_err(internal_error)?,
        sequence: row.try_get("sequence").map_err(internal_error)?,
        box_spec: row.try_get("box_spec").map_err(internal_error)?,
        status: row.try_get("status").map_err(internal_error)?,
        carrier: row.try_get("carrier").map_err(internal_error)?,
        tracking_number: row.try_get("tracking_number").map_err(internal_error)?,
        packed_at: row.try_get("packed_at").map_err(internal_error)?,
        shipped_at: row.try_get("shipped_at").map_err(internal_error)?,
        signed_at: row.try_get("signed_at").map_err(internal_error)?,
        created_at: row.try_get("created_at").map_err(internal_error)?,
    }))
}

// --------------------------------------------------------------------------------------
// 哈希链
// --------------------------------------------------------------------------------------

/// 待写入事件的哈希输入（键序不重要，[`canonical_payload`] 会按字典序重排）。
///
/// 字段对 crate 可见，便于 `tests/trace_tests.rs` 用夹具 golden 值直接验证哈希。
pub(crate) struct HashInput<'a> {
    pub(crate) event_type: &'a str,
    pub(crate) source_type: &'a str,
    pub(crate) occurred_at: DateTime<Utc>,
    pub(crate) title: &'a str,
    pub(crate) description: &'a str,
    pub(crate) location: &'a str,
    pub(crate) actor: &'a str,
    pub(crate) source_reference: &'a str,
    pub(crate) image_urls: &'a Value,
    pub(crate) data: &'a Value,
    pub(crate) previous_hash: &'a str,
}

/// 蓝本 `TraceEvent._hash_value()`：
///
/// ```python
/// payload = {'batch': ..., 'eventType': ..., 'sourceType': ..., 'occurredAt': isoformat(),
///            'title': ..., 'description': ..., 'location': ..., 'actor': ...,
///            'sourceReference': ..., 'imageUrls': ..., 'data': ..., 'previousHash': ...}
/// canonical = json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(',', ':'))
/// sha256(canonical.encode('utf-8')).hexdigest()
/// ```
///
/// 注意：`occurredAt` 用 `isoformat()`（`+00:00`、微秒为 0 时省略小数），**不是** DRF 的 `Z`。
fn canonical_payload(batch_code: &str, input: &HashInput<'_>) -> String {
    let mut map = Map::new();
    map.insert("actor".into(), Value::String(input.actor.to_string()));
    map.insert("batch".into(), Value::String(batch_code.to_string()));
    map.insert("data".into(), input.data.clone());
    map.insert("description".into(), Value::String(input.description.to_string()));
    map.insert("eventType".into(), Value::String(input.event_type.to_string()));
    map.insert("imageUrls".into(), input.image_urls.clone());
    map.insert("location".into(), Value::String(input.location.to_string()));
    map.insert("occurredAt".into(), Value::String(ser::dt_offset(input.occurred_at)));
    map.insert(
        "previousHash".into(),
        Value::String(input.previous_hash.to_string()),
    );
    map.insert(
        "sourceReference".into(),
        Value::String(input.source_reference.to_string()),
    );
    map.insert("sourceType".into(), Value::String(input.source_type.to_string()));
    map.insert("title".into(), Value::String(input.title.to_string()));

    Value::Object(map).to_string()
}

pub(crate) fn evidence_hash(batch_code: &str, input: &HashInput<'_>) -> String {
    let canonical = canonical_payload(batch_code, input);
    hex_lower(&Sha256::digest(canonical.as_bytes()))
}

/// 从库里读出的行重算哈希（用于 `verify_chain`）。
fn stored_row_hash(batch_code: &str, event: &EventRow) -> String {
    evidence_hash(
        batch_code,
        &HashInput {
            event_type: &event.event_type,
            source_type: &event.source_type,
            occurred_at: event.occurred_at,
            title: &event.title,
            description: &event.description,
            location: &event.location,
            actor: &event.actor,
            source_reference: &event.source_reference,
            image_urls: &event.image_urls,
            data: &event.data,
            previous_hash: &event.previous_hash,
        },
    )
}

/// 蓝本 `TraceEvent.verify_chain`：按 `recorded_at, id` 顺序串链 + 逐条重算哈希。
async fn chain_is_valid(pool: &PgPool, batch_code: &str, batch_id: Uuid) -> Result<bool, Response> {
    let mut expected_previous = String::new();
    for event in load_events_for_chain(pool, batch_id).await? {
        if event.previous_hash != expected_previous {
            return Ok(false);
        }
        if event.evidence_hash != stored_row_hash(batch_code, &event) {
            return Ok(false);
        }
        expected_previous = event.evidence_hash;
    }
    Ok(true)
}

/// 追加一条溯源事件（蓝本 `TraceEvent.save()` 的等价物）。
///
/// `previous_hash` 取该批次**最后一条**（`order_by('recorded_at','id').last()`）事件的
/// `evidence_hash`；`data`/`image_urls` 用**手上的 JSON 值**参与哈希，不用库里的往返结果。
async fn append_trace_event(
    tx: &mut Transaction<'_, Postgres>,
    batch_code: &str,
    batch_id: Uuid,
    input: HashInput<'_>,
) -> Result<String, Response> {
    let previous_hash: Option<String> = sqlx::query_scalar(
        "SELECT evidence_hash FROM trace_event WHERE batch_id = $1 ORDER BY recorded_at DESC, id DESC LIMIT 1",
    )
    .bind(batch_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_error)?;
    let previous_hash = previous_hash.unwrap_or_default();

    // 哈希输入只借用一次，出了作用域就让出所有权（后面还要把各字段绑进 SQL）。
    let hash = {
        let with_previous = HashInput {
            previous_hash: &previous_hash,
            ..input
        };
        evidence_hash(batch_code, &with_previous)
    };

    let now = Utc::now();
    sqlx::query(
        r#"INSERT INTO trace_event
               (id, batch_id, event_type, source_type, occurred_at, title, description, location,
                actor, source_reference, image_urls, data, previous_hash, evidence_hash, recorded_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)"#,
    )
    .bind(Uuid::new_v4())
    .bind(batch_id)
    .bind(input.event_type)
    .bind(input.source_type)
    .bind(input.occurred_at)
    .bind(input.title)
    .bind(input.description)
    .bind(input.location)
    .bind(input.actor)
    .bind(input.source_reference)
    .bind(input.image_urls)
    .bind(input.data)
    .bind(&previous_hash)
    .bind(&hash)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(internal_error)?;

    Ok(hash)
}

// --------------------------------------------------------------------------------------
// 公开读接口
// --------------------------------------------------------------------------------------

/// `GET /api/orchards`（+ v1 别名）。
pub(crate) async fn orchard_list_impl(pool: &PgPool, keyword: Option<&str>) -> TraceResult {
    // 照抄 Django 的 `filter(...).order_by('-sales_batches__is_featured', 'name').distinct()`：
    // DISTINCT 作用于「果园列 + is_featured」，所以同一果园的多个**同 is_featured** 批次会被并掉。
    let mut sql = String::from(
        r#"SELECT DISTINCT o.id, o.name, b.is_featured AS is_featured
             FROM orchard o
             JOIN sales_batch b ON b.orchard_id = o.id
             JOIN citrus_product p ON p.sales_batch_id = b.id AND p.status = 'on_sale'
            WHERE o.status = 'verified'"#,
    );
    if keyword.is_some() {
        sql.push_str(
            r#" AND (o.name ILIKE $1 OR o.county ILIKE $1 OR o.main_variety ILIKE $1 OR o.tagline ILIKE $1)"#,
        );
    }
    sql.push_str(" ORDER BY is_featured DESC, o.name ASC");

    let mut query = sqlx::query(&sql);
    if let Some(keyword) = keyword {
        query = query.bind(format!("%{keyword}%"));
    }
    let rows = query.fetch_all(pool).await.map_err(internal_error)?;

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(internal_error)?;
        if let Some(orchard) = load_orchard(pool, id).await? {
            items.push(orchard_public_json(pool, &orchard).await?);
        }
    }

    Ok(api_ok(json!({ "items": items, "count": items.len() })))
}

/// `GET /api/orchards/<uuid>`（+ v1 别名）。
pub(crate) async fn orchard_detail_impl(
    pool: &PgPool,
    orchard_id: Uuid,
    sku_type: Option<&str>,
) -> TraceResult {
    let orchard = load_orchard(pool, orchard_id).await?;
    let Some(orchard) = orchard.filter(|row| row.status == "verified") else {
        return Err(view_error(StatusCode::NOT_FOUND, ERR_ORCHARD_NOT_PUBLIC));
    };

    let mut product_sql = format!(
        r#"SELECT {PRODUCT_COLUMNS} FROM citrus_product p
             LEFT JOIN "user" u ON u.id = p.seller_id
             JOIN sales_batch b ON b.id = p.sales_batch_id
            WHERE b.orchard_id = $1 AND p.status = 'on_sale'"#
    );
    if sku_type.is_some() {
        product_sql.push_str(" AND p.sku_type = $2");
    }
    product_sql.push(' ');
    product_sql.push_str(PRODUCT_ORDER);

    let mut product_query = sqlx::query(&product_sql).bind(orchard_id);
    if let Some(sku_type) = sku_type {
        product_query = product_query.bind(sku_type);
    }
    let mut products = Vec::new();
    for row in product_query.fetch_all(pool).await.map_err(internal_error)? {
        products.push(product_json(pool, &ProductRow::from_row(&row)?).await?);
    }

    let batch_rows = sqlx::query(&format!(
        r#"SELECT {BATCH_COLUMNS} FROM sales_batch b
            WHERE b.orchard_id = $1 AND b.status NOT IN ('draft', 'cancelled')
            ORDER BY b.is_featured DESC, b.open_at DESC, b.created_at DESC"#
    ))
    .bind(orchard_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;
    let mut batches = Vec::with_capacity(batch_rows.len());
    for row in &batch_rows {
        batches.push(batch_summary_json(pool, &BatchRow::from_row(row)?).await?);
    }

    let trees = load_trees(pool, orchard_id)
        .await?
        .iter()
        .map(TreeRow::to_json)
        .collect::<Vec<_>>();

    let archive_rows = sqlx::query(&format!(
        r#"SELECT {HARVEST_COLUMNS} FROM harvest_archive h
             LEFT JOIN fruit_tree_archive t ON t.id = h.tree_id
             JOIN sales_batch b ON b.id = h.batch_id
            WHERE b.orchard_id = $1
            ORDER BY h.harvested_at DESC, h.created_at DESC, h.id ASC"#
    ))
    .bind(orchard_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;
    let archives = archive_rows
        .iter()
        .map(HarvestRow::from_row)
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .map(HarvestRow::to_json)
        .collect::<Vec<_>>();

    Ok(api_ok(json!({
        "orchard": orchard_public_json(pool, &orchard).await?,
        "products": products,
        "batches": batches,
        "fruitTrees": trees,
        "harvestArchives": archives,
    })))
}

/// `GET /api/supply-batches`（+ v1 别名）。
pub(crate) async fn supply_batch_list_impl(pool: &PgPool) -> TraceResult {
    let rows = sqlx::query(&format!(
        r#"SELECT {BATCH_COLUMNS} FROM sales_batch b
            WHERE b.status IN ('warming', 'open', 'closed', 'harvesting', 'fulfilling')
            ORDER BY b.is_featured DESC, b.open_at DESC, b.created_at DESC"#
    ))
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(batch_summary_json(pool, &BatchRow::from_row(row)?).await?);
    }

    Ok(api_ok(json!({ "items": items, "count": items.len() })))
}

/// `GET /api/supply-batches/<uuid>`（+ v1 别名）。
pub(crate) async fn supply_batch_detail_impl(pool: &PgPool, batch_id: Uuid) -> TraceResult {
    let Some(batch) = load_batch(pool, batch_id).await? else {
        return Err(view_error(StatusCode::NOT_FOUND, ERR_BATCH_NOT_FOUND));
    };

    let mut payload = batch_summary_json(pool, &batch).await?;

    let product_rows = sqlx::query(&format!(
        r#"SELECT {PRODUCT_COLUMNS} FROM citrus_product p
             LEFT JOIN "user" u ON u.id = p.seller_id
            WHERE p.sales_batch_id = $1 AND p.status = 'on_sale' {PRODUCT_ORDER}"#
    ))
    .bind(batch_id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;
    let mut products = Vec::with_capacity(product_rows.len());
    for row in &product_rows {
        products.push(product_json(pool, &ProductRow::from_row(row)?).await?);
    }

    let samples = load_samples(pool, batch_id, None)
        .await?
        .iter()
        .map(SampleRow::to_json)
        .collect::<Vec<_>>();
    let events = load_events(pool, batch_id)
        .await?
        .iter()
        .map(EventRow::to_json)
        .collect::<Vec<_>>();
    let event_count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM trace_event WHERE batch_id = $1")
        .bind(batch_id)
        .fetch_one(pool)
        .await
        .map_err(internal_error)?;

    if let Some(object) = payload.as_object_mut() {
        object.insert("products".into(), Value::Array(products));
        object.insert("qualitySamples".into(), Value::Array(samples));
        object.insert("traceEvents".into(), Value::Array(events));
        object.insert(
            "integrity".into(),
            json!({
                "chainValid": chain_is_valid(pool, &batch.code, batch_id).await?,
                "eventCount": event_count,
            }),
        );
    }

    Ok(api_ok(payload))
}

/// `GET /api/traces/<trace_code>`（+ v1 别名）。
///
/// 依次匹配：`FruitTreeArchive.trace_code|tree_number` → `TracePackage.trace_code` →
/// `SalesBatch.trace_code|code`，`scope` 相应为 `tree`/`package`/`batch`。
pub(crate) async fn trace_lookup_impl(pool: &PgPool, trace_code: &str) -> TraceResult {
    let code = trace_code.trim();

    let tree_row = sqlx::query(&format!(
        r#"SELECT {TREE_COLUMNS} FROM fruit_tree_archive t
            WHERE t.trace_code ILIKE $1 OR t.tree_number ILIKE $1
            ORDER BY t.is_featured DESC, t.tree_number ASC
            LIMIT 1"#
    ))
    .bind(code)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    if let Some(row) = tree_row {
        let tree = TreeRow::from_row(&row)?;
        let orchard_id: Uuid = sqlx::query_scalar("SELECT orchard_id FROM fruit_tree_archive WHERE id = $1")
            .bind(tree.id)
            .fetch_one(pool)
            .await
            .map_err(internal_error)?;

        let batch_id: Option<Uuid> = sqlx::query_scalar(
            r#"SELECT b.id FROM sales_batch b
                WHERE b.orchard_id = $1 AND b.status NOT IN ('draft', 'cancelled')
                ORDER BY b.is_featured DESC, b.open_at DESC, b.created_at DESC
                LIMIT 1"#,
        )
        .bind(orchard_id)
        .fetch_optional(pool)
        .await
        .map_err(internal_error)?;

        let Some(batch_id) = batch_id else {
            return Err(view_error(StatusCode::NOT_FOUND, ERR_TREE_NO_PUBLIC_BATCH));
        };
        let batch = load_batch(pool, batch_id)
            .await?
            .ok_or_else(|| view_error(StatusCode::NOT_FOUND, ERR_BATCH_NOT_FOUND))?;

        return Ok(api_ok(trace_payload(pool, &batch, None, Some(tree)).await?));
    }

    let package_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM trace_package WHERE trace_code ILIKE $1 ORDER BY order_id ASC, sequence ASC LIMIT 1",
    )
    .bind(code)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    if let Some(package_id) = package_id {
        let batch_id: Uuid = sqlx::query_scalar("SELECT batch_id FROM trace_package WHERE id = $1")
            .bind(package_id)
            .fetch_one(pool)
            .await
            .map_err(internal_error)?;
        let batch = load_batch(pool, batch_id)
            .await?
            .ok_or_else(|| view_error(StatusCode::NOT_FOUND, ERR_BATCH_NOT_FOUND))?;
        let package = load_package(pool, package_id)
            .await?
            .ok_or_else(|| view_error(StatusCode::NOT_FOUND, ERR_BATCH_NOT_FOUND))?;

        return Ok(api_ok(
            trace_payload(pool, &batch, Some(package), None).await?,
        ));
    }

    let batch_id: Option<Uuid> = sqlx::query_scalar(
        r#"SELECT b.id FROM sales_batch b
            WHERE b.trace_code ILIKE $1 OR b.code ILIKE $1
            ORDER BY b.is_featured DESC, b.open_at DESC, b.created_at DESC
            LIMIT 1"#,
    )
    .bind(code)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    let Some(batch_id) = batch_id else {
        return Err(view_error(StatusCode::NOT_FOUND, ERR_TRACE_NOT_FOUND));
    };
    let batch = load_batch(pool, batch_id)
        .await?
        .ok_or_else(|| view_error(StatusCode::NOT_FOUND, ERR_BATCH_NOT_FOUND))?;

    Ok(api_ok(trace_payload(pool, &batch, None, None).await?))
}

/// 蓝本 `_trace_payload`：公开溯源视图（不含购买者信息、箱码面单打码）。
async fn trace_payload(
    pool: &PgPool,
    batch: &BatchRow,
    package: Option<PackageRow>,
    tree: Option<TreeRow>,
) -> Result<Value, Response> {
    let events = load_events(pool, batch.id).await?;
    let samples = load_samples(pool, batch.id, None).await?;
    let trees = match &tree {
        Some(tree) => vec![tree.to_json()],
        None => load_trees(pool, batch.orchard_id)
            .await?
            .iter()
            .map(TreeRow::to_json)
            .collect(),
    };
    let archives = load_harvest_archives(pool, batch.id)
        .await?
        .iter()
        .map(HarvestRow::to_json)
        .collect::<Vec<_>>();

    let chain_valid = chain_is_valid(pool, &batch.code, batch.id).await?;
    let orchard = load_orchard(pool, batch.orchard_id)
        .await?
        .ok_or_else(|| view_error(StatusCode::NOT_FOUND, ERR_ORCHARD_NOT_PUBLIC))?;
    let latest_hash = events
        .last()
        .map(|event| event.evidence_hash.clone())
        .unwrap_or_default();

    let lookup_code = match &tree {
        Some(tree) => tree.trace_code.clone(),
        None => match &package {
            Some(package) => package.trace_code.clone(),
            None => batch.trace_code.clone(),
        },
    };
    let scope = if tree.is_some() {
        "tree"
    } else if package.is_some() {
        "package"
    } else {
        "batch"
    };

    Ok(json!({
        "lookupCode": lookup_code,
        "scope": scope,
        "verified": orchard.status == "verified" && chain_valid,
        "orchard": orchard_public_json(pool, &orchard).await?,
        "batch": batch_summary_json(pool, batch).await?,
        "fruitTrees": trees,
        "harvestArchives": archives,
        "qualitySamples": samples.iter().map(SampleRow::to_json).collect::<Vec<_>>(),
        "events": events.iter().map(EventRow::to_json).collect::<Vec<_>>(),
        "package": package.map(|row| row.to_json()),
        "integrity": {
            "chainValid": chain_valid,
            "eventCount": events.len(),
            "latestHash": latest_hash,
            // 蓝本用 `timezone.now().isoformat()`（`+00:00`，夹具已屏蔽该路径）。
            "checkedAt": ser::dt_offset(Utc::now()),
            "statement": INTEGRITY_STATEMENT,
        },
        "privacy": { "statement": PRIVACY_STATEMENT },
    }))
}

// --------------------------------------------------------------------------------------
// 果农端
// --------------------------------------------------------------------------------------

/// `Meta.ordering = ['-verified_at', '-created_at']`。
pub(crate) async fn farmer_orchards_impl(pool: &PgPool, user: &AuthUser) -> TraceResult {
    let rows = sqlx::query(&format!(
        r#"SELECT {ORCHARD_COLUMNS} FROM orchard o
            WHERE o.owner_id = $1
            ORDER BY o.verified_at DESC, o.created_at DESC"#
    ))
    .bind(user.id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(orchard_public_json(pool, &OrchardRow::from_row(row)?).await?);
    }

    Ok(api_ok(Value::Array(items)))
}

/// `GET /api/v1/farmer/orchards/<uuid>/trees`。
pub(crate) async fn farmer_trees_impl(
    pool: &PgPool,
    user: &AuthUser,
    orchard_id: Uuid,
) -> TraceResult {
    own_orchard(pool, user, orchard_id).await?;
    let trees = load_trees(pool, orchard_id)
        .await?
        .iter()
        .map(TreeRow::to_json)
        .collect::<Vec<_>>();

    Ok(api_ok(Value::Array(trees)))
}

async fn own_orchard(pool: &PgPool, user: &AuthUser, orchard_id: Uuid) -> Result<OrchardRow, Response> {
    let row = sqlx::query(&format!(
        "SELECT {ORCHARD_COLUMNS} FROM orchard o WHERE o.id = $1 AND o.owner_id = $2"
    ))
    .bind(orchard_id)
    .bind(user.id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    row.as_ref()
        .map(OrchardRow::from_row)
        .transpose()?
        .ok_or_else(|| view_error(StatusCode::NOT_FOUND, ERR_ORCHARD_NOT_FOUND_OR_FORBIDDEN))
}

/// `POST /api/v1/farmer/orchards/<uuid>/trees`：`FruitTreeArchiveCreateSerializer`。
pub(crate) async fn farmer_tree_create_impl(
    pool: &PgPool,
    user: &AuthUser,
    orchard_id: Uuid,
    body: &Value,
) -> TraceResult {
    own_orchard(pool, user, orchard_id).await?;

    let map = body.as_object().cloned().unwrap_or_default();
    // DRF 是「把每个声明字段都校验一遍再一起报错」，所以不能拿到第一个错就返回。
    let mut errors = FieldErrors::default();
    let tree_number = required_text(&map, "tree_number", &mut errors);
    let variety = required_text(&map, "variety", &mut errors);
    if tree_number.is_none() || variety.is_none() || !errors.is_empty() {
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    }
    let (tree_number, variety) = (tree_number.unwrap(), variety.unwrap());

    let plot_name = optional_text(&map, "plot_name").unwrap_or_default();
    let planted_year = optional_int(&map, "planted_year");
    let growth_stage = optional_text(&map, "growth_stage").unwrap_or_default();
    let health_status = optional_text(&map, "health_status").unwrap_or_else(|| "healthy".into());
    let growth_summary = optional_text(&map, "growth_summary").unwrap_or_default();
    let latest_temperature = optional_decimal(&map, "latest_temperature");
    let latest_humidity = optional_decimal(&map, "latest_humidity");
    let last_observed_at = optional_datetime(&map, "last_observed_at");
    let cover_image_url = optional_text(&map, "cover_image_url").unwrap_or_default();
    let image_urls = map.get("image_urls").cloned().unwrap_or_else(|| json!([]));
    let video_urls = map.get("video_urls").cloned().unwrap_or_else(|| json!([]));
    let is_featured = map
        .get("is_featured")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let id = Uuid::new_v4();
    let now = Utc::now();
    let trace_code = random_code("CGJ-TREE-", 10);

    let insert = sqlx::query(
        r#"INSERT INTO fruit_tree_archive
               (id, trace_code, orchard_id, tree_number, plot_name, variety, planted_year,
                growth_stage, health_status, growth_summary, latest_temperature, latest_humidity,
                last_observed_at, cover_image_url, image_urls, video_urls, is_featured,
                created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                   $18, $18)"#,
    )
    .bind(id)
    .bind(&trace_code)
    .bind(orchard_id)
    .bind(&tree_number)
    .bind(&plot_name)
    .bind(&variety)
    .bind(planted_year)
    .bind(&growth_stage)
    .bind(&health_status)
    .bind(&growth_summary)
    .bind(latest_temperature)
    .bind(latest_humidity)
    .bind(last_observed_at)
    .bind(&cover_image_url)
    .bind(&image_urls)
    .bind(&video_urls)
    .bind(is_featured)
    .bind(now)
    .execute(pool)
    .await;

    if let Err(err) = insert {
        // D5：蓝本让 DB 唯一约束冒泡成 500，这里按设计意图返回 400。
        if is_unique_violation(&err) {
            let mut errors = FieldErrors::default();
            errors.push("tree_number", ERR_TREE_NUMBER_TAKEN);
            return Ok(errors.into_response(StatusCode::BAD_REQUEST));
        }
        return Err(internal_error(err));
    }

    let row = sqlx::query(&format!(
        "SELECT {TREE_COLUMNS} FROM fruit_tree_archive t WHERE t.id = $1"
    ))
    .bind(id)
    .fetch_one(pool)
    .await
    .map_err(internal_error)?;

    Ok(api_ok_message(MSG_TREE_CREATED, TreeRow::from_row(&row)?.to_json()))
}

/// `GET /api/v1/farmer/batches`。
pub(crate) async fn farmer_batches_impl(pool: &PgPool, user: &AuthUser) -> TraceResult {
    let rows = sqlx::query(&format!(
        r#"SELECT {BATCH_COLUMNS} FROM sales_batch b
             JOIN orchard o ON o.id = b.orchard_id
            WHERE o.owner_id = $1
            ORDER BY b.is_featured DESC, b.open_at DESC, b.created_at DESC"#
    ))
    .bind(user.id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(batch_summary_json(pool, &BatchRow::from_row(row)?).await?);
    }

    Ok(api_ok(Value::Array(items)))
}

/// `POST /api/v1/farmer/batches/create`：视图**先查果园**，缺 `orchard_id` 也是 404。
pub(crate) async fn farmer_batch_create_impl(
    pool: &PgPool,
    user: &AuthUser,
    body: &Value,
) -> TraceResult {
    let map = body.as_object().cloned().unwrap_or_default();
    let orchard_id = map
        .get("orchard_id")
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok());
    let orchard = match orchard_id {
        Some(id) => own_orchard(pool, user, id).await?,
        None => return Err(view_error(StatusCode::NOT_FOUND, ERR_ORCHARD_NOT_FOUND_OR_FORBIDDEN)),
    };

    let mut errors = FieldErrors::default();
    let title = match required_text(&map, "title", &mut errors) {
        Some(value) => value,
        None => return Ok(errors.into_response(StatusCode::BAD_REQUEST)),
    };

    let subtitle = optional_text(&map, "subtitle").unwrap_or_default();
    let planned_quantity = optional_int(&map, "planned_quantity").unwrap_or(0);
    let expected_harvest_start = optional_date(&map, "expected_harvest_start");
    let expected_harvest_end = optional_date(&map, "expected_harvest_end");
    let expected_ship_start = optional_date(&map, "expected_ship_start");
    let expected_ship_end = optional_date(&map, "expected_ship_end");
    let maturity_standard = optional_text(&map, "maturity_standard").unwrap_or_default();
    let quality_commitment = optional_text(&map, "quality_commitment").unwrap_or_default();
    let natural_variation_note = optional_text(&map, "natural_variation_note").unwrap_or_default();
    let aftersale_policy = optional_text(&map, "aftersale_policy").unwrap_or_default();
    let cover_image_url = optional_text(&map, "cover_image_url").unwrap_or_default();
    let environment_summary = map.get("environment_summary").cloned().unwrap_or_else(|| json!({}));
    let payment_mode = optional_text(&map, "payment_mode").unwrap_or_else(|| "full".into());
    let deposit_ratio = optional_decimal(&map, "deposit_ratio").unwrap_or(Decimal::ZERO);

    let id = Uuid::new_v4();
    let now = Utc::now();
    // 蓝本：`f'CGJ-{orchard.code.replace("GY-", "")}-{uuid4().hex[:6].upper()}'`
    let code = format!(
        "CGJ-{}-{}",
        orchard.code.replace("GY-", ""),
        random_code("", 6)
    );
    let trace_code = random_code("CGJ-", 12);

    sqlx::query(
        r#"INSERT INTO sales_batch
               (id, code, trace_code, orchard_id, title, subtitle, status, planned_quantity,
                expected_harvest_start, expected_harvest_end, expected_ship_start,
                expected_ship_end, maturity_standard, quality_commitment, natural_variation_note,
                aftersale_policy, cover_image_url, environment_summary, payment_mode,
                deposit_ratio, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, 'draft', $7, $8, $9, $10, $11, $12, $13, $14, $15,
                   $16, $17, $18, $19, $20, $20)"#,
    )
    .bind(id)
    .bind(&code)
    .bind(&trace_code)
    .bind(orchard.id)
    .bind(&title)
    .bind(&subtitle)
    .bind(planned_quantity)
    .bind(expected_harvest_start)
    .bind(expected_harvest_end)
    .bind(expected_ship_start)
    .bind(expected_ship_end)
    .bind(&maturity_standard)
    .bind(&quality_commitment)
    .bind(&natural_variation_note)
    .bind(&aftersale_policy)
    .bind(&cover_image_url)
    .bind(&environment_summary)
    .bind(&payment_mode)
    .bind(deposit_ratio)
    .bind(now)
    .execute(pool)
    .await
    .map_err(internal_error)?;

    let batch = load_batch(pool, id)
        .await?
        .ok_or_else(|| view_error(StatusCode::NOT_FOUND, ERR_BATCH_NOT_FOUND))?;

    Ok(api_ok_message(
        MSG_BATCH_CREATED,
        batch_summary_json(pool, &batch).await?,
    ))
}

/// 已解析好的上传件。
pub(crate) struct ImageUpload {
    file_name: String,
    content_type: String,
    bytes: Vec<u8>,
}

/// 解析 multipart；没有图片字段时返回 `None`（交给 400 分支）。
async fn read_image_upload(request: Request) -> Result<Option<ImageUpload>, Response> {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if !content_type.starts_with("multipart/form-data") {
        return Ok(None);
    }

    let Ok(mut multipart) = Multipart::from_request(request, &()).await else {
        return Ok(None);
    };

    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() != Some("image") {
            continue;
        }
        let file_name = field.file_name().unwrap_or_default().to_string();
        if file_name.is_empty() {
            continue;
        }
        let content_type = field.content_type().unwrap_or_default().to_string();
        let bytes = field.bytes().await.map_err(|err| {
            tracing::error!("compat 果园域读取上传件失败: {err}");
            ApiReject::bad_request(ERR_IMAGE_MISSING).into_response()
        })?;
        return Ok(Some(ImageUpload {
            file_name,
            content_type,
            bytes: bytes.to_vec(),
        }));
    }

    Ok(None)
}

/// `POST /api/v1/farmer/upload-image`：返回绝对 URL（蓝本 `request.build_absolute_uri`）。
pub(crate) async fn farmer_upload_impl(
    headers: &HeaderMap,
    upload: Option<ImageUpload>,
) -> TraceResult {
    let Some(upload) = upload else {
        return Err(view_error(StatusCode::BAD_REQUEST, ERR_IMAGE_MISSING));
    };
    if !upload.content_type.starts_with("image/") {
        return Err(view_error(StatusCode::BAD_REQUEST, ERR_IMAGE_TYPE));
    }
    if upload.bytes.len() > MAX_UPLOAD_BYTES {
        return Err(view_error(StatusCode::BAD_REQUEST, ERR_IMAGE_TOO_LARGE));
    }

    // 蓝本用 `os.path.splitext(name)[1].lower()`（**带点**）；不在白名单就退回 `.jpg`。
    let extension = match std::path::Path::new(&upload.file_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_lowercase())
    {
        Some(ext) if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif") => {
            format!(".{ext}")
        }
        _ => ".jpg".to_string(),
    };
    let file_name = format!("{}{extension}", Uuid::new_v4().simple());
    let directory = std::path::Path::new(MEDIA_UPLOAD_DIR);
    std::fs::create_dir_all(directory).map_err(|err| {
        tracing::error!("compat 果园域创建上传目录失败: {err}");
        drf_error(ApiReject::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal server error",
        ))
    })?;
    std::fs::write(directory.join(&file_name), &upload.bytes).map_err(|err| {
        tracing::error!("compat 果园域写入上传件失败: {err}");
        drf_error(ApiReject::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal server error",
        ))
    })?;

    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost");
    let url = format!("http://{host}{MEDIA_PUBLIC_PREFIX}/{file_name}");

    Ok(api_ok_message(MSG_IMAGE_UPLOADED, json!({ "url": url })))
}

/// `GET /api/v1/farmer/products`。
pub(crate) async fn farmer_products_impl(pool: &PgPool, user: &AuthUser) -> TraceResult {
    let rows = sqlx::query(&format!(
        r#"SELECT {PRODUCT_COLUMNS} FROM citrus_product p
             LEFT JOIN "user" u ON u.id = p.seller_id
            WHERE p.seller_id = $1 {PRODUCT_ORDER}"#
    ))
    .bind(user.id)
    .fetch_all(pool)
    .await
    .map_err(internal_error)?;

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(product_json(pool, &ProductRow::from_row(row)?).await?);
    }

    Ok(api_ok(Value::Array(items)))
}

/// `POST /api/v1/farmer/products`：`FarmerProductCreateSerializer`。
pub(crate) async fn farmer_product_create_impl(
    pool: &PgPool,
    user: &AuthUser,
    body: &Value,
) -> TraceResult {
    let map = body.as_object().cloned().unwrap_or_default();
    let mut errors = FieldErrors::default();
    let name = required_text(&map, "name", &mut errors);
    let origin = required_text(&map, "origin", &mut errors);
    let price = required_decimal(&map, "price", &mut errors);
    if name.is_none() || origin.is_none() || price.is_none() || !errors.is_empty() {
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    }
    let (name, origin, price) = (name.unwrap(), origin.unwrap(), price.unwrap());

    if price < Decimal::ZERO {
        let mut errors = FieldErrors::default();
        errors.push("price", "价格不能为负数");
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    }
    let stock = optional_int(&map, "stock").unwrap_or(0);
    if stock < 0 {
        let mut errors = FieldErrors::default();
        errors.push("stock", "库存不能为负数");
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    }

    let sales_batch_id = map
        .get("sales_batch")
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok());
    let batch = match sales_batch_id {
        Some(id) => load_batch(pool, id).await?,
        None => None,
    };
    let Some(batch) = batch else {
        return Err(view_error(StatusCode::BAD_REQUEST, ERR_PRODUCT_NEEDS_BATCH));
    };
    if owner_of(pool, batch.orchard_id).await? != Some(user.id) {
        return Err(view_error(StatusCode::FORBIDDEN, ERR_BATCH_NOT_YOURS));
    }

    let id = Uuid::new_v4();
    let now = Utc::now();
    sqlx::query(
        r#"INSERT INTO citrus_product
               (id, seller_id, sales_batch_id, name, sku_type, fruit_type, variety, origin,
                description, price, unit, stock, sweetness, grade, harvest_date, shipping_note,
                cover_image_url, purchase_limit, minimum_order_quantity, status, created_at,
                updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                   $18, $19, $20, $21, $21)"#,
    )
    .bind(id)
    .bind(user.id)
    .bind(batch.id)
    .bind(&name)
    .bind(optional_text(&map, "sku_type").unwrap_or_else(|| "family".into()))
    .bind(optional_text(&map, "fruit_type").unwrap_or_else(|| "脐橙".into()))
    .bind(optional_text(&map, "variety").unwrap_or_else(|| "纽荷尔脐橙".into()))
    .bind(&origin)
    .bind(optional_text(&map, "description").unwrap_or_default())
    .bind(price)
    .bind(optional_text(&map, "unit").unwrap_or_else(|| "5斤/箱".into()))
    .bind(stock)
    .bind(optional_decimal(&map, "sweetness"))
    .bind(optional_text(&map, "grade").unwrap_or_default())
    .bind(optional_date(&map, "harvest_date"))
    .bind(optional_text(&map, "shipping_note").unwrap_or_default())
    .bind(optional_text(&map, "cover_image_url").unwrap_or_default())
    .bind(optional_int(&map, "purchase_limit").unwrap_or(20))
    .bind(optional_int(&map, "minimum_order_quantity").unwrap_or(1))
    .bind(optional_text(&map, "status").unwrap_or_else(|| "draft".into()))
    .bind(now)
    .execute(pool)
    .await
    .map_err(internal_error)?;

    let product = load_product(pool, id)
        .await?
        .ok_or_else(|| view_error(StatusCode::NOT_FOUND, ERR_PRODUCT_NOT_FOUND_OR_FORBIDDEN))?;

    Ok(api_ok_message(
        MSG_PRODUCT_SAVED,
        product_json(pool, &product).await?,
    ))
}

async fn owner_of(pool: &PgPool, orchard_id: Uuid) -> Result<Option<Uuid>, Response> {
    sqlx::query_scalar("SELECT owner_id FROM orchard WHERE id = $1")
        .bind(orchard_id)
        .fetch_optional(pool)
        .await
        .map_err(internal_error)
}

/// `PATCH|PUT /api/v1/farmer/products/<uuid>`（蓝本对两者都传 `partial=True`）。
pub(crate) async fn farmer_product_update_impl(
    pool: &PgPool,
    user: &AuthUser,
    product_id: Uuid,
    body: &Value,
) -> TraceResult {
    let Some(existing) = load_product(pool, product_id).await? else {
        return Err(view_error(StatusCode::NOT_FOUND, ERR_PRODUCT_NOT_FOUND_OR_FORBIDDEN));
    };
    let seller: Option<Uuid> = sqlx::query_scalar("SELECT seller_id FROM citrus_product WHERE id = $1")
        .bind(product_id)
        .fetch_one(pool)
        .await
        .map_err(internal_error)?;
    if seller != Some(user.id) {
        return Err(view_error(StatusCode::NOT_FOUND, ERR_PRODUCT_NOT_FOUND_OR_FORBIDDEN));
    }

    let map = body.as_object().cloned().unwrap_or_default();
    let mut errors = FieldErrors::default();
    if let Some(price) = optional_decimal(&map, "price")
        && price < Decimal::ZERO
    {
        errors.push("price", "价格不能为负数");
    }
    if let Some(stock) = optional_int(&map, "stock")
        && stock < 0
    {
        errors.push("stock", "库存不能为负数");
    }
    if !errors.is_empty() {
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    }

    let new_batch_id = match map.get("sales_batch") {
        Some(Value::Null) | None => existing.sales_batch_id,
        Some(value) => value
            .as_str()
            .and_then(|raw| Uuid::parse_str(raw).ok()),
    };
    if let Some(batch_id) = map
        .get("sales_batch")
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok())
    {
        let batch = load_batch(pool, batch_id)
            .await?
            .ok_or_else(|| view_error(StatusCode::NOT_FOUND, ERR_BATCH_NOT_FOUND))?;
        if owner_of(pool, batch.orchard_id).await? != Some(user.id) {
            return Err(view_error(StatusCode::FORBIDDEN, ERR_BATCH_NOT_YOURS));
        }
    }

    let name = optional_text(&map, "name").unwrap_or(existing.name);
    let sku_type = optional_text(&map, "sku_type").unwrap_or(existing.sku_type);
    let fruit_type = optional_text(&map, "fruit_type").unwrap_or(existing.fruit_type);
    let variety = optional_text(&map, "variety").unwrap_or(existing.variety);
    let origin = optional_text(&map, "origin").unwrap_or(existing.origin);
    let description = optional_text(&map, "description").unwrap_or(existing.description);
    let price = optional_decimal(&map, "price").unwrap_or(existing.price);
    let unit = optional_text(&map, "unit").unwrap_or(existing.unit);
    let stock = optional_int(&map, "stock").unwrap_or(existing.stock);
    let sweetness = match map.get("sweetness") {
        Some(Value::Null) => None,
        Some(_) => optional_decimal(&map, "sweetness"),
        None => existing.sweetness,
    };
    let grade = optional_text(&map, "grade").unwrap_or(existing.grade);
    let harvest_date = match map.get("harvest_date") {
        Some(Value::Null) => None,
        Some(_) => optional_date(&map, "harvest_date"),
        None => existing.harvest_date,
    };
    let shipping_note = optional_text(&map, "shipping_note").unwrap_or(existing.shipping_note);
    let cover_image_url =
        optional_text(&map, "cover_image_url").unwrap_or(existing.cover_image_url);
    let purchase_limit = optional_int(&map, "purchase_limit").unwrap_or(existing.purchase_limit);
    let minimum_order_quantity = optional_int(&map, "minimum_order_quantity")
        .unwrap_or(existing.minimum_order_quantity);
    let status = optional_text(&map, "status").unwrap_or(existing.status);

    sqlx::query(
        r#"UPDATE citrus_product
              SET sales_batch_id = $2, name = $3, sku_type = $4, fruit_type = $5, variety = $6,
                  origin = $7, description = $8, price = $9, unit = $10, stock = $11,
                  sweetness = $12, grade = $13, harvest_date = $14, shipping_note = $15,
                  cover_image_url = $16, purchase_limit = $17, minimum_order_quantity = $18,
                  status = $19, updated_at = $20
            WHERE id = $1"#,
    )
    .bind(product_id)
    .bind(new_batch_id)
    .bind(&name)
    .bind(&sku_type)
    .bind(&fruit_type)
    .bind(&variety)
    .bind(&origin)
    .bind(&description)
    .bind(price)
    .bind(&unit)
    .bind(stock)
    .bind(sweetness)
    .bind(&grade)
    .bind(harvest_date)
    .bind(&shipping_note)
    .bind(&cover_image_url)
    .bind(purchase_limit)
    .bind(minimum_order_quantity)
    .bind(&status)
    .bind(Utc::now())
    .execute(pool)
    .await
    .map_err(internal_error)?;

    let product = load_product(pool, product_id)
        .await?
        .ok_or_else(|| view_error(StatusCode::NOT_FOUND, ERR_PRODUCT_NOT_FOUND_OR_FORBIDDEN))?;

    Ok(api_ok_message(
        MSG_PRODUCT_UPDATED,
        product_json(pool, &product).await?,
    ))
}

/// 批次归属校验；文案按调用点区分（harvest-archives 用长句，其余三处用短句）。
async fn own_batch(
    pool: &PgPool,
    user: &AuthUser,
    batch_id: Uuid,
    not_found: &str,
) -> Result<BatchRow, Response> {
    let row = sqlx::query(&format!(
        r#"SELECT {BATCH_COLUMNS} FROM sales_batch b
             JOIN orchard o ON o.id = b.orchard_id
            WHERE b.id = $1 AND o.owner_id = $2"#
    ))
    .bind(batch_id)
    .bind(user.id)
    .fetch_optional(pool)
    .await
    .map_err(internal_error)?;

    row.as_ref()
        .map(BatchRow::from_row)
        .transpose()?
        .ok_or_else(|| view_error(StatusCode::NOT_FOUND, not_found))
}

/// `GET /api/v1/farmer/batches/<uuid>/harvest-archives`。
pub(crate) async fn farmer_harvest_list_impl(
    pool: &PgPool,
    user: &AuthUser,
    batch_id: Uuid,
) -> TraceResult {
    own_batch(pool, user, batch_id, ERR_BATCH_NOT_FOUND_OR_FORBIDDEN).await?;
    let items = load_harvest_archives(pool, batch_id)
        .await?
        .iter()
        .map(HarvestRow::to_json)
        .collect::<Vec<_>>();

    Ok(api_ok(Value::Array(items)))
}

/// `POST /api/v1/farmer/batches/<uuid>/harvest-archives`：落档案 + 追加一条 harvest 事件。
pub(crate) async fn farmer_harvest_create_impl(
    pool: &PgPool,
    user: &AuthUser,
    batch_id: Uuid,
    body: &Value,
) -> TraceResult {
    let batch = own_batch(pool, user, batch_id, ERR_BATCH_NOT_FOUND_OR_FORBIDDEN).await?;
    let map = body.as_object().cloned().unwrap_or_default();

    let mut errors = FieldErrors::default();
    let harvested_at = required_datetime(&map, "harvested_at", &mut errors);
    let picker = required_text(&map, "picker", &mut errors);
    let quantity_kg = required_decimal(&map, "quantity_kg", &mut errors);
    if harvested_at.is_none() || picker.is_none() || quantity_kg.is_none() || !errors.is_empty() {
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    }
    let (harvested_at, picker, quantity_kg) =
        (harvested_at.unwrap(), picker.unwrap(), quantity_kg.unwrap());

    let tree_id = map
        .get("tree")
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok());
    if let Some(tree_id) = tree_id {
        let orchard_id: Option<Uuid> =
            sqlx::query_scalar("SELECT orchard_id FROM fruit_tree_archive WHERE id = $1")
                .bind(tree_id)
                .fetch_optional(pool)
                .await
                .map_err(internal_error)?;
        match orchard_id {
            None => {
                let mut errors = FieldErrors::default();
                errors.push(
                    "tree",
                    &format!("无效主键 “{tree_id}” － 对象不存在。"),
                );
                return Ok(errors.into_response(StatusCode::BAD_REQUEST));
            }
            Some(orchard_id) if orchard_id != batch.orchard_id => {
                return Err(view_error(StatusCode::BAD_REQUEST, ERR_TREE_NOT_IN_ORCHARD));
            }
            Some(_) => {}
        }
    }

    let plot_name = optional_text(&map, "plot_name").unwrap_or_default();
    let harvest_method =
        optional_text(&map, "harvest_method").unwrap_or_else(|| "人工分批采摘".into());
    let maturity_brix = optional_decimal(&map, "maturity_brix");
    let grade = optional_text(&map, "grade").unwrap_or_default();
    let pre_harvest_status = optional_text(&map, "pre_harvest_status").unwrap_or_default();
    let harvest_weather = optional_text(&map, "harvest_weather").unwrap_or_default();
    let fruit_condition = optional_text(&map, "fruit_condition").unwrap_or_else(|| "good".into());
    let appearance_note = optional_text(&map, "appearance_note").unwrap_or_default();
    let pest_status = optional_text(&map, "pest_status").unwrap_or_default();
    let damage_rate_percent = optional_decimal(&map, "damage_rate_percent");
    let summary = optional_text(&map, "summary").unwrap_or_default();
    let image_urls = map.get("image_urls").cloned().unwrap_or_else(|| json!([]));
    let video_urls = map.get("video_urls").cloned().unwrap_or_else(|| json!([]));

    let orchard = load_orchard(pool, batch.orchard_id)
        .await?
        .ok_or_else(|| view_error(StatusCode::NOT_FOUND, ERR_ORCHARD_NOT_PUBLIC))?;
    let tree_number = match tree_id {
        Some(tree_id) => sqlx::query_scalar::<_, String>("SELECT tree_number FROM fruit_tree_archive WHERE id = $1")
            .bind(tree_id)
            .fetch_optional(pool)
            .await
            .map_err(internal_error)?
            .unwrap_or_default(),
        None => String::new(),
    };

    let archive_id = Uuid::new_v4();
    let harvest_code = random_code("CGJ-HV-", 10);
    let now = Utc::now();

    let mut tx = pool.begin().await.map_err(internal_error)?;
    sqlx::query(
        r#"INSERT INTO harvest_archive
               (id, harvest_code, batch_id, tree_id, harvested_at, picker, plot_name,
                harvest_method, quantity_kg, maturity_brix, grade, pre_harvest_status,
                harvest_weather, fruit_condition, appearance_note, pest_status, damage_rate_percent,
                summary, image_urls, video_urls, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                   $18, $19, $20, $21, $21)"#,
    )
    .bind(archive_id)
    .bind(&harvest_code)
    .bind(batch.id)
    .bind(tree_id)
    .bind(harvested_at)
    .bind(&picker)
    .bind(&plot_name)
    .bind(&harvest_method)
    .bind(quantity_kg)
    .bind(maturity_brix)
    .bind(&grade)
    .bind(&pre_harvest_status)
    .bind(&harvest_weather)
    .bind(&fruit_condition)
    .bind(&appearance_note)
    .bind(&pest_status)
    .bind(damage_rate_percent)
    .bind(&summary)
    .bind(&image_urls)
    .bind(&video_urls)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;

    // 蓝本在同一个事务里追加 harvest 事件（哈希链参与方）。
    let event_title = format!("采摘档案 {harvest_code}");
    let location = if plot_name.is_empty() {
        orchard.origin_text()
    } else {
        plot_name.clone()
    };
    let actor = if picker.is_empty() {
        user.username.clone()
    } else {
        picker.clone()
    };
    let event_data = json!({
        "quantityKg": ser::dec_scaled(quantity_kg, 2),
        "maturityBrix": maturity_brix.map(|value| ser::dec_scaled(value, 1)).unwrap_or_default(),
        "treeNumber": tree_number,
        "preHarvestStatus": pre_harvest_status,
        "fruitCondition": fruit_condition,
        "appearanceNote": appearance_note,
        "pestStatus": pest_status,
        "damageRatePercent": damage_rate_percent
            .map(|value| ser::dec_scaled(value, 2))
            .unwrap_or_default(),
        "videoUrls": video_urls,
    });

    append_trace_event(
        &mut tx,
        &batch.code,
        batch.id,
        HashInput {
            event_type: "harvest",
            source_type: "farmer",
            occurred_at: harvested_at,
            title: &event_title,
            description: &summary,
            location: &location,
            actor: &actor,
            source_reference: &harvest_code,
            image_urls: &image_urls,
            data: &event_data,
            previous_hash: "",
        },
    )
    .await?;
    tx.commit().await.map_err(internal_error)?;

    let row = sqlx::query(&format!(
        r#"SELECT {HARVEST_COLUMNS} FROM harvest_archive h
             LEFT JOIN fruit_tree_archive t ON t.id = h.tree_id
            WHERE h.id = $1"#
    ))
    .bind(archive_id)
    .fetch_one(pool)
    .await
    .map_err(internal_error)?;

    Ok(api_ok_message(
        MSG_HARVEST_SAVED,
        HarvestRow::from_row(&row)?.to_json(),
    ))
}

/// `POST /api/v1/farmer/batches/<uuid>/trace-events`：`TraceEventCreateSerializer`。
pub(crate) async fn farmer_trace_event_impl(
    pool: &PgPool,
    user: &AuthUser,
    batch_id: Uuid,
    body: &Value,
) -> TraceResult {
    // 蓝本此处文案是短句（harvest-archives 才是「供货批次不存在或无权操作」）。
    let batch = own_batch(pool, user, batch_id, ERR_BATCH_NOT_FOUND_OR_FORBIDDEN_SHORT).await?;
    let map = body.as_object().cloned().unwrap_or_default();

    let mut errors = FieldErrors::default();
    let event_type = required_text(&map, "event_type", &mut errors);
    let occurred_at = required_datetime(&map, "occurred_at", &mut errors);
    let title = required_text(&map, "title", &mut errors);
    if event_type.is_none() || occurred_at.is_none() || title.is_none() || !errors.is_empty() {
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    }
    let (event_type, occurred_at, title) =
        (event_type.unwrap(), occurred_at.unwrap(), title.unwrap());

    // `source_type` 与 `actor` 由视图覆盖，请求里传什么都不生效。
    let source_type = "farmer".to_string();
    let actor = user.username.clone();
    let description = optional_text(&map, "description").unwrap_or_default();
    let location = optional_text(&map, "location").unwrap_or_default();
    let source_reference = optional_text(&map, "source_reference").unwrap_or_default();
    let image_urls = map.get("image_urls").cloned().unwrap_or_else(|| json!([]));
    let data = map.get("data").cloned().unwrap_or_else(|| json!({}));

    let id = Uuid::new_v4();
    let mut tx = pool.begin().await.map_err(internal_error)?;
    let previous_hash: Option<String> = sqlx::query_scalar(
        "SELECT evidence_hash FROM trace_event WHERE batch_id = $1 ORDER BY recorded_at DESC, id DESC LIMIT 1",
    )
    .bind(batch.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal_error)?;
    let previous_hash = previous_hash.unwrap_or_default();
    let hash = {
        let input = HashInput {
            event_type: &event_type,
            source_type: &source_type,
            occurred_at,
            title: &title,
            description: &description,
            location: &location,
            actor: &actor,
            source_reference: &source_reference,
            image_urls: &image_urls,
            data: &data,
            previous_hash: &previous_hash,
        };
        evidence_hash(&batch.code, &input)
    };

    let recorded_at = Utc::now();
    sqlx::query(
        r#"INSERT INTO trace_event
               (id, batch_id, event_type, source_type, occurred_at, title, description, location,
                actor, source_reference, image_urls, data, previous_hash, evidence_hash, recorded_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)"#,
    )
    .bind(id)
    .bind(batch.id)
    .bind(&event_type)
    .bind(&source_type)
    .bind(occurred_at)
    .bind(&title)
    .bind(&description)
    .bind(&location)
    .bind(&actor)
    .bind(&source_reference)
    .bind(&image_urls)
    .bind(&data)
    .bind(&previous_hash)
    .bind(&hash)
    .bind(recorded_at)
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;
    tx.commit().await.map_err(internal_error)?;

    let event = EventRow {
        id,
        event_type,
        source_type,
        occurred_at,
        title,
        description,
        location,
        actor,
        source_reference,
        image_urls,
        data,
        previous_hash,
        evidence_hash: hash,
        recorded_at,
    };

    Ok(api_ok_message(MSG_EVENT_APPENDED, event.to_json()))
}

/// `GET /api/v1/farmer/batches/<uuid>/quality-samples`（与 `health-records` 同实现）。
pub(crate) async fn farmer_quality_list_impl(
    pool: &PgPool,
    user: &AuthUser,
    batch_id: Uuid,
    product_id: Option<Uuid>,
) -> TraceResult {
    own_batch(pool, user, batch_id, ERR_BATCH_NOT_FOUND_OR_FORBIDDEN_SHORT).await?;
    let items = load_samples(pool, batch_id, product_id)
        .await?
        .iter()
        .map(SampleRow::to_json)
        .collect::<Vec<_>>();

    Ok(api_ok(Value::Array(items)))
}

/// `POST /api/v1/farmer/batches/<uuid>/quality-samples`（与 `health-records` 同实现）。
pub(crate) async fn farmer_quality_create_impl(
    pool: &PgPool,
    user: &AuthUser,
    batch_id: Uuid,
    body: &Value,
) -> TraceResult {
    let batch = own_batch(pool, user, batch_id, ERR_BATCH_NOT_FOUND_OR_FORBIDDEN_SHORT).await?;
    let map = body.as_object().cloned().unwrap_or_default();

    let mut errors = FieldErrors::default();
    let sampled_at = required_datetime(&map, "sampled_at", &mut errors);
    if sampled_at.is_none() || !errors.is_empty() {
        return Ok(errors.into_response(StatusCode::BAD_REQUEST));
    }
    let sampled_at = sampled_at.unwrap();

    let product_id = map
        .get("product")
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok());
    let product = match product_id {
        Some(id) => {
            let row = load_product(pool, id).await?;
            match row {
                Some(row) if row.sales_batch_id == Some(batch.id) => Some(row),
                Some(_) => return Err(view_error(StatusCode::BAD_REQUEST, ERR_SAMPLE_PRODUCT_MISMATCH)),
                None => {
                    let mut errors = FieldErrors::default();
                    errors.push("product", "无效主键 － 对象不存在。");
                    return Ok(errors.into_response(StatusCode::BAD_REQUEST));
                }
            }
        }
        None => None,
    };

    let archive_id = map
        .get("harvest_archive")
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok());
    let harvest_code = match archive_id {
        Some(id) => {
            let batch_id_of_archive: Option<Uuid> =
                sqlx::query_scalar("SELECT batch_id FROM harvest_archive WHERE id = $1")
                    .bind(id)
                    .fetch_optional(pool)
                    .await
                    .map_err(internal_error)?;
            match batch_id_of_archive {
                Some(found) if found == batch.id => {
                    sqlx::query_scalar::<_, String>("SELECT harvest_code FROM harvest_archive WHERE id = $1")
                        .bind(id)
                        .fetch_one(pool)
                        .await
                        .map_err(internal_error)?
                }
                Some(_) => return Err(view_error(StatusCode::BAD_REQUEST, ERR_SAMPLE_ARCHIVE_MISMATCH)),
                None => {
                    let mut errors = FieldErrors::default();
                    errors.push("harvest_archive", "无效主键 － 对象不存在。");
                    return Ok(errors.into_response(StatusCode::BAD_REQUEST));
                }
            }
        }
        None => String::new(),
    };

    let inspection_stage =
        optional_text(&map, "inspection_stage").unwrap_or_else(|| "pre_harvest".into());
    let health_status = optional_text(&map, "health_status").unwrap_or_else(|| "qualified".into());
    let sample_size = optional_int(&map, "sample_size").unwrap_or(1);
    let sweetness_brix = optional_decimal(&map, "sweetness_brix");
    let acidity = optional_decimal(&map, "acidity");
    let average_weight_grams = optional_int(&map, "average_weight_grams");
    let diameter_mm = optional_decimal(&map, "diameter_mm");
    let grade = optional_text(&map, "grade").unwrap_or_default();
    let result = optional_text(&map, "result").unwrap_or_else(|| "符合本批次标准".into());
    let appearance_status = optional_text(&map, "appearance_status").unwrap_or_default();
    let pest_status = optional_text(&map, "pest_status").unwrap_or_default();
    let pesticide_residue_result =
        optional_text(&map, "pesticide_residue_result").unwrap_or_default();
    let inspector = optional_text(&map, "inspector").unwrap_or_default();
    let report_image_url = optional_text(&map, "report_image_url").unwrap_or_default();
    let note = optional_text(&map, "note").unwrap_or_default();

    let sample_id = Uuid::new_v4();
    let now = Utc::now();

    let mut tx = pool.begin().await.map_err(internal_error)?;
    sqlx::query(
        r#"INSERT INTO batch_quality_sample
               (id, batch_id, product_id, harvest_archive_id, sampled_at, inspection_stage,
                health_status, sample_size, sweetness_brix, acidity, average_weight_grams,
                diameter_mm, grade, result, appearance_status, pest_status,
                pesticide_residue_result, inspector, report_image_url, note, created_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                   $18, $19, $20, $21)"#,
    )
    .bind(sample_id)
    .bind(batch.id)
    .bind(product_id)
    .bind(archive_id)
    .bind(sampled_at)
    .bind(&inspection_stage)
    .bind(&health_status)
    .bind(sample_size)
    .bind(sweetness_brix)
    .bind(acidity)
    .bind(average_weight_grams)
    .bind(diameter_mm)
    .bind(&grade)
    .bind(&result)
    .bind(&appearance_status)
    .bind(&pest_status)
    .bind(&pesticide_residue_result)
    .bind(&inspector)
    .bind(&report_image_url)
    .bind(&note)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(internal_error)?;

    // 蓝本同一事务里追加 quality 事件，标题用商品名（无商品时退回批次标题）。
    let orchard = load_orchard(pool, batch.orchard_id)
        .await?
        .ok_or_else(|| view_error(StatusCode::NOT_FOUND, ERR_ORCHARD_NOT_PUBLIC))?;
    let product_name = product
        .as_ref()
        .map(|row| row.name.clone())
        .unwrap_or_default();
    let target_name = if product_name.is_empty() {
        batch.title.clone()
    } else {
        product_name.clone()
    };
    let event_title = format!("{target_name}健康检查");
    let event_description = if note.is_empty() { result.clone() } else { note.clone() };
    let actor = if inspector.is_empty() {
        user.username.clone()
    } else {
        inspector.clone()
    };
    let event_images = if report_image_url.is_empty() {
        json!([])
    } else {
        json!([report_image_url])
    };
    let event_data = json!({
        "productId": product_id.map(|id| id.to_string()).unwrap_or_default(),
        "productName": product_name,
        "inspectionStage": inspection_stage,
        "healthStatus": health_status,
        "sampleSize": sample_size,
        "sweetnessBrix": sweetness_brix.map(|value| ser::dec_scaled(value, 1)).unwrap_or_default(),
        "diameterMm": diameter_mm.map(|value| ser::dec_scaled(value, 1)).unwrap_or_default(),
        "averageWeightGrams": average_weight_grams.map(|value| value.to_string()).unwrap_or_default(),
        "appearanceStatus": appearance_status,
        "pestStatus": pest_status,
        "pesticideResidueResult": pesticide_residue_result,
        "harvestCode": harvest_code,
    });

    append_trace_event(
        &mut tx,
        &batch.code,
        batch.id,
        HashInput {
            event_type: "quality",
            source_type: "quality",
            occurred_at: sampled_at,
            title: &event_title,
            description: &event_description,
            location: &orchard.origin_text(),
            actor: &actor,
            source_reference: &sample_id.to_string(),
            image_urls: &event_images,
            data: &event_data,
            previous_hash: "",
        },
    )
    .await?;
    tx.commit().await.map_err(internal_error)?;

    let row = sqlx::query(&format!(
        r#"SELECT {SAMPLE_COLUMNS} FROM batch_quality_sample s
             LEFT JOIN citrus_product p ON p.id = s.product_id
             LEFT JOIN harvest_archive a ON a.id = s.harvest_archive_id
            WHERE s.id = $1"#
    ))
    .bind(sample_id)
    .fetch_one(pool)
    .await
    .map_err(internal_error)?;

    Ok(api_ok_message(
        MSG_SAMPLE_SAVED,
        SampleRow::from_row(&row)?.to_json(),
    ))
}

// --------------------------------------------------------------------------------------
// 输入解析（DRF 字段语义）
// --------------------------------------------------------------------------------------

fn field_error<T>(errors: &mut FieldErrors, field: &str, message: &str) -> Option<T> {
    errors.push(field, message);
    None
}

fn required_text(map: &Map<String, Value>, field: &str, errors: &mut FieldErrors) -> Option<String> {
    match map.get(field) {
        None => field_error(errors, field, ERR_REQUIRED),
        Some(Value::Null) => field_error(errors, field, ERR_NULL),
        Some(Value::String(text)) if text.is_empty() => field_error(errors, field, ERR_BLANK),
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => field_error(errors, field, ERR_REQUIRED),
    }
}

fn optional_text(map: &Map<String, Value>, field: &str) -> Option<String> {
    match map.get(field) {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    }
}

fn required_decimal(
    map: &Map<String, Value>,
    field: &str,
    errors: &mut FieldErrors,
) -> Option<Decimal> {
    match map.get(field) {
        None => field_error(errors, field, ERR_REQUIRED),
        Some(Value::Null) => field_error(errors, field, ERR_NULL),
        Some(value) => match decimal_from_value(value) {
            Some(decimal) => Some(decimal),
            None => field_error(errors, field, ERR_INVALID_NUMBER),
        },
    }
}

fn optional_decimal(map: &Map<String, Value>, field: &str) -> Option<Decimal> {
    map.get(field).and_then(decimal_from_value)
}

fn decimal_from_value(value: &Value) -> Option<Decimal> {
    match value {
        Value::Number(number) => Decimal::from_str(&number.to_string()).ok(),
        Value::String(text) => Decimal::from_str(text.trim()).ok(),
        _ => None,
    }
}

fn optional_int(map: &Map<String, Value>, field: &str) -> Option<i32> {
    match map.get(field) {
        Some(Value::Number(number)) => number.as_i64().map(|value| value as i32),
        Some(Value::String(text)) => text.trim().parse::<i32>().ok(),
        _ => None,
    }
}

fn required_datetime(
    map: &Map<String, Value>,
    field: &str,
    errors: &mut FieldErrors,
) -> Option<DateTime<Utc>> {
    match map.get(field) {
        None => field_error(errors, field, ERR_REQUIRED),
        Some(Value::Null) => field_error(errors, field, ERR_NULL),
        Some(value) => match datetime_from_value(value) {
            Some(parsed) => Some(parsed),
            None => field_error(errors, field, ERR_INVALID_DATETIME),
        },
    }
}

fn optional_datetime(map: &Map<String, Value>, field: &str) -> Option<DateTime<Utc>> {
    map.get(field).and_then(datetime_from_value)
}

fn datetime_from_value(value: &Value) -> Option<DateTime<Utc>> {
    let text = value.as_str()?;
    DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

fn optional_date(map: &Map<String, Value>, field: &str) -> Option<NaiveDate> {
    let text = map.get(field)?.as_str()?;
    NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()
}

/// Postgres 唯一约束冲突（`23505`）。
fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(|db_err| db_err.code())
        .map(|code| code == "23505")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    //! 纯函数单测放在这里；需要 scratch schema 的回归测试见 `tests/trace_tests.rs`。

    use super::*;

    #[test]
    fn canonical_payload_matches_blueprint_shape() {
        let data = json!({});
        let images = json!([]);
        let payload = canonical_payload(
            "CGJ-2026-XF-001",
            &HashInput {
                event_type: "orchard",
                source_type: "operator",
                occurred_at: "2026-08-11T14:20:37.928176Z"
                    .parse::<DateTime<Utc>>()
                    .unwrap(),
                title: "合作果园完成建档核验",
                description: "核验合作果园主体、位置、种植品种和本批次供货意向。",
                location: "江西省赣州市信丰县",
                actor: "橙管家运营",
                source_reference: "DEMO-CGJ-2026-XF-001-40",
                image_urls: &images,
                data: &data,
                previous_hash: "",
            },
        );

        assert_eq!(
            payload,
            concat!(
                r#"{"actor":"橙管家运营","batch":"CGJ-2026-XF-001","data":{},"#,
                r#""description":"核验合作果园主体、位置、种植品种和本批次供货意向。","#,
                r#""eventType":"orchard","imageUrls":[],"location":"江西省赣州市信丰县","#,
                r#""occurredAt":"2026-08-11T14:20:37.928176+00:00","previousHash":"","#,
                r#""sourceReference":"DEMO-CGJ-2026-XF-001-40","sourceType":"operator","#,
                r#""title":"合作果园完成建档核验"}"#
            )
        );
    }

    #[test]
    fn mask_tracking_number_matches_blueprint() {
        assert_eq!(mask_tracking_number("SF1234567890"), "SF1******890");
        // `len <= 6` 才原样返回；7 位会变成 `3 + 1 星 + 3`。
        assert_eq!(mask_tracking_number("SF12345"), "SF1*345");
        assert_eq!(mask_tracking_number("SF123"), "SF123");
        assert_eq!(mask_tracking_number(""), "");
    }

    #[test]
    fn progress_percent_uses_python_rounding() {
        assert_eq!(python_round(15.25), 15);
        assert_eq!(python_round(0.5), 0);
        assert_eq!(python_round(1.5), 2);
        assert_eq!(python_round(2.5), 2);
    }

    #[test]
    fn labels_fall_back_to_raw_value() {
        assert_eq!(label(EVENT_TYPE, "orchard"), "果园建档");
        assert_eq!(label(EVENT_TYPE, "unknown"), "unknown");
    }

    #[test]
    fn random_code_uses_expected_prefix_and_length() {
        let tree = random_code("CGJ-TREE-", 10);
        assert_eq!(tree.len(), "CGJ-TREE-".len() + 10);
        assert!(tree.trim_start_matches("CGJ-TREE-").chars().all(|c| c.is_ascii_hexdigit()));
    }
}
