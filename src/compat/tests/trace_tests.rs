//! 果园与溯源域的契约测试。
//!
//! 两层结构：
//!
//! 1. **哈希链逐字节对齐**（不需要数据库）：拿夹具里录到的（**含微秒**的）字段值重算
//!    `evidence_hash`，必须与 Django 当年写进库的十六进制串逐字节相等。这是本域风险最高的
//!    回归闸门——canonical JSON 的任何一个细节（键名大小写 / `sort_keys` / `separators` /
//!    `ensure_ascii` / `occurredAt` 的 `isoformat()`）写错，这里立刻红。
//! 2. **链自洽**（需要 `compat_*` scratch schema）：走真实写入路径追加两条事件后，
//!    `evidence_hash` 必须能被「从库里读回再重算」验证通过（`chainValid=true`），
//!    证明哈希输入没有被动过手脚——尤其是 jsonb 往返（数字归一化、键重排）没有污染它。
//!
//! 跑法（先建 schema，见 `W1_BRIEF.md` §6）：
//! ```text
//! $env:COMPAT_TEST_SCHEMA='compat_w1b'
//! cargo test -- compat
//! ```

use axum::{http::StatusCode, response::Response};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::{
    auth::AuthUser,
    views_trace::{
        HashInput, evidence_hash, farmer_products_impl, farmer_trace_event_impl,
        orchard_detail_impl, supply_batch_detail_impl,
    },
};

// --------------------------------------------------------------------------------------
// 1. 哈希链：夹具里的 golden 值
// --------------------------------------------------------------------------------------

/// 夹具 `traces_lookup_batch_ok` 的 6 条事件 × `CGJ-2026-XF-001`。
///
/// 字段值逐字取自 `db/tests/fixtures/contract/orchard_trace.json`（`occurred_at` 是
/// `...Z` 形态，重算时要换成 Python `isoformat()` 的 `+00:00`）。
struct GoldenEvent {
    event_type: &'static str,
    source_type: &'static str,
    occurred_at: &'static str,
    title: &'static str,
    description: &'static str,
    location: &'static str,
    actor: &'static str,
    source_reference: &'static str,
    previous_hash: &'static str,
    evidence_hash: &'static str,
}

