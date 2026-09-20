# 契约偏差登记（Django → Rust）

本文件是**唯一的偏差权威清单**。四个域的实现在遇到「蓝本行为与设计意图不一致」时，
按这里的裁定执行；新增偏差必须登记在此，不允许悄悄偏离。

裁定时间：2026-09-20。基准来源：`db/tests/fixtures/contract/`（251 条 golden 用例）。

## 已裁定

| ID | 项 | 蓝本实际行为 | 我方决定 | 理由 |
|---|---|---|---|---|
| **D1** | `api/agent_views.py` 漏 `import status` | `/api/agent/select`、`inquiry`、`chat`、`feedback`、`approvals`、`approvals/<id>/decision` 的**全部校验分支恒返回 500** `Internal server error`（DRF 异常体，无 timestamp） | **修正为设计意图的 400/404** | 用户裁定。App 不可能依赖 500 崩溃；复刻等于把 bug 搬进新后端 |
| **D2** | `api/views.py::fertilization_plan_api` 引用未导入的 `FertilizationPlanRequestSerializer` | `POST /api/generate/fertilization-plan` **恒 500**（message 是 Python 异常文本，且带 timestamp） | **按序列化器意图实现正常语义** | 该接口在蓝本里从未可用，App 不可能在用它。**需 App 侧确认**，见「待确认」 |
| **D3** | `complete_task_api` 写 `completed_at` 用 `datetime.now()` | ~~序列化出无时区偏移的本地时间串~~ **【已更正】** 实测走 DRF 序列化，输出的是 `…294565Z`（带 `Z`、无偏移串） | **改用 `ser::dt_z`**（原按"无偏移"实现，是误判） | 前一版结论基于误读；该字段被 `normalize` 屏蔽所以没体现为失败，但形态差异对 App 侧 `new Date()` 解析是真实的。**待修** |
| **D4** | 时间后缀两种形态并存 | `Z` 718 处（DRF `JSONEncoder`）、`+00:00` 152 处（DRF `DateTimeField`） | **按字段逐个对齐夹具实测值**；比对器把两者归一化后比较，并单独计数漂移 | 两者语义等价，App 侧 `new Date()` 解析结果相同；但要在报告里可见 |
| **D5** | 重复 `tree_number` 建树 | DB 唯一约束 `IntegrityError` 冒泡 → **500** | **返回 400** | 输入校验错误应当是 4xx |
| **D6** | `category_labels` 顺序不可复现 | `commerce_serializers.py` 的 `.distinct()` 无 `order_by`，两次运行顺序不同 | **Rust 侧定序输出**；比对时按**集合**比较 | 蓝本自身不确定，无法逐字节对齐 |
| **D7** | 蓝本 `choices` 不生成 SQL `CHECK` | Django 只建裸 `varchar`，Python 层校验 | **我方可保留 CHECK**（更严格） | CHECK 不进 HTTP 契约；夹具取值均来自同一批 `TextChoices`，不会误伤 |
| **D8** | 蓝本不生成 SQL `DEFAULT` | 默认值由 Python 层兜底 | **我方可保留 DEFAULT** | 不进 HTTP 契约；且 W1 漏列时行为与 Django 的 Python 默认一致 |
| **D9** | 蓝本外键不带 `ON DELETE` | 级联在 Python 层（`on_delete`） | **我方按 `on_delete` 写 CASCADE/SET NULL/RESTRICT，并加 `DEFERRABLE INITIALLY DEFERRED`** | 保证数据一致性；`DEFERRABLE` 让夹具乱序导入在单事务内也成立 |
| **D10** | `login` 凭据错误分支的信封形状 | 蓝本源码（`serializers.ValidationError`）看似该走 DRF 异常体（**无** timestamp），但实测录到的是 `{code:401, message:{"non_field_errors":["Invalid credentials"]}, data:null, timestamp:…}`——**成功体形状、message 是对象、带 timestamp** | **取夹具** | 这是 App 实际收到的字节。`views_auth.rs` 已用 ⚠️ 注释标出该反直觉点 |
| **D11** | 字符串长度校验 | 蓝本靠 DB 约束（`VARCHAR(150)` 等），超长会变成 500 | **待补：在契约层加 `max_length` 校验返回 400** | 目前 `username > 150` / `password > 128` / `email` 超长会撞列宽变 500，与蓝本行为也不一致。属已知缺口 |
| **D12** | 夹具把「平局时的数据库返回序」当成了契约 | 蓝本 `Task` / `AgentFeedback` 的 `Meta.ordering` 只有 `-created_at` 没有次级键；测试库里多条记录 `created_at` **完全相同**，SQLite 按 rowid 返回，PG 按物理顺序返回，两者不同 | **接受为夹具不确定，实现侧不迁就**（不为匹配某个物理顺序而写凑数代码） | 残留 4 条失败全部源于此：`core/tasks_list_ok`、`agent/agent_context_ok`、`agent/agent_feedback_get_buyer_ok`、`agent/agent_risk_alert_ok`。**建议夹具给这些列表补稳定次级键**，或让 seed 不再产生平局。这是夹具自身的不确定性，不是实现缺陷 |
| **D13** | 单测写死了录制实例的字面量 | — | **待修：commerce 单测改为从夹具派生，不手写条数/uuid/数组字面量** | 重录夹具后 `commerce_tests.rs` 11 条失败（`:206` `10 != 7`、`:565` `3 != 2`、商品 uuid 等），**不是实现回归**——端到端全序列 commerce 81/81 全绿。手写字面量会持续污染判断力 |

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
