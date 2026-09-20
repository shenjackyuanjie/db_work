# 网页端后端依赖盘点（W0-d 侦察）

> 目的：在「Rust 侧新增 `src/compat/` 契约兼容层、旧 `store_*`/`commerce_*`/`app_*` 表退役」的合并中，
> 提前识别**网页端用到但 Django 从未建模**的能力。这些接口没有现成契约可抄，是真实的排期风险。
>
> 事实来源：`db/static/**` 全部前端资源、`db/src/server.rs`、`db/src/user_routes/mod.rs`、
> `db/src/server/bootstrap.rs`、`navel_backend_git/api/urls.py`、`navel_backend_git/api/{views,models}.py`。
> 所有结论均标注文件与行号，**未做推测**。

## 0. 结论摘要

| 分类 | 定义 | 存活调用数 |
|---|---|---|
| **A** 有 Django 等价契约 | 能在 `api/urls.py` 找到等价路由 | **0** |
| **B** 无 Django 契约（Rust 独有） | Django 完全没有对应能力 | **31** |
| **C** 有同名路径/能力但契约不同 | 路由存在但响应字段或认证通道不兼容 | **4** |
| 合计（网页端可达调用） | | **35** |

另有 **12 条死代码调用**（所在文件不被任何页面加载，或绑定函数从未被调用），单列于 §6。

**最重结论：网页端没有一个调用能直接映射到 Django 现有契约。** 整个 `db/static/` 都建立在 Rust 自研
会话体系（Cookie `session_token` + `/user/validate` 返回 `valid/is_admin`）与自研运营模型之上，
而 Django 只服务手机 App（Bearer token、`farmer`/`buyer` 角色、批次商城）。

## 1. 页面 ↔ 资源 ↔ 路由映射（实测）

`server.rs:60-217` 注册页面路由，`handlers_core/pages.rs:39-51` 在 `</head>` 前注入 `/app-bridge.js`。
`app-shell.js:13-20` 的 `names` 定义 shell 允许的 6 条路由，`app-shell.js:110-120` 映射到 iframe 路径。

| shell 路由 | iframe 页面 | HTML | JS | 额外 |
|---|---|---|---|---|
| `/` | `/app-content/home` | `index.html` | `index.js` | — |
| `/store` | `/app-content/store` | `store.html` | `store.js` | `store-support.js` |
| `/cart` | `/app-content/cart` | `cart.html` | `cart.js` | `store-support.js` |
| `/analyze` | `/app-content/analyze` | `analyze.html` | `analyze.js` | 需登录（`pages.rs:95-118`） |
| `/orchard-3d` | `/app-content/orchard-3d` | `orchard-3d.html` | `orchard-3d.js` | 走 CDN 加载 three.js（`orchard-3d.html:13`） |
| `/admin` | `/app-content/admin` | `admin.html` | `admin.js` | 需 `is_admin`（`pages.rs:83-93`） |
| `/admin?mode=store` | `/app-content/store-admin` | `store-admin.html` | `store-admin.js` | 需 `is_admin`（`pages.rs:57-66`） |

无页面加载的孤儿资源：`commerce.js`、`commerce.css`（全仓 grep 仅在 `server.rs:123` 的
`/commerce.html` 重定向中出现，且该重定向指向 `/store`）、`app-bridge.js` 仅有 DOM/History 逻辑无网络请求。

## 2. 主表：网页端全部后端调用

触发点列 `文件:行`；「读取字段」指前端实际消费的响应字段。

### 2.1 登录与会话（4 条，全部 C）