const GOLDEN_EVENTS: &[GoldenEvent] = &[
    GoldenEvent {
        event_type: "orchard",
        source_type: "operator",
        occurred_at: "2026-08-11T14:20:37.928176Z",
        title: "合作果园完成建档核验",
        description: "核验合作果园主体、位置、种植品种和本批次供货意向。",
        location: "江西省赣州市信丰县",
        actor: "橙管家运营",
        source_reference: "DEMO-CGJ-2026-XF-001-40",
        previous_hash: "",
        evidence_hash: "2b99fdb6e55587c404ff44fc61df26fa582622bb04978cda69d6a565024b6c6b",
    },
    GoldenEvent {
        event_type: "environment",
        source_type: "sensor",
        occurred_at: "2026-09-10T14:20:37.929180Z",
        title: "果园环境记录",
        description: "节点记录当前果园温湿度。",
        location: "江西省赣州市信丰县",
        actor: "果园节点",
        source_reference: "DEMO-CGJ-2026-XF-001-10",
        previous_hash: "2b99fdb6e55587c404ff44fc61df26fa582622bb04978cda69d6a565024b6c6b",
        evidence_hash: "b6e4f826119942c003ad929bb2c3210073586460e6ada8390b4c906eedafb697",
    },
    GoldenEvent {
        event_type: "harvest",
        source_type: "farmer",
        occurred_at: "2026-09-14T14:20:37.930176Z",
        title: "成熟采摘",
        description: "第一批脐橙达到采摘标准，开始人工轻采。",
        location: "A区示范地块",
        actor: "李师傅",
        source_reference: "DEMO-CGJ-2026-XF-001-6",
        previous_hash: "b6e4f826119942c003ad929bb2c3210073586460e6ada8390b4c906eedafb697",
        evidence_hash: "6f62dd18fda3b06950890bccec6cbf1a4fbb3e054479973f077b1128dbc06238",
    },
    GoldenEvent {
        event_type: "sorting",
        source_type: "operator",
        occurred_at: "2026-09-15T14:20:37.932209Z",
        title: "分选称重",
        description: "按果径与糖度分选，剔除不合格果。",
        location: "果园分选区",
        actor: "分选员",
        source_reference: "DEMO-CGJ-2026-XF-001-5",
        previous_hash: "6f62dd18fda3b06950890bccec6cbf1a4fbb3e054479973f077b1128dbc06238",
        evidence_hash: "5472d845ba7b79bb8013297c0e669de3d0ed0ad335513c06bcfd38170285b793",
    },
    GoldenEvent {
        event_type: "packing",
        source_type: "operator",
        occurred_at: "2026-09-17T14:20:37.933177Z",
        title: "装箱赋码",
        description: "按规格装箱并赋追溯箱码。",
        location: "产地包装区",
        actor: "包装员",
        source_reference: "DEMO-CGJ-2026-XF-001-3",
        previous_hash: "5472d845ba7b79bb8013297c0e669de3d0ed0ad335513c06bcfd38170285b793",
        evidence_hash: "6fe79125092d355237e31d0e2a42074847721ed6f0cd836dfadf46fbb518f0d2",
    },
    GoldenEvent {
        event_type: "shipping",
        source_type: "logistics",
        occurred_at: "2026-09-18T14:20:37.934176Z",
        title: "产地发货",
        description: "订单装车产地直发。",
        location: "产地发货区",
        actor: "物流",
        source_reference: "DEMO-CGJ-2026-XF-001-2",
        previous_hash: "5472d845ba7b79bb8013297c0e669de3d0ed0ad335513c06bcfd38170285b793",
        evidence_hash: "ed0ab10a7d3fe952e422d25346f2c5fed6a61d560746f4a200a64f5c798478cc",
    },
];

/// `Z` 形态 → Python `isoformat()` 的 `+00:00` 形态。
fn iso_of(value: &str) -> DateTime<Utc> {
    value.parse::<DateTime<Utc>>().expect("夹具时间是合法 RFC3339")
}

#[test]
fn seed_event_hashes_match_django_byte_for_byte() {
    let empty_images = json!([]);
    let empty_data = json!({});

    for event in GOLDEN_EVENTS {
        let computed = evidence_hash(
            "CGJ-2026-XF-001",
            &HashInput {
                event_type: event.event_type,
                source_type: event.source_type,
                occurred_at: iso_of(event.occurred_at),
                title: event.title,
                description: event.description,
                location: event.location,
                actor: event.actor,
                source_reference: event.source_reference,
                image_urls: &empty_images,
                data: &empty_data,
                previous_hash: event.previous_hash,
            },
        );

        assert_eq!(
            computed, event.evidence_hash,
            "事件「{}」的 evidence_hash 与 Django 不一致",
            event.title
        );
        // 夹具里的 hash_short = evidence_hash[:12].upper()
        assert_eq!(
            computed[..12].to_uppercase(),
            event.evidence_hash[..12].to_uppercase()
        );
    }
}

/// `occurredAt` 走 `isoformat()`：微秒为 0 时**不带**小数部分。
#[test]
fn hash_uses_isoformat_without_fraction_when_micros_are_zero() {
    let images = json!([]);
    let data = json!({});
    let whole = evidence_hash(
        "B",
        &HashInput {
            event_type: "harvest",
            source_type: "farmer",
            occurred_at: "2026-09-12T02:30:00Z".parse::<DateTime<Utc>>().unwrap(),
            title: "t",
            description: "",
            location: "",
            actor: "a",
            source_reference: "",
            image_urls: &images,
            data: &data,
            previous_hash: "",
        },
    );
    let micro = evidence_hash(
        "B",
        &HashInput {
            event_type: "harvest",
            source_type: "farmer",
            occurred_at: "2026-09-12T02:30:00.000001Z"
                .parse::<DateTime<Utc>>()
                .unwrap(),
            title: "t",
            description: "",
            location: "",
            actor: "a",
            source_reference: "",
            image_urls: &images,
            data: &data,
            previous_hash: "",
        },
    );

    assert_ne!(whole, micro);
}

