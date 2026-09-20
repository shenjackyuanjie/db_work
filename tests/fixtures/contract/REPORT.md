# Wave 0' 契约基准报告（Django 5 + DRF → Rust golden fixtures）

- **captured_at**（UTC）：`2026-09-20T14:05:36+00:00`
- **seed_signature**：`sha256:eca5166b7d330e244a7d7b00cb3044649bed9632131abf85ca0a0a33e4a16851`
- 夹具目录：`db/tests/fixtures/contract/`
- 蓝本：`navel_backend_git/`（Django 5.0.14 / DRF 3.15.2 / Python 3.12.10）
- 本次录制域：agent, auth, commerce, core, orchard_trace

## 1. 覆盖统计

- 自省 `api/urls.py`：**79 条 `path()`**（任务书里写的 103 有误，实测 79）
- 域划分：auth 9 / core 14 / orchard_trace 21 / commerce 24 / agent 11（合计 79）
- 用例总数：**246**（path×method 上限 99）
- covered（有用例且状态码与设计一致）：**79 / 79**
- partial（有用例但状态码与设计不符）：**0**
- blocked：**0**
- unresolved（自省失败，不静默跳过）：**0**（无）

> 域划分按**路径前缀**：`/api/v1/farmer/*` 归 orchard_trace，`/api/{products,cart,addresses,orders,after-sales}` 及其 `v1/` 别名归 commerce。每条 `path()` 至少 1 条用例；数值以实际录制的 `expected_status` 为准，`expected_status_design` 保留人工设计值以便发现蓝本漂移。

### 1.1 逐域覆盖

| 域 | path | 用例 | 200 | 400 | 401 | 403 | 404 | 405 | 5xx | shape_only |
|---|---|---|---|---|---|---|---|---|---|---|
| auth | 9 | 26 | 9 | 4 | 13 | 0 | 0 | 0 | 0 | 0 |
| core | 14 | 42 | 16 | 9 | 6 | 8 | 1 | 0 | 2 | 4 |
| commerce | 24 | 77 | 38 | 13 | 5 | 10 | 11 | 0 | 0 | 0 |
| orchard_trace | 21 | 65 | 34 | 7 | 4 | 10 | 9 | 0 | 1 | 3 |
| agent | 11 | 36 | 18 | 0 | 9 | 0 | 0 | 0 | 9 | 0 |

逐 path 明细（用例数 / 覆盖到的状态码）：