| 调用 | 触发点 | 分类 | 请求体/通道 | 读取字段 | 对应 Django 路径与差异 |
|---|---|---|---|---|---|
| `POST /user/validate` | `index.js:191`（首屏）、`app-shell.js:63`（每次导航）、`store.js:116`、`cart.js:104`、`analyze.js:55`、`orchard-3d.js:826`、`admin.js:131`、`store-admin.js:385` | **C** | 无体，Cookie | `valid`、`username`、`is_admin`（`index.js:198`、`app-shell.js:71-76`、`store-admin.js:386`） | 无。Django 最接近的是 `api/me`（`urls.py:8`），但语义是「取当前用户」，不是「校验并返回布尔」；且 Django 无 `is_admin` |
| `POST /user/login` | `index.js:225` | **C** | `{username, password}` | `data`（`unwrapApiPayload`）后紧跟再调 `/user/validate`（`index.js:239`） | `api/login`（`urls.py:6`）存在但契约不同：Django 返回 `{token, expiresAt, user}`，Web 依赖 Cookie 会话 + 二次校验 |
| `POST /user/register` | `index.js:260` | **C** | `{username, password, requested_role, invitation_code}`（`index.js:264-269`） | `data.hint`；`res.status===202 \|\| message==="pending_approval"`（`index.js:275-284`） | `api/register`（`urls.py:5`）存在但无 `requested_role`/`invitation_code`/待审批语义；Django 注册仅 `farmer`/`buyer` 且果农必填 `orchard_address` |
| `POST /user/logout` | `index.js:307`、`app-shell.js:166`、`store.js:313`、`cart.js:241`、`analyze.js:247`、`orchard-3d.js:73`、`admin.js:87` | **C** | 无体，Cookie | 无（仅看 HTTP status） | `api/logout`（`urls.py:7`）走 Bearer header 删除 `auth_token`，与 Cookie 通道不兼容 |

### 2.2 登录页系统状态（1 条，B）

| 调用 | 触发点 | 分类 | 读取字段 | 缺口 |
|---|---|---|---|---|
| `GET /api/system-status` | `index.js:143`（首屏必调，失败静默） | **B** | `open_registration`、`invite_bypass_enabled`、`maintenance_mode`（`index.js:102-138`） | Django 无系统设置模型。Rust 侧表：`app_system_settings`（`bootstrap.rs:84`，读写见 `system_settings.rs:62,104`） |

### 2.3 病害识别（1 条，C）

| 调用 | 触发点 | 分类 | 请求体 | 读取字段 | 差异 |
|---|---|---|---|---|---|
| `POST /api/citrus-disease-v2` | `analyze.js:196`（识别按钮） | **C** | `{image: <dataURL>}`（`analyze.js:202`） | `predicted_class`、`is_healthy`、`disease_name`、`severity`、`confidence`、`treatment_suggestion`、`preventive_measures`、`image_quality_warning`、`stage`（`analyze.js:213-234`） | Django `api/citrus-disease`（`urls.py:124`）只返回 `{predicted_class, confidence, stage}`（`views.py:483-487`）。**缺** `is_healthy`/`disease_name`/`severity`/`treatment_suggestion`/`preventive_measures`/`image_quality_warning` 六项。这六项由 Rust 的两阶段流程产生（`handlers_ai/advanced.rs:180-208`、`review.rs:37-60`）并在 `persistence.rs:5` 落 17 列宽表 |

### 2.4 现货商城（9 条，全部 B）

| 调用 | 触发点 | 分类 | 请求体 | 读取字段 |
|---|---|---|---|---|
| `GET /api/store/products` | `store.js:284`、`cart.js:183` | **B** | — | `products[].id`（**整数**，`store.js:48` 做 `Number(id)`）、`name`、`sku`、`unit_label`、`price_cents`（**分**）、`stock_quantity`、`description`、`cover_image`、`is_active`（`store.js:178-218`） |
| `GET /user/store/orders` | `store.js:274` | **B** | — | `orders[].order_no`、`status`（6 态字典 `store.js:6-13`）、`recipient_name`、`recipient_phone`、`total_cents`、`created_at`（**毫秒**）、`items[].product_name/unit_label/quantity/line_total_cents/unit_price_cents`（`store.js:234-268`） |
| `POST /user/store/orders` | `cart.js:216`（提交订单） | **B** | `{recipient_name, recipient_phone, shipping_address, items:[{product_id, quantity}]}`（`cart.js:219-224`） | `data.order_no`（`cart.js:226`） |
| `GET /user/store/support` | `store-support.js:36`（打开客服弹窗 + 每 10s 轮询，`store-support.js:89-91`） | **B** | — | `messages[].is_staff`、`created_at`、`content`（`store-support.js:40-47`） |
| `POST /user/store/support` | `store-support.js:75`（发送消息） | **B** | `{content}` | 同 GET（发送后立即 refresh） |
| `GET /user/admin/store/analytics?days=` | `store-admin.js:79`（经营概览） | **B** | — | `summary.{revenue,orders,buyers,pending}`、`trend[].{day,revenue,orders}`、`statuses[].{status,count}`、`products[].{product_name,quantity,revenue}`（`store-admin.js:81-110`） |
| `GET /user/admin/store/overview` | `store-admin.js:1505`、`admin.js:1505` | **B** | — | `product_count`、`order_count`、`gross_amount_cents`、`pending_order_count`（`admin.js:1496-1502`） |
| `GET /user/admin/store/products` | `store-admin.js:113`、`admin.js:1568` | **B** | — | 同上商品字段 + `description`（`store-admin.js:139-146`） |
| `POST /user/admin/store/orders` | `store-admin.js:179`、`admin.js:1726` | **B** | — | `orders[]`（含 `username`、`shipping_address`，`store-admin.js:158-175`） |

