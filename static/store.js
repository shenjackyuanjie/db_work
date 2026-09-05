(() => {
  const state = { products: [], cart: {}, session: null };
  const $ = (id) => document.getElementById(id);
  const CART_KEY = "store_cart_v1";

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

  function loadCart() {
    try {
      const parsed = JSON.parse(localStorage.getItem(CART_KEY) || "{}");
      state.cart = parsed && typeof parsed === "object" ? parsed : {};
    } catch {
      state.cart = {};
    }
  }

  function saveCart() {
    try {
      localStorage.setItem(CART_KEY, JSON.stringify(state.cart));
    } catch {
      /* localStorage 可能不可用，忽略 */
    }
  }

  // 仅保留仍可购买的商品，并裁剪超出库存的数量。
  function pruneCart() {
    for (const [id, quantity] of Object.entries(state.cart)) {
      const product = state.products.find((p) => p.id === Number(id));
      if (!product || product.stock_quantity <= 0 || quantity <= 0) {
        delete state.cart[id];
      } else if (quantity > product.stock_quantity) {
        state.cart[id] = product.stock_quantity;
      }
    }
    saveCart();
  }

  // 只允许展示可信的封面来源，避免注入任意属性/URL。
  function coverImageSrc(value) {
    const src = String(value || "").trim();
    if (/^(https?:\/\/|\/store-images\/|\/uploads\/)/.test(src)) return src;
    return "";
  }

  function cartEntries() {
    return Object.entries(state.cart)
      .map(([productId, quantity]) => {
        const product = state.products.find((p) => p.id === Number(productId));
        return product ? { product, quantity } : null;
      })
      .filter(Boolean);
  }

  async function request(url, options = {}) {
    const headers = new Headers(options.headers || {});
    const response = await fetch(url, {
      ...options,
      headers,
      credentials: "same-origin",
    });
    const body = await response.json().catch(() => ({}));
    if (!response.ok) {
      throw new Error(
        body.message || body.error || body.data?.message || "请求失败",
      );
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
      const data = await request("/user/validate", { method: "POST" });
      state.session = data.valid ? data : null;
    } catch {
      state.session = null;
    }
    renderSession();
  }

  function renderCartSummary() {
    const entries = cartEntries();
    const count = entries.reduce((sum, entry) => sum + entry.quantity, 0);
    const subtotal = entries.reduce(
      (sum, entry) => sum + entry.product.price_cents * entry.quantity,
      0,
    );
    const badge = $("cartNavCount");
    badge.hidden = count === 0;
    badge.textContent = count > 99 ? "99+" : String(count);

    $("cartStatus").textContent = count ? `${count} 件` : "空";
    if (!entries.length) {
      $("cartSummary").innerHTML =
        '<div class="empty-block">还没有选择商品，快去挑选吧。</div>';
      return;
    }
    $("cartSummary").innerHTML = `
      <div class="mini-cart-list">
        ${entries
          .map(
            ({ product, quantity }) => `
          <div class="mini-cart-item">
            <div class="mini-cart-item__info">
              <strong>${escapeHtml(product.name)}</strong>
              <small>${escapeHtml(product.unit_label)} · ${money(product.price_cents)}</small>
            </div>
            <div class="mini-cart-item__qty">
              <button type="button" data-dec-product="${product.id}" aria-label="减少">−</button>
              <span>${quantity}</span>
              <button type="button" data-inc-product="${product.id}" aria-label="增加">+</button>
            </div>
            <div class="mini-cart-item__line">${money(product.price_cents * quantity)}</div>
            <button class="mini-cart-item__remove" type="button" data-remove-product="${product.id}" aria-label="移除">×</button>
          </div>
        `,
          )
          .join("")}
      </div>
      <div class="cart-summary-total">
        <span>合计</span><strong>${money(subtotal)}</strong>
      </div>
    `;
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
    const products = state.products.filter((p) =>
      `${p.name} ${p.description} ${p.unit_label}`
        .toLowerCase()
        .includes(query),
    );
    const sort = $("productSort").value;
    if (sort === "price-asc")
      products.sort((a, b) => a.price_cents - b.price_cents);
    if (sort === "price-desc")
      products.sort((a, b) => b.price_cents - a.price_cents);
    if (sort === "stock")
      products.sort((a, b) => b.stock_quantity - a.stock_quantity);
    if (!products.length) {
      list.innerHTML =
        '<div class="empty-block">没有找到匹配商品，试试其他关键词。</div>';
      return;
    }
    list.innerHTML = products
      .map((product) => {
        const soldOut = product.stock_quantity <= 0;
        const coverSrc = coverImageSrc(product.cover_image);
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
      })
      .join("");
  }

  function renderOrders(orders) {
    const list = $("ordersList");
    if (!orders.length) {
      list.innerHTML = state.session
        ? '<div class="empty-block">还没有订单。</div>'
        : "";
      return;
    }
    list.innerHTML = orders
      .map((order) => {
        const items = (Array.isArray(order.items) ? order.items : [])
          .map((item) => {
            const lineTotal =
              item.line_total_cents ?? item.unit_price_cents * item.quantity;
            return `
          <li class="order-item">
            <span class="order-item__name">${escapeHtml(item.product_name)}${item.unit_label ? `（${escapeHtml(item.unit_label)}）` : ""}</span>
            <span class="order-item__qty">× ${item.quantity}</span>
            <span class="order-item__price">${money(lineTotal)}</span>
          </li>
        `;
          })
          .join("");
        const statusText =
          statusLabels[order.status] || escapeHtml(order.status);
        return `
        <article class="order-card">
          <div class="order-card__top">
            <strong>${escapeHtml(order.order_no)}</strong>
            <span class="order-card__status order-card__status--${escapeHtml(order.status)}">${statusText}</span>
          </div>
          <div class="order-card__meta">
            ${escapeHtml(order.recipient_name)} · ${escapeHtml(order.recipient_phone)}<br />
            金额 ${money(order.total_cents)} · ${new Date(Number(order.created_at)).toLocaleString("zh-CN")}
          </div>
          <details class="order-card__items">
            <summary>查看商品明细</summary>
            <ul>${items || "<li class='order-item'>无商品明细</li>"}</ul>
          </details>
        </article>
      `;
      })
      .join("");
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
      pruneCart();
      renderProducts();
      renderCartSummary();
      $("storeMessage").textContent = state.products.length
        ? "选择商品加入购物车，再到购物车页结算。"
        : "当前没有在售商品。";
    } catch (error) {
      $("storeMessage").textContent = error.message;
      showToast(error.message, true);
    }
  }

  function setCartQuantity(productId, quantity) {
    const product = state.products.find((p) => p.id === productId);
    const stock = Math.max(0, product?.stock_quantity ?? 0);
    const next = Math.min(stock, Math.max(0, quantity));
    if (next <= 0) {
      delete state.cart[productId];
    } else {
      state.cart[productId] = next;
    }
    saveCart();
    renderCartSummary();
  }

  async function logout() {
    try {
      await request("/user/logout", { method: "POST" });
    } catch {
      /* 会话可能已过期 */
    }
    state.session = null;
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

    $("productList").addEventListener("click", (event) => {
      const button = event.target.closest("[data-add-product]");
      if (!button) return;
      const productId = Number(button.dataset.addProduct);
      const product = state.products.find((p) => p.id === productId);
      const stock = Math.max(0, product?.stock_quantity ?? 0);
      if (stock <= 0) {
        showToast("该商品暂时售罄", true);
        return;
      }
      const input = document.querySelector(
        `input[data-product-id="${button.dataset.addProduct}"]`,
      );
      const raw = Number(input?.value || 1);
      const quantity = Math.min(
        Math.max(1, Number.isFinite(raw) ? raw : 1),
        stock,
      );
      const current = state.cart[productId] || 0;
      setCartQuantity(productId, Math.min(stock, current + quantity));
      showToast(`已加入 ${quantity} 件 ${product.name}，去购物车结算`);
    });

    // 侧栏购物车摘要：数量增减与移除
    $("cartSummary").addEventListener("click", (event) => {
      const dec = event.target.closest("[data-dec-product]");
      if (dec) {
        const id = Number(dec.dataset.decProduct);
        setCartQuantity(id, (state.cart[id] || 0) - 1);
        return;
      }
      const inc = event.target.closest("[data-inc-product]");
      if (inc) {
        const id = Number(inc.dataset.incProduct);
        const product = state.products.find((p) => p.id === id);
        const stock = Math.max(0, product?.stock_quantity ?? 0);
        const next = (state.cart[id] || 0) + 1;
        if (next > stock) {
          showToast("已达库存上限", true);
          return;
        }
        setCartQuantity(id, next);
        return;
      }
      const remove = event.target.closest("[data-remove-product]");
      if (remove) {
        const id = Number(remove.dataset.removeProduct);
        delete state.cart[id];
        saveCart();
        renderCartSummary();
        showToast("已移除该商品");
      }
    });
  }

  async function init() {
    loadCart();
    bindEvents();
    await validateSession();
    await loadProducts();
    await loadOrders();
  }

  init();
})();
