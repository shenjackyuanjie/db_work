use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use crate::models::{ChatMessage, ChatRequest, ChatResponse, ResponseFormat};
use serde_json::{json, Value};
use std::time::Instant;

/// 通用聊天调用选项
#[derive(Debug, Clone, Default)]
pub struct ChatOptions {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<u32>,
    pub response_format: Option<ResponseFormat>,
}

impl ChatOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn temperature(mut self, val: f64) -> Self {
        self.temperature = Some(val);
        self
    }

    pub fn top_p(mut self, val: f64) -> Self {
        self.top_p = Some(val);
        self
    }

    pub fn max_tokens(mut self, val: u32) -> Self {
        self.max_tokens = Some(val);
        self
    }

    pub fn json_response(mut self) -> Self {
        self.response_format = Some(ResponseFormat::json_object());
        self
    }

    pub fn with_system_message(mut self, _message: &str) -> Self {
        // 标记需要 system message，实际内容在调用时处理
        self
    }
}

/// 简化的聊天请求参数
#[derive(Debug, Clone)]
pub struct SimpleChatRequest {
    pub message: String,
    pub image: Option<String>,
    pub options: ChatOptions,
    pub system_message: Option<String>,
}

impl SimpleChatRequest {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            image: None,
            options: ChatOptions::default(),
            system_message: None,
        }
    }

    pub fn with_image(mut self, image_path: impl Into<String>) -> Self {
        self.image = Some(image_path.into());
        self
    }

    pub fn with_options(mut self, options: ChatOptions) -> Self {
        self.options = options;
        self
    }

    pub fn with_system(mut self, message: impl Into<String>) -> Self {
        self.system_message = Some(message.into());
        self
    }
}

/// 聊天响应结果（简化版）
#[derive(Debug)]
pub struct SimpleChatResponse {
    pub id: String,
    pub model: String,
    pub content: String,
    pub usage: crate::models::Usage,
}

#[derive(Clone)]
pub struct GlmClient {
    api_key: String,
    base_url: String,
    model: String,
}

