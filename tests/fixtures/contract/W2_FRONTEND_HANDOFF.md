# S4-3 前端交接规格（`admin.js` / `admin.html` / `store-admin.js` / `store-admin.html`）

> 作者：W2-S4-2。**只写文档，未改任何代码。**
> 所有结论以 `db/static/**` 实际代码为准，给出「文件:行」。取证脚本：`D:\githubs\db_work\.s43_{analyze.py,analyze2.py,farmer.py,fields.py}`（仓库外）。

---

## 0. 先说结论：有一个**阻塞级**发现，会改变 S4-3 的做法

**规划里「后台商品管理改打 Django 农户端接口」这条路走不通。** 两条实测证据（都在 `orchard_trace.json` 夹具里）：

| 证据 | 内容 |
|---|---|
| `farmer_products_buyer_403`、`farmer_product_put_buyer_403` | `/api/v1/farmer/*` 全部 10 条接口对 buyer 返回 **403 `该接口仅限果农使用`** → 它是 `IsFarmer` |
| `farmer_product_patch_foreign_404` | `farmer_xunwu` 打 `farmer_xinfeng` 的商品 → **404**（按 `seller_id` 隔离，只能管自己的） |

而**后台商品管理要管的是全平台商品，且操作者身份是 `is_admin`** —— `is_admin` 是 `"user"` 的加法列，与 `role` **正交**：一个管理员完全可能是 `role=buyer`。

所以：
- 管理员若 `role=buyer` → 调 farmer 接口 **403**，后台商品管理直接不可用；
- 管理员即使 `role=farmer` → 也**只能看到自己名下的商品**，看不到其它果农的，后台失去意义。

**建议：为后台商品管理保留 `/web/admin/store/products*` 超集接口**（管理端口径，跨 `seller_id` 读写 `citrus_product`），不要指向 `/api/v1/farmer/products`。
这条与 `W2_PLAN.md` §2.3 的原文冲突，**请主线裁定**。裁定前 S4-3 不要动商品 CRUD 的路径，否则会引入一个「改完看起来对、但管理员一登录就 403」的坏入口。

---

## 1. 逐调用替换表

### 1.1 reader 的三种行为（决定每个端点必须返回什么形状）

| reader | 定义位置 | 行为 | 对端点的要求 |
|---|---|---|---|
| `admin.js::postJson` | `admin.js:110-128` | 返回 `{ok, data: <整个 body>}`；消费方再 `unwrapApiPayload(resp.data)` | **必须信封**（见下） |
| `admin.js::unwrapApiPayload` | `admin.js:10-15` | `payload.data ?? payload`（`data == null` 时**回落成整个 payload**） | 成功时 **`data` 不能是 null** |
| `admin.js::readErrorMessage` | `admin.js:17-19` | `payload.error \|\| payload.message \|\| unwrapApiPayload(payload)?.error \|\| fallback` | 错误体必须有**顶层 `message`** |
| `admin.js::storeRequest` | `admin.js:1469-1490` | 抛 `Error(readErrorMessage(...))`，成功 `return unwrapApiPayload(data)` | 同 `postJson` |
| `store-admin.js::request` | `store-admin.js:40-50` | `b.data ?? b`；错误 `b.message \|\| b.error` | **裸 JSON 或信封都吃** |
| 裸 `fetch`（封面上传） | `admin.js:1650`、`store-admin.js:327` | 读 `b.message` | 错误体要有顶层 `message` |

> **一句话**：`admin.js` 的接口**必须信封 + 成功时 `data` 非 null**；`store-admin.js` 两种都吃；
> `store-support.js`（不是你的文件）必须**裸 JSON**。

### 1.2 `admin.js` —— 存活调用（14 条管理端 + 1 会话）

