# W2 S5 执行规划（路由 / 模块 / 旧表退役）

> 作者：W2-S4-2。**本轮只写文档，未改任何代码。**
> 所有结论以 `src/**` 与 `static/**` 实际源码为准，给出「文件:行」。取证脚本：`D:\githubs\db_work\.s5_probe.py`（仓库外）。

---

## 0. 一页结论：**顺序约束**（S5 的风险几乎全在这里）

S5 的症状（运行期 `relation does not exist`、启动 panic、前端白屏）都很难查，所以先把**不可交换的顺序**列出来。

| # | 约束 | 为什么不可交换 | 证据 |
|---|---|---|---|
| **O1** | **先做「活/死切分」，再删任何 `#[allow(dead_code)]`** | 三个属性**全都掩盖着活代码**（见 §4），先删属性会喷出 60+ 条告警，而且**真死代码与活代码混在一起**，你会失去唯一的信号 | `server.rs:25,28,31`；`handlers_ai::citrus_disease_advanced_handler` 在 `server.rs:141` 与 `web/orchard.rs:58` 都被路由；`handlers_core::recognition_image_handler` 在 `server.rs:177,181` 被路由 |
| **O2** | **先删死 handler，再删 `shared.rs` 的死项** | `shared.rs` 从 `server.rs` 再导出 **24 项，按「外部引用数」看**全部非零 —— 但其中多数只被**本身就要死**的 `handlers_core`/`handlers_ai` 用。不先删死 handler，你无法判断哪一项真死 | §1 的计数表 |
| **O3** | **先删 `handlers_core` 的死部分，再删 `/user/*` nest** | `handlers_core/pages.rs:69,78,145` 引用 `user_routes::{extract_auth_token, ensure_admin, me_handler}`；`/user` 一删，这些残留引用立刻编译不过 | `pages.rs:69,78,145` |
| **O4** | **双表桥的两段必须同一步删**（`shared.rs:277-285` 的 `app_sessions` 分支 + `app_sessions` DDL） | 桥的第二段就是 `app_sessions`；先删表 → 运行期 `relation does not exist`；先删桥 → 若表还在则无害但也别留着当僵尸。**而且删桥是删 `user_routes::now_secs` 的前提**（`shared.rs:290` 是它最后一个外部使用者） | `shared.rs:253-286`、`shared.rs:290` |
| **O5** | **`app_tasks` 的退役被 `handlers_ai/persistence.rs:124` 阻塞** | 那条 `INSERT INTO app_tasks` 属于**活链路**（`-v2` 识别），先删表 → 识别成功后写任务时 500 | `persistence.rs:124`；活链路见 §2 |
| **O6** | **一律「先删代码引用、后删 DDL」** | `bootstrap::init_database` **每次启动都会跑**。反序（先删 DDL）会让新库不建表，而**仍引用该表的代码在运行期才炸** | `server.rs:271`（启动即调）、`bootstrap.rs:11-53` |
| **O7** | **删 DDL ≠ 删已存在的表** | 从 `legacy_tables.rs` 删掉 `CREATE TABLE IF NOT EXISTS` 只影响**新库**；现网 `public` 里那 12 张旧表**仍然存在**。要真删必须另跑一个**显式执行**的 DROP 脚本，且必须在切换验证通过之后 | `legacy_tables.rs` 全是 `CREATE TABLE IF NOT EXISTS` |
| **O8** | **`server.rs` 是热点，必须一股独占** | 路由删除、`.nest("/user")` 删除、`pub(crate) use shared::{...}` 再导出列表三处都在它里面；两股并行必然冲突 | `server.rs:174`、`server.rs:34-41` |

---

## 1. 路由退役清单（逐条 + 「删之前必须确认什么」）

### 1.1 当前 `server.rs` 里**该删**的路由（`create_router`，`server.rs:68-191`）

| 行 | 路径 | 处置 | 删之前必须确认 |
|---|---|---|---|
| 94-97 | `GET /api/system-status` | **删** | 前端零调用者（S4-1 已把它迁到 `/web/system-status`）；或确认 S4-1 未完工时**暂留**并在其清单里标注 |
| 139-142 | `POST /api/citrus-disease-v2` | **删** | 同上：S4-1 迁到 `/web/citrus-disease-v2`。**注意 `web/orchard.rs:58` 也路由了同一个 handler**，删这条不影响 `/web/*` |
| 148-151 | `GET /api/commerce/storefront` | **删** | 只有死文件 `commerce.js:198` 调它；确认 `commerce.js` 已删（S4-1 的活） |
| 152-155 | `GET /api/commerce/batches/{batch_id}/trace` | **删** | 同上 |
| 156-159 | `GET /api/store/products` | **删** | 只被 `store.js`/`cart.js` 的**旧版**调用；S4-2 已改成 `/api/products`。**grep 确认 `static/**` 零命中** |
| 173-174 | `.nest("/user", crate::user_routes::router(state.clone()))` | **删** | 见 §2：先确认 `user_routes` 的外部引用全部消失（O3） |

> **不在退役范围内**（`server.rs` 里保留）：页面路由 69-135（`/health`、`/app-content/*`、`*.html` 重定向、`/admin`、`/store-admin`、`/store`、`/cart`、`/orchard-3d`**）、契约层 `merge`/`nest("/compat")`（169-170）、超集层 `nest("/web")`（172）、以及 **§7 的三条静态通道（161-163、175-182）**。

### 1.2 `user_routes/mod.rs` 内部的 37 条路由

整棵随 `.nest("/user")` 一起删（见 §2）。

### 1.3 S4 释放的、已在 `server.rs` 里删掉的部分（供对账）

`/api/register|login|logout|validate|user|home|growth-tracking|diagnose|temperature-humidity|citrus-disease|recognition-records|disease-treatment|tasks*|health-point|generate*` 与 `/citrus/analyze`
—— **S1 已删路由，但 handler 代码仍在**（这正是那三个 `#[allow(dead_code)]` 的来源，见 §4）。

---

## 2. `user_routes/**` 整棵退役

### 2.1 外部引用清点（**逐个 grep 过，别整棵删完才发现有人在用**）

```
server\handlers_ai\reports.rs     ×3  ensure_authenticated
server\handlers_ai\request.rs     ×1  ensure_authenticated
server\handlers_core\health_point.rs ×1 ensure_authenticated
server\handlers_core\media.rs     ×1  ensure_authenticated          ← ⚠️ media 是**活代码**（静态通道）
server\handlers_core\pages.rs     ×3  ensure_admin / extract_auth_token / me_handler   ← ⚠️ pages 是**活代码**（页面外壳）
server\handlers_core\records.rs   ×2  ensure_authenticated / username_matches_session
server\handlers_core\tasks.rs     ×9  ensure_authenticated / username_matches_session
server\handlers_core\temperature.rs ×4 ensure_authenticated / username_matches_session
server\shared.rs                  ×1  now_secs                       ← ⚠️ shared 是**活代码**
server.rs                        ×1  router                          ← 要删的那个 nest
```

### 2.2 因此 `user_routes` **不能**一刀切删

`handlers_core/pages.rs` 与 `handlers_core/media.rs` **是活的**（页面外壳 + 识别图静态通道），它们依赖 `user_routes::ensure_admin` / `extract_auth_token` / `me_handler` / `ensure_authenticated`。