| path | name | 方法 | 用例 | 状态码 |
|---|---|---|---|---|
| `/api/register` | `register_api` | POST | 5 | 200×1, 400×4 |
| `/api/login` | `login_api` | POST | 5 | 200×1, 401×4 |
| `/api/logout` | `logout_api` | POST | 3 | 200×1, 401×2 |
| `/api/me` | `me_api` | GET | 5 | 200×2, 401×3 |
| `/api/v1/auth/register` | `v1_register_api` | POST | 1 | 200×1 |
| `/api/v1/auth/login` | `v1_login_api` | POST | 1 | 200×1 |
| `/api/v1/auth/logout` | `v1_logout_api` | POST | 3 | 200×1, 401×2 |
| `/api/v1/auth/me` | `v1_me_api` | GET | 1 | 401×1 |
| `/api/user` | `get_user_api` | GET | 2 | 200×1, 401×1 |
| `/api/products` | `product_list_api` | GET | 4 | 200×4 |
| `/api/products/<uuid:product_id>` | `product_detail_api` | GET | 2 | 200×1, 404×1 |
| `/api/products/<uuid:product_id>/health-archive` | `product_health_archive_api` | GET | 2 | 200×1, 404×1 |
| `/api/orchards` | `orchard_list_api` | GET | 2 | 200×2 |
| `/api/orchards/<uuid:orchard_id>` | `orchard_detail_api` | GET | 3 | 200×1, 404×2 |
| `/api/supply-batches` | `supply_batch_list_api` | GET | 1 | 200×1 |
| `/api/supply-batches/<uuid:batch_id>` | `supply_batch_detail_api` | GET | 2 | 200×1, 404×1 |
| `/api/traces/<str:trace_code>` | `trace_lookup_api` | GET | 6 | 200×5, 404×1 |
| `/api/cart` | `cart_api` | GET, POST | 8 | 200×4, 400×2, 401×1, 403×1 |
| `/api/cart/<uuid:item_id>` | `cart_item_api` | PATCH, DELETE | 5 | 200×3, 403×1, 404×1 |
| `/api/addresses` | `address_api` | GET, POST | 4 | 200×2, 400×1, 401×1 |
| `/api/addresses/<uuid:address_id>` | `address_detail_api` | PATCH, DELETE | 3 | 200×1, 404×2 |
| `/api/v1/addresses` | `v1_address_api` | GET, POST | 3 | 200×1, 403×2 |
| `/api/v1/addresses/<uuid:address_id>` | `v1_address_detail_api` | PATCH, DELETE | 2 | 403×1, 404×1 |
| `/api/orders` | `order_api` | GET, POST | 7 | 200×2, 400×2, 401×1, 403×1, 404×1 |
| `/api/orders/<uuid:order_id>` | `order_detail_api` | GET | 2 | 200×1, 404×1 |
| `/api/orders/<uuid:order_id>/pay` | `order_pay_api` | POST | 3 | 200×1, 400×1, 401×1 |
| `/api/orders/<uuid:order_id>/cancel` | `order_cancel_api` | POST | 4 | 200×1, 400×2, 403×1 |
| `/api/after-sales` | `after_sale_api` | GET, POST | 5 | 200×2, 400×1, 401×1, 403×1 |
| `/api/v1/supply-batches` | `v1_supply_batch_list_api` | GET | 1 | 200×1 |
| `/api/v1/supply-batches/<uuid:batch_id>` | `v1_supply_batch_detail_api` | GET | 1 | 200×1 |
| `/api/v1/traces/<str:trace_code>` | `v1_trace_lookup_api` | GET | 1 | 200×1 |
| `/api/v1/orchards` | `v1_orchard_list_api` | GET | 1 | 200×1 |
| `/api/v1/orchards/<uuid:orchard_id>` | `v1_orchard_detail_api` | GET | 1 | 200×1 |
| `/api/v1/products` | `v1_product_list_api` | GET | 1 | 200×1 |
| `/api/v1/products/<uuid:product_id>` | `v1_product_detail_api` | GET | 1 | 200×1 |
| `/api/v1/products/<uuid:product_id>/health-archive` | `v1_product_health_archive_api` | GET | 1 | 200×1 |
| `/api/v1/cart` | `v1_cart_api` | GET, POST | 4 | 200×3, 400×1 |
| `/api/v1/cart/<uuid:item_id>` | `v1_cart_item_api` | PATCH, DELETE | 3 | 200×1, 403×1, 404×1 |
| `/api/v1/orders` | `v1_order_api` | GET, POST | 4 | 200×3, 400×1 |
| `/api/v1/orders/<uuid:order_id>` | `v1_order_detail_api` | GET | 1 | 200×1 |
| `/api/v1/orders/<uuid:order_id>/pay` | `v1_order_pay_api` | POST | 3 | 200×1, 403×1, 404×1 |
| `/api/v1/orders/<uuid:order_id>/cancel` | `v1_order_cancel_api` | POST | 2 | 400×1, 404×1 |
| `/api/v1/after-sales` | `v1_after_sale_api` | GET, POST | 3 | 200×2, 400×1 |
| `/api/v1/farmer/orchards` | `farmer_orchards_api` | GET | 3 | 200×1, 401×1, 403×1 |
| `/api/v1/farmer/orchards/<uuid:orchard_id>/trees` | `farmer_tree_archive_api` | GET, POST | 6 | 200×2, 400×1, 403×1, 404×1, 500×1 |
| `/api/v1/farmer/batches` | `farmer_batches_api` | GET | 3 | 200×2, 403×1 |
| `/api/v1/farmer/batches/create` | `farmer_batch_create_api` | POST | 6 | 200×2, 401×1, 403×1, 404×2 |
| `/api/v1/farmer/upload-image` | `farmer_image_upload_api` | POST | 3 | 200×1, 400×1, 403×1 |
| `/api/v1/farmer/products` | `farmer_product_list_create_api` | GET, POST | 5 | 200×2, 400×2, 403×1 |
| `/api/agent/context` | `agent_context_api` | GET | 3 | 200×2, 401×1 |
| `/api/agent/daily-report` | `agent_daily_report_api` | GET | 3 | 200×2, 401×1 |
| `/api/agent/select` | `agent_select_api` | POST | 2 | 200×1, 500×1 |
| `/api/agent/inquiry` | `agent_inquiry_api` | POST | 2 | 200×1, 500×1 |
| `/api/agent/risk-alert` | `agent_risk_alert_api` | GET | 2 | 200×1, 401×1 |
| `/api/agent/repurchase` | `agent_repurchase_api` | GET | 3 | 200×2, 401×1 |
| `/api/agent/chat` | `agent_chat_api` | POST | 3 | 200×1, 401×1, 500×1 |
| `/api/agent/approvals` | `agent_approval_create_api` | POST | 3 | 200×1, 401×1, 500×1 |
| `/api/agent/feedback` | `agent_feedback_api` | GET, POST | 5 | 200×2, 401×1, 500×2 |
| `/api/agent/approvals/list` | `agent_approval_list_api` | GET | 4 | 200×3, 401×1 |
| `/api/agent/approvals/<uuid:approval_id>/decision` | `agent_approval_decision_api` | POST | 6 | 200×2, 401×1, 500×3 |
| `/api/v1/farmer/products/<uuid:product_id>` | `farmer_product_detail_api` | PUT, PATCH | 5 | 200×2, 401×1, 403×1, 404×1 |
| `/api/v1/farmer/batches/<uuid:batch_id>/harvest-archives` | `farmer_harvest_archive_api` | GET, POST | 4 | 200×2, 400×1, 403×1 |
| `/api/v1/farmer/batches/<uuid:batch_id>/trace-events` | `farmer_trace_event_api` | POST | 3 | 200×1, 400×1, 403×1 |
| `/api/v1/farmer/batches/<uuid:batch_id>/quality-samples` | `farmer_quality_sample_api` | GET, POST | 4 | 200×2, 400×1, 403×1 |
| `/api/v1/farmer/batches/<uuid:batch_id>/health-records` | `farmer_product_health_record_api` | GET, POST | 4 | 200×2, 401×1, 404×1 |
| `/api/home` | `home_api` | GET | 3 | 200×1, 401×1, 403×1 |
| `/api/growth-tracking` | `growth_tracking_api` | GET | 2 | 200×1, 403×1 |
| `/api/diagnose` | `diagnose_api` | GET | 2 | 200×1, 401×1 |
| `/api/temperature-humidity` | `temperature_humidity_api` | GET, POST | 7 | 200×2, 400×3, 401×1, 403×1 |
| `/api/generate` | `generate_api` | GET | 2 | 200×1, 401×1 |
| `/api/generate/fertilization-plan` | `fertilization_plan_api` | POST | 2 | 500×2 |
| `/api/citrus-disease` | `citrus_disease_api` | POST | 3 | 200×1, 400×1, 403×1 |
| `/api/recognition-records` | `recognition_records_api` | GET | 2 | 200×1, 403×1 |
| `/api/disease-treatment` | `get_disease_treatment_api` | GET | 4 | 200×2, 400×1, 403×1 |
| `/api/tasks` | `get_tasks_api` | GET | 3 | 200×1, 401×1, 403×1 |
| `/api/tasks/add` | `add_task_api` | POST | 4 | 200×1, 400×1, 401×1, 403×1 |
| `/api/tasks/complete` | `complete_task_api` | POST | 3 | 200×1, 400×1, 404×1 |
| `/api/tasks/generate/disease` | `generate_task_from_disease_api` | POST | 3 | 200×2, 400×1 |
| `/api/tasks/generate/environment` | `generate_task_from_environment_api` | POST | 2 | 200×1, 400×1 |

