// 商城页（普通购买）：商品列表 + 侧栏购物车 + 我的订单。
//
// ## 这一版换了什么（W2 商城收敛）
//
// | 项 | 旧 | 新 |
// |---|---|---|
// | 商品来源 | `/api/store/products`（自研 `store_products`，整数 id + `price_cents` 分） | `/api/products`（Django 契约表 `citrus_product`，UUID + `price` 十进制**字符串**） |
// | 购物车 | `localStorage["store_cart_v1"]`（客户端） | 服务端 `/api/cart`（契约表 `cart_item`） |
// | 订单 | `/user/store/orders` | `/api/orders`（+ `/api/orders/<id>/pay`、`/cancel`） |
// | 会话 | `/user/validate`、`/user/logout` | `/web/session/validate`、`/web/session/logout` |
//
// ## 两个必须记住的前提
//
// 1. **网页商城是给 buyer 账号用的**。`/api/products` 是公开的，但 `/api/cart`、`/api/orders`
//    都是 `IsBuyer`：`/web/session/register` 注册出来的是 `role=farmer`，所以**果农账号登录后
//    调购物车与订单会拿到 403 `该接口仅限购买者使用`——这是正确行为，不是 bug**。
//    页面用 `state.buyerBlocked` 把这个 403 转成可读提示，而不是弹一句英文报错。
// 2. **认证靠 HttpOnly cookie，前端不需要拿 token**。契约层鉴权是三通道
//    （Bearer → Cookie `session_token` → `X-Session-Token`），网页登录后 cookie 自动带上，
//    所以这里所有请求只需 `credentials: "same-origin"`。
//
// 旧购物车数据（`store_cart_v1` 存的是整数 id，而契约表是 UUID，无可靠映射）**按裁定直接丢弃**，
// 初始化时显式清掉该 key，不留半残状态。