**处置**：`user_routes` 的**鉴权与 token 解析**要**搬家**（搬到 `shared` 或新建 `auth` 模块），而不是跟着 `/user/*` 一起消失。
**未选的那条路**：直接在 `pages.rs`/`media.rs` 里改用 `compat::auth` 或 `web::session` —— 会引入跨层依赖（`server::handlers_core` → `web::session`），且 `web::session` 的语义是「超集 cookie 会话」，与页面外壳的诉求不完全一致。

**建议**：新建 `src/auth.rs`（`pub(crate) mod auth`），把 `ensure_authenticated` / `ensure_admin` / `extract_auth_token` / `now_secs` / `username_matches_session` 从 `user_routes` 搬过去，`user_routes` 只留 `router()` 与各 handler；然后删 `user_routes/**`。

### 2.3 `user_routes` 内部还有没有别处被引用

`handlers_core/pages.rs:145` 调 `crate::user_routes::me_handler` —— 这是 `pages.rs` 自己那套 legacy `/api/user` 处理链（已删路由），**属于死代码的一部分**；删 `pages.rs` 的死部分时一并处理。

---

## 3. 旧表 DDL 退役（逐表 + 顺序 + 「谁还在引用」）

全部 DDL 在 `src/server/bootstrap/legacy_tables.rs`。

| 表 | DDL 行 | 现在还谁在引用（文件:行） | 删 DDL 前必须先改掉 |
|---|---|---|---|
| `store_products` | 206 | `handlers_store.rs:57`、`user_routes/store.rs:156,199,238,284,327,417,476,541,680`、`bootstrap.rs:233`（seed demo） | 删 `handlers_store.rs` + `user_routes/store.rs` + `bootstrap.rs` 的 `ensure_store_demo_data` |
| `store_orders` | 219 | `user_routes/store.rs:509,574,605,641,684,689,695` | 同上 |
| `store_order_items` | 233 | `user_routes/store.rs:111,529` | 同上 |
| `commerce_orchards` | 124 | `handlers_commerce.rs:76,166`、`user_routes/commerce.rs:179,216,378,457` | 删 `handlers_commerce.rs` + `user_routes/commerce.rs` |
| `commerce_products` | 135 | `handlers_commerce.rs:26`、`user_routes/commerce.rs:270,307,416,550` | 同上 |
| `commerce_batches` | 146 | `handlers_commerce.rs:75,165`、`user_routes/commerce.rs:393,457,523,874` | 同上 |
| `commerce_batch_products` | （同段） | `user_routes/commerce.rs:550` | 同上 |
| `commerce_orders` | 170 | `user_routes/commerce.rs:589,661,695,721,765,815,865` | 同上 |
| `commerce_order_items` | （同段） | **零引用**（只有 DDL 里的 `REFERENCES`） | 可直接删 |
| `commerce_order_status_logs` | （同段） | **零引用** | 可直接删 |
| `app_users` | 15 | `user_routes/{session,registration,auth,admin/*}.rs` 共 12 处 | 删 `user_routes/**` |
| `app_sessions` | 24 | `user_routes/session.rs:130,205,261`、`shared.rs:278`（**双表桥**）、`bootstrap.rs:21-36`（三条迁移语句） | **O4**：删 `user_routes` + 桥的第二段 + `bootstrap.rs` 的 3 条 ALTER/UPDATE + 那个索引 |
| `app_tasks` | 42 | `handlers_core/tasks.rs:35,107,162,172,280,378`（死）、**`handlers_ai/persistence.rs:124`（活！）** | **O5**：先处理 `persistence.rs:124` 的 `INSERT INTO app_tasks`（改到契约表 `task`，或随 `create_disease_task_if_needed` 一起删） |
| `app_temperature_humidity` | 55 | `handlers_core/temperature.rs:36,116`（死）、`handlers_ai/diagnosis.rs:67`（死 handler）、`user_routes/admin/dashboard.rs:104`（死） | 删这批死代码 |

**必须保留的表**（不在退役范围）：
`app_orchard_trees`、`app_tree_sensor_records`（3D 沙盘）、`app_system_settings`、`app_admin_audit_logs`、`app_invitations`、`app_pending_users`、`store_support_messages`、`web_diagnosis_records`（见 `bootstrap/web_tables.rs` 与 `bootstrap.rs:55-59` 的 `ensure_default_settings`）。

### 3.1 DROP 的执行顺序（对现网 `public`）

`legacy_tables.rs` 里删 CREATE 只影响新库。现网要真删表，**另写一个显式脚本** `scripts/drop_legacy_tables.sql`，并遵守子表先删（旧表之间有外键）：

```
store_order_items → store_orders → store_products
commerce_batch_products → commerce_batches → commerce_orchards
commerce_products（commerce_batch_products 引用它 → 上面那行已先删）
commerce_order_items / commerce_order_status_logs → commerce_orders
app_sessions → app_users（store_orders / commerce_orders 都 REFERENCES app_users）
app_tasks / app_diagnosis_records / app_temperature_humidity（无外键）
```

**执行时机**：切换验证**全部通过之后**，且**先 `pg_dump` 备份**（`scripts/pg_env.ps1 -Backup`）。**不要**把这步混进 `bootstrap::init_database` —— 那会让每次启动都可能删数据。

---

## 4. 三个 `#[allow(dead_code)]`（**前提被推翻**）

`server.rs:19-32` 的注释写：「下面三个模块里的自研 `/api/*` handler 已在 S1 删掉路由……因此整体变成死代码」「S5 删除这些代码时必须把下面三个 `#[allow(dead_code)]` 一起删掉」。

**这个前提对三个模块都不成立**（实测）：

| 属性位置 | 模块 | 实测结论 |
|---|---|---|
| `server.rs:25` | `handlers_ai` | **⚠️ 不是全死**：`citrus_disease_advanced_handler`（`handlers_ai/advanced.rs:18`）被 **`server.rs:141` 与 `web/orchard.rs:58` 两处路由**；它经 `advanced.rs:13` → `persistence.rs` 拉出 `create_disease_task_if_needed`/`save_record_image`/`store_diagnosis_record`，这条链**全是活的** |
| `server.rs:28` | `handlers_core` | **⚠️ 不是全死**：`media::recognition_image_handler`（`media.rs:18`）被 `server.rs:177,181` 路由（`/media/recognition_records/*`、`/uploads/*`，§7）；`pages.rs` 的 `app_shell_handler`/`*_page_handler` 被 `server.rs:69-135` 大量路由 |
| `server.rs:31` | `shared` | **⚠️ 不是全死**：`AppState`（249 处外部引用）、`api_response`（75）、`now_millis`（56）、`task_payload`（10）、`classify_environment_risk`（14）、`save_store_cover_image`（5）等被 `compat/**`、`web/**`、`system_settings.rs` 用着 |

**处置**：
1. **先做活/死切分**（O1）：把三个模块按「活 handler / 死 handler」拆开，死的那部分**连同引用它的 `shared` 项**一起删。
2. **只有当某个模块里再没有死代码时**，才删它头上的属性。
3. 判断「真死」的机械方法：删掉候选代码后 `cargo check --all-targets` **不得出现新的 `never used` 告警**——若出现，说明你删漏了或删错了。

**未选的那条路**：直接把三个属性删掉，让编译器喷告警再逐个清。**不要** —— 告警会同时包含真死代码与活代码内部的局部死代码（例如 `shared.rs` 里 `disease_treatment_text` 只被活链路 `persistence.rs` 用，看着像「没人用」），极易误删。