### 2.5 商城后台写操作（6 条，全部 B）

| 调用 | 触发点 | 分类 | 请求体 | 说明 |
|---|---|---|---|---|
| `POST /user/admin/store/products` | `store-admin.js:278`、`admin.js:1620` | **B** | `{name,sku,unit_label,price_cents,stock_quantity,description,cover_image}`（`store-admin.js:268-276`） | 创建商品 |
| `PUT /user/admin/store/products/{id}` | `store-admin.js:278`（editing 分支）、`admin.js:1617` | **B** | 同上 | 编辑商品 |
| `POST /user/admin/store/products/{id}/toggle` | `store-admin.js:309`、`admin.js:1675` | **B** | — | 上下架 |
| `POST /user/admin/store/products/{id}/cover` | `store-admin.js:327`、`admin.js:1650` | **B** | **multipart** `image` 文件，前端限 8MB（`store-admin.js:1633-1646`） | 封面文件上传 |
| `POST /user/admin/store/orders/status` | `store-admin.js:348`、`admin.js:1733` | **B** | `{order_id, status}` | 订单状态 |
| `GET/POST /user/admin/store/support` | `store-admin.js:188`（按 `?username=` 取会话）、`store-admin.js:203`（会话列表）、`store-admin.js:375`（回复 `{username,content}`） | **B** | 见左 | 客服后台 |

### 2.6 运营后台（15 条，全部 B）

| 调用 | 触发点 | 分类 | 请求体 | 读取字段 |
|---|---|---|---|---|
| `POST /user/validate` | 见 §2.1 | C | — | — |
| `POST /user/admin/orchard/overview` | `admin.js:887`（`?section=orchard` 2D 园区地图） | **B** | `{username}` | `trees[]`、`legend[]`、`summary`、`coordinate_range`（`admin.js:900-907`） |
| `POST /user/orchard/overview` | `orchard-3d.js:839`（3D 大屏，无体） | **B** | 无 | `coordinate_range.{min_x,max_x,min_y,max_y}`、`legend[].{label,count,level,color}`、`summary.{total_trees,online_trees,last_sampled_at}`、`weather`、`trees[]`（`orchard-3d.js:846-853`、`384-419`、`536-580`） |
| `POST /user/admin/dashboard/stats` | `admin.js:929` | **B** | 无 | `totals.{detections,healthy_count,diseased_count,healthy_rate,users,admins,pending_users}`、`environment.{current_temperature,current_humidity,health_score,active_alerts}`、`daily_counts[].{date,count}`（`admin.js:936-980`） |
| `POST /user/admin/dashboard/logs` | `admin.js:1011` | **B** | 无 | `logs[].{type,message,created_at}`（`admin.js:1018-1048`） |
| `POST /user/admin/settings/get` | `admin.js:1063` | **B** | 无 | `open_registration`、`invite_bypass_enabled`、`maintenance_mode`、`default_invite_ttl_seconds`、`confidence_threshold`、`log_retention_days`（`admin.js:1051-1060`） |
| `POST /user/admin/settings/update` | `admin.js:1079` | **B** | 同上 6 字段（`admin.js:1080-1086`） | 回写同上 |
| `POST /user/admin/invitations/create` | `admin.js:146` | **B** | `{ttl_seconds}` | `code`、`expires_at`（`admin.js:156-157`） |
| `POST /user/admin/invitations/list` | `admin.js:266` | **B** | 无 | `invitations[].{code,used,expires_at}`（`admin.js:178-206`） |
| `POST /user/admin/users/list` | `admin.js:277` | **B** | 无 | `users[].{username,is_admin,created_at}`（`admin.js:208-230`） |
| `POST /user/admin/set_admin` | `admin.js:165` | **B** | `{target_username, make_admin}` | — |
| `POST /user/admin/pending/list` | `admin.js:255` | **B** | 无 | `pending_users[].{username,requested_role,created_at}`（`admin.js:232-252`） |
| `POST /user/admin/pending/approve` | `admin.js:288` | **B** | `{username}` | — |
| `POST /user/admin/pending/reject` | `admin.js:300` | **B** | `{username}` | — |

