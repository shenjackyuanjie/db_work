# W2 后台管理超集：S2-b 交接记录

范围：`src/web/admin.rs` 的 10 条路径 + `/web/citrus-disease-v2` 的挂载。
前置：`W2_PLAN.md`（处置规划）、`VERIFICATION.md`（契约层跑法）。

## 1. 10 条路径与实测形状（全部带信封，`data` 非 null）

实测环境：`compat_s2b`（53 张表 / 100 行种子）+ 影子服务 11201。

| 路径 | 方法 | 实测 | 关键形状 |
|---|---|---|---|
| `/web/system-status` | GET | 200 | `data` = 6 个键（见 §3） |
| `/web/admin/settings/get` | POST | 200 | `data.settings` = 8 个键 |
| `/web/admin/settings/update` | POST | 200 | `data.settings`，`updated_by` 被写入 |
| `/web/admin/users/list` | POST | 200 | `data.users[].created_at` **秒** |
| `/web/admin/invitations/create` | POST | 200 | `data.{code,expires_at}`；`ttl_seconds=0` → `expires_at=9223372036854775807` |
| `/web/admin/invitations/list` | POST | 200 | `data.invitations[].expires_at` **秒** |
| `/web/admin/pending/list` | POST | 200 | `data.pending_users[].requested_role` 归一为 `admin`/`user` |
| `/web/admin/pending/approve` | POST | 200 / 404 | `data.status="approved"`；通过后**能用注册时的口令登录**（`password_hash` 原样迁移到 `"user".password`） |
| `/web/admin/pending/reject` | POST | 200 / 404 | `data.status="rejected"` |
| `/web/admin/set_admin` | POST | 200 / 404 | `data.status="updated"` |

负向全对：无 cookie → **401 `请先登录`**；buyer cookie → **403 `当前账号无管理权限`**。

## 2. 三条硬约束 = 一套「静默失败清单」（**谁改谁踩**，不是风格问题）

这三条的共同点：写错了**不会报错**，只是前端悄悄拿不到数据。所以必须留在文档里，
不能只写进代码注释——注释会被下一次重构洗掉。

### 2.1 列表/读取端点**不能**挂 `Json` extractor

`admin.js:110-118` 的 `postJson(url, body)`：

```js
const withBody = body !== undefined;
... headers: requestHeaders(withBody), body: withBody ? JSON.stringify(body) : undefined
```

前端对 `users/list`、`invitations/list`、`pending/list`、`settings/get` 的调用是 **`postJson(url)`——没有 body**，于是**既不发 body 也不发 `Content-Type`**。
若这些 handler 挂了 `Json<T>`，axum 会因为缺 `Content-Type` 直接拒绝，前端只会看到「获取失败」，而且**很难联想到是 extractor 的问题**。

### 2.2 时间的单位**按消费方 reader 分别确定**（不是「一律秒」）

`admin.js` 里**两套约定并存**，按端点所属的前端 reader 决定，**不能一刀切**：

| reader | 位置 | 单位 | 用在 |
|---|---|---|---|
| `formatDate(unixSeconds)` | `admin.js:73` | **秒** | 邀请码 `expires_at`、用户/待审批 `created_at` → **本模块 `admin.rs`** |
| `new Date(Number(x))` | `admin.js:1700`、`store-admin.js:164` | **毫秒** | 商城订单 `created_at` → `store_admin.rs` |
| `parseTime(t)` | `admin.js:1023-1029` | 两吃（`t > 1e12 ? t : t * 1000`） | 审计日志 → `dashboard.rs` |

**本模块（`/web/admin/*` 的用户 / 审批 / 邀请码）必须输出「秒」**，依据：
`admin.js:73` 是 `formatDate(unixSeconds)` → `new Date(unixSeconds * 1000)`，且
`> 4102444800` 渲染成「永久有效」——**邀请码 `ttl_seconds=0` 的哨兵依赖这条**（故用 `i64::MAX`）。

契约表 `"user".created_at` 是 `TIMESTAMPTZ`，`app_pending_users.created_at` 是 epoch 秒，
两者混用会显示成 1970 年。`admin.rs` 已显式转换并加注释。

> ⚠️ **不要把这条当成无条件规则。** 完整判据表在 `W2_FRONTEND_HANDOFF.md §8.2`。
> 按「一律秒」去改商城订单的 `created_at`，`admin.js:1700` 与 `store-admin.js:164`
> 会把它显示成 **1970 年**。这条最初被记成了无条件规则，是 S3 拿出反证纠正的。

