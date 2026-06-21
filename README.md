# 柑橘果园智能诊断与管理后端

这是一个基于 Rust 构建的单体后端服务，用于支撑柑橘果园场景下的用户登录、病害识别、环境监测、任务管理、管理员后台和施肥方案生成。

服务启动后会同时提供：

- Web 静态页面（`/index.html`、`/analyze.html`、`/admin.html`、`/orchard-3d.html`）
- 面向前端的 HTTP API
- PostgreSQL 数据存储与启动时自动建表
- 本地 ONNX 推理或 OpenRouter 驱动的混合推理能力

当前仓库的入口在 `src/main.rs`，默认读取 `config.toml` 并直接启动 HTTP 服务，不再是早期 README 中描述的 CLI 聊天工具。

## 主要能力

- 用户注册、登录、登出、会话校验
- 管理员后台、待审核用户、邀请码、系统设置、审计日志
- 柑橘病害识别，支持本地 ONNX 推理和远端 AI 混合推理
- 识别记录落库、图片归档、历史记录查询
- 温湿度采样、果园健康点统计、任务生成与完成管理
- 果园总览、后台二维地图、Three.js 3D 沙盘
- 树位与传感器示例数据自动初始化
- 基于 OpenRouter 的柑橘分析和施肥建议生成
- 前后端同端口部署，静态资源由服务直接托管

## 技术栈

- Rust 2024
- Tokio
- Axum
- SQLx + PostgreSQL
- Tract ONNX
- Reqwest
- Serde
- Tower HTTP
- Three.js（前端 3D 沙盘，经 CDN 加载）

## 运行要求

- Rust 1.85 或更新的稳定版
- 可访问的 PostgreSQL 实例
- 本地 ONNX 模型文件（使用 `onnx` 模式时必需）
- OpenRouter API Key（使用柑橘分析、施肥建议等 AI 能力时必需）
- 可访问 jsDelivr CDN 的浏览器网络环境（使用 3D 沙盘页面时必需）

## 配置说明

服务从根目录的 `config.toml` 读取配置。一个最小可用示例如下：

```toml
[server]
addr = "0.0.0.0:11000"
log_level = "info"

[ai]
openrouter_api_key = "sk-or-v1-your-key"

[database]
postgres_url = "postgres://user:password@127.0.0.1:5432/db_name"

[inference]
mode = "onnx"
model_1_path = "onnx/model_1.onnx"
model_2_path = "onnx/model_2.onnx"
```

配置项含义：

- `server.addr`：服务监听地址
- `server.log_level`：日志级别
- `ai.openrouter_api_key`：OpenRouter 调用密钥
- `database.postgres_url`：PostgreSQL 连接串
- `inference.mode`：推理模式，支持 `onnx` 和 `remote`
- `inference.model_1_path`：第一阶段果树识别模型路径
- `inference.model_2_path`：第二阶段病害识别模型路径

推理模式说明：

- `onnx`：本地完成果树识别和病害分类，适合离线或低延迟场景
- `remote`：先用本地模型判断是否为果树，再调用 OpenRouter 做病害分析与建议生成

注意事项：

- 服务启动时会自动创建所需数据表
- 当果树与传感器表为空时，会自动写入一组演示树位和采样数据
- 请不要把真实数据库地址或 API Key 提交到版本库

## 快速启动

1. 准备 PostgreSQL 数据库并确保连接串可用。
2. 将 ONNX 模型放到 `onnx/model_1.onnx` 和 `onnx/model_2.onnx`，或在配置里改成你的实际路径。
3. 修改根目录 `config.toml`。
4. 启动服务。

```bash
cargo run --release
```

启动成功后，默认可通过以下地址访问：

- `http://127.0.0.1:11000/`：会重定向到首页
- `http://127.0.0.1:11000/index.html`：公开首页 / 登录入口
- `http://127.0.0.1:11000/analyze.html`：识别分析页，需要有效登录态
- `http://127.0.0.1:11000/admin.html`：管理员后台，需要管理员权限
- `http://127.0.0.1:11000/orchard-3d.html`：3D 园区沙盘，页面可直接打开，数据接口需要有效登录态

## 关键接口概览

下面列的是当前代码中已注册的主要接口分组，具体行为以 `src/server.rs` 和 `src/user_routes/mod.rs` 中的路由为准。

### 基础与会话

- `GET /health`：健康检查
- `POST /api/register`：注册
- `POST /api/login`：登录
- `POST /api/logout`：登出
- `POST /api/validate`：校验 token
- `GET /api/user`：获取当前用户或默认用户信息
- `GET /api/system-status`：获取系统设置状态
- `POST /user/login`、`POST /user/register`、`POST /user/logout`、`POST /user/validate`：同名用户路由命名空间
- `POST /user/me`：获取当前登录用户

