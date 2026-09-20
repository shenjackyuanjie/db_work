//! 智能体域规则引擎。
//!
//! 蓝本为 `navel_backend_git/api/agent_service.py`。该模块**以规则与 SQL 为主**，
//! LLM 只用于意图识别与摘要，且必须能在 `AGENT_LLM_API_KEY` 为空时走规则兜底
//! （契约基准即在该模式下录制）。
//!
//! 几个不能忘的对齐点：
//! - 列表顺序全部来自 Django 的 `Meta.ordering`，SQL 里显式写同样的 `ORDER BY`；
//! - `timezone.localdate()` 在蓝本 `TIME_ZONE='UTC'` 下是 **UTC 日期**，不要用服务器本地日期；
//! - `DecimalField` 经 `float()` 后按 `f'{x:.2f}'` 输出**字符串**，`FloatField` 输出**数字**；
//! - LLM 开关只认环境变量 `AGENT_LLM_API_KEY`（见 [`llm_api_key`]），
//!   蓝本里硬编码的密钥**不抄**（`DEVIATIONS.md` S1）；
//! - 蓝本 `agent_views.py` 漏 `import status` 的 6 个校验分支按 `DEVIATIONS.md` D1
//!   修正为设计意图的 400/404，**不要复刻 500**。

use std::collections::HashMap;

use axum::http::StatusCode;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::auth::AuthUser;
use super::errors::ApiReject;
use super::ser;

use crate::client::{ChatOptions, OpenRouterClient, SimpleChatRequest};

/// `llm_detect_intent` 允许的意图集合。
const LLM_INTENTS: [&str; 7] = [
    "select",
    "inquiry",
    "risk",
    "report",
    "repurchase",
    "context",
    "unknown",
];

fn db_error(err: sqlx::Error) -> ApiReject {
    tracing::error!("compat 智能体域查询失败: {err}");
    ApiReject::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}

/// 按给定键序构造 JSON 对象（`serde_json` 开了 `preserve_order`，键序即输出序）。
fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

/// Django `str(date)`：`2026-09-23`；`None` → `''`。
fn date_or_empty(value: Option<NaiveDate>) -> Value {
    Value::String(value.map(ser::dt_date).unwrap_or_default())
}

/// Python `str(float)`：整数值带 `.0`（`26.0`），其余走最短表示。
fn py_float(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

/// Python `str(datetime)`：空格分隔 + `+00:00` + 6 位微秒。
fn py_datetime(value: DateTime<Utc>) -> String {
    value
        .with_timezone(&Utc)
        .format("%Y-%m-%d %H:%M:%S%.6f+00:00")
        .to_string()
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

// --------------------------------------------------------------------------------------
// 极简扫描器：Cargo.toml 没有 `regex` 依赖（且不许加），这些查询文本又都是短串，
// 所以按 Python `re.search` 的「最左优先」语义手写扫描。`\s` 一律用
// `char::is_whitespace()`，与 Python 的 Unicode 空白语义对齐。
// --------------------------------------------------------------------------------------

fn skip_ws(chars: &[char], mut index: usize) -> usize {
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    index
}

/// `\d{min,max}`，贪婪取满 `max` 位；不足 `min` 位返回 `None`。
fn scan_digits(chars: &[char], start: usize, min: usize, max: usize) -> Option<(i64, usize)> {
    let mut end = start;
    while end < chars.len() && end - start < max && chars[end].is_ascii_digit() {
        end += 1;
    }
    if end - start < min {
        return None;
    }
    chars[start..end]
        .iter()
        .collect::<String>()
        .parse::<i64>()
        .ok()
        .map(|value| (value, end))
}

/// `\d+(?:\.\d+)?`：小数点后必须有数字才纳入小数部分。
fn scan_number(chars: &[char], start: usize) -> Option<(f64, usize)> {
    let (_, mut end) = scan_digits(chars, start, 1, usize::MAX)?;
    if end < chars.len() && chars[end] == '.' {
        let mut after = end + 1;
        while after < chars.len() && chars[after].is_ascii_digit() {
            after += 1;
        }
        if after > end + 1 {
            end = after;
        }
    }
    chars[start..end]
        .iter()
        .collect::<String>()
        .parse::<f64>()
        .ok()
        .map(|value| (value, end))
}

/// `(?:\d+(?:\.\d+)?)\s*元` → 预算值。
fn scan_budget(text: &str) -> Option<f64> {
    let chars: Vec<char> = text.chars().collect();
    for start in 0..chars.len() {
        if !chars[start].is_ascii_digit() {
            continue;
        }
        let Some((value, end)) = scan_number(&chars, start) else {
            continue;
        };
        let after = skip_ws(&chars, end);
        if after < chars.len() && chars[after] == '元' {
            return Some(value);
        }
    }
    None
}

/// `(\d+\s*斤)`：返回值**含**中间空白与「斤」字（蓝本 group(1) 就是这样）。
fn scan_unit_kw(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    for start in 0..chars.len() {
        if !chars[start].is_ascii_digit() {
            continue;
        }
        let Some((_, end)) = scan_digits(&chars, start, 1, usize::MAX) else {
            continue;
        };
        let after = skip_ws(&chars, end);
        if after < chars.len() && chars[after] == '斤' {
            return Some(chars[start..=after].iter().collect());
        }
    }
    None
}

/// `(\d+)\s*(?:箱|件)` → 数量。
fn scan_quantity(text: &str) -> Option<i64> {
    let chars: Vec<char> = text.chars().collect();
    for start in 0..chars.len() {
        if !chars[start].is_ascii_digit() {
            continue;
        }
        let Some((value, end)) = scan_digits(&chars, start, 1, usize::MAX) else {
            continue;
        };
        let after = skip_ws(&chars, end);
        if after < chars.len() && (chars[after] == '箱' || chars[after] == '件') {
            return Some(value);
        }
    }
    None
}

/// `(\d{4}-\d{1,2}-\d{1,2})` → `YYYY-MM-DD`；日期非法时蓝本 `pass`（不再继续找）。
fn scan_iso_date(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    for start in 0..chars.len() {
        if !chars[start].is_ascii_digit() {
            continue;
        }
        let Some((year, after_year)) = scan_digits(&chars, start, 4, 4) else {
            continue;
        };
        if chars.get(after_year) != Some(&'-') {
            continue;
        }
        let Some((month, after_month)) = scan_digits(&chars, after_year + 1, 1, 2) else {
            continue;
        };
        if chars.get(after_month) != Some(&'-') {
            continue;
        }
        let Some((day, _)) = scan_digits(&chars, after_month + 1, 1, 2) else {
            continue;
        };
        return NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32).map(ser::dt_date);
    }
    None
}

/// `(\d{1,2})\s*月(\d{1,2})\s*日前` → `2026-MM-DD`（**不做**合法性校验，与蓝本一致）。
fn scan_month_day(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    for start in 0..chars.len() {
        if !chars[start].is_ascii_digit() {
            continue;
        }
        let Some((month, after_month)) = scan_digits(&chars, start, 1, 2) else {
            continue;
        };
        let month_sep = skip_ws(&chars, after_month);
        if chars.get(month_sep) != Some(&'月') {
            continue;
        }
        let Some((day, after_day)) = scan_digits(&chars, month_sep + 1, 1, 2) else {
            continue;
        };
        let day_sep = skip_ws(&chars, after_day);
        if chars[day_sep..].starts_with(&['日', '前']) {
            return Some(format!("2026-{month:02}-{day:02}"));
        }
    }
    None
}

// --------------------------------------------------------------------------------------
// choices 的 display 映射（逐字取自 `api/models.py` 的 TextChoices）
// --------------------------------------------------------------------------------------

fn batch_status_display(status: &str) -> &str {
    match status {
        "draft" => "筹备中",
        "warming" => "即将上架",
        "open" => "在售",
        "closed" => "已停止销售",
        "harvesting" => "采摘中",
        "fulfilling" => "履约中",
        "completed" => "已完成",
        "cancelled" => "已取消",
        other => other,
    }
}

fn sku_type_display(sku_type: &str) -> &str {
    match sku_type {
        "trial" => "试吃装",
        "family" => "家庭装",
        "gift" => "礼赠装",
        "juice" => "榨汁装",
        "enterprise" => "企业装",
        "specialty" => "特色果品",
        other => other,
    }
}

fn order_status_display(status: &str) -> &str {
    match status {
        "pending_payment" => "待支付",
        "pending_deposit" => "待付订金",
        "pending_balance" => "待付尾款",
        "paid" => "待发货",
        "picking" => "采摘分选中",
        "packed" => "已装箱",
        "shipped" => "待收货",
        "completed" => "已完成",
        "after_sale" => "售后处理中",
        "cancelled" => "已取消",
        other => other,
    }
}

/// `AgentApproval.ticket_type` 的 display（`restock` 的中文是「品质抽检」，不是「补货」）。
pub(crate) fn ticket_type_display(ticket_type: &str) -> &str {
    match ticket_type {
        "quote" => "报价单",
        "risk_action" => "风险处置",
        "restock" => "品质抽检",
        "other" => "其他",
        other => other,
    }
}

/// `AgentApproval.status` 的 display。
pub(crate) fn approval_status_display(status: &str) -> &str {
    match status {
        "pending" => "待审批",
        "approved" => "已批准",
        "rejected" => "已拒绝",
        other => other,
    }
}

/// `AgentFeedback.rating` 的 display。
pub(crate) fn rating_display(rating: i16) -> &'static str {
    match rating {
        1 => "差",
        2 => "较差",
        3 => "一般",
        4 => "好",
        5 => "很好",
        _ => "",
    }
}

