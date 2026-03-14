use crate::models::{
    ChatMessage, ChatRequest, ChatResponse, CitrusAnalysisResponse, ResponseFormat,
};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName};
use serde_json::Value;
use std::str::FromStr;
use std::time::Instant;

/// 通用聊天调用选项
#[derive(Debug, Clone, Default)]
pub struct ChatOptions {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<u32>,
    pub response_format: Option<ResponseFormat>,
    /// 是否使用流式传输
    pub stream: Option<bool>,
    /// 供应商偏好设置
    pub provider: Option<crate::models::ProviderPreferences>,
    /// 请求转换（如 ["middle-out"]）
    pub transforms: Option<Vec<String>>,
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

    /// 启用流式传输
    pub fn stream(mut self, enabled: bool) -> Self {
        self.stream = Some(enabled);
        self
    }

    /// 设置供应商偏好
    pub fn with_provider(mut self, provider: crate::models::ProviderPreferences) -> Self {
        self.provider = Some(provider);
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
#[allow(dead_code)]
pub struct SimpleChatResponse {
    pub id: String,
    pub model: String,
    pub content: String,
    pub usage: crate::models::Usage,
    /// 实际使用的供应商
    pub provider: Option<String>,
}

#[derive(Clone)]
pub struct OpenRouterClient {
    api_key: String,
    base_url: String,
    model: String,
    /// 请求超时时间（秒）
    timeout_secs: u64,
    /// 默认供应商偏好
    default_provider: Option<crate::models::ProviderPreferences>,
}

impl OpenRouterClient {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://openrouter.ai/api/v1/chat/completions".to_string(),
            // 默认使用 Kimi K2.5，支持图像理解
            model: "moonshotai/kimi-k2.5".to_string(),
            timeout_secs: 60,
            default_provider: None,
        }
    }

    /// 构建请求头
    fn build_headers(&self) -> anyhow::Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, "application/json".parse()?);
        headers.insert(AUTHORIZATION, format!("Bearer {}", self.api_key).parse()?);

        // OpenRouter 推荐使用这些 Header 来标识应用（用于排行榜统计）
        // 尝试从环境变量读取，否则使用默认值
        let referer = std::env::var("OPENROUTER_REFERER")
            .unwrap_or_else(|_| "https://github.com/citrus-ai".to_string());
        let title =
            std::env::var("OPENROUTER_TITLE").unwrap_or_else(|_| "citrus-ai-analyzer".to_string());

        headers.insert(HeaderName::from_str("HTTP-Referer")?, referer.parse()?);
        headers.insert(HeaderName::from_str("X-Title")?, title.parse()?);

        Ok(headers)
    }

    /// 底层 API 调用
    pub async fn chat_completions(&self, request: &ChatRequest) -> anyhow::Result<ChatResponse> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .build()?;

        let headers = self.build_headers()?;

        let response = client
            .post(&self.base_url)
            .headers(headers)
            .json(request)
            .send()
            .await?;

        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await?;

            // 尝试解析 OpenRouter 的错误格式
            if let Ok(error_resp) =
                serde_json::from_str::<crate::models::OpenRouterErrorResponse>(&error_text)
            {
                let code = error_resp.error.code.unwrap_or(-1);
                let msg = &error_resp.error.message;

                // 根据错误码提供更详细的错误信息
                let detailed_msg = match code {
                    400 => format!("请求参数错误 (400): {}", msg),
                    401 => format!("API Key 无效或已过期 (401): {}", msg),
                    402 => format!("账户余额不足 (402): {}", msg),
                    403 => format!("没有权限访问该模型 (403): {}", msg),
                    408 => format!("请求超时 (408): {}", msg),
                    429 => format!("请求过于频繁 (429): {}", msg),
                    500 => format!("OpenRouter 服务器错误 (500): {}", msg),
                    502 => format!("上游供应商错误 (502): {}", msg),
                    503 => format!("模型暂时不可用 (503): {}", msg),
                    _ => format!("OpenRouter API 错误 (code: {}): {}", code, msg),
                };

                anyhow::bail!("{}", detailed_msg);
            }

            // 尝试解析标准错误 JSON
            if let Ok(error_json) = serde_json::from_str::<Value>(&error_text)
                && let Some(error_msg) = error_json
                    .get("error")
                    .and_then(|e| e.as_object())
                    .and_then(|error_obj| error_obj.get("message"))
                    .and_then(|m| m.as_str())
            {
                anyhow::bail!("API 错误：{}", error_msg);
            }

            anyhow::bail!("API request failed with status {}: {}", status, error_text);
        }

        let chat_response: ChatResponse = response.json().await?;
        Ok(chat_response)
    }

    /// 简化的聊天接口（统一 CLI 和 Server 的调用方式）
    pub async fn chat(&self, request: SimpleChatRequest) -> anyhow::Result<SimpleChatResponse> {
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

        // 合并默认 provider 和请求中的 provider
        let provider = request
            .options
            .provider
            .or_else(|| self.default_provider.clone());

        let chat_request = ChatRequest {
            model: self.model.clone(),
            messages,
            temperature: request.options.temperature,
            top_p: request.options.top_p,
            max_tokens: request.options.max_tokens,
            response_format: request.options.response_format,
            stream: request.options.stream,
            provider,
            transforms: request.options.transforms,
        };

        let response = self.chat_completions(&chat_request).await?;

        let content = response
            .choices
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
            provider: response.provider,
        })
    }

    /// 从 HTTP API 风格的 ChatApiRequest 统一执行聊天并返回 SimpleChatResponse
    #[allow(dead_code)]
    pub async fn exec_chat_from_api(
        &self,
        api_req: crate::models::ChatApiRequest,
    ) -> anyhow::Result<SimpleChatResponse> {
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
        if let Some(stream) = api_req.stream {
            options = options.stream(stream);
        }

        // 如果指定了首选供应商，创建 provider 偏好
        if let Some(preferred) = api_req.preferred_provider {
            let provider = crate::models::ProviderPreferences::with_order(vec![preferred]);
            options = options.with_provider(provider);
        }

        options = options.json_response();

        let request = SimpleChatRequest {
            message: api_req.message.unwrap_or_else(|| "请分析图片".to_string()),
            image: api_req.image,
            options,
            system_message: None,
        };

        let resp = self.chat(request).await?;
        Ok(resp)
    }

    /// 构建柑橘分析用的请求（供 analyze_citrus 使用）
    fn build_citrus_request(&self, image: Option<impl Into<String>>) -> SimpleChatRequest {
        const CITRUS_SYSTEM_MESSAGE: &str = r#"你是柑橘方面专家。现在要诊断柑橘和它的相关病症
请分析用户上传的图片，并严格按JSON格式返回以下结构，不要返回其他内容、不要使用Markdown代码块、不要添加额外字段：
{
  \"is_citrus_leaf\": true/false,                     // 是否为柑橘叶片
  \"citrus_type\": \"脐橙|砂糖橘|柚子|柠檬|其他|非柑橘\",  // 非柑橘时必须为 \"非柑橘\"
  \"disease_analysis\": {
    \"is_healthy\": true/false,    // 健康则为 true
    \"disease_name\": \"string\",    // 健康时填空字符串
    \"severity\": \"健康|轻度|中度|重度\",    // 健康时必须为 \"健康\"
    \"confidence\": 0~1,                 // 置信度，0 到 1 的小数
    \"treatment_suggestion\": \"string\",  // 健康时给出日常养护建议
    \"preventive_measures\": \"string\"    // 预防措施
  },
  \"image_quality_warning\": \"string\" // 图片质量告警；无则填空字符串
}
辅助诊断要点：
1. 柑橘叶片识别：柑橘叶片通常为卵形或椭圆形，叶片边缘有波浪状，叶片有光泽，叶脉明显
2. 常见病害特征：
   - 黄龙病：叶片黄化、斑驳、不对称，叶片变厚变脆；果实变小、畸形、着色不均（\"红鼻果\"或\"青头果\"），果皮变厚、汁少味酸
   - 溃疡病：叶片出现圆形黄色晕圈，中间有棕色或黑色凹陷斑点
   - 炭疽病：叶片出现圆形或椭圆形褐色斑点，边缘有黄色晕圈
   - 红蜘蛛危害：叶片出现白色或黄色斑点，叶片背面可见红色小点
