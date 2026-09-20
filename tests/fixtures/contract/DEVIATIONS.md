# 契约偏差登记（Django → Rust）

本文件是**唯一的偏差权威清单**。四个域的实现在遇到「蓝本行为与设计意图不一致」时，
按这里的裁定执行；新增偏差必须登记在此，不允许悄悄偏离。

裁定时间：2026-09-20。基准来源：`db/tests/fixtures/contract/`（251 条 golden 用例）。

## 已裁定

| ID | 项 | 蓝本实际行为 | 我方决定 | 理由 |
|---|---|---|---|---|
| **D1** | `api/agent_views.py` 漏 `import status` | `/api/agent/select`、`inquiry`、`chat`、`feedback`、`approvals`、`approvals/<id>/decision` 的**全部校验分支恒返回 500** `Internal server error`（DRF 异常体，无 timestamp） | **修正为设计意图的 400/404** | 用户裁定。App 不可能依赖 500 崩溃；复刻等于把 bug 搬进新后端 |
| **D2** | `api/views.py::fertilization_plan_api` 引用未导入的 `FertilizationPlanRequestSerializer` | `POST /api/generate/fertilization-plan` **恒 500**（message 是 Python 异常文本，且带 timestamp） | **按序列化器意图实现正常语义** | 该接口在蓝本里从未可用，App 不可能在用它。**需 App 侧确认**，见「待确认」 |
| **D3** | `complete_task_api` 写 `completed_at` 用 `datetime.now()` | ~~序列化出无时区偏移的本地时间串~~ **【已更正】** 实测走 DRF 序列化，输出的是 `…294565Z`（带 `Z`、6 位微秒） | **✅ 已解决（2026-09-20 收尾）：改用 `ser::dt_z`** | 前一版结论基于误读。该字段被 `$..completed_at` normalize 屏蔽（写入时刻、逐值不可比），端到端**看不出**形态错，故加 3 条单测钉死形态（见收口记录 §1）。`ser::dt_naive_local` 此后无调用方，但它是冻结接口，未删 |
| **D4** | 时间后缀两种形态并存 | `Z` 718 处（DRF `JSONEncoder`）、`+00:00` 152 处（DRF `DateTimeField`） | **按字段逐个对齐夹具实测值**；比对器把两者归一化后比较，并单独计数漂移 | 两者语义等价，App 侧 `new Date()` 解析结果相同；但要在报告里可见 |
| **D5** | 重复 `tree_number` 建树 | DB 唯一约束 `IntegrityError` 冒泡 → **500** | **返回 400** | 输入校验错误应当是 4xx |
| **D6** | `category_labels` 顺序不可复现 | `commerce_serializers.py` 的 `.distinct()` 无 `order_by`，两次运行顺序不同 | **Rust 侧定序输出**；比对时按**集合**比较 | 蓝本自身不确定，无法逐字节对齐 |
| **D7** | 蓝本 `choices` 不生成 SQL `CHECK` | Django 只建裸 `varchar`，Python 层校验 | **我方可保留 CHECK**（更严格） | CHECK 不进 HTTP 契约；夹具取值均来自同一批 `TextChoices`，不会误伤 |
| **D8** | 蓝本不生成 SQL `DEFAULT` | 默认值由 Python 层兜底 | **我方可保留 DEFAULT** | 不进 HTTP 契约；且 W1 漏列时行为与 Django 的 Python 默认一致 |
| **D9** | 蓝本外键不带 `ON DELETE` | 级联在 Python 层（`on_delete`） | **我方按 `on_delete` 写 CASCADE/SET NULL/RESTRICT，并加 `DEFERRABLE INITIALLY DEFERRED`** | 保证数据一致性；`DEFERRABLE` 让夹具乱序导入在单事务内也成立 |
| **D10** | `login` 凭据错误分支的信封形状 | 蓝本源码（`serializers.ValidationError`）看似该走 DRF 异常体（**无** timestamp），但实测录到的是 `{code:401, message:{"non_field_errors":["Invalid credentials"]}, data:null, timestamp:…}`——**成功体形状、message 是对象、带 timestamp** | **取夹具** | 这是 App 实际收到的字节。`views_auth.rs` 已用 ⚠️ 注释标出该反直觉点 |
| **D11** | 字符串长度校验 | 蓝本靠 DB 约束（`VARCHAR(150)` 等），超长会变成 500 | **✅ 已解决（2026-09-20 收尾）：契约层按蓝本 `max_length` 校验，返回 400** | 原先 `username > 150` / `email > 254` / `orchard_address > 255` 会撞 `VARCHAR` 列宽变 **500**。上限与文案都是**实测**蓝本序列化器拿到的（`请确保这个字段不能超过 N 个字符。`）。**`password` 未加上限** —— 蓝本也没有（见收口记录 §2） |
| **D12** | 夹具把「平局时的数据库返回序」当成了契约 | 蓝本 `Task` / `AgentFeedback` 的 `Meta.ordering` 只有 `-created_at` 没有次级键；测试库里多条记录 `created_at` **完全相同**，SQLite 按 rowid 返回，PG 按物理顺序返回，两者不同 | **接受为夹具不确定，实现侧不迁就**（不为匹配某个物理顺序而写凑数代码） | 残留 4 条失败全部源于此：`core/tasks_list_ok`、`agent/agent_context_ok`、`agent/agent_feedback_get_buyer_ok`、`agent/agent_risk_alert_ok`。**建议夹具给这些列表补稳定次级键**，或让 seed 不再产生平局。这是夹具自身的不确定性，不是实现缺陷 |
| **D13** | 单测写死了录制实例的字面量 | — | **✅ 已解决（2026-09-20 收尾）：commerce 单测改为从夹具 `expected_body` / `index.json.seed_refs` 派生，或断言结构性质** | 重录夹具后 `commerce_tests.rs` 8 条失败（`10 != 7`、商品/订单/地址/购物车 uuid 字面量、`category_labels` 与 `archiveCode` 字面量）**不是实现回归**。改法与「重录前后两次全绿」的验证见收口记录 §3 |