// --------------------------------------------------------------------------------------
// 行载体与序列化（`agent_views.py` 的 `_ctx_*` / `_approval_json` / `_feedback_json`）
// --------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct BatchRow {
    pub(crate) id: Uuid,
    pub(crate) code: String,
    pub(crate) trace_code: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) planned_quantity: i32,
    pub(crate) sold_quantity: i32,
    pub(crate) expected_harvest_start: Option<NaiveDate>,
    pub(crate) expected_harvest_end: Option<NaiveDate>,
    pub(crate) expected_ship_start: Option<NaiveDate>,
    pub(crate) expected_ship_end: Option<NaiveDate>,
    pub(crate) close_at: Option<DateTime<Utc>>,
}

impl BatchRow {
    /// `SalesBatch.available_quantity`：`max(planned - sold, 0)`。
    pub(crate) fn available_quantity(&self) -> i32 {
        (self.planned_quantity - self.sold_quantity).max(0)
    }

    /// 键序即蓝本 `_ctx_batch` 的字典字面量序。
    pub(crate) fn ctx(&self) -> Value {
        obj(vec![
            ("id", Value::String(self.id.to_string())),
            ("code", Value::String(self.code.clone())),
            ("trace_code", Value::String(self.trace_code.clone())),
            ("title", Value::String(self.title.clone())),
            ("status", Value::String(self.status.clone())),
            (
                "status_display",
                Value::String(batch_status_display(&self.status).to_string()),
            ),
            (
                "planned_quantity",
                Value::Number(self.planned_quantity.into()),
            ),
            ("sold_quantity", Value::Number(self.sold_quantity.into())),
            (
                "available_quantity",
                Value::Number(self.available_quantity().into()),
            ),
            (
                "expected_harvest_start",
                date_or_empty(self.expected_harvest_start),
            ),
            (
                "expected_harvest_end",
                date_or_empty(self.expected_harvest_end),
            ),
            (
                "expected_ship_start",
                date_or_empty(self.expected_ship_start),
            ),
            ("expected_ship_end", date_or_empty(self.expected_ship_end)),
        ])
    }
}

const BATCH_COLUMNS: &str = "b.id, b.code, b.trace_code, b.title, b.status, b.planned_quantity, \
                             b.sold_quantity, b.expected_harvest_start, b.expected_harvest_end, \
                             b.expected_ship_start, b.expected_ship_end, b.close_at";

/// Django `Meta.ordering = ['-is_featured', '-open_at', '-created_at']`。
///
/// `open_at` 有 NULL（未开售/筹备中的批次），而 `DESC` 默认为 `NULLS FIRST`，
/// 会把 NULL 排到最前；实测夹具是把 NULL 排在最后（`XF-001` 在 `C030B3`/`8C406B` 之前），
/// 所以显式写 `NULLS LAST`。
const BATCH_ORDER: &str =
    " ORDER BY b.is_featured DESC, b.open_at DESC NULLS LAST, b.created_at DESC";

fn batch_from_row(row: &sqlx::postgres::PgRow) -> Result<BatchRow, ApiReject> {
    Ok(BatchRow {
        id: row.try_get("id").map_err(db_error)?,
        code: row.try_get("code").map_err(db_error)?,
        trace_code: row.try_get("trace_code").map_err(db_error)?,
        title: row.try_get("title").map_err(db_error)?,
        status: row.try_get("status").map_err(db_error)?,
        planned_quantity: row.try_get("planned_quantity").map_err(db_error)?,
        sold_quantity: row.try_get("sold_quantity").map_err(db_error)?,
        expected_harvest_start: row.try_get("expected_harvest_start").map_err(db_error)?,
        expected_harvest_end: row.try_get("expected_harvest_end").map_err(db_error)?,
        expected_ship_start: row.try_get("expected_ship_start").map_err(db_error)?,
        expected_ship_end: row.try_get("expected_ship_end").map_err(db_error)?,
        close_at: row.try_get("close_at").map_err(db_error)?,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct ProductRow {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) sku_type: String,
    pub(crate) variety: String,
    pub(crate) origin: String,
    pub(crate) price: Decimal,
    pub(crate) unit: String,
    pub(crate) stock: i32,
    pub(crate) status: String,
    pub(crate) sales_batch_code: Option<String>,
    pub(crate) minimum_order_quantity: i32,
    pub(crate) sweetness: Option<Decimal>,
    /// `p.sales_batch is None or p.sales_batch.is_open`（`SalesBatch.is_open` 的属性语义）。
    pub(crate) batch_open: bool,
}

impl ProductRow {
    /// 键序即蓝本 `_ctx_product` 的字典字面量序；`price` 是 `float()` 后的**数字**。
    pub(crate) fn ctx(&self) -> Value {
        obj(vec![
            ("id", Value::String(self.id.to_string())),
            ("name", Value::String(self.name.clone())),
            ("sku_type", Value::String(self.sku_type.clone())),
            (
                "sku_type_display",
                Value::String(sku_type_display(&self.sku_type).to_string()),
            ),
            ("variety", Value::String(self.variety.clone())),
            ("origin", Value::String(self.origin.clone())),
            ("price", json!(self.price.to_f64().unwrap_or_default())),
            ("unit", Value::String(self.unit.clone())),
            ("stock", Value::Number(self.stock.into())),
            ("status", Value::String(self.status.clone())),
            (
                "sales_batch_code",
                Value::String(self.sales_batch_code.clone().unwrap_or_default()),
            ),
        ])
    }
}

/// Django `Meta.ordering = ['sort_order', '-created_at']`。
///
/// `batch_open` 复刻 `SalesBatch.is_open`（状态 open + 开售/停售窗口 + 仍有可供量），
/// `sales_batch_id` 为 NULL 时按蓝本 `p.sales_batch is None` 视为「不拦」。
const PRODUCT_COLUMNS: &str = "p.id, p.name, p.sku_type, p.variety, p.origin, p.price, p.unit, \
                               p.stock, p.status, p.minimum_order_quantity, p.sweetness, \
                               sb.code AS sales_batch_code, \
                               (p.sales_batch_id IS NULL OR (sb.status = 'open' \
                                  AND (sb.open_at IS NULL OR sb.open_at <= now()) \
                                  AND (sb.close_at IS NULL OR sb.close_at > now()) \
                                  AND (sb.planned_quantity - sb.sold_quantity) > 0)) AS batch_open";
const PRODUCT_ORDER: &str = " ORDER BY p.sort_order ASC, p.created_at DESC";

fn product_from_row(row: &sqlx::postgres::PgRow) -> Result<ProductRow, ApiReject> {
    Ok(ProductRow {
        id: row.try_get("id").map_err(db_error)?,
        name: row.try_get("name").map_err(db_error)?,
        sku_type: row.try_get("sku_type").map_err(db_error)?,
        variety: row.try_get("variety").map_err(db_error)?,
        origin: row.try_get("origin").map_err(db_error)?,
        price: row.try_get("price").map_err(db_error)?,
        unit: row.try_get("unit").map_err(db_error)?,
        stock: row.try_get("stock").map_err(db_error)?,
        status: row.try_get("status").map_err(db_error)?,
        sales_batch_code: row.try_get("sales_batch_code").map_err(db_error)?,
        minimum_order_quantity: row.try_get("minimum_order_quantity").map_err(db_error)?,
        sweetness: row.try_get("sweetness").map_err(db_error)?,
        batch_open: row.try_get("batch_open").map_err(db_error)?,
    })
}

/// 一个批次的抽检汇总（替代 Django 侧的多次 `quality_samples.filter(...).count()`）。
#[derive(Debug, Default, Clone, Copy)]
struct SampleStats {
    total: i64,
    qualified: i64,
    rejected: i64,
    watch: i64,
}

async fn sample_stats(
    pool: &PgPool,
    batch_ids: &[Uuid],
) -> Result<HashMap<Uuid, SampleStats>, ApiReject> {
    if batch_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query(
        r#"SELECT batch_id,
                  count(*) AS total,
                  count(*) FILTER (WHERE health_status = 'qualified') AS qualified,
                  count(*) FILTER (WHERE health_status = 'rejected') AS rejected,
                  count(*) FILTER (WHERE health_status = 'watch') AS watch
             FROM batch_quality_sample
            WHERE batch_id = ANY($1::uuid[])
            GROUP BY batch_id"#,
    )
    .bind(batch_ids)
    .fetch_all(pool)
    .await
    .map_err(db_error)?;

    let mut stats = HashMap::new();
    for row in rows {
        let batch_id: Uuid = row.try_get("batch_id").map_err(db_error)?;
        stats.insert(
            batch_id,
            SampleStats {
                total: row.try_get("total").map_err(db_error)?,
                qualified: row.try_get("qualified").map_err(db_error)?,
                rejected: row.try_get("rejected").map_err(db_error)?,
                watch: row.try_get("watch").map_err(db_error)?,
            },
        );
    }
    Ok(stats)
}

async fn fetch_batches(
    pool: &PgPool,
    where_clause: &str,
    owner: Option<Uuid>,
    limit: i64,
) -> Result<Vec<BatchRow>, ApiReject> {
    let sql = format!(
        "SELECT {BATCH_COLUMNS} FROM sales_batch b {where_clause}{BATCH_ORDER} LIMIT {limit}"
    );

    let mut query = sqlx::query(&sql);
    if let Some(owner) = owner {
        query = query.bind(owner);
    }
    let rows = query.fetch_all(pool).await.map_err(db_error)?;
    rows.iter().map(batch_from_row).collect()
}

async fn fetch_products(
    pool: &PgPool,
    where_clause: &str,
    bind: Option<Uuid>,
    limit: i64,
) -> Result<Vec<ProductRow>, ApiReject> {
    let sql = format!(
        "SELECT {PRODUCT_COLUMNS} FROM citrus_product p \
         LEFT JOIN sales_batch sb ON sb.id = p.sales_batch_id \
         {where_clause}{PRODUCT_ORDER} LIMIT {limit}"
    );

    let mut query = sqlx::query(&sql);
    if let Some(value) = bind {
        query = query.bind(value);
    }
    let rows = query.fetch_all(pool).await.map_err(db_error)?;
    rows.iter().map(product_from_row).collect()
}

// --------------------------------------------------------------------------------------
// build_context
// --------------------------------------------------------------------------------------

/// `agent_service.build_context`：身份/果园/批次/商品/任务/风险事件。
pub(crate) async fn build_context(pool: &PgPool, user: &AuthUser) -> Result<Value, ApiReject> {
    let is_farmer = user.is_farmer();

    let orchards = if is_farmer {
        let rows = sqlx::query(
            r#"SELECT o.id, o.code, o.name, o.status, o.province, o.city, o.county, o.main_variety,
                      (SELECT count(*) FROM fruit_tree_archive t WHERE t.orchard_id = o.id) AS tree_count
                 FROM orchard o
                WHERE o.owner_id = $1
                ORDER BY o.verified_at DESC, o.created_at DESC"#,
        )
        .bind(user.id)
        .fetch_all(pool)
        .await
        .map_err(db_error)?;

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let province: String = row.try_get("province").map_err(db_error)?;
            let city: String = row.try_get("city").map_err(db_error)?;
            let county: String = row.try_get("county").map_err(db_error)?;
            let status: String = row.try_get("status").map_err(db_error)?;
            let tree_count: i64 = row.try_get("tree_count").map_err(db_error)?;
            items.push(obj(vec![
                (
                    "id",
                    Value::String(row.try_get::<Uuid, _>("id").map_err(db_error)?.to_string()),
                ),
                (
                    "code",
                    Value::String(row.try_get("code").map_err(db_error)?),
                ),
                (
                    "name",
                    Value::String(row.try_get("name").map_err(db_error)?),
                ),
                ("status", Value::String(status.clone())),
                (
                    "status_display",
                    Value::String(
                        match status.as_str() {
                            "draft" => "筹备中",
                            "verified" => "已认证",
                            "inactive" => "已停用",
                            other => other,
                        }
                        .to_string(),
                    ),
                ),
                ("origin", Value::String(format!("{province}{city}{county}"))),
                (
                    "main_variety",
                    Value::String(row.try_get("main_variety").map_err(db_error)?),
                ),
                ("tree_count", Value::Number(tree_count.into())),
            ]));
        }
        items
    } else {
        Vec::new()
    };

    let batches = if is_farmer {
        fetch_batches(
            pool,
            "WHERE b.orchard_id IN (SELECT id FROM orchard WHERE owner_id = $1)",
            Some(user.id),
            50,
        )
        .await?
    } else {
        fetch_batches(
            pool,
            "WHERE b.status IN ('open', 'warming', 'harvesting', 'fulfilling')",
            None,
            50,
        )
        .await?
    };

    let products = if is_farmer {
        fetch_products(pool, "WHERE p.seller_id = $1", Some(user.id), 50).await?
    } else {
        fetch_products(pool, "WHERE p.status = 'on_sale'", None, 50).await?
    };

    let tasks = if is_farmer {
        let rows = sqlx::query(
            r#"SELECT id, title, risk_level, task_type, is_completed, created_at
                 FROM task
                WHERE user_id = $1 AND is_completed = FALSE
                ORDER BY created_at DESC
                LIMIT 20"#,
        )
        .bind(user.id)
        .fetch_all(pool)
        .await
        .map_err(db_error)?;

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(obj(vec![
                (
                    "id",
                    Value::String(row.try_get::<Uuid, _>("id").map_err(db_error)?.to_string()),
                ),
                (
                    "title",
                    Value::String(row.try_get("title").map_err(db_error)?),
                ),
                (
                    "risk_level",
                    Value::String(row.try_get("risk_level").map_err(db_error)?),
                ),
                (
                    "task_type",
                    Value::String(row.try_get("task_type").map_err(db_error)?),
                ),
                (
                    "is_completed",
                    Value::Bool(row.try_get("is_completed").map_err(db_error)?),
                ),
                (
                    "created_at",
                    Value::String(ser::dt_offset(
                        row.try_get::<DateTime<Utc>, _>("created_at")
                            .map_err(db_error)?,
                    )),
                ),
            ]));
        }
        items
    } else {
        Vec::new()
    };

    let risk_events = if is_farmer {
        let rows = sqlx::query(
            r#"SELECT disease_name, risk_level, area, recognition_date
                 FROM disease_recognition_record
                WHERE user_id = $1 AND risk_level NOT IN ('正常', '低风险')
                ORDER BY created_at DESC
                LIMIT 10"#,
        )
        .bind(user.id)
        .fetch_all(pool)
        .await
        .map_err(db_error)?;

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(obj(vec![
                (
                    "disease_name",
                    Value::String(row.try_get("disease_name").map_err(db_error)?),
                ),
                (
                    "risk_level",
                    Value::String(row.try_get("risk_level").map_err(db_error)?),
                ),
                (
                    "area",
                    Value::String(row.try_get("area").map_err(db_error)?),
                ),
                (
                    "recognition_date",
                    date_or_empty(row.try_get("recognition_date").map_err(db_error)?),
                ),
            ]));
        }
        items
    } else {
        Vec::new()
    };

    Ok(obj(vec![
        ("role", Value::String(user.role.clone())),
        (
            "user",
            obj(vec![
                ("id", Value::String(user.id.to_string())),
                ("username", Value::String(user.username.clone())),
            ]),
        ),
        ("orchards", Value::Array(orchards)),
        (
            "batches",
            Value::Array(batches.iter().map(BatchRow::ctx).collect()),
        ),
        (
            "products",
            Value::Array(products.iter().map(ProductRow::ctx).collect()),
        ),
        ("tasks", Value::Array(tasks)),
        ("risk_events", Value::Array(risk_events)),
    ]))
}

