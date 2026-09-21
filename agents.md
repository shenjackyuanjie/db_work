# 柑橘果园智能诊断与管理系统架构说明（agents.md）

> 本文件是**现状说明书**，不是设计意图书。最后结构性同步：S5/G2（`/api/**` 收归契约层、
> `/user/*` 整棵退役、旧表 DDL 剪枝）。历史决策、推翻过的前提与实测记录在
> `tests/fixtures/contract/W2_S5_PLAN.md`；逐字契约的差异登记在
> `tests/fixtures/contract/DEVIATIONS.md`。

## 项目定位
这是一个 Rust 单体服务（Axum + PostgreSQL），“一进程多职责”部署。

**三层命名空间 —— 改路由前先认准自己在哪一层**：

| 名称空间 | 实现 | 谁在用 | 硬约束 |
|---|---|---|---|
| `/api/**`、`/api/v1/**` | `src/compat/` 契约层 | 手机 App | **逐字节复刻 Django 蓝本**（`navel_backend_git`）：字段名、字段顺序、时间格式、十进制字符串、中文文案都不能动 |
| `/web/**` | `src/web/` 超集层 | 网页前端（`static/*.js`） | 允许超出 Django 的能力，但响应形状必须与消费者的读法一致（见下「响应形状两类」） |
| 页面外壳 + `static/` | `src/server/handlers_core/pages.rs` + `ServeDir` | 浏览器 | 页面路由全部返回同一个外壳 HTML，路由由前端 `app-shell.js` 自己走 |

- **不再有自研 `/api/*`**：G2 之后 `/api/**` 与 `/api/v1/**` **只**由契约层注册。
  要加自研接口请放 `/web/*`——把超集塞进 `/api/**` 会直接破坏 App 的兼容目标。
- 会话与权限、业务数据、推理结果都落在同一套 PostgreSQL 体系里。
- 前端是原生静态文件（`static/`），由服务端静态兜底托管。

## 启动链路（运行入口）
- `src/main.rs`
  - 加载 `config.toml`。
  - 初始化日志。
  - 调用 `server::run_server`。
- `src/server.rs::run_server`
  - 解析监听地址 → 建立 PostgreSQL 连接池 → 调用 `bootstrap::init_database`（建表、默认
    系统设置、可选 demo 数据）→ 组装 `AppState` → 启动 Axum。

## 配置面
- `src/config.rs`
  - `AppConfig`：`server`、`ai`、`database`、`bootstrap`、`inference`。
  - `server`：监听地址、数据库连接池大小、cookie 安全位。
  - `ai`：OpenRouter API Key。
  - `inference`：推理模式 `remote|onnx` 与两个 ONNX 路径。

## 核心层次结构

### 1) 服务层（`src/server/*`）
- `server.rs`
  - **唯一**组装 Router 的地方。顺序：页面路由 → 契约层（根上 `merge`）→ `/compat` 前缀
    （同一个 router 的**第二次**挂载，`scripts/replay_diff.py --base-url .../compat` 依赖它，
    删了等于把回归网拆了）→ `/web` 超集层 → 三条静态通道 →
    `fallback_service(ServeDir::new("static"))`。
  - 注入中间件（请求日志）与 CORS。
- `server/shared.rs`
  - 全局 `AppState`：`OpenRouterClient`、`InferenceRuntime`、`PgPool`、`secure_session_cookie`。
  - 工具：`now_millis`、图片落盘与路径规范化、`username_by_token`。
  - `lookup_session_username`：**只认 `auth_token` 一张表**（原「双表桥」已随 `app_sessions`
    的 DDL 在 G2 同一步删除）。
- `server/bootstrap.rs` + `server/bootstrap/*_tables.rs`
  - 自举建库。**契约表按外键依赖顺序跨分组交错执行**，`init_database` 里那段编号注释是硬约束，
    不要重排。
  - `"user".is_admin` 是**加法列**（`ALTER TABLE ... ADD COLUMN IF NOT EXISTS`）。
    **不要**把它挪进 `core_tables.rs` 的 `CREATE TABLE`：那张表要与 Django `db_table` 逐字对齐，
    多一列就没法用 Django 的 `dumpdata` 直接把种子灌进来。
  - 这里**只写 `CREATE TABLE IF NOT EXISTS`**，不写 `DROP TABLE`（删 DDL 只影响新建库）。

### 2) 契约层（`src/compat/`，逐字兼容 Django）
- `ser.rs`：`dt_z` / `dt_offset` / `dt_naive_local` / `dt_date` / `dec` / `dec_scaled`。
  时间格式与十进制字符串是兼容性的命门。
