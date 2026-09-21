# W2 人工浏览器走查清单

**这是全项目唯一无法由 agent 完成的验收。** 前面所有验证（契约回放 239/0/12、单测、按前端实际请求打服务端）都只能证明「服务端返回对」，**证不了 DOM 交互**——事件绑定是否接上、canvas 是否渲染、`window.confirm` 是否弹出。本清单让不懂代码的人按步骤点一遍，并在出错时能定位到具体请求。

出问题时请按每条末尾的「看哪条请求」在浏览器 **F12 → Network** 里对账：请求路径、HTTP 状态、响应体的 `code`/`data` 形状。

---

## 0. 前置条件（**先读，否则会白试半天**）

| 页面 | 需要的账号 | 原因 |
|---|---|---|
| 登录页、识别页、3D 沙盘 | 任意已登录账号 | 只要求会话 |
| **商城 / 购物车下单** | **buyer 账号**（`buyer_zhang`） | `/api/cart`、`/api/orders`、`/api/addresses` 在契约层是 **`IsBuyer`**；用果农账号会拿到 **403 `该接口仅限购买者使用`**，那是**正确行为、不是 bug** |
| **后台（admin / store-admin）** | **`is_admin = TRUE` 的账号** | 否则 `/web/admin/*` 一律 **403 `当前账号无管理权限`**（同样正确） |

**演示账号**（来自 `seed_demo_data`）：`farmer_xinfeng` / `farmer_xunwu` / `farmer_anyuan`（口令 `farmer123`）、`buyer_zhang`（口令 `buyer123`）。

**把某个果农变成管理员**：
```sql
UPDATE "user" SET is_admin = TRUE WHERE username = 'farmer_xinfeng';
```

**3D 沙盘额外需要数据**：`app_orchard_trees` / `app_tree_sensor_records` **不在 `load_seed` 的 dumpdata 里**，不补的话 `trees=[]`，沙盘是空的（不是坏了）。见 §7 的补数据 SQL。

---

## 1. 登录页（`index.html` + `index.js`）

| 步骤 | 期望 | 看哪条请求 |
|---|---|---|
| 打开站点根路径 | 登录页渲染，**不出错就不显示维护提示** | `GET /web/system-status` → **信封**，`data` 含 6 键（`open_registration`/`maintenance_mode`/…） |
| 刷新页面（已登录时） | 顶部显示「已登录：<用户名>」，管理员显示模式选择器 | `POST /web/session/validate` → **扁平体**（顶层直接有 `valid`/`username`/`is_admin`，**没有 `data` 包裹**） |
| 输入错误口令点登录 | 提示「登录失败」类文案，**不白屏** | `POST /web/session/login` → **401 + 信封** |
| 用 `buyer_zhang`/`buyer123` 登录 | 进入买家态；`is_admin=false` 时不显示管理员模式选择器 | 同上 → **200 + 信封**，`data` 非 null 且含 `username` |
| 切到注册页签，填一个**已存在**的用户名 | 提示用户名已存在 | `POST /web/session/register` → **409** |
| 填新用户 + `requested_role=admin` | 提示「已提交管理员申请，等待管理员审核」 | 同上 → **202**，`message` 为 `pending_approval`，可能带 `data.hint` |
| 点退出 | 回到未登录态，**不出现「退出失败，请重试」** | `POST /web/session/logout` → **恒 200**（旧端点无 token 时返回 400，那正是这个提示的来源） |
| 点密码显隐眼睛按钮 | 明文/密文切换 | 无请求（纯 DOM） |

---

## 2. 外壳与导航（`app-shell.html` + `app-shell.js`）

| 步骤 | 期望 | 看哪条请求 |
|---|---|---|
| 点左侧导航在各个页面间切换 | iframe 内容跟着换，地址栏同步 | `POST /web/session/validate`（每次导航都会校验一次） |
| **浏览器后退/前进** | 内容随地址栏回退，**不重复叠加** | 同上（`popstate` 触发） |
| 切到别的窗口再切回来 | 会话状态刷新（比如在别处登出后，这里会变成未登录） | 同上（`focus` 触发） |
| 点外壳上的退出按钮 | 回首页且显示未登录 | `POST /web/session/logout` → 200 |
| 用**非管理员**账号点「后台」 | 被拦下（不进入后台） | `POST /web/session/validate` → `is_admin=false` |

