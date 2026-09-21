//! 网页后台仪表盘与审计日志超集。
//!
//! 负责的 `/web/*` 路径（外层已 `nest("/web")`）：
//!
//! | 路径 | 方法 | 旧路径 | 前端引用 |
//! |---|---|---|---|
//! | `/admin/dashboard/stats` | POST | `/user/admin/dashboard/stats` | `admin.js:929` |
//! | `/admin/dashboard/logs` | POST | `/user/admin/dashboard/logs` | `admin.js:1011` |
//!
//! ## 数据源（`W2_PLAN.md` §3.1）
//!
//! | 用途 | 表 | 说明 |
//! |---|---|---|
//! | 识别统计 / 日志 | `web_diagnosis_records` | 网页专表，17 列富字段（`is_healthy` / `is_citrus_leaf` / `predicted_class` / `severity`）。**契约表 `disease_recognition_record` 只有 9 列，统计不出来** |
//! | 用户 / 管理员数 | `"user"` + 加法列 `is_admin` | 不再读 `app_users` |
//! | 待审批数 | `app_pending_users` | 保留（注册审批是 Rust 超集） |
//! | 审计日志 | `app_admin_audit_logs` | 保留 |
//! | 环境温湿度 | `temperature_humidity_data` | 改读契约表；`app_temperature_humidity` 退役。**注意它的 `timestamp` 是 `TIMESTAMPTZ`**（旧表是 epoch 毫秒） |
//!
//! ## 响应形状：逐字段对齐 `admin.js`（不是猜的）
//!
//! `admin.js:936-979` 读的是 `totals.{detections,healthy_count,diseased_count,healthy_rate,users,admins,pending_users}`
//! / `daily_counts[].{date,count}` / `environment.{current_temperature,current_humidity,health_score,active_alerts}`；
//! `renderLogs`（`admin.js:1031+`）读 `logs[].{type,message,created_at,actor}`，`type` 用于筛选
//! （`detect` / `warning` / `action`）。这些字段名一个都不能改。
//!
//! `created_at` 一律输出**毫秒**：`admin.js:1700`、`parseTime`（`admin.js:1023-1029`）
//! 按 `> 1e12 ? ms : s` 处理，秒与毫秒混用会显示成 1970 年。

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};

use crate::server::AppState;
use crate::system_settings::load_system_settings;

use super::session;

/// 系统设置读不到时的兜底保留天数。
///
/// 旧实现直接返回 500；超集层没有契约约束，读不到配置就退化比让整个仪表盘空掉更有用。
const DEFAULT_LOG_RETENTION_DAYS: i32 = 30;

/// 与旧实现一致的日志条数上限（`dashboard_logs_handler` 取 120，最终 `take(150)`）。
const LOG_FETCH_LIMIT: i64 = 120;
const LOG_OUTPUT_LIMIT: usize = 150;

/// 近 7 天趋势的窗口长度（`admin.js:949` 的「近7天共 N 条」）。
const TREND_DAYS: i64 = 7;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/dashboard/stats", post(stats_api))
        .route("/admin/dashboard/logs", post(logs_api))
}

// --------------------------------------------------------------------------------------
// 纯函数（可单测）
// --------------------------------------------------------------------------------------

/// 健康率：与旧实现一致——总数为 0 时返回 100（而不是 0），前端直接显示 `healthyRate + "%"`。
fn healthy_rate(total: i64, healthy: i64) -> i64 {
    if total > 0 {
        ((healthy as f64 / total as f64) * 100.0).round() as i64
    } else {
        100
    }
}

/// 把识别时间戳（**毫秒**，`web_diagnosis_records.timestamp`）按天分桶成近 `days` 天的序列。
///
/// 时间基准沿用旧实现的 `Utc`（不是 `Asia/Shanghai`）——改基准会让已上线仪表盘的
/// 日期整体偏移 8 小时，属于显示口径变更，本次不动。
fn bucket_daily_counts(timestamps_ms: &[i64], today: NaiveDate, days: i64) -> Vec<(String, i64)> {
    let mut series: Vec<(String, i64)> = (0..days)
        .map(|offset| {
            let date = today - Duration::days(days - 1 - offset);
            (date.format("%Y-%m-%d").to_string(), 0_i64)
        })
        .collect();

    for timestamp in timestamps_ms {
        let Some(datetime) = Utc.timestamp_millis_opt(*timestamp).single() else {
            continue;
        };
        let diff_days = (today - datetime.date_naive()).num_days();
        if (0..days).contains(&diff_days) {
            if let Some((_, count)) = series.get_mut((days - 1 - diff_days) as usize) {
                *count += 1;
            }
        }
    }

    series
}

