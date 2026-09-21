# W2 权威处置规划（路由 / 前端 / 超集 / 旧表退役 / 分阶段）

> 作者：W2-P。基准 commit `ef80a2e`。所有结论均从源码实证，标注「文件:行」。
> 用户已裁定：① 6 类 Django 从未建模的能力保留为 Rust 超集挂 `/web/*`；② 现货商城收敛到
> Django `citrus_product`；③ 旧 `store_*` 据此退役。

---

## 0. 结论摘要（先看这一节）

**目标命名空间（三层，互不重叠）**

| 层 | 路径 | 性质 | 谁用 |
|---|---|---|---|
| **契约层** | `/api/**`、`/api/v1/**` | 从 `/compat` **原样提升**，逐字兼容 Django | App + 网页 |
| **超集层** | `/web/**` | Rust 独有运营能力，cookie 会话，**不保证**逐字兼容 | 仅网页 |
| **外壳层** | `/`、`/app-content/*`、`*.html`、`/health`、`/store-images/*`、`/media/**`、`static` 兜底 | 页面外壳 + 静态/媒体 | 浏览器 |

**`/user/**` 整棵树 + 自研 `/api/*` 处理器全部删除。**

**处置统计**（遗留侧共 **92** 条 route 条目 = `src/server.rs` 55 + `src/user_routes/mod.rs` 37）

| 处置 | 条数 | 说明 |
|---|---|---|
| **KEEP** | 28 | 页面外壳、`*.html` 重定向、`/health`、`/store-images/*`、`/media/recognition_records/*`、`/uploads/*`、static 兜底 |
| **MOVE → `/web/*`** | 27 | 会话 5、3D 2、客服 3、设置/审批/邀请码/仪表盘 11、商城运营超集 4、识别与系统状态 2 |
| **DELETE** | 36 | 自研 `/api/*` 24、`/user/commerce/*` 8、`/user/store/orders` 1、`/user/admin/store/products*` 3 |
| **（nest 自身）** | 1 | `.nest("/compat")` 被提升接线取代 |
| **PROMOTE** | 79 | compat 的 Django 路径（含 `/api` 与 `/api/v1` 双前缀） |

**最重要的一个发现**：网页端**从不调用任何自研 `/api/*` 处理器**——`/api/login`、`/api/register`、
`/api/logout`、`/api/validate`、`/api/user`、`/api/home`、`/api/tasks*`、`/api/diagnose`、
`/api/health-point`、`/citrus/analyze`、`/api/recognition-records`、`/api/disease-treatment`、
`/api/growth-tracking`、`/api/temperature-humidity`、`/api/generate*` 在 `db/static/**` 的
9 个 `.js` 与 8 个 `.html` 里**零命中**（实证命令见 §1.4）。它们只服务过 App 或历史版本，
因此**删除它们不需要任何前端改动**。

---

## 1. 路由处置总表

### 1.1 来源与基线

清点自两处（这是**全部**注册点，无遗漏）：

- `src/server.rs:59-217` —— 根 Router，55 条 `.route()` + 1 个 `.nest("/compat")` + 1 个 `.nest("/user")` + `fallback_service`
- `src/user_routes/mod.rs:28-152` —— `/user` 子树，37 条 `.route()`
- `src/compat.rs:38-46` —— `/compat` 内装配 5 个子路由，展开为 Django 的 79 条 path

### 1.2 KEEP（28 条，不动）

| 路径 | 证据 | 理由 |
|---|---|---|
| `/health` | `server.rs:60` | 存活探针 |
| `/` | `server.rs:61` | app-shell 入口 |
| `/app-content/{home,store,cart,admin,analyze,orchard-3d,store-admin}` | `server.rs:62-77` | iframe 内容页，`app-shell.js:112-120` 按 `names` 映射拼出 |
| `/index.html` | `server.rs:78-81` | 永久重定向到 `/` |
| `/admin`、`/store-admin`、`/store-admin.html`、`/analyze`、`/store`、`/cart`、`/orchard-3d`、`/commerce` | `server.rs:105,106,107,112,126,131,136,118` | 页面入口；`app-shell.js:30-34` 把 `/commerce`→`/store`、`/store-admin`→`/admin?mode=store` 做规范化 |
| `/admin.html`、`/analyze.html`、`/commerce.html`、`/store.html`、`/cart.html`、`/orchard-3d.html` | `server.rs:108,113,122,127,132,137` | 老书签兼容的永久重定向 |
| `/store-images/{file_name}` | `server.rs:193-196` | **隐式静态通道**，见 §2.4 |
| `/media/recognition_records/{file_name}` | `server.rs:201-204` | **隐式静态通道**；Django `MEDIA_URL='/media/'`，App 拿到的就是 `/media/recognition_records/...` |
| `/uploads/{file_name}` | `server.rs:205-208` | 历史输入别名；输出侧已被 `normalize_recognition_record_image_path` 归一到 `/media/...`（`src/server/shared.rs:123-146`）。**可删**，但需先确认库里没有存量 `/uploads/` 值 |
| `fallback_service(ServeDir::new("static"))` | `server.rs:209` | 托管全部静态资源 |

### 1.3 DELETE（36 条）

**D-1 自研 `/api/*` 与页面级 API（21 条）** —— 全部被 compat 取代，且**前端零引用**：

`/api/register`(82)、`/api/login`(83)、`/api/logout`(84)、`/api/validate`(85-88)、
`/api/user`(89)、`/api/home`(94)、`/api/growth-tracking`(95-98)、`/api/diagnose`(99)、
`/api/temperature-humidity`(100-104)、`/citrus/analyze`(141)、`/api/citrus-disease`(142-145)、
`/api/recognition-records`(150-153)、`/api/disease-treatment`(154-157)、`/api/tasks`(158)、
`/api/tasks/add`(159)、`/api/tasks/complete`(160-163)、`/api/tasks/generate/disease`(164-167)、
`/api/tasks/generate/environment`(168-171)、`/api/health-point`(172-175)、`/api/generate`(176)、
`/api/generate/fertilization-plan`(177-180)。

**D-2 自研团购（8 条）** —— 前端只有**死代码**引用（`commerce.js` 未被任何 html 加载，见 §2.5）：

`/user/commerce/orders`(mod.rs:74-77)、`/user/commerce/orders/{order_id}`(78-81)、
`/user/admin/commerce/orchards`(82-85)、`/user/admin/commerce/products`(86-89)、
`/user/admin/commerce/batches`(90-93)、`/user/admin/commerce/orders`(94-97)、
`/user/admin/commerce/orders/status`(98-101)、`/user/admin/commerce/overview`(102-105)。
对应表 `commerce_*`（7 张）一并退役（§4）。