---

## 5. 别名与双表桥的收尾

### 5.1 派生的兼容别名：**建议「留待观察」，不要现在删**

清单（都在 `/web/*` 的响应里，只读派生、不存库）：`order_no`、`total_cents`、`unit_price_cents`、`line_total_cents`、`price_cents`、`stock_quantity`、`unit_label`、`cover_image`、`is_active`。

**它们现在正被前端消费**，而且不是装饰性的：

| 别名 | 消费方 | 删了会怎样 |
|---|---|---|
| `price_cents` | `store-admin.js:140` `money(p.price_cents)`、`admin.js:1492` `storeMoney` | 立刻 **100 倍误差**（`money()` 是 `Number(x)/100`） |
| `order_no` / `total_cents` | `admin.js:1700` 订单行 | 显示 `undefined` / `NaN` |
| `stock_quantity` / `unit_label` / `cover_image` | 商品列表与编辑表单回填 | 显示 `undefined`，编辑表单空掉 |

**两案对比**：

| 方案 | 代价 | 收益 | 风险 |
|---|---|---|---|
| **A 现在删** | **要求前端再改一遍（第二遍前端改动）**：`money()` 改成直接消费 `price` 字符串、4 处字段名再换一次、编辑回填再对一次 | 少一套字段 | **高**：S4 刚改完、**尚未真机走查**，此时再动一遍等于把未验证的改动叠加成两层 |
| **B 切换稳定后单独删一轮**（建议） | 多留一套只读字段 | 不叠加改动；别名有**明确的删除触发条件**（前端不再读它） | 低 |

**建议 B**，并且把别名**登记为「兼容层」而非「技术债」**——它与 `/compat` 前缀同类：**留着不影响契约，删它需要一个前置条件**。
**触发条件（写进 S5 清单）**：前端改为直接消费 `price`（字符串）**并且**有「100 倍误差已消除」的实测证据，两件事在**同一次提交**里完成。
**不要**默认「登记了就该删」——这一步删错了，症状是「商品价格显示 ¥1.68」，而它不会报错。

#### 触发条件（**可判定**，不是「以后有空再说」）

**当且仅当下面三条同时成立**，才启动别名删除；缺任何一条则保持现状：

