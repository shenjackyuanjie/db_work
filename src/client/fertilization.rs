use super::{ChatOptions, OpenRouterClient, SimpleChatRequest};
use crate::models::{
    DiagnosisRecord, FertilizationPlanLlmOutput, FertilizationPlanRequest,
    FertilizationPlanResponse,
};

impl OpenRouterClient {
    /// 根据最近识别记录生成施肥方案摘要文本（用于 GET /api/generate）
    pub async fn generate_fertilization_text(
        &self,
        records: &[DiagnosisRecord],
    ) -> anyhow::Result<String> {
        const SYSTEM_MESSAGE: &str = r#"你是赣南脐橙种植专家，擅长根据柑橘病害诊断记录制定施肥建议。
请根据用户提供的最近识别记录，生成一段简洁的施肥策略摘要文本（150~300字），直接返回纯文本内容，不要使用JSON格式，不要使用Markdown格式，不要添加标题。
重点关注：
1. 当前病害状态对营养需求的影响
2. 针对性的施肥策略（基肥、追肥、叶面肥）
3. 需要补充或控制的具体元素（氮、磷、钾、钙、镁、硼等）
4. 注意事项和时间节点"#;

        let record_summary = if records.is_empty() {
            "暂无识别记录，请根据赣南脐橙一般情况给出通用施肥建议。".to_string()
        } else {
            let lines: Vec<String> = records
                .iter()
                .rev()
                .take(10)
                .map(|r| {
                    format!(
                        "- 识别结果：{}，健康状态：{}，病害：{}，严重程度：{}",
                        r.predicted_class,
                        if r.is_healthy { "健康" } else { "患病" },
                        if r.disease_name.is_empty() {
                            "无"
                        } else {
                            &r.disease_name
                        },
                        r.severity
                    )
                })
                .collect();
            format!(
                "最近 {} 条识别记录（最新优先）：\n{}",
                records.len().min(10),
                lines.join("\n")
            )
        };

        let request = SimpleChatRequest::new(&record_summary)
            .with_options(
                ChatOptions::new()
                    .temperature(0.7)
                    .top_p(0.95)
                    .max_tokens(600),
            )
            .with_system(SYSTEM_MESSAGE);

        let response = self.chat(request).await?;
        Ok(response.content.trim().to_string())
    }

    /// 根据土壤参数生成详细施肥方案（用于 POST /api/generate/fertilization-plan）
    pub async fn generate_fertilization_plan(
        &self,
        req: &FertilizationPlanRequest,
    ) -> anyhow::Result<FertilizationPlanResponse> {
        const SYSTEM_MESSAGE: &str = r#"你是赣南脐橙种植专家。请根据用户提供的土壤和果树信息，生成详细施肥方案。
严格按以下JSON格式返回，不要返回其他内容、不要使用Markdown代码块、不要添加额外字段：
{
  "title": "施肥方案标题",
  "content": "方案详细说明文字（200~400字）",
  "recommended_fertilizers": [
    {
      "name": "肥料名称",
      "amount": "用量（如 50kg/亩）",
      "application_method": "施用方式（如 穴施/撒施/叶面喷施）"
    }
  ],
  "application_schedule": [
    {
      "stage": "施肥阶段名称",
      "date": "建议日期（如 2025-12-15）",
      "description": "施肥操作说明"
    }
  ]
}
推荐肥料数量建议 3~5 种，施用计划建议 2~4 个节点。"#;

        let soil_type = req.soil_type.as_deref().unwrap_or("红壤");
        let ph_value = req.ph_value.unwrap_or(5.5);
        let nitrogen = req.nitrogen_level.as_deref().unwrap_or("medium");
        let phosphorus = req.phosphorus_level.as_deref().unwrap_or("medium");
        let potassium = req.potassium_level.as_deref().unwrap_or("medium");
        let growth_stage = req.growth_stage.as_deref().unwrap_or("涨果期");
        let tree_age = req.tree_age.unwrap_or(5);
        let area_size = req.area_size.unwrap_or(1000);

        let user_message = format!(
            "土壤类型：{soil_type}\npH值：{ph_value}\n氮素水平：{nitrogen}\n磷素水平：{phosphorus}\n钾素水平：{potassium}\n当前生长阶段：{growth_stage}\n树龄：{tree_age}年\n种植面积：{area_size}平方米"
        );

        let request = SimpleChatRequest::new(&user_message)
            .with_options(
                ChatOptions::new()
                    .temperature(0.3)
                    .top_p(0.9)
                    .json_response(),
            )
            .with_system(SYSTEM_MESSAGE);

        let response = self.chat(request).await?;

        let llm_output: FertilizationPlanLlmOutput = serde_json::from_str(&response.content)
            .map_err(|e| {
                anyhow::anyhow!("无法解析施肥方案JSON: {}, raw: {}", e, response.content)
            })?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let plan_id = format!("fp-{}-{:03}", chrono_date_str(), now % 1000);

        Ok(FertilizationPlanResponse {
            plan_id,
            title: llm_output.title,
            content: llm_output.content,
            recommended_fertilizers: llm_output.recommended_fertilizers,
            application_schedule: llm_output.application_schedule,
        })
    }
}

fn chrono_date_str() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    let year_approx = 1970 + days / 365;
    let day_of_year = days % 365;
    let month = day_of_year / 30 + 1;
    let day = day_of_year % 30 + 1;
    format!("{:04}{:02}{:02}", year_approx, month.min(12), day.min(31))
}
