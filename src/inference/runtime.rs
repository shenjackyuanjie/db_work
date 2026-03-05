use crate::{client::OpenRouterClient, config::InferenceMode};

use super::{onnx::OnnxInference, remote::RemoteInference, types::DiseasePrediction};

#[derive(Clone)]
pub enum InferenceRuntime {
    Remote(RemoteInference),
    Onnx(OnnxInference),
}

impl InferenceRuntime {
    pub fn new(config: &crate::config::InferenceConfig, client: OpenRouterClient) -> Self {
        match config.mode {
            InferenceMode::Remote => Self::Remote(RemoteInference::new(
                client,
                config.model_1_path.clone(),
                config.model_2_path.clone(),
            )),
            InferenceMode::Onnx => Self::Onnx(OnnxInference::new(
                config.model_1_path.clone(),
                config.model_2_path.clone(),
            )),
        }
    }

    pub async fn predict_citrus_disease(
        &self,
        image_data: Option<String>,
    ) -> anyhow::Result<DiseasePrediction> {
        match self {
            InferenceRuntime::Remote(remote) => remote.predict_citrus_disease(image_data).await,
            InferenceRuntime::Onnx(onnx) => onnx.predict_citrus_disease(image_data).await,
        }
    }
}