**D-3 现货商城自研（1 条）**：`/user/store/orders`(mod.rs:106-109) → 前端改打 Django `/api/orders`（§2.2）。

**D-4 自研商品 CRUD（3 条）**：`/user/admin/store/products`(110-113, POST+GET)、
`/user/admin/store/products/{product_id}`(118-121, PUT)、
`/user/admin/store/products/{product_id}/toggle`(114-117, POST)
→ 前端改打 Django `/api/v1/farmer/products`(POST+GET) 与 `/api/v1/farmer/products/<uuid>`(GET/PUT?)，
详见 §2.3。

**D-5 被取代的商城只读（3 条）**：`/api/commerce/storefront`(181-184) —— 只被死代码 `commerce.js:198` 调用；
`/api/commerce/batches/{batch_id}/trace`(185-188) —— 语义与 Django `/api/traces/<trace_code>` **不同**
（batch_id vs trace_code），无前端引用；`/api/store/products`(189-192) —— 前端改用 `/api/products`（§2.2）。

**对账（必须自洽）**

```
遗留 route 条目 = 92
  src/server.rs:59-217 ................ 55 条 .route() + 2 个 .nest()  → 计入 55
  src/user_routes/mod.rs:28-152 ....... 37 条 .route()
  55 + 37 = 92 ✓

处置分解：
  KEEP   = 28
  MOVE   = 27
  DELETE = 21 (D-1) + 8 (D-2) + 1 (D-3) + 3 (D-4) + 3 (D-5) = 36
  nest   =  1   （.nest("/compat") 自身；.nest("/user") 随其 37 条子路由一并处置，不另计）
  28 + 27 + 36 + 1 = 92 ✓

另：PROMOTE 新增 79 条 Django 路径（/api/** 与 /api/v1/** 双前缀），
    这条不占遗留 92 的名额，是净新增。最终根 Router ≈ 28 + 79 + 27 = 134 条条目。
```

### 1.4 MOVE → `/web/*`（27 条）

| 旧路径 | 新路径 | 方法 | 前端引用者 | 理由 |
|---|---|---|---|---|
| `/user/login` (mod.rs:43) | `/web/session/login` | POST | `index.js:225` | cookie 会话是 Rust 超集（Django 用 Bearer） |
| `/user/register` (44) | `/web/session/register` | POST | `index.js:260` | 同上，且带审批流 |
| `/user/logout` (45) | `/web/session/logout` | POST | **9 个文件** | 同上 |
| `/user/validate` (46) | `/web/session/validate` | POST | **9 个文件** | 返回 `valid/is_admin`，Django `/api/me` 没有 `is_admin` |
| `/user/me` (47) | `/web/session/me` | POST | 无 | 与 validate 同族，一并迁移以免留孤儿 |
| `/user/orchard/overview` (70-73) | `/web/orchard/overview` | POST | `orchard-3d.js:839` | 3D 沙盘几何数据 |
| `/user/admin/orchard/overview` (66-69) | `/web/admin/orchard/overview` | POST | `admin.js:887` | 同上（管理端口径） |
| `/user/store/support` (30-34) | `/web/support` | GET+POST | `store-support.js:19`、`cart.js`(经 store-support) | 客服会话 |
| `/user/admin/store/support` (35-38) | `/web/admin/support` | GET+POST | `store-admin.js:188,203,375`、`admin.js` | 客服会话（管理端） |
| `/user/admin/store/analytics` (39-42) | `/web/admin/store/analytics` | GET | `store-admin.js:79` | 后台仪表盘 |
| `/user/admin/set_admin` (48) | `/web/admin/set_admin` | POST | `admin.js:165` | 账号管理超集 |
| `/user/admin/invitations/create` (49-52) | `/web/admin/invitations/create` | POST | `admin.js:146` | 邀请码 |
| `/user/admin/invitations/list` (53-56) | `/web/admin/invitations/list` | POST | `admin.js:266` | 邀请码 |
| `/user/admin/users/list` (57) | `/web/admin/users/list` | POST | `admin.js:277` | 账号管理超集 |
| `/user/admin/settings/get` (58-61) | `/web/admin/settings/get` | POST | `admin.js:1063` | 系统设置 |
| `/user/admin/settings/update` (62-65) | `/web/admin/settings/update` | POST | `admin.js:1079` | 系统设置 |
| `/user/admin/pending/list` (140-143) | `/web/admin/pending/list` | POST | `admin.js:255` | 注册审批 |
| `/user/admin/pending/approve` (144-147) | `/web/admin/pending/approve` | POST | `admin.js:288` | 注册审批 |
| `/user/admin/pending/reject` (148-151) | `/web/admin/pending/reject` | POST | `admin.js:300` | 注册审批 |
| `/user/admin/dashboard/stats` (135-138) | `/web/admin/dashboard/stats` | POST | `admin.js:929` | 后台仪表盘 |
| `/user/admin/dashboard/logs` (139) | `/web/admin/dashboard/logs` | POST | `admin.js:1011` | 审计日志 |
| `/user/admin/store/overview` (134) | `/web/admin/store/overview` | POST | `admin.js:1505`、`store-admin.js` | 商城概览超集 |
| `/user/admin/store/orders` (126-129) | `/web/admin/store/orders` | POST | `admin.js:1726`、`store-admin.js:179` | 订单管理超集（Django 无管理端状态接口） |
| `/user/admin/store/orders/status` (130-133) | `/web/admin/store/orders/status` | POST | `admin.js:1733`、`store-admin.js:348` | 同上 |
| `/user/admin/store/products/{product_id}/cover` (122-125) | `/web/admin/store/products/{id}/cover` | POST | `admin.js:1650`、`store-admin.js:327` | 封面 multipart 上传 |
| `/api/citrus-disease-v2` (server.rs:146-149) | `/web/citrus-disease-v2` | POST | `analyze.js:196` | 识别超集：响应比 Django 多 6 个字段 |
| `/api/system-status` (90-93) | `/web/system-status` | GET | `index.js:143` | 系统设置；挂在登录页首屏，**不能**留在 `/api/**` |

### 1.5 冲突点清单（提升后必须显式处理）

**19 处直接路径冲突** —— 全部是「自研实现 vs Django 契约」同名：