> 说明：`admin.html` 的 `data-admin-section` 只有 `orchard/access/overview/audit/settings` 5 个导航项
> （`admin.html:39,42,48,51,54`），但页面里存在 6 个面板，多出的 `data-admin-page="store"`
> （`admin.html:218-342`）是 `store-admin.html` 的**重复副本**，且导航项已被 `/store-admin` 链接顶替
> （`admin.html:45`）。`app-shell.js:31-41` 会把 `/store-admin` 与 `?section=store` 统一重写成
> `/admin?mode=store`，因此该面板实际上不可达。

## 3. B/C 缺口风险清单（按切换阻塞程度排序）

### R1 — 3D 果园沙盘：Django 完全没有几何与传感器模型（阻塞级）

- 前端需要：`trees[].position.{x,y}`、`terrain_height`、`id`(整数)、`tree_code`、`tag_serial_number`、
  `status.{level,label,color}`、`latest_sensor.{temperature,humidity,sampled_at}`、
  `latest_diagnosis.{disease_name,predicted_class,confidence,timestamp}`（`orchard-3d.js:384-419`），
  外加 `weather.{current,current_units,latitude,longitude,city,timezone}`（`orchard-3d.js:582-624`）与
  `coordinate_range`（`admin.js:904-907`、`orchard-3d.js:846-849`）。
- Rust 数据源：`app_orchard_trees`（`bootstrap.rs:103`，列 `tree_code/tag_serial_number/pos_x/pos_y/terrain_height/is_active`）
  + `app_tree_sensor_records`（`bootstrap.rs:115`），查询见 `admin/orchard.rs:175-192`、`handlers_core/health_point.rs:30`。
- Django 侧：只有 `FruitTreeArchive`（`models.py:315-368`），字段是 `tree_number/plot_name/variety/planted_year/
  growth_stage/health_status/latest_temperature/latest_humidity/last_observed_at`——**无坐标、无地形高度、
  无 `tag_serial_number`、无传感器记录表**；`Orchard`（`models.py:264-309`）有 `latitude/longitude` 但那是果园中心点。
- **必须新造契约**：需要决策「在 `fruit_tree_archive` 上扩几何字段」还是「保留 `app_orchard_trees` 为 Rust 超集」。
  天气字段（`weather_code`/`wind_speed`/`current_units`）Django 侧零对应，需外部天气源或新表。

### R2 — 系统设置 / 注册开关 / 维护模式 / 邀请码 / 待审批 / 审计日志：5 张 Rust 独有表（阻塞级）

- 被调用位置：`/api/system-status`（`index.js:143`，**登录页首屏**）、
  `/user/admin/settings/{get,update}`（`admin.js:1063,1079`）、
  `/user/admin/invitations/*`（`admin.js:146,266`）、`/user/admin/pending/*`（`admin.js:255,288,300`）、
  `/user/admin/dashboard/logs`（`admin.js:1011`）。
- 相关表：`app_system_settings`、`app_invitations`、`app_pending_users`、`app_admin_audit_logs`
  （`bootstrap.rs:32,37,84,95`）；注册流写 `app_pending_users`/`app_invitations`
  （`registration.rs:73,108,133,160,190`），审批流见 `admin/pending.rs:27-168`，审计日志见
  `system_settings.rs:182,200`。
- Django 侧：**这 5 张表一个都没有**（`models.py` 全表清单见 §4）。Django 注册是直接建 `User`
  （`auth_views.py`），没有审批与邀请码概念。