| 行 | 方法 | 旧路径 | **新路径** | reader | 消费方读的字段 |
|---|---|---|---|---|---|
| 87 | POST | `/user/logout` | `/web/session/logout` | 裸 fetch | — |
| 131 | POST | `/user/validate` | `/web/session/validate` | `postJson` + `unwrapApiPayload` | `valid`、`username`、`is_admin`、`maintenance_mode`、`open_registration`（`admin.js:132` 起；**端点必须是扁平体**，见 §6） |
| 146 | POST | `/user/admin/invitations/create` | `/web/admin/invitations/create` | `postJson` | `code`、`expires_at` |
| 165 | POST | `/user/admin/set_admin` | `/web/admin/set_admin` | `postJson` | （只看 `ok`） |
| 255 | POST | `/user/admin/pending/list` | `/web/admin/pending/list` | `postJson` | `pending_users`、`invitations` |
| 266 | POST | `/user/admin/invitations/list` | `/web/admin/invitations/list` | `postJson` | `invitations`、`users` |
| 277 | POST | `/user/admin/users/list` | `/web/admin/users/list` | `postJson` | `users` |
| 288 | POST | `/user/admin/pending/approve` | `/web/admin/pending/approve` | `postJson` | （只看 `ok`） |
| 300 | POST | `/user/admin/pending/reject` | `/web/admin/pending/reject` | `postJson` | （只看 `ok`） |
| 887 | POST | `/user/admin/orchard/overview` | `/web/admin/orchard/overview` | `postJson` | `trees`、`legend`、`summary`、`coordinate_range` |
| 929 | POST | `/user/admin/dashboard/stats` | `/web/admin/dashboard/stats` | `postJson` | `totals`、`environment`、`daily_counts`（S3 已实现，实测通过） |
| 1011 | POST | `/user/admin/dashboard/logs` | `/web/admin/dashboard/logs` | `postJson` | `logs`（S3 已实现，实测通过） |
| 1063 | POST | `/user/admin/settings/get` | `/web/admin/settings/get` | `postJson` | `settings` |
| 1079 | POST | `/user/admin/settings/update` | `/web/admin/settings/update` | `postJson` | `settings` |

### 1.3 `admin.js` —— 商城段（**5 条存活的要重定向，其余整段删除**）

| 行 | 方法 | 旧路径 | **新路径** | 说明 |
|---|---|---|---|---|
| 1505 | POST | `/user/admin/store/overview` | `/web/admin/store/overview` | S3 已实现；`renderStoreOverview(data)` 读 `product_count`/`order_count`/`gross_amount_cents`/`pending_order_count` |
| 1568 | GET | `/user/admin/store/products` | **⚠️ 待裁定**（见 §0） | 读 `products[]`；字段是旧命名（见 §4） |
| 1617 | PUT | `/user/admin/store/products/${editingId}` | **⚠️ 待裁定** | 请求体是旧命名 |
| 1620 | POST | `/user/admin/store/products` | **⚠️ 待裁定** | 请求体是旧命名 |
| 1675 | POST | `/user/admin/store/products/${id}/toggle` | **⚠️ Django 无对应** | 需 `/web/admin/store/products/{id}/toggle` 超集，或删掉该功能 |
| 1650 | POST | `/user/admin/store/products/${id}/cover` | `/web/admin/store/products/{id}/cover` | S3 已实现；注意**路径参数改 UUID** |
| 1726 | POST | `/user/admin/store/orders` | `/web/admin/store/orders` | S3 已实现；读 `orders[]` |
| 1733 | POST | `/user/admin/store/orders/status` | `/web/admin/store/orders/status` | S3 已实现 |

**删除的（commerce 段，全死）**：`1293`、`1302`、`1308`、`1314`、`1320`、`1417` —— 见 §3.2。

### 1.4 `store-admin.js` —— 全部调用（7 条）

| 行 | 方法 | 旧路径 | **新路径** | reader | 读的字段 |
|---|---|---|---|---|---|
| 79 | GET | `/user/admin/store/analytics?days=N` | `/web/admin/store/analytics` | `request`（两吃） | `summary`、`trend`、`statuses`、`products`（S3 已实现，实测通过） |
| 113 | GET | `/user/admin/store/products` | **⚠️ 待裁定**（§0） | `request` | `products[]` |
| 179 | POST | `/user/admin/store/orders` | `/web/admin/store/orders` | `request` | `orders[]` |
| 188 | GET | `/user/admin/store/support?username=X` | `/web/admin/support` | `request` | `messages[].{is_staff,created_at,content}` |
| 203 | GET | `/user/admin/store/support` | `/web/admin/support` | `request` | `conversations[].{username,is_staff,content}` |
| 327 | POST | `/user/admin/store/products/{id}/cover` | `/web/admin/store/products/{id}/cover` | 裸 fetch | `message`（仅错误路径） |
| 348 | POST | `/user/admin/store/orders/status` | `/web/admin/store/orders/status` | `request` | — |
| 375 | POST | `/user/admin/store/support` | `/web/admin/support` | `request` | — |
| 385 | POST | `/user/validate` | `/web/session/validate` | `request`（`b.data ?? b`，能容忍扁平体） | `valid`、`is_admin` |