**注意**：`app-shell.js` 里有一条 `/commerce → /store` 的**路径重写是故意保留**的历史入口兼容，看到它不要当 bug。

---

## 3. 商城（`store.html` + `store.js`）

> **必须用 buyer 账号**（`buyer_zhang`）。

| 步骤 | 期望 | 看哪条请求 |
|---|---|---|
| 打开商城页 | 商品列表出现，价格显示为 **¥168.00 这种正常金额**（不是 **¥1.68**，也不是 `undefined`） | `GET /api/products` → 列表在 **`data.items`**（不是 `data.products`）；`price` 是**字符串** `"168.00"` |
| 切「规格类型」筛选 / 搜索名称 | 列表随之收窄 | `GET /api/products?sku_type=…` / `?q=…` |
| 点「加入购物车」 | 侧栏/购物车里出现该商品，数量 +1 | `POST /api/cart` → 200；随后 `GET /api/cart` |
| **先加 A 批次商品，再加 B 批次商品** | 弹出一个**带批次号**的确认框；同意后清空并加入 B 批次 | `POST /api/cart` → **400 `一次只能结算同一果园供货批次…`**（这是后端规则，确认框只是提前告知） |
| 打开购物车（`cart.html`） | 商品、单价、数量、合计与服务端一致 | `GET /api/cart` → 每项带完整 `product`；合计用服务端的 `totalAmount`，前端**不本地累加** |
| 改数量 / 删一项 | 金额随之更新 | `PATCH /api/cart/<item_id>` / `DELETE /api/cart/<item_id>` |
| 点「结算」 | 进入下单（填收货信息/选地址） | `GET /api/addresses`（先查找或创建），再 `POST /api/orders` |
| 提交订单 | 生成订单号，状态「待支付」 | `POST /api/orders`，body 是 `{address_id}`（**明细由服务端购物车决定**，前端不再传 `items[]`） |
| 下单后再看购物车 | **购物车被清空**（这是契约层的正确行为） | `GET /api/cart` → `items: []` |
| 点「支付」 | 状态变「已支付/待发货」 | `POST /api/orders/<id>/pay` |
| 对**已支付**订单点「取消」 | 提示不能取消 | `POST /api/orders/<id>/cancel` → **400** |
| 对**待支付**订单点「取消」 | 状态变已取消 | 同上 → 200 |

**已知无法在本环境验证**：跨批次确认框用 `window.confirm`，**只有真机点击才会弹出**。

---

## 4. 识别页（`analyze.html` + `analyze.js`）

| 步骤 | 期望 | 看哪条请求 |
|---|---|---|
| 未登录时打开 | 提示需要登录，分析按钮禁用 | `POST /web/session/validate` |
| 登录后打开 | 显示用户名徽标；管理员额外显示后台入口 | 同上 |
| 选一张柑橘叶照片并提交 | 进度条走完，出结果徽标（健康/患病）与详情 | `POST /web/citrus-disease-v2` → **信封且 `code === 200`**（前端就是判 `code`），结果在 `data` 里 |
| 提交一张明显不是树叶的图 | 结果标为「非果树」类 | 同上 |
| 点退出 | 回未登录态 | `POST /web/session/logout` → 200 |

**注意**：识别走的是 `handlers_ai`（自研推理），**不是**契约层的 `/api/citrus-disease`。它同时把结果**双写**进 `web_diagnosis_records`（网页仪表盘用）与 `disease_recognition_record`（App 用）。

---

## 5. 3D 沙盘（`orchard-3d.html` + `orchard-3d.js`）

> **必须先用 §7 补 `app_orchard_trees` 数据**，否则树列表为空。

