(() => {
  // A legacy page may assign location directly after login. Return that request
  // to the existing shell instead of nesting another application inside it.
  if (window.parent !== window && window.parent.AppShell) {
    window.parent.AppShell.navigate(location.pathname + location.search, {
      force: true,
    });
    return;
  }

  const $ = (id) => document.getElementById(id);
  const frame = $("shellContent");
  const names = {
    "/": "登录 / 注册",
    "/store": "鲜果商城",
    "/cart": "购物车",
    "/analyze": "病害识别",
    "/orchard-3d": "果园沙盘",
    "/admin": "果园管理",
  };
  let active = "";
  let version = 0;
  let sessionIdentity;

  function normalize(value) {
    const url = new URL(value, location.origin);
    if (url.origin !== location.origin) return null;
    url.pathname = url.pathname.replace(/\.html$/, "");
    if (url.pathname === "/index") url.pathname = "/";
    if (url.pathname === "/commerce") url.pathname = "/store";
    if (url.pathname === "/store-admin") {
      url.pathname = "/admin";
      url.searchParams.set("mode", "store");
    }
    if (
      url.pathname === "/admin" &&
      url.searchParams.get("section") === "store"
    ) {
      url.searchParams.delete("section");
      url.searchParams.set("mode", "store");
    }
    if (!Object.hasOwn(names, url.pathname)) return null;
    return url;
  }

  function paint(url) {
    const merchant =
      url.pathname === "/admin" && url.searchParams.get("mode") === "store";
    const title = merchant ? "商城管理" : names[url.pathname];
    $("shellPageTitle").textContent = title;
    document.title = `${title} | 橙管家`;
    document.querySelectorAll(".shell-nav [data-route]").forEach((link) => {
      const target = normalize(link.href);
      const selected =
        target.pathname === url.pathname &&
        (target.searchParams.get("mode") === "store") === merchant;
      if (selected) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    });
  }

  async function session() {
    const response = await fetch("/user/validate", {
      method: "POST",
      credentials: "same-origin",
    });
    if (!response.ok && response.status !== 401)
      throw new Error("暂时无法验证登录状态，请稍后重试。");
    const body = await response.json();
    const data = body.data ?? body;
    const current = response.ok && data.valid ? data : null;
    $("shellUsername").textContent = current?.username || "";
    $("shellLogin").hidden = Boolean(current);
    $("shellLogout").hidden = !current;
    document.querySelectorAll("[data-admin]").forEach((item) => {
      item.hidden = !current?.is_admin;
    });
    return current;
  }

  async function navigate(value, { replace = false, force = false } = {}) {
    let url = normalize(value);
    if (!url) return false;
    const request = ++version;
    try {
      const user = await session();
      if (request !== version) return true;
      const identity = `${user?.username || ""}:${Boolean(user?.is_admin)}`;
      if (identity !== sessionIdentity) force = true;
      sessionIdentity = identity;
      $("shellStatus").textContent = "";
      if (
        (url.pathname === "/admin" && !user?.is_admin) ||
        (url.pathname === "/analyze" && !user)
      ) {
        $("shellStatus").textContent = user
          ? "当前账号无管理权限。"
          : "请先登录后使用该功能。";
        url = normalize(user ? "/store" : "/");
      }
      const route = url.pathname + url.search + url.hash;
      if (location.pathname + location.search + location.hash !== route) {
        history[replace ? "replaceState" : "pushState"]({}, "", route);
      }
      paint(url);
      if (active === route && !force) return true;
      active = route;
      $("shellLoading").hidden = false;
      $("shellMain").setAttribute("aria-busy", "true");
      const merchant =
        url.pathname === "/admin" && url.searchParams.get("mode") === "store";
      const page = merchant
        ? "store-admin"
        : url.pathname === "/"
          ? "home"
          : url.pathname.slice(1);
      frame.title = $("shellPageTitle").textContent;
      frame.contentWindow.location.replace(
        `/app-content/${page}${url.search}${url.hash}`,
      );
    } catch (error) {
      if (request === version) {
        $("shellStatus").textContent = error.message;
        $("shellLoading").hidden = true;
        $("shellMain").setAttribute("aria-busy", "false");
      }
    }
    return true;
  }

  function sync(value, { push = false } = {}) {
    const url = normalize(value);
    if (!url) return;
    active = url.pathname + url.search + url.hash;
    history[push ? "pushState" : "replaceState"]({}, "", active);
    paint(url);
  }

  window.AppShell = { navigate, normalize, sync };
  document.addEventListener("click", (event) => {
    const link = event.target.closest("a[data-route]");
    if (
      !link ||
      event.button !== 0 ||
      event.ctrlKey ||
      event.metaKey ||
      event.shiftKey ||
      event.altKey
    )
      return;
    event.preventDefault();
    void navigate(link.href);
  });
  frame.addEventListener("load", () => {
    if (
      frame.contentWindow.location.pathname === "/" ||
      !frame.contentWindow.location.pathname.startsWith("/app-content/")
    )
      return;
    $("shellLoading").hidden = true;
    $("shellMain").setAttribute("aria-busy", "false");
  });
  $("shellLogout").onclick = async () => {
    $("shellLogout").disabled = true;
    try {
      const response = await fetch("/user/logout", {
        method: "POST",
        credentials: "same-origin",
      });
      if (!response.ok) throw new Error("退出失败，请重试。");
      await navigate("/", { force: true });
    } catch (error) {
      $("shellStatus").textContent = error.message;
    } finally {
      $("shellLogout").disabled = false;
    }
  };
  window.addEventListener("popstate", () => {
    void navigate(location.href, { replace: true });
  });
  window.addEventListener("focus", () => {
    void navigate(location.href);
  });
  void navigate(location.href, { replace: true });
})();