> `store-admin.js` 还有**商品表单提交**（`store-admin.js:255-305`）走同一条 `/user/admin/store/products`，请求体字段见 §4。

### 1.5 `admin.html` / `store-admin.html` 里的后端调用

**两个 HTML 文件里都没有任何 `fetch`/inline XHR 调用**，也没有 inline handler 触发后端（唯一的 inline handler 是 `admin.html` 的 `onclick="logout()"` 与 `onclick="copyInviteCode()"`，都是本地函数）。

但 **`store-admin.html` 把字段名写进了表单控件的 `name` 属性**，而 `store-admin.js` 用 `f.get("stock_quantity")` 之类读取 —— 这属于**跨文件耦合的字段名**，改名时必须两边同时改：

```
store-admin.html:74   name="unit_label"        ←→ store-admin.js:271,296
store-admin.html:87   name="stock_quantity"    ←→ store-admin.js:260,273,297
store-admin.html:95   name="cover_image"       ←→ store-admin.js:275,299
```

---

## 2. 重复 CRUD 复核（决定 S4-3 该删什么）

**结论：`admin.html` 的商城段是重复实现，应删除；保留 `store-admin.*`。**

证据链（四条互相印证）：

1. **shell 主动把商城管理路由到独立页**：`app-shell.js:31-34` 把 `/store-admin` 重写成 `/admin?mode=store`；`app-shell.js:110-116` 再由 `mode === "store"` 决定加载 `/app-content/store-admin`（即 `store-admin.html`）。而 `/admin`（无 `mode`）才加载 `admin.html`。
2. **shell 把唯一能选中该段的 URL 也重写走了**：`app-shell.js:35-41` 明确把 `/admin?section=store` 改成 `?mode=store` —— 即 shell 已经把「商城」这件事**认定为独立页**。
3. **admin 页的导航里没有商城入口**：`admin.html:39-54` 的 `data-admin-section` 只有 `orchard`/`access`/`overview`/`audit`/`settings`，**没有 `store`**；虽然 `admin.js:322` 的 `ADMIN_SECTIONS` 里留着 `"store"`。
4. **admin 页自己就链到独立页**：`admin.html:45` 有 `<a href="/store-admin" ...>`。

而 `admin.html:216-238` 确实存在一整块 `data-admin-page="store"`（含 `storeOverview`、`storeProductForm`、`storeOrdersWrap`、`storeCoverFile` 等），且 `admin.js:1767` 的 `bindStoreActions()` **是活的**（会被调用）—— 所以它不是死代码，而是**一个只能靠手工输入 `/app-content/admin?section=store` 才能看到的重复面板**。

**建议**：删除 `admin.html` 的商城段（`data-admin-page="store"` 整块）+ `admin.js` 的商城段（`storeRequest`/`storeMoney`/`renderStore*/loadStore*/submitStoreProduct/bindStoreActions`，约 `1469-1794`）+ `ADMIN_SECTIONS` 里的 `"store"`（`admin.js:322`）。**同时**把 §1.3 里那 5 条存活调用的改点全部转由 `store-admin.js` 承担。

> ⚠️ 但 §0 的阻塞项必须先裁定：删了 `admin.js` 的商城段之后，「管理端商品管理」就只剩 `store-admin.js` 一处，而它的商品 CRUD 正卡在没有可用接口上。

---

## 3. 死代码清单（逐条给证据）

### 3.1 `commerce.js` —— **整个文件是死的**