| 步骤 | 期望 | 看哪条请求 |
|---|---|---|
| 打开沙盘 | **3D 场景渲染出来**，树按坐标分布 | `POST /web/orchard/overview` → **裸 JSON**（顶层直接是 `trees`/`coordinate_range`/`weather`，**没有 `code` 信封**） |
| 看顶部概览 | 总株数/在线株数与数据一致 | 同上 → `summary.total_trees` |
| 鼠标悬停某棵树 | 出现 tooltip | 无请求 |
| **点开某棵树** | 详情显示「病害信息」一栏 | 同上 → `trees[i].latest_diagnosis.disease_name` 或 `.predicted_class`（两者都空时显示「未发现异常」） |
| 拖动旋转 / 缩放 | 视角跟随（`autoRotate` 开启时缓慢自转） | 无请求 |
| 点退出 | 回未登录态 | `POST /web/session/logout` |

**已知无法在本环境验证**：three.js 的 canvas 渲染、GPU/`devicePixelRatio`、鼠标拖拽手感——**只有真机**。服务端侧我能证到的是：`position{x,y}`、`terrain_height`、`status{level,label,color}`、`latest_sensor{temperature,humidity,sampled_at}`、`latest_diagnosis{timestamp,predicted_class,disease_name,confidence}`、`coordinate_range{min_x,max_x,min_y,max_y}`、`weather.current` 全都实测到位（含一条有诊断的树与一条无诊断的对照树）。

---

## 6. 后台（`admin.html` + `admin.js`；商城运营在 `store-admin.html` + `store-admin.js`）

> **必须 `is_admin = TRUE`**。

| 步骤 | 期望 | 看哪条请求 |
|---|---|---|
| 进入后台 | 概览面板出数字（不是空白/全 0） | `POST /web/admin/dashboard/stats` → **信封且 `data` 非 null**，含 `totals`/`daily_counts`/`environment` |
| 看审计日志 | 出现若干条日志，**时间不是 1970 年** | `POST /web/admin/dashboard/logs` → 时间走 `parseTime`（秒/毫秒都吃） |
| 系统设置页 | 显示注册开关、维护模式、阈值 | `POST /web/admin/settings/get` |
| 改一个设置并保存 | 提示成功，刷新后值保留 | `POST /web/admin/settings/update` |
| 用户列表 | 列出用户，**创建时间不是 1970 年** | `POST /web/admin/users/list` → 时间必须是**秒**（`formatDate` 按秒解析） |
| 邀请码：新建（TTL 留空/0） | 显示「永久有效」 | `POST /web/admin/invitations/create` → `ttl=0` 时 `expires_at` 是哨兵大数 |
| 待审批：通过一个 | 该用户可以**用注册时的原口令登录成功** | `POST /web/admin/pending/approve` |
| 待审批：驳回一个 | 状态变驳回 | `POST /web/admin/pending/reject` |
| 把某人设为管理员 / 取消 | 列表里的管理员标记随之变化 | `POST /web/admin/set_admin` |
| **商城运营页**（`/store-admin`） | 商品列表、订单列表、成交统计出数 | `POST /web/admin/store/products`、`/orders`、`/analytics`、`/overview` |
| 新建商品（**用旧字段名填**：`price_cents`/`stock_quantity`） | 建成功，且**在商城页能立刻看到** | `POST /web/admin/store/products` → 新商品默认 `status=on_sale` |
| 上传商品封面 | 封面显示出来（不是碎图） | `POST /web/admin/store/products/<id>/cover` → 落库路径**必须带 `/store-images/` 前缀**，否则 App 与网页同时 404 |
| 改订单状态 | 状态更新 | `POST /web/admin/store/orders/status` |
| 客服会话（后台侧） | 看到买家留言并回复 | `POST /web/admin/support` |

**已知缺口**：`citrus_product` **没有 `sku` 字段**，后台 SKU 输入框已移除（登记为 `DEVIATIONS.md` D18，相对旧后台的能力损失）。

---

## 7. 客服（`store-support.js`，加载于 `store.html` 与 `cart.html`）

