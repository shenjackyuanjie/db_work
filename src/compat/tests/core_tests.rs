//! 农事与识别域的契约回放测试。
//!
//! 分三层：
//!
//! 1. **纯函数层**（不连库）：风险分级规则、银行家舍入、`HH:MM` 归一、词典查表、ISO 解析；
//! 2. **DB 层**（打 `COMPAT_TEST_SCHEMA` 指定的 scratch schema）：鉴权三态、任务增查改、
//!    识别记录读取、温湿度写入；
//! 3. **契约形状层**：键序与时间后缀形态（`Z` vs `+00:00`，D4）。
//!
//! 铁律：只碰 `compat_*` scratch schema；用例自己建的行走 `cleanup_*` 收尾，
//! 免得污染后续回放（`tasks_list_ok` 依赖种子任务集合）。

use super::support;
use crate::compat::views_core::*;

use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use chrono::{TimeZone, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

// --------------------------------------------------------------------------------------
// 辅助
// --------------------------------------------------------------------------------------

/// 回放工具 `replay_diff.py::token_for` 的固定 token：
/// `uuid5(NAMESPACE_URL, "contract:<key>")`。`uuid` crate 只开了 `v4`，
/// 所以这里直接钉住录制当时算出来的值（不要跟着改）。
const TOKEN_FARMER_XINFENG: &str = "74313af4-555d-5e30-b63e-fc594355f4c9";
const TOKEN_BUYER_ZHANG: &str = "c8e3da7a-65d6-54c3-80a9-52fbece1eb67";

fn auth_headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    headers
}

async fn body_json(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn pool_or_skip() -> PgPool {
    support::pool().await
}

/// 返回 `true` 表示 scratch schema 里没有可用的固定 token（种子没灌），此时只能测纯函数。
async fn farmer_headers(pool: &PgPool) -> Option<HeaderMap> {
    let token = Uuid::parse_str(TOKEN_FARMER_XINFENG).unwrap();
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM auth_token WHERE key = $1)")
            .bind(token)
            .fetch_one(pool)
            .await
            .unwrap_or(false);

    if exists {
        Some(auth_headers(TOKEN_FARMER_XINFENG))
    } else {
        eprintln!("跳过：scratch schema 里没有 farmer_xinfeng 的固定 token，先跑 load_seed.py");
        None
    }
}

// --------------------------------------------------------------------------------------
// 纯函数层
// --------------------------------------------------------------------------------------

/// 夹具 `tasks_generate_environment_shape_only`：27.5 / 88.0 → 高风险。
#[test]
fn environment_risk_replicates_django_rules() {
    let (risk, description) = classify_environment_risk(27.5, 88.0);
    assert_eq!(risk, "高风险");
    assert_eq!(
        description,
        "加强果园巡查，每7-10天喷施木虱防治药剂（如噻虫嗪、高效氯氟氰菊酯）；发现病树立即标记并挖除，防止传播。"
    );

    // 中风险：温度落在 (32,35]，湿度落在 [60,80]。
    assert_eq!(classify_environment_risk(34.0, 70.0).0, "中风险");
    assert_eq!(classify_environment_risk(20.0, 70.0).0, "中风险");
    // 低风险：都不中。
    assert_eq!(classify_environment_risk(10.0, 30.0).0, "低风险");
    assert_eq!(classify_environment_risk(40.0, 90.0).0, "低风险");
    // 边界是**闭区间**：22/32 与 80/100 都算高风险。
    assert_eq!(classify_environment_risk(22.0, 80.0).0, "高风险");
    assert_eq!(classify_environment_risk(32.0, 100.0).0, "高风险");
    // 高风险优先：温度 32 同时落在中风险的第二段，湿度 80 同时落在中风险段。
    assert_eq!(classify_environment_risk(32.0, 80.0).0, "高风险");
}

#[test]
fn disease_treatment_dictionary_is_verbatim() {
    assert!(disease_treatment("黄龙病").starts_with("先杀虫，后砍树："));
    assert!(disease_treatment("沙皮病").starts_with("清园+药剂防治："));
    assert!(disease_treatment("溃疡病").starts_with("采用\"药-剪-药\"策略："));
    assert_eq!(disease_treatment("不存在的病害"), "暂无治理建议");
}

