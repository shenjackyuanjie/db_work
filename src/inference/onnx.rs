use super::types::{DiseasePrediction, FruitTreeGatePrediction};
use base64::Engine;
use image::imageops::FilterType;
use std::path::Path;
use tract_onnx::prelude::*;

#[derive(Clone)]
pub struct OnnxInference {
    pub model_1_path: String,
    pub model_2_path: String,
}

impl OnnxInference {
    pub fn new(model_1_path: String, model_2_path: String) -> Self {
        Self {
            model_1_path,
            model_2_path,
        }
    }

    pub async fn predict_citrus_disease(
        &self,
        image_data: Option<String>,
    ) -> anyhow::Result<DiseasePrediction> {
        let image_data = image_data.ok_or_else(|| anyhow::anyhow!("缺少图片数据"))?;
        let input = preprocess_image_data(&image_data)?;

        let logits_1 = run_model(&self.model_1_path, &input)?;
        let prob_1 = softmax(&logits_1);
        let (idx_1, conf_1) = argmax_with_confidence(&prob_1);

        if idx_1 == 0 {
            return Ok(DiseasePrediction {
                predicted_class: "非果树".to_string(),
                confidence: conf_1 * 100.0,
                stage: "model_1".to_string(),
                is_citrus_leaf: false,
                citrus_type: "非柑橘".to_string(),
                is_healthy: false,
                disease_name: "".to_string(),
                severity: "健康".to_string(),
                treatment_suggestion: "请上传清晰的柑橘叶片图片以便继续诊断".to_string(),
                preventive_measures: "确保拍摄主体为单片柑橘叶，光线充足、无遮挡".to_string(),
                image_quality_warning: "".to_string(),
            });
        }

        let logits_2 = run_model(&self.model_2_path, &input)?;
        let prob_2 = softmax(&logits_2);
        let (idx_2, conf_2) = argmax_with_confidence(&prob_2);
        let disease_classes = ["黄龙病", "健康果树", "溃疡病", "沙皮病"];
        let predicted = disease_classes
            .get(idx_2)
            .copied()
            .unwrap_or("健康果树")
            .to_string();

        let is_healthy = predicted == "健康果树";
        let disease_name = if is_healthy {
            "".to_string()
        } else {
            predicted.clone()
        };

        Ok(DiseasePrediction {
            predicted_class: predicted,
            confidence: conf_2 * 100.0,
            stage: "model_2".to_string(),
            is_citrus_leaf: true,
            citrus_type: "其他".to_string(),
            is_healthy,
            disease_name,
            severity: severity_from_confidence(conf_2),
            treatment_suggestion: treatment_by_class(disease_classes[idx_2]).to_string(),
            preventive_measures: "保持果园通风透光，定期巡查并清理病残体。".to_string(),
            image_quality_warning: "".to_string(),
        })
    }

    pub async fn predict_fruit_tree(
        &self,
        image_data: Option<String>,
    ) -> anyhow::Result<FruitTreeGatePrediction> {
        let image_data = image_data.ok_or_else(|| anyhow::anyhow!("缺少图片数据"))?;
        let input = preprocess_image_data(&image_data)?;

        let logits = run_model(&self.model_1_path, &input)?;
        let prob = softmax(&logits);
        let (idx, conf) = argmax_with_confidence(&prob);
        let is_fruit_tree = idx == 1;

        Ok(FruitTreeGatePrediction {
            is_fruit_tree,
            predicted_class: if is_fruit_tree {
                "是果树".to_string()
            } else {
                "非果树".to_string()
            },
            confidence: conf * 100.0,
        })
    }
}

fn preprocess_image_data(image_data: &str) -> anyhow::Result<Tensor> {
    let raw = decode_image_data(image_data)?;
    let image = image::load_from_memory(&raw)
        .map_err(|e| anyhow::anyhow!("解析图片失败: {}", e))?
        .to_rgb8();
    let resized = image::imageops::resize(&image, 224, 224, FilterType::Triangle);

    let mean = [0.485_f32, 0.456_f32, 0.406_f32];
    let std = [0.229_f32, 0.224_f32, 0.225_f32];

    let mut input = tract_ndarray::Array4::<f32>::zeros((1, 3, 224, 224));
    for y in 0..224 {
        for x in 0..224 {
            let pixel = resized.get_pixel(x as u32, y as u32);
            for channel in 0..3 {
                let value = pixel[channel] as f32 / 255.0;
                input[[0, channel, y, x]] = (value - mean[channel]) / std[channel];
            }
        }
    }

    Ok(input.into_tensor())
}

fn decode_image_data(image_data: &str) -> anyhow::Result<Vec<u8>> {
    let input = image_data.trim();
    let preview: String = input.chars().take(48).collect();
    let input_lower = input.to_ascii_lowercase();
    tracing::debug!(
        "onnx decode输入: len={} has_data_uri={} preview={}...",
        input.len(),
        input_lower.starts_with("data:"),
        preview
    );

    if let Some((header, encoded)) = input.split_once(',') {
        let header_lower = header.trim().to_ascii_lowercase();
        if header_lower.starts_with("data:") && header_lower.contains(";base64") {
            tracing::debug!(
                "onnx decode 走 data-uri 路径: header={} encoded_len={}",
                header,
                encoded.trim().len()
            );
            return base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .map_err(|e| anyhow::anyhow!("base64 图片解码失败: {}", e));
        }
    }

    if Path::new(input).exists() {
        tracing::debug!("onnx decode 走本地文件路径: {}", input);
        return std::fs::read(input).map_err(|e| anyhow::anyhow!("读取图片文件失败: {}", e));
    }

    tracing::debug!("onnx decode 走纯base64路径: len={}", input.len());

    base64::engine::general_purpose::STANDARD
        .decode(input)
        .map_err(|e| anyhow::anyhow!("base64 图片解码失败: {}", e))
}

