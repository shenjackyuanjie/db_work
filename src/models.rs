use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: serde_json::Value,
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
}

/// 结构化输出格式
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResponseFormat {
    #[serde(rename = "type")]
    pub format_type: String,
}

impl ResponseFormat {
    /// 创建 JSON 对象格式的结构化输出
    pub fn json_object() -> Self {
        Self {
            format_type: "json_object".to_string(),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ChatChoice {
    pub index: i32,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
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
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatApiRequest {
    pub message: Option<String>,
    pub image: Option<String>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<u32>,
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
