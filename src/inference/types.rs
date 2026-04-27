use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct ClimateScore {
    pub suitability: f64,
    pub temperature_score: Option<f64>,
    pub humidity_score: Option<f64>,
    pub multiplier: f64,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClimateValidationResult {
    pub used: bool,
    pub reason: Option<String>,
    pub temperature: Option<f64>,
    pub humidity: Option<f64>,
    pub image_predicted_class: Option<String>,
    pub image_confidence: Option<f64>,
    pub adjusted_predicted_class: Option<String>,
    pub adjusted_confidence: Option<f64>,
    pub support_level: Option<String>,
    pub class_scores: Option<BTreeMap<String, ClimateScore>>,
    pub adjusted_probabilities: Option<Vec<f64>>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiseasePrediction {
    pub predicted_class: String,
    pub confidence: f64,
    pub stage: String,
    pub is_citrus_leaf: bool,
    pub citrus_type: String,
    pub is_healthy: bool,
    pub disease_name: String,
    pub severity: String,
    pub treatment_suggestion: String,
    pub preventive_measures: String,
    pub image_quality_warning: String,
    pub image_predicted_class: Option<String>,
    pub image_confidence: Option<f64>,
    pub climate_validation: Option<ClimateValidationResult>,
}

#[derive(Debug, Clone)]
pub struct FruitTreeGatePrediction {
    pub is_fruit_tree: bool,
    pub predicted_class: String,
    pub confidence: f64,
}
