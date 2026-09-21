# Wave 0' 交割说明：契约基准的两套产物与硬伤核验

> **【裁定已生效 2026-09-20】** 主线实测两套产物（用例数 150 vs 251、`normalize` 为空 121/150 vs 0/251、
> `index.json` 缺 6 项必需字段 vs 全含），**采信 `.tmp/contract/` 版本**，已把它的
> `capture_contract.py` 与全部产物落位到 `db/scripts/` 与 `db/tests/fixtures/contract/`，
> 并删除非交付物 `probes.json`。下文 §0 的「请二选一」已由该裁定解决，保留原文作为核验证据。
> 复核方式：`python db\scripts\audit_fixtures.py db\tests\fixtures\contract <另一套夹具目录>`。

> 本文件是**核验与交割记录**，不是任务书要求的产物本身。任务书要求的 7 个文件见下。

## 0. 决策点（需上层裁定，未能在本 agent 内解决）

`db/scripts/capture_contract.py` 与 `db/tests/fixtures/contract/*` **同时被另一个 agent 拥有**：
本 agent 的写入尝试被 "file changed since it was read" 连续拒绝（对方在 21:39→21:46 之间
反复重写该文件）。因此本 agent **没有覆盖**对方产物，而是把**完整正确版脚本 + 全量重录结果**
落到「换输出目录」路径 `D:\githubs\db_work\.tmp\contract\`，并把问题写成书面核验（见 §2）。

**请二选一**：
- **A（推荐）**：把 `db/scripts/capture_contract.py` 换成 `.tmp/contract/capture_contract.py`，
  并重跑一次全量（命令见 §5），覆盖 `db/tests/fixtures/contract/` 下 6 个 JSON；
  同时**删除** `db/tests/fixtures/contract/probes.json`（非交付物）。
- **B**：保留对方版本，由对方按 §2 的硬伤清单自行修复（本文件即修复清单）。

## 1. 两套产物对照（均为实测）

| 项 | `db/tests/fixtures/contract/`（对方，21:46:41） | `.tmp/contract/`（本 agent，实测通过） |
|---|---|---|
| 79 条 `path()` 自省 | ✅ 一致 | ✅ 一致 |
| 域划分 auth/core/orchard_trace/commerce/agent | ✅ 9/14/21/24/11 | ✅ 9/14/21/24/11 |
| 用例总数 | 150 显式 + 10 自动探针 = 160 | **246**（全显式） |
| covered / partial / blocked | 未统计（index 无该字段） | **79 / 0 / 0** |
| `path×method` 覆盖 | 靠 10 条**无语义探针**补齐 | 每条均由**显式用例**覆盖（`--smoke` 强制核对） |
| `normalize` 有效的 200 用例 | **121/150 为空** | **0 为空** |
| 夹具内未屏蔽的运行期 UUID | **248 处** | **0 处**（实测扫描 `expected_body`） |
| `index.json` 统计字段 | 缺 covered/partial/blocked/uncovered 计数/mutation_order/alias_conclusion | 全含（含 52 条 mutation_order、逐 path status_histogram、别名结论明细） |
| 别名自检 | 仅 `REPORT.md` 表格，未落 JSON | index.alias_conclusion：checked 18 / equal 16 / mismatch 2（2 条为预期的不同用户/不同请求载荷） |
| shape_only | 11 条（含把蓝本 500 也标 shape_only 的做法） | 7 条，**恰好覆盖任务书列举的 6 个**（另加 quality-samples 写入） |
| 交付物清单 | 多出 `probes.json` | 严格 7 个文件，无多余 |
| `--domain` | 单值（不可重复） | `action="append"`，可重复 |
| `seed.json` | 末态快照（含全部写类副作用） | **纯 seed 态**（在跑任何用例前导出），Rust 从它加载后按序回放 |

## 2. 对方版本的三处硬伤（实测证据）

### 硬伤一：`normalize` 为空 → 夹具无法逐字节比对
`auth.json::me_farmer_ok` 实测 `normalize = []`，而 `expected_body.data.id` 是
`uuid.uuid4()`、`data.created_at` 是写入时刻、`timestamp` 是毫秒时间戳。
扫描全部域：**121/150 用例 `normalize` 为空，248 处运行期 UUID/token 未屏蔽**。
另有既有 normalize 写法与实际键名不符：`$.data.orderNumber` 而响应里是
`order_number`（DRF 用模型字段名）。

### 硬伤二：`index.json` 缺关键统计
实测顶层 keys 仅：
`captured_at, seed_signature, path_total, method_total, domain_path_counts, seed_refs,
captured_values, uncovered, routes, global_normalize, shape_only_cases, server_errors,
transport_errors`。
任务书要求的 `covered` / `partial` / `blocked` / `uncovered`(计数) / `mutation_order` /
`alias_conclusion` / `unresolved` **全部缺失**；`routes[]` 用 `url_name` 而非 `name`，
且无逐 path 的 covered/partial/blocked。

### 硬伤三：交付物与 CLI
- 多出非交付物 `db/tests/fixtures/contract/probes.json`（10 条无语义探针）。
- `--domain` 为 `choices=DOMAINS` 单值，不能重复传入。
- `--smoke` 只做环境自检，不做「每条 path×method 至少 1 条用例」的矩阵核对，
  所以「覆盖 100%」是靠事后探针得到的数字，而不是用例设计保证。

## 3. 本 agent 版本的关键修复（都在 `.tmp/contract/capture_contract.py`）

1. **normalize 真正生效**：录制时在 `expected_body` 的**副本**上就地替换为
   `<sha256:16hex>`，并把命中字段名写入 `normalize_hits`。
   核心 bug 修复：`$..id` 曾被解析成 rollup 键 `".id"`（带点），导致**所有 rollup 型屏蔽静默失效**。
2. **屏蔽名册**（含 DRF `PrimaryKeyRelatedField` 的裸主键）：
   `timestamp / created_at / createdAt / updated_at / updatedAt / id / token / expiresAt /
   expires_at / tree / harvest_archive / product / order / package / ref_id`
   + 订单号 `orderNumber|order_number`、支付 `providerTransactionId`、哈希链
   `previous_hash|evidence_hash|hash_short`、`recorded_at|recordedAt`、`generated_at`、
   `checkedAt`、`verifiedAt`、`planId`、上传 `path|url`、`code|trace_code|traceCode`、
   `harvest_code`、写类后的 `stock|sold_quantity`。
3. **别名自检可执行**：显式 `ALIAS_PRIMARY` 映射（同一主路径常有多用例，自动配对会拿
   不同用户/不同状态比出假不一致），并且比较用「**屏蔽后结构等价**」而不是直接 `==`
   （占位符按值哈希，两次请求必然不同）。
4. **写类时序按真实状态机排**：`cart_api` POST 的同批次约束实测语义是
   「车中**除目标商品外**其他商品的批次集合必须为空或恰好等于目标批次」，
   故写类序列改为 `DELETE b3 → DELETE b1（v1 别名） → 空车校验 → POST 加购 → PATCH →
   v1 POST → DELETE → 下单 → 支付 → 取消(400) → 空车下单(400) → v1 加购 → v1 下单 →
   v1 支付 → v1 取消(400)`，全部 200/400 与设计一致。
5. **矩阵断言**：`--smoke` 即核对每条 `path×method` 都有显式用例；不产出 `probes.json`。
6. **`seed.json` 前置导出**（纯 seed 态），使 Rust 侧「加载 seed → 按 mutation_order 回放」
   每步状态自然与录制时一致。

## 4. 实测结果（`.tmp/contract/`）

```
[matrix] 用例总数 = 238 → 补 4 个缺失组合后 246
[matrix] 全部 path×method 均有用例 ✓
=== 汇总 ===
path=79 covered=79 partial=0 blocked=0 cases=246
alias: checked=18 equal=16 mismatch=2
5xx 用例 12 条（其中 10 条是蓝本 bug 的真实行为，见 §6）
未屏蔽 UUID 残留（expected_body 扫描）：0
```

逐域：auth 27 / core 42 / commerce 81 / orchard_trace 65 / agent 36。
`index.json.mutation_order`：52 条（写类顺序与依赖已记录 note）。

## 5. 重跑命令（把正确版落位后）

```powershell
$env:PYTHONIOENCODING="utf-8"
$py = "D:\githubs\db_work\.venv-django\Scripts\python.exe"

