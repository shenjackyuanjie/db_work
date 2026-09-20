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
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::compat::views_auth::login_impl;
use crate::compat::views_commerce::{
    address_detail_impl, address_impl, after_sale_impl, cart_impl, cart_item_impl,
    expire_stale_orders, invalid_pk_message, order_cancel_impl, order_detail_impl, order_impl,
    order_pay_impl, parse_list_query, parse_replay_now, percent_decode, product_detail_impl,
    product_health_archive_impl, product_list_impl, python_round,
};

// --------------------------------------------------------------------------------------
// 夹具里的固定 id（`index.json.seed_refs`）
// --------------------------------------------------------------------------------------

const PRODUCT_XF_FAMILY: &str = "04d854b4-aa5b-4d63-8f3c-f40819927ec6";
const PRODUCT_NEWHALL_ENTERPRISE: &str = "8e31b734-be41-4568-9f97-97fd70e86ce2";
const CART_ITEM_B1: &str = "22b203eb-9770-485d-96fe-7747cff23807";
const ADDRESS_DEFAULT: &str = "e49860c6-6817-4bdf-b8e0-c3298cb6aa8d";
const ORDER_COMPLETED: &str = "353ef930-8477-4648-82ea-332ad00b3888";
const ORDER_PENDING: &str = "3a0e20d0-5e51-4553-b0ec-7280321b4e5a";
const UNKNOWN_UUID: &str = "00000000-0000-4000-8000-000000000000";

const SEED_BUYER: &str = "buyer_zhang";
const SEED_BUYER_PASSWORD: &str = "buyer123";
const SEED_FARMER: &str = "farmer_xinfeng";
const SEED_FARMER_PASSWORD: &str = "farmer123";

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

/// 列表顺序 = `sort_order ASC, created_at DESC, id ASC`（seed 里同秒的两条靠 id 定序）。
#[tokio::test]
async fn product_list_matches_fixture_order_and_amounts() {
    let pool = pool_or_skip!();

    let (status, body) = body_json(product_list_impl(&pool, None).await.expect("应 200")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "success");

    let items = body["data"]["items"].as_array().unwrap();
    assert_eq!(body["data"]["count"], 7);
    assert_eq!(items.len(), 7);

    let names: Vec<&str> = items
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "红肉脐橙家庭装",
            "红肉脐橙企业装",
            "朋娜脐橙精品尝鲜装",
            "朋娜脐橙礼赠装",
            "纽荷尔企业装",
            "纽荷尔试吃装",
            "赣南纽荷尔家庭装",
        ]
    );

    // 金额是字符串且带列精度：NUMERIC(10,2) → "45.00"。
    assert_eq!(items[0]["price"], "45.00");
    assert_eq!(items[0]["sweetness"], "13.8");
    assert_eq!(items[6]["price"], "39.90");
    assert_eq!(items[6]["sales_batch"]["deposit_ratio"], "0.00");
    assert_eq!(items[6]["sales_batch"]["progress_percent"], 15);
    assert_eq!(items[6]["sales_batch"]["available_quantity"], 680);
    assert_eq!(items[6]["is_available"], true);
}