## 收口记录（2026-09-20 收尾：D3 / D11 / D13）

三项待办全部关闭。本节保留历史、记录**修法**与**证据**。
本轮只改了 `src/compat/views_core.rs`、`src/compat/views_auth.rs`、
`src/compat/tests/commerce_tests.rs` 与本文件；夹具 JSON / `scripts/` / `ser.rs` / `errors.rs` 未动
（重录验证前后各备份一次，验证完**原样恢复**，16 个夹具文件逐文件 sha256 一致）。

### §1 D3 —— `completed_at` 的序列化形态

| | |
|---|---|
| 修法 | `views_core.rs::task_payload` 改用 `ser::dt_z`；取值抽成 `completed_at_value(Option<DateTime<Utc>>) -> Value`，让单测能直接钉形态 |
| 为什么必须靠单测 | 该字段被 `replay_diff.py` 的 `$..completed_at` normalize 屏蔽（写入时刻），端到端回放**跑绿也证明不了形态对** |
| 证据 | `completed_at_uses_drf_z_form`（`Z` + 6 位微秒）、`completed_at_is_neither_naive_nor_offset`（既不是 `+00:00` 也不是无偏移朴素串）、`completed_at_is_null_for_unfinished_tasks` |
| 全仓复核 | `rg dt_naive_local src/` 只剩 `ser.rs` 里的定义与它自己的单测 —— **再无调用方**，没有别的字段误用无偏移形态。helper 是冻结接口，按约定未删 |

### §2 D11 —— 字符串长度校验

**上限与文案都是实测出来的**，不是照抄任务描述：直接跑蓝本
`UserRegistrationSerializer(data=...).is_valid()` 取 `serializer.errors`
（`LANGUAGE_CODE=zh-hans`、DRF 3.15.2）。结论：