// --------------------------------------------------------------------------------------
// daily_report
// --------------------------------------------------------------------------------------

/// 规则模板建议（无 LLM 兜底），文案逐字取自蓝本 `rule_suggestions`。
fn rule_suggestions(high: i64, medium: i64, abnormal: i64, open_batches: i64) -> Vec<Value> {
    let mut tips = Vec::new();
    if high > 0 || medium > 0 {
        tips.push(Value::String(format!(
            "有未处理的风险任务（高风险 {high} / 中风险 {medium}），建议优先安排巡园与复检。"
        )));
    }
    if abnormal > 0 {
        tips.push(Value::String(format!(
            "近期存在 {abnormal} 条异常识别记录（黄龙病/溃疡/沙皮等），请及时复核并生成对应农艺处理。"
        )));
    }
    if high == 0 && medium == 0 && abnormal == 0 {
        tips.push(Value::String(
            "当前无高风险任务与异常识别记录，可保持正常巡园节奏。".to_string(),
        ));
    }
    if open_batches == 0 {
        tips.push(Value::String(
            "暂无在售供货批次，可在「售卖」页新建批次并上架商品。".to_string(),
        ));
    }
    tips
}

/// `agent_service.daily_report`：按角色过滤的经营日报。
pub(crate) async fn daily_report(pool: &PgPool, user: &AuthUser) -> Result<Value, ApiReject> {
    let now = Utc::now();

    if !user.is_farmer() {
        return Ok(obj(vec![
            ("role", Value::String(user.role.clone())),
            (
                "message",
                Value::String("经营日报面向果农/经营者，当前角色不可用".to_string()),
            ),
            ("report", Value::Null),
        ]));
    }

    let orchard_count: i64 = sqlx::query_scalar("SELECT count(*) FROM orchard WHERE owner_id = $1")
        .bind(user.id)
        .fetch_one(pool)
        .await
        .map_err(db_error)?;

    let batches = fetch_batches(
        pool,
        "WHERE b.orchard_id IN (SELECT id FROM orchard WHERE owner_id = $1)",
        Some(user.id),
        1_000,
    )
    .await?;
    let batch_ids: Vec<Uuid> = batches.iter().map(|batch| batch.id).collect();
    let batch_count = batches.len() as i64;
    let open_batch_count = batches
        .iter()
        .filter(|batch| batch.status == "open")
        .count() as i64;

    let on_sale_product_count: i64 = sqlx::query_scalar(
        r#"SELECT count(*)
             FROM citrus_product p
             JOIN sales_batch b ON b.id = p.sales_batch_id
             JOIN orchard o ON o.id = b.orchard_id
            WHERE o.owner_id = $1 AND p.status = 'on_sale'"#,
    )
    .bind(user.id)
    .fetch_one(pool)
    .await
    .map_err(db_error)?;

    let since = now - Duration::hours(24);
    let today_order_count: i64 = sqlx::query_scalar(
        r#"SELECT count(*) FROM "order"
            WHERE sales_batch_id = ANY($1::uuid[]) AND created_at >= $2"#,
    )
    .bind(&batch_ids)
    .bind(since)
    .fetch_one(pool)
    .await
    .map_err(db_error)?;

    let today_amount: Decimal = sqlx::query_scalar(
        r#"SELECT COALESCE(sum(total_amount), 0) FROM "order"
            WHERE sales_batch_id = ANY($1::uuid[]) AND created_at >= $2"#,
    )
    .bind(&batch_ids)
    .bind(since)
    .fetch_one(pool)
    .await
    .map_err(db_error)?;

    let high: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task WHERE user_id = $1 AND is_completed = FALSE AND risk_level = '高风险'",
    )
    .bind(user.id)
    .fetch_one(pool)
    .await
    .map_err(db_error)?;
    let medium: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task WHERE user_id = $1 AND is_completed = FALSE AND risk_level = '中风险'",
    )
    .bind(user.id)
    .fetch_one(pool)
    .await
    .map_err(db_error)?;
    let low: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task WHERE user_id = $1 AND is_completed = FALSE AND risk_level = '低风险'",
    )
    .bind(user.id)
    .fetch_one(pool)
    .await
    .map_err(db_error)?;
    let abnormal: i64 = sqlx::query_scalar(
        r#"SELECT count(*) FROM (
               SELECT 1 FROM disease_recognition_record
                WHERE user_id = $1 AND risk_level NOT IN ('正常', '低风险')
                ORDER BY created_at DESC
                LIMIT 10
           ) AS abnormal_records"#,
    )
    .bind(user.id)
    .fetch_one(pool)
    .await
    .map_err(db_error)?;

    let supply_batches = batches
        .iter()
        .take(10)
        .map(|batch| {
            obj(vec![
                ("code", Value::String(batch.code.clone())),
                ("title", Value::String(batch.title.clone())),
                (
                    "status_display",
                    Value::String(batch_status_display(&batch.status).to_string()),
                ),
                (
                    "available_quantity",
                    Value::Number(batch.available_quantity().into()),
                ),
                (
                    "expected_harvest_start",
                    date_or_empty(batch.expected_harvest_start),
                ),
                (
                    "expected_ship_start",
                    date_or_empty(batch.expected_ship_start),
                ),
            ])
        })
        .collect::<Vec<_>>();

    let overview = obj(vec![
        ("orchard_count", Value::Number(orchard_count.into())),
        ("batch_count", Value::Number(batch_count.into())),
        (
            "on_sale_product_count",
            Value::Number(on_sale_product_count.into()),
        ),
        ("open_batch_count", Value::Number(open_batch_count.into())),
        ("today_order_count", Value::Number(today_order_count.into())),
        (
            "today_order_amount",
            Value::String(format!("{:.2}", today_amount.to_f64().unwrap_or_default())),
        ),
    ]);

    let risk_todo = obj(vec![
        ("high_risk_tasks", Value::Number(high.into())),
        ("medium_risk_tasks", Value::Number(medium.into())),
        ("low_risk_tasks", Value::Number(low.into())),
        ("abnormal_records", Value::Number(abnormal.into())),
    ]);

    let report = obj(vec![
        ("generated_at", Value::String(ser::dt_offset(now))),
        ("overview", overview),
        ("supply_batches", Value::Array(supply_batches)),
        ("risk_todo", risk_todo),
        (
            "suggestions",
            Value::Array(rule_suggestions(high, medium, abnormal, open_batch_count)),
        ),
    ]);

    Ok(obj(vec![
        ("role", Value::String("farmer".to_string())),
        ("report", report),
    ]))
}

