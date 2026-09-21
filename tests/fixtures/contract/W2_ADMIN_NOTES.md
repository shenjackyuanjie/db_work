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

## 2. 两条硬约束（**谁改谁踩**，不是风格问题）

### 2.1 列表/读取端点**不能**挂 `Json` extractor

`admin.js:110-118` 的 `postJson(url, body)`：

```js
const withBody = body !== undefined;
... headers: requestHeaders(withBody), body: withBody ? JSON.stringify(body) : undefined
```

前端对 `users/list`、`invitations/list`、`pending/list`、`settings/get` 的调用是 **`postJson(url)`——没有 body**，于是**既不发 body 也不发 `Content-Type`**。
若这些 handler 挂了 `Json<T>`，axum 会因为缺 `Content-Type` 直接拒绝，前端只会看到「获取失败」，而且**很难联想到是 extractor 的问题**。

### 2.2 时间一律输出「秒」

`admin.js:73` 的实现是 `formatDate(unixSeconds)` → `new Date(unixSeconds * 1000)`，且
`> 4102444800` 渲染成「永久有效」——**邀请码 `ttl_seconds=0` 的哨兵值依赖这条**（所以用 `i64::MAX`）。

契约表 `"user".created_at` 是 `TIMESTAMPTZ`，`app_pending_users.created_at` 是 epoch 秒，
两者混用会显示成 1970 年。`admin.rs` 里已显式转换并加注释。

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

## 5. ⚠️ 未解决的跨层缺口：`/web/citrus-disease-v2` 对网页会话恒 401

**现象**（实测）：`POST /web/citrus-disease-v2` 带网页 cookie → **401 `{"error":"Invalid token"}`**。

**根因**：`handlers_ai` 的鉴权走 `crate::user_routes::ensure_authenticated`
（`handlers_ai/request.rs:34-38`、`reports.rs:21`），那是查**退役的 `app_sessions`**；
而网页会话现在落在 `auth_token`，所以永远查不到。

**影响面**：所有「`/web/*` 复用 legacy handler」的端点，不只是这一条。
`orchard.rs` 的 overview 不受影响（它直接查 `"user"`，不走 legacy 鉴权）。

**建议修法**（一行级，在 `src/user_routes/auth.rs`，**属主线文件**）：
让 `ensure_authenticated` / `username_by_token` **同时接受 `app_sessions` 与 `auth_token`**。
这本来也是 S5「统一会话表」要做的，提前一步做掉可以让整条 legacy 复用链立刻可用。

## 6. S5 待办（从本层看出去的）

1. `src/user_routes/admin/**` 的旧实现（`settings/pending/management/orchard/dashboard`）
   在本层完全取代后即可整棵删除——它读的全是退役表。
2. `src/server.rs` 里给 `handlers_core` / `handlers_ai` / `shared` 挂的
   `#[allow(dead_code)]` 属性：删代码时**必须连属性一起删**，否则那三个模块里日后的真死代码会隐身。
3. 旧路径 `/api/system-status`、`/api/citrus-disease-v2` 在 S4 前端切完之后由 S5 删。
4. `app_sessions` / `app_users` / `app_diagnosis_records` 的 DDL 退役（`bootstrap/legacy_tables.rs`）。

## 7. 环境坑（都实际踩过）

1. **PowerShell 里 `\"` 不是转义**。写 `"UPDATE \"user\" ..."` 会把反斜杠原样传给 psql 并语法报错。
   用反引号 `` `"user`" `` 或单引号字符串。
2. **`Select-Object -First N` 会提前终止上游管道**，把 Python/psql 拦腰杀掉。
   要 `*> 日志` 之后 `Get-Content -Tail/First`。
3. **`COMPAT_TEST_SCHEMA` 会被单测写脏**：`cargo test` 会往该 schema 插测试数据
   （例如 trace 域的 `w1b_*`）。做实测前先 `-Reset` + `-Apply` + `load_seed` 重灌一遍。
