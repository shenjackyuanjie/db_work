# W1-A（core 域）交付说明与主线待处理项

- worktree：`D:\githubs\db_work\wt-w1a`，分支 `wt/w1a`，提交 `4bebf78`
- 只改了：`src/compat/views_core.rs`、`src/compat/tests/core_tests.rs`
- 主仓库 `D:\githubs\db_work\db` 未改动（`git worktree list` 确认 `main` 仍为 `964f125`）

## 1. 结果

```
$env:CARGO_BUILD_RUSTC_WRAPPER=''; $env:COMPAT_TEST_SCHEMA='compat_w1a'
scripts\pg_env.ps1 -Reset compat_w1a ; scripts\pg_env.ps1 -Apply compat_w1a
python scripts\load_seed.py --schema compat_w1a
scripts\pg_env.ps1 -Serve compat_w1a -Port 11101
python scripts\replay_diff.py --base-url http://127.0.0.1:11101/compat --domain core --schema compat_w1a
```

- 单测：`cargo test -- compat` → **65 passed / 0 failed**
  （需 `--test-threads=1`，见 §3.1）
- 端到端回放：**42 条 → 36 pass / 4 fail / 2 expected_deviation**

4 条 fail 全部指向夹具/工具问题，不是实现问题，且**在不改夹具与 `scripts/` 的前提下无法消除**。
逐条证据见 §2。

## 2. 4 条 fail 的根因（都能复现）

### 2.1 `tasks_complete_ok` —— 工具缺口：录制期捕获值没有在回放期重建

| | id |
|---|---|
| `tasks_add_ok` 实际创建的 task | `@capture:task_id` → 运行期 `uuid4()`，每次不同 |
| 夹具 `core.json` 里 `tasks_complete_ok` 的请求体 | `"task_id": "9e401969-9f8b-42e7-b301-225c0cb7b3f8"` |
| `index.json.captured_values.task_id` | 同上（**录制当时**那个 uuid4） |

`capture_contract.py` 的 `Ctx.resolve()` 在**录制期**把 `@capture:task_id` 替换成真值，
夹具里存的因此是录制当时的 uuid4；而 `replay_diff.py` 的 `build_request()` 只做
`body.get("IMAGE")` 的替换，**没有**任何 `@capture:` / `captured_values` 重建逻辑。
于是回放时打过去的就是那个陈旧 id，服务按蓝本语义查不到 → 404 `Task not found`。

实现侧无解：这个 id 既不在 `seed.json` 里，也不是本次请求产生的。

**影响面（这会挡住多个域，不只 core）**——按「请求 path/body 里出现非种子 UUID」统计：

| 域 | 用例数 | 含硬编码（捕获）UUID 的用例 |
|---|---|---|
| auth | 27 | 0 |
| core | 42 | 1 |
| commerce | 81 | **38** |
| orchard_trace | 65 | **34** |
| agent | 36 | 5 |

commerce / orchard_trace 的 `@capture:cart_item_b1` / `@capture:order_new` /
`@capture:harvest_archive_id` / `@capture:approval_new` 全在 **URL 路径**里，同理不可达。

**建议（主线改 `replay_diff.py`，一处即可）**：给 `case` 条目补一个
「本用例从哪个响应字段捕获、键名是什么」的映射（`capture_contract.py` 已有该信息，
只是没落盘），回放时按顺序把捕获值回填到 `body` 与 `path`。
落盘位置二选一：`core.json` 各 case 加 `capture: {"task_id": "$.data.id"}`，
或 `index.json` 加 `capture_plan: {case_name: {key: json_path}}`。

### 2.2 `temperature_humidity_get_ok` / `temperature_humidity_post_ok` / `tasks_list_ok` —— 夹具与 `seed.json` 的时间戳精度不一致（且夹具内部不自洽）

`seed.json` 里所有时间戳都是**毫秒精度**（共 214 处 `xxx` 三位小数），灌进 PG 后库里就是 3 位；
而 `core.json` 的期望值带**非零微秒**。更关键的是，同一批记录在不同用例里的期望**互相矛盾**：

| 用例 | 种子 `...37.935Z` 的期望 | 差值 |
|---|---|---|
| `temperature_humidity_get_ok` | `...37.935180+00:00` | +180µs |
| `temperature_humidity_post_ok` | `...37.935176+00:00` | +176µs |

两条用例读的是同一张表、同一 `user_id`、同一段 `ORDER BY ... LIMIT 10`，**记录集合完全相同**，
所以不存在任何实现能同时命中 `+180` 与 `+176`。

单条路径内部也不自洽（以 `temperature_humidity_get_ok` 为例）：

| 记录 | 种子值 | 夹具期望 | 余数 |
|---|---|---|---|
| `recentRecords[0]` | `.936Z` | `.936176` | +176 |
| `recentRecords[1..8]` | `.935Z` | `.935180` | +180 |
| `recentRecords[9]` | `.934Z` | `.934176` | +176 |