| compat 路径 | 现有自研路由 | 处理 |
|---|---|---|
| `/api/register` | `server.rs:82` | **自研 DELETE** |
| `/api/login` | `server.rs:83` | 自研 DELETE |
| `/api/logout` | `server.rs:84` | 自研 DELETE |
| `/api/user` | `server.rs:89` | 自研 DELETE |
| `/api/home` | `server.rs:94` | 自研 DELETE |
| `/api/growth-tracking` | `server.rs:95` | 自研 DELETE |
| `/api/diagnose` | `server.rs:99` | 自研 DELETE |
| `/api/temperature-humidity` | `server.rs:100` | 自研 DELETE |
| `/api/citrus-disease` | `server.rs:142` | 自研 DELETE |
| `/api/recognition-records` | `server.rs:150` | 自研 DELETE |
| `/api/disease-treatment` | `server.rs:154` | 自研 DELETE |
| `/api/tasks` | `server.rs:158` | 自研 DELETE |
| `/api/tasks/add` | `server.rs:159` | 自研 DELETE |
| `/api/tasks/complete` | `server.rs:160` | 自研 DELETE |
| `/api/tasks/generate/disease` | `server.rs:164` | 自研 DELETE |
| `/api/tasks/generate/environment` | `server.rs:168` | 自研 DELETE |
| `/api/generate` | `server.rs:176` | 自研 DELETE |
| `/api/generate/fertilization-plan` | `server.rs:177` | 自研 DELETE |
| （`/api/me`、`/api/v1/**`） | 无 | 无冲突，纯新增 |

> **为什么必须删而不是「谁后注册谁生效」**：axum 对同路径重复注册**直接 panic**
> （重复 `.route()` 或 `Router::merge` 同路径都会 panic，启动即挂）。所以
> **必须「先删后提升」**，或把提升写成一个独立 Router 再 merge —— 但两条路都不允许同名共存。

**7 处「相邻/影子」需注意（非冲突，但会误导）**：

| 项 | 现状 | 结论 |
|---|---|---|
| `/api/store/products`(189) vs `/api/products`(compat) | 两条都在 | 前端改用 `/api/products`，删自研条 |
| `/api/commerce/storefront`(181) vs `/api/products` | 语义重复 | 删（只被死代码调用） |
| `/api/commerce/batches/{batch_id}/trace`(185) vs `/api/traces/{trace_code}` | **键语义不同**（batch_id ≠ trace_code） | 删自研；若日后需要按 batch 查溯源，另开 `/api/v1/supply-batches/<uuid>`（Django 已有） |
| `/api/citrus-disease`(契约) vs `/api/citrus-disease-v2`(超集) | axum 精确匹配，可共存 | `-v2` MOVE 出 `/api/**`，避免超集污染契约层 |
| `/user/me`(POST) vs `/api/me`(GET) | 同义异构 | `/user/me` → `/web/session/me`；`/api/me` 是契约 |
| `/api/validate`(85) vs `/user/validate`(46) | 两条，前端只用后者 | `/api/validate` 直接 DELETE（零引用） |
| `/api/health-point`(172) | 零引用 | DELETE（3D 用 `/web/orchard/overview`） |

### 1.6 落地写法（给人照着改 `create_router`）

```rust
// src/server.rs::create_router —— 目标形态
Ok(Router::new()
    // ---- 外壳层：原样保留 ----
    .route("/health", get(handlers_core::health_handler))
    .route("/", get(handlers_core::app_shell_handler))
    .route("/app-content/home", get(handlers_core::index_page_handler))
    /* ...其余 /app-content/*、页面与 *.html 重定向不变... */

    // ---- 契约层：原 /compat 的 79 条路径，直接挂到根（其内部就是 /api/** 与 /api/v1/**）----
    .merge(crate::compat::router())

    // ---- 超集层 ----
    .nest("/web", crate::web::router())

    // ---- 静态/媒体（不变） ----
    .route("/store-images/{file_name}", get(handlers_store::store_cover_image_handler))
    .route("/media/recognition_records/{file_name}", get(handlers_core::recognition_image_handler))
    .route("/uploads/{file_name}", get(handlers_core::recognition_image_handler))
    .fallback_service(ServeDir::new("static"))
    /* ...middleware / CORS / with_state 不变... */)
```

要点：
1. **删掉** 全部 D-1..D-5 的 `.route()` 与 `.nest("/user", ...)`。
2. `crate::compat::router()` 现在返回含 `/api/**` 与 `/api/v1/**` 的 `Router<AppState>`，**用 `.merge()` 而不是 `.nest("/compat", ...)`**。
3. 新增 `crate::web::router()`，形态与 `compat::router()` 完全一致（`Router<AppState>`，子模块各自 `router()`），这样并行开发时 `/web` 下的子模块互不冲突。
4. 删完自研路由后，`handlers_core`、`handlers_ai`、`handlers_store`、`handlers_commerce` 里会有大量死函数；
   **先不删**（保留到 S5 一次性清理），但要把 `handlers_commerce.rs` 整文件、`handlers_store.rs` 的
   `storefront_handler` 标为待删。
5. `src/user_routes/mod.rs` 的 `.nest("/user", ...)` 删除后，`user_routes` 模块仍需存在（`/web` 会复用
   `admin::*`、`store_workspace::*` 等 handler），但 `mod.rs::router()` 应整体删除。

---

## 2. 前端改动清单

### 2.1 会话路径替换（机械改动，9 个文件）

`/user/validate` → `/web/session/validate`、`/user/logout` → `/web/session/logout`

| 文件 | 行 | 现值 |
|---|---|---|
| `app-shell.js` | 63, 166 | `/user/validate`、`/user/logout` |
| `index.js` | 191, 307 | 同上（另见 §2.2 的 login/register） |
| `store.js` | 116, 313 | 同上 |
| `cart.js` | 104, 241 | 同上 |
| `analyze.js` | 55, 247 | 同上 |
| `admin.js` | 87, 131 | 同上 |
| `store-admin.js` | 385 | `/user/validate` |
| `orchard-3d.js` | 73, 826 | 同上 |
| `commerce.js` | 76, 253 | **整文件删除**，无需改 |

`index.js` 另需：`:225 /user/login` → `/web/session/login`、`:260 /user/register` → `/web/session/register`、
`:143 /api/system-status` → `/web/system-status`。

**响应形状不变**（`/web/session/*` 是自研 handler 的平移），所以只改字符串，不改解析逻辑。

### 2.2 现货商城：整数 id / 分 → UUID / Decimal（`store.js`、`cart.js`）