- `errors.rs`：`api_ok` / `api_ok_message` / `api_error` / `api_response` / `ApiReject` / `ApiResult`。
  信封键顺序必须是 `code,message,data,timestamp` —— 这依赖 `Cargo.toml` 里 `serde_json` 的
  **`preserve_order` feature，不要关掉**。
- `auth.rs`：Bearer 通道（Django `AuthToken`）。与网页的 cookie 通道是两套，互不读对方。
- `views_{auth,core,trace,commerce,agent}.rs`：五个域的路由与 handler，同时注册 `/api/**`
  与 `/api/v1/**` 两套前缀。
- `tests/`：夹具比对测试，基准是 `tests/fixtures/contract/*.json`（Django 端 capture 出来的）。
  **跑之前必须先重灌种子**，否则看到的红是假失败——规则见 `W2_S5_PLAN.md` §0.2。

### 3) 超集层（`src/web/`）
`session.rs`（cookie 会话 `/web/session/*`）· `admin.rs`（后台 10 条）· `store_admin.rs`
（后台商品 CRUD / 订单 / 统计）· `support.rs`（客服会话）· `orchard.rs`（3D 沙盘几何 +
`/web/citrus-disease-v2` 识别入口）· `dashboard.rs`（仪表盘统计与日志）。

**响应形状两类，改之前先看消费者怎么读**：信封（`app_ok` / `app_err`）与裸体。
实测过的三个消费者口味不同：`admin.js` 的 `unwrapApiPayload` 是 `payload.data ?? payload`
（**必须**有非空 `data`）；`store-admin.js` 的 `request` 是 `b.data ?? b`（两种都吃）；
`store-support.js` 直接读裸 JSON。

### 4) 鉴权与授权
- `src/auth.rs`：`extract_auth_token`（Cookie `session_token` → `X-Session-Token` 两通道，
  **不**读 `Authorization`）、`ensure_authenticated`、`ensure_admin`。
  页面外壳与 legacy handler 用的是这一套。
- 网页侧另有一套 `web::session::{lookup_is_admin, require_admin}`。**两套读同一列
  `"user".is_admin`** —— 让它们读不同的表会导致「网页登录的管理员打不开后台页面」。
- 会话表：`auth_token`（契约表；`expires_at` 是 `TIMESTAMPTZ`，在 SQL 里用 `now()` 比较，
  不要拿 epoch 秒混绑）。
- 密码哈希：Argon2 为主，兼容旧 blake3，登录后迁移。

### 5) 业务 handler（`src/server/handlers_*`）
- `handlers_core/{pages,media}.rs`：9 个页面 handler + 两条隐式静态通道。**活代码**。
- `handlers_ai/{request,advanced,persistence,review}.rs`：`/web/citrus-disease-v2` 的两阶段链路
  （果树门控 + LLM 高级分析）。
- `handlers_store.rs`：**只剩 `/store-images/*` 一条静态通道**。
  **别因为「只有一个函数」就整文件删掉**，理由见下「隐式静态通道」。
- `handlers_commerce.rs`：已随 `/api/commerce/*` 在 G2 删除。

## 数据模型与持久化
- **契约表（30 张，Django 蓝本）**：`"user"`、`auth_token`、`disease_recognition_record`、`task`、
  `citrus_product`、`"order"`、`order_item`、`sales_batch`、`trace_event`、`agent_approval` 等。
  两个 PostgreSQL 保留字**必须双引号**：`"user"`、`"order"`。
- **网页专表**：`web_diagnosis_records`（17 列富字段，仪表盘与 3D 沙盘的读源）。
- **自研保留表**：`app_system_settings`、`app_invitations`、`app_pending_users`、
  `app_admin_audit_logs`、`app_orchard_trees`、`app_tree_sensor_records`、`store_support_messages`。
- **已退役（S5/G2，DDL 已删）**：`app_sessions`、`app_users`、`app_tasks`、
  `app_temperature_humidity`、7 张 `commerce_*`、`store_products`/`store_orders`/`store_order_items`。
  ⚠️ 删 DDL **只影响新建库**；现网 `public` 里的旧表仍然存在，DROP 由独立脚本在 `pg_dump`
  之后手工执行。
- `app_diagnosis_records` **已无代码读者**（写入口 S2 移到 `web_diagnosis_records`，静态通道
  G2 也跟着移了），留着只是因为它还端着现网的历史行。

