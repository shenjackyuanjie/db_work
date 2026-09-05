# 柑橘果园智能诊断与管理系统架构说明（agents.md）

## 项目定位
这是一个 Rust 单体服务（Axum + PostgreSQL），“一进程多职责”部署：

- 同一进程提供前端页面、Web API、AI 推理接口、商城/订单系统、管理员运营接口。
- 会话与权限、核心业务数据、推理结果都落在同一套 PostgreSQL（`app_*`、`commerce_*`、`store_*`）体系。
- 前端页面为原生静态文件（`static/`），由服务端静态兜底托管。

## 启动链路（运行入口）
- `src/main.rs`
  - 加载 `config.toml`。
  - 初始化日志。
  - 调用 `server::run_server`。

- `src/server.rs::run_server`
  - 解析监听地址。
  - 建立 PostgreSQL 连接池。
  - 调用 `bootstrap::init_database` 初始化表与必要默认配置。
  - 组装 `AppState` 并启动 Axum。

## 配置面
- `src/config.rs`
  - `AppConfig`：`server`、`ai`、`database`、`bootstrap`、`inference`。
  - `server`：监听地址、数据库连接池大小、cookie 安全位。
  - `ai`：OpenRouter API Key。
  - `inference`：推理模式 `remote|onnx` 与两个 ONNX 路径。

## 核心层次结构

### 1) 服务层（Server）
文件：`src/server/*`

- `server.rs`
  - 统一创建 Router。
  - 注册公开路由、核心接口、AI 接口、商城接口和 `/user` 嵌套路由。
  - 注入中间件（请求日志）与 CORS。
  - `fallback` 到 `static/`，实现页面+接口共存。
- `server/shared.rs`
  - 全局共享 `AppState`：`OpenRouterClient`、`InferenceRuntime`、`PgPool`、`secure_session_cookie`。
  - 公共 DTO 与响应工具：`api_success`、`api_response`。
  - 图片保存与路径规范化工具。
- `server/bootstrap.rs`
  - 自举建库：`app_users`、`app_sessions`、`app_tasks`、`app_diagnosis_records`、`app_temperature_humidity`、`app_system_settings`、`app_orchard_trees` 等。
  - 同时保留 `commerce_*` 与 `store_*` 的历史表。
  - 可选 seed demo 数据。

### 2) 业务路由层
- `server/handlers_core.rs`
  - 页面与公共数据读取（状态页、主页/商店页、健康巡检、温湿度、任务列表等）。
  - 以 `handlers_core::{pages,records,static_data,tasks,temperature,media,health_point}` 拆分。

- `server/handlers_ai.rs`
  - AI 识别入口：
    - `citrus_disease_handler`（基础流程）
    - `citrus_disease_advanced_handler`（两阶段：果树门控 + LLM 高级分析）
    - `generate_handler` / `generate_fertilization_plan_handler` / `citrus_analyze_handler`
  - `handlers_ai` 子模块处理请求解析、记录持久化、复核阈值、报告生成。

- `server/handlers_store.rs`
  - 现货商城公开接口：列表查询、封面图静态返回。

- `server/handlers_commerce.rs`
  - 历史团购/批次相关接口：店铺橱窗与批次追溯。

### 3) 用户域路由（`/user/*`）
文件：`src/user_routes/*`

- `mod.rs` 组织 `/user` 下的所有历史兼容和新接口。
- `auth.rs`/`session.rs`：登录注册/登出/会话验证。
- `admin.rs`（含 `admin/common`、`dashboard`、`management`、`orchard`、`pending`、`settings`）
  - 管理后台能力：审批用户、管理员配置、果园概览、仪表盘。
- `commerce.rs`：用户端和管理员端团购接口。
- `store.rs`：现货商城订单、管理员商品与订单状态管理。
- `dto.rs`：共享请求/响应结构。
- `registration.rs`：注册行为。

