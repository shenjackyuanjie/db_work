// 购物车页：服务端购物车 + 提交订单。
//
// ## 这一版换了什么（W2 商城收敛）
//
// | 项 | 旧 | 新 |
// |---|---|---|
// | 购物车 | `localStorage["store_cart_v1"]`（整数 id + `price_cents` 分） | 服务端 `/api/cart`（契约表 `cart_item`，UUID + `price` 十进制**字符串**） |
// | 商品 | 另外拉 `/api/store/products` 再和本地购物车对账 | **不需要**：`/api/cart` 的每一项都带完整 `product` |
// | 下单 | `POST /user/store/orders`，请求体里带 `items[]` | `POST /api/orders {address_id, note}`——**商品由服务端购物车决定**，前端不传明细 |
// | 收货信息 | 三个自由文本字段直接进订单 | 先用这三个字段在 `/api/addresses` **查找或创建**地址，再拿 `address_id` 下单 |
// | 会话 | `/user/validate`、`/user/logout` | `/web/session/validate`、`/web/session/logout` |
//
// ## 两个必须记住的前提
//
// 1. **网页商城是给 buyer 账号用的**：`/api/cart` 与 `/api/orders` 都是 `IsBuyer`。
//    `/web/session/register` 注册出来的是 `role=farmer`，果农登录后调这些接口会拿到
//    **403 `该接口仅限购买者使用`——这是正确行为，不是 bug**。这里把它转成可读提示。
// 2. **认证靠 HttpOnly cookie**，契约层三通道鉴权（Bearer → Cookie `session_token` →
//    `X-Session-Token`），所以前端不需要拿 token，只要 `credentials: "same-origin"`。
//
// 为什么下单要绕一层地址：`cart.html` 的表单是 `recipientName` / `recipientPhone` /
// `shippingAddress` 三个自由文本字段（页面 HTML 不在本次改动范围内），而契约要求
// `address_id`。所以提交时先用这三个值**查找已有地址**（命中则复用，避免每单刷一条新地址），
// 没有再创建，然后下单。
//
// 旧购物车数据按裁定**直接丢弃**（存的是整数 id，契约表是 UUID，无可靠映射）。