## 关键数据流
- **柑橘识别**（`POST /web/citrus-disease-v2` 网页 / `/api/citrus-disease*` App）
  1. `handlers_ai/request` 解析图片、温湿度、用户身份。
  2. 果树门控 + 推理（`advanced`）。
  3. 结果经 `review` 阈值策略修正。
  4. 写 `web_diagnosis_records`（网页读源）并双写契约表。
  5. 必要时建治理任务 —— 写契约表 `task`（**不再写 `app_tasks`**）。
- **网页会话**：`/web/session/{login,register,logout,validate,me}` 读写 `auth_token`。
- **商城与订单**：商品与订单都是契约表（`citrus_product` / `"order"` / `order_item`）。
  网页前台读**契约层**的 `/api/products`、`/api/orders`、`/api/cart`；网页后台读写
  `/web/admin/store/*`；封面文件落到 `storage/store_covers/`，路径串带 `/store-images/` 前缀。

## 运行时存储目录
- `storage/recognition_records/`：诊断图片（URL 前缀 `/media/recognition_records/` 或 `/uploads/`）
- `storage/store_covers/`：商城商品封面（URL 前缀 `/store-images/`）

## 三条「隐式静态通道」（**不要用 grep 判它们的死活**）
`/store-images/{file_name}`、`/media/recognition_records/{file_name}`、`/uploads/{file_name}`。

它们**没有 JS fetch 调用者**：URL 是**数据库里的路径字符串**（`citrus_product.cover_image_url`、
`web_diagnosis_records.image_path`），由页面直接 `<img src>` 取用。所以 grep 永远判它们「没人用」，
而删掉它们会让图片全部 404 —— 这类「数据驱动的路径串」是本项目最容易误杀的地方。
改动时唯一可信的验证：查 DB 里的路径串 → 按它发 HTTP → **200 且 `Content-Type: image/*`**，
再做一次反证（库里没有的路径必须 404，以排除是 `static/` 兜底把文件端出来的假证据）。

## 当前开发特点（重要）
- 单机单进程架构，适合快速交付；若要横向扩展，先抽离静态资源、推理服务和会话验证层。
- 推理可在 onnx 与 remote 间切换，但 remote 模式仍依赖 onnx 做门控模型初始化。
- 前台 5 个页面 + 后台 2 个页面共用同一个外壳，SPA 路由与地址归一化在 `static/app-shell.js`。
- 嵌入式 App 用 `/app-content/*` 提供页面内页，`static/app-bridge.js` 负责把它们桥到外层外壳。

## 建议的演进方向（给新维护者）
- 守住两条边界：契约层只增不改（要改先登记 `DEVIATIONS.md`）；自研能力一律进 `/web/*`。
- 明确职责边界：业务规则（复核阈值、诊断策略）从 handler 里提取到 `domain`/`service` 层。
- 按接口分流：公开接口、业务接口、后台运营接口三套中间件/限流策略。
- 部署加固：收紧 CORS；给静态资源与媒体目录加签名访问；配置连接池上限与超时。

## 不要做的事（都是踩过的坑）
1. **不要往 `public` schema 写任何东西**（真实业务数据）。试验一律用 `compat_*` scratch schema。
2. **不要关掉 `serde_json` 的 `preserve_order`** —— 信封键顺序会变，契约测试立刻红。
3. **不要给 `handlers_ai` / `handlers_core` / `shared` 加回 `#[allow(dead_code)]`**：
   实测过它们掩盖的是活代码，而属性会让真死代码隐身（`W2_S5_PLAN.md` §4）。
4. **不要用裸名字 grep 判活死**：`compat/views_core.rs` 有自己的 `classify_environment_risk`，
   `web/admin.rs` 的注释里「复刻」了 `system_status_api_handler` —— 裸名字会两者都算成引用。
   判据只能用编译器或路径限定引用（`W2_S5_PLAN.md` §0.1）。
5. **不要动**：契约表建表顺序、两个保留字的双引号、`/compat` 的第二次挂载、三条静态通道。
6. **不要跑无种子的契约测试**：`compat_test` 是多流共享 schema，夹具 JSON 会被别的流重 capture。
   红之前先 `-Reset` + `-Apply` + 重灌种子（`W2_S5_PLAN.md` §0.2）。

## 代码格式约定

- 保持代码清晰易读，不要刻意压行，尤其是 Rust 代码。
- Rust 格式化必须使用 nightly 工具链，执行 `cargo +nightly fmt`，不要使用默认或 stable 工具链代替。
- 前端 HTML、CSS 和 JavaScript 同样使用正常的多行排版，不要手工压缩源文件。