// --------------------------------------------------------------------------------------
// 选品（意图识别 + 候选排序）
// --------------------------------------------------------------------------------------

/// 规则版意图抽取（`detect_intent`）。键序 = 蓝本写入顺序。
pub(crate) fn detect_intent(query: &str) -> Map<String, Value> {
    let mut intent = Map::new();

    if let Some(value) = scan_budget(query) {
        intent.insert("max_price".to_string(), json!(value));
    }

    if ["送", "礼物", "礼赠", "送礼"]
        .iter()
        .any(|key| query.contains(key))
    {
        intent.insert("purpose".to_string(), Value::String("gift".to_string()));
    }
    if ["自用", "家庭", "一家", "自己吃"]
        .iter()
        .any(|key| query.contains(key))
    {
        intent.insert("purpose".to_string(), Value::String("family".to_string()));
    }
    if ["企业", "公司", "团购", "采购", "定制"]
        .iter()
        .any(|key| query.contains(key))
    {
        intent.insert(
            "sku_type".to_string(),
            Value::String("enterprise".to_string()),
        );
    }
    if ["特色", "尝鲜", "高糖", "特色果"]
        .iter()
        .any(|key| query.contains(key))
    {
        intent.insert(
            "sku_type".to_string(),
            Value::String("specialty".to_string()),
        );
    }

    if let Some(unit) = scan_unit_kw(query) {
        intent.insert("unit_kw".to_string(), Value::String(unit));
    }

    if ["甜", "高甜", "甜一点"]
        .iter()
        .any(|key| query.contains(key))
    {
        intent.insert("sweet".to_string(), Value::Bool(true));
    }

    intent
}

/// `determine_missing`：还差哪些信息才能给出推荐。
fn determine_missing(intent: &Map<String, Value>) -> Vec<Value> {
    let mut missing = Vec::new();
    if !intent.contains_key("max_price") {
        missing.push(Value::String(
            "大概预算（例如「预算80元以内」）".to_string(),
        ));
    }
    if !intent.contains_key("purpose") && !intent.contains_key("sku_type") {
        missing.push(Value::String("用途（自用 / 送礼 / 企业采购）".to_string()));
    }
    if !intent.contains_key("unit_kw") {
        missing.push(Value::String("期望规格（例如「5斤装」）".to_string()));
    }
    missing
}

fn sku_label(key: &str) -> String {
    match key {
        "trial" => "试吃装".to_string(),
        "family" => "家庭装".to_string(),
        "gift" => "礼赠装".to_string(),
        "juice" => "榨汁装".to_string(),
        "enterprise" => "企业装".to_string(),
        "specialty" => "特色果品".to_string(),
        other => other.to_string(),
    }
}

/// 反馈学习：某商品的历史评分均值与样本数。
async fn feedback_rating_stats(
    pool: &PgPool,
    product_name: &str,
) -> Result<Option<(f64, i64)>, ApiReject> {
    let ratings: Vec<i16> =
        sqlx::query_scalar("SELECT rating FROM agent_feedback WHERE product_name = $1")
            .bind(product_name)
            .fetch_all(pool)
            .await
            .map_err(db_error)?;

    if ratings.is_empty() {
        return Ok(None);
    }
    let count = ratings.len() as f64;
    let sum: f64 = ratings.iter().map(|value| f64::from(*value)).sum();
    Ok(Some((round2(sum / count), ratings.len() as i64)))
}

/// 反馈学习回灌：历史好评加分、差评减分。
fn feedback_bonus(stats: Option<(f64, i64)>) -> (i64, &'static str) {
    let Some((avg, count)) = stats else {
        return (0, "");
    };
    if count >= 2 && avg >= 4.5 {
        return (3, "用户口碑很好");
    }
    if avg >= 4.0 {
        return (2, "用户口碑好");
    }
    if avg <= 2.0 {
        return (-3, "用户评价较低");
    }
    (0, "")
}

/// 反馈学习回灌：好评缩短复购周期，差评延长。
fn feedback_adjusted_cycle(stats: Option<(f64, i64)>, base: i64) -> i64 {
    let Some((avg, count)) = stats else {
        return base;
    };
    if avg >= 4.5 && count >= 2 {
        return (base - 10).max(15);
    }
    if avg <= 2.5 {
        return base + 15;
    }
    base
}

/// `select_for_shopper`：在真实可售商品内推荐并说明理由。
pub(crate) async fn select_for_shopper(pool: &PgPool, query: &str) -> Result<Value, ApiReject> {
    let intent = detect_intent(query);
    let candidates = fetch_products(
        pool,
        "WHERE p.status = 'on_sale' AND p.stock > 0",
        None,
        1_000,
    )
    .await?;

    let purpose = intent.get("purpose").and_then(Value::as_str);
    let sku_type = intent.get("sku_type").and_then(Value::as_str);
    let unit_kw = intent.get("unit_kw").and_then(Value::as_str);
    let max_price = intent.get("max_price").and_then(Value::as_f64);
    let sweet = intent
        .get("sweet")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut scored: Vec<(i64, Vec<Value>, Value)> = Vec::new();
    for product in &candidates {
        // 蓝本先在 `select_for_shopper` 里过滤掉未开售批次的商品（此时还没算分）。
        if !product.batch_open {
            continue;
        }

        let mut score = 0i64;
        let mut reasons: Vec<Value> = Vec::new();

        if purpose == Some("gift") && matches!(product.sku_type.as_str(), "gift" | "specialty") {
            score += 3;
            reasons.push(Value::String("适合送礼".to_string()));
        }
        if purpose == Some("family") && product.sku_type == "family" {
            score += 3;
            reasons.push(Value::String("适合家庭自用".to_string()));
        }
        if sku_type == Some(product.sku_type.as_str()) {
            score += 4;
            reasons.push(Value::String(sku_label(&product.sku_type)));
        }
        if let Some(unit_kw) = unit_kw
            && product.unit.contains(unit_kw)
        {
            score += 2;
            reasons.push(Value::String(format!("{}包装", product.unit)));
        }
        if let Some(max_price) = max_price
            && product.price.to_f64().unwrap_or_default() <= max_price
        {
            score += 3;
            reasons.push(Value::String("价格在预算内".to_string()));
        }
        if product.minimum_order_quantity <= 1 {
            score += 1;
        }
        if sweet
            && let Some(sweetness) = product.sweetness
            && sweetness.to_f64().unwrap_or_default() >= 12.0
        {
            score += 2;
            reasons.push(Value::String("糖度较高".to_string()));
        }
        if product.stock <= 0 {
            continue;
        }

        let (bonus, reason) = feedback_bonus(feedback_rating_stats(pool, &product.name).await?);
        if bonus != 0 {
            score += bonus;
            reasons.push(Value::String(reason.to_string()));
        }

        scored.push((score, reasons, product.ctx()));
    }

    // Python 的 `sort(key=...)` 稳定：同分保持查询顺序。
    scored.sort_by_key(|entry| -entry.0);

    let recommendations = scored
        .into_iter()
        .take(5)
        .map(|(score, reasons, product)| {
            obj(vec![
                ("product", product),
                ("reasons", Value::Array(reasons)),
                ("score", Value::Number(score.into())),
            ])
        })
        .collect::<Vec<_>>();

    let needs_more = determine_missing(&intent);
    let intent = Value::Object(intent);

    Ok(obj(vec![
        ("intent", intent),
        ("needs_more", Value::Array(needs_more)),
        ("recommendations", Value::Array(recommendations)),
    ]))
}