- **必须新造契约**或砍功能：注意 `/api/system-status` 挂在登录页，砍掉会让首页出现 `open_registration`
  默认值行为（`index.js:24-30` 有兜底，失败静默，但注册开关将失效）。

### R3 — 客服会话：Django 无模型（阻塞级）

- 调用：`store-support.js:19,36,75`（买家侧，挂载于 `store.html:149`、`cart.html:150`）与
  `store-admin.js:188,203,375`（管理员侧）。
- 表：`store_support_messages`（`bootstrap.rs:7`，唯一索引 `bootstrap.rs:15`），读写见
  `store_workspace.rs:39,72,89,112`，前端每 10 秒轮询（`store-support.js:89-91`、`store-admin.js:380-382`）。
- Django 侧：无任何消息/会话模型。**必须新造契约**，或产品决策砍掉（会失去售前售后入口）。

### R4 — 商品封面：文件上传通道归属未定（高）

- 调用：`POST /user/admin/store/products/{id}/cover`，multipart（`store-admin.js:327`、`admin.js:1650`）。
- 落库：`store_products.cover_image`（`store.rs:199` 查询含该列，写入见商品创建/更新路径）；
  前端渲染时只接受 `^https?://`、`/store-images/`、`/uploads/` 三种前缀（`store.js:59-63`、`store-admin.js:25-29`）。
- 图片服务端点：`/store-images/{file_name}`（`server.rs:194`）、`/uploads/{file_name}` 与
  `/media/recognition_records/{file_name}`（`server.rs:202-208`）。
- Django 侧：`ProductImage.image_url`（`models.py:739-747`）是 **URL 字符串**，没有上传端点。
- **必须新造契约**：要么在 compat 层实现 multipart 落盘 + `/store-images/` 静态通道，要么改为
  前端先传图拿 URL 再引用（需产品决策）。

### R5 — 现货商城与 Django 商品模型结构性不匹配（高）

| 维度 | 网页端现在依赖 | Django `CitrusProduct` |
|---|---|---|
| 主键 | **整数** `id`（`store.js:48` 做 `Number(id)`，`store-admin.js:1551` 做 `Number(...)`） | UUID（`models.py:697`） |
| 价格 | `price_cents` 整数分（`store.js:214`、`store-admin.js:15` 手工 `/100`） | `price` Decimal（`models.py:719`） |
| 库存 | `stock_quantity` | `stock`（`models.py:721`） |
| 上架态 | `is_active` 布尔 | `status`（draft/on_sale/off_sale，`models.py:684-687`） |
| 封面 | `cover_image` 本地路径 | `cover_image_url` URL（`models.py:726`） |
| 订单状态 | 6 态（`store.js:6-13`、`store-admin.js:16-23`） | 10 态 |
| 归属 | 独立「现货」，无批次 | 必挂 `sales_batch`（`models.py:706-712`，`PROTECT`） |

另外购物车是**纯前端 localStorage**（`store.js:4,28-43`、`cart.js:4,28-43`，键 `store_cart_v1`），
而 Django 有服务端 `CartItem`（`models.py:768`）——两边购物车模型不一致，切换时需决定是否改前端。

### R6 — 后台仪表盘统计口径依赖 17 列宽表（中高）

- `/user/admin/dashboard/stats` 的 `totals.{detections,healthy_count,diseased_count,healthy_rate}` 与
  `environment.{current_temperature,current_humidity,health_score,active_alerts}`
  依赖 `app_diagnosis_records` 的 `is_healthy`/`is_citrus_leaf`/`predicted_class` 与
  `app_temperature_humidity`（`admin/dashboard.rs:25,33,42,76,104`）。
- Django `DiseaseRecognitionRecord`（`models.py:235-250`）只有 7 列，**没有** `is_citrus_leaf`、
  `citrus_type`、`severity`、`treatment_suggestion`、`preventive_measures`、`image_quality_warning`、
  `temp`、`humm`——Rust 侧是 17 列（`persistence.rs:5`）。统计口径需要重新定义。
- `/user/admin/dashboard/logs` 读 `app_admin_audit_logs`（`dashboard.rs:166`），另一张 Django 没有的表。

### R7 — 识别结果字段缺失（中）