**字段映射表**（Django `citrus_product` ↔ 现有自研 `store_products`）

| 现有前端字段 | 出处 | Django 字段 | 前端改法 |
|---|---|---|---|
| `p.id`（整数，`Number(id)`） | `store.js:48,68,337,361,367,380`；`cart.js:47,66,257,262,275` | `id`（**UUID 字符串**） | **删掉所有 `Number(...)` 转换**；`state.cart` 的键从整数变字符串；`dataset.*Product` 直接透传 |
| `p.price_cents`（分） | `store.js:128,149,156,186,188,214` | `price`（**Decimal 字符串**，如 `"58.00"`） | `money()` 改为接收元字符串：`¥${p.price}`；排序用 `Number(p.price)`；**不要再 `/100`** |
| `p.stock_quantity` | `store.js:49,51,52,190,198,215,217,300,339,369`；`cart.js:48,50,51,170,203,264` | `stock`（整数） | 全量改名 `stock` |
| `p.is_active` | `store-admin.js:142,145`；`admin.js:1538,1542` | `status`（`draft`/`on_sale`/`off_sale`） | `is_active` → `status === "on_sale"`；上下架 = PATCH `status` |
| `p.unit_label` | `store.js:149` | `unit` | 改名 |
| `p.cover_image` | `store.js:199`；`cart.js:134`；`store-admin.js:130`；`admin.js:1526` | `cover_image_url` | 改名；值仍是 `/store-images/...`（见 §2.4） |
| `p.description` | 商品表单 | `description` | 不变 |
| — | — | **`sales_batch_id`（必填外键）** | **新增字段**：商品必挂销售批次。商品列表需带出批次，下单时回传 |
| — | — | `sku_type`/`variety`/`origin`/`sweetness`/`grade`/`purchase_limit`/`minimum_order_quantity`/`harvest_date`/`shipping_note` | 可选展示，前端可先忽略 |
| `p.sku` | 商品表单 | **无对应** | `citrus_product` 没有 `sku`；表单去掉 SKU，或映射为 `sku_type`（枚举，语义不同，**不建议**硬映射） |

**列表接口**：`store.js:284`、`cart.js:183` 的 `/api/store/products` → **`/api/products`**
（Django `product_list_api`，支持 `q`/`orchard_id`/`sku_type` 过滤，只返回 `status='on_sale'`）。

**购物车：localStorage → 服务端**（用户裁定 ②）

- 现状：`store_cart_v1` 存 `{productId: quantity}`（整数键），见 `cart.js`/`store.js` 的 `state.cart`
- 目标：Django `/api/cart`（GET 列表 / POST 加购 / PATCH 改量 / DELETE 删）+ `/api/addresses`
- **旧数据迁移策略：直接丢弃**。理由：本地存的是**整数 id**，Django 是 UUID，没有任何可靠映射；
  `sales_batch` 约束也无从推断。实施：首次加载时若检测到 `store_cart_v1` 非空，
  `localStorage.removeItem("store_cart_v1")`，并用 `app-shell` 状态栏提示一次
  「购物车已升级，请重新添加商品」。
  **未选的那条路**：写一个整数→UUID 的映射表 —— 需要旧 `store_products` 与新 `citrus_product`
  的对应关系，而两者 SKU 语义不同（前者 `sku`，后者无 `sku`），无法自动对齐，成本远大于收益。
- 数量上限：Django 有 `purchase_limit`（限购）与 `minimum_order_quantity`（起购），
  前端 `Math.min(quantity, stock)` 要加上这两个约束。

**订单**：`store.js:274`、`cart.js:216` 的 `/user/store/orders` → **`/api/orders`**
- 提交体从「整数 product_id + quantity」变成 Django 的订单结构（含 `sales_batch_id`、`recipient_*`、`items[*].product_id`=UUID）。**实施前必须先读 `commerce.json` 里 `orders_create_ok_buyer` 与 `v1_order_api` 夹具**确认真实字段名——那是唯一权威。
- 订单列表响应字段：`total_amount`(Decimal 串)、`order_number`、`status`(10 态) 取代
  `total_cents`、`order_no`、`status`(旧 5 态)。`store.js:239,259` 的 `line_total_cents`/`total_cents`
  与 `new Date(Number(order.created_at))` 都要改（Django 是 ISO8601 字符串，直接 `new Date(str)`）。

### 2.3 后台商品管理：改打 Django 农户端接口

`/user/admin/store/products`(POST+GET) → **`/api/v1/farmer/products`**（`farmer_product_list_create_api`）
`/user/admin/store/products/{id}`(PUT) → **`/api/v1/farmer/products/<uuid>`**（`farmer_product_detail_api`）
`/{id}/toggle` → Django 无对应端点，**用 PATCH 改 `status`**（`on_sale` ↔ `off_sale`）；若契约里详情接口不支持 PATCH，则退化为「先 GET 再 PUT」。

表单需新增 **销售批次选择器**（`sales_batch_id` 必填）：批次列表来自 `/api/v1/farmer/batches`。

**未选的那条路**：把商品 CRUD 留在 `/web/admin/store/products`（保留自研 `store_products` 表）。
否掉的原因：与用户裁定 ② 直接冲突，且会让两套商品模型长期共存。

### 2.4 隐式静态通道（**最容易漏**，单独一节）

这三条路由**没有任何 JS `fetch`**，只通过 `<img src>` 或后端返回的路径字符串被引用。
grep 实证：`db/static/**` 里 `<img src="/store-images...">`、`src="/uploads`、`src="/media` **零命中**；
只有 `admin.html:299` 的 placeholder 文本提到了 `/store-images/xxx.jpg`。真正的引用方式是**数据驱动**：

| 通道 | 值从哪来 | 谁渲染 | 结论 |
|---|---|---|---|
| `/store-images/{file}` | 商品数据里的 `cover_image` / `cover_image_url` | `store.js:199`、`cart.js:134`、`admin.js:1526-1527`、`store-admin.js:130` 经 `coverImageSrc()` 拼 `<img src>` | **路由保留**。上传后仍写入 `/store-images/<uuid>.jpg` 形式的值 |
| `/media/recognition_records/{file}` | 识别记录里的 `image` / `imagePath` | `analyze.js` 识别结果图；**App 侧**也用（Django `MEDIA_URL='/media/'`） | **路由必须保留**，且 compat 输出的路径必须就是这个前缀 |
| `/uploads/{file}` | 历史库里可能存的旧前缀 | 无 JS 引用 | 保留作输入别名；**待数据核查后可删** |