## 2. 未覆盖 / partial 清单

**79 条 path 全部至少 1 条用例，uncovered = 0。**

所有用例的实际状态码与设计期望一致（无 partial 偏差）。

### 2.1 已知未覆盖的语义分支（及原因）

- `/api/temperature-humidity` GET 的 **404 分支**（该果农无记录且 CSV 缺失）无法覆盖：seed 给每个果农 10 条记录，且仓库内不存在 `temperature_humidity_data.csv`。
- `/api/orders/<id>/pay` 的**订金/尾款分支**未覆盖：seed 的批次全部 `payment_mode='full'`；构造 `deposit_balance` 批次需要改 seed，会破坏夹具一致性。
- `/api/orders/<id>/cancel` 的**退款分支**（`paid_amount>0` 时插 `MOCK-REFUND-*`）未覆盖：已支付订单会被 400 拦下（契约如此）。
- `/api/v1/farmer/batches/<uuid>/quality-samples` GET 的 **product_id 过滤**未单独出例：已覆盖无过滤的 200 与缺字段 400。
- `/api/agent/approvals/<id>/decision` 的 `payload.action='create_task'` 执行分支未覆盖：已覆盖 `action` 缺失 -> `exec_note` 为空 的分支。
- `/api/v1/farmer/upload-image` 的 >5MB / 伪造 content_type 分支未覆盖：需要构造大文件或改写 MIME。
- `/admin/`（Django admin）与 `/media/` 静态兜底不属于 App API 契约，未录制。

## 3. shape_only 清单（只校验键与类型，不校验值）

| 用例 | path | 原因 |
|---|---|---|
| `diagnose_shape_only` | `/api/diagnose` | shape_only：惰性创建后返回聚合结构，键固定、值随数据变 |
| `recognition_records_shape_only` | `/api/recognition-records` | shape_only：Django 侧 mock、Rust 侧真 ONNX；seed 记录 image 为空 |
| `tasks_generate_environment_shape_only` | `/api/tasks/generate/environment` | shape_only：风险分级由规则常量表决定，Rust 侧需逐字复刻规则 |
| `citrus_disease_upload_shape_only` | `/api/citrus-disease` | shape_only：走 base64 IMAGE 分支；Django 侧 mock 随机识别，Rust 侧真 ONNX |
| `farmer_upload_image_shape_only` | `/api/v1/farmer/upload-image` | shape_only：返回绝对 URL（含 host 与随机文件名） |
| `farmer_batch_create_shape_only` | `/api/v1/farmer/batches/create` | shape_only：code/trace_code 由 uuid 生成，Rust 侧无法逐字复现 |
| `farmer_quality_sample_create_shape_only` | `/api/v1/farmer/batches/a8756f98-4664-4fe6-9df5-fe96758fb818/quality-samples` | shape_only：写入同时追加 TraceEvent（哈希链依赖时间序列） |

shape_only 用例的键/类型契约写在用例的 `shape` 字段里（形如 `$.data.items[*].name:str`），Rust 侧按此校验，不要比对数值。

## 4. 时间格式实证

`USE_TZ=True` + `TIME_ZONE='UTC'` 下所有 datetime 走 DRF 默认表示：
`YYYY-MM-DDTHH:MM:SS.ffffff+00:00` —— **带 `+00:00` 偏移、6 位微秒**，不是 `Z` 结尾。

