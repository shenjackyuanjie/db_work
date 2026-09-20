# 契约层验证收口报告（W1 全序列权威验证）

- 验证时间（UTC）：`2026-09-20T15:57` 起
- 分支：`main`（四域契约实现已合并；`cargo check --all-targets` 通过）
- 权威结果：**251 条用例 → 235 pass / 4 fail / 12 expected_deviation / 0 transport_error**
- 逐域：auth 27/27 · core 39/42（2 expected_deviation）· commerce **81/81** · orchard_trace **64/65**（1 expected_deviation）· agent 24/36（9 expected_deviation）

本报告只写**实测**结论，每一项都带复现命令。发现但**没有**修的实现问题全部列在 §5，交主线裁定。

---

## 1. 权威跑法与完整复现命令

**单域回放测不准**：夹具的读类期望值里含**前置域 mutation 的效果**（commerce 的写类副作用
体现在 orchard_trace 的读类里，agent 域依赖前面各域），所以「单域 + fresh seed」必然有假失败。
权威跑法是**在同一个 schema 上、按录制器的域顺序（`auth → core → commerce → orchard_trace
→ agent`）、加载一次纯种子态、跑完全部 251 条**。`replay_diff.py` **不给 `--domain` 时就是全序列**。

```powershell
# 0) 环境：sccache 在本机起不来，跑 cargo 前必须清掉 wrapper
$env:CARGO_BUILD_RUSTC_WRAPPER=''
$env:PYTHONIOENCODING='utf-8'
cd D:\githubs\db_work\db

# 1) 编译门槛
cargo check --all-targets

# 2) Rust 单测（**必须 --test-threads=1**，见 §7）
$env:COMPAT_TEST_SCHEMA='compat_verify'
cargo test -- --test-threads=1 compat

# 3) 建 scratch schema（只在 compat_* 里操作，绝不碰现网 public）
scripts\pg_env.ps1 -Reset compat_verify
scripts\pg_env.ps1 -Apply compat_verify          # DDL 从 src/server/bootstrap/*.rs 现场抽取

# 4) 起影子服务
scripts\pg_env.ps1 -Serve compat_verify -Build -Port 11500

# 5) 载入纯种子态（seed 已保住微秒，见 §3）
& D:\githubs\db_work\.venv-django\Scripts\python.exe scripts\load_seed.py --schema compat_verify

# 6) 权威全序列回放（不带 --domain）
& D:\githubs\db_work\.venv-django\Scripts\python.exe scripts\replay_diff.py `
    --base-url http://127.0.0.1:11500/compat --schema compat_verify `
    --out db\tests\fixtures\contract\REPLAY_final.json

