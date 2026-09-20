//! 契约层请求体 DTO。
//!
//! 一律沿用 Django 的原始字段名（snake_case），**不做** camelCase 转换——转换只发生在
//! 响应侧，且是按字段逐个对齐的。
//!
//! 另一个刻意为之的点：请求体不 derive 到强类型 struct，而是保留 [`Input`] 三态
//! （缺字段 / 显式 `null` / 有值）。DRF 的校验文案对这三态**是区分**的：
//!
//! | 输入 | `CharField(required=True)` 的报错 |
//! |---|---|
//! | 字段不存在 | `该字段是必填项。` |
//! | 字段为 `null` | `该字段不能为 null。` |
//! | 字段为 `""` | `该字段不能为空。` |
//!
//! 直接 derive 成 `Option<String>` 会把前两态合并成一个 `None`，文案就回不去了。

use serde_json::{Map, Value};

/// 单个字段的三态输入。
#[derive(Debug, Clone)]
pub(crate) enum Input {
    /// 请求体里没有这个键。
    Missing,
    /// 显式传了 `null`。
    Null,
    /// 传了实际值。
    Value(Value),
}

impl Input {
    /// 从请求体里取字段；`body` 不是对象时一律当作缺字段（DRF 对非 dict 体会先报
    /// `无效数据。期待为字典类型，得到的是 list 类型。`，本域夹具未覆盖该分支）。
    pub(crate) fn take(body: &Map<String, Value>, key: &str) -> Self {
        match body.get(key) {
            None => Input::Missing,
            Some(Value::Null) => Input::Null,
            Some(value) => Input::Value(value.clone()),
        }
    }

    pub(crate) fn is_missing(&self) -> bool {
        matches!(self, Input::Missing)
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Input::Value(Value::String(text)) => Some(text.as_str()),
            _ => None,
        }
    }

    pub(crate) fn as_u64(&self) -> Option<u64> {
        match self {
            Input::Value(Value::Number(number)) => number.as_u64(),
            _ => None,
        }
    }

    /// DRF `FloatField` 接受 JSON number，也接受可解析的字符串。
    pub(crate) fn as_f64(&self) -> Option<f64> {
        match self {
            Input::Value(Value::Number(number)) => number.as_f64(),
            Input::Value(Value::String(text)) => text.trim().parse::<f64>().ok(),
            _ => None,
        }
    }
}

/// `POST /api/register`、`POST /api/v1/auth/register` 的请求体。
///
/// 字段序与 `serializers.py::UserRegistrationSerializer.Meta.fields` 一致，
/// 校验顺序（决定「一次只报哪个字段」）也依赖这个顺序。
#[derive(Debug, Clone)]
pub(crate) struct UserRegistrationBody {
    pub(crate) username: Input,
    pub(crate) email: Input,
    pub(crate) password: Input,
    pub(crate) role: Input,
    pub(crate) orchard_address: Input,
    pub(crate) latitude: Input,
    pub(crate) longitude: Input,
}

impl UserRegistrationBody {
    /// 所有字段都是 `Input::Missing` 的形态。
    pub(crate) fn empty() -> Self {
        Self {
            username: Input::Missing,
            email: Input::Missing,
            password: Input::Missing,
            role: Input::Missing,
            orchard_address: Input::Missing,
            latitude: Input::Missing,
            longitude: Input::Missing,
        }
    }

    pub(crate) fn from_value(body: &Value) -> Self {
        let Some(map) = body.as_object() else {
            return Self::empty();
        };

        Self {
            username: Input::take(map, "username"),
            email: Input::take(map, "email"),
            password: Input::take(map, "password"),
            role: Input::take(map, "role"),
            orchard_address: Input::take(map, "orchard_address"),
            latitude: Input::take(map, "latitude"),
            longitude: Input::take(map, "longitude"),
        }
    }
}

/// `POST /api/login`、`POST /api/v1/auth/login` 的请求体。
#[derive(Debug, Clone)]
pub(crate) struct UserLoginBody {
    pub(crate) username: Input,
    pub(crate) password: Input,
}

impl UserLoginBody {
    pub(crate) fn from_value(body: &Value) -> Self {
        let Some(map) = body.as_object() else {
            return Self {
                username: Input::Missing,
                password: Input::Missing,
            };
        };

        Self {
            username: Input::take(map, "username"),
            password: Input::take(map, "password"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_null_and_value_are_three_distinct_states() {
        let body =
            UserLoginBody::from_value(&serde_json::json!({"username": "a", "password": null}));
        assert_eq!(body.username.as_str(), Some("a"));
        assert!(matches!(body.password, Input::Null));

        let body = UserLoginBody::from_value(&serde_json::json!({"username": "a"}));
        assert!(body.password.is_missing());
    }

    #[test]
    fn non_object_body_degrades_to_all_missing() {
        let body = UserRegistrationBody::from_value(&serde_json::json!([1, 2, 3]));
        assert!(body.username.is_missing());
        assert!(body.password.is_missing());
    }

    #[test]
    fn float_field_accepts_number_and_numeric_string() {
        assert_eq!(Input::Value(serde_json::json!(25.1)).as_f64(), Some(25.1));
        assert_eq!(Input::Value(serde_json::json!("26")).as_f64(), Some(26.0));
        assert_eq!(Input::Value(serde_json::json!("abc")).as_f64(), None);
    }
}
