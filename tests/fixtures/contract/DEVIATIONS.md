# 契约偏差登记（Django → Rust）

本文件是**唯一的偏差权威清单**。四个域的实现在遇到「蓝本行为与设计意图不一致」时，
按这里的裁定执行；新增偏差必须登记在此，不允许悄悄偏离。

裁定时间：2026-09-20。基准来源：`db/tests/fixtures/contract/`（251 条 golden 用例）。

## 已裁定

| ID | 项 | 蓝本实际行为 | 我方决定 | 理由 |
|---|---|---|---|---|
| **D1** | `api/agent_views.py` 漏 `import status` | `/api/agent/select`、`inquiry`、`chat`、`feedback`、`approvals`、`approvals/<id>/decision` 的**全部校验分支恒返回 500** `Internal server error`（DRF 异常体，无 timestamp） | **修正为设计意图的 400/404** | 用户裁定。App 不可能依赖 500 崩溃；复刻等于把 bug 搬进新后端 |
| **D2** | `api/views.py::fertilization_plan_api` 引用未导入的 `FertilizationPlanRequestSerializer` | `POST /api/generate/fertilization-plan` **恒 500**（message 是 Python 异常文本，且带 timestamp） | **按序列化器意图实现正常语义** | 该接口在蓝本里从未可用，App 不可能在用它。**需 App 侧确认**，见「待确认」 |
| **D3** | `complete_task_api` 写 `completed_at` 用 `datetime.now()` | 序列化出**无时区偏移**的本地时间串（`2026-09-20T22:03:05.671819`） | **逐字复刻**，用 `ser::dt_naive_local` | 属于 App 已适配的实际契约形状，改动风险大于收益 |
| **D4** | 时间后缀两种形态并存 | `Z` 718 处（DRF `JSONEncoder`）、`+00:00` 152 处（DRF `DateTimeField`） | **按字段逐个对齐夹具实测值**；比对器把两者归一化后比较，并单独计数漂移 | 两者语义等价，App 侧 `new Date()` 解析结果相同；但要在报告里可见 |
| **D5** | 重复 `tree_number` 建树 | DB 唯一约束 `IntegrityError` 冒泡 → **500** | **返回 400** | 输入校验错误应当是 4xx |
| **D6** | `category_labels` 顺序不可复现 | `commerce_serializers.py` 的 `.distinct()` 无 `order_by`，两次运行顺序不同 | **Rust 侧定序输出**；比对时按**集合**比较 | 蓝本自身不确定，无法逐字节对齐 |
| **D7** | 蓝本 `choices` 不生成 SQL `CHECK` | Django 只建裸 `varchar`，Python 层校验 | **我方可保留 CHECK**（更严格） | CHECK 不进 HTTP 契约；夹具取值均来自同一批 `TextChoices`，不会误伤 |
| **D8** | 蓝本不生成 SQL `DEFAULT` | 默认值由 Python 层兜底 | **我方可保留 DEFAULT** | 不进 HTTP 契约；且 W1 漏列时行为与 Django 的 Python 默认一致 |
| **D9** | 蓝本外键不带 `ON DELETE` | 级联在 Python 层（`on_delete`） | **我方按 `on_delete` 写 CASCADE/SET NULL/RESTRICT，并加 `DEFERRABLE INITIALLY DEFERRED`** | 保证数据一致性；`DEFERRABLE` 让夹具乱序导入在单事务内也成立 |

## 安全问题（已记录，未处理）

| ID | 项 | 说明 |
|---|---|---|
| **S1** | `navel_backend_git/api/agent_service.py:17` 硬编码 DeepSeek API Key | 明文密钥进版本库。Rust 侧必须走 `config.toml` / 环境变量，**不得**照抄 |
| **S2** | `requirements.txt` 缺 `requests` | `agent_service.py` 依赖它，Django 环境实际缺依赖。已在本地 venv 补装，未改仓库文件 |
| **S3** | `navel_backend_git` 仓库把 `__pycache__/*.pyc`（43 个）与 `media/`（24 个）提交进了 git，且无 `.gitignore` | 清理时**必须** `git checkout -- .` 还原，否则会删掉被跟踪文件 |

## 待确认（需 App 侧或用户确认）

1. **D2 的接口语义**：`/api/generate/fertilization-plan` 在蓝本里恒 500，没有可参照的正确响应。我方按意图实现后，需要确认 App 是否真的没在用、或期望什么形状。
2. **切换窗口**：W2 切换期间 Django 是否继续为 App 提供线上服务（计划假设「是」）。

## 使用方式

- W1 各域实现时：**夹具 > 本清单 > 个人判断**。夹具与蓝本冲突时以夹具为准，并回来登记。
- `replay_diff.py` 会把命中 D1/D2 的用例标为 `expected_deviation`，不计入失败。