/// 失败体一律走超集信封（`session::app_*`），与 `/web/*` 其余接口保持一致。
fn error_response(status: StatusCode, message: &str) -> Response {
    session::app_err(status, message, Value::Null)
}

fn internal_error(message: &str) -> Response {
    error_response(StatusCode::INTERNAL_SERVER_ERROR, message)
}

// --------------------------------------------------------------------------------------
// 统计
// --------------------------------------------------------------------------------------

async fn stats_api(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = session::require_admin(&state, &headers).await {
        return response;
    }

    match stats_impl(&state.db).await {
        Ok(payload) => session::app_ok("success", payload),
        Err(error) => {
            tracing::error!("读取仪表盘统计失败: {error}");
            internal_error("读取统计数据失败")
        }
    }
}

async fn stats_impl(pool: &PgPool) -> Result<Value, sqlx::Error> {
    let total_detections =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM web_diagnosis_records")
            .fetch_one(pool)
            .await?;
    let healthy_count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM web_diagnosis_records WHERE is_healthy")
            .fetch_one(pool)
            .await?;
    // 「病害」= 是柑橘叶片、不健康、且预测类别不是「非果树」——三层过滤缺一不可，
    // 否则非果树与正常叶片会被算成病害。
    let diseased_count = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM web_diagnosis_records
           WHERE is_citrus_leaf AND NOT is_healthy AND predicted_class <> '非果树'"#,
    )
    .fetch_one(pool)
    .await?;

    let user_count = sqlx::query_scalar::<_, i64>(r#"SELECT COUNT(*) FROM "user""#)
        .fetch_one(pool)
        .await?;
    let admin_count = sqlx::query_scalar::<_, i64>(r#"SELECT COUNT(*) FROM "user" WHERE is_admin"#)
        .fetch_one(pool)
        .await?;
    let pending_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM app_pending_users")
        .fetch_one(pool)
        .await?;

    let detection_rows = sqlx::query(
        "SELECT timestamp FROM web_diagnosis_records ORDER BY timestamp DESC LIMIT 500",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let timestamps: Vec<i64> = detection_rows
        .iter()
        .filter_map(|row| row.try_get::<i64, _>("timestamp").ok())
        .collect();

    let today = Utc::now().date_naive();
    let series = bucket_daily_counts(&timestamps, today, TREND_DAYS);

    // 契约表的 timestamp 是 TIMESTAMPTZ（旧表是 epoch 毫秒），必须按时间类型解码。
    let latest_env = sqlx::query(
        "SELECT temperature, humidity FROM temperature_humidity_data ORDER BY timestamp DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .unwrap_or(None);
    let current_temperature = latest_env
        .as_ref()
        .and_then(|row| row.try_get::<f64, _>("temperature").ok());
    let current_humidity = latest_env
        .as_ref()
        .and_then(|row| row.try_get::<f64, _>("humidity").ok());

    let rate = healthy_rate(total_detections, healthy_count);

    Ok(json!({
        "totals": {
            "detections": total_detections,
            "healthy_count": healthy_count,
            "diseased_count": diseased_count,
            "users": user_count,
            "admins": admin_count,
            "pending_users": pending_count,
            "healthy_rate": rate,
        },
        "daily_counts": series
            .into_iter()
            .map(|(date, count)| json!({ "date": date, "count": count }))
            .collect::<Vec<_>>(),
        "environment": {
            "current_temperature": current_temperature,
            "current_humidity": current_humidity,
            "health_score": rate,
            "active_alerts": diseased_count + pending_count,
        },
    }))
}

// --------------------------------------------------------------------------------------
// 日志
// --------------------------------------------------------------------------------------

async fn logs_api(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = session::require_admin(&state, &headers).await {
        return response;
    }

    match logs_impl(&state.db).await {
        Ok(payload) => session::app_ok("success", payload),
        Err(error) => {
            tracing::error!("读取审计日志失败: {error}");
            internal_error("读取日志失败")
        }
    }
}

async fn logs_impl(pool: &PgPool) -> Result<Value, sqlx::Error> {
    let retention_days = match load_system_settings(pool).await {
        Ok(settings) => settings.log_retention_days,
        Err(error) => {
            tracing::warn!(
                "读取系统设置失败，日志窗口退化到 {DEFAULT_LOG_RETENTION_DAYS} 天: {error}"
            );
            DEFAULT_LOG_RETENTION_DAYS
        }
    };
    let cutoff = Utc::now() - Duration::days(retention_days as i64);
    let cutoff_millis = cutoff.timestamp_millis();

    let audit_rows = sqlx::query(
        r#"SELECT log_type, actor_username, message, created_at
           FROM app_admin_audit_logs WHERE created_at >= $1
           ORDER BY created_at DESC LIMIT $2"#,
    )
    .bind(cutoff_millis)
    .bind(LOG_FETCH_LIMIT)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let diagnosis_rows = sqlx::query(
        r#"SELECT username, predicted_class, confidence, timestamp, is_healthy, is_citrus_leaf
           FROM web_diagnosis_records WHERE timestamp >= $1
           ORDER BY timestamp DESC LIMIT $2"#,
    )
    .bind(cutoff_millis)
    .bind(LOG_FETCH_LIMIT)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    // app_pending_users.created_at 是 epoch **秒**（`now_secs()` 写入），下面要 ×1000。
    let pending_rows = sqlx::query(
        r#"SELECT username, requested_role, created_at FROM app_pending_users
           WHERE created_at >= $1 ORDER BY created_at DESC LIMIT $2"#,
    )
    .bind(cutoff_millis / 1000)
    .bind(LOG_FETCH_LIMIT / 2)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    // 契约表 "user".created_at 是 TIMESTAMPTZ（不再是 epoch 秒），所以按时间解码。
    let user_rows = sqlx::query(
        r#"SELECT username, is_admin, created_at FROM "user"
           WHERE created_at >= $1 ORDER BY created_at DESC LIMIT $2"#,
    )
    .bind(cutoff)
    .bind(LOG_FETCH_LIMIT / 2)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut logs: Vec<Value> = Vec::new();

    for row in audit_rows {
        logs.push(json!({
            "type": row.try_get::<String, _>("log_type").unwrap_or_else(|_| "action".to_string()),
            "message": row.try_get::<String, _>("message").unwrap_or_default(),
            "created_at": row.try_get::<i64, _>("created_at").unwrap_or(0),
            "actor": row.try_get::<Option<String>, _>("actor_username").ok().flatten(),
        }));
    }

    for row in diagnosis_rows {
        let predicted_class = row
            .try_get::<String, _>("predicted_class")
            .unwrap_or_default();
        let confidence = row.try_get::<f64, _>("confidence").unwrap_or(0.0);
        let username = row
            .try_get::<Option<String>, _>("username")
            .ok()
            .flatten()
            .unwrap_or_else(|| "匿名用户".to_string());
        let is_healthy = row.try_get::<bool, _>("is_healthy").unwrap_or(false);
        let is_citrus_leaf = row.try_get::<bool, _>("is_citrus_leaf").unwrap_or(false);
        // 判定口径与 stats 的 diseased_count 保持一致。
        let log_type = if is_citrus_leaf && !is_healthy && predicted_class != "非果树" {
            "warning"
        } else {
            "detect"
        };
        logs.push(json!({
            "type": log_type,
            "message": format!("{username} 完成病害识别，结果：{predicted_class}（{confidence:.1}%）"),
            "created_at": row.try_get::<i64, _>("timestamp").unwrap_or(0),
            "actor": username,
        }));
    }

    for row in pending_rows {
        let username = row.try_get::<String, _>("username").unwrap_or_default();
        let requested_role = row
            .try_get::<String, _>("requested_role")
            .unwrap_or_else(|_| "user".to_string());
        // 蓝本的 role 只有 farmer/buyer；自研审批流用 admin/user，两种都要认，
        // 否则管理员申请会被显示成「用户」。
        let role_text = if requested_role.eq_ignore_ascii_case("admin") {
            "管理员"
        } else if requested_role.eq_ignore_ascii_case("farmer") {
            "果农"
        } else if requested_role.eq_ignore_ascii_case("buyer") {
            "购买者"
        } else {
            "用户"
        };
        logs.push(json!({
            "type": "warning",
            "message": format!("{username} 提交了{role_text}注册申请，等待审核"),
            "created_at": row.try_get::<i64, _>("created_at").unwrap_or(0) * 1000,
            "actor": username,
        }));
    }

    for row in user_rows {
        let username = row.try_get::<String, _>("username").unwrap_or_default();
        let is_admin = row.try_get::<bool, _>("is_admin").unwrap_or(false);
        let created_at = row
            .try_get::<DateTime<Utc>, _>("created_at")
            .map(|value| value.timestamp_millis())
            .unwrap_or(0);
        logs.push(json!({
            "type": "action",
            "message": format!("用户 {username} 注册成功{}", if is_admin { "（管理员）" } else { "" }),
            "created_at": created_at,
            "actor": username,
        }));
    }

    logs.sort_by(|left, right| {
        let right_ts = right.get("created_at").and_then(Value::as_i64).unwrap_or(0);
        let left_ts = left.get("created_at").and_then(Value::as_i64).unwrap_or(0);
        right_ts.cmp(&left_ts)
    });

    Ok(json!({
        "logs": logs.into_iter().take(LOG_OUTPUT_LIMIT).collect::<Vec<_>>()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> NaiveDate {
        text.parse().unwrap()
    }

    #[test]
    fn healthy_rate_is_100_when_there_is_nothing_to_divide() {
        // 旧实现的既有行为：没有识别记录时显示 100% 而不是 0%（admin.js 直接显示它）。
        assert_eq!(healthy_rate(0, 0), 100);
        assert_eq!(healthy_rate(4, 1), 25);
        assert_eq!(healthy_rate(3, 2), 67);
        assert_eq!(healthy_rate(1, 1), 100);
    }

    #[test]
    fn daily_buckets_cover_the_full_window_in_chronological_order() {
        let today = date("2026-09-20");
        // 空数据也要给出 7 个连续日期，否则前端「近7天共 0 条」的图表会没有横轴。
        let empty = bucket_daily_counts(&[], today, TREND_DAYS);
        assert_eq!(empty.len(), 7);
        assert_eq!(empty.first().unwrap().0, "2026-09-14");
        assert_eq!(empty.last().unwrap().0, "2026-09-20");
        assert!(empty.iter().all(|(_, count)| *count == 0));
    }

    #[test]
    fn daily_buckets_place_timestamps_on_the_right_day() {
        let today = date("2026-09-20");
        let stamp = |text: &str| text.parse::<DateTime<Utc>>().unwrap().timestamp_millis();
        let counts = bucket_daily_counts(
            &[
                stamp("2026-09-20T00:00:00Z"), // 今天
                stamp("2026-09-20T23:59:59Z"), // 仍是今天
                stamp("2026-09-19T12:00:00Z"), // 昨天
            ],
            today,
            TREND_DAYS,
        );
        assert_eq!(counts[6].1, 2, "{counts:?}");
        assert_eq!(counts[5].1, 1, "{counts:?}");
    }

    #[test]
    fn daily_buckets_ignore_rows_outside_the_window_and_junk_timestamps() {
        let today = date("2026-09-20");
        let old = "2026-09-01T00:00:00Z"
            .parse::<DateTime<Utc>>()
            .unwrap()
            .timestamp_millis();
        let future = "2026-09-25T00:00:00Z"
            .parse::<DateTime<Utc>>()
            .unwrap()
            .timestamp_millis();
        // 窗口外的与越界的时间戳都不能被算进趋势，也不能 panic（下面两个是极端值）。
        let counts = bucket_daily_counts(&[old, future, -1, i64::MAX], today, TREND_DAYS);
        assert!(counts.iter().all(|(_, count)| *count == 0), "{counts:?}");
    }
}
