//! 响应信封与错误体：逐字对齐 Django 侧 `api/responses.py` 与 `api/exceptions.py`。
//!
//! 两条**不能搞混**的路径：
//! - 成功体：`{code, message, data, timestamp}`，`timestamp` 是毫秒整数；
//! - DRF 异常体：`{code, message, data: null}`，**没有 `timestamp`**。
//!
//! 另外蓝本用「200 + 自定义 message」表达「无需操作」（如
//! `"No task needed for healthy tree"`），此时 `data` 通常为 `null`。

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::Value;

/// 成功体，HTTP status 与 `code` 都是 200。
pub(crate) fn api_ok(data: Value) -> Response {
    api_response(
        StatusCode::OK,
        200,
        Value::String("success".to_string()),
        data,
    )
}

/// 200 + 自定义 message（蓝本的「无需操作」语义）。
pub(crate) fn api_ok_message(message: impl Into<String>, data: Value) -> Response {
    api_response(StatusCode::OK, 200, Value::String(message.into()), data)
}

/// DRF 异常体。`message` 允许是字符串或对象——蓝本里
/// `Invalid credentials` 这类校验错误会渲染成 `{"non_field_errors":["..."]}`。
pub(crate) fn api_error(status: StatusCode, message: Value) -> Response {
    (
        status,
        Json(serde_json::json!({
            "code": status.as_u16(),
            "message": message,
            "data": Value::Null
        })),
    )
        .into_response()
}

/// 成功体通用构造，用于逐字复刻蓝本里少数形状特殊的分支。
pub(crate) fn api_response(status: StatusCode, code: u16, message: Value, data: Value) -> Response {
    (
        status,
        Json(serde_json::json!({
            "code": code,
            "message": message,
            "data": data,
            "timestamp": crate::server::now_millis()
        })),
    )
        .into_response()
}

/// 处理器里可以用 `?` 抛出的契约错误，`IntoResponse` 输出 DRF 异常体。
#[derive(Debug)]
pub(crate) struct ApiReject {
    status: StatusCode,
    message: Value,
}

impl ApiReject {
    pub(crate) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: Value::String(message.into()),
        }
    }

    /// 蓝本中 `message` 是对象的情形（DRF 校验错误）。
    pub(crate) fn with_json(status: StatusCode, message: Value) -> Self {
        Self { status, message }
    }

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub(crate) fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    pub(crate) fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }
}

impl IntoResponse for ApiReject {
    fn into_response(self) -> Response {
        api_error(self.status, self.message)
    }
}

pub(crate) type ApiResult = Result<Response, ApiReject>;

#[cfg(test)]
mod tests {
    use super::*;

    async fn body_json(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn success_envelope_carries_millis_timestamp() {
        let value = body_json(api_ok(serde_json::json!({"a": 1}))).await;
        assert_eq!(value["code"], 200);
        assert_eq!(value["message"], "success");
        assert_eq!(value["data"]["a"], 1);
        assert!(value["timestamp"].is_i64(), "{value}");
    }

    #[tokio::test]
    async fn error_envelope_has_no_timestamp() {
        let response = api_error(
            StatusCode::UNAUTHORIZED,
            Value::String("登录凭证无效".into()),
        );
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let value = body_json(response).await;
        assert_eq!(value["code"], 401);
        assert_eq!(value["message"], "登录凭证无效");
        assert_eq!(value["data"], Value::Null);
        assert!(value.get("timestamp").is_none(), "{value}");
    }

    #[tokio::test]
    async fn reject_renders_drf_shape() {
        let value = body_json(ApiReject::forbidden("该接口仅限果农使用").into_response()).await;
        assert_eq!(value["code"], 403);
        assert!(value.get("timestamp").is_none(), "{value}");
    }
}