// --------------------------------------------------------------------------------------
// 询价（parse_inquiry / 风险评估 / 报价成本）
// --------------------------------------------------------------------------------------

/// `parse_inquiry`：数量/规格/最晚到货日。
pub(crate) fn parse_inquiry(query: &str) -> Map<String, Value> {
    let mut requirement = Map::new();

    if let Some(quantity) = scan_quantity(query) {
        requirement.insert("quantity".to_string(), Value::Number(quantity.into()));
    }

    if let Some(spec) = scan_unit_kw(query) {
        requirement.insert("spec".to_string(), Value::String(spec));
    }

    if let Some(deadline) = scan_iso_date(query) {
        requirement.insert("deadline".to_string(), Value::String(deadline));
    } else if let Some(deadline) = scan_month_day(query) {
        requirement.insert("deadline".to_string(), Value::String(deadline));
    }

    requirement
}

/// `assess_batch_risk`：抽检 + 开售状态。
fn assess_batch_risk(batch: &BatchRow, stats: SampleStats) -> Vec<Value> {
    let mut risks = Vec::new();
    if stats.total > 0 {
        if stats.rejected > 0 {
            risks.push(Value::String("存在不合格抽检记录".to_string()));
        } else if stats.watch > 0 {
            risks.push(Value::String("存在持续观察的抽检记录".to_string()));
        }
    } else {
        risks.push(Value::String(
            "品质待确认（本批次尚无抽检记录）".to_string(),
        ));
    }
    if batch.status != "open" {
        risks.push(Value::String("批次未开售".to_string()));
    }
    risks
}

/// `can_meet_deadline`：预计发货 + 3 天物流 ≤ 最晚到货日。
fn can_meet_deadline(batch: &BatchRow, deadline: Option<&str>) -> bool {
    let Some(deadline) = deadline else {
        return true;
    };
    let Ok(target) = NaiveDate::parse_from_str(deadline, "%Y-%m-%d") else {
        return true;
    };
    let Some(ship) = batch.expected_ship_start.or(batch.expected_harvest_start) else {
        return true;
    };
    ship.num_days_from_ce() + 3 <= target.num_days_from_ce()
}

/// `cost_for_batch`：单位箱成本 + 支付手续费。
fn cost_for_batch(_batch: &BatchRow, costs: &CostRow, quantity: i64) -> Result<Value, ApiReject> {
    let q = quantity.max(1);
    let per_box = costs.per_box();
    let fee_rate = costs.payment_fee_rate.to_f64().unwrap_or_default() / 100.0;
    let subtotal = per_box * q as f64;
    let fee = subtotal * fee_rate;

    Ok(obj(vec![
        ("per_box_cost", Value::String(format!("{per_box:.2}"))),
        ("quantity", Value::Number(q.into())),
        ("goods_cost", Value::String(format!("{subtotal:.2}"))),
        ("payment_fee", Value::String(format!("{fee:.2}"))),
        (
            "total_cost_estimate",
            Value::String(format!("{:.2}", subtotal + fee)),
        ),
    ]))
}

/// `sales_batch` 的成本列（报价成本项的唯一来源）。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CostRow {
    pub(crate) procurement_cost_per_box: Decimal,
    pub(crate) sorting_cost_per_box: Decimal,
    pub(crate) packaging_cost_per_box: Decimal,
    pub(crate) shipping_cost_per_box: Decimal,
    pub(crate) aftersale_reserve_per_box: Decimal,
    pub(crate) promotion_cost_per_box: Decimal,
    pub(crate) payment_fee_rate: Decimal,
}

impl CostRow {
    fn per_box(&self) -> f64 {
        [
            self.procurement_cost_per_box,
            self.sorting_cost_per_box,
            self.packaging_cost_per_box,
            self.shipping_cost_per_box,
            self.aftersale_reserve_per_box,
            self.promotion_cost_per_box,
        ]
        .iter()
        .map(|value| value.to_f64().unwrap_or_default())
        .sum()
    }
}

async fn fetch_costs(pool: &PgPool) -> Result<HashMap<Uuid, CostRow>, ApiReject> {
    let rows = sqlx::query(
        r#"SELECT id, procurement_cost_per_box, sorting_cost_per_box, packaging_cost_per_box,
                  shipping_cost_per_box, aftersale_reserve_per_box, promotion_cost_per_box,
                  payment_fee_rate
             FROM sales_batch"#,
    )
    .fetch_all(pool)
    .await
    .map_err(db_error)?;

    let mut costs = HashMap::new();
    for row in rows {
        let id: Uuid = row.try_get("id").map_err(db_error)?;
        costs.insert(
            id,
            CostRow {
                procurement_cost_per_box: row
                    .try_get("procurement_cost_per_box")
                    .map_err(db_error)?,
                sorting_cost_per_box: row.try_get("sorting_cost_per_box").map_err(db_error)?,
                packaging_cost_per_box: row.try_get("packaging_cost_per_box").map_err(db_error)?,
                shipping_cost_per_box: row.try_get("shipping_cost_per_box").map_err(db_error)?,
                aftersale_reserve_per_box: row
                    .try_get("aftersale_reserve_per_box")
                    .map_err(db_error)?,
                promotion_cost_per_box: row.try_get("promotion_cost_per_box").map_err(db_error)?,
                payment_fee_rate: row.try_get("payment_fee_rate").map_err(db_error)?,
            },
        );
    }
    Ok(costs)
}

/// `fulfillment_risk_score`：加权评分 → 等级 + 概率（规则可解释的预测）。
async fn fulfillment_risk_score(
    pool: &PgPool,
    batch: &BatchRow,
    stats: SampleStats,
    required_quantity: Option<i64>,
) -> Result<Value, ApiReject> {
    let mut factors: Vec<Value> = Vec::new();
    let mut score: i64 = 0;

    if stats.total > 0 {
        if stats.rejected > 0 {
            score += 25;
            factors.push(Value::String("存在不合格抽检记录".to_string()));
        } else if stats.watch > 0 {
            score += 15;
            factors.push(Value::String("存在持续观察抽检记录".to_string()));
        }
    } else {
        score += 10;
        factors.push(Value::String(
            "品质待确认（本批次尚无抽检记录）".to_string(),
        ));
    }

    // 蓝本 `timezone.localdate()` 在 TIME_ZONE='UTC' 下就是 UTC 日期。
    let today = Utc::now().date_naive();
    if let Some(ship_start) = batch.expected_ship_start {
        let ship_days = (ship_start - today).num_days();
        if ship_days < 0 {
            score += 30;
            factors.push(Value::String("已过预计发货日".to_string()));
        } else if ship_days <= 3 {
            score += 15;
            factors.push(Value::String("发货窗口临近".to_string()));
        }
    } else if let Some(harvest_start) = batch.expected_harvest_start {
        if (harvest_start - today).num_days() < 0 {
            score += 20;
            factors.push(Value::String("已过预计采摘日".to_string()));
        }
    } else {
        score += 10;
        factors.push(Value::String("无预计发货时间".to_string()));
    }

    let qty = required_quantity.unwrap_or(1).max(1);
    if f64::from(batch.available_quantity()) < qty as f64 * 1.2 {
        score += 20;
        factors.push(Value::String("安全余量不足".to_string()));
    }

    if orchard_has_disease_record(pool, batch).await? {
        score += 20;
        factors.push(Value::String("果园存在高风险病害记录".to_string()));
    }

    let score = score.clamp(0, 100);
    let level = if score <= 30 {
        "低"
    } else if score <= 60 {
        "中"
    } else {
        "高"
    };

    Ok(obj(vec![
        ("batch_code", Value::String(batch.code.clone())),
        ("score", Value::Number(score.into())),
        ("level", Value::String(level.to_string())),
        ("probability", json!(round2(score as f64 / 100.0))),
        ("factors", Value::Array(factors)),
        (
            "prediction",
            Value::String(format!("预计履约风险「{level}」({score} 分)")),
        ),
    ]))
}