// --------------------------------------------------------------------------------------
// 2. 需要 scratch schema 的回归
// --------------------------------------------------------------------------------------

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读取响应体失败");
    serde_json::from_slice(&bytes).expect("响应体不是 JSON")
}

fn short_id() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_uppercase()
}

async fn seed_farmer(pool: &PgPool) -> AuthUser {
    let id = Uuid::new_v4();
    let username = format!("w1b_{}", short_id().to_lowercase());
    let now = Utc::now();
    sqlx::query(
        r#"INSERT INTO "user" (id, username, password, email, role, created_at, updated_at)
           VALUES ($1, $2, '', $3, 'farmer', $4, $4)"#,
    )
    .bind(id)
    .bind(&username)
    .bind(format!("{username}@example.com"))
    .bind(now)
    .execute(pool)
    .await
    .expect("建测试果农失败");

    AuthUser {
        id,
        username,
        email: None,
        role: "farmer".to_string(),
        orchard_address: None,
        latitude: None,
        longitude: None,
        created_at: now,
        token_key: id,
    }
}

async fn seed_orchard(pool: &PgPool, owner: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    let now = Utc::now();
    sqlx::query(
        r#"INSERT INTO orchard (id, code, owner_id, name, grower_name, county, status,
                                created_at, updated_at)
           VALUES ($1, $2, $3, 'W1B 测试果园', 'w1b-tester', '信丰县', 'verified', $4, $4)"#,
    )
    .bind(id)
    .bind(format!("GY-W1B-{}", short_id()))
    .bind(owner)
    .bind(now)
    .execute(pool)
    .await
    .expect("建测试果园失败");
    id
}

async fn seed_batch(pool: &PgPool, orchard_id: Uuid, planned: i32) -> Uuid {
    let id = Uuid::new_v4();
    let now = Utc::now();
    sqlx::query(
        r#"INSERT INTO sales_batch (id, code, trace_code, orchard_id, title, status,
                                    planned_quantity, created_at, updated_at)
           VALUES ($1, $2, $3, $4, 'W1B 测试批次', 'open', $5, $6, $6)"#,
    )
    .bind(id)
    .bind(format!("CGJ-W1B-{}", short_id()))
    .bind(format!("CGJ-W1BTRACE-{}", short_id()))
    .bind(orchard_id)
    .bind(planned)
    .bind(now)
    .execute(pool)
    .await
    .expect("建测试批次失败");
    id
}

