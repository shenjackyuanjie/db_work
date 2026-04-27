mod math;

/// 正常柑橘环境温湿度监测范围（由各病害 ClimateProfile support 范围联合推导）
/// 温度范围：15.0 ~ 41.6 °C
/// 湿度范围：40.0 ~ 100.0 %
pub const NORMAL_TEMP_RANGE: (f64, f64) = (15.0, 41.6);
pub const NORMAL_HUMIDITY_RANGE: (f64, f64) = (40.0, 100.0);

/// 判断温湿度读数是否在正常柑橘环境监测范围内。
/// 在范围内返回 true（正常），超出范围返回 false（异常）。
pub fn is_climate_in_range(temperature: f64, humidity: f64) -> bool {
    temperature >= NORMAL_TEMP_RANGE.0
        && temperature <= NORMAL_TEMP_RANGE.1
        && humidity >= NORMAL_HUMIDITY_RANGE.0
        && humidity <= NORMAL_HUMIDITY_RANGE.1
}

use super::types::{
    ClimateScore, ClimateValidationResult, DiseasePrediction, FruitTreeGatePrediction,
};
use base64::Engine;
use image::imageops::FilterType;
use std::collections::BTreeMap;
use std::path::Path;
use tract_onnx::prelude::*;

use math::{argmax_with_confidence, softmax};

const CLIMATE_INFLUENCE: f64 = 0.25;
const DISEASE_CLASSES: [&str; 4] = ["黄龙病", "健康果树", "溃疡病", "沙皮病"];

