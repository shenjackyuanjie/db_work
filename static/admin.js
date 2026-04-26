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
  if (!token) return false;
  const resp = await postJson("/user/validate");
  return resp.ok && resp.data.valid && resp.data.is_admin;
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
let allLogs = [];

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

function formatHealthIndex(value) {
  const numeric = Number(value);
  return Number.isFinite(numeric) ? Math.round(numeric * 100) : null;
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
    const healthIndex = isFiniteNumber(sensor.health_index) ? Number(sensor.health_index) : null;
    const palette = orchardTreePalette(item?.status?.level || "offline", item?.status?.color || "");
    const canopyRadius = 5.1 + (healthIndex == null ? 1.8 : (1 - healthIndex) * 2.6) + ((Number(item?.id || index) % 3) * 0.35);

    return {
      id: treeCode,
      dbId: Number(item?.id || 0),
      treeCode,
      label: item?.status?.label || "果树",
      statusLevel: item?.status?.level || "offline",
      statusColor: item?.status?.color || palette.accent,
      variety: item?.variety || "未知品种",
      rawX,
      rawY,
      x: mapped.x,
      y: mapped.y,
      terrainHeight: Number(item?.terrain_height || 0),
      canopyRadius,
      hitRadius: canopyRadius + 6,
      temperature: isFiniteNumber(sensor.temperature) ? Number(sensor.temperature) : null,
      humidity: isFiniteNumber(sensor.humidity) ? Number(sensor.humidity) : null,
      nitrogen: isFiniteNumber(sensor.nitrogen) ? Number(sensor.nitrogen) : null,
      phosphorus: isFiniteNumber(sensor.phosphorus) ? Number(sensor.phosphorus) : null,
      potassium: isFiniteNumber(sensor.potassium) ? Number(sensor.potassium) : null,
      healthIndex,
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
  const healthScore = formatHealthIndex(tree.healthIndex);
  const diagnosisText = tree.latestDiagnosis?.disease_name || tree.latestDiagnosis?.predicted_class || "无";

  tooltip.innerHTML = `
    <div class="orchard-tooltip__eyebrow">${escapeHtml(tree.label)} · ${escapeHtml(tree.variety)}</div>
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
      <div class="orchard-tooltip__meta-item">
        <span>N / P / K</span>
        <strong>${formatMetric(tree.nitrogen, 0)} / ${formatMetric(tree.phosphorus, 0)} / ${formatMetric(tree.potassium, 0)}</strong>
      </div>
      <div class="orchard-tooltip__meta-item">
        <span>健康指数</span>
        <strong>${healthScore == null ? "--" : healthScore}</strong>
      </div>
    </div>
    <div style="margin-top: 10px; font-size: 11px; color: var(--muted); line-height: 1.45;">
      诊断结果：${escapeHtml(diagnosisText)}<br>
      最近采样：${escapeHtml(tree.sampledAt ? parseTime(tree.sampledAt) : "暂无")}
    </div>
  `;

  tooltip.style.borderColor = `${tree.palette.accent}66`;
  tooltip.style.setProperty("--orchard-tooltip-border", `${tree.palette.accent}66`);
  tooltip.style.left = `${Math.max(90, Math.min(rect.width - 90, offsetX))}px`;
  tooltip.style.top = `${Math.max(24, Math.min(rect.height - 24, offsetY))}px`;
  tooltip.classList.toggle("below", offsetY < 96);
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
  const avgHealth = isFiniteNumber(summary.average_health_index)
    ? `${Math.round(Number(summary.average_health_index) * 100)}`
    : "--";

  legend.innerHTML = items.map((item) => `
    <div style="display: flex; align-items: center; gap: 6px;">
      <div style="width: 10px; height: 10px; border-radius: 50%; background: ${item.color}; box-shadow: 0 0 8px ${item.color};"></div>
      <span>${escapeHtml(item.label)} <strong style="color:var(--text)">${Number(item.count || 0)}</strong></span>
    </div>
  `).join("") + `
    <div style="display: flex; align-items: center; gap: 10px; padding-left: 8px; border-left: 1px solid rgba(148, 163, 184, 0.18);">
      <span>在线 ${onlineTrees}/${totalTrees}</span>
      <span>均值 ${avgHealth}</span>
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
  const resp = await postJson("/user/admin/orchard/overview");
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

function bindEvents() {
  bindInviteActions();
  bindDashboardActions();
  bindSettingsActions();
}

async function initPage() {
  if (!(await validateAdmin())) {
    alert("无权限访问或登录已过期，请重新登录。");
    window.location.href = "/index.html";
    return;
  }
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