/// 真实写入路径 → 链必须自洽（`chainValid=true`）。
///
/// 这条测试覆盖「哈希输入不能被 jsonb 往返污染」：两条事件的 `data` 分别是整数与浮点数
/// （`1e2` 会被 PG 的 jsonb 归一化成 `100.0`），写入后再从库里读回来重算，必须得到同一个哈希。
/// 用**单键**对象是刻意的——多键对象的键序会被 jsonb 重排，那是蓝本自身的另一处行为，
/// 见 [`multi_key_data_breaks_chain_the_same_way_as_blueprint`]。
#[tokio::test]
async fn appended_events_form_a_verifiable_chain() {
    let pool = super::support::pool().await;
    let user = seed_farmer(&pool).await;
    let orchard_id = seed_orchard(&pool, user.id).await;
    let batch_id = seed_batch(&pool, orchard_id, 100).await;

    let first = farmer_trace_event_impl(
        &pool,
        &user,
        batch_id,
        &json!({
            "event_type": "quality",
            "source_type": "farmer",
            "occurred_at": "2026-09-12T03:00:00+00:00",
            "title": "链自洽 #1",
            "description": "第一条",
            "location": "果园分选区",
            "actor": "qa",
            "source_reference": "QA-1",
            "image_urls": [],
            "data": {"sampleSize": 12},
        }),
    )
    .await
    .expect("第一条事件应写入成功");
    let first_body = body_json(first).await;
    let first_hash = first_body["data"]["evidence_hash"]
        .as_str()
        .expect("evidence_hash 是字符串")
        .to_string();

    assert_eq!(first_body["data"]["previous_hash"], "");
    assert_eq!(
        first_body["data"]["hash_short"],
        first_hash[..12].to_uppercase(),
        "hash_short = evidence_hash[:12].upper()"
    );
    assert_eq!(first_body["data"]["occurred_at"], "2026-09-12T03:00:00Z");
    assert_eq!(first_body["data"]["actor"], user.username);

    // recorded_at 是 `ORDER BY recorded_at` 的排序键，拉开一点避免同微秒。
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let second = farmer_trace_event_impl(
        &pool,
        &user,
        batch_id,
        &json!({
            "event_type": "quality",
            "occurred_at": "2026-09-12T04:00:00+00:00",
            "title": "链自洽 #2",
            "data": {"ratio": 1e2},
        }),
    )
    .await
    .expect("第二条事件应写入成功");
    let second_body = body_json(second).await;
    let second_hash = second_body["data"]["evidence_hash"]
        .as_str()
        .expect("evidence_hash 是字符串")
        .to_string();

    assert_eq!(
        second_body["data"]["previous_hash"], first_hash,
        "第二条事件的 previous_hash 必须是上一条的 evidence_hash"
    );
    assert_ne!(first_hash, second_hash);
    // 请求里没给的字段走序列化器默认值（`TraceEventCreateSerializer`）。
    assert_eq!(second_body["data"]["description"], "");
    assert_eq!(second_body["data"]["image_urls"], json!([]));

    let detail = body_json(
        supply_batch_detail_impl(&pool, batch_id)
            .await
            .expect("批次详情应返回成功体"),
    )
    .await;
    assert_eq!(detail["data"]["integrity"]["chainValid"], true);
    assert_eq!(detail["data"]["integrity"]["eventCount"], 2);
    // 批次详情的 integrity 只有 chainValid/eventCount（`latestHash` 只在 traces 视图里）
    assert!(detail["data"]["integrity"].get("latestHash").is_none());
    assert_eq!(detail["data"]["traceEvents"][0]["previous_hash"], "");
    assert_eq!(detail["data"]["traceEvents"][1]["previous_hash"], first_hash);
    // 事件按 Meta.ordering = ['occurred_at', 'recorded_at', 'id'] 升序
    assert_eq!(
        detail["data"]["traceEvents"][0]["occurred_at"],
        "2026-09-12T03:00:00Z"
    );
}

/// 蓝本自身的瑕疵：`data` 是多键对象时 `chainValid` 恒为 false，**逐字复刻**。
///
/// `_hash_value()` 用的是**写库前**的 Python dict（键序 = 请求体），而 `verify_chain`
/// 重算时用的是**读回来**的 JSONField（PG 的 jsonb 会把键按「长度 + 字节序」重排）——
/// 两次 `json.dumps` 的字符串不同，哈希自然对不上。用 PG 作后端的 Django 就是这个行为
/// （录制时用的 sqlite 不会重排，所以夹具里没有这种情况；但夹具里 `XF` 批次的
/// `chainValid` 也是 false，两条路径的观测值一致）。
///
/// 这里刻意断言 `false`：谁哪天「顺手修好」它，这条测试就会红。
#[tokio::test]
async fn multi_key_data_breaks_chain_the_same_way_as_blueprint() {
    let pool = super::support::pool().await;
    let user = seed_farmer(&pool).await;
    let orchard_id = seed_orchard(&pool, user.id).await;
    let batch_id = seed_batch(&pool, orchard_id, 100).await;

    let response = farmer_trace_event_impl(
        &pool,
        &user,
        batch_id,
        &json!({
            "event_type": "quality",
            "occurred_at": "2026-09-12T06:00:00+00:00",
            "title": "多键 data",
            // 请求体顺序 zz → a；jsonb 读回来会重排成 a → zz。
            "data": {"zz": 1, "a": 2},
        }),
    )
    .await
    .expect("事件应写入成功");
    let created = body_json(response).await;
    assert_eq!(created["data"]["data"], json!({"zz": 1, "a": 2}));

    let detail = body_json(
        supply_batch_detail_impl(&pool, batch_id)
            .await
            .expect("批次详情应返回成功体"),
    )
    .await;
    assert_eq!(detail["data"]["integrity"]["chainValid"], false);
    // 回读时键序已按 jsonb 归一化（键长优先）
    assert_eq!(
        detail["data"]["traceEvents"][0]["data"],
        json!({"a": 2, "zz": 1})
    );
}