**实施要求**：
1. 封面/识别图上传后写入 DB 的字符串，**必须**保持 `/store-images/...` 与 `/media/recognition_records/...` 前缀，否则 App 与网页都会拿到 404。
2. `citrus_product.cover_image_url` 是 `VARCHAR(500)`，塞相对路径没问题（App 侧按 baseUrl 拼接的行为需在切换前实测一次，见 §5 验收）。
3. `coverImageSrc()`（`store.js`/`cart.js`/`store-admin.js` 各自实现一份）需确认它不假设整数/相对路径以外的东西——**它接受 `/store-images/...` 与 `http(s)://` 两种**，迁移后值不变，故不用改。

### 2.5 死代码（建议 W2 一开始就删，减少后续改动面）

| 项 | 证据 | 结论 |
|---|---|---|
| `commerce.js`（275 行） | 无任何 html 加载：`grep '<script src=' db/static/*.html` 只命中 `store.js`/`store-support.js`/`store-admin.js`/`index.js`/`cart.js`/`app-shell.js`/`analyze.js`/`admin.js` | **整文件删除** |
| `admin.js` 的 commerce 段（约 1180-1451：`commerceMoney`、`renderCommerce*`、`refreshCommerceData`(1325)、`bindCommerceActions`(1430)、`commerceRequest`） | ① 两个函数**只在定义处出现**，无调用点；② `admin.html` 里 grep `commerce` **零命中**，DOM 元素根本不存在 | **整段删除** |
| `admin.html` 的 store 面板（218-342：`storeSection`、`storeOverview`、`storeProductForm`、`storeProductsWrap`、`storeOrdersWrap`、`storeCoverFile`…） | 与 `store-admin.html` 功能重复；`app-shell.js:31-34` 把 `/store-admin` 规范化成 `/admin?mode=store` → 加载 `store-admin.html` | **删除面板 + 同步删 `admin.js` 的 store 段（1462-1790）**，否则 `bindStoreActions()`(1767) 会因找不到元素报错 |
| `admin.html:299` placeholder 提到 `/store-images/` | — | 随封面改造一起更新文案 |

> `admin.js` 删完后只剩：会话、邀请码、待审批、用户列表、系统设置、3D/果园概览、仪表盘、审计日志。
> 这是**唯一**还需要的后台页面。

### 2.6 其它前端注意点

- `analyze.js` 的 `/api/citrus-disease-v2` → `/web/citrus-disease-v2`（响应字段**不变**，超集保留）。
- `analyze.js` 还依赖 Django 没有的 6 个字段（`is_citrus_leaf`/`severity`/`preventive_measures`/`treatment_suggestion`/`image_quality_warning`/`citrus_type`），这正是它必须留在超集层的原因。
- `orchard-3d.js` 的 `/user/orchard/overview` → `/web/orchard/overview`，**响应形状不变**（超集保留 `trees[].position/terrain_height/tag_serial_number/latest_sensor/latest_diagnosis`、`coordinate_range`、`weather.*`）。
- `store-support.js` 的 `/user/store/support` → `/web/support`，形状不变。

---

## 3. `/web/*` 命名空间设计

**原则**：`/web/**` 只承载「Django 从未建模」的能力 + 网页会话；**任何** Django 已有等价契约的功能都走 `/api/**`。绝不为了超集去改 compat 的响应。

| 类别 | 路径 | 复用 handler | 表变化 |
|---|---|---|---|
| **会话** | `POST /web/session/{login,register,logout,validate,me}` | 包一层 `compat::views_auth`（login/logout）+ 自研 `session::validate_token_handler`、`registration::register_handler`。**新增职责：login 时下发 HttpOnly cookie `session_token`，logout 时清除** | 读 `"user"` / `auth_token`；**不再用** `app_users`/`app_sessions` |
| **系统设置** | `GET /web/system-status`、`POST /web/admin/settings/{get,update}` | `handlers_core::system_status_api_handler`、`admin::get/update_system_settings_handler` | 保留 `app_system_settings` |
| **注册审批 + 邀请码** | `POST /web/admin/pending/{list,approve,reject}`、`/web/admin/invitations/{create,list}`、`/web/admin/set_admin`、`/web/admin/users/list` | `admin::pending::*`、`admin::management::*` | 保留 `app_pending_users`、`app_invitations`；**改读写 `"user"`**，并新增列 `"user".is_admin`（见下） |
| **客服会话** | `GET\|POST /web/support`、`GET\|POST /web/admin/support` | `store_workspace::{customer_messages,customer_send,admin_messages,admin_send}` | 保留 `store_support_messages` |
| **3D 沙盘** | `POST /web/orchard/overview`、`POST /web/admin/orchard/overview` | `admin::orchard_overview_public_handler`、`orchard_overview_handler` | 保留 `app_orchard_trees`、`app_tree_sensor_records` |
| **封面上传** | `POST /web/admin/store/products/{id}/cover` | `store::upload_store_cover_handler`（`user_routes/store.rs:327-420`） | 改写入 `citrus_product.cover_image_url`，值保持 `/store-images/<uuid>.jpg` |
| **后台仪表盘** | `POST /web/admin/dashboard/{stats,logs}`、`GET /web/admin/store/analytics`、`POST /web/admin/store/overview`、`POST /web/admin/store/orders`、`POST /web/admin/store/orders/status` | `admin::dashboard::*`、`store_workspace::analytics`、`store::store_overview_handler` | **改读写** `"order"`/`order_item`/`citrus_product`（替代 `store_orders`/`store_order_items`/`store_products`） |
| **识别超集** | `POST /web/citrus-disease-v2` | `handlers_ai::citrus_disease_advanced_handler` | 见下（识别富字段的归属） |

### 3.1 两个必须先定的数据设计点

**(a) `is_admin` 放哪？** Django 的 `"user"` 表只有 `role`（`farmer`/`buyer`），没有管理员概念。
网页的后台鉴权（`app-shell.js:75-77`、`admin.js`）依赖 `is_admin`。

- **采纳**：给契约表加**加法列** `ALTER TABLE "user" ADD COLUMN IF NOT EXISTS is_admin BOOLEAN NOT NULL DEFAULT FALSE`。
  加法列不进 DRF 序列化（Django 只序列化声明字段），**不破坏逐字兼容**；
  也保证日后 Django→PG 的数据迁移不会因缺列失败。
- **未选**：把管理员映射成第三个 `role` 值（`admin`）—— 会污染 Django 的 `role` 契约（App 可能按
  `role` 分支），且蓝本 `Role` 只有两个选项。