- **没有任何 HTML 加载它**：`static/*.html` 的 `<script>` 标签全集是 `store.js`、`store-support.js`、`analyze.js`、`index.js`、`store-admin.js`、`app-shell.js`、`cart.js`、`orchard-3d.js`、`admin.js` —— **没有 `commerce.js`**。
- **没有任何 HTML 含 `commerce` 字样**（全文件 grep：除 `app-shell.js:30` 的 `/commerce → /store` 路由重写外，`commerce` 只出现在 `admin.js` 自己的函数名里）。

→ 275 行死文件，S5 可删。它调用的 `/user/commerce/orders`、`/api/commerce/storefront` 也随之无人使用。

### 3.2 `admin.js` 的 commerce 段（约 `1123-1467`）—— **全死**

机械统计（`参考 .s43_analyze2.py`，跨 `*.js` + `*.html` 统计裸标识符）：

| 函数 | 行 | 零引用 |
|---|---|---|
| `refreshCommerceData` | 1325 | ✅ 全仓零引用 |
| `bindCommerceActions` | 1430 | ✅ 全仓零引用 |

**其余 9 个 `loadCommerce*`/`renderCommerce*`/`createCommerce*` 的唯一调用者就是这两个**，所以它们跟着一起死。

第二重证据：`bindCommerceActions` 要绑 `commerceOrchardForm`/`commerceProductForm`/`commerceBatchForm`（`admin.js:1431-1433`），而**全仓 `*.html` 里不含 `commerce` 字样**——这些表单**根本不存在**，它一旦被调用就会在 `null.addEventListener` 上抛异常。这也解释了它为什么从未被接上。

→ 与 W2-P 早期判断一致（它说 `admin.js:1123-1451` / `bindCommerceActions(1430)` / `refreshCommerceData(1325)` 无调用点）。实测范围略宽：**`1123-1467`**。

### 3.3 修正：v1 分析器的假阳性

第一版用 `name\s*\(` 计调用点，把**作为回调传入**的函数误判为死代码 —— 例如 `submitStoreProduct` 是通过
`$("storeProductForm").addEventListener("submit", submitStoreProduct)`（`admin.js:1746`）传入的，**没有括号**。
修正后用裸标识符跨文件统计，`admin.js` 的零引用函数只剩上面 2 个。**不要采信 v1 的 10 个名单。**

### 3.4 不是死代码、但需要 S4-3 注意的

- `admin.js:1745 bindStoreActions` —— 活的（`admin.js:1767` 调用），绑的是 `admin.html` 的商城段。
- `admin.js:79 copyInviteCode`、`admin.js:85 logout` —— v1 曾误报为死；实际由 `admin.html` 的 inline handler 引用（`onclick="copyInviteCode()"`、`onclick="logout()"`）。

---

## 4. 字段改名：**每一处出现位置**

> 这是「显示 undefined」类 bug 的唯一防法。契约层字段是 `price`（**字符串**）/`stock`/`unit`/`cover_image_url`，商品的 `id` 也从整数变成 **UUID**。

### 4.1 `admin.js`（18 处）

```
L1228  product.sku            ← 契约表**没有 sku 列**（见下）      commerce 段（将删）
L1228  product.unit_label                                          commerce 段（将删）
L1229  product.price_cents                                         commerce 段（将删）
L1238  product.price_cents                                         commerce 段（将删）
L1365  unit_label: ...  (请求体)                                    commerce 段（将删）
L1366  price_cents: ... (请求体)                                    commerce 段（将删）
L1525  product.cover_image                                         → cover_image_url
L1526  product.cover_image                                         → cover_image_url
L1527  product.cover_image                                         → cover_image_url
L1531  product.cover_image                                         → cover_image_url
L1535  product.unit_label                                          → unit
L1536  storeMoney(product.price_cents)                             → money(product.price)（且金额单位变了！）
L1537  product.stock_quantity                                      → stock
L1577  unit_label: ... (请求体)                                     → unit
L1578  price_cents: ... (请求体)                                    → price
L1579  stock_quantity: ... (请求体)                                 → stock
L1581  cover_image: ... (请求体)                                    → cover_image_url
L1600  product.unit_label                                          → unit
L1601  product.price_cents / 100                                   → product.price（字符串，不需要 /100）
L1602  product.stock_quantity                                      → stock
L1604  product.cover_image                                         → cover_image_url
```