/// 商品列表的确定性排序：`created_at` 撞在同一毫秒时必须靠 `id ASC` 兜底。
#[tokio::test]
async fn product_order_is_deterministic_when_created_at_ties() {
    let pool = super::support::pool().await;
    let user = seed_farmer(&pool).await;
    let orchard_id = seed_orchard(&pool, user.id).await;
    let batch_id = seed_batch(&pool, orchard_id, 100).await;

    let same_instant: DateTime<Utc> = "2026-09-20T14:20:37.927Z".parse().unwrap();
    let mut ids = vec![Uuid::new_v4(), Uuid::new_v4()];
    ids.sort();

    for (index, id) in ids.iter().enumerate() {
        sqlx::query(
            r#"INSERT INTO citrus_product
                   (id, seller_id, sales_batch_id, name, origin, price, stock, sweetness,
                    status, created_at, updated_at)
               VALUES ($1, $2, $3, $4, '江西赣州信丰县', 19.90, 10, 12.5, 'on_sale', $5, $5)"#,
        )
        .bind(id)
        .bind(user.id)
        .bind(batch_id)
        .bind(format!("W1B 商品 {}", index + 1))
        .bind(same_instant)
        .execute(&pool)
        .await
        .expect("建测试商品失败");
    }

    let body = body_json(
        farmer_products_impl(&pool, &user)
            .await
            .expect("商品列表应返回成功体"),
    )
    .await;
    let items = body["data"].as_array().expect("data 是数组");

    assert_eq!(items.len(), 2);
    // 与 `PRODUCT_ORDER` 一致：sort_order ASC, created_at DESC, id ASC
    assert_eq!(items[0]["id"], ids[0].to_string());
    assert_eq!(items[1]["id"], ids[1].to_string());
    // DecimalField → 字符串；FloatField 才输出数字
    assert_eq!(items[0]["price"], "19.90");
    assert_eq!(items[0]["sweetness"], "12.5");
    assert_eq!(items[0]["sku_type_display"], "家庭装");
    assert_eq!(items[0]["is_available"], true);
    assert_eq!(items[0]["sales_batch"]["available_quantity"], 100);
    assert_eq!(items[0]["sales_batch"]["status_display"], "在售");
}

/// 视图层 404 走**成功体形状**（蓝本 `api_response(None, msg, 404)` → 带 `timestamp`）。
///
/// 与鉴权类 401/403/405 的 DRF 异常体（**无** `timestamp`）是两条不同的路径，
/// 夹具 `orchard_detail_unknown_404` 记的就是这一条。
#[tokio::test]
async fn unknown_orchard_returns_success_envelope_with_404() {
    let pool = super::support::pool().await;
    let response = orchard_detail_impl(&pool, Uuid::new_v4(), None)
        .await
        .expect_err("未知果园应 404");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    assert_eq!(body["code"], 404);
    assert_eq!(body["message"], "果园不存在或尚未通过认证");
    assert_eq!(body["data"], Value::Null);
    assert!(body["timestamp"].is_i64(), "视图层 4xx 带 timestamp：{body}");
}
