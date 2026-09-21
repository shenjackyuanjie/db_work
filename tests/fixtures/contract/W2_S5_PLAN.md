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