### 4.2 `store-admin.js`（10 处）

```
L130   coverImageSrc(p.cover_image)      → cover_image_url
L139   p.sku / p.unit_label              → ⚠️ sku 列不存在；unit_label → unit
L140   money(p.price_cents)              → money(p.price)（单位变了）
L141   p.stock_quantity                  → p.stock
L260   stock = Number(f.get("stock_quantity"))   → stock（同时改 store-admin.html:87）
L271   unit_label: f.get("unit_label")           → unit（同时改 store-admin.html:74）
L272   price_cents: price                        → price
L273   stock_quantity: stock                     → stock
L275   cover_image: f.get("cover_image")          → cover_image_url（同时改 store-admin.html:95）
L296   "unit_label" (回填字段名)                  → unit
L297   "stock_quantity"                          → stock
L299   "cover_image"                             → cover_image_url
L302   p.price_cents / 100                       → p.price（字符串，不需要 /100）
```

### 4.3 `store-admin.html`（3 处表单 `name`）

```
L74    name="unit_label"      → name="unit"
L87    name="stock_quantity"  → name="stock"
L95    name="cover_image"     → name="cover_image_url"
```

> `store-admin.js` 用 `f.get(...)`/`f.elements.namedItem(...)` **按 name 取值**，所以 HTML 的 `name` 与 JS 的字符串**必须同时改**，改一边就是 undefined。

### 4.4 `commerce.js`（4 处）

全部在死文件里（§3.1），**直接删文件即可**：`L114`、`L115`、`L149`、`L150`。

### 4.5 ⚠️ 金额单位的坑

旧代码是「**分**」(`price_cents / 100`、`storeMoney(cents)`)，契约是 **`NUMERIC` 字符串**（`"168.00"`）。
`admin.js:1492 storeMoney` 与 `store-admin.js` 的 `money` 都是 `(Number(x)/100).toFixed(2)` —— **直接套到契约字符串上会显示成 ¥1.68**
（168.00/100）。必须改成 `Number(x).toFixed(2)`，或统一换一个 `moneyFromDecimal()`。**这是 S4-3 最可能出的 100 倍误差。**

### 4.6 ⚠️ `sku` 列不存在

`citrus_product` **没有 `sku` 字段**（`admin.js:1228`、`store-admin.js:139` 都在读它）。
契约里有 `sku_type`/`sku_type_display`，但语义是「规格类型」（试吃装/家庭装/企业装…），**不是 SKU 编码**，不能顶替。
S3 已在 `W2_PLAN.md` 登记过这个缺口，S4-3 需要产品侧决定：去掉 SKU 展示，或由超集另存。

---

## 5. `/web/system-status` 与 `/web/citrus-disease-v2` 的 reader

虽然 `admin.rs` / S2 那边的 handler 还没落地，**消费方的读法现在就能定死**：

| 端点 | 消费方 | 读法（证据） | 因此端点必须返回 |
|---|---|---|---|
| `/web/system-status`（旧 `/api/system-status`） | `index.js:141-152` | `const payload = await res.json(); applySystemStatus(unwrapApiPayload(payload));` | `unwrapApiPayload` 是 `data ?? payload` → **信封（`data` 非 null）或扁平体都行**；但读的是 `applySystemStatus` 的入参，所以 **`maintenance_mode` / `open_registration` 必须在最外层**（`index.js:175` 读 `state.systemStatus.maintenance_mode`） |
| `/web/citrus-disease-v2`（旧 `/api/citrus-disease-v2`） | `analyze.js:196-234` | `const data = await resp.json(); if (!resp.ok \|\| data.code !== 200) throw ...; const result = data.data \|\| {};` 然后读 `result.predicted_class` / `is_healthy` / `disease_name` / `severity` / `confidence` / `treatment_suggestion` / `preventive_measures` / `image_quality_warning` / `stage` | **必须信封**，且 **`code` 必须等于 200**、`data` 是对象 —— 扁平体或 `data: null` 都会让它 `throw "识别失败"` |
| `/web/session/validate` | `index.js:189-205` | **直接** `applySystemStatus(data)`（**不 unwrap**）并读 `data.valid` | **必须扁平体且恒 200**（S2 已如此实现，`admin.js:131` / `store-admin.js:385` / `app-shell.js:63` 也都能吃扁平体） |