**(b) 识别富字段（17 列 vs 9 列）放哪？** `app_diagnosis_records` 有 17 列，Django
`disease_recognition_record` 只有 9 列（`id`/`user_id`/`image`/`disease_name`/`area`/`risk_level`/
`recognition_date`/`confidence`/`created_at`）。仪表盘统计依赖 `is_citrus_leaf`/`is_healthy`/
`predicted_class`/`severity`/`timestamp`（`admin/dashboard.rs:25,33,42,76,174`；`admin/orchard.rs:192,392`）。

- **采纳**：**保留 `app_diagnosis_records` 但改名为 `web_diagnosis_records`**，作为识别富字段的
  web 专表；识别超集写入时**同时**写 compat 的 `disease_recognition_record`（供 App 读）与
  `web_diagnosis_records`（供网页读）。
- **未选**：把 17 列全加到 `disease_recognition_record` —— 会让契约表承担非契约字段，
  与「契约表照抄 Django」的原则冲突，且日后对不上 Django 的迁移。

**(c) 温度湿度**：`admin/dashboard.rs:104` 读 `app_temperature_humidity` 取最新一条。
→ **直接改读 Django 的 `temperature_humidity_data`**（有 `timestamp`/`temperature`/`humidity`），
无需富字段，`app_temperature_humidity` 可退役。

---

## 4. 旧表退役计划

### 4.1 清单

| 表（张数） | 谁在引用（实证） | 处置 | 顺序 |
|---|---|---|---|
| `commerce_*`（7：`commerce_orchards/products/batches/batch_products/orders/order_items/order_status_logs`） | `src/user_routes/commerce.rs`（全文）、`src/server/handlers_commerce.rs`（全文）、`bootstrap/legacy_tables.rs:124-205` | **删除** | 与 D-2 同步，最早 |
| `store_products`/`store_orders`/`store_order_items`（3） | `handlers_store.rs:57`、`user_routes/store.rs`（多处）、`user_routes/store_workspace.rs:135,141`（analytics）、`bootstrap.rs:212`（demo seed）、`bootstrap/legacy_tables.rs:206-242` | **删除** | 待 §3 的仪表盘/封面改读 `citrus_product`/`"order"` 之后 |
| `app_users`（1） | `admin/dashboard.rs:51,59,190`、`admin/management.rs:35,179`、`admin/orchard.rs:45`、`admin/pending.rs:101,117`、`auth.rs:236`、`registration.rs:54,222`、`session.rs:43,112,149,299` | **删除**，全部改读写 `"user"`（+ 新列 `is_admin`） | 在 §3 会话/审批改造后 |
| `app_sessions`（1） | `server/shared.rs:239`、`user_routes/auth.rs:204`、`session.rs:130,205,213,261`、`bootstrap.rs:19-35`（迁移三步 + 索引） | **删除**，会话统一用 `auth_token` | 同上 |
| `app_tasks`（1） | `handlers_ai/persistence.rs:68`、`handlers_core/tasks.rs:35,107,162,172,280,378` | **删除**（前端零引用；App 侧契约是 `task` 表） | 与 D-1 同步 |
| `app_diagnosis_records`（1） | `admin/dashboard.rs:25,33,42,76,174`、`admin/orchard.rs:192,392`、`handlers_core/media.rs:34`、`records.rs:35`、`handlers_ai/persistence.rs:5`、`reports.rs:73` | **改名 `web_diagnosis_records` 保留**（§3.1b） | 随识别超集迁移 |
| `app_temperature_humidity`（1） | `admin/dashboard.rs:104`、`handlers_ai/diagnosis.rs:67`、`handlers_core/tasks.rs:343`、`temperature.rs:36,116` | **删除**，仪表盘改读 `temperature_humidity_data` | 与 D-1 同步 |

### 4.2 必须保留（**不要动**）

| 表 | 引用者 | 为什么留 |
|---|---|---|
| `app_orchard_trees` | `bootstrap.rs:77,111,140`、`admin/orchard.rs:175,375` | 3D 沙盘几何数据（Django `FruitTreeArchive` 无坐标/地形/`tag_serial_number`） |
| `app_tree_sensor_records` | `bootstrap.rs:126,158`、`admin/orchard.rs:181,381`、`handlers_core/health_point.rs:30`、`temperature.rs:93,137` | 沙盘传感器读数（Django 完全没有传感器表） |
| `app_system_settings` | `src/system_settings.rs:62,104,148` | 维护模式/注册开关/阈值 |
| `app_admin_audit_logs` | `src/system_settings.rs:182,200`、`admin/dashboard.rs:166` | 审计日志 |
| `app_invitations` | `registration.rs:160,216`、`admin/management.rs:102,145` | 邀请码 |
| `app_pending_users` | `registration.rs:73,108,133,190`、`admin/pending.rs:27,70,134,168`、`admin/dashboard.rs:67,182` | 注册审批 |
| `store_support_messages` | `user_routes/store_workspace.rs:39,72,89,112` | 客服会话 |

### 4.3 DDL 同步

`src/server/bootstrap/legacy_tables.rs` 目前包含 **22 张表 + 14 条索引**（其中要删 12 张表：
`commerce_*` 7 + `store_*` 3 + `app_users` + `app_sessions`）。
处理顺序：

1. 先在 `web_tables.rs`（新建）里加 `web_diagnosis_records` 的建表语句与 `"user".is_admin` 加法列。
2. 等 §3 的 handler 全部改完并验证通过后，**再**从 `legacy_tables.rs` 删掉那 12 张表的 DDL。
3. `src/server/bootstrap.rs:19-35` 的 `app_sessions` 迁移三步（`ADD COLUMN`/`UPDATE`/`ALTER`/`CREATE INDEX`）随之删除。
4. `src/server/bootstrap.rs:212` 的 `store_products` demo seed 删除；如需演示数据，改写进 `citrus_product`。

> **不要一步到位**：`bootstrap` 是每次启动都跑的，先删表会让还在引用它的 handler 在运行期报
> `relation does not exist`。**必须「先改引用、后删 DDL」**。

---

## 5. 分阶段与并行实施流

### 5.1 实施流划分（文件所有权互不重叠）

