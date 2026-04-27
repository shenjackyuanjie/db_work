use axum::{
    Json,
    body::to_bytes,
    extract::Request,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::{AppState, username_by_token};

#[derive(Debug)]
pub(super) struct ParsedCitrusRequest {
    pub image_data: String,
    pub username: String,
    pub area: Option<String>,
    pub temperature: Option<f64>,
    pub humidity: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct CitrusDiseaseJsonRequest {
    #[serde(rename = "IMAGE")]
    image_upper: Option<String>,
    image: Option<String>,
    username: Option<String>,
    area: Option<String>,
    temperature: Option<Value>,
    humidity: Option<Value>,
}

async fn auth_username_from_headers(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<String>, Response> {
    match crate::user_routes::extract_auth_token(headers) {
        Some(token) => match username_by_token(state, &token).await {
            Some(name) => Ok(Some(name)),
            None => Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "code": 401,
                    "message": "Invalid token",
                    "data": null
                })),
            )
                .into_response()),
        },
        None => Ok(None),
    }
}

fn payload_preview(input: &str) -> String {
    input.chars().take(48).collect()
}

fn parse_optional_json_f64(value: Option<Value>, field_name: &str) -> Result<Option<f64>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_f64()
            .map(Some)
            .ok_or_else(|| format!("{} 必须是合法数字", field_name)),
        Some(Value::String(text)) => parse_optional_text_f64(&text, field_name),
        Some(_) => Err(format!("{} 必须是数字或数字字符串", field_name)),
    }
}

fn parse_optional_text_f64(value: &str, field_name: &str) -> Result<Option<f64>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    trimmed
        .parse::<f64>()
        .map(Some)
        .map_err(|_| format!("{} 必须是合法数字", field_name))
}

fn bad_request_response(message: String) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "code": 400,
            "message": message,
            "data": null
        })),
    )
        .into_response()
}

