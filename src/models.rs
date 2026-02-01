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
    pub message: String,
    pub image: Option<String>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<u32>,
    /// 结构化输出格式，可选值为 "json_object" 或 null
    /// 设置为 "json_object" 时，模型将返回 JSON 格式的结构化数据
    pub response_format: Option<String>,
}

// 柑橘分析结构化输出
#[derive(Debug, Serialize, Deserialize)]
pub struct CitrusAnalysisResult {
    pub is_leaf: bool,           // 是否叶子
    pub is_citrus: bool,         // 是否柑橘
    pub disease_info: Option<DiseaseInfo>,  // 病害信息
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiseaseInfo {
    pub has_disease: bool,       // 是否有病
    pub severity: Option<String>, // 病症程度
    pub solution: Option<String>, // 可能解决方案
}