fn run_model(model_path: &str, input: &Tensor) -> anyhow::Result<Vec<f32>> {
    if !Path::new(model_path).exists() {
        anyhow::bail!("ONNX 模型文件不存在: {}", model_path);
    }

    let model = tract_onnx::onnx()
        .model_for_path(model_path)
        .map_err(|e| anyhow::anyhow!("加载 ONNX 模型失败: {}", e))?
        .into_optimized()
        .map_err(|e| anyhow::anyhow!("优化 ONNX 模型失败: {}", e))?
        .into_runnable()
        .map_err(|e| anyhow::anyhow!("构建 ONNX 可执行模型失败: {}", e))?;

    let outputs = model
        .run(tvec!(input.clone().into()))
        .map_err(|e| anyhow::anyhow!("ONNX 推理失败: {}", e))?;
    let output = outputs
        .first()
        .ok_or_else(|| anyhow::anyhow!("ONNX 推理无输出"))?;

    let view = output
        .to_array_view::<f32>()
        .map_err(|e| anyhow::anyhow!("解析 ONNX 输出失败: {}", e))?;
    Ok(view.iter().copied().collect())
}

fn softmax(logits: &[f32]) -> Vec<f32> {
    if logits.is_empty() {
        return Vec::new();
    }
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exp_values: Vec<f32> = logits.iter().map(|v| (v - max).exp()).collect();
    let sum: f32 = exp_values.iter().sum();
    if sum == 0.0 {
        return vec![0.0; logits.len()];
    }
    exp_values.into_iter().map(|v| v / sum).collect()
}

fn argmax_with_confidence(probabilities: &[f32]) -> (usize, f64) {
    let (idx, conf) =
        probabilities
            .iter()
            .copied()
            .enumerate()
            .fold((0usize, 0.0f32), |acc, (idx, value)| {
                if value > acc.1 { (idx, value) } else { acc }
            });
    (idx, conf as f64)
}

fn severity_from_confidence(confidence: f64) -> String {
    if confidence >= 0.9 {
        "重度".to_string()
    } else if confidence >= 0.75 {
        "中度".to_string()
    } else if confidence >= 0.55 {
        "轻度".to_string()
    } else {
        "轻度".to_string()
    }
}

fn treatment_by_class(class_name: &str) -> &'static str {
    match class_name {
        "黄龙病" => "先控木虱再清除病株，及时消毒工具并加强检疫。",
        "溃疡病" => "采用药-剪-药策略，铜制剂防治并清理病枝病叶病果。",
        "沙皮病" => "加强清园与关键期保护性喷药，减少机械伤和日灼伤口。",
        "健康果树" => "树势正常，保持水肥平衡并持续病虫监测。",
        _ => "建议补拍清晰叶片图片后复检。",
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_image_data, preprocess_image_data, softmax};
    use base64::Engine;
    use image::{DynamicImage, ImageFormat, RgbImage};
    use std::io::Cursor;

    fn one_by_one_png_data_uri() -> String {
        let img = RgbImage::from_pixel(1, 1, image::Rgb([255, 255, 255]));
        let dyn_img = DynamicImage::ImageRgb8(img);
        let mut bytes = Vec::new();
        dyn_img
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .expect("encode png should succeed");
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    }

    #[test]
    fn decode_data_uri_ok() {
        let data_uri = one_by_one_png_data_uri();
        let bytes = decode_image_data(&data_uri).expect("decode should succeed");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn decode_data_uri_with_spaces_and_uppercase_ok() {
        let data_uri = one_by_one_png_data_uri();
        let encoded = data_uri.split(',').nth(1).unwrap_or_default();
        let noisy = format!("  DATA:IMAGE/PNG;BASE64,{}  \n", encoded);
        let bytes = decode_image_data(&noisy).expect("decode with spaces should succeed");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn decode_octet_stream_data_uri_ok() {
        let data_uri = one_by_one_png_data_uri();
        let encoded = data_uri.split(',').nth(1).unwrap_or_default();
        let octet = format!("data:application/octet-stream;base64,{}", encoded);
        let bytes = decode_image_data(&octet).expect("decode octet-stream data uri should succeed");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn preprocess_to_nchw_224_ok() {
        let data_uri = one_by_one_png_data_uri();
        let tensor = preprocess_image_data(&data_uri).expect("preprocess should succeed");
        assert_eq!(tensor.shape(), &[1, 3, 224, 224]);
    }

    #[test]
    fn softmax_sums_to_one() {
        let probs = softmax(&[1.0, 2.0, 3.0]);
        let sum: f32 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }
}