见 §2.3：Django 的 `/api/citrus-disease` 响应是 Web 所需字段的真子集，缺 6 个字段。
需扩 Django 契约或让 compat 层补齐（但补齐所需数据 Django 侧不产生）。

### R8 — 隐式静态资源通道（中）

`<img src>` 直接引用 `/store-images/*`、`/uploads/*`、`/media/recognition_records/*`
（白名单见 `store.js:59-63`；服务端见 `server.rs:194,202-208`，鉴权见 `handlers_core/media.rs:34`）。
这些路径不在任何 JS 调用里，切换时最容易漏。

### R9 — 会话通道差异（中）

Web 全程依赖 **Cookie**（`credentials: "same-origin"`，`store.js:79`、`cart.js:74`、`admin.js:115`、
`store-admin.js:41`），`commerce.js:36-44` 还会读 JS 可读的 `session_token` 并转成 `X-Session-Token`。
Django 是 `Authorization: Bearer <uuid>`。compat 层需同时支持三通道（Bearer / Cookie / `X-Session-Token`）。

### R10 — 权限模型差异（中）

Web 只有 `is_admin` 布尔（`app-shell.js:75-77`、`store-admin.js:386`、`admin.js:132`），
Django 是 `farmer`/`buyer` 角色 + 隐含的管理员。compat 层需把 `is_admin` 映射到某个角色或额外字段。

## 4. 数据表归属

Rust 现网 `public` schema 实有 22 张表（`db/src/server/bootstrap.rs` 建表语句行号在下表）。
Django 模型清单来自 `navel_backend_git/api/models.py`（30 个模型，全部显式 `db_table`）。

| 网页端读写的表 | 建表位置 | 谁在写 | Django 对应物 | 判定 |
|---|---|---|---|---|
| `app_users` | `bootstrap.rs:16` | 登录/注册/管理员（`session.rs:43,299`、`registration.rs:222`、`pending.rs:117`） | `User`（`models.py:174`，`db_table='user'`） | **部分**：Django 是 UUID 主键 + `password`；Rust 是 `username` 主键 + `password_hash` + `is_admin` + `session_token` + `created_at` bigint |
| `app_sessions` | `bootstrap.rs:25` | 登录（`session.rs:130`） | `AuthToken`（`models.py:253`，`db_table='auth_token'`） | **部分**：`token/username` vs `key/user_id` |
| `app_pending_users` | `bootstrap.rs:37` | 注册审批（`registration.rs:108`、`pending.rs:27`） | **无** | **缺** |
| `app_invitations` | `bootstrap.rs:32` | 邀请码（`management.rs:102,145`、`registration.rs:160`） | **无** | **缺** |
| `app_system_settings` | `bootstrap.rs:84` | 系统设置（`system_settings.rs:62`） | **无** | **缺** |
| `app_admin_audit_logs` | `bootstrap.rs:95` | 审计（`system_settings.rs:182,200`） | **无** | **缺** |
| `app_tasks` | `bootstrap.rs:43` | 任务（`tasks.rs:107,280,378`） | `Task`（`models.py:216`） | **部分**：字段基本齐，`username` vs `user_id` FK，`created_at` bigint vs timestamptz |
| `app_temperature_humidity` | `bootstrap.rs:56` | 温湿度（`temperature.rs:116`、`tasks.rs:343`） | `TemperatureHumidityData`（`models.py:200`） | **部分**：`node_id` 默认 `0x4a80` vs username 归属 |
| `app_diagnosis_records` | `bootstrap.rs:64` | 识别/统计（`persistence.rs:5`） | `DiseaseRecognitionRecord`（`models.py:235`） | **缺大半**：Django 7 列 vs Rust 17 列；缺 `is_citrus_leaf/citrus_type/severity/treatment_suggestion/preventive_measures/image_quality_warning/temp/humm` |
| `app_orchard_trees` | `bootstrap.rs:103` | 3D 沙盘/健康点（`admin/orchard.rs:175`、`health_point.rs:30`） | `FruitTreeArchive`（`models.py:315`） | **缺几何**：无 `pos_x/pos_y/terrain_height/tag_serial_number` |
| `app_tree_sensor_records` | `bootstrap.rs:115` | 3D 沙盘（`orchard.rs:181`、`temperature.rs:93,137`） | **无** | **缺** |
| `store_support_messages` | `bootstrap.rs:7` | 客服（`store_workspace.rs:89`） | **无** | **缺** |
| `store_products` | `bootstrap.rs:207` | 现货商城（`store.rs:156`） | `CitrusProduct` + `ProductImage`（`models.py:683,739`） | **部分**：整数 id/`price_cents`/`stock_quantity`/`is_active` vs UUID/Decimal/`stock`/`status` |
| `store_orders` | `bootstrap.rs:220` | 现货订单（`store.rs:509`） | `Order`（`models.py:784`） | **部分**：6 态 vs 10 态；`total_cents` vs Decimal |
| `store_order_items` | `bootstrap.rs:234` | 现货订单行（`store.rs:529`） | `OrderItem`（`models.py:833`） | **部分** |
| `commerce_orchards` | `bootstrap.rs:125` | 开团（`commerce.rs:179`） | `Orchard`（`models.py:264`） | 语义近亲，**网页端已无调用方**（死代码） |
| `commerce_products` | `bootstrap.rs:136` | 开团（`commerce.rs:270`） | `CitrusProduct` | 同上 |
| `commerce_batches` | `bootstrap.rs:147` | 开团（`commerce.rs:393`） | `SalesBatch`（`models.py:377`） | 同上 |
| `commerce_batch_products` | `bootstrap.rs:164` | 开团（`commerce.rs:424`） | 无（Django 商品直挂批次） | 同上 |
| `commerce_orders` | `bootstrap.rs:171` | 开团（`commerce.rs:589`） | `Order` | 同上 |
| `commerce_order_items` | `bootstrap.rs:188` | 开团（`commerce.rs:609`） | `OrderItem` | 同上 |
| `commerce_order_status_logs` | `bootstrap.rs:198` | 开团（`commerce.rs:631,823`） | 无（Django 无状态流水） | 同上 |

