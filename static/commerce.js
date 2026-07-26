(() => {
  const state = { batches: [], selectedBatch: null, session: null };
  const $ = (id) => document.getElementById(id);

  const statusLabels = {
    pending_payment: "待收款",
    paid: "已付款",
    confirmed: "已确认",
    harvesting: "采摘中",
    packing: "分选装箱",
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

  function dateLabel(timestamp) {
    if (!timestamp) return "待安排";
    const date = new Date(Number(timestamp));
    return Number.isNaN(date.getTime()) ? "待安排" : date.toLocaleDateString("zh-CN");
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
    if (state.selectedBatch) renderOrderPanel();
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

  function renderBatches() {
    const list = $("batchList");
    $("batchCount").textContent = state.batches.length;
    if (!state.batches.length) {
      list.innerHTML = '<div class="empty-block">当前没有开放中的批次，请稍后再来看看。</div>';
      return;
    }

    list.innerHTML = state.batches.map((batch) => {
      const orchard = batch.orchard || {};
      const products = Array.isArray(batch.products) ? batch.products : [];
      return `
        <article class="batch-card">
          <div class="batch-card__top">
            <div>
              <span class="batch-code">${escapeHtml(batch.batch_code)}</span>
              <h3>${escapeHtml(batch.title)}</h3>
            </div>
            <span class="panel-status">${batch.status === "preorder" ? "预售中" : "开团中"}</span>
          </div>
          <div class="batch-card__orchard">${escapeHtml(orchard.name || "合作果园")}</div>
          <p class="batch-card__description">${escapeHtml(orchard.description || "赣南产地合作果园，按订单组织采摘和发货。")}</p>
          <div class="batch-card__meta">
            <span>产地 ${escapeHtml(orchard.location || "赣南")}</span>
            <span>预计采摘 ${dateLabel(batch.harvest_start_at)}</span>
            <span>预计发货 ${dateLabel(batch.ship_at)}</span>
          </div>
          <div class="product-list">
            ${products.length ? products.map((product) => `
              <div class="product-row">
                <div><strong>${escapeHtml(product.name)}</strong><small>${escapeHtml(product.unit_label)} · 剩余 ${product.remaining_quantity} 份</small></div>
                <span class="product-price">${money(product.price_cents)}</span>
              </div>
            `).join("") : '<div class="empty-block">该批次暂未配置商品规格。</div>'}
          </div>
          <div class="batch-card__footer">
            <small>截团时间：${dateLabel(batch.close_at)}</small>
            <button class="app-nav__button app-nav__button--primary" type="button" data-select-batch="${escapeHtml(batch.id)}">选择此批次</button>
          </div>
        </article>
      `;
    }).join("");

    list.querySelectorAll("[data-select-batch]").forEach((button) => {
      button.addEventListener("click", () => selectBatch(button.dataset.selectBatch));
    });
  }

  function selectBatch(batchId) {
    state.selectedBatch = state.batches.find((batch) => batch.id === batchId) || null;
    renderOrderPanel();
    $("orderTitle").scrollIntoView({ behavior: "smooth", block: "start" });
  }

  function renderOrderPanel() {
    const batch = state.selectedBatch;
    $("orderEmpty").hidden = Boolean(batch);
    $("orderForm").hidden = !batch;
    if (!batch) return;

    const orchard = batch.orchard || {};
    $("orderStatus").textContent = state.session ? "可提交预订" : "登录后下单";
    $("selectedBatchSummary").innerHTML = `<strong>${escapeHtml(batch.title)}</strong><small>${escapeHtml(orchard.name || "合作果园")} · 预计发货 ${dateLabel(batch.ship_at)}</small>`;
    $("orderItems").innerHTML = (batch.products || []).map((product) => `
      <div class="order-item">
        <div><strong>${escapeHtml(product.name)}</strong><small>${escapeHtml(product.unit_label)} · ${money(product.price_cents)} · 剩余 ${product.remaining_quantity}</small></div>
        <input type="number" min="0" max="${Math.max(0, product.remaining_quantity)}" value="0" data-product-id="${product.id}" data-price-cents="${product.price_cents}" data-deposit-cents="${product.deposit_cents}" aria-label="${escapeHtml(product.name)}数量" />
      </div>
    `).join("");
    $("loginNote").hidden = Boolean(state.session);
    $("submitOrderButton").disabled = !state.session;
    $("orderItems").querySelectorAll("input").forEach((input) => input.addEventListener("input", updateTotal));
    updateTotal();
  }

  function updateTotal() {
    let total = 0;
    let deposit = 0;
    $("orderItems").querySelectorAll("input").forEach((input) => {
      const quantity = Math.max(0, Number(input.value || 0));
      total += quantity * Number(input.dataset.priceCents || 0);
      deposit += quantity * Number(input.dataset.depositCents || 0);
    });
    $("orderTotal").textContent = money(total);
    $("depositHint").textContent = deposit > 0 ? `本批次预计订金：${money(deposit)}，等待管理员确认收款` : "下单后等待管理员确认收款";
  }

  function renderOrders(orders) {
    const list = $("ordersList");
    if (!orders.length) {
      list.innerHTML = state.session ? '<div class="empty-block">还没有订单。</div>' : "";
      return;
    }
    list.innerHTML = orders.map((order) => `
      <article class="order-card">
        <div class="order-card__top"><strong>${escapeHtml(order.order_no)}</strong><span class="order-card__status">${statusLabels[order.status] || escapeHtml(order.status)}</span></div>
        <div class="order-card__meta">${escapeHtml(order.recipient_name)} · ${escapeHtml(order.recipient_phone)}<br />金额 ${money(order.total_cents)} · ${new Date(Number(order.created_at)).toLocaleString("zh-CN")}</div>
      </article>
    `).join("");
  }

  async function loadOrders() {
    if (!state.session) return;
    try {
      const data = await request("/user/commerce/orders");
      renderOrders(Array.isArray(data.orders) ? data.orders : []);
    } catch (error) {
      showToast(error.message, true);
    }
  }

  async function loadStorefront() {
    $("storefrontMessage").textContent = "正在加载开团批次...";
    try {
      const data = await request("/api/commerce/storefront");
      state.batches = Array.isArray(data.batches) ? data.batches : [];
      renderBatches();
      $("storefrontMessage").textContent = state.batches.length ? "选择一个批次查看规格并提交预订。" : "当前没有开放中的批次。";
    } catch (error) {
      $("storefrontMessage").textContent = error.message;
      showToast(error.message, true);
    }
  }

  async function submitOrder(event) {
    event.preventDefault();
    if (!state.session) {
      window.location.href = "/";
      return;
    }
    const items = [...$("orderItems").querySelectorAll("input")]
      .map((input) => ({ product_id: Number(input.dataset.productId), quantity: Number(input.value || 0) }))
      .filter((item) => item.quantity > 0);
    if (!items.length) {
      showToast("请至少选择一件商品", true);
      return;
    }

    const button = $("submitOrderButton");
    button.disabled = true;
    button.textContent = "提交中...";
    try {
      const data = await request("/user/commerce/orders", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          batch_id: state.selectedBatch.id,
          recipient_name: $("recipientName").value.trim(),
          recipient_phone: $("recipientPhone").value.trim(),
          shipping_address: $("shippingAddress").value.trim(),
          items,
        }),
      });
      showToast(`预订已提交：${data.order_no}`);
      $("orderForm").reset();
      await Promise.all([loadStorefront(), loadOrders()]);
      if (state.selectedBatch) {
        state.selectedBatch = state.batches.find((batch) => batch.id === state.selectedBatch.id) || null;
        renderOrderPanel();
      }
    } catch (error) {
      showToast(error.message, true);
    } finally {
      button.disabled = !state.session;
      button.textContent = "提交预订";
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
    $("refreshButton").addEventListener("click", loadStorefront);
    $("refreshOrdersButton").addEventListener("click", loadOrders);
    $("orderForm").addEventListener("submit", submitOrder);
    $("logoutButton").addEventListener("click", logout);
  }

  async function init() {
    bindEvents();
    await validateSession();
    await loadStorefront();
    await loadOrders();
  }

  init();
})();
