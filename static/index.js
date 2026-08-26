(() => {
  const PASSWORD_ICONS = {
    open: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="12" cy="12" rx="10" ry="6"></ellipse><circle cx="12" cy="12" r="3"></circle></svg>',
    closed: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="12" cy="12" rx="10" ry="6"></ellipse><circle cx="12" cy="12" r="3"></circle><line x1="4" y1="4" x2="20" y2="20"></line></svg>'
  };

  const els = {
    loginBtn: document.getElementById("login-btn"),
    modeSwitch: document.getElementById("mode-switch"),
    userModeBtn: document.getElementById("enter-user-mode"),
    adminModeBtn: document.getElementById("enter-admin-mode"),
    registerBtn: document.getElementById("register-btn"),
    loginMsg: document.getElementById("login-msg"),
    registerMsg: document.getElementById("register-msg"),
    systemBanner: document.getElementById("system-banner"),
    tabLogin: document.getElementById("tab-login"),
    tabRegister: document.getElementById("tab-register"),
    sectionLogin: document.getElementById("login-section"),
    sectionRegister: document.getElementById("register-section"),
    logoutBtn: document.getElementById("logout-btn"),
    inviteHint: document.querySelector(".invite-body .small")
  };

  const state = {
    systemStatus: {
      open_registration: true,
      invite_bypass_enabled: true,
      maintenance_mode: false,
    }
  };

  function unwrapApiPayload(payload) {
    if (payload && typeof payload === "object" && "data" in payload && payload.data != null) {
      return payload.data;
    }
    return payload || {};
  }

  function readApiMessage(payload, fallback) {
    return payload?.error || payload?.message || unwrapApiPayload(payload)?.error || fallback;
  }

  function normalizeUserPayload(payload) {
    const data = unwrapApiPayload(payload);
    return (data && data.user) ? data.user : data;
  }

  function setMsg(el, text, type) {
    el.textContent = text || "";
    el.classList.remove("error", "success");
    if (type) el.classList.add(type);
    if (el.id === "login-msg" || el.id === "register-msg") {
      el.style.display = text ? "block" : "none";
    }
  }

  function showTab(tabName) {
    const showLogin = tabName === "login";
    els.tabLogin.classList.toggle("active", showLogin);
    els.tabRegister.classList.toggle("active", !showLogin);
    els.sectionLogin.classList.toggle("active", showLogin);
    els.sectionRegister.classList.toggle("active", !showLogin);
  }

  function hideAdminModeChooser() {
    els.modeSwitch.style.display = "none";
    els.loginBtn.style.display = "block";
  }

  function showAdminModeChooser() {
    els.loginBtn.style.display = "none";
    els.modeSwitch.style.display = "flex";
  }

  function restoreLoggedOutState() {
    showTab("login");
    hideAdminModeChooser();
    els.tabRegister.style.display = "block";
    els.tabLogin.textContent = "登录";
    els.loginBtn.dataset.mode = "login";
    els.loginBtn.disabled = false;
    els.loginBtn.textContent = "登录";
    if (els.logoutBtn) {
      els.logoutBtn.style.display = "none";
    }
    setMsg(els.loginMsg, "", null);
  }

  function setRegisterEnabled(enabled) {
    els.tabRegister.disabled = !enabled;
    els.sectionRegister.querySelectorAll("input, select, button").forEach((el) => {
      el.disabled = !enabled;
    });

    if (!enabled && els.sectionRegister.classList.contains("active")) {
      showTab("login");
    }

    setMsg(els.registerMsg, enabled ? "" : "当前已关闭新用户注册", enabled ? null : "error");
  }

  function applySystemStatus(status) {
    if (!status || typeof status !== "object") return;

    state.systemStatus = {
      ...state.systemStatus,
      ...status,
    };

    const notices = [];
    if (state.systemStatus.maintenance_mode) {
      notices.push("系统维护中，仅管理员可登录");
    }
    if (!state.systemStatus.open_registration) {
      notices.push("当前已关闭新用户注册");
    }

    if (notices.length > 0) {
      els.systemBanner.textContent = notices.join(" · ");
      els.systemBanner.hidden = false;
    } else {
      els.systemBanner.hidden = true;
      els.systemBanner.textContent = "";
    }

    if (els.inviteHint) {
      els.inviteHint.textContent = state.systemStatus.invite_bypass_enabled
        ? "输入正确邀请码可直接注册，输入错误会提示并可改为提交审核申请。"
        : "当前已关闭邀请码免审核功能，填写邀请码也会进入审批队列。";
    }

    setRegisterEnabled(Boolean(state.systemStatus.open_registration));

    if (state.systemStatus.maintenance_mode) {
      setMsg(els.loginMsg, "系统维护中，仅管理员可登录", "error");
    } else if (els.loginMsg.textContent === "系统维护中，仅管理员可登录") {
      setMsg(els.loginMsg, "", null);
    }
  }

  async function fetchSystemStatus() {
    try {
      const res = await fetch("/api/system-status", {
        method: "GET",
        credentials: "same-origin",
      });
      const payload = await res.json().catch(() => ({}));
      applySystemStatus(unwrapApiPayload(payload));
    } catch (e) {
      // 忽略状态获取失败，不阻断登录页渲染
    }
  }

  function applyLoggedInState(userOrResp) {
    const user = normalizeUserPayload(userOrResp);

    els.tabRegister.style.display = "none";
    els.tabLogin.textContent = "已登录";
    showTab("login");

    if (els.logoutBtn) {
      els.logoutBtn.style.display = "block";
    }

    if (user.is_admin) {
      els.loginBtn.dataset.mode = "login";
      els.loginBtn.disabled = false;
      setMsg(els.loginMsg, "已登录（管理员），请选择模式", "success");
      showAdminModeChooser();
      return;
    }

    hideAdminModeChooser();
    const name = user.username || user.name || "未知用户";
    if (state.systemStatus.maintenance_mode) {
      els.loginBtn.dataset.mode = "disabled";
      els.loginBtn.textContent = "系统维护中";
      els.loginBtn.disabled = true;
      setMsg(els.loginMsg, `已登录：${name}。当前系统维护中，普通用户访问已暂停`, "error");
      return;
    }

    els.loginBtn.dataset.mode = "enter-user";
    els.loginBtn.disabled = false;
    els.loginBtn.textContent = "进入用户模式";
    setMsg(els.loginMsg, `已登录：${name}`, "success");
  }

  async function fetchValidatedSession() {
    try {
      const res = await fetch("/user/validate", {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
      });
      const data = await res.json().catch(() => ({}));
      applySystemStatus(data);
      if (res.ok && data.valid) {
        return data;
      }
    } catch (e) {
      // 忽略网络错误，按未验证状态继续处理
    }
    return null;
  }

  async function handleLogin() {
    if (els.loginBtn.dataset.mode === "enter-user") {
      window.location.href = "/analyze";
      return;
    }
    if (els.loginBtn.disabled) {
      return;
    }

    hideAdminModeChooser();
    setMsg(els.loginMsg, "登录中...");
    const username = document.getElementById("login-username").value.trim();
    const password = document.getElementById("login-password").value.trim();
    if (!username || !password) {
      setMsg(els.loginMsg, "用户名或密码不能为空", "error");
      return;
    }

    const res = await fetch("/user/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify({ username, password }),
    });

    const raw = await res.json().catch(() => ({}));
    const data = unwrapApiPayload(raw);
    if (!res.ok) {
      setMsg(els.loginMsg, readApiMessage(raw, "登录失败"), "error");
      return;
    }

    const validatedUser = await fetchValidatedSession();
    applyLoggedInState(validatedUser || data);
  }

  async function handleRegister() {
    if (!state.systemStatus.open_registration) {
      setMsg(els.registerMsg, "当前已关闭注册", "error");
      return;
    }

    setMsg(els.registerMsg, "注册中...");
    const username = document.getElementById("register-username").value.trim();
    const password = document.getElementById("register-password").value.trim();
    const requestedRole = document.getElementById("register-role").value;
    const invitationCode = document.getElementById("register-code").value.trim();

    if (!username || !password) {
      setMsg(els.registerMsg, "用户名、密码不能为空", "error");
      return;
    }

    const res = await fetch("/user/register", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify({
        username,
        password,
        requested_role: requestedRole,
        invitation_code: invitationCode,
      }),
    });

    const raw = await res.json().catch(() => ({}));
    const data = unwrapApiPayload(raw);
    if (!res.ok) {
      const hint = data.hint ? `。${data.hint}` : "";
      setMsg(els.registerMsg, `${readApiMessage(raw, "注册失败")}${hint}`, "error");
      return;
    }

    if (res.status === 202 || raw.message === "pending_approval") {
      const roleText = requestedRole === "admin" ? "管理员" : "用户";
      const hint = data.hint ? `，${data.hint}` : "";
      setMsg(els.registerMsg, `已提交${roleText}申请，等待管理员审核${hint}`, "success");
      return;
    }

    if (requestedRole === "admin") {
      setMsg(els.registerMsg, "注册成功（管理员），请登录后选择模式", "success");
      return;
    }

    setMsg(els.registerMsg, "注册成功，请返回登录", "success");
  }

  async function checkSessionOnLoad() {
    const validatedUser = await fetchValidatedSession();
    if (validatedUser) {
      applyLoggedInState(validatedUser);
      return;
    }

    restoreLoggedOutState();
  }

  async function logout() {
    try {
      await fetch("/user/logout", { method: "POST", credentials: "same-origin" });
    } finally {
      location.reload();
    }
  }

  function initPasswordToggles() {
    document.querySelectorAll(".pw-toggle").forEach((btn) => {
      const targetId = btn.getAttribute("data-target");
      const input = document.getElementById(targetId);
      if (!input) return;

      function updateIcon() {
        const isPassword = input.type === "password";
        btn.innerHTML = isPassword ? PASSWORD_ICONS.open : PASSWORD_ICONS.closed;
        btn.title = isPassword ? "显示密码" : "隐藏密码";
        btn.setAttribute("aria-pressed", String(!isPassword));
      }

      btn.addEventListener("click", () => {
        input.type = input.type === "password" ? "text" : "password";
        updateIcon();
      });

      updateIcon();
    });
  }

  function bindEvents() {
    els.tabLogin.addEventListener("click", () => showTab("login"));
    els.tabRegister.addEventListener("click", () => showTab("register"));
    els.userModeBtn.addEventListener("click", () => {
      window.location.href = "/analyze";
    });
    els.adminModeBtn.addEventListener("click", () => {
      window.location.href = "/admin";
    });
    els.loginBtn.addEventListener("click", handleLogin);
    els.registerBtn.addEventListener("click", handleRegister);
    els.logoutBtn?.addEventListener("click", logout);
  }

  async function initPage() {
    els.loginBtn.dataset.mode = "login";
    bindEvents();
    initPasswordToggles();
    await fetchSystemStatus();
    await checkSessionOnLoad();
  }

  void initPage();
})();
