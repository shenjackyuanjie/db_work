use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use crate::models::{ChatRequest, ChatResponse};

#[derive(Clone)]
pub struct GlmClient {
    api_key: String,
    base_url: String,
}

impl GlmClient {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://open.bigmodel.cn/api/paas/v4/chat/completions".to_string(),
        }
    }

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
}