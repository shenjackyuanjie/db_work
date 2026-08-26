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
    $("loginNote").hidden = loggedIn;
    $("submitOrderButton").disabled = !loggedIn;
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

  function updateNavCount() {
    const count = cartEntries().reduce((sum, entry) => sum + entry.quantity, 0);
    const badge = $("cartNavCount");
    badge.hidden = count === 0;
    badge.textContent = count > 99 ? "99+" : String(count);
  }

  function render() {
    const entries = cartEntries();
    const isEmpty = entries.length === 0;
    $("cartEmpty").hidden = !isEmpty;
    $("cartLayout").hidden = isEmpty;
    $("cartMessage").hidden = !isEmpty;
    updateNavCount();
    if (isEmpty) {
      $("cartMessage").textContent = "购物车为空，去挑选几款商品吧。";
      return;
    }

    $("cartMessage").textContent = "请核对商品与数量，填写收货信息后提交订单。";

    $("cartItems").innerHTML = entries.map(({ product, quantity }) => {
      const coverSrc = coverImageSrc(product.cover_image);
      const lineTotal = product.price_cents * quantity;
      return `
        <div class="cart-item">
          <div class="cart-item__product">
            <span class="cart-item__thumb">
              ${coverSrc
                ? `<img src="${escapeHtml(coverSrc)}" alt="${escapeHtml(product.name)}" />`
                : '<span class="cart-item__thumb-mark">橙</span>'}
            </span>
            <span class="cart-item__info">
              <strong>${escapeHtml(product.name)}</strong>
              <small>${escapeHtml(product.unit_label)}</small>
            </span>
          </div>
          <div class="cart-item__unit">${money(product.price_cents)}</div>
          <div class="cart-item__qty">
            <button type="button" data-dec-product="${product.id}" aria-label="减少">−</button>
            <span>${quantity}</span>
            <button type="button" data-inc-product="${product.id}" aria-label="增加">+</button>
          </div>
          <div class="cart-item__line">${money(lineTotal)}</div>
          <button class="cart-item__remove" type="button" data-remove-product="${product.id}" aria-label="移除">×</button>
        </div>
      `;
    }).join("");

    const qty = entries.reduce((sum, entry) => sum + entry.quantity, 0);
    const subtotal = entries.reduce((sum, entry) => sum + entry.product.price_cents * entry.quantity, 0);
    $("summaryQty").textContent = `${qty} 件`;
    $("summarySubtotal").textContent = money(subtotal);
    $("summaryTotal").textContent = money(subtotal);
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
    render();
  }

  async function loadProducts() {
    try {
      const data = await request("/api/store/products");
      state.products = Array.isArray(data.products) ? data.products : [];
      pruneCart();
      render();
    } catch (error) {
      $("cartMessage").textContent = error.message;
      showToast(error.message, true);
    }
  }

  async function submitOrder(event) {
    event.preventDefault();
    if (!state.session) {
      window.location.href = "/";
      return;
    }
    const items = cartEntries()
      .map(({ product, quantity }) => ({
        product_id: product.id,
        // 提交前再次按最新库存裁剪，避免超出库存。
        quantity: Math.min(quantity, Math.max(0, product.stock_quantity ?? 0)),
      }))
      .filter((item) => item.quantity > 0);
    if (!items.length) {
      showToast("请至少选择一件商品", true);
      return;
    }

    const button = $("submitOrderButton");
    button.disabled = true;
    const label = button.textContent;
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
      saveCart();
      $("checkoutForm").reset();
      await loadProducts();
    } catch (error) {
      showToast(error.message, true);
    } finally {
      button.disabled = !state.session;
      button.textContent = label;
    }
  }

  async function logout() {
    try {
      await request("/user/logout", { method: "POST" });
    } catch {
      /* 会话可能已过期 */
    }
    state.session = null;
    renderSession();
    showToast("已退出登录");
  }

  function bindEvents() {
    $("logoutButton").addEventListener("click", logout);
    $("checkoutForm").addEventListener("submit", submitOrder);

    $("cartItems").addEventListener("click", (event) => {
      const dec = event.target.closest("[data-dec-product]");
      if (dec) {
        setCartQuantity(Number(dec.dataset.decProduct), (state.cart[dec.dataset.decProduct] || 0) - 1);
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
        render();
      }
    });
  }

  async function init() {
    loadCart();
    bindEvents();
    await validateSession();
    await loadProducts();
  }

  init();
})();