(() => {
  const state = { cartItems: [], cartTotalAmount: "0.00", session: null, buyerBlocked: false };
  const $ = (id) => document.getElementById(id);
  const LEGACY_CART_KEY = "store_cart_v1";

  // 契约金额是 `NUMERIC` 字符串（`"45.00"`），不是分。展示走 money()，算术一律先转分。
  function money(value) {
    return `¥${Number(value || 0).toFixed(2)}`;
  }

  function escapeHtml(value) {
    return String(value ?? "")
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#039;");
  }

  // 契约层错误体的 `message` 有两种：字符串（业务错）与对象（DRF 字段校验）。
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
    showToast.timer = window.setTimeout(() => toast.classList.remove("is-visible"), 3200);
  }

  // 隐式静态通道前缀（无 fetch，靠 <img src> 生效），只放行可信来源。
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
    $("loginNote").hidden = loggedIn;
    $("submitOrderButton").disabled = !loggedIn || state.buyerBlocked;
  }

  async function validateSession() {
    try {
      const data = await request("/web/session/validate", { method: "POST" });
      state.session = data.valid ? data : null;
    } catch {
      state.session = null;
    }
    renderSession();
  }

  function updateNavCount() {
    const count = state.cartItems.reduce((sum, item) => sum + Number(item.quantity || 0), 0);
    const badge = $("cartNavCount");
    badge.hidden = count === 0;
    badge.textContent = count > 99 ? "99+" : String(count);
  }

  function render() {
    const items = state.cartItems;
    const isEmpty = items.length === 0;
    $("cartEmpty").hidden = !isEmpty;
    $("cartLayout").hidden = isEmpty;
    $("cartMessage").hidden = !isEmpty;
    updateNavCount();

    if (isEmpty) {
      $("cartMessage").textContent = state.buyerBlocked
        ? "当前账号不是购买者，购物车仅对购买者账号开放。"
        : "购物车为空，去挑选几款商品吧。";
      return;
    }

    $("cartMessage").textContent = "请核对商品与数量，填写收货信息后提交订单。";

    $("cartItems").innerHTML = items
      .map((item) => {
        const product = item.product || {};
        const coverSrc = coverImageSrc(product.cover_image_url);
        const itemId = escapeHtml(item.id);
        return `
        <div class="cart-item">
          <div class="cart-item__product">
            <span class="cart-item__thumb">
              ${
                coverSrc
                  ? `<img src="${escapeHtml(coverSrc)}" alt="${escapeHtml(product.name)}" />`
                  : '<span class="cart-item__thumb-mark">橙</span>'
              }
            </span>
            <span class="cart-item__info">
              <strong>${escapeHtml(product.name)}</strong>
              <small>${escapeHtml(product.unit)}</small>
            </span>
          </div>
          <div class="cart-item__unit">${money(product.price)}</div>
          <div class="cart-item__qty">
            <button type="button" data-dec-item="${itemId}" aria-label="减少">−</button>
            <span>${Number(item.quantity || 0)}</span>
            <button type="button" data-inc-item="${itemId}" aria-label="增加">+</button>
          </div>
          <div class="cart-item__line">${money(item.subtotal)}</div>
          <button class="cart-item__remove" type="button" data-remove-item="${itemId}" aria-label="移除">×</button>
        </div>
      `;
      })
      .join("");

    // 合计直接用服务端给的 totalAmount，不在客户端累加（浮点尾差 + 与库存口径不一致）。
    $("summaryQty").textContent = `${items.reduce((sum, item) => sum + Number(item.quantity || 0), 0)} 件`;
    $("summarySubtotal").textContent = money(state.cartTotalAmount);
    $("summaryTotal").textContent = money(state.cartTotalAmount);
  }

  async function loadCart() {
    if (!state.session) {
      state.cartItems = [];
      state.cartTotalAmount = "0.00";
      render();
      return;
    }
    try {
      const data = await request("/api/cart");
      state.cartItems = Array.isArray(data.items) ? data.items : [];
      state.cartTotalAmount = data.totalAmount || "0.00";
      state.buyerBlocked = false;
    } catch (error) {
      if (error.status === 403) {
        state.buyerBlocked = true;
        state.cartItems = [];
        state.cartTotalAmount = "0.00";
      } else {
        showToast(error.message, true);
      }
    }
    renderSession();
    render();
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

  /// 用表单里的三个字段解析出一个可下单的 `address_id`：命中已有地址就复用，否则新建。
  async function resolveAddressId() {
    const recipientName = $("recipientName").value.trim();
    const phone = $("recipientPhone").value.trim();
    const detail = $("shippingAddress").value.trim();
    if (!recipientName || !phone || !detail) {
      throw new Error("请填写收货人、手机号与收货地址");
    }

    const existing = await request("/api/addresses");
    const matched = (Array.isArray(existing) ? existing : []).find(
      (address) =>
        address.recipient_name === recipientName &&
        address.phone === phone &&
        address.detail === detail,
    );
    if (matched) return matched.id;

    const created = await request("/api/addresses", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        recipient_name: recipientName,
        phone,
        detail,
        is_default: false,
      }),
    });
    return created.id;
  }

  async function submitOrder(event) {
    event.preventDefault();
    if (!state.session) {
      window.location.href = "/";
      return;
    }
    if (state.buyerBlocked) {
      showToast("购物车仅对购买者账号开放", true);
      return;
    }
    if (!state.cartItems.length) {
      showToast("请至少选择一件商品", true);
      return;
    }

    const button = $("submitOrderButton");
    button.disabled = true;
    const label = button.textContent;
    button.textContent = "提交中...";
    try {
      const addressId = await resolveAddressId();
      // 请求体里**不带商品明细**：服务端按购物车下单，并会做单批次、库存、起购/限购校验。
      const order = await request("/api/orders", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ address_id: addressId }),
      });
      showToast(`订单已提交：${order.order_number}，请在商城页完成支付`);
      $("checkoutForm").reset();
      // 服务端下单成功后会清空购物车，这里重新读一次以拿到真实状态。
      await loadCart();
    } catch (error) {
      showToast(error.message, true);
    } finally {
      button.disabled = !state.session || state.buyerBlocked;
      button.textContent = label;
    }
  }

  async function logout() {
    try {
      await request("/web/session/logout", { method: "POST" });
    } catch {
      /* 会话可能已过期 */
    }
    state.session = null;
    state.cartItems = [];
    state.buyerBlocked = false;
    renderSession();
    render();
    showToast("已退出登录");
  }

  function bindEvents() {
    $("logoutButton").addEventListener("click", logout);
    $("checkoutForm").addEventListener("submit", submitOrder);

    $("cartItems").addEventListener("click", async (event) => {
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
  }

  function dropLegacyCart() {
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
    await loadCart();
  }

  init();
})();
