#[derive(Debug, Clone)]
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
}

#[derive(Debug, Clone)]
pub struct FruitTreeGatePrediction {
    pub is_fruit_tree: bool,
    pub predicted_class: String,
    pub confidence: f64,
}
