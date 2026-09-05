(() => {
  const style = document.createElement("style");
  style.textContent = `.support-launch{position:fixed;right:22px;bottom:24px;z-index:90;background:#ff5000;color:white;border:0;border-radius:30px;padding:14px 24px;cursor:pointer;box-shadow:0 8px 25px #b8482533}.support-dialog{position:fixed;inset:auto 20px 85px auto;margin:0;width:min(390px,calc(100vw - 32px));max-height:75vh;border:1px solid #eee;border-radius:18px;padding:20px;background:white;color:#292524;box-shadow:0 20px 70px #0003}.support-head{display:flex;justify-content:space-between;align-items:center}.support-dialog button{cursor:pointer}.support-history{height:260px;overflow:auto;background:#faf7f3;padding:12px;border-radius:10px;margin:12px 0}.support-message{padding:10px;background:#fff;border:1px solid #eee;border-radius:10px;margin-bottom:10px;white-space:pre-wrap;overflow-wrap:anywhere}.support-message.mine{background:#fff0e4}.support-message small{display:block;color:#777;margin-bottom:5px}.support-dialog textarea{box-sizing:border-box;width:100%;padding:10px;color:#222;background:white;border:1px solid #ddd;border-radius:8px}.support-send{background:#ff5000;color:white;border:0;border-radius:9px;padding:10px 22px}.support-note{font-size:12px;color:#777}.support-faq summary{cursor:pointer}.support-status{font-size:13px;color:#aa3911}`;
  document.head.append(style);
  const launch = document.createElement("button");
  launch.className = "support-launch";
  launch.textContent = "联系客服";
  launch.type = "button";
  const dialog = document.createElement("dialog");
  dialog.className = "support-dialog";
  dialog.setAttribute("aria-label", "商城客服");
  dialog.innerHTML =
    '<div class="support-head"><strong>橙管家 · 购物客服</strong><button type="button" aria-label="关闭客服">关闭</button></div><p class="support-note">商品咨询、物流查询、售后问题，欢迎留言。</p><details class="support-faq"><summary>常见问题：如何查看物流与申请售后？</summary><p>在商城「我的订单」查看订单状态。需要配送详情、退换货或商品问题处理，请在下方发送订单号和具体情况，由客服回复。</p></details><div class="support-history" role="log" aria-label="会话消息"></div><div class="support-status" role="status"></div><form><textarea required maxlength="2000" rows="3" aria-label="咨询内容" placeholder="请输入问题，可附上订单号"></textarea><button class="support-send" type="submit">发送消息</button></form><p class="support-note">留言会保存，客服回复后可在此查看。当前展示最近 200 条消息。</p>';
  document.body.append(launch, dialog);
  const history = dialog.querySelector(".support-history"),
    status = dialog.querySelector(".support-status");
  let busy = false;
  async function request(options) {
    const r = await fetch("/user/store/support", {
      credentials: "same-origin",
      ...options,
    });
    const b = await r.json();
    if (!r.ok)
      throw Error(
        r.status === 401
          ? "请先登录，再发送和查看客服消息。"
          : b.message || "客服暂时不可用",
      );
    return b;
  }
  async function refresh() {
    if (busy || !dialog.open) return;
    busy = true;
    try {
      const b = await request();
      const bottom =
        history.scrollHeight - history.scrollTop - history.clientHeight < 50;
      history.replaceChildren();
      for (const m of b.messages) {
        const item = document.createElement("div");
        item.className = "support-message" + (m.is_staff ? "" : " mine");
        const small = document.createElement("small");
        small.textContent = `${m.is_staff ? "商城客服" : "我"} · ${new Date(m.created_at).toLocaleString("zh-CN")}`;
        item.append(small, document.createTextNode(m.content));
        history.append(item);
      }
      if (!b.messages.length)
        history.textContent = "还没有消息，描述您的问题即可开始咨询。";
      if (bottom) history.scrollTop = history.scrollHeight;
      status.textContent = "";
    } catch (e) {
      status.textContent = e.message;
    } finally {
      busy = false;
    }
  }
  function open() {
    if (!dialog.open) dialog.show();
    refresh();
    dialog.querySelector("textarea").focus();
  }
  launch.onclick = open;
  document
    .querySelectorAll("[data-open-support]")
    .forEach((b) => (b.onclick = open));
  dialog.querySelector(".support-head button").onclick = () => dialog.close();
  dialog.querySelector("form").onsubmit = async (e) => {
    e.preventDefault();
    const input = dialog.querySelector("textarea"),
      button = dialog.querySelector(".support-send");
    if (!input.value.trim()) return;
    button.disabled = true;
    try {
      await request({
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ content: input.value }),
      });
      input.value = "";
      await refresh();
      history.scrollTop = history.scrollHeight;
    } catch (e) {
      status.textContent = e.message;
    } finally {
      button.disabled = false;
    }
  };
  setInterval(() => {
    if (!document.hidden) refresh();
  }, 10000);
})();
