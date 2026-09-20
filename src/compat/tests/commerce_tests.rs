//! 商城域的契约回放测试。
//!
//! 目标不是"再跑一遍 replay_diff"（那是端到端，见 `W1_BRIEF.md §6`），而是把
//! **蓝本语义里最容易写错、回放里又看不出原因**的几处钉在单测里：
//!
//! - 金额的 `NUMERIC(_,2)` 文本形态（`ser::dec_scaled` 的补零）；
//! - `category_labels` 的定序规则（D6：`created_at DESC` 取首个出现的 sku_type）；
//! - 购物车/订单的稳定定序（seed 同秒 → 补 `id ASC`）；
//! - `_product_health_payload` 的聚合口径（不合格 > 待复检 > 已核验 > 待检查、果树健康计数）；
//! - `_expire_stale_orders` 的过期取消 + 库存回滚；
//! - 文案字典里的全角标点（`无效主键 “x” － 对象不存在。`）。
//!
//! 铁律：只打 `compat_*` scratch schema；写类用例自带清理，且回放前 `load_seed.py`
//! 会 TRUNCATE 重灌，不会互相污染。

use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use serde_json::{Map, Value, json};
use sqlx::PgPool;
use std::sync::OnceLock;
use uuid::Uuid;

use crate::compat::views_auth::login_impl;
use crate::compat::views_commerce::{
    address_detail_impl, address_impl, after_sale_impl, cart_impl, cart_item_impl,
    expire_stale_orders, invalid_pk_message, order_cancel_impl, order_detail_impl, order_impl,
    order_pay_impl, parse_list_query, parse_replay_now, percent_decode, product_detail_impl,
    product_health_archive_impl, product_list_impl, python_round,
};

// --------------------------------------------------------------------------------------
// 期望值一律从夹具派生（D13：不手抄录制值）
// --------------------------------------------------------------------------------------

/// 库里一定没有的 uuid，用于 404 分支。**不是**录制值，所以可以写字面量。
const UNKNOWN_UUID: &str = "00000000-0000-4000-8000-000000000000";

const SEED_BUYER: &str = "buyer_zhang";
const SEED_BUYER_PASSWORD: &str = "buyer123";
const SEED_FARMER: &str = "farmer_xinfeng";
const SEED_FARMER_PASSWORD: &str = "farmer123";

/// 本文件里 `PRODUCT_XF_FAMILY` 那个语义：「信丰果园的赣南纽荷尔家庭装」。
///
/// 商品**名**是 `seed_demo_data` 写死的，稳定；商品 **id** 是 uuid4，重录会变，
/// 所以只在这里放名字，id 走 [`product_xf_family`]。
const PRODUCT_XF_FAMILY_NAME: &str = "赣南纽荷尔家庭装";

/// `tests/fixtures/contract/commerce.json`：用例名 → 整个用例（含 `path` / `expected_body`）。
///
/// ⚠️ **D13**：条数 / uuid / 数组字面量都是**录制期的实例值**，重录夹具后全部会变。
/// 单测里的期望值必须从这里（或 `index.json.seed_refs`）派生，**不要手抄** ——
/// 手抄的后果不是「夹具过时」，而是把重录后的正常差异**误读成实现回归**。
fn fixture_cases() -> &'static Map<String, Value> {
    static CASES: OnceLock<Map<String, Value>> = OnceLock::new();

    CASES.get_or_init(|| {
        let path = std::path::Path::new("tests/fixtures/contract/commerce.json");
        let text = std::fs::read_to_string(path).unwrap_or_else(|err| {
            panic!(
                "读不到夹具 {}（契约测试必须在 db/ 目录下跑）：{err}",
                path.display()
            )
        });
        let document: Value = serde_json::from_str(&text).expect("commerce.json 不是合法 JSON");

        document["cases"]
            .as_array()
            .expect("commerce.json.cases 必须是数组")
            .iter()
            .map(|case| {
                let name = case["name"].as_str().expect("用例缺 name").to_string();
                (name, case.clone())
            })
            .collect()
    })
}

/// 取某个用例。用例名写错会立刻 panic，不静默放过。
fn case_of(case: &str) -> &'static Value {
    fixture_cases()
        .get(case)
        .unwrap_or_else(|| panic!("commerce.json 里没有用例 {case}"))
}

/// 取某个用例的 `expected_body`（golden）。
fn expected(case: &str) -> &'static Value {
    &case_of(case)["expected_body"]
}

/// `tests/fixtures/contract/index.json`：录制期的 seed refs / capture 值。
fn fixture_index() -> &'static Value {
    static INDEX: OnceLock<Value> = OnceLock::new();

    INDEX.get_or_init(|| {
        let path = std::path::Path::new("tests/fixtures/contract/index.json");
        let text = std::fs::read_to_string(path).unwrap_or_else(|err| {
            panic!(
                "读不到夹具 {}（契约测试必须在 db/ 目录下跑）：{err}",
                path.display()
            )
        });
        serde_json::from_str(&text).expect("index.json 不是合法 JSON")
    })
}

/// 按路径取 `index.json.seed_refs` 里的字符串，例如 `seed_ref(&["address_id"])`。
fn seed_ref(path: &[&str]) -> String {
    let node = path
        .iter()
        .fold(&fixture_index()["seed_refs"], |node, key| &node[*key]);

    node.as_str()
        .unwrap_or_else(|| panic!("index.json.seed_refs.{} 不是字符串", path.join(".")))
        .to_string()
}

/// 商品 id：`seed_refs.product_ids`（按商品名取）。
fn product_id(name: &str) -> String {
    seed_ref(&["product_ids", name])
}

/// 「赣南纽荷尔家庭装」的商品 id。
fn product_xf_family() -> String {
    product_id(PRODUCT_XF_FAMILY_NAME)
}