/// 批次所属果园的 owner 是否有高风险/中风险病害记录。
async fn orchard_has_disease_record(pool: &PgPool, batch: &BatchRow) -> Result<bool, ApiReject> {
    let has: bool = sqlx::query_scalar(
        r#"SELECT EXISTS (
               SELECT 1
                 FROM disease_recognition_record r
                WHERE r.user_id = (SELECT o.owner_id FROM sales_batch b
                                     JOIN orchard o ON o.id = b.orchard_id
                                    WHERE b.id = $1)
                  AND r.risk_level NOT IN ('正常', '低风险')
           )"#,
    )
    .bind(batch.id)
    .fetch_one(pool)
    .await
    .map_err(db_error)?;
    Ok(has)
}

/// `inquiry_quote`：候选过滤 + 评分 + 主/备选方案 + 风险 + 报价成本项。
pub(crate) async fn inquiry_quote(pool: &PgPool, query: &str) -> Result<Value, ApiReject> {
    let requirement = parse_inquiry(query);
    let quantity = requirement
        .get("quantity")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let deadline = requirement
        .get("deadline")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let batches = fetch_batches(pool, "WHERE b.status = 'open'", None, 1_000).await?;
    let batch_ids: Vec<Uuid> = batches.iter().map(|batch| batch.id).collect();
    let stats = sample_stats(pool, &batch_ids).await?;
    let costs = fetch_costs(pool).await?;

    let mut candidates: Vec<(i64, Value)> = Vec::new();
    let mut hard_failed: Vec<Value> = Vec::new();

    for batch in &batches {
        if i64::from(batch.available_quantity()) < quantity {
            hard_failed.push(Value::String(format!(
                "{} 可供量 {} 箱不足",
                batch.code,
                batch.available_quantity()
            )));
            continue;
        }
        if !can_meet_deadline(batch, deadline.as_deref()) {
            hard_failed.push(Value::String(format!(
                "{} 无法在最晚到货日前送达",
                batch.code
            )));
            continue;
        }

        let sample = stats.get(&batch.id).copied().unwrap_or_default();
        let risks = assess_batch_risk(batch, sample);
        let mut score = 0i64;
        if i64::from(batch.available_quantity()) >= quantity * 2 {
            score += 3;
        }
        if risks.is_empty() {
            score += 3;
        }
        if sample.qualified >= 3 {
            score += 2;
        }

        let cost = costs.get(&batch.id).copied().unwrap_or_default();

        let candidate = obj(vec![
            ("batch", batch.ctx()),
            ("score", Value::Number(score.into())),
            ("risks", Value::Array(risks)),
            (
                "fulfillment_risk",
                fulfillment_risk_score(pool, batch, sample, Some(quantity)).await?,
            ),
            ("cost", cost_for_batch(batch, &cost, quantity)?),
        ]);
        candidates.push((score, candidate));
    }

    candidates.sort_by_key(|entry| -entry.0);

    let primary = candidates
        .first()
        .map(|entry| entry.1.clone())
        .unwrap_or(Value::Null);
    let alternatives = candidates
        .iter()
        .skip(1)
        .take(2)
        .map(|entry| entry.1.clone())
        .collect::<Vec<_>>();

    Ok(obj(vec![
        ("requirements", Value::Object(requirement)),
        ("hard_constraints_failed", Value::Array(hard_failed)),
        ("primary", primary),
        ("alternatives", Value::Array(alternatives)),
        (
            "needs_confirmation",
            Value::Array(vec![
                Value::String(
                    "报价含包装/分选/物流/售后计提，需运营确认后生成正式报价单".to_string(),
                ),
                Value::String("正式报价、合同与锁货走业务流程，智能体不直接执行".to_string()),
            ]),
        ),
    ]))
}

// --------------------------------------------------------------------------------------
// 风险履约
// --------------------------------------------------------------------------------------

fn suggest_actions(source_type: &str) -> Vec<Value> {
    let actions: &[&str] = match source_type {
        "识别异常" => &[
            "补充巡园拍照复核",
            "安排品质抽检",
            "暂缓新增销售",
            "必要时联系商家协商",
        ],
        "风险任务" => &["补充巡检并按任务处置", "复核结果后决定是否暂缓发货"],
        "环境异常" => &[
            "补充巡园与水肥管理",
            "启用备选批次预案",
            "评估是否影响已分配批次",
        ],
        _ => &["补充巡检复核"],
    };
    actions
        .iter()
        .map(|item| Value::String(item.to_string()))
        .collect()
}

fn is_high_risk_env(temperature: f64, humidity: f64) -> bool {
    (22.0..=32.0).contains(&temperature) && (80.0..=100.0).contains(&humidity)
}

/// `_estimated_deadline`：发货日 > 采摘日 > 停售日，取最早。
fn estimated_deadline(batches: &[BatchRow]) -> String {
    let mut dates: Vec<NaiveDate> = Vec::new();
    for batch in batches {
        if let Some(value) = batch.expected_ship_start {
            dates.push(value);
        } else if let Some(value) = batch.expected_harvest_start {
            dates.push(value);
        } else if let Some(value) = batch.close_at {
            dates.push(value.date_naive());
        }
    }
    dates
        .into_iter()
        .min()
        .map(ser::dt_date)
        .unwrap_or_default()
}

/// `risk_alert`：异常识别/风险任务/环境异常 → 风险卡。
pub(crate) async fn risk_alert(pool: &PgPool, user: &AuthUser) -> Result<Value, ApiReject> {
    let mut sources: Vec<Value> = Vec::new();

    let abnormal = sqlx::query(
        r#"SELECT disease_name, risk_level, area, recognition_date
             FROM disease_recognition_record
            WHERE user_id = $1 AND risk_level NOT IN ('正常', '低风险')
            ORDER BY created_at DESC
            LIMIT 5"#,
    )
    .bind(user.id)
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    for row in abnormal {
        let disease_name: String = row.try_get("disease_name").map_err(db_error)?;
        let risk_level: String = row.try_get("risk_level").map_err(db_error)?;
        let area: String = row.try_get("area").map_err(db_error)?;
        let recognition_date: Option<NaiveDate> =
            row.try_get("recognition_date").map_err(db_error)?;
        sources.push(obj(vec![
            ("type", Value::String("识别异常".to_string())),
            (
                "evidence",
                Value::String(format!("{disease_name}（{risk_level}）@{area}")),
            ),
            (
                "occurred",
                Value::String(recognition_date.map(ser::dt_date).unwrap_or_default()),
            ),
        ]));
    }

    let tasks = sqlx::query(
        r#"SELECT title, risk_level, created_at
             FROM task
            WHERE user_id = $1 AND is_completed = FALSE AND risk_level IN ('高风险', '中风险')
            ORDER BY created_at DESC
            LIMIT 5"#,
    )
    .bind(user.id)
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    for row in tasks {
        let title: String = row.try_get("title").map_err(db_error)?;
        let risk_level: String = row.try_get("risk_level").map_err(db_error)?;
        let created_at: Option<DateTime<Utc>> = row.try_get("created_at").map_err(db_error)?;
        sources.push(obj(vec![
            ("type", Value::String("风险任务".to_string())),
            ("evidence", Value::String(title)),
            ("risk_level", Value::String(risk_level)),
            (
                "occurred",
                Value::String(
                    created_at
                        .map(|value| ser::dt_date(value.date_naive()))
                        .unwrap_or_default(),
                ),
            ),
        ]));
    }

    let latest_th = sqlx::query("SELECT timestamp, temperature, humidity FROM temperature_humidity_data WHERE user_id = $1 ORDER BY timestamp DESC LIMIT 1")
        .bind(user.id)
        .fetch_optional(pool)
        .await
        .map_err(db_error)?;
    if let Some(row) = latest_th {
        let timestamp: DateTime<Utc> = row.try_get("timestamp").map_err(db_error)?;
        let temperature: f64 = row.try_get("temperature").map_err(db_error)?;
        let humidity: f64 = row.try_get("humidity").map_err(db_error)?;
        if is_high_risk_env(temperature, humidity) {
            sources.push(obj(vec![
                ("type", Value::String("环境异常".to_string())),
                (
                    "evidence",
                    Value::String(format!(
                        "温度 {}℃ / 湿度 {}%",
                        py_float(temperature),
                        py_float(humidity)
                    )),
                ),
                ("occurred", Value::String(py_datetime(timestamp))),
            ]));
        }
    }

    let batches = fetch_batches(
        pool,
        "WHERE b.orchard_id IN (SELECT id FROM orchard WHERE owner_id = $1) \
         AND b.status NOT IN ('cancelled', 'completed')",
        Some(user.id),
        1_000,
    )
    .await?;
    let batch_ids: Vec<Uuid> = batches.iter().map(|batch| batch.id).collect();
    let batch_ctx = batches.iter().map(BatchRow::ctx).collect::<Vec<Value>>();

    let orders = sqlx::query(
        r#"SELECT o.order_number, o.status, b.code AS sales_batch_code
             FROM "order" o
             LEFT JOIN sales_batch b ON b.id = o.sales_batch_id
            WHERE o.sales_batch_id = ANY($1::uuid[])
              AND o.status IN ('pending_payment', 'pending_deposit', 'paid', 'picking', 'packed')
            ORDER BY o.created_at DESC
            LIMIT 10"#,
    )
    .bind(&batch_ids)
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    let order_ctx = orders
        .iter()
        .map(|row| {
            let status: String = row.try_get("status").map_err(db_error)?;
            let sales_batch_code: Option<String> =
                row.try_get("sales_batch_code").map_err(db_error)?;
            Ok(obj(vec![
                (
                    "order_number",
                    Value::String(row.try_get("order_number").map_err(db_error)?),
                ),
                (
                    "status_display",
                    Value::String(order_status_display(&status).to_string()),
                ),
                (
                    "sales_batch_code",
                    Value::String(sales_batch_code.unwrap_or_default()),
                ),
            ]))
        })
        .collect::<Result<Vec<Value>, ApiReject>>()?;

    let safety_margin: i64 = batches
        .iter()
        .map(|batch| i64::from(batch.available_quantity()))
        .sum();
    let deadline = estimated_deadline(&batches);

    let stats = sample_stats(pool, &batch_ids).await?;
    let mut batch_risk_scores = Vec::with_capacity(batches.len());
    for batch in &batches {
        let sample = stats.get(&batch.id).copied().unwrap_or_default();
        batch_risk_scores.push(fulfillment_risk_score(pool, batch, sample, None).await?);
    }

    if sources.is_empty() {
        return Ok(obj(vec![
            ("has_alert", Value::Bool(false)),
            ("risk_cards", Value::Array(Vec::new())),
            ("affected_batches", Value::Array(batch_ctx)),
            ("safety_margin", Value::Number(safety_margin.into())),
            ("batch_risk_scores", Value::Array(batch_risk_scores)),
        ]));
    }

    let mut cards = Vec::with_capacity(sources.len());
    for source in sources {
        let source_type = source
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        cards.push(obj(vec![
            ("source", source),
            ("affected_batches", Value::Array(batch_ctx.clone())),
            ("affected_orders", Value::Array(order_ctx.clone())),
            ("safety_margin", Value::Number(safety_margin.into())),
            ("batch_risk_scores", Value::Array(batch_risk_scores.clone())),
            (
                "suggested_actions",
                Value::Array(suggest_actions(&source_type)),
            ),
            (
                "responsible",
                Value::String("果园负责人 / 果农（需人工确认）".to_string()),
            ),
            ("latest_deadline", Value::String(deadline.clone())),
            (
                "note",
                Value::String(
                    "环境异常不直接扣减可供量；可供量变化需果园/供应链确认后更新。".to_string(),
                ),
            ),
        ]));
    }

    Ok(obj(vec![
        ("has_alert", Value::Bool(true)),
        ("risk_cards", Value::Array(cards)),
        ("affected_batches", Value::Array(batch_ctx)),
        ("safety_margin", Value::Number(safety_margin.into())),
        ("batch_risk_scores", Value::Array(batch_risk_scores)),
    ]))
}