| 流 | owns（唯一可写） | 依赖 | 验收判据 |
|---|---|---|---|
| **S1 路由手术** | `src/server.rs`、`src/compat.rs`、新建 `src/web.rs`（含全部 `web/*` 子模块**空桩**） | 无 | `cargo check` 绿；`curl /api/login` 命中 compat；`/user/login` 返回 404；无重复路由 panic |
| **S2 会话与超集平移** | `src/web/session.rs`、`src/web/admin.rs`、`src/web/support.rs`、`src/web/orchard.rs` | S1（要 `src/web.rs` 的桩） | 9 个前端文件改完路径后，登录/登出/身份校验/后台 gating 人工走查通过 |
| **S3 商城收敛** | `src/web/store_admin.rs`、`src/web/dashboard.rs` | S1、S2 的 `"user".is_admin` 列 | 商品 CRUD（走 Django 农户端）+ 封面上传 + 订单管理 + 仪表盘走查通过；`citrus_product` 有数据 |
| **S4 前端** | 见下（再拆 3 组） | S1+S2+S3 的**路径冻结** | 每个页面走查通过，无 404 |
| **S5 旧表退役 + 死代码清理** | `src/server/bootstrap/{legacy_tables,web_tables}.rs`、`src/server/bootstrap.rs`、`src/server/handlers_{store,commerce}.rs`（删除）、`src/user_routes/mod.rs`（删 `router()`） | S2、S3 全部合并后 | 现网 `public` 上旧表已 drop；重启服务无报错；全序列回放仍 235/251 |

**S4 内部再拆 3 组（文件互不重叠）**：
- **S4a 会话路径替换**：`app-shell.js`、`index.js`、`analyze.js`、`orchard-3d.js`、`store-support.js` + 删 `commerce.js`
- **S4b 商城主链路**：`store.js`、`cart.js`（字段迁移 + 购物车上云 + 下单）
- **S4c 后台**：`admin.js`、`admin.html`、`store-admin.js`、`store-admin.html`（删重复面板/死段 + 后台路径）

### 5.2 并行约束

1. **S1 必须先合**：它是唯一改 `src/server.rs` 的流，其它流都要等 `/web` 命名空间就位。
2. **S2 与 S3 可并行**（不同文件），但都依赖 S1 建立的 `src/web.rs` 桩（与 compat 同样的模式：`src/web.rs` 声明 `mod` 并 merge 各子模块 `router()`）。
3. **S4 必须在 S1+S2+S3 之后**，因为前端要按最终路径改。
4. **S5 必须最后**，且顺序是「先改引用、后删 DDL」。
5. 沿用 W1 验证过的隔离手段：**每个流一个 git worktree** + 独立 `compat_*` scratch schema + 独立端口 + 只 `rustfmt` 自己的文件（`cargo +nightly fmt` 会重写别人的在写文件）。

### 5.3 切换门槛（W2 完成的定义）

1. `cargo +nightly fmt --check`、`cargo check --all-targets`、`cargo test -- --test-threads=1 compat` 全绿。
2. **契约回放不回归**：全序列仍 **≥235 pass / 4 fail（D12）/ 12 expected_deviation**，跑法见 `VERIFICATION.md`（注意：`capture → load_seed → replay` 必须 20 分钟内跑完，见 `DEVIATIONS.md` D16）。
3. **App 侧零改动**：把 App 的 baseUrl 从 Django 指向 Rust，走查全部页面与关键流程（登录/识别/商城下单/溯源/我的订单）。
4. **网页侧全流程走查**：登录 → 商城 → 购物车 → 下单 → 我的订单 → 识别 → 3D 沙盘 → 后台（商品/订单/审批/邀请码/设置/客服/仪表盘）。
5. **媒体通道实测**（§2.4）：App 拿到的识别图与商品封面 URL 能真的打开。
6. `DEVIATIONS.md` 新增项全部收口或明确登记。

---

## 6. 需要裁定或确认的点

1. **`/web/session/login` 与 `/api/login` 的关系**：本规划让网页走 `/web/session/login`（下发 HttpOnly cookie），App 走 `/api/login`（返回 Bearer）。两条路都调同一份 `auth.rs` 逻辑。
   **未选**：让网页也用 `/api/login` + `localStorage` 存 Bearer —— 否掉的理由是 token 暴露给 XSS，且 `is_admin` gating 会逼我们在契约响应里加字段，与「不污染 `/api/**`」冲突。**请确认这个取舍。**
2. **`admin.html` 的 store 面板是否真的删掉**：删了之后只有 `/admin?mode=store` → `store-admin.html` 一条路径能管商城。若有人直接收藏 `/app-content/admin` 会少功能。**建议删**（消除重复维护面）。
3. **旧购物车直接丢弃**（§2.2）需你确认可接受——这会让用户重新加购一次。
4. **`/api/versions` 级别的切换窗口**：契约层提升是**一次性**动作（`/api/login` 从自研变契约，语义从「cookie 会话」变「Bearer」）。若 App 与网页都在同一时间点切，需要一个短暂窗口。**建议**：S1 先只 `PROMOTE` 到 `/api/v2/**` 做一次灰度？——见下条。
5. **是否要灰度**：我**不建议**为提升做第二套前缀（会长期留下两套 `/api/`）。更简单的做法是：S1 合并到一个维护窗口，网页与 App 同时切，出问题用 `git revert` 单次回滚。**请确认接受一次性切换。**
6. **`D2`（`/api/generate/fertilization-plan` 蓝本恒 500）** 仍待 App 侧确认期望形状（`DEVIATIONS.md` 已登记）。

---

## 附：本次规划的实证命令（可复现）

```powershell
# 路由清点
Select-String -Path db\src\server.rs -Pattern '\.route\(|\.nest\('
Select-String -Path db\src\user_routes\mod.rs -Pattern '\.route\('

# 前端调用清点
Select-String -Path db\static\*.js -Pattern 'fetch\(|apiRequest|/user/|/api/'

# 「自研 /api/* 是否被前端引用」——结果：零命中
Select-String -Path db\static\*.js,db\static\*.html -Pattern 'health-point|citrus/analyze|/api/validate|/api/login|/api/register|/api/logout|/api/user|/api/me|/api/home|/api/tasks|/api/diagnose'

# 死代码证据
Select-String -Path db\static\*.html -Pattern '<script src='        # commerce.js 不在其中
Select-String -Path db\static\admin.html -Pattern 'commerce'       # 零命中
Select-String -Path db\static\admin.js -Pattern 'bindCommerceActions\(|refreshCommerceData\('  # 只有定义

# 表引用清点
Select-String -Path db\src\*.rs -Recurse -Pattern 'store_products|commerce_|app_users|app_sessions|app_tasks|app_diagnosis_records|app_temperature_humidity'
```

---

