(() => {
  const state = { products: [], cart: {}, session: null };
  const $ = (id) => document.getElementById(id);

  const statusLabels = {
    pending_payment: "待收款",
    paid: "已付款",
    shipped: "已发货",
    completed: "已完成",
    cancelled: "已取消",
    refunded: "已退款",
  };

  function escapeHtml(value) {
    return String(value ?? "")
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#039;");
  }

  function money(cents) {
    return `¥${(Number(cents || 0) / 100).toFixed(2)}`;
  }

  function cookieToken() {
    const item = document.cookie.split(";").map((part) => part.trim()).find((part) => part.startsWith("session_token="));
    return item ? decodeURIComponent(item.slice("session_token=".length)) : "";
  }

  async function request(url, options = {}) {
    const headers = new Headers(options.headers || {});
    const token = cookieToken();
    if (token) headers.set("X-Session-Token", token);
    const response = await fetch(url, { ...options, headers, credentials: "same-origin" });
    const body = await response.json().catch(() => ({}));
    if (!response.ok) {
      throw new Error(body.message || body.error || body.data?.message || "请求失败");
    }
    return body.data ?? body;
  }

  function showToast(message, isError = false) {
    const toast = $("toast");
    toast.textContent = message;
    toast.classList.toggle("is-error", isError);
    toast.classList.add("is-visible");
    window.clearTimeout(showToast.timer);
    showToast.timer = window.setTimeout(() => toast.classList.remove("is-visible"), 3200);
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
    renderCart();
  }

  async function validateSession() {
    try {
      const data = await request("/user/validate", { method: "POST" });
      state.session = data.valid ? data : null;
    } catch {
      state.session = null;
    }
    renderSession();
  }

  function renderProducts() {
    const list = $("productList");
    $("productCount").textContent = state.products.length;
    if (!state.products.length) {
      list.innerHTML = '<div class="empty-block">当前没有在售商品，请稍后再来看看。</div>';
      return;
    }

    list.innerHTML = state.products.map((product) => {
      const soldOut = product.stock_quantity <= 0;
      return `
        <article class="product-card">
          <div>
            <h3 class="product-card__name">${escapeHtml(product.name)}</h3>
            <div class="product-card__unit">${escapeHtml(product.unit_label)}</div>
          </div>
          <p class="product-card__desc">${escapeHtml(product.description || "赣南产地精选脐橙，现货直发。")}</p>
          <div class="product-card__price">${money(product.price_cents)}</div>
          <div class="product-card__stock">${soldOut ? "暂时售罄" : `库存 ${product.stock_quantity} 件`}</div>
          <div class="product-card__buy">
            <input type="number" min="1" max="${Math.max(1, product.stock_quantity)}" value="1" data-product-id="${product.id}" aria-label="${escapeHtml(product.name)}数量" ${soldOut ? "disabled" : ""} />
            <button class="app-nav__button app-nav__button--primary" type="button" data-add-product="${product.id}" ${soldOut ? "disabled" : ""}>加入购物车</button>
          </div>
        </article>
      `;
    }).join("");
  }

  function renderCart() {
    const entries = Object.entries(state.cart)
      .map(([productId, quantity]) => {
        const product = state.products.find((p) => p.id === Number(productId));
        return product ? { product, quantity } : null;
      })
      .filter(Boolean);
    const loggedIn = Boolean(state.session?.username);
    const count = entries.reduce((sum, entry) => sum + entry.quantity, 0);
    $("cartStatus").textContent = count ? `${count} 件` : "空";
    $("cartEmpty").hidden = entries.length > 0;
    $("orderForm").hidden = entries.length === 0;
    if (!entries.length) return;

    $("cartItems").innerHTML = entries.map(({ product, quantity }) => `
      <div class="cart-item">
        <div class="cart-item__info">
          <strong>${escapeHtml(product.name)}</strong>
          <small>${escapeHtml(product.unit_label)} · 单价 ${money(product.price_cents)}</small>
        </div>
        <div class="cart-item__qty">
          <button type="button" data-dec-product="${product.id}" aria-label="减少">−</button>
          <span>${quantity}</span>
          <button type="button" data-inc-product="${product.id}" aria-label="增加">+</button>
        </div>
        <div class="cart-item__price">${money(product.price_cents * quantity)}</div>
      </div>
    `).join("");

    let total = 0;
    entries.forEach(({ product, quantity }) => {
      total += product.price_cents * quantity;
    });
    $("orderTotal").textContent = money(total);
    $("loginNote").hidden = loggedIn;
    $("submitOrderButton").disabled = !loggedIn;
  }

  function renderOrders(orders) {
    const list = $("ordersList");
    if (!orders.length) {
      list.innerHTML = state.session ? '<div class="empty-block">还没有订单。</div>' : "";
      return;
    }
    list.innerHTML = orders.map((order) => `
      <article class="order-card">
        <div class="order-card__top">
          <strong>${escapeHtml(order.order_no)}</strong>
          <span class="order-card__status">${statusLabels[order.status] || escapeHtml(order.status)}</span>
        </div>
        <div class="order-card__meta">
          ${escapeHtml(order.recipient_name)} · ${escapeHtml(order.recipient_phone)}<br />
          金额 ${money(order.total_cents)} · ${new Date(Number(order.created_at)).toLocaleString("zh-CN")}
        </div>
      </article>
    `).join("");
  }

  async function loadOrders() {
    if (!state.session) return;
    try {
      const data = await request("/user/store/orders");
      renderOrders(Array.isArray(data.orders) ? data.orders : []);
    } catch (error) {
      showToast(error.message, true);
    }
  }

  async function loadProducts() {
    $("storeMessage").textContent = "正在加载商品...";
    try {
      const data = await request("/api/store/products");
      state.products = Array.isArray(data.products) ? data.products : [];
      renderProducts();
      renderCart();
      $("storeMessage").textContent = state.products.length ? "选择商品加入购物车，填写收货信息后提交订单。" : "当前没有在售商品。";
    } catch (error) {
      $("storeMessage").textContent = error.message;
      showToast(error.message, true);
    }
  }

  async function submitOrder(event) {
    event.preventDefault();
    if (!state.session) {
      window.location.href = "/";
      return;
    }
    const items = Object.entries(state.cart)
      .map(([productId, quantity]) => ({ product_id: Number(productId), quantity }))
      .filter((item) => item.quantity > 0);
    if (!items.length) {
      showToast("请至少选择一件商品", true);
      return;
    }

    const button = $("submitOrderButton");
    button.disabled = true;
    button.textContent = "提交中...";
    try {
      const data = await request("/user/store/orders", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          recipient_name: $("recipientName").value.trim(),
          recipient_phone: $("recipientPhone").value.trim(),
          shipping_address: $("shippingAddress").value.trim(),
          items,
        }),
      });
      showToast(`订单已提交：${data.order_no}`);
      state.cart = {};
      $("orderForm").reset();
      await Promise.all([loadProducts(), loadOrders()]);
      renderCart();
    } catch (error) {
      showToast(error.message, true);
    } finally {
      button.disabled = !state.session;
      button.textContent = "提交订单";
    }
  }

  async function logout() {
    try { await request("/user/logout", { method: "POST" }); } catch { /* session may already be expired */ }
    state.session = null;
    renderSession();
    renderOrders([]);
    showToast("已退出登录");
  }

  function bindEvents() {
    $("refreshButton").addEventListener("click", loadProducts);
    $("refreshOrdersButton").addEventListener("click", loadOrders);
    $("orderForm").addEventListener("submit", submitOrder);
    $("logoutButton").addEventListener("click", logout);

    $("productList").addEventListener("click", (event) => {
      const button = event.target.closest("[data-add-product]");
      if (!button) return;
      const productId = Number(button.dataset.addProduct);
      const input = document.querySelector(`input[data-product-id="${button.dataset.addProduct}"]`);
      const quantity = Math.max(1, Number(input?.value || 1));
      const product = state.products.find((p) => p.id === productId);
      const max = Math.max(0, product?.stock_quantity ?? 0);
      state.cart[productId] = Math.min(max, (state.cart[productId] || 0) + quantity);
      if (state.cart[productId] <= 0) delete state.cart[productId];
      renderCart();
    });

    $("cartItems").addEventListener("click", (event) => {
      const dec = event.target.closest("[data-dec-product]");
      if (dec) {
        const id = Number(dec.dataset.decProduct);
        state.cart[id] = Math.max(0, (state.cart[id] || 0) - 1);
        if (state.cart[id] <= 0) delete state.cart[id];
        renderCart();
        return;
      }
      const inc = event.target.closest("[data-inc-product]");
      if (inc) {
        const id = Number(inc.dataset.incProduct);
        const product = state.products.find((p) => p.id === id);
        const max = Math.max(0, product?.stock_quantity ?? 0);
        state.cart[id] = Math.min(max, (state.cart[id] || 0) + 1);
        if (state.cart[id] <= 0) delete state.cart[id];
        renderCart();
      }
    });
  }

  async function init() {
    bindEvents();
    await validateSession();
    await loadProducts();
    await loadOrders();
  }

  init();
})();
