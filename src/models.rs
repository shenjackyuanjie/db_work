use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: serde_json::Value,
}

/// OpenRouter 供应商偏好设置
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ProviderPreferences {
    /// 按优先级排序的供应商 ID 列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
    /// 是否允许使用未在 order 中列出的供应商作为回退
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_fallbacks: Option<bool>,
    /// 是否只使用 openrouter 计算的提供商（不直接发送到上游）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_parameters: Option<bool>,
    /// 按数据隐私排序（选择不训练模型的提供商）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_collection: Option<String>,
}

impl ProviderPreferences {
    pub fn with_order(providers: Vec<String>) -> Self {
        Self {
            order: Some(providers),
            allow_fallbacks: Some(true),
            require_parameters: None,
            data_collection: None,
        }
    }

    /// 只使用指定的供应商，不允许回退
    pub fn strict_order(providers: Vec<String>) -> Self {
        Self {
            order: Some(providers),
            allow_fallbacks: Some(false),
            require_parameters: None,
            data_collection: None,
        }
    }
}

/// OpenRouter 路由配置
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum RouteConfig {
    /// 默认路由
    Fallback,
    /// 最大化吞吐量
    Nitro,
    /// 最低价格
    Floor,
}

impl Default for RouteConfig {
    fn default() -> Self {
        RouteConfig::Fallback
    }
}

#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    /// 是否使用流式传输
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// 供应商偏好设置
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderPreferences>,
    /// 请求/响应转换（如 ["middle-out"] 用于截断长对话）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transforms: Option<Vec<String>>,
}

/// 结构化输出格式
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResponseFormat {
    #[serde(rename = "type")]
    pub format_type: String,
    /// JSON Schema 定义（用于 strict JSON 模式）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<serde_json::Value>,
}

impl ResponseFormat {
    /// 创建 JSON 对象格式的结构化输出
    pub fn json_object() -> Self {
        Self {
            format_type: "json_object".to_string(),
            json_schema: None,
        }
    }

    /// 创建带有 JSON Schema 的结构化输出
    pub fn with_schema(schema: serde_json::Value) -> Self {
        Self {
            format_type: "json_object".to_string(),
            json_schema: Some(schema),
        }
    }
}

/// OpenRouter 错误响应
#[derive(Debug, Deserialize)]
pub struct OpenRouterError {
    pub message: String,
    pub code: Option<i32>,
    pub metadata: Option<serde_json::Value>,
}

/// OpenRouter 错误响应包装
#[derive(Debug, Deserialize)]
pub struct OpenRouterErrorResponse {
    pub error: OpenRouterError,
}

/// OpenRouter 特定的 usage 信息（包含生成 tokens 的详情）
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GenerationDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicted: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ChatChoice {
    pub index: i32,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
    /// OpenRouter 特定：是否为原生 finish_reason
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_finish_reason: Option<String>,
    /// OpenRouter 特定：logprobs（如果请求中要求）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    /// OpenRouter 特定：生成 tokens 的详细信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_details: Option<GenerationDetails>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: Usage,
    /// OpenRouter 特定：实际使用的供应商
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatApiRequest {
    pub message: Option<String>,
    pub image: Option<String>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<u32>,
    /// 指定首选供应商（如 "Moonshot AI", "Together", "Fireworks" 等）
    pub preferred_provider: Option<String>,
    /// 是否启用流式传输
    pub stream: Option<bool>,
}

// 柑橘分析结构化输出（新 schema）
#[derive(Debug, Serialize, Deserialize)]
pub struct CitrusAnalysisResult {
    /// 是否为柑橘叶片
    pub is_citrus_leaf: bool,
    /// 识别的柑橘类型（非柑橘时为 "非柑橘"）
    pub citrus_type: CitrusType,
    /// 病害分析
    pub disease_analysis: DiseaseAnalysis,
    /// 图片质量告警（如：模糊/过暗/遮挡/反光/主体过小 等；无则可为空字符串）
    pub image_quality_warning: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiseaseAnalysis {
    pub is_healthy: bool,
    pub disease_name: String,
    pub severity: Severity,
    /// 置信度 0~1
    pub confidence: f64,
    pub treatment_suggestion: String,
    pub preventive_measures: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum CitrusType {
    脐橙,
    砂糖橘,
    柚子,
    柠檬,
    其他,
    非柑橘,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Severity {
    健康,
    轻度,
    中度,
    重度,
}

/// 柑橘分析完整响应（包含分析结果、使用量、性能指标）
#[derive(Debug, Serialize, Deserialize)]
pub struct CitrusAnalysisResponse {
    pub success: bool,
    pub data: CitrusAnalysisResult,
    pub usage: Usage,
    pub metrics: AnalysisMetrics,
}

/// 分析性能指标
#[derive(Debug, Serialize, Deserialize)]
pub struct AnalysisMetrics {
    pub duration_secs: f64,
    pub tokens_per_sec: f64,
}