| 字段 | 上限 | 来源 | 超长时的响应 |
|---|---|---|---|
| `username` | 150 | 模型 `CharField(max_length=150)`，`ModelSerializer` 自动带上限 | `{"username": ["请确保这个字段不能超过 150 个字符。"]}` |
| `email` | 254 | 模型 `EmailField()`，Django 默认 `max_length` | `{"email": ["请确保这个字段不能超过 254 个字符。"]}` |
| `orchard_address` | 255 | 模型 `CharField(max_length=255)`（也是客户端可传字段） | `{"orchard_address": ["请确保这个字段不能超过 255 个字符。"]}` |
| `password` | **无** | 序列化器**显式**写 `CharField(write_only=True, required=True, min_length=6)`，**覆盖**了模型的 `max_length=128` | **不报错**：129 / 300 字符 `is_valid=True` |
| `latitude` / `longitude` | 无 | `FloatField` | `1e30` 也 `is_valid=True` |
| login 的 `username` / `password` | **无** | `UserLoginSerializer` 只写 `required=True` | 查不到 → 401 `Invalid credentials`（400 字符也是 401，不会 400/500） |

- **`password` 没有补上限，这是刻意的**：蓝本没有，且我方 `hash_password` 落库的是定长
  pbkdf2 串（≤128 字符），本来就不会撞列宽。给口令补一个 `max_length=128` 不是「补全缺口」，
  而是**制造新偏差**。任务描述里的「`password > 128` 会 500」与实测不符 —— 以实测为准。
  （`register_accepts_password_longer_than_column_width` 是这条的**反向钉子**：谁补上限它就红。）
- 长度按**字符数**判，不是字节数（DRF 比的是 Python 的 `len(str)`）：151 个汉字必须超长、
  50 个汉字（150 字节）必须通过 —— 钉在 `register_username_length_counts_characters_not_bytes`。
- **顺带对齐的 DRF 语义（同一处重构的必要结果）**：`register` 的字段级错误改成**逐字段收集**。
  原实现是「遇错即停」，与它自己的注释「等其它字段都收完错误再来查库」自相矛盾，也与 DRF 不符。
  实测蓝本：「超长 `username` + 超长 `email`」两个键都在；「重复用户名 + 短口令」`username` 与
  `password` 都在；`email` 又超长又格式非法时**同一字段两条文案**
  （`["请确保这个字段不能超过 254 个字符。", "请输入合法的邮件地址。"]`）。
  现在 `FieldErrors` 统一「同字段追加、按 `Meta.fields` 声明序渲染」。
  **夹具 27 条不受影响**（原有 register 失败用例全是单键）。
- 证据（单测）：`register_username_over_max_length_is_400`、`register_email_over_max_length_is_400`、
  `register_email_reports_length_and_shape_together`、`register_orchard_address_over_max_length_is_400`、
  `register_collects_all_field_errors_at_once`、`register_accepts_password_longer_than_column_width`、
  `login_has_no_length_limit`、`field_errors_render_in_declaration_order`。

### §3 D13 —— 单测不再耦合录制值

- 所有录制期实例值改成**现读现用**：
  - 期望响应 → `commerce.json` 的 `case.expected_body`（`expected(case)`）；
  - 路径参数 / seed id → `index.json` 的 `seed_refs`（`seed_ref` / `product_id` / `path_arg`）；
  - 删掉 6 个手抄 uuid 常量（`PRODUCT_XF_FAMILY` / `CART_ITEM_B1` / `ADDRESS_DEFAULT` /
    `ORDER_COMPLETED` / `ORDER_PENDING` / `PRODUCT_NEWHALL_ENTERPRISE`）。
- **顺带发现「顺序」也不该钉**：商品 / 购物车条目在蓝本里没有 `order_by`，seed 里 `created_at`
  同秒的记录只能退化成按 uuid 定序，uuid 是 uuid4 —— 重录即变。这与 `replay_diff.py` 的
  `UNORDERED_LEAVES`（`items`）是同一裁定，所以单测改成「先按稳定键排序，再把
  `sales_batch.orchard.category_labels`（D6 已判无序）归一化后**整体比对**」。
  这比逐字段列举覆盖面更大：漏列的字段差异照样会被抓住。
