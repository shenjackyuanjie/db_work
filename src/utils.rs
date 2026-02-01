use anyhow::{Result, bail};
use base64::Engine;
use std::fs;
use std::path::Path;

/// 将图片文件转换为 base64 格式的 data URL
pub fn image_to_base64(image_path: &str) -> Result<String> {
    let path = Path::new(image_path);

    if !path.exists() {
        bail!("图片文件不存在: {}", image_path);
    }

    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .ok_or_else(|| anyhow::anyhow!("无法获取图片文件扩展名"))?;

    let mime_type = match extension.to_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => bail!("不支持的图片格式: {}", extension),
    };

    let image_data = fs::read(path)?;
    let base64_data = base64::engine::general_purpose::STANDARD.encode(&image_data);

    Ok(format!("data:{};base64,{}", mime_type, base64_data))
}

/// 构建消息内容
pub fn build_message_content(text: &str, image_path: Option<&String>) -> serde_json::Value {
    match image_path {
        Some(img_path) => {
            // 检查是否已经是 data URL（客户端可能已经转换过）
            let image_url = if img_path.starts_with("data:") {
                img_path.to_string()
            } else {
                match image_to_base64(img_path) {
                    Ok(url) => url,
                    Err(e) => {
                        eprintln!("警告: 图片处理失败: {}, 将只发送文本", e);
                        return serde_json::Value::String(text.to_string());
                    }
                }
            };

            serde_json::json!([
                {
                    "type": "text",
                    "text": text
                },
                {
                    "type": "image_url",
                    "image_url": {
                        "url": image_url
                    }
                }
            ])
        }
        None => serde_json::Value::String(text.to_string()),
    }
}