1. **前端零消费**（机械判据）：
   ```powershell
   rg -n "price_cents|stock_quantity|unit_label|cover_image\b|is_active|order_no|total_cents|unit_price_cents|line_total_cents" `
      db\static\*.js db\static\*.html
   ```
   **必须零命中**。（注意 `cover_image` 要带词边界，否则会误伤 `cover_image_url`。）

2. **/api 契约不受影响**（回归判据）：这些别名**只存在于 `/web/**` 的超集响应里**，`/api/**` 与 `/api/v1/**`（compat 层）一个字节都不含它们。
   因此删别名**不得**改动 compat 层任何字节 —— 用全序列回放守住：**239 pass / 0 fail / 12 expected_deviation**（跑法与前置见 `VERIFICATION.md`）。

3. **有「100 倍误差已消除」的实测证据**（业务判据）：把前端 `money()` 改成直接消费 `price`（`NUMERIC` 字符串）**之后**，
   必须用当前同一件商品做断言 —— 契约实测值 `price='168.00'`，前端显示必须是 **`¥168.00`** 而不是 **`¥1.68`**，
   并把这条断言的前后对照贴进提交说明。

**三条必须在同一次提交里完成并留下证据**（1 的 grep 输出、2 的回放数字、3 的前后对照）。

### 5.2 双表桥：**与 `app_sessions` 同一步**（O4）

`shared.rs:253-286` 的 `lookup_session_username` 是过渡态：

1. 先查 `auth_token`（新表，`TIMESTAMPTZ`，`> now()`）—— 第 260-275 行
2. miss 再查 `app_sessions`（旧表，**epoch 秒**，`> $2` 且传 `now_secs`）—— 第 277-285 行

它存在的原因（`shared.rs:243-245` 有实测记录）：`/web/*` 复用了 `handlers_ai` 等 legacy handler，而那些 handler 的鉴权只认 `app_sessions`；不桥接则「网页已登录」在 `POST /web/citrus-disease-v2` 上会 401。

**收尾必须一次做完三件事**：
1. 删 `shared.rs:277-285`（`app_sessions` 分支）
2. 删 `app_sessions` 的 DDL（`legacy_tables.rs:24`）+ `bootstrap.rs:21-36` 的三条迁移语句与索引
3. 收掉 `shared.rs:290` 对 `crate::user_routes::now_secs` 的引用（`now_secs` 参数也随之从 `lookup_session_username` 签名里去）

> ⚠️ 第 3 条是**删 `user_routes` 的前置条件**：`shared.rs:290` 是 `user_routes::now_secs` 最后一个外部使用者。

**注意两张表的过期列类型不同**（`shared.rs:250-252` 明确写了），删的时候不要把 `now()` 与 `now_secs` 混用。

---

## 6. 未使用端点候选（扫了一遍全部 `/web/*`）

`/web/*` 当前共 **28 条**（`web/admin.rs` 10 + `web/session.rs` 5 + `web/store_admin.rs` 8 + `web/dashboard.rs` 2 + `web/support.rs` 2 + `web/orchard.rs` 3 —— 其中 `web/orchard.rs:56` 那条是 `-v2`）。

| 端点 | 前端调用方 | 判断 |
|---|---|---|
| `/web/admin/store/overview` | **无**（原 `admin.js:1505` 随 admin.html 商城段删除；`store-admin.js` 的 KPI 用 `analytics` 的 `summary`） | **候选**，见下标准 |
| `/web/session/me` | 无（`web/session.rs:11` 自己标注「前端未使用（旧 `/user/me` 也是孤儿）」） | **候选** |
| `/web/admin/orchard/overview` | `admin.js:887` ✓ | 留 |
| 其余 25 条 | 都有前端调用方（S4-2/S4-3 实测表） | 留 |

**判断标准**（建议）：
- **删**：功能已被另一个端点覆盖，且无人调用（`/web/session/me` 属于此类 —— `/web/session/validate` 返回了同样的 `valid/username/is_admin`）。
- **留作运维接口**：能独立回答一个运维问题、且不依赖网页 DOM（`/web/admin/store/overview` 属于此类 —— 它是「商品/订单/成交额/待处理」四个数的**单次聚合**，`analytics` 需要 `days` 参数且返回体大 10 倍，用它做健康巡检/监控探针更合适）。
- **留**：被前端调用。

**建议**：`/web/admin/store/overview` **留作运维接口并在文档里标注**；`/web/session/me` **删**（与 `validate` 完全重叠，没有任何独立价值）。

---

## 7. 隐式静态通道（**不在退役范围内** + 切换后怎么实测）

这三条**没有任何 JS `fetch`**，靠数据里的路径串 + `<img src>` 生效。路由清理时极易顺手删掉它们的 handler：

| 通道 | 注册点 | handler | 谁在消费 |
|---|---|---|---|
| `/store-images/{file_name}` | `server.rs:160-163` | `handlers_store::store_cover_image_handler`（`handlers_store.rs:22`） | 商品封面。`store-admin.js:140` 一带的 `<img src>`；值由 `shared.rs:17` 的 `STORE_COVER_MEDIA_PREFIX = "/store-images"` 生产 |
| `/media/recognition_records/{file_name}` | `server.rs:175-178` | `handlers_core::recognition_image_handler`（`media.rs:18`） | 识别记录图。`shared.rs:14` 的 `RECOGNITION_RECORDS_MEDIA_PREFIX` |
| `/uploads/{file_name}` | `server.rs:179-182` | 同一个 `recognition_image_handler` | 历史路径，`media.rs:32` 生成旧格式串 |

**⚠️ 注销这三条的连锁后果**：
1. `handlers_store.rs` / `handlers_core/media.rs` **不能整文件删**（§4 已述）；
2. `shared.rs:133-135` 的 `normalize_recognition_record_image_path` 白名单里也硬编码了 `/uploads/` 与 `/media/recognition_records/`；
3. 前端只认这三种前缀（`store.js:61`、`cart.js:59`、`store-admin.js:27` 的正则），**上传后写入 DB 的字符串必须保持前缀**，否则 App 与网页同时 404。

**切换后的实测方法（必须做）**：
```powershell
# 1) 找一个真实存在的文件（上传封面或识别图会产生）
$f = Get-ChildItem db\storage\store_covers -File | Select-Object -First 1
# 2) 从 HTTP 打开它，必须 200 且 Content-Type 是图片
curl.exe -sS -o NUL -w "%{http_code} %{content_type}`n" "http://127.0.0.1:11630/store-images/$($f.Name)"
# 3) 数据里存的路径串要与上一步的 URL 对得上
psql ... -Atc "select cover_image_url from citrus_product where cover_image_url <> '' limit 3"
```
再对 `/media/recognition_records/<file>` 做一次同样的事。

---

## 8. 可并行的拆分（文件所有权互不重叠）

**先说实话**：S5 的后端三件事（模块退役 / 路由退役 / DDL 退役）**通过 `server.rs` 强耦合**（O8），所以**不能三股并行**。真正能做的是「一条串行后端链 + 两条可随时并行的文档/验证支线」。

| 流 | owns（互不重叠） | 依赖 | 内容 |
|---|---|---|---|
| **G1 活死切分 + 模块退役**（串行，先做） | `src/server/handlers_core*`、`src/server/handlers_ai*`、`src/server/handlers_store.rs`、`src/server/handlers_commerce.rs`、`src/server/shared.rs`、**新建 `src/auth.rs`** | 无 | §2.2 的鉴权搬家、§4 的活死切分、§1.3 那批已删路由的 handler 清理 |
| **G2 路由 + DDL 收口**（串行，G1 之后） | `src/server.rs`、`src/server/bootstrap.rs`、`src/server/bootstrap/legacy_tables.rs`、`src/user_routes/**` | **G1** | §1 的路由删除、`.nest("/user")` 删除、`user_routes/**` 整棵删、§3 的 DDL 删除、§5.2 的桥收尾 |
| **G3 文档与偏差收尾**（并行，随时） | `tests/fixtures/contract/**` | 无 | §5.1 的别名触发条件、§6 的端点裁定、本文件的执行记录、`DEVIATIONS.md` 补登 |
| **G4 静态通道实测**（并行，随时；不写代码） | 无（只跑命令 + 写 §7 的实测结果进文档） | 服务可起 | §7 的三条通道实测；**切换后必须做** |
| **G5 前端死文件清理**（并行，S4-1 的活，此处只登记） | `db/static/commerce.js` | 无 | 删文件（它现在已零引用者） |

> **为什么 G1 与 G2 不能合并成一股**：G1 是「删几百行、跨 6 个文件、每删一处都要看编译告警」，G2 是「改一个热点文件的三处 + 删一棵模块树」；合并会让一股的改动面过大、无法定位回归。但**它们必须严格串行**。
>
> **未选的那条路**：让 G2 先删路由（改 `server.rs`），G1 后删 handler —— 顺序反了会先出现「路由删了但 handler 还在」的告警噪音，且 G2 想删 `.nest("/user")` 时会被 G1 未清干净的引用挡住（O3）。

---

## 9. 验收判据

### 9.1 每股当下的判据

| 流 | 判据 |
|---|---|
| G1 | `cargo check --all-targets` **0 error**；**没有新增 `never used` 告警**（新增即说明删漏/删错）；`cargo test -- --test-threads=1 compat` 全绿 |
| G2 | 同上；且 **`/user/*` 全部 404**、`/api/system-status` 与 `/api/citrus-disease-v2` 404、`/web/citrus-disease-v2` **仍 200**；`cargo check` 0 error |
| G3/G4 | 文档与实测结果齐备（§7 的命令输出贴进文档） |

### 9.2 **只有 S5 全部做完才能跑的验证**

1. **全序列契约回放不退化**：`--base-url http://127.0.0.1:11500/compat` 仍 **239 pass / 0 fail / 12 expected_deviation**（`VERIFICATION.md` 的标准跑法；注意它要求先 `-Reset` + `-Apply` + `load_seed`）。
2. **前端全链路可走**：登录 → 识别 → 3D 沙盘 → 商城（列表/加购/下单/支付）→ 后台（五个 section + 商城管理 + 商品 CRUD）→ 客服。**这一步需要真机浏览器**，本批仍未做过。
3. **静态通道实测**（§7）：`/store-images/*`、`/media/recognition_records/*`、`/uploads/*` 各打开一个**真实存在**的文件，必须 200 且 Content-Type 是图片。
4. **现网 `public` 未被写入**：`public` 仍是 **22 张表**、`app_users=6` / `app_tasks=143` / `store_products=4`（与开工前一致）；契约表在 `public` 下**不出现**（它们只在 scratch schema 里）。
5. **DROP 脚本只在验证通过后执行**，且执行前有 `pg_dump` 备份。
6. `cargo +nightly fmt --check` exit 0；`cargo check --all-targets` **0 error 且无新增告警**。

### 9.3 需要主线裁定的点

1. **§5.1 别名**：采纳 B（留待观察 + 触发条件）还是 A（现在删）？我建议 B。
2. **§6 `/web/admin/store/overview`**：留作运维接口（建议）还是删？
3. **§2.2 鉴权搬家**：新建 `src/auth.rs`（建议）还是让 `handlers_core`/`media` 直接依赖 `web::session`？
4. **§3.1 DROP 脚本**：谁执行、什么时候执行？（我的建议：S5 验收全绿后由主线手动执行一次，脚本随仓库提交但不自动跑。）

---

## 附：本次取证的可复现命令

```powershell
# shared 的 24 个再导出项各还有谁在用（活/死判据）
python D:\githubs\db_work\.s5_probe.py

# user_routes 的外部引用
rg -n "crate::user_routes::" db\src --glob '!src/user_routes/**'
# 旧表引用
rg -n "store_products|store_orders|app_users|app_sessions|app_tasks|commerce_" db\src
# 静态通道注册点
rg -n '"(/store-images|/uploads|/media/recognition_records)' db\src
```

---

## G1 执行记录（活/死切分 + 模块退役）

> 作者：W2-S4-2。**判据只有一条：`server.rs` / `compat.rs` / `web/*.rs` 的实际路由注册。**
> 工具：`.g1_probe2.py`（路径限定死活判定）、`.g1_clean.py`、`.g1_shared.py`（仓库外）。

### 第一单元：删了什么 + 依据

| 删除 | 依据 |
|---|---|
| `handlers_core/{static_data, health_point, tasks, records, temperature}.rs`（整文件，13 个 handler） | 这 13 个 handler 在 `server.rs` 的路由里**一个都不出现**（S1 已删它们的路由），且没有被任何其它路径限定引用 |
| `handlers_ai/{diagnosis, reports}.rs`（整文件，4 个 handler） | 同上（`diagnosis::citrus_disease_handler` 的再导出被编译器报 unused → 证明 compat 是**自己的同名实现**而不是复用） |
| `client/fertilization.rs` | 只被 `client/mod.rs:1` 的 `mod` 声明引用；`compat/views_core.rs:1015` 有自己的 `fertilization_plan_impl` |
| `pages.rs::api_user_handler`（19 行） | 无路由引用；它同时是 `user_routes::me_handler` 的**最后使用者** |
| `user_routes/auth.rs::username_matches_session` + 测试（14 行） | 唯一使用者是上面删掉的 `tasks/temperature/records` |
| `models.rs` 的 5 个 fertilization struct（55 行） | 唯一使用者是被删的 `reports.rs` |
| `shared.rs` 的 16 个 never-used 项（约 150 行） | 撤掉 `#[allow(dead_code)]` 后由编译器逐个指认 |
| `handlers_ai/review.rs::apply_review_threshold_to_prediction`（22 行） | 编译器指认；**同文件的 `apply_review_threshold_to_fields` 被 `advanced.rs:143` 用着，保留** |
| **三个 `#[allow(dead_code)]`**（`server.rs:25,28,31`） | §4：三者都掩盖活代码，撤销后编译器一次暴露 17 处真死代码 |
| `server.rs` 再导出列表 24 项 → **10 项** | 14 项只被上面删掉的死模块引用 |

### 三条被编译器纠正的偏差（**方法论的教训**）

1. **裸标识符统计会系统性高估「活的」**：v1 探针把 `compat/**` 的**同名局部函数**与**注释里的名字**都算成引用。
   实测：`compat/views_core.rs:158` 有它自己的 `classify_environment_risk`、`:1429` 有它自己的 `task_payload`；
   `web/admin.rs:177` 的注释在说它**复刻**了 `system_status_api_handler`。→ 改用路径限定匹配。
2. `shared` 的 24 个再导出里实际只有 **10 个**是活的（`AppState`/`api_response`/`api_success`/`now_millis`/`save_store_cover_image`/`disease_treatment_text`/`risk_from_disease_name`/`save_recognition_record_image`/`lookup_session_username`/`username_by_token`）。
3. O1 被完整印证：不撤属性就看不见这 17 处。

### 验证

`cargo check --all-targets` **exit 0**；`cargo test -- --test-threads=1 compat` **126 passed / 0 failed**；
告警 **16–18 → 14**，且 14 条**全是既有的**（`compat/*` 9 + `inference/*` 4 + `web/support.rs` 1），**G1 相关清零**。

### 标了「待确认」、**没删**

- `inference/{mod,onnx}.rs` 的 `is_climate_in_range` / `NORMAL_TEMP_RANGE` / `NORMAL_HUMIDITY_RANGE`（4 条告警）—— 不在 G1 文件范围，且 `inference` 是活模块（`AppState` 依赖），不确定是否有别的流要用。
- `compat/*` 的 9 条 unused —— compat 是活层面，其内部未用 helper 属独立话题。
- `web/support.rs:35 MESSAGE_LIMIT` —— S2 的文件。

### 两处偏离（已报主线）

1. 动了不在 G1 清单里的文件：`server.rs`（3 属性 + 再导出列表）、`user_routes/{mod,auth}.rs`、`models.rs`、`client/mod.rs` —— 都是**删除后必须同步的引用**，**均非路由改动**（`server.rs` 路由段一行未动、`.nest("/user")` 仍在）。
2. 每步只跑 `cargo check`（4 秒），**全量 `cargo test` 在单元结束时跑一次**（200 秒）。理由：删除的代码无路由可达，行为不可能变，编译器即定位工具。

### G1 剩余：§2.2 鉴权搬家（下一步）

新建 `src/auth.rs`，把 `ensure_admin` / `ensure_authenticated` / `extract_auth_token` / `now_secs` / `parse_requested_role` 从 `user_routes` 搬过去，让**仍是活代码**的 `handlers_core/{pages,media}.rs` 不再依赖 `user_routes` —— **这是 G2 能删 `/user/*` 的前提（O3）**。本单元已删掉 4 个引用方，搬家前会重新清点 `user_routes` 的外部引用面。

### 第二单元：§2.2 鉴权搬家（已完成）

**新建 `src/auth.rs`**，从 `user_routes/auth.rs` 搬走 5 个项：`now_secs` / `parse_requested_role` /
`extract_auth_token`（+ 私有 `token_from_cookie`）/ `ensure_authenticated` / `ensure_admin`。

| 步骤 | 做法 |
|---|---|
| 4 个**活**调用点 | `handlers_core/{pages,media}.rs`、`handlers_ai/request.rs`、`server/shared.rs` → 改指 `crate::auth::*` |
| `user_routes` 内部约 10 处 | **一行都不用改**：把 `user_routes/mod.rs` 的再导出改成 `pub(crate) use crate::auth::{...}`；另有 5 个文件是**嵌套导入**（`use super::{auth::{...}, dto::X}`）需单独改指 |
| 顺手修正 | **`ensure_admin` 从 `app_users` 改读契约表 `"user".is_admin`**。理由：`app_users` 是退役目标，而网页侧 `web::session::{lookup_is_admin, require_admin}` 已统一读 `"user".is_admin`——两处读不同的表会导致「网页登录的管理员打不开后台页面」 |

**✅ 关键判据达成**：搬完后重新清点，`user_routes` 的外部引用面**只剩 1 处** —— `server.rs: crate::user_routes::router`（就是 G2 要删的那个 `.nest("/user")`）。
**→ G2 可以干净地整棵删掉 `user_routes/**`，不会挂到别的东西。**

> ⚠️ **补记（G2 复核时）：`ensure_admin` 改表的语义代价**
>
> 上表最后一行「顺手修正」不是纯等价重构，它有一处**会改变行为**的过渡代价：
> 「只在 `app_users` 里被标成 `is_admin = TRUE`、而契约表 `"user".is_admin` 仍是默认 `FALSE`
> 的账号」**会丢失页面准入**——`handlers_core/pages.rs` 的 `has_admin_session` 走的正是
> `ensure_admin`，所以这类账号会被 `/admin`、`/store-admin`、`/analyze` 的重定向挡回 `/`。
>
> 为什么不是理论风险：现网 `public` 里 `app_users`（6 行）与契约表 `"user"` 是**两套独立数据**，
> 两边的管理员标记从来没有同步过。
>
> 判定为**可接受的过渡态**，理由：S2 之后管理员身份的唯一权威就是 `"user".is_admin`
> （`web::session::{lookup_is_admin, require_admin}` 读的也是它），G2 只是把仅剩的那处
> 不一致读法对齐；迁移动作是一行 SQL：
> `UPDATE "user" SET is_admin = TRUE WHERE username = '<管理员账号>'`。
>
> **G2 已把 `app_users` 的 DDL 一并删除，所以「两套数据」的产生窗口到此关闭**；但现网
> `app_users` 表与里面的标记**仍在**（O7：删 DDL ≠ 删表）。S5 的 DROP 脚本执行**之前**，
> 必须确认这类账号都已在 `"user"` 里补齐。

验证：`cargo check --all-targets` **exit 0**；`cargo test -- --test-threads=1 compat` **126 passed / 0 failed**；
告警 **16–18 → 14**，且 14 条**全是既有的**（`compat/*` 9 + `inference/*` 4 + `web/support.rs` 1）。

> **第三次遇到同一种假失败**：本轮先出现 **11 条 `commerce_tests` 失败**，全是夹具比对测试；
> `commerce.json` / `seed.json` **确被别的流改过**（`git status` 可见）。按 §0.1 的规则先重灌种子 → **126/0**。
> **代码没有回归。** 这条规则（共享 schema 一红先重灌种子）已连续三次生效，请继续在所有流里执行。

---

## 0.1 判据：**活死怎么判**（G1 实测得出，G2 与 S5 其余部分沿用同一判据）

> **判活死只能用编译器或路径限定引用（`handlers_X::NAME`、`crate::server::handlers_X::NAME`）。
> 裸名字统计会系统性高估「活的」。**

两个真实反例（G1 用自己的错误换来的）：

| 反例 | 现象 |
|---|---|
| `compat/views_core.rs:158` | 定义了**它自己的** `classify_environment_risk`。裸名字统计会把它算成「compat 复用了 `shared.rs` 的同名函数」→ 结论「这项是活的、不能删」。同文件 `:1429` 还有它自己的 `task_payload` |
| `web/admin.rs:177` | 注释写「与旧 `handlers_core::system_status_api_handler` 逐字段一致」——那是在说它**复刻**了，不是在挂载。裸名字统计把注释也算成引用 |

**为什么这个错误危险**：按错误结论「删」会留下真死代码（保守方向，尚安全）；但**反向使用**——用它判断「这些还被用着、所以不能删」——S5 就会永远删不掉东西。

**G1 已验证的正确做法（G2 直接沿用）**：
1. 从 `server.rs` / `compat.rs` / `web/*.rs` 的**实际路由注册**里抽取 `handlers_X::NAME` 集合 → 这才是「活」；
2. 排除注释行（以 `//` / `///` 开头）；
3. 撤掉 `#[allow(dead_code)]`，用**编译器**逐个指认真死代码；
4. 每步 `cargo check --all-targets`（它同时验证「路由注册引用的函数是否还存在」）。

**O1 的实证**：撤掉三个属性后，编译器**一次暴露 17 处 never-used**（`shared.rs` 16 + `review.rs` 1）——这是对 S1 那句「保留属性会让日后的真死代码隐身」的**实测印证**，不是引用。

**工具**：`.g1_probe2.py`（路径限定死活判定）、`.s5_probe.py`（shared 再导出的活/死统计），都在仓库外。

---

## 0.2 前置条件：**跑单测之前必须先重灌种子**（这是前置条件，不是补救手段）

> **任何 `cargo test -- --test-threads=1 compat` 之前，必须先做完这一串：**
> `pg_env.ps1 -Reset <schema>` → `pg_env.ps1 -Apply <schema>` → `load_seed.py --schema <schema>`。
> 直接跑单测，红出来的那几条**全是假失败**，而且是同一副面孔。

`compat_test` 是一个**被多个流共享的 scratch schema**：夹具 JSON
（`commerce.json` / `seed.json` / `core.json` / `auth.json` / `agent.json` / `orchard_trace.json`）
会被别的流重新 capture 而改变，schema 里的数据却还是你上次灌进去的旧种子。
两者一错位，`compat::tests::*_tests` 这批**夹具比对**测试必然红——**代码一行没动也一样红**。

实测计数（同一形态，连续四次）：

| 次数 | 红的条数 | 当时的错判 |
|---|---|---|
| 第 1 次 | **9** 条 | 「我这次改坏了」 |
| 第 2 次 | **11** 条 | 同上 |
| 第 3 次（G1 第二单元） | **11** 条 `commerce_tests` | 同上；`git status` 显示 `commerce.json` / `seed.json` **确被别的流改过** |
| 第 4 次（G2） | 见下方 G2 记录的验证节 | 已按本规则处理 |

**每次重灌后都是 125 passed / 0 failed。**

> ⚠️ **更正：「126」是笔误，真实数字一直是 125。** G1 记录与本文件多处写的 `126 passed`
> 是转录错误，已在下文改正。核对方式（**别再用裸数字对账**）：
> `cargo test -- --test-threads=1 compat` 的过滤是**子串匹配**，命中的是
> **124 条** `src/compat/**` 里的真测试（`support.rs:9` 那一条 `#[tokio::test]` 是**文档注释里的示例**，
> 不是测试！）+ **1 条** `web::session::tests::cookie_name_matches_compat_reader`
> （函数名里含 "compat" 才被捞进来）= **125**。
> 在 `90c2998`（G1 之前）、`73ce4d1`、`fece92f`、以及当前工作树上**都是 125**，逐点核过。
> 实测：在独立 schema `compat_g2` 上 `125 passed; 0 failed; 0 ignored; 43 filtered out`。
> **所以「125 vs 126」不是回归**，不必再查。

所以这条纪律的正确用法是**前置**：不要等它红了
再去猜是回归还是漂移——先在开跑前重灌，让「红」重新变成有信息量的信号。
（真实回归仍然能被抓到：重灌种子后还红，才是回归。）

---

## G2 执行记录（路由 / 模块 / 旧表退役）

### 前置条件核对（主线给的 6 条 + 1 条附加，逐条落实）

| # | 前置条件 | 落实 |
|---|---|---|
| 1 | **O3**：`user_routes` 外部引用面清零才能删 | ✅ 只剩 `server.rs` 的 `.nest("/user", ...)` 一行（G1 第二单元达成） |
| 2 | **O4**：双表桥与 `app_sessions` 的 DDL **同一步**删 | ✅ step A 同一步删掉桥的第二段、`legacy_tables` 的 CREATE + 索引、`bootstrap.rs` 的 3 条迁移语句 |
| 3 | **O5 先修**：治理任务写契约表 `task` | ✅ 先落 `handlers_ai/persistence.rs`，`app_tasks` 从此零代码引用 |
| 4 | **O7**：**不写 `DROP TABLE`** | ✅ 只删 `CREATE TABLE IF NOT EXISTS`；现网 `public` 一行未动 |
| 5 | 不整文件删 `handlers_store.rs` / `handlers_core/media.rs` | ✅ 两者都保留（它们是静态通道），并逐条实测 |
| 6 | 前端零调用者证据必须区分「注释」与「调用」 | ✅ 见下「判活死证据」一栏 |
| + | `store-admin.js:390` 的 `/api/supply-batches` **必须保留** | ✅ 未触碰该文件；它是活的前端调用 |

### Step A：O4 桥退役 + `app_sessions` + 静态通道改表

- `server/shared.rs`：`lookup_session_username` 的 `app_sessions` 分支删除，`now_secs` 参数从
  签名里去掉；文档注释改为「只认 `auth_token`」。
- `legacy_tables.rs`：`app_sessions` 的 CREATE + 索引删除。
- `bootstrap.rs`：`app_sessions` 的 3 条 `ALTER/UPDATE` 迁移语句删除。
- **顺手挖出并修掉第二处 O5 型地雷**：`handlers_core/media.rs` 的归属校验查的是
  `app_diagnosis_records`，而 S2 已把识别写入口切到 `web_diagnosis_records`
  → **两条识别图静态通道本来会永久 404**。
  第一版改法是「改查 `web_diagnosis_records`」，但随后查现网数据发现那会反过来打坏 24 张老图，
  最终定案为**两张表 UNION 的过渡态读桥** —— 完整推理与实测数据见下面
  「`app_diagnosis_records` 的处置说明」。
  这类「读端与写端分两次搬家」的错位，编译器和 grep 都抓不到，**只能靠实测通道 + 查现网数据**。

### 第二个新地雷：识别图通道对**合法会话**也返回 500（INT4 / INT8 解码错）

> **状态：已修复。这是「既有的」缺陷，不是本次改动的回归** —— HEAD 上同款写法同样中招，
> 也就是说这两条通道**在本次改动之前就是坏的**。后来人 bisect 到这里时不要怀疑是 G2 改坏的。

这是「必须实测通道」这条纪律的第二个实证，比 O5 那个更隐蔽——它**改前就存在**：

- **现象**：`/media/recognition_records/<file>` 与 `/uploads/<file>`，带**合法** session token
  返回 **500**；无 token → 401；别人的 token → 404。（401/404 都正常，只有「本该 200」的那条坏。）
- **日志原文**：
  `ERROR … media: 校验识别图片归属失败 err=error occurred while decoding column 0: mismatched types; Rust type i64 (as SQL type INT8) is not compatible with SQL type INT4`
- **根因**：归属校验写的是 `query_scalar::<_, i64>("SELECT 1 FROM … LIMIT 1")`，
  而 PG 里 `SELECT 1` 的字面量类型是 **INT4**，`i64` 要的是 INT8 → sqlx 解码失败。
- **HEAD 上同款写法同样中招**（只是查 `app_diagnosis_records`），所以**不是 UNION 引入的**；
  但它是**活的**两条通道，等于这两条通道一直是坏的。
- **修法**：换成布尔语义，从根上消除类型歧义 ——
  `query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 … UNION ALL SELECT 1 …)")` + `fetch_one`。
- **诚实记录**：这个 bug 编译器、grep、单测**都抓不到**，只有真的带会话发一次 HTTP 才暴露。
  所以「三条隐式静态通道必须实测」不是形式主义 —— 它是本次唯一抓到它的手段。

### 工具链回归：`pg_env.ps1 -Apply` 的阈值被「合法删表」打死（已修）

`Assert-DdlExtracted` 里有一道 `if ($statements.Count -lt 90) { throw … }` 的守卫。
S5/G2 删掉 9 张旧表后抽取总量 91 → **82**，于是 `-Apply` 直接拒绝执行，
**权威配方 `VERIFICATION.md` 的第 2 步整条失效**（验证代理跑配方时撞上的，不是猜的）。

处置：阈值 **90 → 60**，并把注释改成解释「为什么不能把下限写成『当前条数 + 余量』」——
每个分组非空的检查**已经**覆盖「正则失效」这一失败模式，总量下限只兜「契约表整块抽丢」；
60 按契约层量级定（30 张契约表约 66 条，且契约层冻结、只会增不会减）。

> **教训**：守卫的阈值一旦写成「当前值 + 余量」，它就会在下一次**合法**的规模变化时变成拦路石。
> 要么把阈值锚在不变量上（契约层量级），要么只做语义检查（分组非空）。
>
> 这也是 §0.1 的推论：**光读代码、光编译都发现不了它，只有真按 `VERIFICATION.md` 跑一遍配方才会撞上。**
> 判活死靠编译器，判「工具链还能不能跑」只能靠跑。

### Step B：路由 / 模块 / 旧表 DDL 退役

删除的路由（`server.rs`）：`GET /api/system-status`、`POST /api/citrus-disease-v2`、
`GET /api/commerce/storefront`、`GET /api/commerce/batches/{batch_id}/trace`、
`GET /api/store/products`、`.nest("/user", ...)`。

> **这一刀之后 `/api/**` 与 `/api/v1/**` 100% 由契约层注册**，服务里不再有任何自研 `/api/*`。
> 这既是本次的成果，也是往后必须守住的边界（自研能力一律进 `/web/*`）。

删除的模块：`src/user_routes/**`（16 个文件 / 4316 行）、`src/server/handlers_commerce.rs`（236 行）。
删除的 DDL：`app_users`、`app_tasks`、`app_temperature_humidity`、7 张 `commerce_*`、
`store_products` / `store_orders` / `store_order_items`；连带删掉 `bootstrap.rs` 里往
`store_products` 灌示例商品的 `ensure_store_demo_data`（否则新库会插进一张不存在的表）。

**保留（每一条都有活读者，不许因为「看起来像遗留」再删）**：

| 保留项 | 活读者 |
|---|---|
| `store_support_messages` | `web/support.rs`（客服会话） |
| `app_pending_users` / `app_invitations` / `app_admin_audit_logs` / `app_system_settings` | `web/{admin,session,dashboard}.rs`、`system_settings.rs` |
| `app_orchard_trees` / `app_tree_sensor_records` | `web/orchard.rs`、`bootstrap.rs` 的 demo 种子 |
| `handlers_store.rs` | `/store-images/*` 静态通道 |
| `handlers_core/media.rs` | `/media/recognition_records/*`、`/uploads/*` 两条静态通道 |
| `bootstrap/commerce_tables.rs` | **契约表**（`citrus_product` 等）。名字带 commerce，但和退役的 `commerce_*` 毫无关系——**这是本次最容易误删的文件** |
| `app_diagnosis_records` | `handlers_core/media.rs` 的**双表读桥**（UNION 第二支）+ 现网 24 行历史行，详见下 |

`app_diagnosis_records` 的处置说明（**中途改过一次结论，这里是最终版**）：

- 原计划保留的理由：「活的 `handlers_core/media.rs` 还在引用它」。
- step A 把 `media.rs` 改查 `web_diagnosis_records` 之后，这个理由一度**不成立**，
  它变成零代码引用 —— 当时的打算是「只为了端历史行而保留」。
- 随后查现网发现**这个改动本身会打坏历史图片**，于是定案为**双表读桥**：

| 事实（现网 `public` 实测，只读查询） | 值 |
|---|---|
| `app_diagnosis_records` 行数 | **24**（24 行都带 `image_path`） |
| `image_path` 前缀分布 | `/media/recognition_records/` × 24，`/uploads/` × 0 |
| 这 24 行的 username | 全部 `tester` |
| `web_diagnosis_records` | **表还不存在**（S2 的 DDL 要等首次启动才建，且建出来是空的） |

也就是说：识别记录的**写口**在 S2 从 `app_diagnosis_records` 搬到了 `web_diagnosis_records`，
但**历史行没搬**。于是

- 只查新表 → 那 24 张老图**全部 404**（归属校验在新表里找不到行）；
- 只查旧表 → 所有**新**图 404（写口已经不往旧表写了，这正是 step A 修掉的原始 bug）。

**定案**：`media.rs` 的归属校验改成两张表 `UNION ALL`，任一支命中即放行。
它是**过渡态桥**，与 S5 的 `DROP` **同一步收尾**：将来删 `app_diagnosis_records` 时，
必须同时删掉 UNION 的第二个分支，否则新库直接 `relation does not exist`。
这条耦合已写进 `legacy_tables.rs` 与 `media.rs` 的注释里。

> 这与 O4 的会话双表桥是同一种病：**同一份数据在「读端」与「写端」分两次搬家**，
> 中间任何时刻都存在一个「读的一侧是空表」的窗口。识别图这一处是第二例，
> 而且**是查现网数据才发现的**——编译器、grep、单测都不会报它。

### 判活死证据（按 §0.1 的判据，不用裸名字）

- 5 条候选路由在 `db/static/**` 的所有 `.js` / `.html` 里的命中**全部是注释或迁移说明表**
  （`store.js:7-10`、`cart.js:8-11`、`analyze.js:197`、`index.js:148` 都是「旧路径 → 新路径」的
  对照注释），**没有一处真实调用**；`db/scripts/**` 与 `db/tests/**` 的 `.rs` 零命中
  （只有 `.md` 文档提到它们）。
- `commerce.js` 在 S4-1 已删除，`/api/commerce/*` 的最后两个调用方随之消失。
- `handlers_core::system_status_api_handler` 的唯一「引用」在 `web/admin.rs:177` 的**注释**里
  （那句话是在说「复刻」而不是「挂载」）——这正是 §0.1 记录的第二个反例。

### 编译器纠正的两处（与 G1 同样的模式）

删完路由后 `cargo check` 立刻指出还有两处再导出悬空，**都不是路由**：

1. `handlers_core.rs` 的 `pub(crate) use pages::{..., system_status_api_handler}` —— 函数本体
   已随路由删除，再导出要同步删（`/web/system-status` 由 `web/admin.rs` 自己的同形实现承担）。
2. `server.rs` 的 `pub(crate) use shared::{api_response, api_success, ...}` —— 这两个信封构造器
   的**唯一**使用者是 `handlers_store::storefront_handler`（已删）。
   → 连带把 `shared.rs` 里这两个函数本体也删掉（`compat` 有自己的 `api_ok`/`api_error`，
   `web` 有自己的 `app_ok`/`app_err`，这份是第三份重复实现）。

另外清掉 4 处因删除而变成死代码的项：`auth.rs::{now_secs, parse_requested_role}`、
`models.rs::{RequestedRole, RequestedRole::is_admin}`（`web/admin.rs:149` 与 `web/session.rs:232`
各自有私有实现，不受影响）。

**告警回到基线 14 条**（`compat/*` 9 + `inference/*` 4 + `web/support.rs` 1），**无新增**。

### 同一批文档

- §0.2 新增（单测前置规则）。
- §2.2 补记 `ensure_admin` 改表的语义代价。
- **裁定：`static/app-shell.js:30` 的 `/commerce → /store` 归一化「保留」**（并补了注释说明
  **为什么不能用「零引用」当删除依据**）：仓库里确实已无任何 html/js 链接到 `/commerce`
  （`commerce.js` 已删），但 `app-bridge.js:44` **包住了 `history.pushState`**，会把
  **原生 App 传来的任意同源路径**交给 `shell.normalize()` —— 那条入口**不经过服务端**，
  `server.rs` 的 308 覆盖不到它。删掉这一行的后果不是「功能消失」（终点仍是 `/store`），
  而是退化成整页重载、iframe 里多拉一次 `/app-content/`。既然代价只是一行，
  就保留并把理由写在代码旁，免得下一个读到它的人又把它当死代码删掉。
- `AGENTS.md` 重写：原文还在描述「`/user/*` 是用户域路由层」「会话落 `app_sessions`」
  「`handlers_commerce.rs` 负责团购」，而且完全没提 `compat/` 与 `web/` 两层 —— 全是过时事实。
- `static/WEB_ENDPOINT_MAP.md` 加历史快照横幅（它描述的是 S1 之前的状态）。
- `W2_ADMIN_NOTES.md` §5 补一行「本节所述双表桥已不存在」。

### 留给 S5 的动作项（G2 只取证，没动手）

1. **历史识别数据回填（用户可见）**：网页仪表盘与 3D 沙盘的读源是 `web_diagnosis_records`，
   而现网的历史 24 行还在 `app_diagnosis_records`。**部署后仪表盘的识别统计会显示 0**，
   直到发生新的识别。两张表列结构相同（已逐列核对 `bootstrap/web_tables.rs` 与
   `bootstrap/legacy_tables.rs`：17 列的**名字、顺序、类型全部一致**，
   `web_diagnosis_records` 就是照抄 `app_diagnosis_records` 建的），所以回填是：

   ```sql
   INSERT INTO web_diagnosis_records
       (id, timestamp, predicted_class, confidence, is_citrus_leaf, citrus_type, is_healthy,
        disease_name, severity, treatment_suggestion, preventive_measures, image_quality_warning,
        username, area, image_path, temp, humm)
   SELECT id, timestamp, predicted_class, confidence, is_citrus_leaf, citrus_type, is_healthy,
          disease_name, severity, treatment_suggestion, preventive_measures, image_quality_warning,
          username, area, image_path, temp, humm
     FROM app_diagnosis_records
   ON CONFLICT (id) DO NOTHING;
   ```

   两表 `timestamp` 语义也一致（**都是 epoch 毫秒**）。上面按列名显式列出而不用 `SELECT *`——
   位置虽然对得上，但显式列名在日后再加列时不会静默错位。
   这与「媒体通道的双表读桥」是同一件事的两半：**桥让图能看，回填让统计能看**。
2. **`app_diagnosis_records` 的 DROP 必须与 `media.rs` 的 UNION 第二支同一步**（见上），
   而且**拆桥之前先要满足一个数据条件**，不能只当作代码顺序问题：
   `public.app_diagnosis_records` 那 **24 行是现网真实资产**（`tester` 的识别图，不是测试数据），
   所以拆桥 / 删表之前，要么**先执行上面第 1 条的回填**（新表里有了对应行，
   UNION 第二支就不再被需要），要么**明确记录并接受这 24 张图失效**。
   三种顺序里有两种是坏的：

   - 先拆 UNION、后回填 → 老图在中间窗口**全部 404**；
   - 先删 DDL、后拆 UNION → UNION 第二支直接 `relation does not exist`；
   - ✅ **正确顺序：回填 → 验证新表命中 → 再「同一步」拆 UNION + 删 DDL。**
3. 现网 `public` 的现状（只读实测，供部署对账）：
   - **22 张表**，全是自研遗留表（`app_*` / `store_*` / `commerce_*`），**包含 `app_sessions`**
     —— O7 的实证：DDL 删了，表还在。
   - **30 张契约表一张都不在 `public`**：`to_regclass('public.disease_recognition_record')`
     与 `to_regclass('public.web_diagnosis_records')` **都是 NULL**；契约表只存在于
     `compat_*` scratch schema 里。
   - 结论：**合并后的服务还没在现网库上启动过**。首次启动会由 `bootstrap` 建出契约表
     + `web_diagnosis_records`（都是 `IF NOT EXISTS`，不会动已有的 22 张表）。
     这不是待办，是**首次部署时的预期**，写下来免得第一次启动时被「怎么多出一堆表」吓一跳。
4. `static/commerce.css` 已成孤儿（`commerce.js` 在 S4-1 删了，8 个 html 里零引用；
   同目录的 `market.css` 仍被 `store.html` 引用，**别一起删**）。属前端清理，不在 G2 范围。
5. `admin.css:614-760` 那组 `.commerce-*` 类是否还有使用者，**未裁定**（`market.css` 同理有
   一段 commerce 专用样式）。删之前要按 §0.1 的判据查 class 使用而非文件引用。

---