| JSON 路径 | 实测样例 | 判定 |
|---|---|---|
| `$.me_ok_buyer.data.created_at` | `2026-09-20T14:05:39.686730Z` | ❌ 形状异常 |
| `$.me_ok_farmer.data.created_at` | `2026-09-20T14:05:38.779271Z` | ❌ 形状异常 |
| `$.user_alias_ok_farmer.data.created_at` | `2026-09-20T14:05:38.779271Z` | ❌ 形状异常 |
| `$.login_ok_farmer.data.expiresAt` | `2026-10-20T14:05:40.748454+00:00` | ✅ 带 +00:00 与微秒 |
| `$.login_ok_farmer.data.user.created_at` | `2026-09-20T14:05:38.779271Z` | ❌ 形状异常 |
| `$.v1_login_ok_farmer.data.expiresAt` | `2026-10-20T14:05:41.391258+00:00` | ✅ 带 +00:00 与微秒 |
| `$.v1_login_ok_farmer.data.user.created_at` | `2026-09-20T14:05:38.779271Z` | ❌ 形状异常 |
| `$.register_ok_buyer.data.expiresAt` | `2026-10-20T14:05:41.700861+00:00` | ✅ 带 +00:00 与微秒 |
| `$.register_ok_buyer.data.user.created_at` | `2026-09-20T14:05:41.699862Z` | ❌ 形状异常 |
| `$.v1_register_ok_farmer.data.expiresAt` | `2026-10-20T14:05:42.003211+00:00` | ✅ 带 +00:00 与微秒 |
| `$.v1_register_ok_farmer.data.user.created_at` | `2026-09-20T14:05:42.002209Z` | ❌ 形状异常 |
| `$.diagnose_shape_only.data.getList.list[*].created_at` | `2026-09-20T14:05:42.036209Z` | ❌ 形状异常 |
| `$.temperature_humidity_get_ok.data.recentRecords[*].record_time` | `2026-09-11T12:05:39.700728+00:00` | ✅ 带 +00:00 与微秒 |
| `$.tasks_list_ok.data[*].created_at` | `2026-09-20T14:05:39.701731Z` | ❌ 形状异常 |
| `$.recognition_records_shape_only.data.records[*].created_at` | `2026-09-20T14:05:39.701731Z` | ❌ 形状异常 |
| `$.temperature_humidity_post_ok.data.recentRecords[*].record_time` | `2026-09-12T12:05:39.700728+00:00` | ✅ 带 +00:00 与微秒 |
| `$.tasks_add_ok.data.created_at` | `2026-09-20T14:05:42.097849Z` | ❌ 形状异常 |
| `$.tasks_complete_ok.data.created_at` | `2026-09-20T14:05:42.097849Z` | ❌ 形状异常 |
| `$.tasks_complete_ok.data.completed_at` | `2026-09-20T22:05:42.101849Z` | ❌ 形状异常 |
| `$.tasks_generate_disease_ok.data.created_at` | `2026-09-20T14:05:42.107849Z` | ❌ 形状异常 |
| `$.tasks_generate_environment_shape_only.data.created_at` | `2026-09-20T14:05:42.114852Z` | ❌ 形状异常 |
| `$.products_list_ok.data.items[*].sales_batch.open_at` | `2026-08-21T14:05:39.709730Z` | ❌ 形状异常 |
| `$.products_list_search_ok.data.items[*].sales_batch.open_at` | `2026-08-21T14:05:39.709730Z` | ❌ 形状异常 |
| `$.products_list_sku_filter_ok.data.items[*].sales_batch.open_at` | `2026-08-21T14:05:39.709730Z` | ❌ 形状异常 |
| `$.products_list_v1_ok.data.items[*].sales_batch.open_at` | `2026-08-21T14:05:39.709730Z` | ❌ 形状异常 |
| `$.product_detail_ok.data.sales_batch.open_at` | `2026-08-21T14:05:39.688730Z` | ❌ 形状异常 |
| `$.product_detail_ok.data.sales_batch.close_at` | `2026-11-19T14:05:39.688730Z` | ❌ 形状异常 |
| `$.product_detail_ok.data.sales_batch.environment_summary.recordedAt` | `2026-09-20T14:05:39.690731+00:00` | ✅ 带 +00:00 与微秒 |
| `$.product_detail_ok.data.sales_batch.orchard.verified_at` | `2026-07-22T14:05:39.686730Z` | ❌ 形状异常 |
| `$.product_detail_v1_ok.data.sales_batch.open_at` | `2026-08-21T14:05:39.688730Z` | ❌ 形状异常 |

- 形状规则：`^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{1,6})?\+00:00$`
- 形状不符的样例数：**62**
- `DateField`（`recognitionDate`、`expectedHarvestStart/End`、`harvest_date`）输出 `YYYY-MM-DD`，无时间部分。
- envelope 的 `$.timestamp` 是**毫秒整数**（`int(timezone.now().timestamp()*1000)`），不是 ISO 字符串。
- ⚠️ 一处例外：`complete_task_api` 用朴素 `datetime.now()` 写 `completed_at`，DRF 会输出**不带偏移**的 `2026-09-20T21:05:50.123456`。Rust 侧若要逐字兼容需单独处理这一支。
- `TraceEvent.evidence_hash` 的输入含 `occurred_at.isoformat()` 的**微秒**，任何时间改写都会破坏哈希链（`integrity.chainValid` 变 false）。

