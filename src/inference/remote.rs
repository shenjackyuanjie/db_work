use crate::client::OpenRouterClient;

use super::types::DiseasePrediction;

#[derive(Clone)]
pub struct RemoteInference {
    client: OpenRouterClient,
}

impl RemoteInference {
    pub fn new(client: OpenRouterClient) -> Self {
        Self { client }
    }

    pub async fn predict_citrus_disease(
        &self,
        image_data: Option<String>,
    ) -> anyhow::Result<DiseasePrediction> {
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
            confidence: (analysis.disease_analysis.confidence * 100.0).round(),
            stage: "model_2".to_string(),
            is_citrus_leaf: analysis.is_citrus_leaf,
            citrus_type: format!("{:?}", analysis.citrus_type),
            is_healthy: analysis.disease_analysis.is_healthy,
            disease_name: analysis.disease_analysis.disease_name,
            severity: format!("{:?}", analysis.disease_analysis.severity),
            treatment_suggestion: analysis.disease_analysis.treatment_suggestion,
            preventive_measures: analysis.disease_analysis.preventive_measures,
            image_quality_warning: analysis.image_quality_warning,
        })
    }
}