# 7) 收工
scripts\pg_env.ps1 -Serve compat_verify -Stop
```

**重录夹具**（改了 `capture_contract.py` 或要换一批录制值时才需要）：

```powershell
$env:TEMP='D:\githubs\db_work\.tmp'
$env:PYTHONIOENCODING='utf-8'
& D:\githubs\db_work\.venv-django\Scripts\python.exe db\scripts\capture_contract.py
# 约 40 秒；会重写 seed.json / seed_final.json / 5 个域 JSON / index.json / REPORT.md
```

> ⚠️ **重录会改变全部录制值**（uuid4、时间戳），这正好用来暴露「实现/测试里写死了录制值」的情况。
> 本轮重录后 `cargo test -- compat` 有 11 条失败，全部是这个原因，见 §6。

**已知的落盘副作用**：`replay_diff.py` 会往库里补两样东西（录制器在跑任何用例前也做了同样的事）：
scratch 账号 2 个（`qa_scratch_user`/`qa_scratch_user2`）与预置审批单（`index.seed_approval_id`）。
它们**不在** `seed.json` 里（seed 是那之前的纯种子态），不补就是恒 401 / 恒 404。

---

## 2. 逐域结果表

| 域 | 用例 | pass | fail | expected_deviation | shape_only | 说明 |
|---|---|---|---|---|---|---|
| auth | 27 | **27** | 0 | 0 | 0 | 全绿 |
| core | 42 | 39 | **1** | 2 | 4 | 唯一 fail = `tasks_list_ok`（§5.1） |
| commerce | 81 | **81** | 0 | 0 | 0 | 全绿（含 6 条 capture 回填） |
| orchard_trace | 65 | 64 | **0** | 1 | 3 | 全绿（含哈希链用例，§3.3） |
| agent | 36 | 24 | **3** | 9 | 0 | `agent_context_ok` / `agent_feedback_get_buyer_ok` / `agent_risk_alert_ok`（§5） |
| **合计** | **251** | **235** | **4** | **12** | **7** | `transport_error=0`、`offset_style_drift=0` |

`expected_deviation` = 蓝本缺陷（`DEVIATIONS.md` D1/D2/D5）的 12 条，已裁定 Rust 侧修正为正确语义，
**不计失败**。

### 2.1 修复前后对比（每项各修掉多少条）

| 阶段 | 改了什么 | pass | fail |
|---|---|---|---|
| **修复前**（旧夹具 + 旧工具） | — | **173** | 66 |
| + T2a 自省反推 capture 映射 | 按值反推（只能消一部分） | 173 | 66 |
| + T2b **落盘 `capture_plan` + 生产者限定** | 回放器按路径重建运行期 id | 189 | 50 |
| + T3 **seed 保住微秒** + 重录 | dumpdata 截断补回 | 195 | 44 |
| + T4 一批 normalize 补充 + D6 集合比较 | 见 §4 | 220→221 | 19→18 |
| + T3/T4 的二轮重录 + 比对器修复 | 见 §4 | 228 | 11 |
| + `batch_code` 屏蔽 + `unordered_at` 去锚定 | 见 §4 | **235** | **4** |

> 中间数字波动的原因已查清：`capture_plan` 一开始用「按值反推」推不出来（录制值恰好就是回放时
> 拿不到的那个 id，构造上必失败），而且 `cart_item_b1`/`cart_item_b1_2` 的录制值**是同一个 uuid**
> （加购走 `get_or_create`，第二次只累加数量），按值反推还会歧义 —— 必须由录制器落盘路径。

---

## 3. T3 诊断：`seed.json` 与夹具的时间精度（三段证据）

### 3.1 结论：**是 dumpdata 截断，不是「值不同」**

原始值本来就是同一批，只是 `dumpdata` 把微秒截成了毫秒。

### 3.2 三段证据

**A) Django 侧：原始 text → datetime → dumpdata**

```
[A2] connection 游标读出的原始 text : timestamp=2026-09-20 15:25:01.855199
[A3] ORM 得到 datetime             : 2026-09-20T15:25:01.855199+00:00  microsecond=855199
[A4] DjangoJSONEncoder 单独作用     : "2026-09-20T15:25:01.855Z"     ← 截断发生在这里
[A6] dumpdata 落盘                  : api.temperaturehumiditydata.timestamp = '2026-09-20T15:25:01.855Z'
```

根因定位到 `.venv-django/Lib/site-packages/django/core/serializers/json.py:86-93`：
`DjangoJSONEncoder.default()` 的 `datetime` 分支显式做 `o.isoformat(...)[:23]`，**只留 3 位小数**。
SQLite 里存的是 6 位小数文本（`DjangoSQLite` 的 `adapt_datetime` = `val.isoformat(" ")`），
ORM 读回来 microsecond 完好 —— **截断只发生在序列化**。

**B) `load_seed.py` 生成的 INSERT SQL 字面量**：写的就是种子文件里的 `.936Z`（只有毫秒），
即**种子文件本身**已经丢了微秒，与 `load_seed.py` 无关。

**C) psql 灌完之后 `SELECT`**：库里就是毫秒精度那一份（`.936`），
而同一批记录的夹具期望是 `.936176` / `.935180` —— 微秒位对不上，**其余数字逐位相同**。

### 3.3 修法：`dump_seed` 后按主键从 connection 读回原始 datetime

`capture_contract.py::restore_datetime_precision()`：dumpdata 之后按模型/主键从
`connection.cursor()` 读原始行，只把**确实被截断**（`raw.isoformat() != dumped`）的
`DateTimeField` 回填成 6 位微秒（naive 值打上 UTC 标记，让 PG `timestamptz` 语义无歧义）。

- 纯种子态回填 **214** 处；末态（`seed_final.json`）回填 **308** 处；`no_row=0`（一条都没漏）。
- 重录后 seed 的 214 个时间字段**全部**是 6 位小数，与夹具逐位一致。
- **没有**改 seed 的语义、**没有** rebase 任何时间，只补回被序列化吃掉的三位。

> ⚠️ 一个陷阱：SQLite 把 UUID 列存成 `char(32)`（**无连字符**），dumpdata 输出的是带连字符的
> 标准形式，直接字符串相等会让 97/100 行落到 `skipped_no_row`。已用 `_pk_key()` 归一化比较键。

### 3.4 哈希链复查（orchard_trace 域）

`TraceEvent.evidence_hash` 的输入含 `occurred_at.isoformat()` 的微秒，所以改 seed 精度**必须**
重跑 orchard_trace 确认哈希链仍逐字相等 —— 结果是 **64/65 通过**（唯一 non-pass 是 1 条
`expected_deviation`）。哈希链用例 `traces_lookup_*`（含 `integrity.chainValid`）全绿。

---

## 4. 本轮工具改动与 normalize 补充清单

### 4.1 `scripts/replay_diff.py`

| # | 改动 | 为什么 |
|---|---|---|
| 1 | **`CaptureMap`**：按 `index.capture_plan` 重建运行期 id（path + body 都替换） | T2。录制器把**回放过程中产生**的 id 烘进了后续用例的 path/body，真值在 `captured_values`。不重建则 `PATCH /api/cart/<录制时的 uuid>` 必然 404 并级联弄脏后续 |
| 2 | **生产者限定**（`capture_generators` + `producers()`） | `$.data.id` 是**很多**响应的 id 路径，无条件按路径取值会把购物车条目的 id 塞给 `task_id`（真实踩过，一错错一片）。必须「谁生成谁」 |
| 3 | 未观察到的 key 记 `warning` + 用例带 `cause="capture_unresolved:<key>"` | 不静默放过 |
| 4 | **`ensure_seed_approval()`**：回放前补上录制器预置的那张待决策审批单 | 它由 `run_capture` 在**任何用例之前**创建，不在 seed 里；agent 域 3 条用例拿它的 uuid 作路径参数，不补恒 404 |
| 5 | `_sql()` 改走 **stdin + `SET client_encoding`** | 审批单标题是中文；psql 在 Windows 控制台用本地代码页解释 `-c` 参数，实测报 `无效的 "UTF8" 编码字节顺序` |
| 6 | **T1 复核**：`parse_expr` 的 `rest.startswith("..")` + `rest[2:]` | 已由主线修好。复核结论：`$..key` 走 rollup、`$.a.b` 走多级 key 路径、`$.a[*].b` 走 iter，三者均正确，且 `--self-check` 的 231 条变异**全部被检出**（0 漏检），未引入新问题 |
| 7 | `--domain` 不给 = **全序列**（按 `DOMAIN_ORDER`） | T5。单域 + fresh seed 必然假失败 |
| 8 | 报告增加 `capture` 段（recorded / seed / runtime / unobserved / notes）与逐域 `capture_substituted`、`fails_without_cause` | 每一条 fail 都要有归因，不允许「未知」 |
| 9 | **D6 集合比较**（`unordered_at` / `pair_elements`） | 见下 |

### 4.2 比对器放宽的唯一一处：无序集合比较（**有据可查，不是换绿**）

`UNORDERED_LEAVES = ("items", "products", "category_labels")` 的数组按**多重集**比较：
元素必须一一对应、数量必须相同，只是不要求同序。判据：

- **`DEVIATIONS.md` D6 已裁定**：`category_labels` 由 `serializers.SerializerMethodField` 里的
  `.distinct()` 生成，**没有 `order_by`** —— 蓝本自身两次运行的顺序就不同，裁定「Rust 侧定序输出；
  比对时按**集合**比较」。这里只是落实该裁定。
- **商品/果园/批次列表**：`commerce_views._product_queryset()`、`orchard_list_api`、
  `supply_batch_list_api` 都**没有** `order_by`；而 `CitrusProduct.Meta.ordering` 的第一个键
  `sort_order` 在夹具里**全部为 0**，顺序退化成「数据库返回序」——录制在 SQLite 上拿 rowid（插入序），
  PG 上拿堆序，本就不保证一致。
- 三个叶子名在本契约里**无歧义**（没有别的同名字段）。

**没有做的事**：没有把任何字段整体跳过、没有把「值不同」当「顺序不同」放过。
`pair_elements()` 内部用 `compare()` 逐字段配对（占位符仍当通配、类型仍必须一致），
配不上的那一对会把**字段级差异**带进报告。

> 修这个时踩了两个自己的坑，记下来免得后人重踩：
> (1) 多重集一开始用 JSON 文本比对 —— 屏蔽后的占位符是**按值算的哈希**，期望侧与实现侧必然不同，
> 会把「两个元素都被屏蔽」误判成「元素不同」；必须用 `compare` 语义配对。
> (2) `pair_elements()` 传的根路径是 `$`，若不显式带 `unordered=True` 递归下去，
> 嵌套在元素里的 `category_labels` 会**丢掉「无序」属性**。

### 4.3 `scripts/capture_contract.py`

| # | 改动 | 为什么 |
|---|---|---|
| 1 | `restore_datetime_precision()`：dumpdata 后按主键回填微秒 | T3，见 §3 |
| 2 | index 落盘 **`capture_plan`**（`key -> 响应 JSON 路径`）与 **`capture_generators`**（`key -> [生成它的用例名]`） | T2。回放器靠它重建运行期 id；按值反推在构造上不可能成功 |
| 3 | `_pk_key()`：UUID 比较键归一化（去连字符） | SQLite 存 `char(32)` vs dumpdata 带连字符 |

### 4.4 normalize 补充（T4，逐条实测后只补真正需要的）

判据：**同一份夹具上重跑时该字段的值每次都变**，且它是**写入时刻**或**随机编号**，
不是种子里可复现的领域值。

| 新增 | 典型位置 | 为什么必须屏蔽（实测） |
|---|---|---|
| `$..paid_at` | `order_pay_ok` / `order_pay_v1_ok`、`payments[*]` | 支付写入时刻，重跑必变 |
| `$..cancelled_at` | `order_cancel_pending_ok` | 取消写入时刻 |
| `$..decided_at` | `agent_approval_decision_*` | 审批决策时刻 |
| `$..completed_at` | `tasks_complete_ok` | 完成时刻（写入时刻，非种子值） |
| `$..payment_number` | `payments[*]` | `PAY<32hex>`，每次支付随机 |
| `$..trace_code` | 箱码 / 新建批次·果树追溯码 | uuid4 派生 |
| `$..harvest_code` | 采摘档案 | `CGJ-HV-<10hex>` |
| `$..order_number` | 订单 | `NO<时间><6hex>` |
| `$..code` | 新建批次/果树 | uuid4 派生（`farmer_batch_create_*`） |
| `$..batch_code` | `agent_risk_alert` 的 `risk_cards[*].batch_risk_scores[*]` | 装的是**运行期新建批次**的随机 code，不是种子值 |
| `$..open_at` / `$..close_at` | `sales_batch` | `seed_demo_data` 用 `now()-30d` / `now()+60d` 生成 —— **录制时刻不同、值就不同**，微秒位对不上 |
| `$..record_time` | 温湿度历史点 | 同上（`now()-n d`） |

**没有成批乱加**：`verified_at` / `last_observed_at` / `harvested_at` / `sampled_at` /
`occurred_at` / `recorded_at` / `expires_at` 这些**没加** —— 它们的值来自 seed，
修好 T3 的精度后逐位相符，属于**该逐字比对**的领域值。

---

## 5. 残留失败逐条归因（4 条，每条都有实证）

### 5.1 `core/tasks_list_ok` —— 夹具缺陷（期望的是插入序，不是任何确定性规则）

| | |
|---|---|
| 现象 | `$.data[0..1]` 两条互换：期望 `[B区坡地黄龙病复核, 采摘期前糖度抽检, 果园日常巡园]`，实际 `[采摘期前糖度抽检, B区坡地黄龙病复核, 果园日常巡园]`。**集合相同** |
| 实证 | seed 里这 3 条任务 `created_at` **完全相同**（`2026-09-20T15:57:07.985960`）。蓝本 `Task.Meta.ordering = ['-created_at']` **没有第二排序键**，`created_at` 平局时 Django/SQLite 返回的是 **rowid 序（= 插入序）**；Rust 侧补了 `id::text` 做确定性 tie-break，于是顺序与「插入序」不一致 |
| 关键观察 | 真实库 `navel_backend_git/db.sqlite3` 里这 3 条任务的 `created_at` 是**逐条递增的**（`.731084 / .741941 / .750370`，插入序 = 金标序），而在测试库里被压成了同一个微秒 —— 平局是**录制环境的产物**，不是领域语义 |
| 归属 | **夹具缺陷**：`seed.json` 无法表达 rowid，夹具却把 rowid 序当成了契约。修法二选一 —— (a) 让 `seed_demo_data` 的这 3 条任务时间不再平局后重录；(b) 承认它无序，把 `$.data` 加进 `UNORDERED_LEAVES` |
| 附带 | 本条与 §5.2 同源（都是「多行共享同一微秒」） |

### 5.2 `agent/agent_context_ok` —— 同 5.1（夹具缺陷），只是换了个读类入口

`$.data.tasks[2..3]` 互换，内容与 `tasks_list_ok` 同一批任务、同一个根因。

### 5.3 `agent/agent_feedback_get_buyer_ok` —— 夹具缺陷（两条反馈 `created_at` 完全相同）

| | |
|---|---|
| 现象 | `$.data[0]` 与 `$.data[1]` 整体互换（`rating` 5↔4） |
| 实证 | seed 里两条 `api.agentfeedback` 的 `created_at` **完全相同**（`2026-09-20T15:57:08.008573`）。`AgentFeedback.Meta.ordering = ['-created_at']` 无第二键，平局同样是「数据库返回序」 |
| 归属 | **夹具缺陷**，与 §5.1 同一类。修法同 5.1 |

### 5.4 `agent/agent_risk_alert_ok` —— 同 5.1/5.3（夹具缺陷）

`$.data.risk_cards[*].source.evidence` 里那两条任务（`B区坡地黄龙病复核` / `采摘期前糖度抽检`）
互换位置；根因与 §5.1 完全相同。

> 四条残留失败**全部**是同一类：**夹具把「平局时的数据库返回序」当成了契约**。
> 已经确认过 `batch_code`（原先在这条用例里也对不上）是 run-time 随机值，已加 normalize 屏蔽；
> 剩下这 4 条不是比对器问题、不是 implementation 问题。

---

## 6. 需要主线裁定的点

### 6.1 `src/compat/tests/commerce_tests.rs` 里有**写死的录制值** —— 重录后 11 条单测失败

本轮按任务书重录夹具（改了 `dump_seed` 精度与 normalize），**全部录制值都变了**（uuid4、时间戳）。
结果 `cargo test -- --test-threads=1 compat` 从「预期全绿」变成 **102 passed / 11 failed**，
11 条全部在 `commerce_tests.rs`：

| 失败位置 | 断言 | 直接原因 |
|---|---|---|
| `commerce_tests.rs:206` `product_list_matches_fixture_order_and_amounts` | `10 != 7` | 写死了商品条数 |
| `commerce_tests.rs:247` `product_list_search_and_sku_filter` | `10 != 7` | 同上 |
| `commerce_tests.rs:274` `category_labels_follow_created_at_descending` | `Null != ["企业装","试吃装","家庭装"]` | 写死了 `category_labels` 的字面量顺序 |
| `commerce_tests.rs:323` `health_archive_aggregates_like_blueprint` | `404 != 200` | 写死了商品 uuid |
| `commerce_tests.rs:462` `cart_snapshot_matches_fixture` | `0 != 2` | 写死了购物车条目数/商品 uuid |
| `commerce_tests.rs:565` `address_list_prefers_default_first` | `3 != 2` | 写死了地址条数 |
| `commerce_tests.rs:684` `orders_list_matches_fixture` | `5 != 3` | 写死了订单条数 |
| `commerce_tests.rs:723` `order_detail_and_404` | `Null != "39.90"` | 写死了订单 uuid |
| `commerce_tests.rs:753` `order_pay_and_cancel_reject_completed_order` | `404 != 400` | 同上 |
| `commerce_tests.rs:822` `expire_stale_orders_cancels_and_restores_stock` | panic in `expect` | 同上 |
| `commerce_tests.rs:884` `after_sales_list_matches_fixture` | `3 != 1` | 写死了售后单条数 |

**这 11 条不是实现回归**（`replay_diff.py` 的端到端全序列 **commerce 81/81 全绿**），
而是单测直接断言了**录制实例的值**。请主线裁定：

- **建议**：让 `commerce_tests.rs` 与 `replay_diff.py` 同源 —— 从 `tests/fixtures/contract/*.json`
  + `load_seed.py` 加载，不要写死条数/uuid/数组字面量；
- 或：把 `commerce_tests.rs` 里这些断言改成「按夹具派生」，重录时不再需要人手同步。

> 同一现象也是「实现里有没有偷偷写死录制值」的探针：**`src/compat/views_*.rs` 没有受影响**
> （端到端全序列 235/251、commerce 与 orchard_trace 全绿），写死的只有测试模块。

### 6.2 `complete_task` 的 `completed_at` 序列化形态与蓝本不符（未修，被 normalize 盖住）

- 蓝本：`complete_task_api` 用朴素 `datetime.now()` 写 `completed_at`，DRF 序列化出
  **`2026-09-20T23:36:09.294565Z`**（微秒 + `Z`）—— 与 `created_at` 同形。
- 现状：`src/compat/views_core.rs` 用 `ser::dt_naive_local`，输出
  **`2026-09-20T23:43:11.173038`**（无偏移、无 `Z`）。
- 本轮我已把 `$..completed_at` 加进 normalize（它是**写入时刻**，本就逐值不可比），
  所以这条**不体现为失败**；但形态差异是真实的，`W1A_DELIVERY.md` §3.2 把它记为「D3 逐字复刻」，
  而实测蓝本并不产生朴素形态 —— **请主线确认 D3 的裁定是否要改**。
- 影响面：App 侧若对 `completed_at` 做 `new Date()` 解析，`Z` 与无偏移串在部分浏览器下不等价。

### 6.3 `UNORDERED_LEAVES` 是否要补 `$.data`（任务/反馈列表）

若主线接受 §5 的「夹具把插入序当契约」判断，`$.data` 需要加进 `UNORDERED_LEAVES`，
那 4 条残留失败会归零。**我没有自己加** —— 那等于把 D6 之外的「顺序不参与比较」也放宽掉，
属于裁定范围，不在工具职责内。

### 6.4 `DEVIATIONS.md` D11（字符串长度校验）仍是缺口

`username>150` / `password>128` / `email` 超长会撞列宽变 500，与蓝本行为也不一致。本轮未涉及。

---

## 7. 环境注意

- **`cargo test -- compat` 必须 `--test-threads=1`**：`support::pool()` 每个用例都会跑一次
  `bootstrap::init_database`，里面的 `CREATE INDEX IF NOT EXISTS idx_app_sessions_expires_at`
  在 PG 上并发执行会撞 `duplicate key value violates unique constraint "pg_class_relname_nsp_index"`
  （PG 上 `IF NOT EXISTS` 不是原子的）。这是**环境/实现**问题，不是夹具问题。
- **`sccache` 在本机起不来**：跑 cargo 前必须 `$env:CARGO_BUILD_RUSTC_WRAPPER=''`。
- **只在 `compat_*` 里操作**：`replay_diff.py --schema public` 会被硬拒绝；
  `pg_env.ps1` / `load_seed.py` 同样只接受 `^compat_[a-z0-9_]+$`。
- **必须在录制当天（UTC 日历日）回放**：跨天会改变 `SalesBatch.is_open`、
  `_expire_stale_orders`、日报的 `today_*`、复购 `days_since` 等分支。
  本轮录制于 `2026-09-20T15:57Z`，回放同在当天。
- **重跑前务必 `-Reset` + `-Apply` + `load_seed.py` 三连**：`replay_diff.py` 会写库
  （下单、支付、审批、建批次…），在**已被上一次回放改过**的库里再跑一遍必然大面积假失败。

---

## 8. 我没有用「放宽比对」换来的数字

如实说明，本轮的每一处放宽都在 §4.2 有据可查：

1. **唯一放宽**：`unordered_at()` 命中的三个叶子字段的数组改成多重集比较（§4.2）。
   来源是 `DEVIATIONS.md` **D6 的既有裁定** + 蓝本 `.distinct()` / queryset 确实没有 `order_by`。
   **不是**为了跑过而放宽：元素数量、元素配对、其余全部字段仍然逐字比对，配不上时照样报失败。
2. **没有**把任何字段整体跳过比较（`skip` / `ignore`）。
3. **没有**把 `int != float`、`str != dict` 之类的类型差异当等价。
4. **没有**把「值不同」当「顺序不同」放过：占位符之外的字符串仍然逐字比。
5. **normalize 只补真正需要的**（§4.4）：判据是「重跑时该字段每次都在变」，而不是「对不上就加」。
   `verified_at` 这类**来自 seed 的领域值**特意**没有**屏蔽。
6. `--self-check`（变异测试）在全部改动之后仍然 **231/231 检出、0 漏检** —— 比对器没有被改软到
   测不出注入的变异。