> 另：`app-shell.js:63-71` 读 `/user/validate`（→ `/web/session/validate`）用的是 `body.data ?? body`，**两种都吃**；
> 但它 `if (!response.ok && response.status !== 401) throw` —— 所以该端点返回 401 时不能是 500 或 403。

---

## 6. S5 可删清单（前端切换完成后会消失的旧路径）

供 S5 删路由时对账。**来源 = 上述四个文件 + 其它前端文件的现存调用**；S4-3 改完后这些就没人调了。

### 6.1 `admin.js` / `admin.html` 侧

```
/user/logout                              （→ /web/session/logout）
/user/validate                            （→ /web/session/validate）
/user/admin/invitations/create            （→ /web/admin/invitations/create）
/user/admin/set_admin                     （→ /web/admin/set_admin）
/user/admin/pending/list|approve|reject   （→ /web/admin/pending/*）
/user/admin/invitations/list              （→ /web/admin/invitations/list）
/user/admin/users/list                    （→ /web/admin/users/list）
/user/admin/orchard/overview              （→ /web/admin/orchard/overview）
/user/admin/dashboard/stats|logs          （→ /web/admin/dashboard/*）
/user/admin/settings/get|update           （→ /web/admin/settings/*）
/user/admin/store/overview|orders|orders/status|analytics|products/*/cover
                                          （→ /web/admin/store/*）
/user/admin/store/products[/*]            （→ 待裁定，见 §0）
/user/admin/store/products/*/toggle       （→ Django 无对应，待裁定）
/user/admin/commerce/*                    （全死，随 commerce 段一起删）
```

### 6.2 `store-admin.js` / `store-admin.html` 侧

```
/user/admin/store/analytics|products|orders|orders/status|support|products/*/cover
/user/validate
```

### 6.3 其它文件（不是 S4-3 的活，但一并列出便于 S5 一次删干净）

```
index.js        /api/system-status → /web/system-status；/user/{validate,login,register,logout} → /web/session/*
analyze.js      /api/citrus-disease-v2 → /web/citrus-disease-v2；/user/{validate,logout} → /web/session/*
orchard-3d.js   /user/validate → /web/session/validate；/user/orchard/overview → /web/orchard/overview；/user/logout
app-shell.js    /user/validate、/user/logout → /web/session/*
store-support.js /user/store/support → /web/support
commerce.js     /user/commerce/orders、/api/commerce/storefront —— **整文件删除**
```

> 注意 `store.js` / `cart.js` **已经改完**（S4-2），它们不再出现在可删清单里。

---

## 7. 给 S4-3 的建议顺序

1. **先要 §0 的裁定**（商品 CRUD 用超集还是 farmer 接口）。这条不定，商品管理做不了。
2. 先删死代码（§3.1 `commerce.js`、§3.2 `admin.js:1123-1467`）—— 少一大片改动面，且零风险。
3. 再删 `admin.html` 的重复商城段 + `admin.js` 的商城段（§2），把这部分职责收敛到 `store-admin.*`。
4. 然后按 §1 的表做路径替换；**金额单位（§4.5）与 `sku`（§4.6）这两个坑单独过一遍**。
5. 改完对四个文件跑 `node --check`（HTML 没有编译门槛，`store-admin.html` 的 `name` 改动靠 §4.3 的清单核对）。
6. 我这边（S4-2）没跑过真机浏览器 —— S4-3 收尾时**整个前端三组一起做一次人工走查**，重点看：管理端五个 section 的切换、商城管理的商品表单、封面上传预览、订单状态保存。

---

## 8. 追加（来自 `d28eb074` 在 `admin.rs` 上固化的两条约束）

### 8.1 ⚠️ 读取类端点**不能**挂 `Json` extractor

`admin.js:110-118` 的 `postJson(url)` 在**没有 body 时既不发送 body、也不发送 `Content-Type`**：