(() => {
  const state = {
    products: [],
    cartItems: [],
    cartTotalAmount: "0.00",
    session: null,
    buyerBlocked: false,
  };
  const $ = (id) => document.getElementById(id);
  const LEGACY_CART_KEY = "store_cart_v1";

  // 契约里的金额是 `NUMERIC` 序列化出来的**字符串**（如 `"45.00"`），不是分。
  // 展示统一走 money()，涉及算术一律先转成分，避免浮点漂移导致 100 倍或尾差。
  function money(value) {
    return `¥${(Number(value || 0)).toFixed(2)}`;
  }

  function productIsAvailable(product) {
    // 契约给了 is_available（后端综合 status 与批次状态算出来的），优先用它；
    // 退化时才看 status === "on_sale"。
    if (typeof product?.is_available === "boolean") return product.is_available;
    return product?.status === "on_sale";
  }

  function escapeHtml(value) {
    return String(value ?? "")
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#039;");
  }

  // 契约层的错误体有两种 message：字符串（业务错误）与对象（DRF 字段校验）。
  // 对象形态直接 String() 会得到 "[object Object]"，必须摊平。
  function messageOf(body) {
    const raw = body?.message ?? body?.error;
    if (!raw) return "";
    if (typeof raw === "string") return raw;
    if (typeof raw === "object") {
      return Object.entries(raw)
        .map(([field, value]) => {
          const text = Array.isArray(value) ? value.join("；") : String(value);
          return field === "non_field_errors" ? text : `${field}：${text}`;
        })
        .join("；");
    }
    return String(raw);
  }

  async function request(url, options = {}) {
    const response = await fetch(url, {
      ...options,
      headers: new Headers(options.headers || {}),
      credentials: "same-origin",
    });
    const body = await response.json().catch(() => ({}));
    if (!response.ok) {
      const error = new Error(messageOf(body) || `请求失败（${response.status}）`);
      error.status = response.status;
      throw error;
    }
    return body.data ?? body;
  }

  function showToast(message, isError = false) {
    const toast = $("toast");
    toast.textContent = message;
    toast.classList.toggle("is-error", isError);
    toast.classList.add("is-visible");
    window.clearTimeout(showToast.timer);
    showToast.timer = window.setTimeout(
      () => toast.classList.remove("is-visible"),
      3200,
    );
  }

  // 只允许展示可信的封面来源，避免注入任意属性/URL。
  // `/store-images/`、`/uploads/`、`/media/recognition_records/` 是**隐式静态通道**：
  // 没有任何 fetch，靠数据里的路径串 + <img src> 生效。
  function coverImageSrc(value) {
    const src = String(value || "").trim();
    if (/^(https?:\/\/|\/store-images\/|\/uploads\/|\/media\/recognition_records\/)/.test(src)) {
      return src;
    }
    return "";
  }

  function renderSession() {
    const loggedIn = Boolean(state.session?.username);
    $("loginBadge").textContent = loggedIn ? state.session.username : "未登录";
    $("loginBadge").className = loggedIn ? "badge success" : "badge";
    $("loginNavLink").hidden = loggedIn;
    $("logoutButton").hidden = !loggedIn;
    $("adminNavLink").hidden = !state.session?.is_admin;
    $("refreshOrdersButton").hidden = !loggedIn;
    $("ordersEmpty").hidden = loggedIn;
    renderCartSummary();
  }

  async function validateSession() {
    try {
      // 注意：`/web/session/validate` 是**扁平体 + 恒 200**，不套信封，
      // 所以这里的 request() 会原样拿到 `{valid, username, is_admin, ...}`。
      const data = await request("/web/session/validate", { method: "POST" });
      state.session = data.valid ? data : null;
    } catch {
      state.session = null;
    }
    renderSession();
  }

  function renderCartSummary() {
    const items = state.cartItems;
    const count = items.reduce((sum, item) => sum + Number(item.quantity || 0), 0);

    const badge = $("cartNavCount");
    badge.hidden = count === 0;
    badge.textContent = count > 99 ? "99+" : String(count);
    $("cartStatus").textContent = count ? `${count} 件` : "空";

    if (state.buyerBlocked) {
      $("cartSummary").innerHTML =
        '<div class="empty-block">当前账号不是购买者，购物车仅对购买者账号开放。</div>';
      return;
    }
    if (!state.session) {
      $("cartSummary").innerHTML =
        '<div class="empty-block">登录后即可使用购物车。</div>';
      return;
    }
    if (!items.length) {
      $("cartSummary").innerHTML =
        '<div class="empty-block">还没有选择商品，快去挑选吧。</div>';
      return;
    }

    $("cartSummary").innerHTML = `
      <div class="mini-cart-list">
        ${items
          .map((item) => {
            const product = item.product || {};
            return `
          <div class="mini-cart-item">
            <div class="mini-cart-item__info">
              <strong>${escapeHtml(product.name)}</strong>
              <small>${escapeHtml(product.unit)} · ${money(product.price)}</small>
            </div>
            <div class="mini-cart-item__qty">
              <button type="button" data-dec-item="${escapeHtml(item.id)}" aria-label="减少">−</button>
              <span>${Number(item.quantity || 0)}</span>
              <button type="button" data-inc-item="${escapeHtml(item.id)}" aria-label="增加">+</button>
            </div>
            <div class="mini-cart-item__line">${money(item.subtotal)}</div>
            <button class="mini-cart-item__remove" type="button" data-remove-item="${escapeHtml(item.id)}" aria-label="移除">×</button>
          </div>
        `;
          })
          .join("")}
      </div>
      <div class="cart-summary-total">
        <span>合计</span><strong>${money(state.cartTotalAmount)}</strong>
      </div>
    `;
  }

  async function loadCart() {
    if (!state.session) {
      state.cartItems = [];
      state.cartTotalAmount = "0.00";
      renderCartSummary();
      return;
    }
    try {
      // 服务端已经算好了 totalAmount，不要在客户端再累加一遍（浮点尾差 + 与库存口径不一致）。
      const data = await request("/api/cart");
      state.cartItems = Array.isArray(data.items) ? data.items : [];
      state.cartTotalAmount = data.totalAmount || "0.00";
      state.buyerBlocked = false;
    } catch (error) {
      if (error.status === 403) {
        // 果农账号：这是契约层的正确行为（IsBuyer），转成可读提示。
        state.buyerBlocked = true;
        state.cartItems = [];
        state.cartTotalAmount = "0.00";
      } else {
        showToast(error.message, true);
      }
    }
    renderCartSummary();
  }

  function renderProducts() {
    const list = $("productList");
    $("productCount").textContent = state.products.length;
    if (!state.products.length) {
      list.innerHTML =
        '<div class="empty-block">当前没有在售商品，请稍后再来看看。</div>';
      return;
    }

    const query = $("productSearch").value.trim().toLowerCase();
    // 契约层也支持 `?q=`，但本页的排序（价格/库存）需要整份列表，所以仍在客户端过滤，
    // 行为与旧版一致且少一次请求。
    const products = state.products.filter((product) =>
      `${product.name} ${product.description} ${product.unit}`
        .toLowerCase()
        .includes(query),
    );
    const sort = $("productSort").value;
    if (sort === "price-asc") products.sort((a, b) => Number(a.price) - Number(b.price));
    if (sort === "price-desc") products.sort((a, b) => Number(b.price) - Number(a.price));
    if (sort === "stock") products.sort((a, b) => Number(b.stock) - Number(a.stock));

    if (!products.length) {
      list.innerHTML =
        '<div class="empty-block">没有找到匹配商品，试试其他关键词。</div>';
      return;
    }

    list.innerHTML = products
      .map((product) => {
        const available = productIsAvailable(product);
        const stock = Number(product.stock || 0);
        const soldOut = !available || stock <= 0;
        const coverSrc = coverImageSrc(product.cover_image_url);
        const limit = Number(product.purchase_limit || 0);
        const min = Math.max(1, Number(product.minimum_order_quantity || 1));
        const hint = soldOut
          ? "暂时售罄"
          : [
              `库存 ${stock} 件`,
              min > 1 ? `起购 ${min} 件` : "",
              limit > 0 ? `限购 ${limit} 件` : "",
            ]
              .filter(Boolean)
              .join(" · ");
        return `
        <article class="product-card">
          <div class="product-card__image ${coverSrc ? "" : "product-card__image--placeholder"}">
            ${
              coverSrc
                ? `<img loading="lazy" src="${escapeHtml(coverSrc)}" alt="${escapeHtml(product.name)}" />`
                : '<span class="product-card__placeholder-mark">橙</span>'
            }
          </div>
          <div>
            <h3 class="product-card__name">${escapeHtml(product.name)}</h3>
            <div class="product-card__unit">${escapeHtml(product.unit)}</div>
          </div>
          <p class="product-card__desc">${escapeHtml(product.description || "赣南产地精选脐橙，现货直发。")}</p>
          <div class="product-card__price">${money(product.price)}</div>
          <div class="product-card__stock">${escapeHtml(hint)}</div>
          <div class="product-card__buy">
            <input type="number" min="${min}" max="${Math.max(min, Math.min(stock || min, limit || stock || min))}" value="${min}" data-product-id="${escapeHtml(product.id)}" aria-label="${escapeHtml(product.name)}数量" ${soldOut ? "disabled" : ""} />
            <button class="app-nav__button app-nav__button--primary" type="button" data-add-product="${escapeHtml(product.id)}" ${soldOut ? "disabled" : ""}>加入购物车</button>
          </div>
        </article>
      `;
      })
      .join("");
  }

  function renderOrders(orders) {
    const list = $("ordersList");
    if (!orders.length) {
      list.innerHTML = state.session
        ? state.buyerBlocked
          ? '<div class="empty-block">当前账号不是购买者，无法查看订单。</div>'
          : '<div class="empty-block">还没有订单。</div>'
        : "";
      return;
    }
    list.innerHTML = orders
      .map((order) => {
        const items = (Array.isArray(order.items) ? order.items : [])
          .map(
            (item) => `
          <li class="order-item">
            <span class="order-item__name">${escapeHtml(item.product_name)}${item.unit ? `（${escapeHtml(item.unit)}）` : ""}</span>
            <span class="order-item__qty">× ${Number(item.quantity || 0)}</span>
            <span class="order-item__price">${money(item.subtotal)}</span>
          </li>
        `,
          )
          .join("");

        // 状态文案直接用服务端的 status_display，不再维护一份本地映射表——
        // Django 的订单状态机有 10 个取值，本地表早晚会漏。
        const statusText = escapeHtml(order.status_display || order.status);
        const orderId = escapeHtml(order.id);
        const due = Number(order.amount_due || 0);
        const canPay = due > 0;
        const canCancel = ["pending_payment", "pending_deposit", "pending_balance"].includes(
          order.status,
        );

        return `
        <article class="order-card">
          <div class="order-card__top">
            <strong>${escapeHtml(order.order_number)}</strong>
            <span class="order-card__status order-card__status--${escapeHtml(order.status)}">${statusText}</span>
          </div>
          <div class="order-card__meta">
            ${escapeHtml(order.recipient_name)} · ${escapeHtml(order.recipient_phone)}<br />
            金额 ${money(order.total_amount)} · 应付 ${money(order.amount_due)} · ${escapeHtml(order.payment_mode_display || "")}<br />
            ${new Date(order.created_at).toLocaleString("zh-CN")}
          </div>
          <details class="order-card__items">
            <summary>查看商品明细</summary>
            <ul>${items || "<li class='order-item'>无商品明细</li>"}</ul>
          </details>
          ${
            canPay || canCancel
              ? `<div class="order-card__actions">
                   ${canPay ? `<button class="app-nav__button app-nav__button--primary" type="button" data-pay-order="${orderId}">${escapeHtml(order.payment_action_label || "支付")}</button>` : ""}
                   ${canCancel ? `<button class="app-nav__button" type="button" data-cancel-order="${orderId}">取消订单</button>` : ""}
                 </div>`
              : ""
          }
        </article>
      `;
      })
      .join("");
  }

  async function loadOrders() {
    if (!state.session) return;
    try {
      // 契约层这里返回的是**数组本身**（`data` 就是 `[order, ...]`），不是 `{orders: []}`。
      const data = await request("/api/orders");
      state.buyerBlocked = false;
      renderOrders(Array.isArray(data) ? data : []);
    } catch (error) {
      if (error.status === 403) {
        state.buyerBlocked = true;
        renderOrders([]);
        return;
      }
      showToast(error.message, true);
    }
  }

  async function loadProducts() {
    $("storeMessage").textContent = "正在加载商品...";
    try {
      const data = await request("/api/products");
      // 契约层把列表放在 `data.items`（不是 `data.products`），并且带 `count`。
      state.products = Array.isArray(data.items) ? data.items : [];
      renderProducts();
      $("storeMessage").textContent = state.products.length
        ? "选择商品加入购物车，再到购物车页结算。"
        : "当前没有在售商品。";
    } catch (error) {
      $("storeMessage").textContent = error.message;
      showToast(error.message, true);
    }
  }

  // 契约层的购物车**只能装同一果园供货批次**的商品，跨批次加购会 400。
  // 实测文案：`一次只能结算同一果园供货批次，请先完成或清空当前购物车`。
  // 后端这条文案已经可执行，但在商城页是个软死路（用户得自己去侧栏逐个删），
  // 所以这里先做一次客户端预判，给出**明确到批次号**的确认框，同意就清空后重加。
  function cartBatchCode() {
    const owner = state.cartItems.find((item) => item.product?.sales_batch?.code);
    return owner?.product?.sales_batch?.code || "";
  }

  async function clearCart() {
    for (const item of [...state.cartItems]) {
      await request(`/api/cart/${encodeURIComponent(item.id)}`, { method: "DELETE" });
    }
    await loadCart();
  }

  async function addToCart(productId, quantity) {
    await request("/api/cart", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ product_id: productId, quantity }),
    });
    await loadCart();
  }

  async function setCartQuantity(itemId, quantity) {
    if (quantity <= 0) {
      await request(`/api/cart/${encodeURIComponent(itemId)}`, { method: "DELETE" });
    } else {
      await request(`/api/cart/${encodeURIComponent(itemId)}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ quantity }),
      });
    }
    await loadCart();
  }

  async function payOrder(orderId) {
    try {
      // 支付无需请求体；服务端按 payment_mode 决定是付全款还是补尾款。
      await request(`/api/orders/${encodeURIComponent(orderId)}/pay`, { method: "POST" });
      showToast("支付成功");
      await loadOrders();
    } catch (error) {
      showToast(error.message, true);
    }
  }

  async function cancelOrder(orderId) {
    if (!window.confirm("确定取消这笔订单吗？取消后库存会退回。")) return;
    try {
      await request(`/api/orders/${encodeURIComponent(orderId)}/cancel`, { method: "POST" });
      showToast("订单已取消");
      await loadOrders();
    } catch (error) {
      showToast(error.message, true);
    }
  }

  async function logout() {
    try {
      // 契约层恒 200（无 token / 已过期也算成功），所以这里不会因为「重复登出」而报错。
      await request("/web/session/logout", { method: "POST" });
    } catch {
      /* 会话可能已过期 */
    }
    state.session = null;
    state.cartItems = [];
    state.buyerBlocked = false;
    renderSession();
    renderOrders([]);
    showToast("已退出登录");
  }

  function bindEvents() {
    $("searchForm").addEventListener("submit", (event) => {
      event.preventDefault();
      renderProducts();
    });
    $("productSearch").addEventListener("input", renderProducts);
    $("productSort").addEventListener("change", renderProducts);
    $("refreshButton").addEventListener("click", loadProducts);
    $("refreshOrdersButton").addEventListener("click", loadOrders);
    $("logoutButton").addEventListener("click", logout);

    $("productList").addEventListener("click", async (event) => {
      const button = event.target.closest("[data-add-product]");
      if (!button) return;
      if (!state.session) {
        showToast("请先登录后再加入购物车", true);
        return;
      }
      if (state.buyerBlocked) {
        showToast("购物车仅对购买者账号开放", true);
        return;
      }
      const productId = button.dataset.addProduct;
      const product = state.products.find((p) => p.id === productId);
      const stock = Math.max(0, Number(product?.stock || 0));
      if (product && !productIsAvailable(product)) {
        showToast("该商品暂时售罄", true);
        return;
      }
      const input = document.querySelector(`input[data-product-id="${CSS.escape(productId)}"]`);
      const raw = Number(input?.value || 1);
      const quantity = Math.max(1, Number.isFinite(raw) ? raw : 1);
      if (stock > 0 && quantity > stock) {
        showToast("超出库存数量", true);
        return;
      }
      try {
        const currentBatch = cartBatchCode();
        const targetBatch = product?.sales_batch?.code || "";
        if (currentBatch && targetBatch && currentBatch !== targetBatch) {
          const agreed = window.confirm(
            `购物车已有「${currentBatch}」批次的商品，一次只能结算同一批次。\n是否清空购物车后加入「${targetBatch}」批次？`,
          );
          if (!agreed) return;
          await clearCart();
        }
        await addToCart(productId, quantity);
        showToast(`已加入 ${quantity} 件 ${product?.name ?? "商品"}，去购物车结算`);
      } catch (error) {
        showToast(error.message, true);
      }
    });

    // 侧栏购物车摘要：数量增减与移除（现在都打到服务端，本地不再留状态）
    $("cartSummary").addEventListener("click", async (event) => {
      const dec = event.target.closest("[data-dec-item]");
      if (dec) {
        const item = state.cartItems.find((i) => i.id === dec.dataset.decItem);
        if (!item) return;
        try {
          await setCartQuantity(item.id, Number(item.quantity) - 1);
        } catch (error) {
          showToast(error.message, true);
        }
        return;
      }
      const inc = event.target.closest("[data-inc-item]");
      if (inc) {
        const item = state.cartItems.find((i) => i.id === inc.dataset.incItem);
        if (!item) return;
        const stock = Math.max(0, Number(item.product?.stock || 0));
        if (Number(item.quantity) + 1 > stock) {
          showToast("已达库存上限", true);
          return;
        }
        try {
          await setCartQuantity(item.id, Number(item.quantity) + 1);
        } catch (error) {
          showToast(error.message, true);
        }
        return;
      }
      const remove = event.target.closest("[data-remove-item]");
      if (remove) {
        try {
          await setCartQuantity(remove.dataset.removeItem, 0);
          showToast("已移除该商品");
        } catch (error) {
          showToast(error.message, true);
        }
      }
    });

    // 订单卡片里的支付 / 取消（按钮由 renderOrders 注入，页面 HTML 无需改动）
    $("ordersList").addEventListener("click", async (event) => {
      const pay = event.target.closest("[data-pay-order]");
      if (pay) {
        await payOrder(pay.dataset.payOrder);
        return;
      }
      const cancel = event.target.closest("[data-cancel-order]");
      if (cancel) {
        await cancelOrder(cancel.dataset.cancelOrder);
      }
    });
  }

  function dropLegacyCart() {
    // 旧购物车存的是整数 id（`store_products`），契约表是 UUID，无可靠映射 → 直接丢弃。
    try {
      localStorage.removeItem(LEGACY_CART_KEY);
    } catch {
      /* localStorage 可能不可用，忽略 */
    }
  }

  async function init() {
    dropLegacyCart();
    bindEvents();
    await validateSession();
    await loadProducts();
    await loadCart();
    await loadOrders();
  }

  init();
})();