3. 严重程度判断标准：
   - 轻度：病斑面积占叶片面积10%以下，不影响光合作用
   - 中度：病斑面积占叶片面积10%-30%，叶片部分功能受损
   - 重度：病斑面积占叶片面积30%以上，叶片严重变形或枯萎
4. 柑橘品种特征：
   - 脐橙：叶片较大，椭圆形，叶缘波浪明显
   - 砂糖橘：叶片较小，椭圆形，叶色深绿有光泽
   - 柚子：叶片最大，椭圆形或倒卵形，叶面粗糙
   - 柠檬：叶片中等大小，椭圆形，有特殊香气
5. 图片质量评估：
   - 检查图片是否模糊、过暗、过亮
   - 检查叶片是否被遮挡或只显示部分
   - 检查拍摄角度是否影响病害识别"#;

        // 用户消息只发送"请分析图片"，所有详细指令都在 system prompt 中
        let request = SimpleChatRequest::new("请分析图片")
            .with_options(
                ChatOptions::new()
                    .temperature(0.1)
                    .top_p(0.9)
                    .json_response(),
            )
            .with_system(CITRUS_SYSTEM_MESSAGE);

        match image {
            Some(img) => request.with_image(img),
            None => request,
        }
    }

    /// 柑橘分析专用接口（结构化返回，包含分析结果、usage 和 metrics）
    pub async fn analyze_citrus(
        &self,
        image: Option<impl Into<String>>,
    ) -> anyhow::Result<CitrusAnalysisResponse> {
        let start_time = Instant::now();

        let request = self.build_citrus_request(image);
        let response = self.chat(request).await?;

        let duration = start_time.elapsed();
        let total_tokens = response.usage.total_tokens as f64;
        let tokens_per_sec = if duration.as_secs_f64() > 0.0 {
            total_tokens / duration.as_secs_f64()
        } else {
            0.0
        };

        let data: crate::models::CitrusAnalysisResult = serde_json::from_str(&response.content)
            .map_err(|e| {
                anyhow::anyhow!("无法解析模型返回的JSON: {}, raw: {}", e, response.content)
            })?;

        Ok(CitrusAnalysisResponse {
            success: true,
            data,
            usage: response.usage,
            metrics: crate::models::AnalysisMetrics {
                duration_secs: duration.as_secs_f64(),
                tokens_per_sec,
            },
        })
    }
}