### 首页与看板

- `GET /api/home`：首页摘要数据
- `GET /api/growth-tracking`：生长追踪数据
- `GET /api/diagnose`：诊断模块静态数据
- `GET /api/temperature-humidity`：温湿度采样
- `POST /api/temperature-humidity`：提交带标签序列号的温湿度采样，同时写入树传感器记录
- `GET /api/health-point`：果园健康点统计
- `POST /user/orchard/overview`：当前登录用户的果园树位、传感器、诊断和天气摘要数据

### 病害识别与记录

- `POST /api/citrus-disease`：病害识别
- `POST /api/citrus-disease-v2`：增强版病害识别
- `POST /citrus/analyze`：调用 OpenRouter 做柑橘图像分析
- `GET /api/recognition-records`：识别记录列表
- `GET /api/disease-treatment`：病害处置建议
- `GET /media/recognition_records/*`：识别图片静态访问

### 任务与建议生成

- `GET /api/tasks`：任务列表
- `POST /api/tasks/add`：新增任务
- `POST /api/tasks/complete`：完成任务
- `POST /api/tasks/generate/disease`：根据病害生成任务
- `POST /api/tasks/generate/environment`：根据环境生成任务
- `GET /api/generate`：基于历史诊断记录生成施肥建议文本
- `POST /api/generate/fertilization-plan`：生成结构化施肥方案

### 管理员接口

管理员接口挂在 `/user/admin/*` 下，包含：

- 用户管理与管理员设置
- 邀请码创建与查询
- 系统设置读取与更新
- 指定用户的果园总览与天气数据聚合
- 仪表盘统计与日志
- 待审核用户审批与驳回

## 示例请求

健康检查：

```bash
curl http://127.0.0.1:11000/health
```

上传叶片图片做病害识别：

```bash
curl -X POST http://127.0.0.1:11000/api/citrus-disease \
  -F "username=demo" \
  -F "area=NAVEL-001" \
  -F "image=@./leaf.jpg"
```

生成施肥方案：

```bash
curl -X POST http://127.0.0.1:11000/api/generate/fertilization-plan \
  -H "Content-Type: application/json" \
  -d '{
    "soilType": "红壤",
    "phValue": 5.5,
    "nitrogenLevel": "medium",
    "phosphorusLevel": "low",
    "potassiumLevel": "low",
    "growthStage": "涨果期",
    "treeAge": 5,
    "areaSize": 1000
  }'
```

## 数据表

服务启动时会自动初始化以下核心表：

- `app_users`
- `app_sessions`
- `app_invitations`
- `app_pending_users`
- `app_tasks`
- `app_temperature_humidity`
- `app_diagnosis_records`
- `app_system_settings`
- `app_admin_audit_logs`
- `app_orchard_trees`
- `app_tree_sensor_records`

这意味着本项目默认采用“启动即建表”的方式，而不是独立迁移框架。

## 项目结构

```text
.
├── src/
│   ├── main.rs                 # 服务入口
│   ├── config.rs               # 配置加载
│   ├── server.rs               # 路由注册与服务启动
│   ├── server/                 # 页面、看板、识别记录、任务等处理器
│   ├── user_routes/            # 登录、注册、管理员接口
│   ├── inference/              # ONNX / 远端推理运行时
│   ├── client/                 # OpenRouter 客户端与施肥建议调用
│   ├── system_settings.rs      # 系统设置默认值与读取逻辑
│   └── models.rs               # 请求/响应模型
├── static/                     # 前端静态页面与资源
│   └── uploads/                # 识别图片归档目录
├── onnx/                       # 本地推理模型
├── scripts/                    # 数据迁移与辅助脚本
├── config.toml                 # 运行配置
└── README.md
```

## 开发命令

```bash
cargo check
cargo test
cargo fmt
```

## 常见问题

### 1. 服务启动时报数据库连接错误

优先检查 `config.toml` 中的 `database.postgres_url`、数据库账号权限以及目标库是否存在。

### 2. 识别接口返回模型加载失败

通常是 `onnx/model_1.onnx` 或 `onnx/model_2.onnx` 路径不对，或文件本身缺失。

### 3. AI 生成接口调用失败

检查 `ai.openrouter_api_key` 是否有效，以及外网是否可以访问 OpenRouter。

### 4. 为什么访问 `/admin.html` 会被重定向

管理员页面要求管理员会话；普通用户或未登录访问时会被重定向到首页。

## 相关文件

- `scripts/migrate_sqlite_to_pg.py`：历史数据迁移脚本
- `scripts/migrate_add_user_coordinates.sql`：用户坐标字段相关 SQL

## 许可证

仓库中未声明单独许可证时，请按团队或项目约定使用。