/// 关键词 `脐橙` 命中 name/variety/origin/批次标题/果园名的并集（这里等于全部 7 条）。
#[tokio::test]
async fn product_list_search_and_sku_filter() {
    let pool = pool_or_skip!();

    let (_, body) = body_json(
        product_list_impl(&pool, Some("q=%E8%84%90%E6%A9%99"))
            .await
            .expect("应 200"),
    )
    .await;
    assert_eq!(body["data"]["count"], 7);

    let (_, body) = body_json(
        product_list_impl(&pool, Some("sku_type=family"))
            .await
            .expect("应 200"),
    )
    .await;
    let items = body["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["name"], "红肉脐橙家庭装");
    assert_eq!(items[1]["name"], "赣南纽荷尔家庭装");
}

/// `category_labels` 的定序：按商品 `created_at DESC` 取首个出现的 sku_type（D6）。
#[tokio::test]
async fn category_labels_follow_created_at_descending() {
    let pool = pool_or_skip!();

    let (_, body) = body_json(
        product_detail_impl(&pool, PRODUCT_XF_FAMILY)
            .await
            .expect("应 200"),
    )
    .await;

    let orchard = &body["data"]["sales_batch"]["orchard"];
    assert_eq!(
        orchard["category_labels"],
        json!(["企业装", "试吃装", "家庭装"])
    );
    assert_eq!(orchard["product_count"], 3);
    assert_eq!(orchard["tree_count"], 3);
    assert_eq!(orchard["origin"], "江西省赣州市信丰县");
    assert_eq!(orchard["area_mu"], "86.50");
    assert_eq!(orchard["primary_trace_code"], "CGJ-BATCH-XF2026");
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

/// health-archive 的聚合口径：`qualified` + 2 份采摘档案 + 1 条抽检 + 3 棵树（2 健康 1 观察）。
#[tokio::test]
async fn health_archive_aggregates_like_blueprint() {
    let pool = pool_or_skip!();

    let (status, body) = body_json(
        product_health_archive_impl(&pool, PRODUCT_XF_FAMILY)
            .await
            .expect("应 200"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let data = &body["data"];
    assert_eq!(data["archiveCode"], "CGJ-HEALTH-04D854B4AA");
    assert_eq!(data["status"], "qualified");
    assert_eq!(data["statusDisplay"], "健康档案已核验");

    // 手工拼装的字段走 Python `isoformat()` 形态（`+00:00`）。
    assert!(
        data["updatedAt"].as_str().unwrap().ends_with("+00:00"),
        "{}",
        data["updatedAt"]
    );
    // `verifiedAt` 是手工拼装的 `isoformat()` 形态（`+00:00`）。
    //
    // ⚠️ 只断言到**毫秒**：夹具的 `seed.json` 里 DateTimeField 被截断到毫秒
    // （`2026-07-22T14:20:37.922Z`），而 golden body 录的是线上库的微秒
    // （`...922176`）。这 6 位微秒不在 seed 里，任何实现都复现不了——回放里靠
    // `$.data.orchardHealth.verifiedAt` 的 normalize 屏蔽。
    let verified_at = data["orchardHealth"]["verifiedAt"].as_str().unwrap();
    assert!(verified_at.ends_with("+00:00"), "{verified_at}");
    assert!(
        verified_at.starts_with("2026-07-22T14:20:37.922"),
        "{verified_at}"
    );
    assert_eq!(data["orchardHealth"]["status"], "verified");
    assert_eq!(data["orchardHealth"]["archiveCode"], "GY-XF-DEMO-01");

    // 模型字段走 DRF `JSONEncoder` 形态（`Z`）。
    assert!(
        data["treeHealthSummary"]["latestObservedAt"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );

    assert_eq!(data["harvestStatus"]["status"], "recorded");
    assert_eq!(data["harvestStatus"]["recordCount"], 2);
    assert_eq!(
        data["harvestStatus"]["expectedWindow"]["start"],
        "2026-09-23"
    );
    assert_eq!(
        data["harvestStatus"]["latestRecord"]["harvest_code"],
        "CGJ-HV-83563CD93E"
    );
    assert_eq!(
        data["harvestStatus"]["latestRecord"]["quantity_kg"],
        "132.00"
    );

    assert_eq!(data["qualitySummary"]["recordCount"], 1);
    assert_eq!(
        data["qualitySummary"]["latestRecord"]["sweetness_brix"],
        "12.6"
    );
    assert_eq!(
        data["qualitySummary"]["latestRecord"]["acidity"],
        Value::Null
    );

    assert_eq!(data["treeHealthSummary"]["total"], 3);
    assert_eq!(data["treeHealthSummary"]["healthy"], 2);
    assert_eq!(data["treeHealthSummary"]["needsAttention"], 1);

    let trees = data["fruitTrees"].as_array().unwrap();
    assert_eq!(trees.len(), 3);
    // `Meta.ordering = ['-is_featured', 'tree_number']`。
    assert_eq!(trees[0]["tree_number"], "信丰-01-001");
    assert_eq!(trees[0]["health_status_display"], "生长良好");
    assert_eq!(trees[2]["tree_number"], "信丰-02-005");
    assert_eq!(trees[2]["health_status_display"], "持续观察");

    // 事件只取 orchard/environment/quality/harvest/sorting（packing/shipping 不算健康事件），
    // 且按 occurred_at 升序。
    let events = data["healthEvents"].as_array().unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0]["event_type"], "orchard");
    assert_eq!(events[1]["event_type"], "environment");
    assert_eq!(events[3]["event_type"], "sorting");
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

#[tokio::test]
async fn cart_snapshot_matches_fixture() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;

    let (status, body) = body_json(
        cart_impl(&pool, &buyer, &json!({}), true)
            .await
            .expect("应 200"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let items = body["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    // 两条 seed 条目 `updated_at` 同秒 → 靠 id 定序，与夹具一致。
    assert_eq!(items[0]["id"], CART_ITEM_B1);
    assert_eq!(items[0]["product"]["name"], "纽荷尔企业装");
    assert_eq!(items[0]["quantity"], 2);
    assert_eq!(items[0]["subtotal"], "318.00");
    assert_eq!(body["data"]["totalAmount"], "486.00");
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
            &json!({"product_id": PRODUCT_XF_FAMILY, "quantity": 0}),
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
    let reject = cart_item_impl(&pool, &farmer, CART_ITEM_B1, &json!({"quantity": 1}), false)
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

#[tokio::test]
async fn address_list_prefers_default_first() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;

    let (status, body) = body_json(
        address_impl(&pool, &buyer, &json!({}), true)
            .await
            .expect("应 200"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let items = body["data"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], ADDRESS_DEFAULT);
    assert_eq!(items[0]["is_default"], true);
    assert_eq!(items[1]["is_default"], false);

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

#[tokio::test]
async fn orders_list_matches_fixture() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;

    let (status, body) = body_json(
        order_impl(&pool, &buyer, &json!({}), true)
            .await
            .expect("应 200"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let orders = body["data"].as_array().unwrap();
    assert_eq!(orders.len(), 3);
    assert_eq!(orders[0]["id"], ORDER_PENDING);
    assert_eq!(orders[0]["order_number"], "ORD-DEMO-1003");
    // ⚠️ 这条 seed 订单的 `expires_at` 只有 20 分钟窗口（`seed_demo_data.py` 用
    // `now + timedelta(minutes=20)` 造的数据），而蓝本的 `_expire_stale_orders` 会在
    // `GET /api/orders` 时把过期的待支付订单取消。窗口之外跑测试时它已经是 `cancelled`，
    // 所以这里只钉「状态机渲染正确」，过期行为另见 `expire_stale_orders_cancels_and_restores_stock`。
    let status = orders[0]["status"].as_str().unwrap();
    assert!(
        status == "pending_payment" || status == "cancelled",
        "意外状态：{status}"
    );
    if status == "pending_payment" {
        assert_eq!(orders[0]["amount_due"], "45.00");
        assert_eq!(orders[0]["payment_action_label"], "支付全款");
    }
    assert_eq!(orders[0]["items"].as_array().unwrap().len(), 1);
    assert_eq!(orders[0]["items"][0]["product_name"], "红肉脐橙家庭装");
    // `Meta.ordering = ['-created_at']`；1002/1003 的 `created_at` 同秒，靠 id 定序。
    assert_eq!(orders[1]["order_number"], "ORD-DEMO-1002");
    assert_eq!(orders[2]["id"], ORDER_COMPLETED);
    assert_eq!(orders[2]["status_display"], "已完成");
    assert_eq!(orders[2]["amount_due"], "0.00");
    assert_eq!(orders[2]["payment_action_label"], "");
}

#[tokio::test]
async fn order_detail_and_404() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;

    let (_, body) = body_json(
        order_detail_impl(&pool, &buyer, ORDER_COMPLETED)
            .await
            .expect("应 200"),
    )
    .await;

    let order = &body["data"];
    assert_eq!(order["total_amount"], "39.90");
    assert_eq!(order["paid_amount"], "39.90");
    assert_eq!(order["items"][0]["product_name"], "赣南纽荷尔家庭装");
    assert_eq!(order["items"][0]["unit_price"], "39.90");
    assert_eq!(order["trace_packages"][0]["box_spec"], "5斤家庭装");
    assert_eq!(order["trace_packages"][0]["status_display"], "已签收");
    assert_eq!(order["payments"].as_array().unwrap().len(), 0);

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

    let (status, body) = body_json(
        order_pay_impl(&pool, &buyer, ORDER_COMPLETED)
            .await
            .expect("业务错误走 Ok(Response)"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], "当前订单状态不能支付");
    assert!(body["timestamp"].is_i64(), "{body}");

    let (status, body) = body_json(
        order_cancel_impl(&pool, &buyer, ORDER_COMPLETED)
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
    let stock_before: i32 = sqlx::query_scalar("SELECT stock FROM citrus_product WHERE id = $1")
        .bind(Uuid::parse_str(PRODUCT_XF_FAMILY).unwrap())
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
    .bind(Uuid::parse_str(PRODUCT_XF_FAMILY).unwrap())
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
        .bind(Uuid::parse_str(PRODUCT_XF_FAMILY).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stock_after, stock_before + 2);

    // 观测完把商品库存改回去 + 删掉临时数据。
    sqlx::query("UPDATE citrus_product SET stock = $2 WHERE id = $1")
        .bind(Uuid::parse_str(PRODUCT_XF_FAMILY).unwrap())
        .bind(stock_before)
        .execute(&pool)
        .await
        .unwrap();
    cleanup_buyer(&pool, buyer_id).await;
}

// --------------------------------------------------------------------------------------
// 售后
// --------------------------------------------------------------------------------------

#[tokio::test]
async fn after_sales_list_matches_fixture() {
    let pool = pool_or_skip!();
    let buyer = bearer(&pool, SEED_BUYER, SEED_BUYER_PASSWORD).await;

    let (_, body) = body_json(
        after_sale_impl(&pool, &buyer, &json!({}), true)
            .await
            .expect("应 200"),
    )
    .await;

    let items = body["data"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["issue_type"], "logistics");
    assert_eq!(items[0]["issue_type_display"], "物流异常");
    assert_eq!(items[0]["status_display"], "已解决");
    assert_eq!(items[0]["refund_amount"], "0.00");
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
