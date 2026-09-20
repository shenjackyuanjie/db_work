# Wave 1 实施简则（四域共用）

**先读 `DEVIATIONS.md`，再读本文件。** 四个域在同一时刻并行开工，所以下面的隔离规则是硬要求。

## 0. 你的工作区：独立 git worktree

你的任务书会给你 `<域>`（a/b/c/d）。先在**主仓库**上建自己的工作树：

```powershell
git -C D:\githubs\db_work\db worktree add D:\githubs\db_work\wt-w1<域> -b wt/w1<域>
```

之后**所有**读写 / 编译 / 测试都在 `D:\githubs\db_work\wt-w1<域>\` 里进行。
**主仓库 `D:\githubs\db_work\db` 绝对不要动**——另外三个域正在同一时刻工作。

你只允许改任务书列给你的那几个文件（都在你的 worktree 内）。完成后在**自己的 worktree 分支**上提交：

```powershell
git -C D:\githubs\db_work\wt-w1<域> add <你的文件>
git -C D:\githubs\db_work\wt-w1<域> commit -m "feat(compat): <域>域契约实现"
```

## 1. 环境

- **`sccache` 在本机沙箱里起不来**：每个新 shell 里跑 cargo 之前先设
  `$env:CARGO_BUILD_RUSTC_WRAPPER=''`，否则报 "Timed out waiting for server startup"。
- **格式化只格式化你自己的文件**，不要跑 `cargo +nightly fmt`（它会重写其他域正在写的文件）：

  ```powershell
  rustfmt +nightly --edition 2024 --skip-children src\compat\views_<域>.rs
  ```

  主线最后会统一跑一次全量 `cargo +nightly fmt`。

## 2. 已冻结的接口（直接用，不要改）

- `ser.rs`：`dt_z` / `dt_offset` / `dt_naive_local` / `dt_date` / `dec` / `dec_scaled` / `opt_*`
- `errors.rs`：`api_ok` / `api_ok_message` / `api_error` / `api_response` / `ApiReject`（`bad_request`/`unauthorized`/`forbidden`/`not_found`/`conflict`/`with_json`）/ `ApiResult`
- `auth.rs`（账号域已实现并 27/27 通过回放）：
  - 文案常量：`ERR_MISSING_CREDENTIALS` / `ERR_MALFORMED_HEADER` / `ERR_INVALID_TOKEN` / `ERR_EXPIRED_TOKEN` / `ERR_FARMER_ONLY` / `ERR_BUYER_ONLY`
  - `AuthUser`：`id` / `username` / `email` / `role` / `orchard_address` / `latitude` / `longitude` / `created_at` / `token_key`，方法 `payload()` / `is_farmer()` / `is_buyer()`
  - `async fn authenticate(pool, headers) -> Result<AuthUser, ApiReject>`
  - `async fn require_farmer(pool, headers) -> Result<AuthUser, ApiReject>` / `require_buyer(...)`
  - `async fn lookup_token(pool, token) -> Result<AuthUser, ApiReject>`
  - `async fn create_session(pool, user_id)` / `create_session_tx(...)`
  - `fn auth_error(status, message) -> Response` / `fn auth_error_json(status, value) -> Response`（401 自动补 `WWW-Authenticate: Bearer`）
  - `fn render_method_not_allowed(method) -> Response`（405 契约体）
  - `fn hash_password` / `fn verify_password`

  **签名以 `src/compat/auth.rs` 实际内容为准**，自己读一遍，不要凭这份摘要猜。
- `serde_json` 已启用 `preserve_order`：`json!` 的键序被保留，**逐字节对齐是可达目标**。
  信封必须按 Django 声明序：成功体 `code, message, data, timestamp`，异常体 `code, message, data`。

## 3. 路由与 405

- 你在自己文件里写 `pub(crate) fn router() -> Router<AppState>`，`compat.rs` 已装配。
- **每条路由都要挂 405 兜底**：`post(handler).fallback(no_method)`，`no_method` 内调 `auth::render_method_not_allowed(&method)`。
- **绝对不要**挂 `Router::method_not_allowed_fallback`（`Router::merge` 遇路径级 fallback 会 panic）。

## 4. 契约权威来源

1. 蓝本 Python：`D:\githubs\db_work\navel_backend_git\api\` 下你的域对应的 `.py`
2. 夹具：`db/tests/fixtures/contract/<域>.json` —— **逐条对齐基准**；你的路径清单以
   `index.json` 里 `domain == 你的域` 的条目为准
3. `REPORT.md`：文案字典（60+ 条中文）、状态码、契约怪癖
4. `DEVIATIONS.md`：**哪些行为要修正、哪些逐字复刻**
5. `FORMAT_NOTES.md`：时间 / 数值序列化实证
6. 表结构：`db/src/server/bootstrap/<组>_tables.rs`

## 5. 夹具语义（已实测，直接采信）

- `expected_body` 存的是**原始真值**（不是占位符）；比对时套用每条用例自带的 `normalize` 路径屏蔽。
- 回放顺序：域顺序 `auth → core → commerce → orchard_trace → agent`；每域内 read 在前、mutation 在后。
- `seed.json` 是**回放前**的纯种子态（100 行）；`seed_final.json` 是末态，仅供排查状态差异。
- 带 `query` 字段的用例需自行 urlencode 追加到 `path`。
- 每条 case 上的 `server_error: true`（共 12 条）来自蓝本缺陷，**已裁定修正为正确语义**：
  比对器会归为 `expected_deviation` 不计失败。**不要复刻 5xx。**
- 固定 token 由录制器用 uuid5 生成、并在每个写类用例前重建，`replay_diff.py` 已处理。

## 6. 验证（交付门槛）

```powershell
$env:CARGO_BUILD_RUSTC_WRAPPER=''
$env:COMPAT_TEST_SCHEMA='compat_w1<域>'

# 1) 建 scratch schema
scripts\pg_env.ps1 -Reset compat_w1<域>

# 2) 单测
cargo test -- compat

# 3) 端到端回放（必须用你自己的端口，避免四域抢 11000）
scripts\pg_env.ps1 -Serve compat_w1<域> -Build -Port 1110<x>
python scripts\load_seed.py --schema compat_w1<域>
python scripts\replay_diff.py --base-url http://127.0.0.1:1110<x>/compat --domain <域>
scripts\pg_env.ps1 -Serve compat_w1<域> -Stop
```

- 先用 `--help` 确认 `pg_env.ps1` 与 `replay_diff.py` 的实际参数。
- 目标：你的域**全部 pass**（`expected_deviation` 不计失败）。
- 对不上的逐条查清是「实现问题」还是「夹具/工具问题」。**不要改夹具或 `scripts/` 来绕过**——
  工具若确有 bug，报给主线，由主线修（其他域也在用）。
- 注意请求前缀：夹具里是 Django 的 `/api/...`，打你的服务时要加 `/compat`（比对器已处理）。

## 7. 铁律

- 只改任务书列给你的文件。不要动 `compat.rs` / `ser.rs` / `errors.rs` / `auth.rs` / `dto.rs` /
  `views_auth.rs` / `src/server/**` / `Cargo.toml` / `scripts/**` / `tests/fixtures/**`。
- 只在**自己的 worktree 分支**提交，不要提交到主仓库分支。
- 只操作 `compat_w1<域>` schema。**绝不写/删现网 `public` 下的 `app_*`/`store_*`/`commerce_*`。**
- 需要新 helper 就在自己文件内写局部函数。
- **夹具 > `DEVIATIONS.md` > 个人判断**；夹具与蓝本冲突以夹具为准并记录。
- 不确定就取保守方案并记录，不要停下来等确认。