同一字段三种余数（176 / 180 / 176），无法归约成一条规则。
`tasks_list_ok` 的 `created_at` 是同一个问题（种子 `.936000`、夹具 `.936176`）。

我验证过这不是 `load_seed.py` 丢精度：`psql` 直接 `insert '...936176+00:00'::timestamptz`
库里就是 `.936176`；且 `load_seed.py` 生成的 SQL 里写的就是 `'2026-09-11T12:20:37.936Z'::timestamptz`，
即**种子文件本身只有毫秒**。夹具的微秒来自一个精度更高的录制库，「加载 seed 再回放」拿不到。

**处理**：没有做「凑数」的补齐（试过两种，都会把别的用例弄坏），实现老实读库。
**建议（主线二选一）**：
1. 让这批用例来自同一次录制，并把 `record_time` / `created_at` 加进 `normalize`
   （时间点本身不该进逐值比对）；
2. 或让 `scripts/load_seed.py` 保住微秒精度，并把 `seed.json` 重新导出到与夹具同一精度。

### 2.3 顺带修掉的一个真实差异（已固化在实现里）

`tasks_list_ok` 的**排序**曾与夹具相反。根因不是时间戳，是 UUID 比较：
三条种子任务 `created_at` 完全相同（平局），

- Django 在 SQLite 里存的是 CHAR(32) / 文本，**按字符串**比较；
- PG 的 `uuid` 是 **16 字节**比较，`-`(0x2D) 排在数字(0x30-)之后。

结果 `6a537b4a…` 与 `8e8dcf2d…` 在两个引擎里顺序相反。
实现已改成 `ORDER BY created_at DESC, id::text` 复刻 Django 的字符串语义
（这条改完后 `tasks_list_ok` 的**排序**已正确，只剩 §2.2 的 `created_at` 微秒差异）。

## 3. 实现要点 / 踩坑记录

### 3.1 `cargo test -- compat` 需要 `--test-threads=1`
`support::pool()` 每个用例都会跑一次 `bootstrap::init_database`，里面的
`CREATE INDEX IF NOT EXISTS idx_app_sessions_expires_at` 在**并发**下会撞
`duplicate key value violates unique constraint "pg_class_relname_nsp_index"`
（PG 上 `IF NOT EXISTS` 不是原子的）。这不是 core 域引入的，先绕过；若不希望要求
`--test-threads=1`，主线可在 `bootstrap.rs` 里对这句加个错误容忍。

### 3.2 逐字复刻的契约点
- **D2**（`fertilization_plan_api` 蓝本恒 500）：按 `serializers.py` 的
  `FertilizationPlanRequestSerializer` 字段定义（8 个必填）做校验，失败返回
  蓝本那一行的固定文案 **`Invalid request data`**（400，成功体形状）；
  成功则按 `FertilizationPlanSerializer` 输出 `{planId, title, content,
  recommendedFertilizers, applicationSchedule}`，`planId` 形如 `fp-YYYYMMDD-NNNN`。
- **D3**：`tasks_complete_ok` 的 `completed_at` 用 `ser::dt_naive_local`，
  输出**无偏移**的本地时间，不要补 `+00:00`。
- **D4**：`created_at` 系走 `ser::dt_z`（`Z`）；温湿度 `record_time` 走
  `ser::dt_offset`（`+00:00`）。
- 温湿度历史标签：`index % 2 == 0 or index == len - 1` 才带 `HH:MM`，其余空串。
- 温湿度无记录时读**项目根** `temperature_humidity_data.csv`，缺失 → 404
  `Temperature data file not found`；空文件 → 404 `Temperature data file is empty`。
- 截断照抄：温湿度 `[:10]`、识别记录 `[:20]`、`node_id[:50]`。
- 三处 `/200 + data=null` 的「无需操作」文案：
  `No task needed for healthy tree`（健康果树/非果树）、
  `No task needed for low risk`（低风险）。
- `citrus-disease`：走 `AppState.inference`（不引 torch），只输出
  `{predicted_class, confidence, stage}`；base64 分支不落识别记录（与蓝本一致）。
- 反向风险分级表 `TEMP_HUMIDITY_RISKS` / 词典 `DISEASE_TREATMENTS` 逐字复制（含中文标点）。

### 3.3 直接打蓝本核对过的两个实现细节
- `tasks_generate_environment` 的 27.5 / 88.0 → 高风险闭区间命中；
  低风险 → 200 + `data=null`。
- 温湿度 POST 的 `record_time` 会落到响应 `recentRecords` 里，且**不对读库值做任何修饰**。

## 4. 其它

- `replay_diff.py` 每次都报 `seed.json 签名与 index.seed_signature 不一致`
  （`cdfa4e33…` vs `7244af9d…`），且 `seed_phase` 判定为 `post-replay`。
  这与 §2.2 的精度差异是同一件事的两面，一并请主线核对夹具版本。
- 需要新仓内路径时用过 `.tmp_*` 临时脚本，已全部删除；提交里只有那两个文件。