| 步骤 | 期望 | 看哪条请求 |
|---|---|---|
| 未登录时点右下角「联系客服」 | 提示「请先登录，再发送和查看客服消息」 | `GET /web/support` → **401 且响应体是 JSON**（`{"message":"请先登录"}`） |
| 登录后点开 | 打开对话框，历史消息列表出现（空则显示引导文案） | `GET /web/support` → **裸 JSON**，顶层直接是 `messages`（**没有 `code`/`data` 包裹**，套了会 TypeError） |
| 输入内容点发送 | 消息出现在列表里，标记为「我」 | `POST /web/support`，body `{content}` → 裸 JSON |
| 对话框开着不动 | **每 10 秒**自动刷新一次（切到别的标签页会暂停） | 周期性 `GET /web/support` |
| 关掉再打开 | 历史仍在 | `GET /web/support` |

---

## 8. 已起服务的最小命令

```powershell
cd D:\githubs\db_work\db

# sccache 在本机沙箱里起不来，跑 cargo 前必须清掉 wrapper
$env:CARGO_BUILD_RUSTC_WRAPPER=''
# 用独立 target 目录，避免和别的构建抢 exe 锁（会报 link.exe LNK1104）
$env:CARGO_TARGET_DIR='D:\githubs\db_work\db\target\walkthrough'

# 1) 建一个独立的 scratch schema（绝不碰现网 public）
scripts\pg_env.ps1 -Reset compat_walk
scripts\pg_env.ps1 -Apply compat_walk

# 2) 灌契约种子（100 行）
& D:\githubs\db_work\.venv-django\Scripts\python.exe scripts\load_seed.py --schema compat_walk

# 3) 补沙盘数据（不补的话 3D 沙盘 trees=[]）
& 'D:\apps\pg\18\bin\psql.exe' "postgres://db_race:datarace@192.168.3.52:5400/db_race?options=-csearch_path%3Dcompat_walk" -f D:\githubs\db_work\.s41_diag.sql

# 4) 起服务（会自动造运行目录，并把 onnx/ static/ 指回仓库）
scripts\pg_env.ps1 -Serve compat_walk -Build -Port 11241

# 5) 浏览器打开 http://127.0.0.1:11241/    然后按 §1–§7 走查

# 6) 收工
scripts\pg_env.ps1 -Serve compat_walk -Stop
```

> `.s41_diag.sql` 里除了造诊断记录，还会把 `farmer_xinfeng` 设为管理员——走查后台时需要。
> **注意**：`psql` 的中文参数经 PowerShell 传会被破坏成非法 UTF-8，必须走 `-f 文件`（该文件就是为此存在的）。

---

## 9. 已知无法在本环境验证的（不要把「没验」说成「验过了」）

| 项 | 为什么 |
|---|---|
| **LLM 相关分支**（智能体 `/web/*`、任何走 OpenRouter 的推理增强） | `config.toml` 里的 OpenRouter key 已失效，且 `AppConfig::load` **只读 `config.toml`，没有环境变量可覆盖该 key**（只有 `COMPAT_DATABASE_URL` 能覆盖连接串）。所以 LLM 路径在本环境**跑不通**，只能验规则兜底分支 |
| **three.js canvas 渲染 / GPU / 交互手感** | 需要真实浏览器与显卡 |
| **`window.confirm` 弹窗**（商城的跨批次确认框） | 需要真人点击 |
| **iframe 导航、`popstate`/`focus` 触发的会话刷新** | 需要真实浏览器的历史与焦点事件 |
| **`<input type="file">` 选文件对话框** | 需要真人操作；服务端侧只验过「按前端会发出的 base64 请求」 |
| **HttpOnly cookie 的真机行为** | 服务端下发了 `Set-Cookie`（实测有），但「浏览器里 JS 读不到它、请求自动携带」只能真机确认 |
| **多标签页/多窗口的会话一致性** | 依赖浏览器的 storage/cookie 事件 |

**除以上各项外**，前端能由服务端证到的部分（端点路径、reader 形状、金额量级、时间单位、状态码与错误文案）都已实测；服务端契约层另有 251 条 golden 用例覆盖（**239 pass / 0 fail / 12 expected_deviation**）。