// --------------------------------------------------------------------------------------
// 复购提醒
// --------------------------------------------------------------------------------------

/// `repurchase_suggestion`：基于历史订单 + 购买周期 + 当前库存。
pub(crate) async fn repurchase_suggestion(
    pool: &PgPool,
    user: &AuthUser,
) -> Result<Value, ApiReject> {
    if !user.is_buyer() {
        return Ok(obj(vec![
            ("role", Value::String("farmer".to_string())),
            ("should_remind", Value::Bool(false)),
            ("reason", Value::String("复购提醒仅面向购买者".to_string())),
        ]));
    }

    let orders = sqlx::query(
        r#"SELECT id, created_at FROM "order"
            WHERE buyer_id = $1 AND status = 'completed'
            ORDER BY created_at DESC"#,
    )
    .bind(user.id)
    .fetch_all(pool)
    .await
    .map_err(db_error)?;

    if orders.is_empty() {
        return Ok(obj(vec![
            ("role", Value::String("buyer".to_string())),
            ("should_remind", Value::Bool(false)),
            (
                "reason",
                Value::String("暂无历史购买记录，暂不复购提醒".to_string()),
            ),
        ]));
    }

    // 每个商品名的最近一次购买时间；插入序 = 订单倒序，与蓝本一致。
    let mut latest: Vec<(String, Option<DateTime<Utc>>)> = Vec::new();
    for order in &orders {
        let order_id: Uuid = order.try_get("id").map_err(db_error)?;
        let created_at: Option<DateTime<Utc>> = order.try_get("created_at").map_err(db_error)?;
        let items = sqlx::query("SELECT product_name FROM order_item WHERE order_id = $1")
            .bind(order_id)
            .fetch_all(pool)
            .await
            .map_err(db_error)?;
        for item in items {
            let name: String = item.try_get("product_name").map_err(db_error)?;
            if name.is_empty() {
                continue;
            }
            match latest.iter_mut().find(|entry| entry.0 == name) {
                Some(entry) => {
                    if let (Some(current), Some(newest)) = (entry.1, created_at)
                        && current < newest
                    {
                        entry.1 = Some(newest);
                    }
                }
                None => latest.push((name, created_at)),
            }
        }
    }

    let now = Utc::now();
    let mut suggestions: Vec<(i64, Value)> = Vec::new();
    for (name, last) in &latest {
        let on_sale: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM citrus_product WHERE status = 'on_sale' AND name = $1)",
        )
        .bind(name)
        .fetch_one(pool)
        .await
        .map_err(db_error)?;
        if !on_sale {
            continue;
        }

        let days = last.map(|value| (now - value).num_days()).unwrap_or(0);
        let cycle = feedback_adjusted_cycle(feedback_rating_stats(pool, name).await?, 30);
        if days >= cycle {
            suggestions.push((
                days,
                obj(vec![
                    ("product_name", Value::String(name.clone())),
                    (
                        "last_bought_at",
                        Value::String(
                            last.map(|value| ser::dt_date(value.date_naive()))
                                .unwrap_or_default(),
                        ),
                    ),
                    ("days_since", Value::Number(days.into())),
                    ("suggest", Value::Bool(true)),
                    (
                        "reason",
                        Value::String(format!("距上次购买 {days} 天，当前仍在售，可考虑复购")),
                    ),
                ]),
            ));
        }
    }

    suggestions.sort_by_key(|entry| -entry.0);
    let items = suggestions
        .into_iter()
        .take(5)
        .map(|entry| entry.1)
        .collect::<Vec<_>>();

    Ok(obj(vec![
        ("role", Value::String("buyer".to_string())),
        ("should_remind", Value::Bool(!items.is_empty())),
        ("suggestions", Value::Array(items)),
        (
            "avoid_note",
            Value::String(
                "复购提醒基于你的授权历史与购买周期；未到周期或商品已下架时不打扰。".to_string(),
            ),
        ),
    ]))
}

// --------------------------------------------------------------------------------------
// 统一入口（意图识别 → 工具路由 → 规则摘要）
// --------------------------------------------------------------------------------------

/// 规则版意图识别（`detect_agent_intent`），用于统一智能体路由。
pub(crate) fn detect_agent_intent(query: &str) -> &'static str {
    let hits = |keys: &[&str]| keys.iter().any(|key| query.contains(key));

    if hits(&["复购", "再买", "回购", "历史购买", "之前买的"]) {
        return "repurchase";
    }
    if hits(&["风险", "异常", "预警", "告警", "影响", "履约"]) {
        return "risk";
    }
    if hits(&["采购", "询价", "报价", "批发", "供货"]) {
        return "inquiry";
    }
    if hits(&["买", "推荐", "选购", "送人", "送礼", "预算", "适合"]) {
        return "select";
    }
    if hits(&["日报", "经营", "订单", "销量", "利润", "概况", "农事"]) {
        return "report";
    }
    if hits(&["果园", "档案", "状态", "果树", "批次"]) {
        return "context";
    }
    "unknown"
}