### 2.3 成功时 `data` 不能是 `null`

`admin.js:10-15` 与 `index.js:32-37` 的 `unwrapApiPayload`：

```js
if (payload && typeof payload === "object" && "data" in payload && payload.data != null) return payload.data;
return payload || {};
```

`data: null` 会让它**回落成整个信封**，前端读 `payload.users` 之类全部拿不到（静默变空列表）。
已加单测 `success_envelope_has_non_null_data` 钉住。

错误体不需要额外造 `error` 键：`readErrorMessage` 是 `payload?.error || payload?.message || ...`，信封里的 `message` 能被正确读到。

## 3. `/web/system-status` 为什么在本地复刻而不是挂载

旧实现 `handlers_core/pages.rs:120` 返回 `api_success(json!({6 个键}))`，
和我用的 `web::session::app_ok` **是同一个信封形状**，所以复刻同样能做到逐字段同形：
同一张 `app_system_settings`、同一个 `load_system_settings`、同一组 6 个键
（`open_registration` / `invite_bypass_enabled` / `maintenance_mode` /
`default_invite_ttl_seconds` / `confidence_threshold` / `log_retention_days`）。

当时直接挂载会撞 `E0603`（`handlers_core` 的外层 `mod` 还是私有）。
**现在主线已把 `mod handlers_core` 与再导出都开成 `pub(crate)`，想合并成直接挂载随时可以。**

消费方：`index.js:143` → `unwrapApiPayload` → `applySystemStatus`，只读
`maintenance_mode` / `open_registration` / `invite_bypass_enabled` 三个键；
**失败是静默的**（`catch` 里啥也不做），所以形状错了不会报错、只会少提示横幅——改的时候要格外小心。

## 4. 表归属

| 用途 | 表 |
|---|---|
| 账号、管理员标记 | `"user"`（**保留字，必须双引号**）+ 加法列 `is_admin` |
| 会话 | `auth_token`（**不再碰 `app_users` / `app_sessions`**） |
| 系统设置 | `app_system_settings`（保留表） |
| 邀请码 | `app_invitations`（保留表） |
| 待审批 | `app_pending_users`（保留表） |
| 审计日志 | `app_admin_audit_logs`（保留表） |

网页注册/审批放行的用户一律 `role='farmer'`（与 `web::session::WEB_DEFAULT_ROLE` 一致）。

## 5. ✅ 跨层缺口已修：会话「双表桥」（**临时过渡态**）

**原症状**（实测）：`POST /web/citrus-disease-v2` 带网页 cookie → **401 `{"error":"Invalid token"}`**。

**根因**：`handlers_ai` 的鉴权走 `crate::user_routes::ensure_authenticated`
（`handlers_ai/request.rs:34-38`、`reports.rs:21`），它原本**只查退役的 `app_sessions`**；
而网页会话落在 `auth_token`，所以永远查不到。影响面是**所有「`/web/*` 复用 legacy handler」的端点**，
不只是这一条（`orchard.rs` 的 overview 不受影响——它直接查 `"user"`）。

**修法（已落地）**：新增 `server::shared::lookup_session_username`，**先查 `auth_token`、
miss 再查 `app_sessions`**；`ensure_authenticated` 与 `username_by_token` 都改走它
（后者保持「吞掉错误」的旧语义，前者保留 DB 错误 → 500 的区分）。

⚠️ **两张表的过期列类型不同**：`auth_token.expires_at` 是 `TIMESTAMPTZ`（SQL 里用 `now()` 比），
`app_sessions.expires_at` 是 epoch 秒（绑 `now_secs`）——**混用会直接报类型错**。

⚠️ **双表桥是临时的**：S5 统一会话表后只保留 `auth_token` 分支，删掉 `app_sessions` 那段
（已登记进 §6 待办）。

### 5.1 仍未桥接的第三处（未改，超出本次授权范围）

`src/user_routes/session.rs:261` —— 旧 `/user/validate` 的查询，join 的还是 `app_users`。
它只服务即将退役的 `/user/validate`；S4-1 把前端全部切到 `/web/session/validate` 后就不影响。
**但切换过程中若有页面还在调旧路径，会看到「未登录」。** 要不要一起桥接由主线定。

### 5.2 本环境无法验证「门控通过」那条分支