/// `round(26.25, 1)` 在 Python 里是 26.2（half-even），Rust 的 `f64::round` 会给 26.3。
#[test]
fn python_round_is_half_even() {
    assert_eq!(round_half_even(26.25, 1), 26.2);
    assert_eq!(round_half_even(26.35, 1), 26.4);
    assert_eq!(round_half_even(27.5, 1), 27.5);
    assert_eq!(round_half_even(25.0, 1), 25.0);
    assert_eq!(round_half_even(92.375, 2), 92.38);
    assert_eq!(round_half_even(92.385, 2), 92.38);
}

#[test]
fn csv_timestamp_labels_are_zero_padded() {
    assert_eq!(normalize_hh_mm("9:5"), "09:05");
    assert_eq!(normalize_hh_mm("12:20"), "12:20");
    assert_eq!(normalize_hh_mm("not-a-time"), "not-a-time");
}

#[test]
fn history_label_follows_django_alternating_rule() {
    let base = Utc.with_ymd_and_hms(2026, 9, 11, 12, 20, 37).unwrap();
    // index % 2 == 0 或最后一格才带标签。
    assert_eq!(history_label(0, 10, base), "12:20");
    assert_eq!(history_label(1, 10, base), "");
    assert_eq!(history_label(2, 10, base), "12:20");
    assert_eq!(history_label(9, 10, base), "12:20");
    // 奇数长度时，最后一个是奇数下标也要带标签。
    assert_eq!(history_label(5, 6, base), "12:20");
}

#[test]
fn iso8601_parser_accepts_drf_shapes() {
    assert!(parse_iso8601("2026-09-13T10:20:30.123456+00:00").is_some());
    assert!(parse_iso8601("2026-09-13T10:20:30Z").is_some());
    assert!(parse_iso8601("2026-09-13T10:20:30").is_some());
    assert!(parse_iso8601("not-a-time").is_none());
    assert!(parse_iso8601("2026-09-13").is_none());
}

