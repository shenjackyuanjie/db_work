use crate::client::OpenRouterClient;

use super::{
    onnx::OnnxInference,
    types::{DiseasePrediction, FruitTreeGatePrediction},
};

#[derive(Clone)]
pub struct RemoteInference {
    client: OpenRouterClient,
    pub fruit_tree_gate: OnnxInference,
}

impl RemoteInference {
    pub fn new(client: OpenRouterClient, model_1_path: String, model_2_path: String) -> Self {
        Self {
            client,
            fruit_tree_gate: OnnxInference::new(model_1_path, model_2_path),
        }
    }

    pub async fn predict_fruit_tree(
        &self,
        image_data: Option<String>,
    ) -> anyhow::Result<FruitTreeGatePrediction> {
        self.fruit_tree_gate.predict_fruit_tree(image_data).await
    }

    pub async fn predict_citrus_disease(
        &self,
        image_data: Option<String>,
    ) -> anyhow::Result<DiseasePrediction> {
        let gate = self
            .fruit_tree_gate
            .predict_fruit_tree(image_data.clone())
            .await?;

        if !gate.is_fruit_tree {
            return Ok(DiseasePrediction {
                predicted_class: gate.predicted_class,
                confidence: gate.confidence,
                stage: "model_1".to_string(),
                is_citrus_leaf: false,
                citrus_type: "非柑橘".to_string(),
                is_healthy: false,
                disease_name: String::new(),
                severity: "健康".to_string(),
                treatment_suggestion: "请上传清晰的果树叶片图片以便继续诊断".to_string(),
                preventive_measures: "确保拍摄主体为单片叶片，光线充足、无遮挡。".to_string(),
                image_quality_warning: String::new(),
                image_predicted_class: None,
                image_confidence: None,
                climate_validation: None,
            });
        }

        let response = self.client.analyze_citrus(image_data).await?;
        let analysis = response.data;

        let predicted_class = if !analysis.is_citrus_leaf {
            "非果树".to_string()
        } else if analysis.disease_analysis.is_healthy {
            "健康果树".to_string()
        } else {
            analysis.disease_analysis.disease_name.clone()
        };

        Ok(DiseasePrediction {
            predicted_class,
            confidence: analysis.disease_analysis.confidence * 100.0,
            stage: "openrouter".to_string(),
            is_citrus_leaf: analysis.is_citrus_leaf,
            citrus_type: format!("{:?}", analysis.citrus_type),
            is_healthy: analysis.disease_analysis.is_healthy,
            disease_name: analysis.disease_analysis.disease_name,
            severity: format!("{:?}", analysis.disease_analysis.severity),
            treatment_suggestion: analysis.disease_analysis.treatment_suggestion,
            preventive_measures: analysis.disease_analysis.preventive_measures,
            image_quality_warning: analysis.image_quality_warning,
            image_predicted_class: None,
            image_confidence: None,
            climate_validation: None,
        })
    }
}