```js
async function postJson(url, body) {
  const withBody = body !== undefined;
  const res = await fetch(url, {
    method: "POST",
    headers: requestHeaders(withBody),   // withBody=false → 不带 Content-Type
    body: withBody ? JSON.stringify(body) : undefined,
  });
```

而 axum 的 `Json<T>` extractor 要求 `Content-Type: application/json`，否则**直接 415/400 拒掉**，
前端表现为「拿不到数据」而不是报错——**典型静默失败**。

**受影响的是「无请求体的 POST 读取端点」**：`admin.js:929`（dashboard/stats）、`admin.js:1011`（dashboard/logs）、
`admin.js:1063`（settings/get）、`admin.js:255`（pending/list）、`admin.js:266`（invitations/list）、
`admin.js:277`（users/list）、`admin.js:1505`（store/overview）、`admin.js:1726`（store/orders）、
`store-admin.js:179`（orders）。

**这些 handler 只能用 `State` + `HeaderMap`，绝不能加 `Json`。**（`admin.rs` 正是踩了这个才被卡住。）

> 我核对过：`store_admin.rs` 的 `products_list_api` / `product_detail_api` / `product_delete_api` /
> `product_toggle_api` / `overview_api` / `orders_api` / `analytics_api` **都没有 `Json`**；
> 只有 `product_create_api` / `product_update_api` / `order_status_api` 有，而它们对应的前端调用
> （`admin.js:1620`/`admin.js:1617`、`store-admin.js:277`、`admin.js:1733`）**都真的带 body**，所以安全。

### 8.2 ⚠️ 时间单位：**两套约定并存，不能一刀切**

转来的结论是「时间一律输出秒」，依据是 `admin.js:73` 的 `formatDate(unixSeconds)` 做 `new Date(unixSeconds*1000)`，
且 `> 4102444800` 渲染成「永久有效」（邀请码 `ttl=0` 的哨兵语义依赖它）。

**但我实测发现 `admin.js` 里两套约定并存，这条只对 `admin.rs` 的端点成立：**

| reader | 位置 | 单位 | 用于 |
|---|---|---|---|
| `formatDate(unixSeconds)` | `admin.js:73` | **秒** | 邀请码 `expires_at`（`admin.js:157,187`）、待审批/用户列表 `created_at`（`admin.js:223,244`）—— **全是 `admin.rs` 的端点** |
| `new Date(Number(x))` | `admin.js:1700`、`store-admin.js:164` | **毫秒** | 商城订单 `created_at` —— **`store_admin.rs` 的端点** |
| `parseTime(t)` | `admin.js:1023-1029` | **两种都吃**（`t > 1e12 ? t : t*1000`） | 审计日志 `created_at`（`dashboard.rs`） |

**结论**：`store_admin.rs` 的订单 `created_at` **必须输出毫秒**；若按「一律输出秒」改，
`admin.js:1700` 与 `store-admin.js:164` 会显示成 **1970 年**。
`admin.rs` 的邀请码 `expires_at` 与用户 `created_at` **必须输出秒**。两边都不能照抄对方。

### 8.3 追加：整数 id 比较的陷阱（UUID 下静默失效）

`store-admin.js:290` 是 `products.find((p) => p.id === Number(b.dataset.edit))` —— **整数比较**。
商品 id 变成 UUID 后 `Number("6cd1e05d-…")` 是 `NaN`，**永远匹配不上**，表现为「点编辑没反应」（不报错、不抛异常）。
`admin.js` 的商品/订单编辑路径要一并检查。**改为字符串比较**（`String(p.id) === b.dataset.edit`）。

### 8.4 共享 `compat_test` 的种子漂移（我踩过，值得记）

别的流重录契约夹具后 `seed.json` 变了，而**共享 schema `compat_test` 里还是旧种子**
→ `cargo test -- --test-threads=1 compat` 报 **9 条失败**（**不是代码回归**）。
重灌即复原：`pg_env.ps1 -Reset compat_test` → `-Apply` → `load_seed.py --schema compat_test`
（改完实测 126 passed / 0 failed）。
**规则：共享 schema 上的单测一旦红，先重灌种子再怀疑代码；自己的验证优先用独占 schema。**