## 5. 契约怪癖（Rust 侧必须逐字复刻）

### 5.1 响应体形状

- 成功体：`{code, message, data, timestamp}`，`timestamp` 为**毫秒整数**。
- 异常体（`api/exceptions.py::custom_exception_handler`）：`{code, message, data}` —— **没有 `timestamp`**。
- 未捕获异常也走异常体：`{code:500, message:'Internal server error', data:null}`。
- 但 `views.py` 的 `except Exception -> create_response(None, str(e), 500)` 属**成功体形状**（带 timestamp，message 是异常文本）。两条 500 路径形状不同。
- 序列化器校验失败时 `message` 是**对象**（`{'字段': ['文案']}` 或 `{'non_field_errors': [...]}`），不是字符串。

### 5.2 错误文案字典（逐字，取自实际响应）

| 状态码 | message | 出处 |
|---|---|---|
| 401 | `身份认证信息未提供。` | `/api/me` |
| 401 | `Authorization 请求头格式无效` | `/api/me` |
| 401 | `登录凭证无效` | `/api/me` |
| 405 | `方法 “DELETE” 不被允许。` | `/api/me` |
| 401 | `{"password": ["该字段是必填项。"]}` | `/api/login` |
| 401 | `{"username": ["该字段是必填项。"], "password": ["该字段是必填项。"]}` | `/api/login` |
| 401 | `{"non_field_errors": ["Invalid credentials"]}` | `/api/login` |
| 400 | `{"orchard_address": ["果农需要填写果园地址"]}` | `/api/register` |
| 400 | `{"role": ["“operator” 不是合法选项。"]}` | `/api/register` |
| 400 | `{"password": ["请确保这个字段至少包含 6 个字符。"]}` | `/api/register` |
| 400 | `{"username": ["具有 username 的 User 已存在。"]}` | `/api/register` |
| 403 | `该接口仅限果农使用` | `/api/home` |
| 400 | `disease_name parameter is required` | `/api/disease-treatment` |
| 400 | `{"title": ["该字段是必填项。"], "description": ["该字段是必填项。"]}` | `/api/tasks/add` |
| 400 | `task_id is required` | `/api/tasks/complete` |
| 404 | `Task not found` | `/api/tasks/complete` |
| 400 | `disease_name is required` | `/api/tasks/generate/disease` |
| 400 | `temperature and humidity are required` | `/api/tasks/generate/environment` |
| 400 | `No image provided` | `/api/citrus-disease` |
| 500 | `name 'FertilizationPlanRequestSerializer' is not defined` | `/api/generate/fertilization-plan` |
| 400 | `temperature and humidity must be numbers` | `/api/temperature-humidity` |
| 400 | `temperature or humidity is outside the supported range` | `/api/temperature-humidity` |
| 400 | `record_time must be ISO 8601` | `/api/temperature-humidity` |
| 404 | `商品不存在或已下架` | `/api/products/00000000-0000-4000-8000-000000000000` |
| 404 | `商品不存在、已下架或尚未关联供货档案` | `/api/products/00000000-0000-4000-8000-000000000000/health-archive` |
| 403 | `该接口仅限购买者使用` | `/api/cart` |
| 400 | `{"product_id": ["该字段是必填项。"]}` | `/api/cart` |
| 400 | `{"product_id": ["Must be a valid UUID."]}` | `/api/cart` |
| 400 | `{"quantity": ["请确保该值大于或者等于 1。"]}` | `/api/v1/cart` |
| 404 | `购物车商品不存在` | `/api/cart/00000000-0000-4000-8000-000000000000` |
| 400 | `{"recipient_name": ["该字段是必填项。"], "phone": ["该字段是必填项。"], "detail": ["该字段是必填项。"]}` | `/api/addresses` |
| 404 | `收货地址不存在` | `/api/addresses/00000000-0000-4000-8000-000000000000` |
| 405 | `方法 “GET” 不被允许。` | `/api/v1/orders/105e11d5-4839-4d64-87eb-03e134ff8722/pay` |
| 404 | `订单不存在` | `/api/orders/00000000-0000-4000-8000-000000000000` |
| 400 | `{"address_id": ["该字段是必填项。"]}` | `/api/orders` |
| 400 | `当前订单状态不能取消` | `/api/orders/105e11d5-4839-4d64-87eb-03e134ff8722/cancel` |
| 400 | `当前订单状态不能支付` | `/api/orders/105e11d5-4839-4d64-87eb-03e134ff8722/pay` |
| 400 | `{"order": ["该字段是必填项。"], "issue_type": ["该字段是必填项。"], "description": ["该字段是必填项。"]}` | `/api/after-sales` |
| 400 | `{"order": ["无效主键 “00000000-0000-4000-8000-000000000000” － 对象不存在。"]}` | `/api/v1/after-sales` |
| 400 | `请选择有效的购物车商品` | `/api/orders` |
| 404 | `果园不存在或尚未通过认证` | `/api/orchards/00000000-0000-4000-8000-000000000000` |
| 404 | `供货批次不存在` | `/api/supply-batches/00000000-0000-4000-8000-000000000000` |
| 404 | `未查询到对应的来源追溯记录` | `/api/traces/CGJ-NOT-EXIST` |
| 404 | `果园不存在或无权操作` | `/api/v1/farmer/orchards/cf76b166-e6a5-4088-a971-f50850f76e82/trees` |
| 400 | `{"tree_number": ["该字段是必填项。"], "variety": ["该字段是必填项。"]}` | `/api/v1/farmer/orchards/cf76b166-e6a5-4088-a971-f50850f76e82/trees` |
| 400 | `未提供图片文件` | `/api/v1/farmer/upload-image` |
| 400 | `{"name": ["该字段是必填项。"], "origin": ["该字段是必填项。"], "price": ["该字段是必填项。"]}` | `/api/v1/farmer/products` |
| 404 | `商品不存在或无权操作` | `/api/v1/farmer/products/0fa00dc3-2d5c-437a-a933-6f67f1b53d3a` |
| 400 | `{"harvested_at": ["该字段是必填项。"], "picker": ["该字段是必填项。"], "quantity_kg": ["该字段是必填项。"]}` | `/api/v1/farmer/batches/a8756f98-4664-4fe6-9df5-fe96758fb818/harvest-archives` |
| 400 | `{"event_type": ["该字段是必填项。"], "occurred_at": ["该字段是必填项。"], "title": ["该字段是必填项。"]}` | `/api/v1/farmer/batches/a8756f98-4664-4fe6-9df5-fe96758fb818/trace-events` |
| 400 | `{"sampled_at": ["该字段是必填项。"]}` | `/api/v1/farmer/batches/a8756f98-4664-4fe6-9df5-fe96758fb818/quality-samples` |
| 404 | `批次不存在或无权操作` | `/api/v1/farmer/batches/a8756f98-4664-4fe6-9df5-fe96758fb818/health-records` |
| 500 | `Internal server error` | `/api/v1/farmer/orchards/cf76b166-e6a5-4088-a971-f50850f76e82/trees` |
| 400 | `上架商品必须关联一个供货批次` | `/api/v1/farmer/products` |