pub(super) async fn extract_citrus_request(
    state: &AppState,
    headers: &HeaderMap,
    request: Request,
    log_prefix: &str,
) -> Result<ParsedCitrusRequest, Response> {
    let auth_username = auth_username_from_headers(state, headers).await?;
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();

    tracing::info!("{} 请求: content_type={}", log_prefix, content_type);

    let body_bytes = match to_bytes(request.into_body(), 20 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::error!("读取请求体失败: {}", err);
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "code": 400,
                    "message": format!("读取请求体失败: {}", err),
                    "data": null
                })),
            )
                .into_response());
        }
    };

    tracing::info!("{} 请求体大小: {} bytes", log_prefix, body_bytes.len());

    let mut image_data: Option<String> = None;
    let mut image_base64_text: Option<String> = None;
    let mut username: Option<String> = None;
    let mut area: Option<String> = None;
    let mut temperature: Option<f64> = None;
    let mut humidity: Option<f64> = None;

    if content_type.starts_with("application/json") {
        let payload: CitrusDiseaseJsonRequest = match serde_json::from_slice(&body_bytes) {
            Ok(payload) => payload,
            Err(err) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "code": 400,
                        "message": format!("JSON 请求体格式错误: {}", err),
                        "data": null
                    })),
                )
                    .into_response());
            }
        };

        tracing::info!(
            "{} JSON字段: has_IMAGE={} has_image={} has_username={} has_area={} has_temperature={} has_humidity={}",
            log_prefix,
            payload
                .image_upper
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty()),
            payload
                .image
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty()),
            payload
                .username
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty()),
            payload
                .area
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty()),
            payload.temperature.is_some(),
            payload.humidity.is_some()
        );

        image_base64_text = payload.image_upper.or(payload.image);
        username = payload
            .username
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        area = payload
            .area
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        temperature = match parse_optional_json_f64(payload.temperature, "temperature") {
            Ok(value) => value,
            Err(message) => return Err(bad_request_response(message)),
        };
        humidity = match parse_optional_json_f64(payload.humidity, "humidity") {
            Ok(value) => value,
            Err(message) => return Err(bad_request_response(message)),
        };
    } else if content_type.starts_with("multipart/form-data") {
        let boundary = match multer::parse_boundary(&content_type) {
            Ok(boundary) => boundary,
            Err(err) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "code": 400,
                        "message": format!("multipart boundary 解析失败: {}", err),
                        "data": null
                    })),
                )
                    .into_response());
            }
        };

        let stream = futures_util::stream::once(async move {
            Ok::<axum::body::Bytes, std::io::Error>(body_bytes)
        });
        let mut multipart = multer::Multipart::new(stream, boundary);

        loop {
            match multipart.next_field().await {
                Ok(Some(field)) => {
                    let field_name = field.name().map(|value| value.to_string());
                    tracing::debug!("{} multipart 字段: {:?}", log_prefix, field_name);

                    match field_name.as_deref() {
                        Some("image") => {
                            let mime_type = field
                                .content_type()
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "image/jpeg".to_string());

                            match field.bytes().await {
                                Ok(bytes) => {
                                    let b64 =
                                        base64::engine::general_purpose::STANDARD.encode(&bytes);
                                    image_data = Some(format!("data:{};base64,{}", mime_type, b64));
                                    tracing::info!(
                                        "{} multipart image读取成功: mime={} size={} bytes",
                                        log_prefix,
                                        mime_type,
                                        bytes.len()
                                    );
                                }
                                Err(err) => {
                                    tracing::error!("读取图片字段失败: {}", err);
                                    return Err((
                                        StatusCode::BAD_REQUEST,
                                        Json(json!({
                                            "code": 400,
                                            "message": format!("读取图片数据失败: {}", err),
                                            "data": null
                                        })),
                                    )
                                        .into_response());
                                }
                            }
                        }
                        Some("IMAGE") => match field.text().await {
                            Ok(text) => {
                                tracing::info!(
                                    "{} multipart IMAGE读取成功: len={} preview={}...",
                                    log_prefix,
                                    text.len(),
                                    payload_preview(text.trim())
                                );
                                image_base64_text = Some(text);
                            }
                            Err(err) => {
                                tracing::error!("读取 IMAGE 字段失败: {}", err);
                                return Err((
                                    StatusCode::BAD_REQUEST,
                                    Json(json!({
                                        "code": 400,
                                        "message": format!("读取IMAGE字段失败: {}", err),
                                        "data": null
                                    })),
                                )
                                    .into_response());
                            }
                        },
                        Some("username") => {
                            if let Ok(text) = field.text().await {
                                let trimmed = text.trim();
                                if !trimmed.is_empty() {
                                    username = Some(trimmed.to_string());
                                }
                            }
                        }
                        Some("area") => {
                            if let Ok(text) = field.text().await {
                                let trimmed = text.trim();
                                if !trimmed.is_empty() {
                                    area = Some(trimmed.to_string());
                                }
                            }
                        }
                        Some("temperature") => match field.text().await {
                            Ok(text) => match parse_optional_text_f64(&text, "temperature") {
                                Ok(value) => temperature = value,
                                Err(message) => return Err(bad_request_response(message)),
                            },
                            Err(err) => {
                                tracing::error!("读取 temperature 字段失败: {}", err);
                                return Err(bad_request_response(format!(
                                    "读取temperature字段失败: {}",
                                    err
                                )));
                            }
                        },
                        Some("humidity") => match field.text().await {
                            Ok(text) => match parse_optional_text_f64(&text, "humidity") {
                                Ok(value) => humidity = value,
                                Err(message) => return Err(bad_request_response(message)),
                            },
                            Err(err) => {
                                tracing::error!("读取 humidity 字段失败: {}", err);
                                return Err(bad_request_response(format!(
                                    "读取humidity字段失败: {}",
                                    err
                                )));
                            }
                        },
                        _ => {}
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    tracing::error!("解析 multipart 失败: {}", err);
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "code": 400,
                            "message": format!("解析请求失败: {}", err),
                            "data": null
                        })),
                    )
                        .into_response());
                }
            }
        }
    } else {
        return Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Json(json!({
                "code": 415,
                "message": "仅支持 application/json 或 multipart/form-data",
                "data": null
            })),
        )
            .into_response());
    }

    if image_data.is_none()
        && let Some(mut base64_image) = image_base64_text
    {
        tracing::info!(
            "{} 原始IMAGE: len={} has_data_uri={} preview={}...",
            log_prefix,
            base64_image.len(),
            base64_image
                .trim()
                .to_ascii_lowercase()
                .starts_with("data:image/"),
            payload_preview(base64_image.trim())
        );

        if !base64_image.starts_with("data:image/") {
            base64_image = format!("data:image/jpeg;base64,{}", base64_image);
            tracing::info!(
                "{} IMAGE补齐data-uri后: len={} preview={}...",
                log_prefix,
                base64_image.len(),
                payload_preview(base64_image.trim())
            );
        }

        image_data = Some(base64_image);
    }

    if username.is_none() {
        username = auth_username;
    }

    let username = match username {
        Some(u) => u,
        None => return Err(bad_request_response("请求中未包含 username".to_string())),
    };

    let image_data = match image_data {
        Some(image_data) => image_data,
        None => return Err(bad_request_response("请求中未找到 image 字段".to_string())),
    };

    Ok(ParsedCitrusRequest {
        image_data,
        username,
        area,
        temperature,
        humidity,
    })
}