## 认证与授权
- 使用服务端会话（`session_token`）
  - 支持 Cookie（HttpOnly）与 `X-Session-Token` 头。
  - Token 过期由 `app_sessions.expires_at` 管控（30天）。
- 鉴权策略：
  - `ensure_authenticated`：校验会话。
  - `ensure_admin`：基于 `app_users.is_admin`。
- 兼容历史行为：密码哈希支持 Argon2 与旧 blake3，登录后可迁移。

## 推理与 AI 集成层
文件：`src/inference/*`、`src/client/*`

- `inference/runtime.rs`
  - 提供统一 `InferenceRuntime`：`Remote` 与 `Onnx` 可插拔。
- `inference/onnx.rs`
  - 加载并运行两套 ONNX 模型。
  - model_1 做果树门控，model_2 做病害分类；附温湿度规则加权。
- `inference/remote.rs`
  - Remote 模式仍先做门控，再调用 OpenRouter 结构化分析。
- `client/openrouter.rs`
  - 构建 OpenRouter 请求、JSON 结构化约束、错误处理与返回计量。
- `client/fertilization.rs`（未展开读取）负责施肥方案生成/请求构造相关逻辑。

## 数据模型与持久化
- 认证/账号：`app_users`、`app_sessions`、`app_pending_users`、`app_admin_audit_logs`
- 任务与农情：`app_tasks`、`app_temperature_humidity`、`app_diagnosis_records`、`app_system_settings`
- 果园：`app_orchard_trees`、`app_tree_sensor_records`
- 商城：`store_products`、`store_orders`、`store_order_items`
- 历史团购：`commerce_orchards`、`commerce_products`、`commerce_batches`、`commerce_batch_products`、`commerce_orders`、`commerce_order_status_logs`

## 关键数据流（示例）
- 柑橘识别（`POST /api/citrus-disease*`）
  1. `handlers_ai/request` 解析图片、温湿度、用户身份。
  2. `handlers_ai/diagnosis` 或 `advanced` 调用 `AppState.inference`。
  3. 结果经过 `review` 阈值策略修正。
  4. 识别记录入库，必要时自动生成治理任务。
  5. 返回结构化 JSON。

- 登录会话
  1. `user_routes/session` 写入 `app_sessions`。
  2. 下游接口通过 `auth::ensure_authenticated` 从 token 查会话。

- 商城与订单
  1. 页面路由从 `handlers_store` / `handlers_commerce` 拉取列表。
  2. `/user/store/*` 与 `/user/commerce/*` 完成订单与状态变更。
  3. 商品封面上传保存到 `storage/store_covers`。

## 运行时存储目录
- `storage/recognition_records/`：诊断图片
- `storage/store_covers/`：商城商品封面

## 当前开发特点（重要）
- 路由有较强“兼容历史 API”导向：部分旧接口保留。
- 单机单进程架构，适合快速交付；若要横向扩展，先抽离静态资源、推理服务和会话验证层。
- 推理可在 onnx 与 remote 间切换，但 remote 模式仍依赖 onnx 做门控模型初始化。

## 建议的演进方向（给新维护者）
- 明确职责边界：
  - API 层继续向 handlers 拆分（按模块再细化）。
  - 业务规则（如复核阈值、诊断策略）单独提取到 `domain`/`service` 层。
- 按接口分流：
  - 公开接口、业务接口、后台运营接口三套中间件/限流策略。
- 部署加固：
  - 替换宽松 CORS。
  - 加入静态资源和媒体目录的签名访问。
  - 配置连接池上限和超时策略。

## 代码格式约定

- 保持代码清晰易读，不要刻意压行，尤其是 Rust 代码。
- Rust 格式化必须使用 nightly 工具链，执行 `cargo +nightly fmt`，不要使用默认或 stable 工具链代替。
- 前端 HTML、CSS 和 JavaScript 同样使用正常的多行排版，不要手工压缩源文件。