impl GlmClient {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://open.bigmodel.cn/api/paas/v4/chat/completions".to_string(),
            model: "glm-4.6v-flash".to_string(),
        }
    }

    /// 设置默认模型
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// 底层 API 调用
    pub async fn chat_completions(&self, request: &ChatRequest) -> Result<ChatResponse, Box<dyn std::error::Error>> {
        let client = reqwest::Client::new();

        let response = client
            .post(&self.base_url)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key))
            .json(request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            return Err(format!("API request failed with status {}: {}", status, error_text).into());
        }

        let chat_response: ChatResponse = response.json().await?;
        Ok(chat_response)
    }

    /// 简化的聊天接口（统一 CLI 和 Server 的调用方式）
    pub async fn chat(&self, request: SimpleChatRequest) -> Result<SimpleChatResponse, Box<dyn std::error::Error>> {
        let content = crate::utils::build_message_content(&request.message, request.image.as_ref());

        let mut messages = Vec::new();
        
        // 添加 system message（如果有）
        if let Some(system) = request.system_message {
            messages.push(ChatMessage {
                role: "system".to_string(),
                content: serde_json::Value::String(system),
            });
        }

        messages.push(ChatMessage {
            role: "user".to_string(),
            content,
        });

        let chat_request = ChatRequest {
            model: self.model.clone(),
            messages,
            temperature: request.options.temperature,
            top_p: request.options.top_p,
            max_tokens: request.options.max_tokens,
            response_format: request.options.response_format,
        };

        let response = self.chat_completions(&chat_request).await?;
        
        let content = response.choices
            .first()
            .map(|choice| match &choice.message.content {
                serde_json::Value::String(s) => s.clone(),
                _ => choice.message.content.to_string(),
            })
            .unwrap_or_default();

        Ok(SimpleChatResponse {
            id: response.id,
            model: response.model,
            content,
            usage: response.usage,
        })
    }

    /// 将 SimpleChatResponse 转为统一的 JSON（供 server/cli 复用）
    fn simple_chat_response_to_json(resp: &SimpleChatResponse) -> Value {
        json!({
            "id": resp.id,
            "model": resp.model,
            "message": resp.content,
            "usage": resp.usage,
        })
    }

    /// 从 HTTP API 风格的 ChatApiRequest 统一执行聊天并返回 SimpleChatResponse（供 CLI 使用，保留原有调试字段）
    pub async fn exec_chat_from_api(&self, api_req: crate::models::ChatApiRequest) -> Result<SimpleChatResponse, Box<dyn std::error::Error>> {
        // 构建 ChatOptions
        let mut options = ChatOptions::new();
        if let Some(t) = api_req.temperature {
            options = options.temperature(t);
        }
        if let Some(tp) = api_req.top_p {
            options = options.top_p(tp);
        }
        if let Some(m) = api_req.max_tokens {
            options = options.max_tokens(m);
        }
        if let Some(ref fmt) = api_req.response_format {
            if fmt == "json_object" {
                options = options.json_response();
            }
        }

        let request = SimpleChatRequest {
            message: api_req.message,
            image: api_req.image,
            options,
            system_message: None,
        };

        let resp = self.chat(request).await?;
        Ok(resp)
    }

    /// 供 server 使用的包装方法：接受 ChatApiRequest，返回 serde_json::Value（id/model/message/usage + metrics）
    pub async fn exec_chat_api(&self, api_req: crate::models::ChatApiRequest) -> Result<Value, Box<dyn std::error::Error>> {
        let start_time = Instant::now();
        let resp = self.exec_chat_from_api(api_req).await?;
        let duration = start_time.elapsed();

        let total_tokens = resp.usage.total_tokens as f64;
        let tps = if duration.as_secs_f64() > 0.0 {
            total_tokens / duration.as_secs_f64()
        } else {
            0.0
        };

        let mut json_val = Self::simple_chat_response_to_json(&resp);
        if let Some(obj) = json_val.as_object_mut() {
            obj.insert(
                "metrics".to_string(),
                json!({
                    "duration_secs": duration.as_secs_f64(),
                    "tokens_per_sec": tps
                }),
            );
        }

        Ok(json_val)
    }

    /// 供 server 使用：执行柑橘分析并返回 JSON 值
    pub async fn exec_analyze_citrus(&self, message: String, image: Option<String>) -> Result<Value, Box<dyn std::error::Error>> {
        match self.analyze_citrus(message, image).await {
            Ok(analysis) => Ok(json!({
                "success": true,
                "data": analysis,
            })),
            Err(e) => Err(e),
        }
    }

    /// 柑橘分析专用接口
    pub async fn analyze_citrus(
        &self, 
        message: impl Into<String>, 
        image: Option<impl Into<String>>
    ) -> Result<crate::models::CitrusAnalysisResult, Box<dyn std::error::Error>> {
        const CITRUS_SYSTEM_MESSAGE: &str = r#"你是柑橘方面专家。

请分析用户上传的图片，并按JSON格式返回以下结构，不要返回其他内容：
{
    "is_leaf": true/false,      // 识别是否叶子
    "is_citrus": true/false,    // 识别是否柑橘
    "disease_info": {           // 如有病则填写，无病则为null
        "has_disease": true/false,
        "severity": "轻/中/重或具体描述",  // 病症程度
        "solution": "可能的解决方案"        // 可能解决方案
    }
}"#;

        let request = SimpleChatRequest::new(message)
            .with_options(
                ChatOptions::new()
                    .temperature(0.1)
                    .top_p(0.9)
                    .json_response()
            )
            .with_system(CITRUS_SYSTEM_MESSAGE);

        let request = match image {
            Some(img) => request.with_image(img),
            None => request,
        };

        let response = self.chat(request).await?;
        
        let result: crate::models::CitrusAnalysisResult = serde_json::from_str(&response.content)
            .map_err(|e| format!("无法解析模型返回的JSON: {}, raw: {}", e, response.content))?;
        
        Ok(result)
    }
}