/// `category_labels` 按**集合**取（D6：蓝本顺序不可复现，我方定序输出）。
fn sorted_labels(value: &Value) -> Vec<String> {
    let mut labels: Vec<String> = value
        .as_array()
        .expect("category_labels 必须是数组")
        .iter()
        .map(|label| label.as_str().expect("label 必须是字符串").to_string())
        .collect();
    labels.sort_unstable();
    labels
}

/// 把 `2026-07-22T15:57:07.970713+00:00` 截成 `2026-07-22T15:57:07.970`。
///
/// 只用于 `verifiedAt`：seed 里的 DateTimeField 被 `dumpdata` 截到毫秒，那 3 位微秒
/// 任何实现都复现不了（回放里被 normalize 屏蔽）。
fn truncated_to_millis(value: &str) -> &str {
    match value.find('.') {
        Some(dot) if value.len() >= dot + 4 => &value[..dot + 4],
        _ => value,
    }
}

/// 按某个键把 `items` 排序后返回副本 —— 用于「**顺序不是契约**」的列表。
///
/// 商品 / 购物车条目的顺序**不在契约里**：蓝本的 queryset 没有 `order_by`
/// （`commerce_views._product_queryset()`），seed 里 `created_at` 同秒的记录只能退化成
/// 按 uuid 定序，而 uuid 是 uuid4 —— **重录夹具后顺序必然改变**。
///
/// 这与 `replay_diff.py` 的 `UNORDERED_LEAVES`（`items`）是同一裁定，所以这里保持同一
/// 口径：先按稳定键排序，再逐字段配对 —— 既不受顺序影响，又保留了「元素必须一一对应」
/// 的强度（比只断 `count` 强得多）。
fn items_sorted_by(items: &Value, key: &str) -> Vec<Value> {
    let mut list: Vec<Value> = items.as_array().expect("items 必须是数组").to_vec();
    list.sort_by_key(|item| item[key].as_str().map(str::to_string).unwrap_or_default());
    list
}

/// 把每个元素里**已知无序**的 `sales_batch.orchard.category_labels` 排好序。
///
/// 这一列的顺序在蓝本里**不可复现**（D6：`.distinct()` 没有 `order_by`），所以它不能参与
/// 整体 `Value` 比较 —— 归一化之后，`items_sorted_by` 的排序结果就可以**整体比对**了
/// （比逐字段列举覆盖面更大：任何漏列的字段差异都会被抓住）。
///
/// ⚠️ 只碰这一个字段：`images` / `feature_tags` 之类的数组先后是有语义的，不能顺手排。
fn with_unordered_labels_sorted(items: Vec<Value>) -> Vec<Value> {
    items
        .into_iter()
        .map(|mut item| {
            if let Some(Value::Array(labels)) =
                item.pointer_mut("/sales_batch/orchard/category_labels")
            {
                labels.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
            }
            item
        })
        .collect()
}

// --------------------------------------------------------------------------------------
// scratch schema（连不上就跳过：离线也不红）
// --------------------------------------------------------------------------------------

fn schema() -> String {
    std::env::var("COMPAT_TEST_SCHEMA").unwrap_or_else(|_| "compat_test".to_string())
}

async fn pool() -> Option<PgPool> {
    let configured = crate::config::AppConfig::load("config.toml")
        .ok()?
        .database
        .postgres_url;

    let schema = schema();
    if !schema.starts_with("compat_") {
        eprintln!("跳过：COMPAT_TEST_SCHEMA 必须是 compat_* scratch schema，实际 {schema}");
        return None;
    }

    let separator = if configured.contains('?') { '&' } else { '?' };
    let url = format!("{configured}{separator}options=-csearch_path%3D{schema}");
    match sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
    {
        Ok(pool) => {
            // `init_database` 全是 `CREATE TABLE IF NOT EXISTS`，与 support.rs 一样幂等。
            crate::server::bootstrap::init_database(&pool, false)
                .await
                .expect("自举建表失败");
            // 本域所有用例都要求「纯种子态」，而 seed 那条待支付订单带一个 20 分钟窗口：
            // 任何一个订单端点都会先跑 `_expire_stale_orders`，把它取消**并回滚库存**。
            // 统一在拿池子的时候推后窗口，别让用例的成败取决于「谁先跑」。
            keep_seed_pending_order_open(&pool).await;
            Some(pool)
        }
        Err(err) => {
            eprintln!("跳过（连不上 scratch schema {url}）: {err}");
            None
        }
    }
}

macro_rules! pool_or_skip {
    () => {
        match pool().await {
            Some(pool) => pool,
            None => return,
        }
    };
}

