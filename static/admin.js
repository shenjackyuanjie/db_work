(() => {
const $ = (id) => document.getElementById(id);
const SESSION_COOKIE = "session_token";
const DEFAULT_ORCHARD_BOUNDS = Object.freeze({
  min_x: 0,
  max_x: 500,
  min_y: 0,
  max_y: 500,
});

function unwrapApiPayload(payload) {
  if (payload && typeof payload === "object" && "data" in payload && payload.data != null) {
    return payload.data;
  }
  return payload || {};
}

function readErrorMessage(payload, fallback) {
  return payload?.error || payload?.message || unwrapApiPayload(payload)?.error || fallback;
}

function escapeHtml(text) {
  return String(text ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function showToast(message, type = "success") {
  const container = $("toast-container");
  const toast = document.createElement("div");
  toast.className = `toast ${type}`;
  toast.innerHTML = message;
  container.appendChild(toast);
  setTimeout(() => {
    toast.style.opacity = "0";
    toast.style.transform = "translateX(100%)";
    toast.style.transition = "all 0.3s ease";
    setTimeout(() => toast.remove(), 300);
  }, 3000);
}

function copyText(text) {
  if (!text) return;
  if (navigator.clipboard && navigator.clipboard.writeText) {
    navigator.clipboard.writeText(text).then(() => {
      showToast("邀请码已复制到剪贴板");
    }).catch(() => fallbackCopy(text));
  } else {
    fallbackCopy(text);
  }
}

function fallbackCopy(text) {
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.focus();
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    if (ok) showToast("邀请码已复制到剪贴板");
    else showToast("复制失败，请手动选择复制", "error");
  } catch {
    showToast("复制失败，请手动选择复制", "error");
  }
}

function formatDate(unixSeconds) {
  if (!unixSeconds || unixSeconds <= 0 || unixSeconds > 4102444800) return "永久有效";
  const d = new Date(unixSeconds * 1000);
  return isNaN(d.getTime()) ? "未知时间" : d.toLocaleString();
}

function copyInviteCode() {
  const code = $("generatedCode").innerText;
  if (!code) return;
  copyText(code);
}

function logout() {
  document.cookie = `${SESSION_COOKIE}=; path=/; max-age=0; samesite=lax`;
  setTimeout(() => location.reload(), 80);
}

function exposeGlobalActions() {
  Object.assign(window, {
    logout,
    copyText,
    copyInviteCode,
    toggleAdmin,
    approvePending,
    rejectPending,
  });
}

function getCookie(name) {
  const prefix = `${name}=`;
  const part = document.cookie.split(";").map((item) => item.trim()).find((item) => item.startsWith(prefix));
  if (!part) return "";
  return decodeURIComponent(part.slice(prefix.length));
}

function getToken() {
  return getCookie(SESSION_COOKIE);
}

function requestHeaders(withJsonBody) {
  const headers = {};
  if (withJsonBody) headers["Content-Type"] = "application/json";
  const token = getToken();
  if (token) headers["X-Session-Token"] = token;
  return headers;
}

async function postJson(url, body) {
  const withBody = body !== undefined;
  try {
    const res = await fetch(url, {
      method: "POST",
      credentials: "same-origin",
      headers: requestHeaders(withBody),
      body: withBody ? JSON.stringify(body) : undefined,
    });
    const text = await res.text();
    try {
      return { ok: res.ok, data: JSON.parse(text) };
    } catch {
      return { ok: res.ok, data: { raw: text } };
    }
  } catch (e) {
    return { ok: false, data: { error: "网络请求失败" } };
  }
}

async function validateAdmin() {
  const token = getToken();
  if (!token) return null;
  const resp = await postJson("/user/validate");
  if (!resp.ok || !resp.data.valid || !resp.data.is_admin) {
    return null;
  }
  return resp.data;
}

async function createInvite() {
  const btn = $("btnCreateInvite");
  btn.disabled = true;
  btn.innerText = "生成中...";

  const ttlVal = Number($("ttlSelect").value);
  const body = { ttl_seconds: ttlVal };

  const resp = await postJson("/user/admin/invitations/create", body);
  const payload = unwrapApiPayload(resp.data);
  btn.disabled = false;
  btn.innerText = "立即生成";

  if (!resp.ok) {
    showToast(readErrorMessage(resp.data, "生成失败"), "error");
  } else {
    showToast("邀请码生成成功");
    $("inviteResult").classList.add("show");
    $("generatedCode").innerText = payload.code || "";
    $("generatedExpire").innerText = "过期于: " + formatDate(payload.expires_at);
    await listInvites();
  }
}

async function toggleAdmin(username, currentIsAdmin) {
  if (!confirm(`确定要将 ${username} ${currentIsAdmin ? "降级为普通用户" : "设为管理员"} 吗？`)) return;

  const resp = await postJson("/user/admin/set_admin", {
    target_username: username,
    make_admin: !currentIsAdmin,
  });

  if (!resp.ok) {
    showToast(readErrorMessage(resp.data, "权限更新失败"), "error");
  } else {
    showToast(`已成功更新 ${username} 的权限`);
    listUsers();
  }
}

function renderInvites(list) {
  if (!list || list.length === 0) {
    $("invitesWrap").innerHTML = "<div class='empty-state'>暂无历史邀请码数据</div>";
    return;
  }
  const rows = list.map((item) => `
      <tr>
        <td style="font-family: monospace; letter-spacing: 0.5px;">${item.code}</td>
        <td>${item.used ? "<span class='pill user'>已使用</span>" : "<span class='pill admin'>可用</span>"}</td>
        <td>${formatDate(item.expires_at)}</td>
        <td>
          <button class="ghost" onclick="copyText('${item.code}')">复制</button>
        </td>
      </tr>
    `).join("");

  $("invitesWrap").innerHTML = `
      <table>
        <thead>
          <tr>
            <th>邀请码</th>
            <th>状态</th>
            <th>过期时间</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>${rows}</tbody>
      </table>`;
}

function renderUsers(list) {
  if (!list || list.length === 0) {
    $("usersWrap").innerHTML = "<div class='empty-state'>暂无用户数据</div>";
    return;
  }
  const rows = list.map((user) => {
    const safeUsername = encodeURIComponent(user.username);
    const actionBtn = user.is_admin
      ? `<button class="ghost danger" onclick="toggleAdmin(decodeURIComponent('${safeUsername}'), true)">撤销管理</button>`
      : `<button class="ghost" onclick="toggleAdmin(decodeURIComponent('${safeUsername}'), false)">设为管理</button>`;

    return `
      <tr>
        <td>${user.username}</td>
        <td>${user.is_admin ? "<span class='pill admin'>管理员</span>" : "<span class='pill user'>普通用户</span>"}</td>
        <td>${formatDate(user.created_at)}</td>
        <td>${actionBtn}</td>
      </tr>
    `;
  }).join("");

  $("usersWrap").innerHTML = `<table><thead><tr><th>用户名</th><th>身份</th><th>注册时间</th><th>操作</th></tr></thead><tbody>${rows}</tbody></table>`;
}

function renderPending(list) {
  if (!list || list.length === 0) {
    $("pendingWrap").innerHTML = "<div class='empty-state'>当前没有需要审批的请求</div>";
    return;
  }
  const rows = list.map((user) => {
    const role = user.requested_role === "admin" ? "<span class='pill admin'>管理员申请</span>" : "<span class='pill user'>普通用户</span>";
    const encoded = encodeURIComponent(user.username);
    return `
        <tr>
          <td>${user.username}</td>
          <td>${role}</td>
          <td>${formatDate(user.created_at)}</td>
          <td>
            <button class="ghost" style="color:var(--success); border-color:var(--success);" onclick="approvePending(decodeURIComponent('${encoded}'))">通过</button>
            <button class="ghost danger" onclick="rejectPending(decodeURIComponent('${encoded}'))">拒绝</button>
          </td>
        </tr>`;
  }).join("");
  $("pendingWrap").innerHTML = `<table><thead><tr><th>申请人</th><th>期望身份</th><th>申请时间</th><th>审批操作</th></tr></thead><tbody>${rows}</tbody></table>`;
}

async function listPending() {
  const resp = await postJson("/user/admin/pending/list");
  const payload = unwrapApiPayload(resp.data);
  if (resp.ok) {
    renderPending(payload.pending_users || []);
    return true;
  }
  showToast(readErrorMessage(resp.data, "获取待审批列表失败"), "error");
  return false;
}

async function listInvites() {
  const resp = await postJson("/user/admin/invitations/list");
  const payload = unwrapApiPayload(resp.data);
  if (resp.ok) {
    renderInvites(payload.invitations || []);
    return true;
  }
  showToast(readErrorMessage(resp.data, "获取邀请码列表失败"), "error");
  return false;
}

async function listUsers() {
  const resp = await postJson("/user/admin/users/list");
  const payload = unwrapApiPayload(resp.data);
  if (resp.ok) {
    renderUsers(payload.users || []);
    return true;
  }
  showToast(readErrorMessage(resp.data, "获取用户列表失败"), "error");
  return false;
}

async function approvePending(username) {
  const resp = await postJson("/user/admin/pending/approve", { username });
  if (resp.ok) {
    showToast(`已通过 ${username} 的申请`);
    listPending();
    listUsers();
  } else {
    showToast(readErrorMessage(resp.data, "操作失败"), "error");
  }
}

async function rejectPending(username) {
  if (!confirm(`确定要拒绝 ${username} 的申请吗？`)) return;
  const resp = await postJson("/user/admin/pending/reject", { username });
  if (resp.ok) {
    showToast(`已拒绝 ${username} 的申请`);
    listPending();
  } else {
    showToast(readErrorMessage(resp.data, "操作失败"), "error");
  }
}

const orchardState = {
  rawTrees: [],
  trees: [],
  legend: [],
  summary: null,
  bounds: { ...DEFAULT_ORCHARD_BOUNDS },
  hoveredTreeId: null,
  backgroundImageData: null,
  backgroundCacheKey: "",
};
let currentAdminUsername = "";
let allLogs = [];

const ADMIN_SECTIONS = new Set(["orchard", "access", "overview", "audit", "settings"]);

function requestedAdminSection() {
  const section = new URLSearchParams(window.location.search).get("section");
  return ADMIN_SECTIONS.has(section) ? section : "orchard";
}

function showAdminSection(section, updateUrl = false) {
  const nextSection = ADMIN_SECTIONS.has(section) ? section : "orchard";

  document.querySelectorAll("[data-admin-page]").forEach((page) => {
    page.hidden = page.dataset.adminPage !== nextSection;
  });

  document.querySelectorAll("[data-admin-section]").forEach((link) => {
    const active = link.dataset.adminSection === nextSection;
    link.classList.toggle("active", active);
    if (active) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  });

  if (updateUrl) {
    window.history.pushState({}, "", `/admin?section=${encodeURIComponent(nextSection)}`);
  }

  if (nextSection === "orchard") {
    window.requestAnimationFrame(() => renderOrchardMap());
  }
}

function bindAdminSectionNavigation() {
  document.querySelectorAll("[data-admin-section]").forEach((link) => {
    link.addEventListener("click", (event) => {
      event.preventDefault();
      showAdminSection(link.dataset.adminSection, true);
    });
  });
  window.addEventListener("popstate", () => showAdminSection(requestedAdminSection()));
  showAdminSection(requestedAdminSection());
}

function orchardTreePalette(level, accentColor) {
  const accent = String(accentColor || "").toLowerCase();
  if (accent === "#38bdf8") {
    return {
      accent: "#38bdf8",
      highlight: "#e0f2fe",
      mid: "#7dd3fc",
      edge: "#0284c7",
      trunk: "#7c4a22",
      glow: "rgba(56, 189, 248, 0.24)",
      shadow: "rgba(8, 145, 178, 0.3)",
    };
  }

  switch (level) {
    case "healthy":
      return {
        accent: "#22c55e",
        highlight: "#dcfce7",
        mid: "#4ade80",
        edge: "#15803d",
        trunk: "#7c4a22",
        glow: "rgba(34, 197, 94, 0.22)",
        shadow: "rgba(3, 105, 55, 0.28)",
      };
    case "attention":
      return {
        accent: "#f59e0b",
        highlight: "#fef3c7",
        mid: "#fbbf24",
        edge: "#b45309",
        trunk: "#7c4a22",
        glow: "rgba(245, 158, 11, 0.24)",
        shadow: "rgba(146, 64, 14, 0.3)",
      };
    case "warning":
      return {
        accent: "#ef4444",
        highlight: "#fecaca",
        mid: "#fb7185",
        edge: "#b91c1c",
        trunk: "#7c3f1d",
        glow: "rgba(239, 68, 68, 0.24)",
        shadow: "rgba(127, 29, 29, 0.3)",
      };
    case "critical":
      return {
        accent: "#8b5cf6",
        highlight: "#ddd6fe",
        mid: "#a78bfa",
        edge: "#6d28d9",
        trunk: "#5b3c74",
        glow: "rgba(139, 92, 246, 0.24)",
        shadow: "rgba(76, 29, 149, 0.3)",
      };
    default:
      return {
        accent: "#64748b",
        highlight: "#e2e8f0",
        mid: "#94a3b8",
        edge: "#475569",
        trunk: "#6b4f33",
        glow: "rgba(100, 116, 139, 0.2)",
        shadow: "rgba(30, 41, 59, 0.28)",
      };
  }
}

function isFiniteNumber(value) {
  return Number.isFinite(Number(value));
}

function formatMetric(value, digits = 1) {
  const numeric = Number(value);
  return Number.isFinite(numeric) ? numeric.toFixed(digits) : "--";
}

function orchardCanvasPoint(rawX, rawY, width, height) {
  const bounds = orchardState.bounds || DEFAULT_ORCHARD_BOUNDS;
  const paddingX = 34;
  const paddingY = 26;
  const rangeX = Math.max(1, Number(bounds.max_x) - Number(bounds.min_x));
  const rangeY = Math.max(1, Number(bounds.max_y) - Number(bounds.min_y));
  const safeX = isFiniteNumber(rawX) ? Number(rawX) : Number(bounds.min_x);
  const safeY = isFiniteNumber(rawY) ? Number(rawY) : Number(bounds.min_y);
  const ratioX = Math.max(0, Math.min(1, (safeX - Number(bounds.min_x)) / rangeX));
  const ratioY = Math.max(0, Math.min(1, (safeY - Number(bounds.min_y)) / rangeY));

  return {
    x: paddingX + ratioX * (width - paddingX * 2),
    y: paddingY + ratioY * (height - paddingY * 2),
  };
}

function buildOrchardTrees(width, height) {
  return orchardState.rawTrees.map((item, index) => {
    const position = item?.position || {};
    const sensor = item?.latest_sensor || {};
    const rawX = Number(position.x ?? 0);
    const rawY = Number(position.y ?? 0);
    const mapped = orchardCanvasPoint(rawX, rawY, width, height);
    const treeCode = item?.tree_code || `TREE-${item?.id ?? index + 1}`;
    const palette = orchardTreePalette(item?.status?.level || "offline", item?.status?.color || "");
    const canopyRadius = 6.9 + ((Number(item?.id || index) % 3) * 0.35);

    return {
      id: treeCode,
      dbId: Number(item?.id || 0),
      treeCode,
      label: item?.status?.label || "果树",
      statusLevel: item?.status?.level || "offline",
      statusColor: item?.status?.color || palette.accent,
      rawX,
      rawY,
      x: mapped.x,
      y: mapped.y,
      terrainHeight: Number(item?.terrain_height || 0),
      canopyRadius,
      hitRadius: canopyRadius + 6,
      temperature: isFiniteNumber(sensor.temperature) ? Number(sensor.temperature) : null,
      humidity: isFiniteNumber(sensor.humidity) ? Number(sensor.humidity) : null,
      sampledAt: sensor.sampled_at || null,
      latestDiagnosis: item?.latest_diagnosis || null,
      palette,
    };
  }).sort((a, b) => a.y - b.y);
}

function drawOrchardTree(ctx, tree, hovered) {
  const radius = tree.canopyRadius + (hovered ? 1.4 : 0);

  ctx.save();
  ctx.translate(tree.x, tree.y);

  ctx.beginPath();
  ctx.ellipse(0, radius + 7, radius * 1.28, radius * 0.46, 0, 0, Math.PI * 2);
  ctx.fillStyle = hovered ? "rgba(15, 23, 42, 0.58)" : "rgba(15, 23, 42, 0.42)";
  ctx.fill();

  if (tree.statusLevel !== "healthy") {
    ctx.beginPath();
    ctx.arc(0, 0, radius + 6.5, 0, Math.PI * 2);
    ctx.fillStyle = tree.palette.glow;
    ctx.globalAlpha = hovered ? 0.82 : 0.58;
    ctx.fill();
    ctx.globalAlpha = 1;
  }

  ctx.beginPath();
  ctx.moveTo(0, radius * 0.55);
  ctx.lineTo(0, radius + 8);
  ctx.strokeStyle = tree.palette.trunk;
  ctx.lineWidth = Math.max(1.4, radius * 0.34);
  ctx.lineCap = "round";
  ctx.stroke();

  const crownGradient = ctx.createRadialGradient(-radius * 0.48, -radius * 0.62, 1, 0, 0, radius * 1.45);
  crownGradient.addColorStop(0, tree.palette.highlight);
  crownGradient.addColorStop(0.55, tree.palette.mid);
  crownGradient.addColorStop(1, tree.palette.edge);

  ctx.fillStyle = crownGradient;
  ctx.shadowBlur = hovered ? 18 : 12;
  ctx.shadowColor = tree.palette.shadow;

  const crownParts = [
    { x: -radius * 0.58, y: radius * 0.02, r: radius * 0.76 },
    { x: radius * 0.56, y: radius * 0.04, r: radius * 0.72 },
    { x: 0, y: -radius * 0.34, r: radius * 0.9 },
  ];

  crownParts.forEach((part) => {
    ctx.beginPath();
    ctx.arc(part.x, part.y, part.r, 0, Math.PI * 2);
    ctx.fill();
  });

  ctx.shadowBlur = 0;

  ctx.beginPath();
  ctx.arc(-radius * 0.24, -radius * 0.42, Math.max(1.1, radius * 0.2), 0, Math.PI * 2);
  ctx.fillStyle = "rgba(255, 255, 255, 0.78)";
  ctx.fill();

  if (hovered) {
    ctx.beginPath();
    ctx.arc(0, 0, radius + 4.2, 0, Math.PI * 2);
    ctx.strokeStyle = tree.palette.accent;
    ctx.lineWidth = 1.4;
    ctx.stroke();
  }

  ctx.restore();
}

function findHoveredTree(canvas, event) {
  const rect = canvas.getBoundingClientRect();
  const scaleX = canvas.width / rect.width;
  const scaleY = canvas.height / rect.height;
  const mouseX = (event.clientX - rect.left) * scaleX;
  const mouseY = (event.clientY - rect.top) * scaleY;

  for (let index = orchardState.trees.length - 1; index >= 0; index--) {
    const tree = orchardState.trees[index];
    const dx = mouseX - tree.x;
    const dy = mouseY - tree.y;
    if ((dx * dx) + (dy * dy) <= tree.hitRadius * tree.hitRadius) {
      return tree;
    }
  }

  return null;
}

function hideOrchardTooltip() {
  const tooltip = $("orchardTooltip");
  const canvas = $("orchardMap");
  tooltip.classList.remove("show", "below");
  tooltip.setAttribute("aria-hidden", "true");
  canvas.classList.remove("is-hovering");
}

function showOrchardTooltip(tree, event) {
  const tooltip = $("orchardTooltip");
  const canvas = $("orchardMap");
  const rect = canvas.getBoundingClientRect();
  const offsetX = event.clientX - rect.left;
  const offsetY = event.clientY - rect.top;
  const diagnosisText = tree.latestDiagnosis?.disease_name || tree.latestDiagnosis?.predicted_class || "无";

  tooltip.innerHTML = `
    <div class="orchard-tooltip__eyebrow">${escapeHtml(tree.label)}</div>
    <div class="orchard-tooltip__title">${escapeHtml(tree.treeCode)} / ID ${escapeHtml(tree.dbId)}</div>
    <div class="orchard-tooltip__meta">
      <div class="orchard-tooltip__meta-item">
        <span>坐标</span>
        <strong>${formatMetric(tree.rawX, 1)}, ${formatMetric(tree.rawY, 1)}</strong>
      </div>
      <div class="orchard-tooltip__meta-item">
        <span>温度</span>
        <strong>${formatMetric(tree.temperature, 1)}°C</strong>
      </div>
      <div class="orchard-tooltip__meta-item">
        <span>湿度</span>
        <strong>${formatMetric(tree.humidity, 1)}%</strong>
      </div>
      <div class="orchard-tooltip__meta-item">
        <span>地形高程</span>
        <strong>${formatMetric(tree.terrainHeight, 1)} m</strong>
      </div>
    </div>
    <div style="margin-top: 10px; font-size: 11px; color: var(--muted); line-height: 1.45;">
      诊断结果：${escapeHtml(diagnosisText)}<br>
      最近采样：${escapeHtml(tree.sampledAt ? parseTime(tree.sampledAt) : "暂无")}
    </div>
  `;

  tooltip.style.borderColor = `${tree.palette.accent}66`;
  tooltip.style.setProperty("--orchard-tooltip-border", `${tree.palette.accent}66`);

  // 测量 tooltip 实际尺寸（offsetWidth/Height 不受 transform / opacity 影响）
  const tw = tooltip.offsetWidth;
  const th = tooltip.offsetHeight;
  const gap = 8;

  // ── 水平方向 ──
  // CSS 用 translate(-50%) 水平居中，tooltip 左边缘 = left - tw/2，右边缘 = left + tw/2
  // 保证整个 tooltip 不溢出容器边界
  const minLeft = Math.max(gap + tw / 2, 0);
  const maxLeft = Math.min(rect.width - gap - tw / 2, rect.width);
  const finalLeft = Math.max(minLeft, Math.min(maxLeft, offsetX));

  // ── 垂直方向 ──
  // "上方"模式（默认）：tooltip 下边缘在 top - 18（箭头间隙），上边缘在 top - 18 - th
  // "下方"模式（below）：tooltip 上边缘在 top + 18，下边缘在 top + 18 + th
  const fitsAbove = offsetY - 18 - th >= gap;
  const fitsBelow = offsetY + 18 + th <= rect.height - gap;

  let finalTop, showBelow;
  if (fitsAbove) {
    // 上方空间充足，放在上方
    finalTop = offsetY;
    showBelow = false;
  } else if (fitsBelow) {
    // 上方不足但下方充足，放在下方
    finalTop = offsetY;
    showBelow = true;
  } else {
    // 两侧空间都不够，选空间更大的一侧并 clamp
    const roomAbove = offsetY - gap - 18 - th;
    const roomBelow = rect.height - gap - (offsetY + 18 + th);
    if (roomAbove >= roomBelow) {
      finalTop = Math.max(gap + 18 + th, offsetY);
      showBelow = false;
    } else {
      finalTop = Math.min(rect.height - gap - 18 - th, offsetY);
      showBelow = true;
    }
  }

  tooltip.style.left = `${finalLeft}px`;
  tooltip.style.top = `${finalTop}px`;
  tooltip.classList.toggle("below", showBelow);
  tooltip.classList.add("show");
  tooltip.setAttribute("aria-hidden", "false");
  canvas.classList.add("is-hovering");
}

function bindOrchardMapInteractions() {
  const canvas = $("orchardMap");
  if (!canvas || canvas.dataset.interactiveBound === "true") return;

  canvas.addEventListener("mousemove", (event) => {
    const hoveredTree = findHoveredTree(canvas, event);
    const nextTreeId = hoveredTree?.id || null;

    if (orchardState.hoveredTreeId !== nextTreeId) {
      orchardState.hoveredTreeId = nextTreeId;
      renderOrchardMap();
    }

    if (!hoveredTree) {
      hideOrchardTooltip();
      return;
    }

    showOrchardTooltip(hoveredTree, event);
  });

  canvas.addEventListener("mouseleave", () => {
    orchardState.hoveredTreeId = null;
    hideOrchardTooltip();
    renderOrchardMap();
  });

  canvas.dataset.interactiveBound = "true";
}

function renderOrchardLegend() {
  const legend = $("orchardLegend");
  if (!legend) return;

  const items = Array.isArray(orchardState.legend) ? orchardState.legend : [];
  const summary = orchardState.summary || {};

  if (items.length === 0) {
    legend.innerHTML = "<span>暂无果树数据</span>";
    return;
  }

  const totalTrees = Number(summary.total_trees || orchardState.rawTrees.length || 0);
  const onlineTrees = Number(summary.online_trees || 0);

  legend.innerHTML = items.map((item) => `
    <div style="display: flex; align-items: center; gap: 6px;">
      <div style="width: 10px; height: 10px; border-radius: 50%; background: ${item.color}; box-shadow: 0 0 8px ${item.color};"></div>
      <span>${escapeHtml(item.label)} <strong style="color:var(--text)">${Number(item.count || 0)}</strong></span>
    </div>
  `).join("") + `
    <div style="display: flex; align-items: center; gap: 10px; padding-left: 8px; border-left: 1px solid rgba(148, 163, 184, 0.18);">
      <span>在线 ${onlineTrees}/${totalTrees}</span>
    </div>
  `;
}

function renderOrchardEmptyState(ctx, width, height) {
  ctx.fillStyle = "rgba(15, 23, 42, 0.78)";
  ctx.fillRect(0, 0, width, height);
  ctx.fillStyle = "rgba(226, 232, 240, 0.86)";
  ctx.font = "600 16px Segoe UI";
  ctx.textAlign = "center";
  ctx.fillText("暂无果树坐标数据", width / 2, height / 2 - 8);
  ctx.fillStyle = "rgba(148, 163, 184, 0.92)";
  ctx.font = "12px Segoe UI";
  ctx.fillText("后端已返回空结果，等待写入 app_orchard_trees / app_tree_sensor_records", width / 2, height / 2 + 18);
}

function renderOrchardMap() {
  renderOrchardLegend();

  const canvas = $("orchardMap");
  if (!canvas) return;
  const ctx = canvas.getContext("2d");
  const W = canvas.width;
  const H = canvas.height;
  const backgroundCacheKey = `${W}x${H}`;

  if (orchardState.backgroundCacheKey !== backgroundCacheKey || !orchardState.backgroundImageData) {
    orchardState.backgroundCacheKey = backgroundCacheKey;

    function hash(x, y) {
      const dot = x * 12.9898 + y * 78.233;
      const sin = Math.sin(dot) * 43758.5453;
      return sin - Math.floor(sin);
    }

    function vnoise(x, y) {
      const i = Math.floor(x);
      const j = Math.floor(y);
      const fx = x - i;
      const fy = y - j;
      const u = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
      const v = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);
      const n00 = hash(i, j);
      const n10 = hash(i + 1.0, j);
      const n01 = hash(i, j + 1.0);
      const n11 = hash(i + 1.0, j + 1.0);
      return n00 + u * (n10 - n00) + v * ((n01 + u * (n11 - n01)) - (n00 + u * (n10 - n00)));
    }

    function fbm(x, y) {
      let val = 0.0;
      let amp = 0.5;
      let freq = 1.0;
      let maxVal = 0.0;
      for (let i = 0; i < 6; i++) {
        val += amp * vnoise(x * freq, y * freq);
        maxVal += amp;
        freq *= 2.0;
        amp *= 0.5;
      }
      return val / maxVal;
    }

    const topoColors = [
      [152, 251, 152],
      [144, 238, 144],
      [189, 252, 201],
      [255, 255, 224],
      [255, 218, 185],
      [255, 182, 193],
      [255, 160, 122]
    ];

    const numColors = topoColors.length;
    const imgData = ctx.createImageData(W, H);
    const pxData = imgData.data;
    const scale = 0.012;

    for (let py = 0; py < H; py++) {
      for (let px = 0; px < W; px++) {
        const val = fbm(px * scale, py * scale);
        const pos = val * (numColors - 1);
        const lo = Math.min(Math.floor(pos), numColors - 2);
        const hi = lo + 1;
        const t = pos - lo;
        const c0 = topoColors[lo];
        const c1 = topoColors[hi];
        let r = c0[0] + (c1[0] - c0[0]) * t;
        let g = c0[1] + (c1[1] - c0[1]) * t;
        let b = c0[2] + (c1[2] - c0[2]) * t;

        const frac = pos % 1.0;
        const edgeDist = Math.abs(frac - 0.5);
        const glow = Math.max(0.0, (0.2 - edgeDist) / 0.2);
        r = Math.min(255, r + glow * 18);
        g = Math.min(255, g + glow * 28);
        b = Math.min(255, b + glow * 42);

        const off = (py * W + px) * 4;
        pxData[off] = r;
        pxData[off + 1] = g;
        pxData[off + 2] = b;
        pxData[off + 3] = 255;
      }
    }

    orchardState.backgroundImageData = imgData;
  }

  orchardState.trees = buildOrchardTrees(W, H);

  ctx.clearRect(0, 0, W, H);
  ctx.putImageData(orchardState.backgroundImageData, 0, 0);

  ctx.strokeStyle = "rgba(255, 255, 255, 0.03)";
  ctx.lineWidth = 1;
  for (let i = 0; i <= W; i += 40) {
    ctx.beginPath();
    ctx.moveTo(i, 0);
    ctx.lineTo(i, H);
    ctx.stroke();
  }
  for (let j = 0; j <= H; j += 40) {
    ctx.beginPath();
    ctx.moveTo(0, j);
    ctx.lineTo(W, j);
    ctx.stroke();
  }

  ctx.strokeStyle = "rgba(59, 130, 246, 0.6)";
  ctx.lineWidth = 2;
  const len = 16;
  ctx.beginPath(); ctx.moveTo(0, len); ctx.lineTo(0, 0); ctx.lineTo(len, 0); ctx.stroke();
  ctx.beginPath(); ctx.moveTo(W - len, 0); ctx.lineTo(W, 0); ctx.lineTo(W, len); ctx.stroke();
  ctx.beginPath(); ctx.moveTo(0, H - len); ctx.lineTo(0, H); ctx.lineTo(len, H); ctx.stroke();
  ctx.beginPath(); ctx.moveTo(W - len, H); ctx.lineTo(W, H); ctx.lineTo(W, H - len); ctx.stroke();

  ctx.fillStyle = "rgba(15, 23, 42, 0.52)";
  ctx.fillRect(W - 244, H - 28, 228, 18);
  ctx.fillStyle = "rgba(226, 232, 240, 0.86)";
  ctx.font = "11px Segoe UI";
  ctx.textAlign = "left";
  ctx.fillText("坐标系 X: 0-500  Y: 0-500", W - 232, H - 14);

  if (orchardState.trees.length === 0) {
    renderOrchardEmptyState(ctx, W, H);
    bindOrchardMapInteractions();
    return;
  }

  orchardState.trees.forEach((tree) => {
    drawOrchardTree(ctx, tree, tree.id === orchardState.hoveredTreeId);
  });

  bindOrchardMapInteractions();
}

async function fetchOrchardOverview() {
  if (!currentAdminUsername) {
    showToast("未获取到当前管理员用户名", "error");
    return false;
  }

  const resp = await postJson("/user/admin/orchard/overview", {
    username: currentAdminUsername,
  });
  if (!resp.ok) {
    showToast(readErrorMessage(resp.data, "获取果园树位数据失败"), "error");
    orchardState.rawTrees = [];
    orchardState.legend = [];
    orchardState.summary = null;
    orchardState.hoveredTreeId = null;
    renderOrchardMap();
    return false;
  }

  const payload = unwrapApiPayload(resp.data);
  orchardState.rawTrees = Array.isArray(payload.trees) ? payload.trees : [];
  orchardState.legend = Array.isArray(payload.legend) ? payload.legend : [];
  orchardState.summary = payload.summary || null;
  orchardState.bounds = {
    ...DEFAULT_ORCHARD_BOUNDS,
    ...(payload.coordinate_range || {}),
  };
  if (!orchardState.rawTrees.some((item) => (item?.tree_code || "") === orchardState.hoveredTreeId)) {
    orchardState.hoveredTreeId = null;
  }
  renderOrchardMap();
  return true;
}

function bindInviteActions() {
  $("btnCreateInvite").addEventListener("click", createInvite);
  $("btnListInvites").addEventListener("click", async () => {
    if (await listInvites()) showToast("邀请码已刷新");
  });
  $("btnListUsers").addEventListener("click", async () => {
    if (await listUsers()) showToast("用户列表已刷新");
  });
  $("btnListPending").addEventListener("click", async () => {
    if (await listPending()) showToast("待审批列表已刷新");
  });
}

async function fetchStats() {
  const resp = await postJson("/user/admin/dashboard/stats");
  if (!resp.ok) {
    showToast(readErrorMessage(resp.data, "获取统计数据失败"), "error");
    return false;
  }

  const payload = unwrapApiPayload(resp.data);
  const totals = payload.totals || {};
  const environment = payload.environment || {};
  const dailyCounts = Array.isArray(payload.daily_counts) ? payload.daily_counts : [];

  const total = Number(totals.detections || 0);
  const healthyCount = Number(totals.healthy_count || 0);
  const diseasedCount = Number(totals.diseased_count || 0);
  const healthyRate = Number(totals.healthy_rate || 0);
  const userCount = Number(totals.users || 0);
  const adminCount = Number(totals.admins || 0);
  const pendingCount = Number(totals.pending_users || 0);

  $("statDetections").textContent = total;
  $("statDetectionsTrend").textContent = `近7天共 ${dailyCounts.reduce((sum, item) => sum + Number(item.count || 0), 0)} 条`;
  $("statDetectionsTrend").className = "stat-trend neutral";

  $("statHealthy").textContent = healthyRate + "%";
  $("statHealthyTrend").textContent = `${healthyCount} / ${total} 为健康`;
  $("statHealthyTrend").className = healthyRate > 70 ? "stat-trend up" : "stat-trend down";

  $("statDiseased").textContent = diseasedCount;
  $("statDiseasedTrend").textContent = diseasedCount > 0 ? `当前 ${pendingCount} 条待处理事项` : "暂无病害";
  $("statDiseasedTrend").className = diseasedCount > 0 ? "stat-trend down" : "stat-trend up";

  $("statUsers").textContent = userCount;
  $("statUsersTrend").textContent = `${adminCount} 位管理员`;
  $("statUsersTrend").className = "stat-trend neutral";

  renderDetectionChart(dailyCounts);

  const temp = environment.current_temperature;
  const hum = environment.current_humidity;
  const healthScore = environment.health_score;
  const alerts = environment.active_alerts;

  $("envTemp").textContent = Number.isFinite(temp) ? temp + " °C" : "-- °C";
  $("envTempRing").textContent = Number.isFinite(temp) ? Math.round(temp) : "--";
  $("envHumidity").textContent = Number.isFinite(hum) ? hum + " %" : "-- %";
  $("envHumRing").textContent = Number.isFinite(hum) ? Math.round(hum) : "--";

  $("envHealth").textContent = Number.isFinite(healthScore) ? healthScore + " 分" : "-- 分";
  $("envHealthRing").textContent = Number.isFinite(healthScore) ? healthScore : "--";
  const alertCount = Number(alerts || 0);
  $("envAlerts").textContent = alertCount + " 条";
  $("envAlertRing").textContent = alertCount > 0 ? "⚡" : "✓";
  return true;
}

function renderDetectionChart(series) {
  const chart = $("detectionChart");
  if (!chart) return;

  if (!Array.isArray(series) || series.length === 0) {
    chart.innerHTML = '<div class="empty-state" style="width:100%; padding:24px 0;">暂无趋势数据</div>';
    $("chartLabelStart").textContent = "--";
    $("chartLabelEnd").textContent = "--";
    return;
  }

  const counts = series.map((item) => Number(item.count || 0));
  const labels = series.map((item) => item.date || "--");

  const max = Math.max(...counts, 1);
  $("chartLabelStart").textContent = labels[0];
  $("chartLabelEnd").textContent = labels[labels.length - 1];

  chart.innerHTML = counts.map((count, index) => {
    const h = Math.max(4, (count / max) * 100);
    const isToday = index === counts.length - 1;
    const color = isToday ? "var(--primary)" : "rgba(59,130,246,0.4)";
    return `<div class="bar" style="height:${h}%; background:${color};" title="${escapeHtml(labels[index])}: ${count} 次检测"></div>`;
  }).join("");
}

async function fetchLogs() {
  const resp = await postJson("/user/admin/dashboard/logs");
  if (!resp.ok) {
    showToast(readErrorMessage(resp.data, "获取日志失败"), "error");
    return false;
  }

  const payload = unwrapApiPayload(resp.data);
  allLogs = Array.isArray(payload.logs) ? payload.logs : [];
  renderLogs();
  return true;
}

function parseTime(t) {
  if (!t) return "未知时间";
  let d;
  if (typeof t === "number") d = new Date(t > 1e12 ? t : t * 1000);
  else d = new Date(t);
  return isNaN(d.getTime()) ? "未知时间" : d.toLocaleString();
}

function renderLogs() {
  const filter = $("logFilter").value;
  const filtered = filter === "all" ? allLogs : allLogs.filter((item) => item.type === filter);

  if (filtered.length === 0) {
    $("logList").innerHTML = "<div class='empty-state'>暂无日志记录</div>";
    return;
  }

  $("logList").innerHTML = filtered.slice(0, 100).map((item) => `
    <div class="log-item">
      <div class="log-dot ${escapeHtml(item.type || "action")}"></div>
      <div class="log-content">
        <div class="log-text">${escapeHtml(item.message || "")}</div>
        <div class="log-time">${parseTime(item.created_at)}</div>
      </div>
    </div>
  `).join("");
}

function applySettingsToForm(settings) {
  $("settingOpenReg").checked = Boolean(settings.open_registration);
  $("settingInviteBypass").checked = Boolean(settings.invite_bypass_enabled);
  $("settingMaintenance").checked = Boolean(settings.maintenance_mode);
  $("settingDefaultTTL").value = String(settings.default_invite_ttl_seconds || 86400);
  $("settingThreshold").value = String(settings.confidence_threshold || 0.75);
  $("settingThresholdVal").textContent = Math.round(Number(settings.confidence_threshold || 0.75) * 100) + "%";
  $("settingLogRetention").value = String(settings.log_retention_days || 30);
  $("ttlSelect").value = String(settings.default_invite_ttl_seconds || 86400);
}

async function loadSettings() {
  const resp = await postJson("/user/admin/settings/get");
  if (!resp.ok) {
    showToast(readErrorMessage(resp.data, "获取系统设置失败"), "error");
    return false;
  }

  const payload = unwrapApiPayload(resp.data);
  applySettingsToForm(payload.settings || payload);
  return true;
}

async function saveSettings() {
  const btn = $("btnSaveSettings");
  btn.disabled = true;
  btn.innerText = "保存中...";

  const resp = await postJson("/user/admin/settings/update", {
    open_registration: $("settingOpenReg").checked,
    invite_bypass_enabled: $("settingInviteBypass").checked,
    maintenance_mode: $("settingMaintenance").checked,
    default_invite_ttl_seconds: Number($("settingDefaultTTL").value),
    confidence_threshold: Number($("settingThreshold").value),
    log_retention_days: Number($("settingLogRetention").value),
  });

  btn.disabled = false;
  btn.innerText = "保存设置";

  if (!resp.ok) {
    showToast(readErrorMessage(resp.data, "保存设置失败"), "error");
    return;
  }

  const payload = unwrapApiPayload(resp.data);
  applySettingsToForm(payload.settings || payload);
  await fetchLogs();
  showToast("设置已保存");
}

function bindSettingsActions() {
  $("settingThreshold").addEventListener("input", function () {
    $("settingThresholdVal").textContent = Math.round(this.value * 100) + "%";
  });
  $("btnSaveSettings").addEventListener("click", saveSettings);
}

function bindDashboardActions() {
  $("btnRefreshStats").addEventListener("click", async () => {
    if (await fetchStats()) showToast("统计数据已刷新");
  });
  $("btnRefreshEnv").addEventListener("click", async () => {
    const [statsOk, orchardOk] = await Promise.all([fetchStats(), fetchOrchardOverview()]);
    if (statsOk || orchardOk) showToast("环境数据已刷新");
  });
  $("btnRefreshLogs").addEventListener("click", async () => {
    if (await fetchLogs()) showToast("日志已刷新");
  });
  $("logFilter").addEventListener("change", renderLogs);
}

const commerceAdminState = {
  orchards: [],
  products: [],
  batches: [],
  orders: [],
};

const commerceBatchStatusLabels = {
  draft: "草稿",
  preorder: "预售中",
  open: "开团中",
  closed: "已截团",
  harvesting: "采摘中",
  shipping: "发货中",
  completed: "已完成",
  cancelled: "已取消",
};

const commerceOrderStatusLabels = {
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

const commercePaymentStatusLabels = {
  unpaid: "未收款",
  deposit_paid: "已收订金",
  paid: "已收全款",
  refunded: "已退款",
};

async function commerceRequest(url, method = "GET", body) {
  try {
    const hasBody = body !== undefined;
    const res = await fetch(url, {
      method,
      credentials: "same-origin",
      headers: requestHeaders(hasBody),
      body: hasBody ? JSON.stringify(body) : undefined,
    });
    const text = await res.text();
    let data;
    try {
      data = JSON.parse(text);
    } catch {
      data = { raw: text };
    }
    if (!res.ok) throw new Error(readErrorMessage(data, "商业接口请求失败"));
    return unwrapApiPayload(data);
  } catch (error) {
    throw new Error(error.message || "网络请求失败");
  }
}

function commerceMoney(cents) {
  return `¥${(Number(cents || 0) / 100).toFixed(2)}`;
}

function commerceDate(timestamp) {
  if (!timestamp) return "未设置";
  const date = new Date(Number(timestamp));
  return Number.isNaN(date.getTime()) ? "未设置" : date.toLocaleString("zh-CN");
}

function commerceDateInput(id) {
  const value = $(id).value;
  if (!value) return null;
  const timestamp = new Date(value).getTime();
  return Number.isFinite(timestamp) ? timestamp : null;
}

function renderCommerceOrchards() {
  const wrap = $("commerceOrchardsWrap");
  if (!commerceAdminState.orchards.length) {
    wrap.innerHTML = "<div class='empty-state'>暂无合作果园</div>";
  } else {
    wrap.innerHTML = commerceAdminState.orchards.map((orchard) => `
      <div class="commerce-admin-list__item">
        <div><strong>${escapeHtml(orchard.name)}</strong><small>${escapeHtml(orchard.location || "未填写所在地")} · ${escapeHtml(orchard.farmer_name || "未填写负责人")}</small></div>
        <span class="pill ${orchard.is_active ? "admin" : "user"}">${orchard.is_active ? "启用" : "停用"}</span>
      </div>
    `).join("");
  }

  const select = $("commerceBatchOrchard");
  const current = select.value;
  select.innerHTML = commerceAdminState.orchards.length
    ? `<option value="">请选择合作果园</option>${commerceAdminState.orchards.map((orchard) => `<option value="${orchard.id}">${escapeHtml(orchard.name)}</option>`).join("")}`
    : '<option value="">请先创建果园</option>';
  if (commerceAdminState.orchards.some((orchard) => String(orchard.id) === current)) select.value = current;
}

function renderCommerceProducts() {
  const wrap = $("commerceProductsWrap");
  if (!commerceAdminState.products.length) {
    wrap.innerHTML = "<div class='empty-state'>暂无商品规格</div>";
  } else {
    wrap.innerHTML = commerceAdminState.products.map((product) => `
      <div class="commerce-admin-list__item">
        <div><strong>${escapeHtml(product.name)}</strong><small>${escapeHtml(product.sku)} · ${escapeHtml(product.unit_label)}</small></div>
        <span class="commerce-admin-list__price">${commerceMoney(product.price_cents)}</span>
      </div>
    `).join("");
  }

  const quotaWrap = $("commerceBatchProducts");
  quotaWrap.innerHTML = commerceAdminState.products.length
    ? commerceAdminState.products.map((product) => `
      <label class="commerce-product-quota__item">
        <span>${escapeHtml(product.name)} · ${commerceMoney(product.price_cents)}</span>
        <input type="number" min="0" value="0" data-batch-product-id="${product.id}" aria-label="${escapeHtml(product.name)}配额" />
      </label>
    `).join("")
    : '<div class="empty-state">先创建商品后配置批次商品配额。</div>';
}

function renderCommerceBatches() {
  const wrap = $("commerceBatchesWrap");
  if (!commerceAdminState.batches.length) {
    wrap.innerHTML = "<div class='empty-state'>暂无销售批次</div>";
    return;
  }
  const rows = commerceAdminState.batches.map((batch) => `
    <tr>
      <td><strong>${escapeHtml(batch.batch_code)}</strong><br /><small>${escapeHtml(batch.title)}</small></td>
      <td>${escapeHtml(batch.orchard?.name || "-")}</td>
      <td><span class="pill ${batch.status === "open" || batch.status === "preorder" ? "admin" : "user"}">${commerceBatchStatusLabels[batch.status] || escapeHtml(batch.status)}</span></td>
      <td>${batch.planned_quantity} 箱</td>
      <td>${commerceDate(batch.harvest_start_at)}<br />${commerceDate(batch.ship_at)}</td>
    </tr>
  `).join("");
  wrap.innerHTML = `<table><thead><tr><th>批次</th><th>合作果园</th><th>状态</th><th>计划数量</th><th>采摘 / 发货</th></tr></thead><tbody>${rows}</tbody></table>`;
}

function renderCommerceOrders() {
  const wrap = $("commerceOrdersWrap");
  if (!commerceAdminState.orders.length) {
    wrap.innerHTML = "<div class='empty-state'>暂无订单</div>";
    return;
  }

  const orderStatusOptions = Object.entries(commerceOrderStatusLabels).map(([value, label]) => `<option value="${value}">${label}</option>`).join("");
  const paymentStatusOptions = Object.entries(commercePaymentStatusLabels).map(([value, label]) => `<option value="${value}">${label}</option>`).join("");
  const rows = commerceAdminState.orders.map((order) => `
    <tr>
      <td><strong>${escapeHtml(order.order_no)}</strong><br /><small>${escapeHtml(order.username)}</small></td>
      <td>${escapeHtml(order.recipient_name)}<br /><small>${escapeHtml(order.recipient_phone)}<br />${escapeHtml(order.shipping_address)}</small></td>
      <td>${(order.items || []).map((item) => `${escapeHtml(item.product_name)} × ${item.quantity}`).join("<br />")}</td>
      <td class="commerce-admin-list__price">${commerceMoney(order.total_cents)}</td>
      <td>
        <select data-order-status="${escapeHtml(order.id)}">${orderStatusOptions.replace(`value="${order.status}"`, `value="${order.status}" selected`)}</select>
        <select data-order-payment="${escapeHtml(order.id)}" style="margin-top:6px;">${paymentStatusOptions.replace(`value="${order.payment_status}"`, `value="${order.payment_status}" selected`)}</select>
      </td>
      <td><button class="ghost" type="button" data-save-order="${escapeHtml(order.id)}">保存</button></td>
    </tr>
  `).join("");
  wrap.innerHTML = `<table class="commerce-orders-table"><thead><tr><th>订单</th><th>收货信息</th><th>商品</th><th>金额</th><th>状态 / 收款</th><th>操作</th></tr></thead><tbody>${rows}</tbody></table>`;

  wrap.querySelectorAll("[data-save-order]").forEach((button) => {
    button.addEventListener("click", () => updateCommerceOrder(button.dataset.saveOrder));
  });
}

async function loadCommerceOverview() {
  const data = await commerceRequest("/user/admin/commerce/overview", "POST");
  const orders = data.orders || {};
  $("commerceBatchCount").textContent = data.active_batch_count ?? 0;
  $("commerceOrderCount").textContent = orders.count ?? 0;
  $("commerceGrossAmount").textContent = commerceMoney(orders.gross_amount_cents);
  $("commerceActiveOrders").textContent = orders.active_count ?? 0;
}

async function loadCommerceOrchards() {
  const data = await commerceRequest("/user/admin/commerce/orchards");
  commerceAdminState.orchards = data.orchards || [];
  renderCommerceOrchards();
}

async function loadCommerceProducts() {
  const data = await commerceRequest("/user/admin/commerce/products");
  commerceAdminState.products = data.products || [];
  renderCommerceProducts();
}

async function loadCommerceBatches() {
  const data = await commerceRequest("/user/admin/commerce/batches");
  commerceAdminState.batches = data.batches || [];
  renderCommerceBatches();
}

async function loadCommerceOrders() {
  const data = await commerceRequest("/user/admin/commerce/orders", "POST");
  commerceAdminState.orders = data.orders || [];
  renderCommerceOrders();
}

async function refreshCommerceData(showMessage = false) {
  try {
    await Promise.all([
      loadCommerceOverview(),
      loadCommerceOrchards(),
      loadCommerceProducts(),
      loadCommerceBatches(),
      loadCommerceOrders(),
    ]);
    if (showMessage) showToast("开团运营数据已刷新");
  } catch (error) {
    showToast(error.message || "加载开团运营数据失败", "error");
  }
}

async function createCommerceOrchard(event) {
  event.preventDefault();
  const form = event.currentTarget;
  try {
    await commerceRequest("/user/admin/commerce/orchards", "POST", {
      name: $("commerceOrchardName").value.trim(),
      location: $("commerceOrchardLocation").value.trim(),
      farmer_name: $("commerceOrchardFarmer").value.trim(),
      description: $("commerceOrchardDescription").value.trim(),
    });
    form.reset();
    showToast("合作果园已创建");
    await Promise.all([loadCommerceOrchards(), loadCommerceOverview()]);
  } catch (error) {
    showToast(error.message, "error");
  }
}

async function createCommerceProduct(event) {
  event.preventDefault();
  const form = event.currentTarget;
  try {
    await commerceRequest("/user/admin/commerce/products", "POST", {
      name: $("commerceProductName").value.trim(),
      sku: $("commerceProductSku").value.trim(),
      unit_label: $("commerceProductUnit").value.trim(),
      price_cents: Math.round(Number($("commerceProductPrice").value) * 100),
      deposit_cents: Math.round(Number($("commerceProductDeposit").value || 0) * 100),
    });
    form.reset();
    $("commerceProductDeposit").value = "0";
    showToast("商品规格已创建");
    await loadCommerceProducts();
  } catch (error) {
    showToast(error.message, "error");
  }
}

async function createCommerceBatch(event) {
  event.preventDefault();
  const products = [...document.querySelectorAll("[data-batch-product-id]")]
    .map((input) => ({ product_id: Number(input.dataset.batchProductId), quota: Number(input.value || 0) }))
    .filter((item) => item.quota > 0);
  if (!products.length) {
    showToast("请至少配置一个批次商品配额", "error");
    return;
  }

  try {
    await commerceRequest("/user/admin/commerce/batches", "POST", {
      batch_code: $("commerceBatchCode").value.trim(),
      title: $("commerceBatchTitle").value.trim(),
      orchard_id: Number($("commerceBatchOrchard").value),
      status: $("commerceBatchStatus").value,
      planned_quantity: Number($("commerceBatchQuantity").value),
      open_at: commerceDateInput("commerceBatchOpenAt"),
      close_at: commerceDateInput("commerceBatchCloseAt"),
      harvest_start_at: commerceDateInput("commerceBatchHarvestStart"),
      harvest_end_at: commerceDateInput("commerceBatchHarvestEnd"),
      ship_at: commerceDateInput("commerceBatchShipAt"),
      products,
    });
    event.currentTarget.reset();
    renderCommerceProducts();
    showToast("销售批次已创建");
    await Promise.all([loadCommerceBatches(), loadCommerceOverview()]);
  } catch (error) {
    showToast(error.message, "error");
  }
}

async function updateCommerceOrder(orderId) {
  const status = document.querySelector(`[data-order-status="${CSS.escape(orderId)}"]`)?.value;
  const paymentStatus = document.querySelector(`[data-order-payment="${CSS.escape(orderId)}"]`)?.value;
  if (!status || !paymentStatus) return;
  const note = window.prompt("履约备注（可选）", "") ?? "";
  try {
    await commerceRequest("/user/admin/commerce/orders/status", "POST", {
      order_id: orderId,
      status,
      payment_status: paymentStatus,
      note,
    });
    showToast("订单状态已更新");
    await Promise.all([loadCommerceOrders(), loadCommerceOverview()]);
  } catch (error) {
    showToast(error.message, "error");
  }
}

function bindCommerceActions() {
  $("commerceOrchardForm").addEventListener("submit", createCommerceOrchard);
  $("commerceProductForm").addEventListener("submit", createCommerceProduct);
  $("commerceBatchForm").addEventListener("submit", createCommerceBatch);
  $("btnRefreshCommerceOrchards").addEventListener("click", async () => {
    await loadCommerceOrchards();
    showToast("合作果园已刷新");
  });
  $("btnRefreshCommerceProducts").addEventListener("click", async () => {
    await loadCommerceProducts();
    showToast("商品规格已刷新");
  });
  $("btnRefreshCommerceBatches").addEventListener("click", async () => {
    await Promise.all([loadCommerceBatches(), loadCommerceOverview()]);
    showToast("销售批次已刷新");
  });
  $("btnRefreshCommerceOrders").addEventListener("click", async () => {
    await Promise.all([loadCommerceOrders(), loadCommerceOverview()]);
    showToast("订单已刷新");
  });
}

function bindEvents() {
  bindAdminSectionNavigation();
  bindInviteActions();
  bindDashboardActions();
  bindSettingsActions();
}

async function initPage() {
  const session = await validateAdmin();
  if (!session) {
    alert("无权限访问或登录已过期，请重新登录。");
    window.location.href = "/";
    return;
  }
  currentAdminUsername = typeof session.username === "string" ? session.username : "";
  bindEvents();
  renderOrchardMap();
  await Promise.all([
    listInvites(),
    listUsers(),
    listPending(),
    loadSettings(),
    fetchStats(),
    fetchLogs(),
    fetchOrchardOverview(),
  ]);
}

exposeGlobalActions();
void initPage();
})();