### 5.3 蓝本自身的缺陷（**必须原样复刻，否则契约比对会失败**）

- `api/agent_views.py` 顶层**没有** `from rest_framework import status`，却在 9 处使用 `status.HTTP_4xx_*`。因此下列「应为 400/404」的分支**实际返回 500 `{'code':500,'message':'Internal server error','data':null}`**：
  - `POST /api/agent/select` 缺 query
  - `POST /api/agent/inquiry` 缺 query
  - `POST /api/agent/chat` 缺 query
  - `POST /api/agent/feedback` rating 缺失 / 非 1-5
  - `POST /api/agent/approvals` 缺 title
  - `POST /api/agent/approvals/<id>/decision` 审批单不存在 / decision 非法 / 重复决策
  （对照：`api/commerce_views.py`、`api/auth_views.py` 都正确 import 了 status）
- `api/views.py::fertilization_plan_api` 引用未定义的 `FertilizationPlanRequestSerializer`（`api/serializers.py` 有定义，但 `views.py` 没 import）-> **`POST /api/generate/fertilization-plan` 恒返回 `500 {code:500, message:"name 'FertilizationPlanRequestSerializer' is not defined"}`**（成功体形状，带 timestamp）。
- 请与产品确认：是有意修 bug（那 Rust 侧不该复刻这些 500），还是按现状等价？**当前夹具按实际行为录制。**

### 5.4 鉴权与权限

- `Authorization: Bearer <uuid>`：
  - 头存在但格式不符 -> 401 `Authorization 请求头格式无效`
  - token 查不到 -> 401 `登录凭证无效`
  - token 过期 -> 401 `登录已过期，请重新登录`
  - 完全没头 -> 401 `身份认证信息未提供。`（**中文**，DRF zh-hans 本地化）
- 权限不足 -> 403 `该接口仅限果农使用`（IsFarmer）/ `该接口仅限购买者使用`（IsBuyer）
- 方法不允许 -> 405 `方法 “DELETE” 不被允许。`（zh-hans，含全角引号）
- 401 响应带 `WWW-Authenticate: Bearer`。
- `POST /api/login` 凭据错误是 **401**（不是 400），`message` 为 `{'non_field_errors': ['Invalid credentials']}`（英文）。
- `/api/register`、`/api/login`、`/api/agent/select`、`/api/agent/inquiry` 未挂 `BearerTokenAuthentication` -> 匿名可调；传 token 也不影响（不校验）。

### 5.5 别名（`v1/*`）结论

