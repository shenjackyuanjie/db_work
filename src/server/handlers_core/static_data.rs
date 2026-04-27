use axum::response::Response;
use serde_json::json;

use super::super::api_success;

pub(crate) async fn home_api_handler() -> Response {
    api_success(json!({
        "weatherCondition": "阴",
        "temperatureRange": "20℃ - 25℃",
        "suggestion": "建议：保持正常天气，注意防晒。",
        "healthScore": 80,
        "weeklyAlerts": 2,
        "pendingTasks": 10,
        "growthRate": 85.5,
        "DiagnosisStatus": "正常",
        "GrowthStatus": "涨果期"
    }))
}

pub(crate) async fn growth_tracking_api_handler() -> Response {
    api_success(json!({
        "growthStageText": "涨果期",
        "growthStageDuration": "30",
        "startDate": "2025-12-01",
        "endDate": "2026-01-30",
        "fruitExpansionStartDate": "2025-12-01",
        "fruitExpansionEndDate": "2026-01-30",
        "colorChangeStartDate": "2026-02-01",
        "colorChangeEndDate": "2026-03-30",
        "youngFruitStartDate": "2026-04-01",
        "youngFruitEndDate": "2026-05-30",
        "diameter": 7.2,
        "ratio": 1.2
    }))
}

pub(crate) async fn diagnose_api_handler() -> Response {
    api_success(json!({
        "data": "2025-12-03",
        "n_P_K_ViewModel": {
            "nitrogenValue": 110.0,
            "phosphorusValue": 50.0,
            "potassiumValue": 10.0
        },
        "percentage": 86.0,
        "getList": {
            "list": [
                {
                    "title": "氮元素含量稳定",
                    "content": "当前氮含量水平有利于叶片生长，维持现状即可"
                },
                {
                    "title": "钾元素缺乏",
                    "content": "第5区果树钾元素偏低，建议补充钾肥提高果实品质"
                },
                {
                    "title": "钙镁元素平衡",
                    "content": "当前钙镁比例适宜，有利于果实发育"
                }
            ]
        }
    }))
}