`handlers_ai/advanced.rs` 的**门控通过**分支会调用 OpenRouter，而 `config.toml` 里的 key 已失效
（实测报 `API Key 无效或已过期 (401): User not found.`），且**代码没有任何环境变量能覆盖这个 key**
（`OPENROUTER_API_KEY` 只有 `config.toml` 的 `[ai].openrouter_api_key` 一个来源）。
所以本环境只能验证**门控拒绝**分支（`advanced.rs:52-98`：落库 + 200、不碰 LLM）。
要端到端验证「真识别」需要换一个有效的 key。

## 6. S5 待办（从本层看出去的）

1. `src/user_routes/admin/**` 的旧实现（`settings/pending/management/orchard/dashboard`）
   在本层完全取代后即可整棵删除——它读的全是退役表。
2. `src/server.rs` 里给 `handlers_core` / `handlers_ai` / `shared` 挂的
   `#[allow(dead_code)]` 属性：删代码时**必须连属性一起删**，否则那三个模块里日后的真死代码会隐身。
3. 旧路径 `/api/system-status`、`/api/citrus-disease-v2` 在 S4 前端切完之后由 S5 删。
4. `app_sessions` / `app_users` / `app_diagnosis_records` 的 DDL 退役（`bootstrap/legacy_tables.rs`）。
5. **双表桥收敛**（§5）：`server::shared::lookup_session_username` 只保留 `auth_token` 分支，
   删掉 `app_sessions` 那段，并处理 `user_routes/session.rs:261`（§5.1 的第三处）。

## 7. 环境坑（都实际踩过）

1. **PowerShell 里 `\"` 不是转义——用单引号，或反引号。**
   写 `"UPDATE \"user\" ..."` 会把反斜杠原样传给 psql 并报语法错。
   **可执行形式**：整条 SQL 用单引号包住（`psql $u -c 'UPDATE "user" SET ...'`——
   PowerShell 单引号内的双引号是字面量），或在双引号串里用反引号 `` `"user`" ``。
   同一个坑还会让 `git commit -m "…\"…"` **静默失败**，于是它的文件被下一个提交吞掉
   （主线为此把一次分块提交搞砸过，最后靠 tree hash 比对才证明内容没丢）。
2. **`Select-Object -First N` 会提前终止上游管道**，把 Python/psql 拦腰杀掉。
   要 `*> 日志` 之后 `Get-Content -Tail/First`。
3. **`COMPAT_TEST_SCHEMA` 会被单测写脏**：`cargo test` 会往该 schema 插测试数据
   （例如 trace 域的 `w1b_*`）。做实测前先 `-Reset` + `-Apply` + `load_seed` 重灌一遍。
4. **`pg_env.ps1 -Serve` 认 `CARGO_TARGET_DIR`**（第 105 行），`ExePath` 随之改变，
   所以并行流各用独立 target 目录时，跑的就是各自的二进制——不会互相串味。
5. **手写 base64 PNG 极容易 CRC 错**。服务端会报 `Format error decoding Png: CRC error`。
   要用 zlib + 正规 `crc32` 现场生成（见仓库外 `.s2b_gate_probe.py` 的 `png()`）。

## 8. 双表桥的三项验证（本次修复的验收证据）

| 项 | 要求 | 实测 |
|---|---|---|
| ① 契约层未被带坏 | 全序列 **239 / 0 / 12** | **auth 27/27 · core 40+2dev · commerce 81/81 · orchard_trace 64+1dev · agent 27+9dev**，`transport_error=0`，退出码 0 ✓ |
| ② 网页 cookie 可用 | `POST /web/citrus-disease-v2` → **200** | 4/4 张图均 **200**；修复前同一请求是 **401 `Invalid token`** ✓ |
| ③ 双写仍生效 | 两张表**各 +1 / 次识别** | `web_diagnosis_records` **0 → 4**、`disease_recognition_record` **4 → 8**（4 次识别）✓ |

③ 的取巧说明（值得记下来）：**本环境拿不到「门控通过」分支的 200**（§5.2 的 LLM key 失效），
所以改用**门控拒绝**分支（`advanced.rs:52-98`，落库 + 200、不碰 LLM）来验证双写。
同一台服务、同一个 handler、同一条落库路径，只是绕开了外部依赖——
这比「换个 key 再跑一次」更可复现，也不需要凭据。

`git status` 同时确认：本层没有把 `app_sessions` 的写入端改掉（`user_routes/session.rs`
的 INSERT/DELETE 未动），所以历史 `/user/*` 会话在迁移期仍然照旧工作。