- 自动对比 **18** 组同状态别名：normalize 后相等 **16** 组，不等 **2** 组。
  - `/api/v1/auth/register`（v1_register_ok_farmer）vs `/api/register`（register_ok_buyer）：diff=[".data.user.email: 'qa-register@example.com' != None", '.data.user.latitude: None != 25.1', '.data.user.longitude: None != 115.1', ".data.user.orchard_address: None != '赣州市信丰县 QA 果园'", ".data.user.role: 'buyer' != 'farmer'", ".data.user.username: 'qa_register_buyer' != 'qa_register_farmer'"]
  - `/api/v1/traces/CGJ-BATCH-XF2026`（traces_lookup_v1_ok）vs `/api/traces/CGJ-BATCH-XF2026`（traces_lookup_batch_ok）：diff=[".data.integrity.checkedAt: '2026-09-20T14:05:43.004202+00:00' != '2026-09-20T14:05:43.098202+00:00'"]
- 全部别名都是「同一个 `@api_view` 函数挂在两条 path 上」，无独立实现。
- `POST /api/v1/auth/login` 与 `POST /api/login` 是**同一视图**，会重建 token（`_create_session` 先删该用户全部 token）。录制时 `v1_login_ok_farmer` 排在复用 token 的用例之后；Rust 侧测试也必须遵守「一次登录、token 复用」的顺序。
- `GET /api/user` 与 `GET /api/me` 是同一视图（`auth_views.me_api`）。

### 5.6 其他怪癖

- `GET /api/traces/<code>` 依次匹配 `FruitTreeArchive.trace_code|tree_number` → `TracePackage.trace_code` → `SalesBatch.trace_code|code`，`data.scope` 为 `tree`/`package`/`batch`；package 分支的 `tracking_number` 被掩码。
- `GET /api/v1/farmer/batches/<uuid>/health-records` **复用** `farmer_quality_sample_api`（与 `quality-samples` 同一实现）。
- `POST /api/v1/farmer/batches/create` 缺 `orchard_id` 返回 **404** `果园不存在或无权操作`（视图先查果园、再校验序列化器），不是 400。
- `POST /api/tasks/generate/disease` 疾病名为 `健康果树`/`非果树` -> **200 + data=null + message='No task needed for healthy tree'**（英文）。
- `POST /api/tasks/generate/environment` 低风险 -> **200 + 'No task needed for low risk'**。
- `POST /api/orders` 会删除已下单的购物车项、扣减库存、并按数量生成 `trace_packages`（`sequence` 从 1 连号）；`trace_code` 是 uuid4 -> 必须屏蔽。
- 支付写 `provider_transaction_id = MOCK-<32位大写hex>`、`provider='mock'`；退款写 `MOCK-REFUND-<32位大写hex>`。
- `planId` 形如 `fp-YYYYMMDD-NNNN`（`random.randint(1000,9999)`）。
- 购物车/下单限制同一供货批次：`一次只能结算同一果园供货批次，请先完成或清空当前购物车`、`一笔订单只能购买同一果园供货批次的商品`。
- `PUT /api/v1/farmer/products/<id>` 传**空 body** 也返回 200 `商品已更新`（`partial=True` 且无必填校验）。
- `DELETE /api/v1/addresses/<id>` 删除默认地址时，会把剩余第一条置为 `is_default=true`。
- `POST /api/v1/farmer/batches/<uuid>/harvest-archives` 与 `.../quality-samples` 写入时会**自动追加 TraceEvent**（哈希链）。

## 6. seed.json

- 来源：`dumpdata <业务模型 label> --format=json --indent=2`（**保留主键**），排除 contenttypes / auth.* / admin.logentry / sessions。
- 行数合计：**147**

| model | rows |
|---|---|
| `api.aftersalerequest` | 3 |
| `api.agentapproval` | 4 |
| `api.agentfeedback` | 3 |
| `api.authtoken` | 7 |
| `api.batchqualitysample` | 6 |
| `api.buyeraddress` | 3 |
| `api.citrusproduct` | 8 |
| `api.diagnosedata` | 1 |
| `api.diagnoselistitem` | 3 |
| `api.diseaserecognitionrecord` | 4 |
| `api.fruittreearchive` | 8 |
| `api.growthtracking` | 1 |
| `api.harvestarchive` | 5 |
| `api.homedata` | 1 |
| `api.orchard` | 4 |
| `api.order` | 5 |
| `api.orderitem` | 5 |
| `api.paymentrecord` | 2 |
| `api.salesbatch` | 5 |
| `api.task` | 8 |
| `api.temperaturehumiditydata` | 31 |
| `api.traceevent` | 18 |
| `api.tracepackage` | 4 |
| `api.user` | 8 |

`seed.json` 是**纯 seed 态**（在跑任何用例之前导出）：Rust 侧从这里加载，然后按 `index.json.mutation_order` / 各用例数组顺序回放，每步状态自然与录制时一致。

## 7. 确定性与「当天回放」约束

