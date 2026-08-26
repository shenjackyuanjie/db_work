use crate::{client::OpenRouterClient, config::InferenceMode};

use super::{
    onnx::OnnxInference,
    remote::RemoteInference,
    types::{DiseasePrediction, FruitTreeGatePrediction},
};

#[derive(Clone)]
pub enum InferenceRuntime {
    Remote(RemoteInference),
    Onnx(OnnxInference),
}

impl InferenceRuntime {
    pub fn new(
        config: &crate::config::InferenceConfig,
        client: OpenRouterClient,
    ) -> anyhow::Result<Self> {
        match config.mode {
            InferenceMode::Remote => Ok(Self::Remote(RemoteInference::new(
                client,
                config.model_1_path.clone(),
                config.model_2_path.clone(),
            )?)),
            InferenceMode::Onnx => Ok(Self::Onnx(OnnxInference::new(
                config.model_1_path.clone(),
                config.model_2_path.clone(),
            )?)),
        }
    }

    pub async fn predict_citrus_disease(
        &self,
        image_data: Option<String>,
        temperature: Option<f64>,
        humidity: Option<f64>,
    ) -> anyhow::Result<DiseasePrediction> {
        match self {
            InferenceRuntime::Remote(remote) => remote.predict_citrus_disease(image_data).await,
            InferenceRuntime::Onnx(onnx) => {
                onnx.predict_citrus_disease(image_data, temperature, humidity)
                    .await
            }
        }
    }

    /// 使用 model_1 判断是否为果树（用于混合推理流程的第一步）
    pub async fn predict_fruit_tree(
        &self,
        image_data: Option<String>,
    ) -> anyhow::Result<FruitTreeGatePrediction> {
        match self {
            InferenceRuntime::Remote(remote) => remote.predict_fruit_tree(image_data).await,
            InferenceRuntime::Onnx(onnx) => onnx.predict_fruit_tree(image_data).await,
        }
    }
}