/// 惰性创建的三份 payload 必须与蓝本写死的值逐字一致。
#[test]
fn lazy_default_payloads_match_blueprint() {
    let home = default_home_payload();
    assert_eq!(home["weatherCondition"], "阴");
    assert_eq!(home["temperatureRange"], "20℃ - 25℃");
    assert_eq!(home["suggestion"], "建议：保持正常天气，注意防晒。");
    assert_eq!(home["healthScore"], 80);
    assert_eq!(home["weeklyAlerts"], 2);
    assert_eq!(home["pendingTasks"], 10);
    assert_eq!(home["growthRate"], 85.5);
    assert_eq!(home["DiagnosisStatus"], "正常");
    assert_eq!(home["GrowthStatus"], "涨果期");

    // 键序 = 蓝本 dict 字面量顺序（`preserve_order` 兜着）。
    let rendered = serde_json::to_string(&home).unwrap();
    let mut last = 0;
    for key in [
        "weatherCondition",
        "temperatureRange",
        "suggestion",
        "healthScore",
        "weeklyAlerts",
        "pendingTasks",
        "growthRate",
        "DiagnosisStatus",
        "GrowthStatus",
    ] {
        let at = rendered.find(&format!(r#""{key}""#)).unwrap();
        assert!(last < at, "{key} 的位置不对：{rendered}");
        last = at;
    }

    let growth = default_growth_tracking_payload();
    assert_eq!(growth["growthStageText"], "涨果期");
    assert_eq!(growth["growthStageDuration"], "30");
    assert_eq!(growth["startDate"], "2025-12-01");
    assert_eq!(growth["youngFruitEndDate"], "2026-05-30");
    assert_eq!(growth["diameter"], 7.2);
    assert_eq!(growth["ratio"], 1.2);
}

/// 温湿度响应：`+00:00` 后缀（DRF `DateTimeField`）、标签交替、`[:10]` 截断。
#[test]
fn temperature_payload_uses_offset_suffix_and_alternating_labels() {
    let base = Utc.with_ymd_and_hms(2026, 9, 11, 12, 20, 37).unwrap();
    let records: Vec<TempRecord> = (0..10)
        .map(|index| TempRecord {
            timestamp: base + chrono::Duration::days(index),
            temperature: 25.0 + index as f64 * 0.1,
            humidity: 65.0,
            node_id: "0x4a80".to_string(),
        })
        .collect();

    let payload = build_records_payload(&records);
    let history = payload["temperatureHistory"].as_array().unwrap();
    assert_eq!(history.len(), 10);
    assert_eq!(history[0]["timestamp"], "12:20");
    assert_eq!(history[1]["timestamp"], "");
    assert_eq!(history[9]["timestamp"], "12:20");

    let recent = payload["recentRecords"].as_array().unwrap();
    let record_time = recent[0]["record_time"].as_str().unwrap();
    assert!(record_time.ends_with("+00:00"), "{record_time}");
    assert!(!record_time.ends_with('Z'), "{record_time}");
    assert_eq!(recent[0]["tag_serial_number"], "0x4a80");

    assert_eq!(payload["currentTemperature"], history[9]["value"]);
    assert_eq!(payload["currentHumidity"], 65.0);
}

/// CSV 兜底分支**没有** `recentRecords` 键。
#[test]
fn csv_payload_has_no_recent_records() {
    let rows: Vec<Vec<String>> = ["09:5", "12:20", "13:40"]
        .iter()
        .enumerate()
        .map(|(index, time)| {
            vec![
                time.to_string(),
                format!("{}", 24 + index),
                "70".to_string(),
            ]
        })
        .collect();

    let payload = build_csv_payload(&rows);
    assert!(payload.get("recentRecords").is_none(), "{payload}");
    assert_eq!(payload["temperatureHistory"][0]["timestamp"], "09:05");
    assert_eq!(payload["temperatureHistory"][2]["timestamp"], "13:40");
    assert_eq!(payload["currentTemperature"], 26.0);
}

/// 空 `ImageField` → `''`（`DiseaseRecognitionRecordSerializer.get_imagePath`）。
#[test]
fn empty_image_field_renders_empty_string() {
    assert_eq!(image_path(""), "");
    assert_eq!(image_path("   "), "");
    assert_eq!(
        image_path("recognition_records/a.jpg"),
        "recognition_records/a.jpg"
    );
}

#[test]
fn router_builds_without_state() {
    let _ = router();
}

// --------------------------------------------------------------------------------------
// DB 层：鉴权与只读
// --------------------------------------------------------------------------------------

#[tokio::test]
async fn missing_token_is_401_for_every_guarded_endpoint() {
    let pool = pool_or_skip().await;
    let headers = HeaderMap::new();

    let cases = [
        ("home", home_impl(&pool, &headers).await),
        ("growth", growth_tracking_impl(&pool, &headers).await),
        ("diagnose", diagnose_impl(&pool, &headers).await),
    ];
    for (name, result) in cases {
        let err = result.expect_err("无凭证必须走 Err 分支");
        let (status, body) = body_json(err.into_response()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{name}");
        assert_eq!(body["message"], "身份认证信息未提供。", "{name}");
        assert_eq!(body["data"], Value::Null, "{name}");
        assert!(body.get("timestamp").is_none(), "{name}: {body}");
    }

    // 温湿度 / 识别记录 / 任务 / 疾病建议 / generate 共用同一个守卫。
    let query = serde_json::Map::new();
    for (name, result) in [
        (
            "temperature",
            temperature_humidity_impl(&pool, &headers, &query).await,
        ),
        ("records", recognition_records_impl(&pool, &headers).await),
        ("tasks", tasks_impl(&pool, &headers).await),
        (
            "treatment",
            disease_treatment_impl(&pool, &headers, &query).await,
        ),
        ("generate", generate_impl(&pool, &headers).await),
    ] {
        let err = result.expect_err("无凭证必须走 Err 分支");
        let (status, body) = body_json(err.into_response()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{name}");
        assert_eq!(body["message"], "身份认证信息未提供。", "{name}");
    }
}

#[tokio::test]
async fn buyer_is_403_with_farmer_only_message() {
    let pool = pool_or_skip().await;
    // 买家 token 由回放工具安装；没装就跳过（离线也不红）。
    let token = Uuid::parse_str(TOKEN_BUYER_ZHANG).unwrap();
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM auth_token WHERE key = $1)")
            .bind(token)
            .fetch_one(&pool)
            .await
            .unwrap_or(false);
    if !exists {
        eprintln!("跳过：scratch schema 里没有 buyer_zhang 的固定 token");
        return;
    }

    let headers = auth_headers(TOKEN_BUYER_ZHANG);
    let err = home_impl(&pool, &headers).await.expect_err("买家必须 403");
    let (status, body) = body_json(err.into_response()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["message"], "该接口仅限果农使用");
    assert!(body.get("timestamp").is_none(), "{body}");
}

#[tokio::test]
async fn temperature_humidity_get_returns_seeded_series() {
    let pool = pool_or_skip().await;
    let Some(headers) = farmer_headers(&pool).await else {
        return;
    };

    let query = serde_json::Map::new();
    let response = temperature_humidity_impl(&pool, &headers, &query)
        .await
        .expect("读温湿度不该走 Err");
    let (status, body) = body_json(response).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 200);
    assert_eq!(body["message"], "success");
    assert_eq!(
        body["data"]["temperatureHistory"].as_array().unwrap().len(),
        10
    );
    assert_eq!(
        body["data"]["humidityHistory"].as_array().unwrap().len(),
        10
    );
    assert_eq!(body["data"]["recentRecords"].as_array().unwrap().len(), 10);
    // 升序：第一个是最早的那条。
    assert_eq!(body["data"]["temperatureHistory"][0]["value"], 26.2);
    assert_eq!(body["data"]["currentTemperature"], 23.5);
}

#[tokio::test]
async fn recognition_records_only_contain_own_seed_rows() {
    let pool = pool_or_skip().await;
    let Some(headers) = farmer_headers(&pool).await else {
        return;
    };

    let response = recognition_records_impl(&pool, &headers)
        .await
        .expect("读识别记录不该走 Err");
    let (status, body) = body_json(response).await;
    assert_eq!(status, StatusCode::OK);

    let records = body["data"]["records"].as_array().unwrap();
    // 夹具是 shape_only（值不比），但形状必须齐。
    assert!(!records.is_empty(), "{body}");
    for record in records {
        assert!(record["id"].is_string());
        assert_eq!(record["imagePath"], "");
        assert!(record["diseaseName"].is_string());
        assert!(record["area"].is_string());
        assert!(record["riskLevel"].is_string());
        assert!(record["recognitionDate"].is_string());
        assert!(record["confidence"].is_f64());
        // `created_at` 走 DRF JSONEncoder 的 `Z` 形态（D4）。
        let created_at = record["created_at"].as_str().unwrap();
        assert!(created_at.ends_with('Z'), "{created_at}");
    }
}

#[tokio::test]
async fn disease_treatment_missing_param_and_unknown_name() {
    let pool = pool_or_skip().await;
    let Some(headers) = farmer_headers(&pool).await else {
        return;
    };

    let empty = serde_json::Map::new();
    let (status, body) = body_json(
        disease_treatment_impl(&pool, &headers, &empty)
            .await
            .expect("校验失败是 Ok 分支（成功体形状）"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], "disease_name parameter is required");
    assert!(body["timestamp"].is_i64(), "{body}");

    let mut query = serde_json::Map::new();
    query.insert("disease_name".into(), json!("不存在的病害"));
    let (status, body) = body_json(
        disease_treatment_impl(&pool, &headers, &query)
            .await
            .expect("词典未命中不是错误"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["treatment"], "暂无治理建议");
}

#[tokio::test]
async fn generate_returns_fixed_advice_text() {
    let pool = pool_or_skip().await;
    let Some(headers) = farmer_headers(&pool).await else {
        return;
    };

    let (status, body) = body_json(
        generate_impl(&pool, &headers)
            .await
            .expect("generate 不该走 Err"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["textii"], FERTILIZATION_TEXT_EXPECTED);
}

/// 夹具里那段中文建议的**开头与结尾**（整串过长，只钉首尾 + 长度特征）。
const FERTILIZATION_TEXT_EXPECTED: &str = "针对赣南脐橙，建议实施\"测土配方、分期精准\"的施肥策略。基肥以腐熟有机肥（如羊粪、饼肥）为主，秋季深施，改良酸性红壤。追肥分三次：春梢期以高氮复合肥促梢保花；壮果期增施钾肥（如硫酸钾），配施磷与中微量元素，提升糖度与果皮光泽；采果前补施速效肥恢复树势。全年注重叶片营养诊断，结合土壤检测结果灵活调整，确保氮、磷、钾与钙、镁、硼等元素平衡，避免偏施氮肥。坚持生草栽培，保墒增肥。";

// --------------------------------------------------------------------------------------
// DB 层：任务（建 → 列 → 完成），用例自带清理
// --------------------------------------------------------------------------------------

#[tokio::test]
async fn task_lifecycle_round_trip() {
    let pool = pool_or_skip().await;
    let Some(headers) = farmer_headers(&pool).await else {
        return;
    };

    // 缺 title / description → 400 且 message 是对象、键序 = 字段声明序。
    let (status, body) = body_json(
        add_task_impl(&pool, &headers, &json!({}))
            .await
            .expect("校验失败是 Ok 分支（成功体形状）"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"]["title"][0], "该字段是必填项。");
    assert_eq!(body["message"]["description"][0], "该字段是必填项。");
    assert!(body["timestamp"].is_i64(), "{body}");
    let rendered = serde_json::to_string(&body).unwrap();
    assert!(
        rendered.find(r#""title""#).unwrap() < rendered.find(r#""description""#).unwrap(),
        "{rendered}"
    );

    // 建任务 → 200 + `Task created successfully`。
    let (status, created) = body_json(
        add_task_impl(
            &pool,
            &headers,
            &json!({
                "title": "QA 巡园记录",
                "description": "契约基准用例生成的巡园任务",
                "risk_level": "低风险",
                "task_type": "日常巡园",
                "source": "用户",
            }),
        )
        .await
        .expect("建任务成功是 Ok 分支"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["message"], "Task created successfully");
    let task_id = created["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["data"]["is_completed"], false);
    assert_eq!(created["data"]["completed_at"], Value::Null);
    assert!(
        created["data"]["created_at"]
            .as_str()
            .unwrap()
            .ends_with('Z'),
        "{created}"
    );

    // 完成任务 → `completed_at` 是**无偏移的本地时间**（D3）。
    let (status, completed) = body_json(
        complete_task_impl(&pool, &headers, &json!({ "task_id": task_id }))
            .await
            .expect("完成任务是 Ok 分支"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(completed["message"], "Task completed successfully");
    assert_eq!(completed["data"]["is_completed"], true);
    let completed_at = completed["data"]["completed_at"].as_str().unwrap();
    assert!(!completed_at.ends_with('Z'), "{completed_at}");
    assert!(!completed_at.contains('+'), "{completed_at}");

    // 未知 task_id → 404 `Task not found`。
    let (status, body) = body_json(
        complete_task_impl(
            &pool,
            &headers,
            &json!({ "task_id": "00000000-0000-4000-8000-000000000000" }),
        )
        .await
        .expect("查不到是 Ok 分支"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["message"], "Task not found");

    // 缺 task_id → 400。
    let (status, body) = body_json(
        complete_task_impl(&pool, &headers, &json!({}))
            .await
            .expect("缺参数是 Ok 分支"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], "task_id is required");

    sqlx::query("DELETE FROM task WHERE id = $1")
        .bind(Uuid::parse_str(&task_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn generate_task_from_disease_and_environment() {
    let pool = pool_or_skip().await;
    let Some(headers) = farmer_headers(&pool).await else {
        return;
    };

    // 缺参数 → 400。
    let (status, body) = body_json(
        generate_task_from_disease_impl(&pool, &headers, &json!({}))
            .await
            .expect("缺参数是 Ok 分支"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], "disease_name is required");

    // 健康果树 → 200 + data=null + 英文 message。
    let (status, body) = body_json(
        generate_task_from_disease_impl(&pool, &headers, &json!({ "disease_name": "健康果树" }))
            .await
            .expect("无需操作是 Ok 分支"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "No task needed for healthy tree");
    assert_eq!(body["data"], Value::Null);

    // 溃疡病 → 高风险任务，描述取自词典。
    let (status, body) = body_json(
        generate_task_from_disease_impl(&pool, &headers, &json!({ "disease_name": "溃疡病" }))
            .await
            .expect("建任务是 Ok 分支"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["title"], "溃疡病治理");
    assert_eq!(body["data"]["risk_level"], "高风险");
    assert_eq!(body["data"]["task_type"], "疾病识别");
    assert_eq!(body["data"]["source"], "自动生成");
    assert_eq!(body["data"]["description"], disease_treatment("溃疡病"));
    let disease_task_id = body["data"]["id"].as_str().unwrap().to_string();

    // 环境：27.5 / 88.0 → 高风险。
    let (status, body) = body_json(
        generate_task_from_environment_impl(&pool, &headers, &json!({}))
            .await
            .expect("缺参数是 Ok 分支"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], "temperature and humidity are required");

    let (status, body) = body_json(
        generate_task_from_environment_impl(
            &pool,
            &headers,
            &json!({ "temperature": 27.5, "humidity": 88.0 }),
        )
        .await
        .expect("建任务是 Ok 分支"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["title"], "环境监测-高风险");
    assert_eq!(body["data"]["risk_level"], "高风险");
    assert_eq!(body["data"]["task_type"], "温湿度监测");
    assert_eq!(body["data"]["completed_at"], Value::Null);
    let env_task_id = body["data"]["id"].as_str().unwrap().to_string();

    // 低风险 → 200 + data=null + 英文 message。
    let (status, body) = body_json(
        generate_task_from_environment_impl(
            &pool,
            &headers,
            &json!({ "temperature": 10.0, "humidity": 30.0 }),
        )
        .await
        .expect("无需操作是 Ok 分支"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "No task needed for low risk");
    assert_eq!(body["data"], Value::Null);

    for id in [disease_task_id, env_task_id] {
        sqlx::query("DELETE FROM task WHERE id = $1")
            .bind(Uuid::parse_str(&id).unwrap())
            .execute(&pool)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn tasks_list_orders_by_created_at_desc_and_keeps_field_order() {
    let pool = pool_or_skip().await;
    let Some(headers) = farmer_headers(&pool).await else {
        return;
    };

    let (status, body) =
        body_json(tasks_impl(&pool, &headers).await.expect("列任务是 Ok 分支")).await;
    assert_eq!(status, StatusCode::OK);

    let tasks = body["data"].as_array().unwrap();
    assert!(!tasks.is_empty(), "seed 里 farmer_xinfeng 有 3 条任务");

    let rendered = serde_json::to_string(&tasks[0]).unwrap();
    let mut last = 0;
    for key in [
        "id",
        "title",
        "description",
        "risk_level",
        "task_type",
        "source",
        "is_completed",
        "created_at",
        "completed_at",
    ] {
        let at = rendered.find(&format!(r#""{key}""#)).unwrap_or_else(|| {
            panic!("task 载荷缺少 {key}：{rendered}");
        });
        assert!(last < at, "{key} 的位置不对：{rendered}");
        last = at;
    }
}

// --------------------------------------------------------------------------------------
// DB 层：温湿度写入
// --------------------------------------------------------------------------------------

#[tokio::test]
async fn post_temperature_humidity_validates_then_saves() {
    let pool = pool_or_skip().await;
    let Some(headers) = farmer_headers(&pool).await else {
        return;
    };

    let bad_cases = [
        (
            json!({ "temperature": "abc", "humidity": 70 }),
            "temperature and humidity must be numbers",
        ),
        (
            json!({ "temperature": 200, "humidity": 70 }),
            "temperature or humidity is outside the supported range",
        ),
        (
            json!({ "temperature": 26, "humidity": 70, "record_time": "not-a-time" }),
            "record_time must be ISO 8601",
        ),
    ];
    for (payload, expected) in bad_cases {
        let (status, body) = body_json(
            post_temperature_humidity_impl(&pool, &headers, &payload)
                .await
                .expect("校验失败是 Ok 分支（成功体形状）"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{payload}");
        assert_eq!(body["message"], expected, "{payload}");
        assert!(body["timestamp"].is_i64(), "{body}");
    }

    // 正常写入：`record_time` 用 HTTP 里给的绝对时间，`tag_serial_number` 原样落库。
    let record_time = "2026-09-13T10:20:30.123456+00:00";
    let (status, body) = body_json(
        post_temperature_humidity_impl(
            &pool,
            &headers,
            &json!({
                "temperature": 27.5,
                "humidity": 88.0,
                "record_time": record_time,
                "tag_serial_number": "NFC-QA-0001",
            }),
        )
        .await
        .expect("写入成功是 Ok 分支"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "Temperature and humidity data saved");

    let recent = body["data"]["recentRecords"].as_array().unwrap();
    let inserted = recent
        .iter()
        .find(|item| item["tag_serial_number"] == "NFC-QA-0001")
        .expect("刚写入的记录必须出现在 recentRecords 里");
    assert_eq!(inserted["temperature"], 27.5);
    assert_eq!(inserted["humidity"], 88.0);
    assert!(
        inserted["record_time"]
            .as_str()
            .unwrap()
            .ends_with("+00:00")
    );

    sqlx::query("DELETE FROM temperature_humidity_data WHERE node_id = 'NFC-QA-0001'")
        .execute(&pool)
        .await
        .unwrap();
}

// --------------------------------------------------------------------------------------
// D2：施肥方案按设计意图实现（不复刻 5xx）
// --------------------------------------------------------------------------------------

#[tokio::test]
async fn fertilization_plan_validates_request_and_returns_serializer_shape() {
    let pool = pool_or_skip().await;
    let Some(headers) = farmer_headers(&pool).await else {
        return;
    };

    // 空体：八个字段全是必填 → 400 `Invalid request data`。
    let (status, body) = body_json(
        fertilization_plan_impl(&pool, &headers, &json!({}))
            .await
            .expect("校验失败是 Ok 分支"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], "Invalid request data");
    assert_eq!(body["code"], 400, "不复刻蓝本的 500");

    let (status, body) = body_json(
        fertilization_plan_impl(
            &pool,
            &headers,
            &json!({
                "soilType": "红壤",
                "phValue": 5.6,
                "nitrogenLevel": "中",
                "phosphorusLevel": "低",
                "potassiumLevel": "低",
                "growthStage": "壮果期",
                "treeAge": 8,
                "areaSize": 12.5,
            }),
        )
        .await
        .expect("正常请求是 Ok 分支"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let data = &body["data"];
    let plan_id = data["planId"].as_str().unwrap();
    assert!(plan_id.starts_with("fp-"), "{plan_id}");
    assert_eq!(data["title"], "赣南脐橙壮果期施肥方案");
    assert!(data["content"].as_str().unwrap().contains("红壤"));
    assert_eq!(data["recommendedFertilizers"].as_array().unwrap().len(), 2);
    assert_eq!(data["applicationSchedule"].as_array().unwrap().len(), 2);

    let mut last = 0;
    let rendered = serde_json::to_string(data).unwrap();
    for key in [
        "planId",
        "title",
        "content",
        "recommendedFertilizers",
        "applicationSchedule",
    ] {
        let at = rendered.find(&format!(r#""{key}""#)).unwrap();
        assert!(last < at, "{key} 的位置不对：{rendered}");
        last = at;
    }

    // 清理：外键 CASCADE 会带走 fertilizers / schedules。
    sqlx::query("DELETE FROM fertilization_plan WHERE plan_id = $1")
        .bind(plan_id)
        .execute(&pool)
        .await
        .unwrap();
}
