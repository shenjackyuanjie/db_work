//! Django / DRF 序列化契约：时间、日期、Decimal 的逐字对齐。
//!
//! 规则来自 `tests/fixtures/contract/FORMAT_NOTES.md` 的实测统计，不要凭直觉修改。

use chrono::{DateTime, Local, NaiveDate, Utc};
use rust_decimal::Decimal;

/// DRF `JSONEncoder` 渲染的 UTC 时间：`2026-09-20T13:41:16.399657Z`（主流形态）。
///
/// 微秒为 0 时整体省略小数部分，与 Python `datetime.isoformat()` 一致。
pub(crate) fn dt_z(value: DateTime<Utc>) -> String {
    format_utc(value, true)
}

/// 走 DRF `DateTimeField.to_representation` 的 UTC 时间：`...+00:00`。
pub(crate) fn dt_offset(value: DateTime<Utc>) -> String {
    format_utc(value, false)
}

fn format_utc(value: DateTime<Utc>, z_suffix: bool) -> String {
    let utc = value.with_timezone(&Utc);
    let suffix = if z_suffix { "Z" } else { "+00:00" };

    if utc.timestamp_subsec_micros() == 0 {
        return utc.format("%Y-%m-%dT%H:%M:%S").to_string() + suffix;
    }

    utc.format("%Y-%m-%dT%H:%M:%S%.6f").to_string() + suffix
}

/// 无偏移的本地时间（服务器时区）。
///
/// 仅用于逐字复刻蓝本的 `complete_task_api` 缺陷：它用 `datetime.now()` 写库，
/// 序列化出来没有时区偏移。除该字段外不要使用本函数。
pub(crate) fn dt_naive_local(value: DateTime<Utc>) -> String {
    let naive = value.with_timezone(&Local).naive_local();
    if naive.and_utc().timestamp_subsec_micros() == 0 {
        return naive.format("%Y-%m-%dT%H:%M:%S").to_string();
    }

    naive.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
}

/// `DateField` → `2026-09-20`。
pub(crate) fn dt_date(value: NaiveDate) -> String {
    value.format("%Y-%m-%d").to_string()
}

/// `DecimalField` → JSON 字符串，小数位由列定义决定（`NUMERIC(10,2)` → `"58.00"`）。
///
/// 从库里读出来的值已带列精度，直接 `to_string` 即可。
pub(crate) fn dec(value: Decimal) -> String {
    value.to_string()
}

/// 用于**在 Rust 侧算出**的金额：按列精度补零，避免 `0` 与蓝本的 `"0.00"` 不一致。
///
/// `Decimal::round_dp` 只做截断不补零，所以必须用 `rescale`。
pub(crate) fn dec_scaled(value: Decimal, places: u32) -> String {
    let mut scaled = value;
    scaled.rescale(places);

    scaled.to_string()
}

pub(crate) fn opt_dec_scaled(value: Option<Decimal>, places: u32) -> Option<String> {
    value.map(|value| dec_scaled(value, places))
}

pub(crate) fn opt_dt_z(value: Option<DateTime<Utc>>) -> Option<String> {
    value.map(dt_z)
}

pub(crate) fn opt_dt_offset(value: Option<DateTime<Utc>>) -> Option<String> {
    value.map(dt_offset)
}

pub(crate) fn opt_dt_naive_local(value: Option<DateTime<Utc>>) -> Option<String> {
    value.map(dt_naive_local)
}

pub(crate) fn opt_date(value: Option<NaiveDate>) -> Option<String> {
    value.map(dt_date)
}

pub(crate) fn opt_dec(value: Option<Decimal>) -> Option<String> {
    value.map(dec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(micros: u32) -> DateTime<Utc> {
        let base = "2026-09-20T13:41:16Z".parse::<DateTime<Utc>>().unwrap();
        base + chrono::Duration::microseconds(micros as i64)
    }

    #[test]
    fn z_suffix_keeps_six_digit_micros() {
        assert_eq!(dt_z(at(399_657)), "2026-09-20T13:41:16.399657Z");
    }

    #[test]
    fn z_suffix_omits_fraction_when_micros_are_zero() {
        assert_eq!(dt_z(at(0)), "2026-09-20T13:41:16Z");
    }

    #[test]
    fn offset_suffix_matches_drf_serializer_output() {
        assert_eq!(dt_offset(at(399_657)), "2026-09-20T13:41:16.399657+00:00");
        assert_eq!(dt_offset(at(0)), "2026-09-20T13:41:16+00:00");
    }

    #[test]
    fn naive_local_has_no_offset_suffix() {
        let rendered = dt_naive_local(at(671_819));
        assert!(!rendered.ends_with('Z'), "{rendered}");
        assert!(!rendered.contains('+'), "{rendered}");
        assert!(rendered.ends_with("16.671819"), "{rendered}");
    }

    #[test]
    fn decimal_keeps_scale_from_database_text() {
        // PostgreSQL 返回 `NUMERIC(10,2)` 的文本形式，小数位原样保留。
        assert_eq!(dec("58.00".parse::<Decimal>().unwrap()), "58.00");
        assert_eq!(dec("0.00".parse::<Decimal>().unwrap()), "0.00");
        assert_eq!(dec("25.5".parse::<Decimal>().unwrap()), "25.5");
    }

    #[test]
    fn decimal_computed_in_rust_is_padded_to_column_scale() {
        let zero = Decimal::ZERO;
        assert_eq!(dec(zero), "0");
        assert_eq!(dec_scaled(zero, 2), "0.00");

        let total = "100".parse::<Decimal>().unwrap();
        let deposit = "30.5".parse::<Decimal>().unwrap();
        assert_eq!(dec_scaled(total - deposit, 2), "69.50");
    }

    #[test]
    fn date_is_plain_iso() {
        assert_eq!(
            dt_date("2026-09-20".parse::<NaiveDate>().unwrap()),
            "2026-09-20"
        );
    }
}