#[derive(Clone, Copy)]
struct ClimateProfile {
    temp_support: (f64, f64),
    temp_peak: (f64, f64),
    humidity_support: (f64, f64),
    humidity_peak: (f64, f64),
    temp_weight: f64,
    humidity_weight: f64,
    note: &'static str,
}

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
        temperature: Option<f64>,
        humidity: Option<f64>,
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
                image_predicted_class: None,
                image_confidence: None,
                climate_validation: None,
            });
        }

        let logits_2 = run_model(&self.model_2_path, &input)?;
        let prob_2 = softmax(&logits_2);
        let (idx_2, conf_2) = argmax_with_confidence(&prob_2);
        let image_predicted = DISEASE_CLASSES
            .get(idx_2)
            .copied()
            .unwrap_or("健康果树")
            .to_string();

        let image_confidence = conf_2 * 100.0;
        let climate_validation = validate_climate(
            &DISEASE_CLASSES,
            &prob_2,
            &image_predicted,
            image_confidence,
            temperature,
            humidity,
        );
        let predicted = climate_validation
            .adjusted_predicted_class
            .clone()
            .filter(|_| climate_validation.used)
            .unwrap_or_else(|| image_predicted.clone());
        let confidence = climate_validation
            .adjusted_confidence
            .filter(|_| climate_validation.used)
            .unwrap_or(image_confidence);
        let is_healthy = predicted == "健康果树";
        let disease_name = if is_healthy {
            "".to_string()
        } else {
            predicted.clone()
        };

        Ok(DiseasePrediction {
            predicted_class: predicted.clone(),
            confidence,
            stage: "model_2".to_string(),
            is_citrus_leaf: true,
            citrus_type: "其他".to_string(),
            is_healthy,
            disease_name,
            severity: severity_from_confidence(confidence / 100.0),
            treatment_suggestion: treatment_by_class(&predicted).to_string(),
            preventive_measures: "保持果园通风透光，定期巡查并清理病残体。".to_string(),
            image_quality_warning: "".to_string(),
            image_predicted_class: Some(image_predicted),
            image_confidence: Some(image_confidence),
            climate_validation: Some(climate_validation),
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

fn climate_profile(label: &str) -> Option<ClimateProfile> {
    match label {
        "黄龙病" => Some(ClimateProfile {
            temp_support: (16.0, 41.6),
            temp_peak: (27.0, 32.0),
            humidity_support: (40.0, 100.0),
            humidity_peak: (60.0, 90.0),
            temp_weight: 0.75,
            humidity_weight: 0.25,
            note: "黄龙病主要由亚洲柑橘木虱传播，即时温湿度只作为媒介活跃度的弱佐证。",
        }),
        "溃疡病" => Some(ClimateProfile {
            temp_support: (20.0, 35.0),
            temp_peak: (25.0, 30.0),
            humidity_support: (50.0, 100.0),
            humidity_peak: (70.0, 100.0),
            temp_weight: 0.55,
            humidity_weight: 0.45,
            note: "溃疡病在温暖、高湿、降雨或风雨传播条件下更易发生。",
        }),
        "沙皮病" => Some(ClimateProfile {
            temp_support: (15.0, 35.0),
            temp_peak: (24.0, 28.0),
            humidity_support: (70.0, 100.0),
            humidity_peak: (80.0, 100.0),
            temp_weight: 0.55,
            humidity_weight: 0.45,
            note: "沙皮病/黑点病受持续湿润、雨水传播和温暖气候影响明显。",
        }),
        _ => None,
    }
}

fn trapezoid_score(value: Option<f64>, support: (f64, f64), peak: (f64, f64)) -> Option<f64> {
    let value = value?;
    let (support_low, support_high) = support;
    let (peak_low, peak_high) = peak;

    if value < support_low || value > support_high {
        return Some(0.0);
    }
    if (peak_low..=peak_high).contains(&value) {
        return Some(1.0);
    }
    if value < peak_low {
        return Some((value - support_low) / (peak_low - support_low));
    }

    Some((support_high - value) / (support_high - peak_high))
}

fn weighted_average(scores: &[(Option<f64>, f64)]) -> f64 {
    let mut total_weight = 0.0;
    let mut total_score = 0.0;

    for (score, weight) in scores {
        if let Some(score) = score {
            total_weight += weight;
            total_score += score * weight;
        }
    }

    if total_weight == 0.0 {
        return 0.5;
    }

    (total_score / total_weight).clamp(0.0, 1.0)
}

fn climate_score(label: &str, temperature: Option<f64>, humidity: Option<f64>) -> ClimateScore {
    let Some(profile) = climate_profile(label) else {
        return ClimateScore {
            suitability: 0.5,
            temperature_score: None,
            humidity_score: None,
            multiplier: 1.0,
            note: "该类别不使用温湿度规则校验。".to_string(),
        };
    };

    let temperature_score = trapezoid_score(temperature, profile.temp_support, profile.temp_peak);
    let humidity_score = trapezoid_score(humidity, profile.humidity_support, profile.humidity_peak);
    let suitability = weighted_average(&[
        (temperature_score, profile.temp_weight),
        (humidity_score, profile.humidity_weight),
    ]);
    let multiplier = 1.0 + CLIMATE_INFLUENCE * (2.0 * suitability - 1.0);

    ClimateScore {
        suitability: round4(suitability),
        temperature_score: temperature_score.map(round4),
        humidity_score: humidity_score.map(round4),
        multiplier: round4(multiplier),
        note: profile.note.to_string(),
    }
}

fn validate_climate(
    classes: &[&str],
    probabilities: &[f32],
    predicted_class: &str,
    confidence: f64,
    temperature: Option<f64>,
    humidity: Option<f64>,
) -> ClimateValidationResult {
    if temperature.is_none() && humidity.is_none() {
        return ClimateValidationResult {
            used: false,
            reason: Some("未提供温度或湿度，跳过温湿度校验。".to_string()),
            temperature,
            humidity,
            image_predicted_class: Some(predicted_class.to_string()),
            image_confidence: Some(confidence),
            adjusted_predicted_class: None,
            adjusted_confidence: None,
            support_level: None,
            class_scores: None,
            adjusted_probabilities: None,
            message: None,
        };
    }

    let mut scores = BTreeMap::new();
    for label in classes {
        scores.insert(
            (*label).to_string(),
            climate_score(label, temperature, humidity),
        );
    }

    let weighted: Vec<f64> = classes
        .iter()
        .zip(probabilities.iter())
        .map(|(label, probability)| {
            let score = scores
                .get(*label)
                .map(|item| item.multiplier)
                .unwrap_or(1.0);
            f64::from(*probability) * score
        })
        .collect();
    let total: f64 = weighted.iter().sum();
    let adjusted_probabilities: Vec<f64> = if total > 0.0 {
        weighted.iter().map(|value| value / total).collect()
    } else {
        vec![0.0; weighted.len()]
    };

    let (adjusted_index, adjusted_confidence) = adjusted_probabilities
        .iter()
        .copied()
        .enumerate()
        .fold((0usize, 0.0_f64), |acc, (index, value)| {
            if value > acc.1 { (index, value) } else { acc }
        });
    let adjusted_class = classes
        .get(adjusted_index)
        .copied()
        .unwrap_or("健康果树")
        .to_string();
    let predicted_suitability = scores
        .get(predicted_class)
        .map(|item| item.suitability)
        .unwrap_or(0.5);

    ClimateValidationResult {
        used: true,
        reason: None,
        temperature,
        humidity,
        image_predicted_class: Some(predicted_class.to_string()),
        image_confidence: Some(confidence),
        adjusted_predicted_class: Some(adjusted_class.clone()),
        adjusted_confidence: Some(adjusted_confidence * 100.0),
        support_level: Some(support_level(predicted_suitability).to_string()),
        class_scores: Some(scores),
        adjusted_probabilities: Some(adjusted_probabilities),
        message: Some(climate_message(
            predicted_class,
            &adjusted_class,
            predicted_suitability,
        )),
    }
}

fn support_level(suitability: f64) -> &'static str {
    if suitability >= 0.75 {
        "strong"
    } else if suitability >= 0.45 {
        "medium"
    } else if suitability > 0.0 {
        "weak"
    } else {
        "none"
    }
}

fn climate_message(image_class: &str, adjusted_class: &str, suitability: f64) -> String {
    if image_class != adjusted_class {
        return format!(
            "温湿度条件更支持{}，图片模型原始结果为{}，建议人工复核或结合近期降雨、田间病史判断。",
            adjusted_class, image_class
        );
    }
    if suitability >= 0.75 {
        return format!("温湿度条件对{}形成强佐证。", image_class);
    }
    if suitability >= 0.45 {
        return format!("温湿度条件对{}形成中等佐证。", image_class);
    }
    if suitability > 0.0 {
        return format!("温湿度条件对{}佐证较弱。", image_class);
    }

    format!(
        "当前温湿度不支持{}的高发条件，建议复核图片结果。",
        image_class
    )
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests;