**Django 有、Rust 与网页端都没有的模型**（属于 App 侧，不影响网页端）：
`HomeData`、`GrowthTracking`、`DiagnoseData`、`DiagnoseListItem`、`FertilizationPlan`、
`RecommendedFertilizer`、`ApplicationSchedule`、`DiseasePrediction`、`SalesBatch`、`HarvestArchive`、
`BatchQualitySample`、`TraceEvent`、`BuyerAddress`、`CartItem`、`PaymentRecord`、`TracePackage`、
`AfterSaleRequest`、`AgentApproval`、`AgentFeedback`。

## 5. 建议

### 5.1 可以直接映射（低风险）

| 网页功能 | 映射方式 |
|---|---|
| 后台用户列表 / 设管理员（`/user/admin/users/list`、`/user/admin/set_admin`） | 落到 Django `User`；`is_admin` 需在 compat 层定为角色字段或新增列 |
| 任务列表（若将来网页要用） | `app_tasks` ↔ `Task` 字段基本对齐 |
| 温湿度数据 | `app_temperature_humidity` ↔ `TemperatureHumidityData` |

### 5.2 需要产品决策（不能由工程单方面定）

1. **注册审批 + 邀请码 + 待审批队列**（`app_pending_users`/`app_invitations`）是否保留？Django 无此概念。
   保留 = compat 层新造 3 条契约 + 2 张表；砍掉 = 注册流程简化为 Django 语义（并接受注册开关失效）。
2. **系统设置**（维护模式 / 注册开关 / 置信度阈值 / 日志保留，`app_system_settings`）是否保留？
3. **客服会话**（`store_support_messages`）保留还是替换？
4. **现货商城是否收敛到 Django 的 `citrus_product`**？若收敛，需接受前端改造（UUID id、
   Decimal 价格、状态枚举、必挂批次）与购物车从 localStorage 改到服务端 `CartItem`。
5. **3D 沙盘几何数据**放在 `fruit_tree_archive` 扩字段，还是保留独立表？

### 5.3 建议保留为 Rust 超集（不进 App 契约层）

理由：这些是 Django **从未建模**的运营能力，硬塞进「逐字兼容 App」的 compat 层会污染兼容性目标。