- LLM 全关：`AGENT_LLM_API_KEY=''`（早于 `import api.*`）-> `agent_service._LLM_VALID is False`；脚本启动断言，不成立即 abort。
- 识别走 mock：仓库内无 `model/`，`MODEL_AVAILABLE=False`；脚本启动断言。
- `random.seed(20260913)` 在导入期设置。
- **不做时间 rebase**：`seed.json` 保存绝对时间戳。`TraceEvent.evidence_hash = sha256(规范化 JSON)`，输入含 `occurredAt.isoformat()` 的微秒，改写时间必然导致 `chainValid=false`。
- 因此 **Rust 侧必须在 `2026-09-20`（UTC 日历日）当天回放**。跨天会改变：`SalesBatch.is_open`（`close_at` 过期）、`_expire_stale_orders`（`expires_at <= now` 的待支付订单被自动取消）、日报 `today_order_count`/`today_order_amount`、复购 `days_since`、`fulfillment_risk_score` 的「发货窗口临近 / 已过预计发货日」分支。
- 若必须跨天回放：请把 Rust 侧时钟冻结到 `2026-09-20T14:05:36+00:00` 附近，或按相同相对时间重建 seed，**不要改夹具**。
- 用 `seed_signature`（`sha256(seed.json)`）识别夹具版本；回放前先校验签名。
- 临时媒体目录：`D:\githubs\db_work\.venv-django-tmp\media`（仓库外，跑完自动清理）。`navel_backend_git/media/` 与 `db.sqlite3` 全程未被写入（脚本前后对 `db.sqlite3` 做 mtime+size 双检，并把 `real_db_sha256` 记进 index/seed）。

## 8. 重跑命令

```powershell
# 0) 一次性环境（仓库外 venv；不装 torch/torchvision）
& D:\githubs\db_work\.venv-django\Scripts\python.exe -m ensurepip --upgrade
& D:\githubs\db_work\.venv-django\Scripts\python.exe -m pip install `
    "Django>=5.0,<5.1" "djangorestframework>=3.15,<3.16" `
    "django-cors-headers>=4.0" "Pillow>=10.0" "requests>=2.31"

# 1) 自检：环境 + 路由枚举 + 用例矩阵核对（不写文件）
& D:\githubs\db_work\.venv-django\Scripts\python.exe db\scripts\capture_contract.py --smoke

# 2) 全量录制
& D:\githubs\db_work\.venv-django\Scripts\python.exe db\scripts\capture_contract.py

# 3) 只重录某些域（可重复；index/seed/REPORT 仍会重写）
& D:\githubs\db_work\.venv-django\Scripts\python.exe db\scripts\capture_contract.py --domain commerce --domain agent

# 4) 输出到别处
& D:\githubs\db_work\.venv-django\Scripts\python.exe db\scripts\capture_contract.py --out D:\githubs\db_work\.tmp\contract
```

录制后复核仓库无意外改动：

```powershell
git -C D:\githubs\db_work\navel_backend_git status --short   # 应为空
git -C D:\githubs\db_work\db status --short                  # 只应看到 db/scripts/capture_contract.py 与 db/tests/fixtures/**
```

## 9. Rust 侧必须屏蔽（normalize）的字段清单

夹具里这些字段的值已被替换为 `<sha256:16hex>`：**键必须存在、类型必须一致，值不参与比较**。

| 字段名（按名递归屏蔽） | 典型位置 | 为什么必须屏蔽 |
|---|---|---|
| `timestamp` | 成功体 envelope、温湿度历史点 | 毫秒时间戳 |
| `created_at` / `createdAt` | 所有模型 / envelope | 写入时刻 |
| `updated_at` / `updatedAt` | 几乎所有模型 | 写入时刻 |
| `id` | 新建对象、`items[*]`、`trace_packages[*]`、审批单… | `uuid.uuid4()` |
| `orderNumber` / `order_number` | 订单 | `NO<时间><6hex>` |
| `expiresAt` / `expires_at` | 订单、登录 | 相对 now 生成 |
| `paidAt` / `cancelledAt` / `paid_at` / `cancelled_at` | 订单、支付 | 写入时刻 |
| `providerTransactionId` / `provider_transaction_id` | 支付记录 | `MOCK-<32hex>` |
| `token` | 登录 / 注册 | `AuthToken.key` = uuid4 |
| `code` / `trace_code` / `traceCode` | 新建批次、箱码、果树 | uuid4 派生 |
| `harvest_code` / `harvestCode` | 采摘档案 | `CGJ-HV-<10hex>` |
| `path` / `url` | 图片上传 | 随机文件名 |
| `planId` | 施肥方案 | `fp-<date>-<4位随机>`（该接口恒 500，保留以防修复） |
| `previous_hash` / `evidence_hash` / `hash_short` | 追溯事件 | 依赖上一条事件 + 写时刻 |
| `recorded_at` / `recordedAt` | 追溯事件 / env summary | 写入时刻 |
| `generated_at` | 经营日报 | 生成时刻 |
| `checkedAt` | 追溯完整性校验 | 校验时刻（每次请求都变） |
| `verifiedAt` | 果园健康档案 | seed 的绝对 `verified_at` |
| `sold_quantity` / `stock` | 批次 / 商品（写类之后） | 随订单与支付变化 |

**不屏蔽**：路径参数里的 `<uuid:...>` 用 `seed.json` 的真实 UUID —— 运行期实值记在 `index.json.seed_refs` 与 `index.json.paths[].path_params_used`。

每个用例的 `normalize`（已展开的 JSON 路径清单）与 `normalize_hits`（录制时实际命中的字段名）都在对应域 JSON 里，可直接消费。