async fn body_json(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// 用登录接口拿一个真 token 再拼 Bearer 头（token 由回放器在每次写类前重建，不会互相干扰）。
async fn bearer(pool: &PgPool, username: &str, password: &str) -> HeaderMap {
    let (_, body) = body_json(
        login_impl(pool, &json!({"username": username, "password": password}))
            .await
            .expect("登录失败"),
    )
    .await;

    let token = body["data"]["token"].as_str().expect("登录体里没有 token");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    headers
}

fn anonymous() -> HeaderMap {
    HeaderMap::new()
}

// --------------------------------------------------------------------------------------
// 纯函数层（不需要数据库）
// --------------------------------------------------------------------------------------

#[test]
fn python_round_matches_banker_rounding() {
    // 蓝本 `round()` 是银行家舍入；Rust 的 `f64::round` 会在这里给出 16 / 14。
    assert_eq!(python_round(15.5), 16);
    assert_eq!(python_round(16.5), 16);
    assert_eq!(python_round(12.5), 12);
    assert_eq!(python_round(13.5), 14);
    assert_eq!(python_round(15.0), 15);
    assert_eq!(python_round(0.0), 0);
}

#[test]
fn percent_decode_handles_plus_and_escapes() {
    // `urlencode({"q": "脐橙"})` 的形态。
    assert_eq!(percent_decode("%E8%84%90%E6%A9%99"), "脐橙");
    assert_eq!(percent_decode("a+b"), "a b");
    assert_eq!(percent_decode("family"), "family");
    // 非法转义原样保留，不 panic。
    assert_eq!(percent_decode("100%"), "100%");
    assert_eq!(percent_decode("%ZZ"), "%ZZ");
}

#[test]
fn list_query_only_keeps_known_keys_and_trims() {
    let query = parse_list_query(Some("q=%E8%84%90%E6%A9%99&sku_type=family&nope=1"));
    assert_eq!(query.keyword.as_deref(), Some("脐橙"));
    assert_eq!(query.sku_type.as_deref(), Some("family"));
    assert_eq!(query.orchard_id, None);

    // 空串等价于不传（蓝本 `request.GET.get('q', '').strip()` 后判空）。
    let query = parse_list_query(Some("q=&orchard_id="));
    assert_eq!(query.keyword, None);
    assert_eq!(query.orchard_id, None);
}

/// 文案里的全角标点必须是 U+201C / U+201D / **U+FF0D** / U+3002，一个都不能换成 ASCII。
#[test]
fn invalid_pk_message_keeps_fullwidth_punctuation() {
    let message = invalid_pk_message(UNKNOWN_UUID);
    let expected = format!("无效主键 \u{201c}{UNKNOWN_UUID}\u{201d} \u{ff0d} 对象不存在\u{3002}");
    assert_eq!(message, expected);
    assert!(message.contains('\u{ff0d}'), "{message}");
    assert!(
        !message.contains(" - "),
        "必须是全角连字符，不是 ASCII 减号：{message}"
    );
}

#[test]
fn replay_clock_override_parses_rfc3339() {
    let frozen = parse_replay_now("2026-09-20T14:20:40Z").expect("应能解析");
    assert_eq!(frozen.to_rfc3339(), "2026-09-20T14:20:40+00:00");
    // 带偏移也归一到 UTC。
    let frozen = parse_replay_now("2026-09-20T22:20:40+08:00").expect("应能解析");
    assert_eq!(frozen.to_rfc3339(), "2026-09-20T14:20:40+00:00");
    // 坏值不 panic、不冻结。
    assert!(parse_replay_now("1999/01/01").is_none());
    assert!(parse_replay_now("").is_none());
}

// --------------------------------------------------------------------------------------
// 商品
// --------------------------------------------------------------------------------------

/// 商品列表的内容与金额（**不钉顺序**，见 [`items_sorted_by`]）。
///
/// 期望从夹具 `products_list_ok` 派生（D13）：条数、商品名、金额都是录制期的实例值。
#[tokio::test]
async fn product_list_matches_fixture_content() {
    let pool = pool_or_skip!();
    let golden = expected("products_list_ok");

    let (status, body) = body_json(product_list_impl(&pool, None).await.expect("应 200")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "success");

    // `count` 与 `items` 必须自洽（分页字段不能各说各话）。
    let items = body["data"]["items"].as_array().unwrap();
    assert_eq!(body["data"]["count"].as_u64().unwrap(), items.len() as u64);
    assert_eq!(body["data"]["count"], golden["data"]["count"]);

    let actual = with_unordered_labels_sorted(items_sorted_by(&body["data"]["items"], "name"));
    let expected_items =
        with_unordered_labels_sorted(items_sorted_by(&golden["data"]["items"], "name"));
    assert_eq!(actual, expected_items);

    // 金额是字符串且带列精度：NUMERIC(10,2) → "45.00"，不是 JSON number。
    for item in &actual {
        assert!(
            item["price"].is_string(),
            "price 必须是字符串：{}",
            item["price"]
        );
    }
}

/// 关键词 `脐橙` 命中 name/variety/origin/批次标题/果园名的并集；`sku_type=family` 只看家庭装。
///
/// 期望从 `products_list_search_ok` / `products_list_sku_filter_ok` 派生（D13）。
/// 顺序不进契约（[`items_sorted_by`]），`category_labels` 也不进（[`with_unordered_labels_sorted`]）。
#[tokio::test]
async fn product_list_search_and_sku_filter() {
    let pool = pool_or_skip!();
    let search = expected("products_list_search_ok");
    let filtered = expected("products_list_sku_filter_ok");

    let (_, body) = body_json(
        product_list_impl(&pool, Some("q=%E8%84%90%E6%A9%99"))
            .await
            .expect("应 200"),
    )
    .await;
    assert_eq!(body["data"]["count"], search["data"]["count"]);
    assert_eq!(
        with_unordered_labels_sorted(items_sorted_by(&body["data"]["items"], "name")),
        with_unordered_labels_sorted(items_sorted_by(&search["data"]["items"], "name"))
    );

    let (_, body) = body_json(
        product_list_impl(&pool, Some("sku_type=family"))
            .await
            .expect("应 200"),
    )
    .await;
    assert_eq!(body["data"]["count"], filtered["data"]["count"]);
    assert_eq!(
        with_unordered_labels_sorted(items_sorted_by(&body["data"]["items"], "name")),
        with_unordered_labels_sorted(items_sorted_by(&filtered["data"]["items"], "name"))
    );

    // 结构性质：过滤器真的生效了（不是把整张表原样吐回来，也不是恒空）。
    let items = body["data"]["items"].as_array().unwrap();
    assert!(!items.is_empty(), "{body}");
    for item in items {
        assert_eq!(item["sku_type"], "family", "{item}");
    }
    assert!(
        (items.len() as u64) < search["data"]["count"].as_u64().unwrap(),
        "sku_type 过滤必须收窄结果"
    );
}

/// `category_labels` 的定序：按商品 `created_at DESC` 取首个出现的 sku_type（D6）。
///
/// D6 已裁定蓝本这一列的顺序**不可复现**（`.distinct()` 无 `order_by`），所以这里只钉
/// **集合**（内容 + 无重复），顺序不进契约。期望值从 `product_detail_ok` 派生（D13）。
#[tokio::test]
async fn category_labels_follow_created_at_descending() {
    let pool = pool_or_skip!();
    let golden_orchard = &expected("product_detail_ok")["data"]["sales_batch"]["orchard"];

    let (_, body) = body_json(
        product_detail_impl(&pool, &product_xf_family())
            .await
            .expect("应 200"),
    )
    .await;

    let orchard = &body["data"]["sales_batch"]["orchard"];
    let labels = sorted_labels(&orchard["category_labels"]);
    assert_eq!(labels, sorted_labels(&golden_orchard["category_labels"]));

    // `.distinct()` 的语义：不许有重复。
    let raw = orchard["category_labels"].as_array().unwrap();
    let mut deduped = labels.clone();
    deduped.dedup();
    assert_eq!(
        raw.len(),
        deduped.len(),
        "category_labels 不许重复：{raw:?}"
    );

    assert_eq!(orchard["id"], golden_orchard["id"]);
    assert_eq!(orchard["product_count"], golden_orchard["product_count"]);
    assert_eq!(orchard["tree_count"], golden_orchard["tree_count"]);
    assert_eq!(orchard["origin"], golden_orchard["origin"]);
    assert_eq!(orchard["area_mu"], golden_orchard["area_mu"]);
    assert_eq!(
        orchard["primary_trace_code"],
        golden_orchard["primary_trace_code"]
    );
}

#[tokio::test]
async fn product_detail_404_for_unknown_and_invalid_ids() {
    let pool = pool_or_skip!();

    // 业务错误是**成功体形状**（带 timestamp），见 `business_error`。
    let (status, body) = body_json(
        product_detail_impl(&pool, UNKNOWN_UUID)
            .await
            .expect("业务错误走 Ok(Response)"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], 404);
    assert_eq!(body["message"], "商品不存在或已下架");
    assert_eq!(body["data"], Value::Null);
    assert!(body["timestamp"].is_i64(), "{body}");

    // 非法 UUID 同样是 404（蓝本把 UUID 转换失败也算查不到）。
    let (status, _) = body_json(
        product_detail_impl(&pool, "not-a-uuid")
            .await
            .expect("业务错误走 Ok(Response)"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// health-archive 的聚合口径：`qualified` + 采摘档案 + 抽检 + 果树健康计数。
///
/// 期望全部从夹具 `product_health_archive_ok` 派生（D13）——`archiveCode` 是商品 uuid 的
/// 前 10 位 hex、`harvest_code` / 抽检值都是录制期实例值，手抄必然随重录漂移。
#[tokio::test]
async fn health_archive_aggregates_like_blueprint() {
    let pool = pool_or_skip!();
    let golden = expected("product_health_archive_ok");
    let g = &golden["data"];

    let (status, body) = body_json(
        product_health_archive_impl(&pool, &product_xf_family())
            .await
            .expect("应 200"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let data = &body["data"];
    assert_eq!(data["archiveCode"], g["archiveCode"]);
    assert_eq!(data["status"], g["status"]);
    assert_eq!(data["statusDisplay"], g["statusDisplay"]);
    assert_eq!(data["orchard"]["id"], g["orchard"]["id"]);

    // 手工拼装的字段走 Python `isoformat()` 形态（`+00:00`）。
    assert!(
        data["updatedAt"].as_str().unwrap().ends_with("+00:00"),
        "{}",
        data["updatedAt"]
    );
    // `verifiedAt` 是手工拼装的 `isoformat()` 形态（`+00:00`）。
    //
    // ⚠️ 只比到**毫秒**：夹具的 `seed.json` 里 DateTimeField 被 dumpdata 截断到毫秒
    // （`2026-07-22T15:57:07.970Z`），而 golden body 录的是线上库的微秒
    // （`...970176`）。那 3 位微秒不在 seed 里，任何实现都复现不了 —— 回放里靠
    // `$.data.orchardHealth.verifiedAt` 的 normalize 屏蔽。
    let verified_at = data["orchardHealth"]["verifiedAt"].as_str().unwrap();
    let golden_verified_at = g["orchardHealth"]["verifiedAt"].as_str().unwrap();
    assert!(verified_at.ends_with("+00:00"), "{verified_at}");
    assert_eq!(
        truncated_to_millis(verified_at),
        truncated_to_millis(golden_verified_at),
        "只比到毫秒（微秒不在 seed 里）"
    );
    assert_eq!(
        data["orchardHealth"]["status"],
        g["orchardHealth"]["status"]
    );
    assert_eq!(
        data["orchardHealth"]["archiveCode"],
        g["orchardHealth"]["archiveCode"]
    );

    // 模型字段走 DRF `JSONEncoder` 形态（`Z`）。
    assert!(
        data["treeHealthSummary"]["latestObservedAt"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );

    assert_eq!(
        data["harvestStatus"]["status"],
        g["harvestStatus"]["status"]
    );
    assert_eq!(
        data["harvestStatus"]["recordCount"],
        g["harvestStatus"]["recordCount"]
    );
    assert_eq!(
        data["harvestStatus"]["expectedWindow"],
        g["harvestStatus"]["expectedWindow"]
    );
    assert_eq!(
        data["harvestStatus"]["latestRecord"]["harvest_code"],
        g["harvestStatus"]["latestRecord"]["harvest_code"]
    );
    assert_eq!(
        data["harvestStatus"]["latestRecord"]["quantity_kg"],
        g["harvestStatus"]["latestRecord"]["quantity_kg"]
    );

    assert_eq!(
        data["qualitySummary"]["recordCount"],
        g["qualitySummary"]["recordCount"]
    );
    assert_eq!(
        data["qualitySummary"]["latestRecord"]["sweetness_brix"],
        g["qualitySummary"]["latestRecord"]["sweetness_brix"]
    );
    assert_eq!(
        data["qualitySummary"]["latestRecord"]["acidity"],
        g["qualitySummary"]["latestRecord"]["acidity"]
    );

    assert_eq!(
        data["treeHealthSummary"]["total"],
        g["treeHealthSummary"]["total"]
    );
    assert_eq!(
        data["treeHealthSummary"]["healthy"],
        g["treeHealthSummary"]["healthy"]
    );
    assert_eq!(
        data["treeHealthSummary"]["needsAttention"],
        g["treeHealthSummary"]["needsAttention"]
    );

    let trees = data["fruitTrees"].as_array().unwrap();
    let golden_trees = g["fruitTrees"].as_array().unwrap();
    assert_eq!(trees.len(), golden_trees.len());
    // `Meta.ordering = ['-is_featured', 'tree_number']`：逐棵比对顺序与状态。
    for (tree, golden_tree) in trees.iter().zip(golden_trees) {
        assert_eq!(tree["tree_number"], golden_tree["tree_number"]);
        assert_eq!(
            tree["health_status_display"],
            golden_tree["health_status_display"]
        );
    }

    // 事件只取 orchard/environment/quality/harvest/sorting（packing/shipping 不算健康事件），
    // 且按 occurred_at 升序。
    let events = data["healthEvents"].as_array().unwrap();
    let golden_events = g["healthEvents"].as_array().unwrap();
    assert_eq!(events.len(), golden_events.len());
    for (event, golden_event) in events.iter().zip(golden_events) {
        assert_eq!(event["event_type"], golden_event["event_type"]);
    }
    assert_eq!(events[0]["hash_short"].as_str().unwrap().len(), 12);
}

#[tokio::test]
async fn health_archive_404_messages() {
    let pool = pool_or_skip!();

    let (status, body) = body_json(
        product_health_archive_impl(&pool, UNKNOWN_UUID)
            .await
            .expect("业务错误走 Ok(Response)"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["message"], "商品不存在、已下架或尚未关联供货档案");
    assert!(body["timestamp"].is_i64(), "{body}");
}

// --------------------------------------------------------------------------------------
// 购物车
// --------------------------------------------------------------------------------------

#[tokio::test]
async fn cart_requires_buyer() {
    let pool = pool_or_skip!();

    let reject = cart_impl(&pool, &anonymous(), &json!({}), true)
        .await
        .err()
        .expect("匿名应 401");
    let (status, body) = body_json(reject.into_response()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["message"], "身份认证信息未提供。");
    assert!(body.get("timestamp").is_none(), "401 是 DRF 异常体：{body}");

    let farmer = bearer(&pool, SEED_FARMER, SEED_FARMER_PASSWORD).await;
    let reject = cart_impl(&pool, &farmer, &json!({}), true)
        .await
        .err()
        .expect("果农应 403");
    let (status, body) = body_json(reject.into_response()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["message"], "该接口仅限购买者使用");
}

/// 购物车快照：两条 seed 条目 + 合计。期望从 `cart_get_ok` 派生（D13）。
///
/// 顺序不进契约：两条 seed 条目 `updated_at` 同秒，只能靠 uuid 定序，而 uuid 重录即变
/// （同 [`items_sorted_by`]）。
#[tokio::test]
async fn cart_snapshot_matches_fixture() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;
    let golden = expected("cart_get_ok");

    let (status, body) = body_json(
        cart_impl(&pool, &buyer, &json!({}), true)
            .await
            .expect("应 200"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let actual = items_sorted_by(&body["data"]["items"], "id");
    let expected_items = items_sorted_by(&golden["data"]["items"], "id");
    assert_eq!(actual.len(), expected_items.len());

    for (item, golden_item) in actual.iter().zip(&expected_items) {
        assert_eq!(item["id"], golden_item["id"]);
        assert_eq!(item["product"]["name"], golden_item["product"]["name"]);
        assert_eq!(item["quantity"], golden_item["quantity"]);
        assert_eq!(item["subtotal"], golden_item["subtotal"]);
    }
    // 合计与顺序无关。
    assert_eq!(body["data"]["totalAmount"], golden["data"]["totalAmount"]);
}

/// 把 seed 里那条**待支付**订单的过期窗口推后，让用例与「什么时候跑」无关。
///
/// `seed_demo_data` 给 `ORD-DEMO-1003` 的 `expires_at` 是 `now + timedelta(minutes=20)`，
/// 即「**录制时刻 + 20 分钟**」。所以录制 20 分钟之后再跑，蓝本的 `_expire_stale_orders`
/// （`GET /api/orders` 会调它）就会把这条订单取消**并回滚库存**
/// （`红肉脐橙家庭装` 300 → 301），连带把商品列表的库存断言弄红。
///
/// ⚠️ 那不是实现行为，是 **seed 的时序窗口**：seed 的领域语义是「有一条待支付订单」，
/// 「20 分钟」只是录制时刻的副产物。单测应该在**纯种子态**上跑，所以这里把窗口推后 ——
/// 只动这一个 `expires_at`，不碰任何领域值；`order_number` 是 seed 写死的稳定键。
///
/// 回放侧**不能**这么修（`replay_diff.py` 与夹具都不许动），所以那边仍有这个时限，
/// 见 `DEVIATIONS.md` 收口记录 §5。
async fn keep_seed_pending_order_open(pool: &PgPool) {
    sqlx::query(
        r#"UPDATE "order" SET expires_at = now() + interval '30 days'
            WHERE order_number = 'ORD-DEMO-1003'"#,
    )
    .execute(pool)
    .await
    .expect("推后 seed 待支付订单的过期窗口失败");
}

/// 购物车条目 id 走哪个商品：取夹具里排在最前面的那一条（`updated_at DESC, id ASC`）。
fn first_cart_item_id() -> String {
    expected("cart_get_ok")["data"]["items"][0]["id"]
        .as_str()
        .expect("cart_get_ok 的第一条缺 id")
        .to_string()
}

#[tokio::test]
async fn cart_post_validates_serializer() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;

    let (status, body) = body_json(
        cart_impl(&pool, &buyer, &json!({}), false)
            .await
            .expect("校验失败是成功体形状"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], json!({"product_id": ["该字段是必填项。"]}));
    assert_eq!(body["data"], Value::Null);
    assert!(body["timestamp"].is_i64(), "{body}");

    let (status, body) = body_json(
        cart_impl(
            &pool,
            &buyer,
            &json!({"product_id": "not-a-uuid", "quantity": 1}),
            false,
        )
        .await
        .expect("校验失败是成功体形状"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["message"],
        json!({"product_id": ["Must be a valid UUID."]})
    );

    let (status, body) = body_json(
        cart_impl(
            &pool,
            &buyer,
            &json!({"product_id": product_xf_family(), "quantity": 0}),
            false,
        )
        .await
        .expect("校验失败是成功体形状"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["message"],
        json!({"quantity": ["请确保该值大于或者等于 1。"]})
    );
}

#[tokio::test]
async fn cart_item_404_and_farmer_403() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;

    let (status, body) = body_json(
        cart_item_impl(&pool, &buyer, UNKNOWN_UUID, &json!({"quantity": 1}), false)
            .await
            .expect("业务错误走 Ok(Response)"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["message"], "购物车商品不存在");
    assert!(body["timestamp"].is_i64(), "{body}");

    let farmer = bearer(&pool, SEED_FARMER, SEED_FARMER_PASSWORD).await;
    let reject = cart_item_impl(
        &pool,
        &farmer,
        &first_cart_item_id(),
        &json!({"quantity": 1}),
        false,
    )
    .await
    .err()
    .expect("果农应 403");
    let (status, body) = body_json(reject.into_response()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["message"], "该接口仅限购买者使用");
}

// --------------------------------------------------------------------------------------
// 地址
// --------------------------------------------------------------------------------------

/// 地址列表：默认地址排第一。期望从 `addresses_list_ok` 派生（D13）。
#[tokio::test]
async fn address_list_prefers_default_first() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;
    let golden_items = expected("addresses_list_ok")["data"]
        .as_array()
        .expect("夹具的 data 必须是数组");

    let (status, body) = body_json(
        address_impl(&pool, &buyer, &json!({}), true)
            .await
            .expect("应 200"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let items = body["data"].as_array().unwrap();
    assert_eq!(items.len(), golden_items.len());
    for (item, golden_item) in items.iter().zip(golden_items) {
        assert_eq!(item["id"], golden_item["id"]);
        assert_eq!(item["recipient_name"], golden_item["recipient_name"]);
        assert_eq!(item["is_default"], golden_item["is_default"]);
    }

    // 「默认优先」的结构性质：第一条是默认，其余都不是。
    assert_eq!(items[0]["is_default"], true);
    assert!(
        items[1..]
            .iter()
            .all(|item| item["is_default"] == json!(false)),
        "{items:?}"
    );

    // 缺少必填字段时按声明序报错，且是成功体形状。
    let (status, body) = body_json(
        address_impl(&pool, &buyer, &json!({}), false)
            .await
            .expect("应 400"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["message"],
        json!({
            "recipient_name": ["该字段是必填项。"],
            "phone": ["该字段是必填项。"],
            "detail": ["该字段是必填项。"]
        })
    );
}

/// 写类：新建 → 改 → 删，全程只碰临时买家的数据。
#[tokio::test]
async fn address_create_patch_delete_round_trip() {
    let pool = pool_or_skip!();
    let buyer_name = unique_username("w1c_buyer");
    let buyer_id = create_buyer(&pool, &buyer_name).await;
    let headers = bearer(&pool, &buyer_name, "qa-pass-123456").await;

    let (status, body) = body_json(
        address_impl(
            &pool,
            &headers,
            &json!({
                "recipient_name": "李四",
                "phone": "13900139000",
                "province": "江西省",
                "city": "赣州市",
                "district": "南康区",
                "detail": "和谐大道 1 号",
                "is_default": false
            }),
            false,
        )
        .await
        .expect("应 200"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "收货地址已保存");
    // 第一条地址被强制设为默认。
    assert_eq!(body["data"]["is_default"], true);
    let address_id = body["data"]["id"].as_str().unwrap().to_string();

    let (status, body) = body_json(
        address_detail_impl(
            &pool,
            &headers,
            &address_id,
            &json!({"detail": "科技园路 9 号"}),
            false,
        )
        .await
        .expect("应 200"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "收货地址已更新");
    assert_eq!(body["data"]["detail"], "科技园路 9 号");
    assert_eq!(body["data"]["is_default"], true);

    let (status, body) = body_json(
        address_detail_impl(&pool, &headers, &address_id, &json!({}), true)
            .await
            .expect("应 200"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "收货地址已删除");
    assert_eq!(body["data"], Value::Null);

    cleanup_buyer(&pool, buyer_id).await;
}

#[tokio::test]
async fn address_detail_404_for_unknown_id() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;

    let (status, body) = body_json(
        address_detail_impl(&pool, &buyer, UNKNOWN_UUID, &json!({"detail": "x"}), false)
            .await
            .expect("业务错误走 Ok(Response)"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["message"], "收货地址不存在");
}

// --------------------------------------------------------------------------------------
// 订单
// --------------------------------------------------------------------------------------

/// 订单列表：`Meta.ordering = ['-created_at']`。期望从 `orders_list_ok` 派生（D13）。
#[tokio::test]
async fn orders_list_matches_fixture() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;
    let golden_orders = expected("orders_list_ok")["data"]
        .as_array()
        .expect("夹具的 data 必须是数组");

    let (status, body) = body_json(
        order_impl(&pool, &buyer, &json!({}), true)
            .await
            .expect("应 200"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let orders = body["data"].as_array().unwrap();
    assert_eq!(orders.len(), golden_orders.len());

    // `Meta.ordering = ['-created_at']`；1002/1003 的 `created_at` 同秒，靠 id 定序。
    assert_eq!(orders[0]["id"], golden_orders[0]["id"]);
    assert_eq!(orders[0]["order_number"], golden_orders[0]["order_number"]);
    assert_eq!(orders[1]["order_number"], golden_orders[1]["order_number"]);
    assert_eq!(orders[2]["id"], golden_orders[2]["id"]);

    // `keep_seed_pending_order_open` 已经把窗口推后，所以这里可以**钉死** `pending_payment`
    // 并逐字段对齐夹具 —— 不必再容忍 `cancelled`（那种容忍正好会掩盖
    // 「`_expire_stale_orders` 提前动手」这类真实问题）。
    assert_eq!(orders[0]["status"], golden_orders[0]["status"]);
    assert_eq!(orders[0]["amount_due"], golden_orders[0]["amount_due"]);
    assert_eq!(
        orders[0]["payment_action_label"],
        golden_orders[0]["payment_action_label"]
    );

    let items = orders[0]["items"].as_array().unwrap();
    let golden_items = golden_orders[0]["items"].as_array().unwrap();
    assert_eq!(items.len(), golden_items.len());
    for (item, golden_item) in items.iter().zip(golden_items) {
        assert_eq!(item["product_name"], golden_item["product_name"]);
        assert_eq!(item["unit_price"], golden_item["unit_price"]);
        assert_eq!(item["quantity"], golden_item["quantity"]);
        assert_eq!(item["subtotal"], golden_item["subtotal"]);
    }

    assert_eq!(
        orders[2]["status_display"],
        golden_orders[2]["status_display"]
    );
    assert_eq!(orders[2]["amount_due"], golden_orders[2]["amount_due"]);
    assert_eq!(
        orders[2]["payment_action_label"],
        golden_orders[2]["payment_action_label"]
    );
}

/// 从用例的 `path` 末段取 uuid：夹具里的路径参数就是录制期的实例 id（D13）。
fn path_arg(case: &str) -> String {
    case_of(case)["path"]
        .as_str()
        .expect("用例缺 path")
        .rsplit('/')
        .next()
        .expect("path 一定有末段")
        .to_string()
}

#[tokio::test]
async fn order_detail_and_404() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;
    let golden = expected("order_detail_ok");
    let g = &golden["data"];

    let (_, body) = body_json(
        order_detail_impl(&pool, &buyer, &path_arg("order_detail_ok"))
            .await
            .expect("应 200"),
    )
    .await;

    let order = &body["data"];
    assert_eq!(order["id"], g["id"]);
    assert_eq!(order["order_number"], g["order_number"]);
    assert_eq!(order["status"], g["status"]);
    assert_eq!(order["total_amount"], g["total_amount"]);
    assert_eq!(order["paid_amount"], g["paid_amount"]);

    let items = order["items"].as_array().unwrap();
    let golden_items = g["items"].as_array().unwrap();
    assert_eq!(items.len(), golden_items.len());
    assert_eq!(items[0]["product_name"], golden_items[0]["product_name"]);
    assert_eq!(items[0]["unit_price"], golden_items[0]["unit_price"]);

    let packages = order["trace_packages"].as_array().unwrap();
    let golden_packages = g["trace_packages"].as_array().unwrap();
    assert_eq!(packages.len(), golden_packages.len());
    for (package, golden_package) in packages.iter().zip(golden_packages) {
        assert_eq!(package["box_spec"], golden_package["box_spec"]);
        assert_eq!(package["status_display"], golden_package["status_display"]);
    }
    assert_eq!(
        order["payments"].as_array().unwrap().len(),
        g["payments"].as_array().unwrap().len()
    );

    let (status, body) = body_json(
        order_detail_impl(&pool, &buyer, UNKNOWN_UUID)
            .await
            .expect("业务错误走 Ok(Response)"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["message"], "订单不存在");
    assert!(body["timestamp"].is_i64(), "{body}");
}

#[tokio::test]
async fn order_pay_and_cancel_reject_completed_order() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;
    let completed = path_arg("order_detail_ok");

    let (status, body) = body_json(
        order_pay_impl(&pool, &buyer, &completed)
            .await
            .expect("业务错误走 Ok(Response)"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], "当前订单状态不能支付");
    assert!(body["timestamp"].is_i64(), "{body}");

    let (status, body) = body_json(
        order_cancel_impl(&pool, &buyer, &completed)
            .await
            .expect("业务错误走 Ok(Response)"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], "当前订单状态不能取消");
}

#[tokio::test]
async fn orders_create_requires_address_field() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;

    let (status, body) = body_json(
        order_impl(&pool, &buyer, &json!({}), false)
            .await
            .expect("应 400"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], json!({"address_id": ["该字段是必填项。"]}));

    let (status, body) = body_json(
        order_impl(&pool, &buyer, &json!({"address_id": UNKNOWN_UUID}), false)
            .await
            .expect("业务错误走 Ok(Response)"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["message"], "收货地址不存在");
}

/// `_expire_stale_orders`：过期未支付订单被取消，商品库存回滚。
#[tokio::test]
async fn expire_stale_orders_cancels_and_restores_stock() {
    let pool = pool_or_skip!();
    let buyer_name = unique_username("w1c_expire");
    let buyer_id = create_buyer(&pool, &buyer_name).await;

    let order_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO \"order\" (id, order_number, buyer_id, sales_batch_id, status, payment_mode, \
                                recipient_name, recipient_phone, shipping_address, total_amount, \
                                deposit_amount, balance_amount, paid_amount, note, expires_at, \
                                created_at, updated_at) \
         VALUES ($1, $2, $3, NULL, 'pending_payment', 'full', '张三', '13800138000', '地址', 39.90, \
                 0, 39.90, 0, '', $4, $5, $5)",
    )
    .bind(order_id)
    .bind(format!("NO-EXPIRE-{}", &order_id.simple().to_string()[..8]))
    .bind(buyer_id)
    .bind(now - chrono::Duration::minutes(1))
    .bind(now)
    .execute(&pool)
    .await
    .expect("插入临时订单失败");

    // 挂一条明细，验证库存回滚（用 seed 商品，回放前会重灌 seed）。
    // 商品 id 从夹具派生（D13），不手抄录制值。
    let seed_product =
        Uuid::parse_str(&product_xf_family()).expect("夹具里的商品 id 必须是合法 uuid");
    let stock_before: i32 = sqlx::query_scalar("SELECT stock FROM citrus_product WHERE id = $1")
        .bind(seed_product)
        .fetch_one(&pool)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO order_item (id, order_id, product_id, product_name, product_image_url, \
                                 batch_code, batch_title, sku_type, unit_price, unit, quantity, \
                                 subtotal) \
         VALUES ($1, $2, $3, '临时', '', '', '', 'family', 39.90, '5斤/箱', 2, 79.80)",
    )
    .bind(Uuid::new_v4())
    .bind(order_id)
    .bind(seed_product)
    .execute(&pool)
    .await
    .expect("插入临时明细失败");

    expire_stale_orders(&pool, Some(buyer_id))
        .await
        .expect("过期清理失败");

    let (status, cancelled_at): (String, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT status, cancelled_at FROM \"order\" WHERE id = $1")
            .bind(order_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "cancelled");
    assert!(cancelled_at.is_some());

    let stock_after: i32 = sqlx::query_scalar("SELECT stock FROM citrus_product WHERE id = $1")
        .bind(seed_product)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stock_after, stock_before + 2);

    // 观测完把商品库存改回去 + 删掉临时数据。
    sqlx::query("UPDATE citrus_product SET stock = $2 WHERE id = $1")
        .bind(seed_product)
        .bind(stock_before)
        .execute(&pool)
        .await
        .unwrap();
    cleanup_buyer(&pool, buyer_id).await;
}

// --------------------------------------------------------------------------------------
// 售后
// --------------------------------------------------------------------------------------

/// 售后列表。期望从 `after_sales_list_ok` 派生（D13）。
#[tokio::test]
async fn after_sales_list_matches_fixture() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;
    let golden_items = expected("after_sales_list_ok")["data"]
        .as_array()
        .expect("夹具的 data 必须是数组");

    let (_, body) = body_json(
        after_sale_impl(&pool, &buyer, &json!({}), true)
            .await
            .expect("应 200"),
    )
    .await;

    let items = body["data"].as_array().unwrap();
    assert_eq!(items.len(), golden_items.len());
    for (item, golden_item) in items.iter().zip(golden_items) {
        assert_eq!(item["id"], golden_item["id"]);
        assert_eq!(item["issue_type"], golden_item["issue_type"]);
        assert_eq!(
            item["issue_type_display"],
            golden_item["issue_type_display"]
        );
        assert_eq!(item["status_display"], golden_item["status_display"]);
        assert_eq!(item["refund_amount"], golden_item["refund_amount"]);
    }
}

#[tokio::test]
async fn after_sale_validation_errors() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;

    let (status, body) = body_json(
        after_sale_impl(&pool, &buyer, &json!({}), false)
            .await
            .expect("应 400"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["message"],
        json!({
            "order": ["该字段是必填项。"],
            "issue_type": ["该字段是必填项。"],
            "description": ["该字段是必填项。"]
        })
    );

    let (status, body) = body_json(
        after_sale_impl(
            &pool,
            &buyer,
            &json!({"order": UNKNOWN_UUID, "issue_type": "other", "description": "qa"}),
            false,
        )
        .await
        .expect("应 400"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["message"],
        json!({"order": [invalid_pk_message(UNKNOWN_UUID)]})
    );
}

// --------------------------------------------------------------------------------------
// 临时买家夹具
// --------------------------------------------------------------------------------------

fn unique_username(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

async fn create_buyer(pool: &PgPool, username: &str) -> Uuid {
    let id = Uuid::new_v4();
    let now = chrono::Utc::now();

    sqlx::query(
        r#"INSERT INTO "user" (id, username, password, email, role, orchard_address, latitude,
                               longitude, created_at, updated_at)
           VALUES ($1, $2, $3, NULL, 'buyer', NULL, NULL, NULL, $4, $4)"#,
    )
    .bind(id)
    .bind(username)
    .bind(crate::compat::auth::hash_password("qa-pass-123456"))
    .bind(now)
    .execute(pool)
    .await
    .expect("建临时买家失败");

    id
}

/// 清理临时买家：先把外键（`order.buyer_id` 是 RESTRICT）清掉，再删用户。
async fn cleanup_buyer(pool: &PgPool, buyer_id: Uuid) {
    let _ = sqlx::query("DELETE FROM after_sale_request WHERE buyer_id = $1")
        .bind(buyer_id)
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM payment_record WHERE order_id IN (SELECT id FROM \"order\" WHERE buyer_id = $1)",
    )
    .bind(buyer_id)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM trace_package WHERE order_id IN (SELECT id FROM \"order\" WHERE buyer_id = $1)",
    )
    .bind(buyer_id)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM order_item WHERE order_id IN (SELECT id FROM \"order\" WHERE buyer_id = $1)",
    )
    .bind(buyer_id)
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM \"order\" WHERE buyer_id = $1")
        .bind(buyer_id)
        .execute(pool)
        .await;
    let _ = sqlx::query(r#"DELETE FROM "user" WHERE id = $1"#)
        .bind(buyer_id)
        .execute(pool)
        .await;
}
