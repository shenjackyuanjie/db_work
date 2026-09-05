(() => {
  const $ = (id) => document.getElementById(id),
    escape = (value) =>
      String(value ?? "").replace(
        /[&<>"']/g,
        (c) =>
          ({
            "&": "&amp;",
            "<": "&lt;",
            ">": "&gt;",
            '"': "&quot;",
            "'": "&#39;",
          })[c],
      );
  const money = (n) => "¥" + (Number(n || 0) / 100).toFixed(2),
    labels = {
      pending_payment: "待收款",
      paid: "待发货",
      shipped: "已发货",
      completed: "已完成",
      cancelled: "已取消",
      refunded: "已退款",
    };
  let view = "dashboard",
    products = [],
    orders = [],
    editing = null,
    coverTarget = null,
    conversation = null,
    conversationVersion = 0,
    ready = false,
    loading = false;
  async function request(url, method = "GET", body) {
    const opts = { method, credentials: "same-origin" };
    if (body !== undefined) {
      opts.headers = { "Content-Type": "application/json" };
      opts.body = JSON.stringify(body);
    }
    const r = await fetch(url, opts);
    const b = await r.json();
    if (!r.ok) throw Error(b.message || b.error || "请求失败");
    return b.data ?? b;
  }
  function error(e) {
    $("notice").textContent = e.message;
  }
  async function action(button, fn) {
    button.disabled = true;
    try {
      await fn();
      $("notice").textContent = "操作已保存";
    } catch (e) {
      error(e);
    } finally {
      button.disabled = false;
    }
  }
  const empty = '<div class="empty">暂无数据</div>';
  function bars(items, label, value, format) {
    const max = Math.max(1, ...items.map(value));
    return (
      items
        .map(
          (i) =>
            `<div class="metric-row"><div class="metric-label"><span>${escape(label(i))}</span><strong>${escape(format(value(i)))}</strong></div><div class="meter"><span style="width:${Math.max(0, (value(i) / max) * 100)}%"></span></div></div>`,
        )
        .join("") || empty
    );
  }
  async function dashboard() {
    const d = await request(
      "/user/admin/store/analytics?days=" + $("period").value,
    );
    const s = d.summary;
    $("kpis").innerHTML = [
      ["成交金额", money(s.revenue)],
      ["订单数", s.orders],
      ["下单买家", s.buyers],
      ["待发货订单", s.pending],
    ]
      .map(
        ([k, v]) => `<div class="kpi">${k}<strong>${escape(v)}</strong></div>`,
      )
      .join("");
    const max = Math.max(1, ...d.trend.map((i) => Number(i.revenue)));
    $("trend").innerHTML = d.trend
      .map(
        (i) =>
          `<div class="trend-column" title="${escape(i.day)}：${money(i.revenue)} / ${i.orders} 单"><div class="trend-bar" role="img" aria-label="${escape(i.day)} 成交 ${money(i.revenue)}，${i.orders} 单" style="height:${(Number(i.revenue) / max) * 170}px"></div><small>${i.day.slice(5)}</small></div>`,
      )
      .join("");
    $("statuses").innerHTML = bars(
      d.statuses,
      (i) => labels[i.status] || i.status,
      (i) => Number(i.count),
      (n) => n + " 单",
    );
    $("ranking").innerHTML = bars(
      d.products,
      (i) => `${i.product_name} · ${i.quantity} 件`,
      (i) => Number(i.revenue),
      money,
    );
  }
  async function loadProducts() {
    const d = await request("/user/admin/store/products");
    products = d.products || [];
    $("products").innerHTML = products.length
      ? `<table><thead><tr><th>商品 / 规格</th><th>单价</th><th>库存</th><th>状态</th><th>操作</th></tr></thead><tbody>${products.map((p) => `<tr><td>${escape(p.name)}<small>${escape(p.sku)} · ${escape(p.unit_label)}</small></td><td>${money(p.price_cents)}</td><td>${p.stock_quantity}</td><td>${p.is_active ? "在售" : "已下架"}</td><td><button data-edit="${p.id}">编辑</button><button data-toggle="${p.id}">${p.is_active ? "下架" : "上架"}</button><button data-cover="${p.id}">上传封面</button></td></tr>`).join("")}</tbody></table>`
      : empty;
  }
  function renderOrders() {
    const q = $("orderSearch").value.trim().toLowerCase();
    const list = orders.filter((o) =>
      `${o.order_no} ${o.username}`.toLowerCase().includes(q),
    );
    $("orders").innerHTML = list.length
      ? `<table><thead><tr><th>订单 / 买家</th><th>收货信息</th><th>商品</th><th>金额</th><th>状态操作</th></tr></thead><tbody>${list
          .map(
            (o) =>
              `<tr><td>${escape(o.order_no)}<small>${escape(o.username)}</small><small>${new Date(o.created_at).toLocaleString("zh-CN")}</small></td><td>${escape(o.recipient_name)} · ${escape(o.recipient_phone)}<small>${escape(o.shipping_address)}</small></td><td>${(o.items || []).map((i) => `${escape(i.product_name)} × ${i.quantity}`).join("<br>")}</td><td>${money(o.total_cents)}</td><td><select aria-label="订单状态" data-status="${escape(o.id)}">${Object.entries(
                labels,
              )
                .map(
                  ([k, v]) =>
                    `<option value="${k}" ${o.status === k ? "selected" : ""}>${v}</option>`,
                )
                .join(
                  "",
                )}</select><button data-save="${escape(o.id)}">保存</button></td></tr>`,
          )
          .join("")}</tbody></table>`
      : empty;
  }
  async function loadOrders() {
    const d = await request("/user/admin/store/orders", "POST");
    orders = d.orders || [];
    renderOrders();
  }
  async function loadMessages() {
    if (!conversation) return;
    const name = conversation,
      version = ++conversationVersion;
    const d = await request(
      "/user/admin/store/support?username=" + encodeURIComponent(name),
    );
    if (version !== conversationVersion || name !== conversation) return;
    const wrap = $("messages"),
      bottom = wrap.scrollHeight - wrap.scrollTop - wrap.clientHeight < 60;
    wrap.innerHTML =
      d.messages
        .map(
          (m) =>
            `<div class="bubble ${m.is_staff ? "staff" : ""}"><small>${m.is_staff ? "客服" : escape(name)} · ${new Date(m.created_at).toLocaleString("zh-CN")}</small>${escape(m.content)}</div>`,
        )
        .join("") || empty;
    if (bottom) wrap.scrollTop = wrap.scrollHeight;
  }
  async function support() {
    const d = await request("/user/admin/store/support");
    $("conversations").innerHTML =
      d.conversations
        .map(
          (c) =>
            `<button class="conversation ${conversation === c.username ? "active" : ""}" data-conversation="${escape(c.username)}">${escape(c.username)} · ${c.is_staff ? "已回复" : "待回复"}<small>${escape(c.content.slice(0, 70))}</small></button>`,
        )
        .join("") || empty;
    await loadMessages();
  }
  async function refresh() {
    if (!ready || loading) return;
    loading = true;
    $("refresh").disabled = true;
    try {
      await { dashboard, products: loadProducts, orders: loadOrders, support }[
        view
      ]();
      $("notice").textContent =
        "已更新 · " + new Date().toLocaleTimeString("zh-CN");
    } catch (e) {
      error(e);
    } finally {
      loading = false;
      $("refresh").disabled = false;
    }
  }
  document.querySelectorAll("[data-view]").forEach(
    (b) =>
      (b.onclick = async () => {
        if (loading) return;
        view = b.dataset.view;
        document
          .querySelectorAll("[data-panel]")
          .forEach((p) => (p.hidden = p.dataset.panel !== view));
        document
          .querySelectorAll("[data-view]")
          .forEach((n) => n.classList.toggle("active", n === b));
        $("viewTitle").textContent = b.textContent;
        await refresh();
      }),
  );
  $("refresh").onclick = refresh;
  $("period").onchange = refresh;
  $("orderSearch").oninput = renderOrders;
  function reset() {
    editing = null;
    $("productForm").reset();
    $("productFormTitle").textContent = "发布商品";
  }
  $("cancelEdit").onclick = reset;
  $("productForm").onsubmit = (e) => {
    e.preventDefault();
    const form = e.currentTarget;
    action(form.querySelector("[type=submit]"), async () => {
      const f = new FormData(form);
      const price = Math.round(Number(f.get("price")) * 100),
        stock = Number(f.get("stock_quantity"));
      if (
        !Number.isSafeInteger(price) ||
        price < 1 ||
        !Number.isInteger(stock) ||
        stock < 0
      )
        throw Error("请输入有效的价格和整数库存");
      const body = {
        name: f.get("name").trim(),
        sku: f.get("sku").trim(),
        unit_label: f.get("unit_label").trim(),
        price_cents: price,
        stock_quantity: stock,
        description: f.get("description"),
        cover_image: f.get("cover_image").trim() || null,
      };
      await request(
        "/user/admin/store/products" + (editing ? "/" + editing : ""),
        editing ? "PUT" : "POST",
        body,
      );
      reset();
      await loadProducts();
    });
  };
  $("products").onclick = (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    if (b.dataset.edit) {
      const p = products.find((p) => p.id === Number(b.dataset.edit));
      editing = p.id;
      const f = $("productForm");
      for (const key of [
        "name",
        "sku",
        "unit_label",
        "stock_quantity",
        "description",
        "cover_image",
      ])
        f.elements.namedItem(key).value = p[key] ?? "";
      f.elements.namedItem("price").value = (p.price_cents / 100).toFixed(2);
      $("productFormTitle").textContent = "编辑商品";
      f.scrollIntoView({ behavior: "smooth", block: "center" });
    }
    if (b.dataset.toggle)
      action(b, async () => {
        await request(
          "/user/admin/store/products/" + b.dataset.toggle + "/toggle",
          "POST",
        );
        await loadProducts();
      });
    if (b.dataset.cover) {
      coverTarget = b.dataset.cover;
      $("coverFile").click();
    }
  };
  $("coverFile").onchange = async (e) => {
    const file = e.target.files[0],
      id = coverTarget;
    if (!file || !id) return;
    try {
      if (file.size > 8 * 1024 * 1024) throw Error("封面不能超过 8 MB");
      const form = new FormData();
      form.append("file", file);
      const r = await fetch("/user/admin/store/products/" + id + "/cover", {
        method: "POST",
        credentials: "same-origin",
        body: form,
      });
      const b = await r.json();
      if (!r.ok) throw Error(b.message || "上传失败");
      await loadProducts();
      $("notice").textContent = "封面已保存";
    } catch (e) {
      error(e);
    } finally {
      e.target.value = "";
      coverTarget = null;
    }
  };
  $("orders").onclick = (e) => {
    const b = e.target.closest("[data-save]");
    if (!b) return;
    const select = b.parentElement.querySelector("select");
    action(b, async () => {
      await request("/user/admin/store/orders/status", "POST", {
        order_id: b.dataset.save,
        status: select.value,
      });
      await loadOrders();
    });
  };
  $("conversations").onclick = (e) => {
    const b = e.target.closest("[data-conversation]");
    if (!b) return;
    conversation = b.dataset.conversation;
    $("conversationTitle").textContent = "正在接待：" + conversation;
    $("reply").value = "";
    $("reply").disabled = false;
    $("replyButton").disabled = false;
    support().catch(error);
  };
  $("quickReply").onchange = (e) => {
    if (conversation) $("reply").value = e.target.value;
    e.target.value = "";
  };
  $("replyForm").onsubmit = (e) => {
    e.preventDefault();
    const username = conversation,
      content = $("reply").value.trim();
    if (!username || !content) return;
    action($("replyButton"), async () => {
      await request("/user/admin/store/support", "POST", { username, content });
      if (conversation === username) $("reply").value = "";
      await support();
    });
  };
  setInterval(() => {
    if (ready && view === "support" && !document.hidden && !loading) refresh();
  }, 10000);
  (async () => {
    try {
      const session = await request("/user/validate", "POST");
      if (!session.valid || !session.is_admin)
        throw Error("请使用管理员账号登录后进入商城后台。");
      $("account").textContent = session.username;
      $("workspace").hidden = false;
      ready = true;
      await refresh();
    } catch (e) {
      error(e);
    }
  })();
})();
