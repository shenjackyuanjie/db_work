use crate::system_settings::load_system_settings;

use super::super::AppState;

pub(super) async fn confidence_threshold_percent(state: &AppState) -> f64 {
    load_system_settings(&state.db)
        .await
        .map(|settings| settings.confidence_threshold_percent())
        .unwrap_or(75.0)
}

pub(super) fn apply_review_threshold_to_prediction(
    prediction: &mut crate::inference::types::DiseasePrediction,
    threshold_percent: f64,
) -> bool {
    if prediction.predicted_class == "非果树" || prediction.confidence >= threshold_percent {
        return false;
    }

    prediction.predicted_class = "待人工复核".to_string();
    prediction.is_healthy = false;
    prediction.disease_name = "待人工复核".to_string();
    prediction.severity = "待复核".to_string();
    prediction.treatment_suggestion =
        "当前识别结果低于管理员设定的置信度阈值，请重新拍摄或人工复核。".to_string();
    prediction.preventive_measures =
        "建议保证叶片主体清晰、居中、无遮挡，并在自然光下重新采集图片。".to_string();
    if prediction.image_quality_warning.is_empty() {
        prediction.image_quality_warning = "识别置信度较低，建议复核".to_string();
    }

    true
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_review_threshold_to_fields(
    predicted_class: &mut String,
    is_healthy: &mut bool,
    disease_name: &mut String,
    severity: &mut String,
    treatment_suggestion: &mut String,
    preventive_measures: &mut String,
    image_quality_warning: &mut String,
    confidence: f64,
    threshold_percent: f64,
) -> bool {
    if predicted_class == "非果树" || confidence >= threshold_percent {
        return false;
    }

    *predicted_class = "待人工复核".to_string();
    *is_healthy = false;
    *disease_name = "待人工复核".to_string();
    *severity = "待复核".to_string();
    *treatment_suggestion =
        "当前识别结果低于管理员设定的置信度阈值，请重新拍摄或人工复核。".to_string();
    *preventive_measures =
        "建议保证叶片主体清晰、居中、无遮挡，并在自然光下重新采集图片。".to_string();
    if image_quality_warning.is_empty() {
        *image_quality_warning = "识别置信度较低，建议复核".to_string();
    }

    true
}
