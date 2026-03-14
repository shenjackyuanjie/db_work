use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub ai: AiConfig,
    pub database: DatabaseConfig,
    #[serde(default)]
    pub inference: InferenceConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub addr: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default)]
    pub proxy: ProxyConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub target_url: Option<String>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            target_url: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AiConfig {
    pub openrouter_api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub postgres_url: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InferenceMode {
    Remote,
    Onnx,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InferenceConfig {
    #[serde(default = "default_inference_mode")]
    pub mode: InferenceMode,
    #[serde(default = "default_model_1_path")]
    pub model_1_path: String,
    #[serde(default = "default_model_2_path")]
    pub model_2_path: String,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_inference_mode() -> InferenceMode {
    InferenceMode::Remote
}

fn default_model_1_path() -> String {
    "../navel_back/model/onnx/model_1.onnx".to_string()
}

fn default_model_2_path() -> String {
    "../navel_back/model/onnx/model_2.onnx".to_string()
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            mode: default_inference_mode(),
            model_1_path: default_model_1_path(),
            model_2_path: default_model_2_path(),
        }
    }
}

impl AppConfig {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("读取配置文件失败({}): {}", path, e))?;
        let config: AppConfig =
            toml::from_str(&content).map_err(|e| anyhow::anyhow!("解析配置文件失败: {}", e))?;
        Ok(config)
    }
}