- **没有退化成空断言**：断言包含 `count == items.len()` 的自洽性、与夹具的逐字段整体比对
  （金额字符串形态、`sku_type`、库存、批次 `progress_percent` / `available_quantity` /
  `deposit_ratio`、`archiveCode`、`tree_number` 顺序、事件序列、文案枚举…）、以及结构性质
  （默认地址必须排第一、`sku_type=family` 过滤后每条都必须是 family 且必须真正收窄结果）。
- 顺带把 `orders_list_matches_fixture` 里原本「容忍 `pending_payment` 或 `cancelled`」的写法
  **收紧**成逐字段对齐夹具 —— 原来的容忍正好会掩盖 `_expire_stale_orders` 提前动手（见 §5.1）。
- **验证：重录前后各一次，两次都全绿**

  | 轮次 | 夹具 | 灌注的 seed | 结果 |
  |---|---|---|---|
  | 1 | 原夹具 | 原 `seed.json` | `cargo test -- --test-threads=1 compat` → **124 passed / 0 failed** |
  | 2 | `capture_contract.py` **重录**（`captured_at` `15:57:04Z` → `16:30:20Z`；`seed_refs` 全部 uuid 与 `archiveCode` 都变，例：`CGJ-HEALTH-B1CEF48A44` → `CGJ-HEALTH-8147511B69`） | 新 `seed.json` | 同上 → **124 passed / 0 failed** |

  > 第 2 轮的**第一次**重录验证暴露了 3 条残留耦合（商品 / 购物车的 uuid 定序），已按上面
  > 「顺序也不该钉」修掉；修完再跑才是上表的结论。

- 附：改动前的**基线**是 `compat_close` + 纯 seed 态下 **105 passed / 8 failed**（8 条全是手抄
  uuid / `category_labels` 字面量）。`VERIFICATION.md §6.1` 记的 11 条是另一个 schema 状态下的
  快照，数量差异来自当时库里有非 seed 残留。

### §4 端到端全序列回归（改动后）

| 跑法 | pass | fail | expected_deviation | 结论 |
|---|---|---|---|---|
| 直接跑（录制后 **33 分钟**） | 230 | 9 | 12 | **5 条 commerce 假失败**，见 §5.1 |
| 把 seed 订单的过期窗口推后 2 天后再跑 | **235** | **4** | 12 | 与 `VERIFICATION.md` 基准**一致**，无新增失败 |

- 两个跑法的**唯一**差别是 `ORD-DEMO-1003` 的 `expires_at`（直接 `UPDATE` 数据库，**没有**改夹具）。
- 那 5 条假失败（`orders_list_ok` / `orders_v1_list_ok` / `orders_v1_list_after_state_ok` /
  `products_list_after_state_ok` / `order_cancel_pending_ok`）的差异方向全是
  「期望 `pending_payment` / 库存 300，实际 `cancelled` / 库存 301」，
  即 `_expire_stale_orders` 在回放时把 seed 订单提前取消了。
  **与 D3/D11/D13 的改动无关**（改动没碰订单/库存逻辑），推后窗口即全绿，实锤。
- 残留 4 条失败仍然是 **D12** 记录的那 4 条：`core/tasks_list_ok`、`agent/agent_context_ok`、
  `agent/agent_feedback_get_buyer_ok`、`agent/agent_risk_alert_ok`。

### §5 本轮新发现（交裁定）

#### §5.1 ⚠️ 回放有「录制后 20 分钟」的时限（`VERIFICATION.md §7` 只说了「跨天」）

`seed_demo_data` 给 `ORD-DEMO-1003` 的 `expires_at` 是 `now + timedelta(minutes=20)`，
即「**录制时刻 + 20 分钟**」。超时之后，任何订单端点触发的 `_expire_stale_orders` 都会把它
取消**并回滚库存**（`红肉脐橙家庭装` 300 → 301），造成 §4 里那 5 条假失败。

- **单测侧已解决**：`commerce_tests::pool()` 在拿连接池时统一把这条订单的窗口推到 30 天后
  （`keep_seed_pending_order_open`，只动这一个 `expires_at`，不碰任何领域值）。
  否则用例的成败会取决于「哪个订单用例先跑」—— 实测踩过：`cancelled_at` 早于推后时刻，
  即更早的用例已经动过手。
