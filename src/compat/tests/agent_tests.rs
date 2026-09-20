//! 智能体域的契约测试。
//!
//! 分两层：
//! 1. **纯规则层**（不触库）：意图抽取、询价解析、摘要文案、choices 显示值。
//!    这些是 `agent.json` 里逐字比对的部分，改了必须让这里先红。
//! 2. **DB 冒烟**：打 scratch schema（`COMPAT_TEST_SCHEMA`，默认 `compat_test`），
//!    确认 SQL 与行解析能跑通。精确的逐字对齐靠 `scripts/replay_diff.py --domain agent`。

use serde_json::{Value, json};

use super::super::agent_service;

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

// --------------------------------------------------------------------------------------
// 意图抽取（`detect_intent`）
// --------------------------------------------------------------------------------------

/// 夹具 `agent_select_ok_public` 的入参：只有预算/用途/规格三项命中，且键序一致。
#[test]
fn detect_intent_matches_fixture_for_select_case() {
    let intent = agent_service::detect_intent("预算80元以内，5斤装自用");
    let keys: Vec<&str> = intent.keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["max_price", "purpose", "unit_kw"], "{intent:?}");
    assert_eq!(intent["max_price"], json!(80.0));
    assert_eq!(intent["purpose"], json!("family"));
    assert_eq!(intent["unit_kw"], json!("5斤"));
}

#[test]
fn detect_intent_keeps_sku_type_and_sweetness() {
    let intent = agent_service::detect_intent("企业团购100箱，要甜的");
    assert_eq!(intent["sku_type"], json!("enterprise"));
    assert_eq!(intent["sweet"], json!(true));

    // 送礼命中 gift；「自用」后写会覆盖 purpose（蓝本同样是后写覆盖）。
    let intent = agent_service::detect_intent("送人还是自用");
    assert_eq!(intent["purpose"], json!("family"));
}

/// 预算扫描要按 Python `re.search` 的最左优先语义：`80.` 不构成匹配，但后面的 `5元` 算。
#[test]
fn budget_scan_is_leftmost_match() {
    let intent = agent_service::detect_intent("不超过80.元");
    assert!(intent.get("max_price").is_none(), "{intent:?}");

    let intent = agent_service::detect_intent("预算 88 元");
    assert_eq!(intent["max_price"], json!(88.0));
}

// --------------------------------------------------------------------------------------
// 询价解析（`parse_inquiry`）
// --------------------------------------------------------------------------------------

/// 夹具 `agent_inquiry_ok_public` 的入参与期望 `requirements` 完全一致。
#[test]
fn parse_inquiry_matches_fixture_requirements() {
    let requirement = agent_service::parse_inquiry("采购100箱，5斤装，2027-01-01 前到货");
    assert_eq!(
        Value::Object(requirement),
        json!({"quantity": 100, "spec": "5斤", "deadline": "2027-01-01"})
    );
}

#[test]
fn parse_inquiry_supports_chinese_deadline_and_pieces() {
    let requirement = agent_service::parse_inquiry("要200件，8月20日前到");
    assert_eq!(
        Value::Object(requirement),
        json!({"quantity": 200, "deadline": "2026-08-20"})
    );
}

/// ISO 分支命中但日期非法 → 蓝本 `pass`，**不再**回头看中文分支。
#[test]
fn parse_inquiry_stops_on_invalid_iso_date() {
    let requirement = agent_service::parse_inquiry("2026-02-31 到货");
    assert!(requirement.get("deadline").is_none(), "{requirement:?}");
}

// --------------------------------------------------------------------------------------
// 统一入口（`detect_agent_intent`）
// --------------------------------------------------------------------------------------

#[test]
fn router_intent_matches_fixture_chat_case() {
    assert_eq!(
        agent_service::detect_agent_intent("今天经营怎么样"),
        "report"
    );
    assert_eq!(agent_service::detect_agent_intent("有什么生产风险"), "risk");
    assert_eq!(
        agent_service::detect_agent_intent("推荐5斤装送人"),
        "select"
    );
    assert_eq!(agent_service::detect_agent_intent("采购100箱"), "inquiry");
    assert_eq!(
        agent_service::detect_agent_intent("有复购提醒吗"),
        "repurchase"
    );
    assert_eq!(
        agent_service::detect_agent_intent("我的果园档案"),
        "context"
    );
    assert_eq!(agent_service::detect_agent_intent("你好"), "unknown");
}