## S1 执行记录（路由手术 + 两个数据前置）

> 作者：W2-S1。依据 `W2_PLAN.md` §1，**按父 agent 的两条修正执行**（见下）。
> 验收用**独占** schema `compat_s1` / 端口 11600，避免与并发流抢 `compat_verify` / 11500。

### 1. 与 §1.6 的两处有意偏离

| 偏离 | 规划原案 | 实际执行 | 原因 |
|---|---|---|---|
| `.nest("/compat")` | 删掉，只 merge 到根 | **保留双挂载**：`.merge(crate::compat::router())` **和** `.nest("/compat", crate::compat::router())` | 整套 L2 回归网（`replay_diff.py --base-url .../compat`）依赖 `/compat`，删掉等于把回归网拆了。两者路径不同，不会重复注册 |
| DELETE 范围 | D-1..D-5 全删（36 条） | **只删 D-1（21 条）** | D-2~D-5 网页现在还在用；先删会让服务在整个 W2 期间不可用。留到 S4 前端切完、S5 统一退役 |

### 2. 实际改动

**`src/server.rs`**
- 删 21 条自研 `/api/*` 与页面级 API（D-1）：`register`/`login`/`logout`/`validate`/`user`/`home`/`growth-tracking`/`diagnose`/`temperature-humidity`/`citrus-disease`/`recognition-records`/`disease-treatment`/`tasks`(×1 + add + complete + generate×2)/`health-point`/`generate`/`generate/fertilization-plan`/`citrus/analyze`
- 新增 `.merge(crate::compat::router())`（正式路径 `/api/**` + `/api/v1/**`）与 `.nest("/web", crate::web::router())`
- 保留：页面外壳 28 条、`/api/system-status`（S2 迁 `/web/system-status`）、`/api/citrus-disease-v2`（S2 迁 `/web/`）、`/api/commerce/*` 与 `/api/store/products`、`/user` 整棵树、`/compat`
- 死代码治理：给 `handlers_ai` / `handlers_core` / `shared` 加模块级 `#[allow(dead_code)]`（60+ 告警 → 17）
  **S5 删代码时必须把这三个属性一起删掉**，否则这三个模块里日后的真死代码会隐身

**新增 `src/web.rs` + `src/web/{session,admin,support,orchard,store_admin,dashboard}.rs`**
- 六个空桩，各自 `pub(crate) fn router() -> Router<AppState>`；每个文件头部注释写明它负责的 `/web/*` 路径清单
- 27 条 MOVE 的分工：session 5 / admin 10（含 `/web/system-status`）/ support 2 / orchard 3（含 `/web/citrus-disease-v2`）/ store_admin 5 / dashboard 2
- **要加路由就往自己文件里加，不要改 `src/web.rs`**

**`src/main.rs`**：`mod web;`

### 3. 两个数据前置（父 agent 追加项）

`src/server/bootstrap.rs`：
- `ALTER TABLE "user" ADD COLUMN IF NOT EXISTS is_admin BOOLEAN NOT NULL DEFAULT FALSE`
  加法列，不进 DRF 序列化（`AuthUser::payload()` 未改）；**没有**引入第三个 `role` 值。
  管理员标记方式：`UPDATE "user" SET is_admin = TRUE WHERE username = '<账号>'`
- 新增 `src/server/bootstrap/web_tables.rs`：`web_diagnosis_records`（17 列，列名与类型照抄
  `app_diagnosis_records`）+ `idx_web_diag_user_time`；在契约表之后注册

**顺带修掉的必要缺陷**：`scripts/pg_env.ps1` 的 DDL 抽取器只认 7 个常量，**抽不到 `web_tables`
与加法 ALTER**，会让镜像 schema 与 `init_database` 脱节（追加项根本无法验收）。已扩展：
- `$DdlOrder` 加 `web_tables.rs::DDL`
- 新增「从 `bootstrap.rs` 现场抽取 `sqlx::query(r#"ALTER TABLE ... ADD COLUMN IF NOT EXISTS ..."#)`」
- 破坏性守卫从「一律拒绝 ALTER」改为「**只放行幂等的加法列形态**」，其余 ALTER 仍拒绝
- `$ExePath` 改为尊重 `CARGO_TARGET_DIR`（并行开发要独占 target，共用 target 会 `LNK1104`）

### 4. 验收结果（全部实测）

| 项 | 结果 |
|---|---|
| `cargo +nightly fmt --check` | exit 0 |
| `cargo check --all-targets` | exit 0（17 告警） |
| `cargo test -- --test-threads=1 compat` | **125 passed / 0 failed** |
| `-Reset` + `-Apply compat_s1` | **105 条语句 / 53 张表**（22 历史 + 30 契约 + 1 web） |
| `"user".is_admin` | `boolean NOT NULL default=false` ✓ |
| `web_diagnosis_records` | 17 列 + 索引建成 ✓ |
| **全序列回放**（`.../compat`） | **239 pass / 0 fail / 12 expected_deviation / 0 transport_error / 0 tz 漂移** |
| 逐域 | auth 27/27、core 40+2dev、commerce 81/81、orchard_trace 64+1dev、agent 27+9dev |
| `/api/login`、`/api/v1/auth/login` | 200 `application/json`，**契约形状**（键序 `code,message,data,timestamp`） |
| `/compat/api/login` | 200，回归网完好 |

> 基线 235 pass / 4 fail → 本次 **239 / 0**。那 4 条 fail 的消失**不是**路由手术的功劳，
> 是并发流把 D12（夹具顺序不确定）修掉了。路由手术本身**没有引入任何新失败**。

### 5. 需要后续流注意

1. **`seed_approval_id` 捕获键从未被观察到**（`replay_diff.py` 已显式告警）：相关用例按录制值发出，
   结果不可信。属夹具侧问题，归夹具负责人 / S5。
2. `src/user_routes/mod.rs::router()` 仍在用（`/user` 树保留），其整体删除属 S5。
3. `handlers_store.rs` / `handlers_commerce.rs` 仍是**活代码**（`/api/store/products`、
   `/api/commerce/*` 还在服务），S5 一并删。
4. `web_diagnosis_records` 的**双写逻辑尚未实现**（识别时同时写契约表与 web 表）——S2/S3 的活，
   S1 只建表。
5. 并行开发的环境约定：`$env:CARGO_TARGET_DIR` 指向**被忽略的 `target/` 之内**的子目录
   （如 `db\target\s2`）。`target-s1` 这种仓库根下的目录**不在 `.gitignore` 里**，会污染 `git status`。