# 1) 自检：环境 + 路由枚举 + 用例矩阵核对（不写文件）
& $py db\scripts\capture_contract.py --smoke

# 2) 全量录制（写出 7 个文件到 db/tests/fixtures/contract/）
& $py db\scripts\capture_contract.py

# 3) 只重录某些域（可重复）
& $py db\scripts\capture_contract.py --domain commerce --domain agent

# 4) 输出到别处
& $py db\scripts\capture_contract.py --out D:\githubs\db_work\.tmp\contract
```

复核仓库零污染：

```powershell
git -C D:\githubs\db_work\navel_backend_git status --short   # 只应见已存在的 __pycache__
git -C D:\githubs\db_work\navel_backend_git hash-object db.sqlite3
#   -> cae569ef8a62ba1c8811deb6d5ea9aef0def3580（实测前后一致）
git -C D:\githubs\db_work\db status --short
```

## 6. 必须让 Rust 侧知道的蓝本缺陷（已写入 REPORT.md §5.3）

1. `api/agent_views.py` 顶层**未** `from rest_framework import status`，却在 9 处使用
   `status.HTTP_4xx_*` → 以下「应为 400/404」的分支**实际返回 500 `Internal server error`**
   （DRF 异常体，**无 timestamp**）：
   `/api/agent/select`、`/api/agent/inquiry`、`/api/agent/chat`、`/api/agent/feedback`、
   `/api/agent/approvals`、`/api/agent/approvals/<id>/decision` 的全部校验分支。
2. `api/views.py::fertilization_plan_api` 引用未导入的 `FertilizationPlanRequestSerializer`
   → `POST /api/generate/fertilization-plan` **恒 500**，message 为
   `name 'FertilizationPlanRequestSerializer' is not defined`，且**带 timestamp**（成功体形状）。
3. 新增发现：`POST /api/v1/farmer/orchards/<uuid>/trees` 用同果园重复 `tree_number`
   → DB 层唯一约束 `IntegrityError` 冒泡 → **500**（非 400）。

## 7. 时间与回放约束（Rust 侧必读）

- `captured_at`（本轮 `.tmp/contract`）= `2026-09-20T14:05:2x+00:00`（UTC）。
- 两种时间形态都要复刻：
  - DRF 常规：`2026-09-20T14:03:03.230368Z` / `...399657+00:00`（带 6 位微秒）
  - 朴素时间例外：`complete_task_api` 用 `datetime.now()` 写 `completed_at` →
    `2026-09-20T22:03:05.671819Z`（**本地时区、无 +00:00**）
- `TraceEvent.evidence_hash` 的 sha256 输入含 `occurred_at.isoformat()` 微秒 →
  **必须在 captured_at 当天回放**；跨天会漂移 `is_open`、`_expire_stale_orders`、
  日报 `today_order_count`、复购 `days_since`、`fulfillment_risk_score` 分支。
