# 后端API接口规范文档

## 1. 首页模块 (Home)

### 1.1 获取首页数据

- **请求方法**: GET
- **URL**: `/api/home`
- **请求头**:
  - `Content-Type: application/json`
- **请求体**: 无
- **响应格式**:

```json
{
  "weatherCondition": "晴",
  "temperatureRange": "20℃ - 25℃",
  "suggestion": "建议：保持正常天气，注意防晒。",
  "healthScore": 80,
  "weeklyAlerts": 2,
  "pendingTasks": 10,
  "growthRate": 85.5,
  "DiagnosisStatus": "正常",
  "GrowthStatus": "涨果期"
}
```

## 2. 生长追踪模块 (Growth Tracking)

### 2.1 获取生长追踪数据

- **请求方法**: GET
- **URL**: `/api/growth-tracking`
- **请求头**:
  - `Content-Type: application/json`
- **请求体**: 无
- **响应格式**:

```json
{
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
}
```

## 3. 营养诊断模块 (Diagnose)

### 3.1 获取诊断数据

- **请求方法**: GET
- **URL**: `/api/diagnose`
- **请求头**:
  - `Content-Type: application/json`
- **请求体**: 无
- **响应格式**:

```json
{
  "data": "2025-12-3",
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
      { "title": "钙镁元素平衡", "content": "当前钙镁比例适宜，有利于果实发育" }
    ]
  }
}
```

## 4. 施肥方案生成模块 (Generate)

### 4.1 获取施肥方案

- **请求方法**: GET
- **URL**: `/api/generate`
- **请求头**:
  - `Content-Type: application/json`
- **请求体**: 无
- **响应格式**:

```json
{
  "textii": "针对赣南脐橙，建议实施\"测土配方、分期精准\"的施肥策略。基肥以腐熟有机肥（如羊粪、饼肥）为主，秋季深施，改良酸性红壤。追肥分三次：春梢期以高氮复合肥促梢保花；壮果期增施钾肥（如硫酸钾），配施磷与中微量元素，提升糖度与果皮光泽；采果前补施速效肥恢复树势。全年注重叶片营养诊断，结合土壤检测结果灵活调整，确保氮、磷、钾与钙、镁、硼等元素平衡，避免偏施氮肥。坚持生草栽培，保墒增肥。"
}
```

### 4.2 生成施肥方案

- **请求方法**: POST
- **URL**: `/api/generate/fertilization-plan`
- **请求头**:
  - `Content-Type: application/json`
- **请求体**:

```json
{
  "soilType": "红壤",
  "phValue": 5.5,
  "nitrogenLevel": "medium",
  "phosphorusLevel": "low",
  "potassiumLevel": "low",
  "growthStage": "涨果期",
  "treeAge": 5,
  "areaSize": 1000
}
```

- **响应格式**:

```json
{
  "planId": "fp-20251201-001",
  "title": "赣南脐橙涨果期施肥方案",
  "content": "针对红壤酸性土壤，建议...",
  "recommendedFertilizers": [
    { "name": "硫酸钾", "amount": "50kg/亩", "applicationMethod": "穴施" },
    { "name": "过磷酸钙", "amount": "30kg/亩", "applicationMethod": "撒施" }
  ],
  "applicationSchedule": [
    {
      "stage": "壮果期初期",
      "date": "2025-12-15",
      "description": "第一次施肥"
    },
    { "stage": "壮果期中期", "date": "2026-01-15", "description": "第二次施肥" }
  ]
}
```

## 5. 相机识别模块 (Camera)

### 5.1 识别柑橘病虫害

- **请求方法**: POST
- **URL**: `/api/citrus-disease`
- **请求头**:
  - `Content-Type: multipart/form-data`
- **请求体**:
  - `image: 文件对象（multipart/form-data格式）`
- **响应格式**:

```json
{
  "code": 200,
  "message": "success",
  "data": {
    "predicted_class": "黄龙病",
    "confidence": 100.0,
    "stage": "model_2"
  },
  "timestamp": 1771051928276
}
```

**说明**:

- `predicted_class`: 预测的病虫害类型，可能的值包括：黄龙病、健康果树、溃疡病、沙皮病、非果树
- `confidence`: 预测置信度（百分比）
- `stage`: 使用的模型阶段，值为 "model_1"（果树识别）或 "model_2"（病害识别）

## 6. 通用响应格式

所有API响应应遵循以下通用格式：

### 6.1 成功响应

```json
{
  "code": 200,
  "message": "success",
  "data": {
    /* 具体数据 */
  },
  "timestamp": 1672531200000
}
```

### 6.2 错误响应

```json
{
  "code": 400,
  "message": "错误信息",
  "data": null
}
```

## 7. 错误代码定义

| 错误代码 | 描述           |
| -------- | -------------- |
| 200      | 成功           |
| 400      | 请求参数错误   |
| 401      | 未授权         |
| 403      | 禁止访问       |
| 404      | 资源不存在     |
| 500      | 服务器内部错误 |
| 502      | 网关错误       |
| 503      | 服务不可用     |
| 504      | 网关超时       |

## 8. 数据类型定义

| 数据类型 | 描述   | 示例             |
| -------- | ------ | ---------------- |
| string   | 字符串 | "晴"             |
| number   | 数字   | 80               |
| boolean  | 布尔值 | true             |
| array    | 数组   | [1, 2, 3]        |
| object   | 对象   | {"key": "value"} |
| date     | 日期   | "2025-12-01"     |

## 9. 安全规范

1. 所有API请求应使用HTTPS协议
2. 敏感操作应进行身份验证和授权
3. 输入数据应进行验证和清理，防止SQL注入和XSS攻击
4. 错误信息不应包含敏感的系统信息
5. API应设置合理的速率限制，防止滥用

## 10. 性能要求

1. API响应时间应控制在500ms以内
2. 系统应能承受至少100个并发请求
3. 数据库查询应优化，避免全表扫描
4. 应使用缓存机制减少重复计算和数据库查询