// --------------------------------------------------------------------------------------
// 规则摘要文案
// --------------------------------------------------------------------------------------

/// 夹具 `agent_chat_report_ok` 的 `reply` 逐字。
#[test]
fn report_summary_matches_fixture_reply() {
    let data = json!({
        "role": "farmer",
        "report": {
            "overview": {
                "open_batch_count": 1,
                "today_order_count": 2,
                "today_order_amount": "79.80"
            },
            "suggestions": [
                "有未处理的风险任务（高风险 3 / 中风险 1），建议优先安排巡园与复检。",
                "近期存在 1 条异常识别记录（黄龙病/溃疡/沙皮等），请及时复核并生成对应农艺处理。"
            ]
        }
    });
    let reply = agent_service::summary_for("report", &data, "今天经营怎么样");
    assert_eq!(
        reply,
        "今日经营：在售批次 1、今日订单 2 单、金额 ¥79.80。有未处理的风险任务（高风险 3 / 中风险 1），建议优先安排巡园与复检。；近期存在 1 条异常识别记录（黄龙病/溃疡/沙皮等），请及时复核并生成对应农艺处理。"
    );
}

#[test]
fn inquiry_summary_uses_primary_cost() {
    let data = json!({
        "primary": {
            "batch": {"title": "信丰示范园·纽荷尔鲜果批次"},
            "cost": {"total_cost_estimate": "4024.00"}
        },
        "alternatives": [{"score": 6}, {"score": 6}]
    });
    let reply = agent_service::summary_for("inquiry", &data, "");
    assert_eq!(
        reply,
        "主方案：「信丰示范园·纽荷尔鲜果批次」预估总成本 ¥4024.00，另有 2 个备选。正式报价需运营确认后生成。"
    );
}

#[test]
fn unknown_intent_falls_back_to_help_text() {
    let reply = agent_service::summary_for("unknown", &json!({}), "你好");
    assert!(reply.starts_with("我能帮你查询经营日报、智能选品、采购询价、生产风险、复购提醒。"));
}

// --------------------------------------------------------------------------------------
// choices 显示值（逐字取自 `api/models.py`）
// --------------------------------------------------------------------------------------

#[test]
fn choice_display_values_match_models() {
    assert_eq!(agent_service::ticket_type_display("restock"), "品质抽检");
    assert_eq!(agent_service::ticket_type_display("quote"), "报价单");
    assert_eq!(
        agent_service::ticket_type_display("risk_action"),
        "风险处置"
    );
    assert_eq!(agent_service::ticket_type_display("other"), "其他");
    assert_eq!(agent_service::approval_status_display("approved"), "已批准");
    assert_eq!(agent_service::rating_display(5), "很好");
    assert_eq!(agent_service::rating_display(4), "好");
}

// --------------------------------------------------------------------------------------
// DB 冒烟（依赖 scratch schema）
// --------------------------------------------------------------------------------------

/// 三个读类端点在种子态上必须跑通（不校验具体值——那是回放比对器的职责）。
#[tokio::test]
async fn agent_read_endpoints_smoke_on_scratch_schema() {
    let pool = super::support::pool().await;
    let user = crate::compat::auth::AuthUser {
        id: uuid::Uuid::new_v4(),
        username: "smoke_user".to_string(),
        email: None,
        role: "farmer".to_string(),
        orchard_address: None,
        latitude: None,
        longitude: None,
        created_at: chrono::Utc::now(),
        token_key: uuid::Uuid::new_v4(),
    };

    let context = agent_service::build_context(&pool, &user)
        .await
        .expect("build_context 失败");
    assert_eq!(text(&context["role"]), "farmer");
    assert!(context["orchards"].is_array());
    assert!(context["batches"].is_array());

    let report = agent_service::daily_report(&pool, &user)
        .await
        .expect("daily_report 失败");
    assert!(report["report"].is_object());

    let selection = agent_service::select_for_shopper(&pool, "预算80元以内，5斤装自用")
        .await
        .expect("select_for_shopper 失败");
    assert!(selection["recommendations"].is_array());
    assert!(selection["needs_more"].is_array());

    let inquiry = agent_service::inquiry_quote(&pool, "采购100箱，5斤装")
        .await
        .expect("inquiry_quote 失败");
    assert!(inquiry.get("requirements").is_some());

    let risk = agent_service::risk_alert(&pool, &user)
        .await
        .expect("risk_alert 失败");
    assert!(risk["has_alert"].is_boolean());
}