/// 规则版摘要回复（`_summary_for`），文案逐字对齐。
pub(crate) fn summary_for(intent: &str, data: &Value, _query: &str) -> String {
    match intent {
        "select" => {
            let empty = Vec::new();
            let recs = data
                .get("recommendations")
                .and_then(Value::as_array)
                .unwrap_or(&empty);
            if recs.is_empty() {
                let needs_more = data
                    .get("needs_more")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if needs_more.is_empty() {
                    return "暂未找到匹配的在售商品。".to_string();
                }
                let joined = join_text(&needs_more, "、");
                return format!("暂未找到匹配的在售商品。请补充：{joined}");
            }
            let names = recs
                .iter()
                .take(3)
                .filter_map(|item| item.pointer("/product/name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("、");
            format!(
                "为你推荐：{names}（共 {} 款），我已按需求在真实在售商品里筛选并给出理由。",
                recs.len()
            )
        }
        "inquiry" => {
            let primary = data.get("primary").cloned().unwrap_or(Value::Null);
            if primary.is_null() {
                let failed = data
                    .get("hard_constraints_failed")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if failed.is_empty() {
                    return "没有满足硬约束的候选批次。".to_string();
                }
                let joined = join_text(&failed.iter().take(3).cloned().collect::<Vec<_>>(), "；");
                return format!("没有满足硬约束的候选批次。排除原因：{joined}");
            }
            let title = primary
                .pointer("/batch/title")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let total = primary
                .pointer("/cost/total_cost_estimate")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let alternatives = data
                .get("alternatives")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!(
                "主方案：「{title}」预估总成本 ¥{total}，另有 {alternatives} 个备选。正式报价需运营确认后生成。"
            )
        }
        "risk" => {
            let empty = Vec::new();
            let cards = data
                .get("risk_cards")
                .and_then(Value::as_array)
                .unwrap_or(&empty);
            if cards.is_empty() {
                return "当前无环境异常、异常识别或风险任务，生产与履约正常。".to_string();
            }
            let types = cards
                .iter()
                .take(3)
                .filter_map(|card| card.pointer("/source/type").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("、");
            let batches = data
                .get("affected_batches")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let margin = data
                .get("safety_margin")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            format!(
                "检测到 {} 项风险：{types}。受影响批次 {batches} 个，安全余量 {margin} 箱。",
                cards.len()
            )
        }
        "report" => {
            let Some(report) = data.get("report").filter(|value| !value.is_null()) else {
                return data
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("当前角色暂不支持经营日报。")
                    .to_string();
            };
            let open_batch_count = report
                .pointer("/overview/open_batch_count")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let today_order_count = report
                .pointer("/overview/today_order_count")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let today_order_amount = report
                .pointer("/overview/today_order_amount")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let suggestions = report
                .get("suggestions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let joined = join_text(
                &suggestions.iter().take(2).cloned().collect::<Vec<_>>(),
                "；",
            );
            format!(
                "今日经营：在售批次 {open_batch_count}、今日订单 {today_order_count} 单、金额 ¥{today_order_amount}。{joined}"
            )
        }
        "repurchase" => {
            let empty = Vec::new();
            let suggestions = data
                .get("suggestions")
                .and_then(Value::as_array)
                .unwrap_or(&empty);
            if suggestions.is_empty() {
                let reason = data
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("未到周期或商品已下架，避免打扰。");
                return format!("暂无到期复购建议。{reason}");
            }
            let names = suggestions
                .iter()
                .take(3)
                .filter_map(|item| item.get("product_name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("、");
            format!("有 {} 项商品到复购周期：{names}。", suggestions.len())
        }
        "context" => format!(
            "已装载上下文：果园 {} 个、批次 {} 个、在售商品 {} 个。",
            array_len(data, "orchards"),
            array_len(data, "batches"),
            array_len(data, "products")
        ),
        _ => "我能帮你查询经营日报、智能选品、采购询价、生产风险、复购提醒。试试说「今天经营怎么样」「推荐5斤装送人」「采购100箱」「有什么生产风险」「有复购提醒吗」。".to_string(),
    }
}

fn array_len(data: &Value, key: &str) -> usize {
    data.get(key).and_then(Value::as_array).map_or(0, Vec::len)
}

fn join_text(values: &[Value], sep: &str) -> String {
    values
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(sep)
}

/// LLM 凭证：**只认环境变量**（`DEVIATIONS.md` S1：蓝本硬编码密钥不得抄进代码）。
///
/// 未设置或为空 → 纯规则层，与契约基准的录制环境一致。
pub(crate) fn llm_api_key() -> Option<String> {
    std::env::var("AGENT_LLM_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 组装 LLM 客户端：**只有**显式配置了 `AGENT_LLM_API_KEY` 才启用，
/// 否则返回 `None`（调用方直接走规则兜底）。
///
/// 凭证走 `state.client`（来源是 `config.toml` 的 `[ai].openrouter_api_key`），
/// 蓝本里那份硬编码密钥不抄（`DEVIATIONS.md` S1）。
pub(crate) fn llm_client(configured: OpenRouterClient) -> Option<OpenRouterClient> {
    llm_api_key().map(|_| configured)
}

async fn llm_chat(
    client: Option<&OpenRouterClient>,
    prompt: &str,
    temperature: f64,
    max_tokens: u32,
) -> String {
    let Some(client) = client else {
        return String::new();
    };
    let request = SimpleChatRequest::new(prompt).with_options(
        ChatOptions::new()
            .temperature(temperature)
            .max_tokens(max_tokens),
    );
    match client.chat(request).await {
        Ok(response) => response.content.trim().to_string(),
        Err(err) => {
            tracing::warn!("智能体 LLM 调用失败，回落规则层: {err}");
            String::new()
        }
    }
}

/// `llm_detect_intent`：失败返回空串，由规则版兜底。
async fn llm_detect_intent(client: Option<&OpenRouterClient>, query: &str) -> String {
    let prompt = format!(
        "你是果园智能助手的意图分类器。只能从这些意图中选一个：{}。用户说：\"{query}\"。只回答意图名，不要其他内容。",
        LLM_INTENTS.join(", ")
    );
    let content = llm_chat(client, &prompt, 0.0, 12).await;
    let cleaned = content
        .trim_matches(|ch| ch == '"' || ch == '\'')
        .trim()
        .to_lowercase();
    if LLM_INTENTS.contains(&cleaned.as_str()) {
        cleaned
    } else {
        String::new()
    }
}

/// `llm_summary`：基于真实工具数据生成解释；失败返回空串。
async fn llm_summary(
    client: Option<&OpenRouterClient>,
    _intent: &str,
    data: &Value,
    query: &str,
) -> String {
    let payload = serde_json::to_string(data).unwrap_or_default();
    let payload = payload.chars().take(2500).collect::<String>();
    let prompt = format!(
        "你是橙管家果园智能助手。请仅根据下列工具返回的真实数据，用中文简洁回应，不要编造。用户问题：{query}。工具数据：{payload}。只输出回复正文。"
    );
    llm_chat(client, &prompt, 0.4, 600).await
}

/// `agent_chat`：统一入口。LLM 到位时做意图识别与摘要，失败一律回落规则层。
pub(crate) async fn agent_chat(
    pool: &PgPool,
    client: Option<&OpenRouterClient>,
    query: &str,
    user: &AuthUser,
) -> Result<Value, ApiReject> {
    let intent = {
        let llm = llm_detect_intent(client, query).await;
        if llm.is_empty() {
            detect_agent_intent(query).to_string()
        } else {
            llm
        }
    };

    let data = match intent.as_str() {
        "select" => select_for_shopper(pool, query).await?,
        "inquiry" => inquiry_quote(pool, query).await?,
        "risk" => risk_alert(pool, user).await?,
        "report" => daily_report(pool, user).await?,
        "repurchase" => repurchase_suggestion(pool, user).await?,
        "context" => build_context(pool, user).await?,
        _ => Value::Object(Map::new()),
    };

    let reply = llm_summary(client, &intent, &data, query).await;
    let (reply, llm_ok) = if reply.is_empty() {
        (summary_for(&intent, &data, query), false)
    } else {
        (reply, true)
    };

    Ok(obj(vec![
        ("intent", Value::String(intent)),
        ("data", data),
        ("reply", Value::String(reply)),
        ("llm", Value::Bool(llm_ok)),
        ("escalated", Value::Bool(false)),
    ]))
}

// --------------------------------------------------------------------------------------
// 审批单 / 反馈的序列化
// --------------------------------------------------------------------------------------

/// `AgentApproval` 的一行（含 `created_by` / `decided_by` 的 username）。
#[derive(Debug, Clone)]
pub(crate) struct ApprovalRow {
    pub(crate) id: Uuid,
    pub(crate) ticket_type: String,
    pub(crate) title: String,
    pub(crate) ref_type: String,
    pub(crate) ref_id: String,
    pub(crate) payload: Value,
    pub(crate) status: String,
    /// `_execute_approval` 里 `a.created_by or user` 需要原始 id。
    pub(crate) created_by_id: Option<Uuid>,
    pub(crate) created_by_username: Option<String>,
    pub(crate) decided_by_username: Option<String>,
    pub(crate) note: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) decided_at: Option<DateTime<Utc>>,
}

/// 键序即蓝本 `_approval_json` 的字典字面量序；时间走 `isoformat()`（`+00:00`）。
pub(crate) fn approval_json(row: &ApprovalRow) -> Value {
    obj(vec![
        ("id", Value::String(row.id.to_string())),
        ("ticket_type", Value::String(row.ticket_type.clone())),
        (
            "ticket_type_display",
            Value::String(ticket_type_display(&row.ticket_type).to_string()),
        ),
        ("title", Value::String(row.title.clone())),
        ("ref_type", Value::String(row.ref_type.clone())),
        ("ref_id", Value::String(row.ref_id.clone())),
        ("payload", row.payload.clone()),
        ("status", Value::String(row.status.clone())),
        (
            "status_display",
            Value::String(approval_status_display(&row.status).to_string()),
        ),
        (
            "created_by",
            Value::String(row.created_by_username.clone().unwrap_or_default()),
        ),
        (
            "decided_by",
            Value::String(row.decided_by_username.clone().unwrap_or_default()),
        ),
        ("note", Value::String(row.note.clone())),
        ("created_at", Value::String(ser::dt_offset(row.created_at))),
        (
            "decided_at",
            Value::String(row.decided_at.map(ser::dt_offset).unwrap_or_default()),
        ),
    ])
}

/// `AgentFeedback` 的一行。
#[derive(Debug, Clone)]
pub(crate) struct FeedbackRow {
    pub(crate) id: Uuid,
    pub(crate) product_name: String,
    pub(crate) batch_code: String,
    pub(crate) rating: i16,
    pub(crate) taste: String,
    pub(crate) package: String,
    pub(crate) note: String,
    pub(crate) created_at: DateTime<Utc>,
}

/// 键序即蓝本 `_feedback_json` 的字典字面量序。
pub(crate) fn feedback_json(row: &FeedbackRow) -> Value {
    obj(vec![
        ("id", Value::String(row.id.to_string())),
        ("product_name", Value::String(row.product_name.clone())),
        ("batch_code", Value::String(row.batch_code.clone())),
        ("rating", Value::Number(row.rating.into())),
        (
            "rating_display",
            Value::String(rating_display(row.rating).to_string()),
        ),
        ("taste", Value::String(row.taste.clone())),
        ("package", Value::String(row.package.clone())),
        ("note", Value::String(row.note.clone())),
        ("created_at", Value::String(ser::dt_offset(row.created_at))),
    ])
}