- **回放侧未解决**：`replay_diff.py` 与夹具都不许动，所以那边仍有这个时限。
  **请裁定**：是给夹具/seed 把窗口放宽到「天」级（要重录），还是把回放前的窗口推后写进标准跑法，
  还是把这段状态从比对里排除。

#### §5.2 `register` 的 `email: ""` 仍是既有偏差（**未改**）

蓝本 `EmailField(allow_blank=True)` 放行空串（实测 `email: ""` 时 `is_valid=True`），
我方报 `请输入合法的邮件地址。`。属 D11 之外的行为，**本轮刻意没动**（避免顺手扩大爆炸半径），
但它是真实差异，建议登记为一条新偏差。

#### §5.3 `trace_tests` 不清理自己的临时数据（**未改**）

`src/compat/tests/trace_tests.rs:446` 的 `product_order_is_deterministic_when_created_at_ties`
插 2 条 `W1B 商品 N` 后直接结束。libtest 按名字序跑时 `commerce_tests` 在 `trace_tests` 之前，
所以**全量跑看不出来**；但**单跑子集或重跑**会把 `products_list` 的条数顶到 9，
让 commerce 单测假红一次。排查这点花了一轮 —— 记下来免得后人重踩。不属于本轮文件范围。

## 安全问题（已记录，未处理）

| ID | 项 | 说明 |
|---|---|---|
| **S1** | `navel_backend_git/api/agent_service.py:17` 硬编码 DeepSeek API Key | 明文密钥进版本库。Rust 侧必须走 `config.toml` / 环境变量，**不得**照抄 |
| **S2** | `requirements.txt` 缺 `requests` | `agent_service.py` 依赖它，Django 环境实际缺依赖。已在本地 venv 补装，未改仓库文件 |
| **S3** | `navel_backend_git` 仓库把 `__pycache__/*.pyc`（43 个）与 `media/`（24 个）提交进了 git，且无 `.gitignore` | 清理时**必须** `git checkout -- .` 还原，否则会删掉被跟踪文件 |
| **S4** | 蓝本对 `register` / `login` / agent 系接口**不挂**鉴权 | 已按契约保留为匿名可调（否则 App 行为不一致）。但 v1 注册完全裸奔，建议切换后单独收口 |

## 已裁定：W2 网页端范围（用户 2026-09-20 决策）

侦察结论：网页端 35 条存活调用中，**能映射到 Django 契约的 0 条**，Rust 独有 31 条，
契约不兼容 4 条。详见 `db/static/WEB_ENDPOINT_MAP.md`。据此裁定：

| 决定 | 内容 |
|---|---|
| **网页端 Rust 独有能力保留为超集，挂 `/web/*`** | 注册审批 + 邀请码、系统设置、客服会话、3D 沙盘几何数据、商品封面上传、后台仪表盘统计——都不塞进逐字兼容 App 的 `/compat` 层，避免污染兼容性目标 |
| **现货商城收敛到 Django 的 `citrus_product`** | 接受前端改造代价：整数 id → UUID、`price_cents` 分 → Decimal `price`、`stock_quantity` → `stock`、`is_active` → `status` 枚举、商品必挂 `sales_batch`、购物车从 localStorage 迁到服务端 `cart_item`。旧 `store_*` 表据此退役 |

由此 W2 拆成两条并行线：**A「可映射部分直接切」** + **B「超集部分保留 `/web/*`」**。

## 待确认（需 App 侧或用户确认）

1. **D2 的接口语义**：`/api/generate/fertilization-plan` 在蓝本里恒 500，没有可参照的正确响应。我方按意图实现后，需要确认 App 是否真的没在用、或期望什么形状。
2. **切换窗口**：W2 切换期间 Django 是否继续为 App 提供线上服务（计划假设「是」）。

## 使用方式

- W1 各域实现时：**夹具 > 本清单 > 个人判断**。夹具与蓝本冲突时以夹具为准，并回来登记。
- `replay_diff.py` 会把命中 D1/D2 的用例标为 `expected_deviation`，不计入失败。