| 能力 | 表/端点 | 保留方式 |
|---|---|---|
| 3D 果园沙盘 | `app_orchard_trees`、`app_tree_sensor_records`、`/user/orchard/overview`、`/user/admin/orchard/overview` | 保留在 Rust 侧（可挂 `/web/*` 前缀），坐标用 `tree_number` 与 `fruit_tree_archive` 做映射 |
| 客服会话 | `store_support_messages`、`/user/store/support`、`/user/admin/store/support` | 同上 |
| 系统设置 / 邀请码 / 待审批 / 审计日志 | 4 张 `app_*` 表 + 8 条 `/user/admin/*` | 同上 |
| 商品封面 multipart 上传 | `/user/admin/store/products/{id}/cover` + `/store-images/*` | 同上 |
| 后台仪表盘统计 / 日志 | `/user/admin/dashboard/*` | 口径依赖 17 列宽表，保留 Rust 侧 |

### 5.4 建议直接退役

- `commerce.js`（275 行，无页面加载）+ `commerce.css` + `market.css` 的 commerce 专用部分。
- `admin.js:1123-1451` 的 commerce 段（`bindCommerceActions`(1430) 与 `refreshCommerceData`(1325)
  **从未被调用**，`bindEvents()`(1762) 未包含）。
- `admin.html:218-342` 的 `data-admin-page="store"` 面板（与 `store-admin.html` 重复且被
  `app-shell.js:31-41` 重写为 `mode=store` 后不可达）。
- 7 张 `commerce_*` 表 + `/user/commerce/*` + `/api/commerce/*` 端点。

## 6. 死代码调用清单（12 条，不计入 A/B/C 统计）

| 调用 | 代码位置 | 为何不可达 |
|---|---|---|
| `GET /user/commerce/orders` | `commerce.js:188` | `commerce.js` 无任何 html 引用；`/commerce` 已重定向 `/store`（`server.rs:118-125`），`app-shell.js:30` 亦归一化 |
| `POST /user/commerce/orders` | `commerce.js:226` | 同上 |
| `GET /api/commerce/storefront` | `commerce.js:198` | 同上 |
| `POST /user/admin/commerce/overview` | `admin.js:1293` | `bindCommerceActions` 从未调用；`admin.html` 无 commerce 面板 |
| `GET /user/admin/commerce/orchards` | `admin.js:1302` | 同上 |
| `GET /user/admin/commerce/products` | `admin.js:1308` | 同上 |
| `GET /user/admin/commerce/batches` | `admin.js:1314` | 同上 |
| `POST /user/admin/commerce/orders` | `admin.js:1320` | 同上 |
| `POST /user/admin/commerce/orchards` | `admin.js:1344` | 同上 |
| `POST /user/admin/commerce/products` | `admin.js:1362` | 同上 |
| `POST /user/admin/commerce/batches` | `admin.js:1389` | 同上 |
| `POST /user/admin/commerce/orders/status` | `admin.js:1417` | 同上 |

## 7. 后端存在但网页端从不调用的端点（供退役评估参考）

以下端点有完整实现，但全仓前端无调用（`grep` 全部 `.js`/`.html` 确认）：

- **会话别名**：`/api/register`、`/api/login`、`/api/logout`、`/api/validate`、`/api/user`（`server.rs:82-89`）、`/user/me`（`user_routes/mod.rs:47`）
- **农事/识别（App 面向，Django 也有）**：`/api/home`、`/api/growth-tracking`、`/api/diagnose`、
  `/api/temperature-humidity`、`/api/tasks`、`/api/tasks/add`、`/api/tasks/complete`、
  `/api/tasks/generate/disease`、`/api/tasks/generate/environment`、`/api/recognition-records`、
  `/api/disease-treatment`、`/api/citrus-disease`、`/api/generate`、`/api/generate/fertilization-plan`、
  `/citrus/analyze`（`server.rs:94-104,141-180`）
- **无对应方**：`/api/health-point`（`server.rs:172-175`）、`/api/commerce/batches/{id}/trace`（`server.rs:185-188`）
- **仅被 `<img src>` 隐式使用**：`/store-images/{file_name}`、`/uploads/{file_name}`、
  `/media/recognition_records/{file_name}`（`server.rs:194,202-208`）
- **健康检查**：`/health`（`server.rs:60`）
