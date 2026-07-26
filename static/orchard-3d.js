import * as THREE from "three";
import { OrbitControls } from "https://cdn.jsdelivr.net/npm/three@0.163.0/examples/jsm/controls/OrbitControls.js";

const $ = (id) => document.getElementById(id);
const SESSION_COOKIE = "session_token";
const DEFAULT_ORCHARD_BOUNDS = Object.freeze({
  min_x: 0,
  max_x: 500,
  min_y: 0,
  max_y: 500,
});
const WORLD_SIZE = Object.freeze({
  width: 180,
  depth: 180,
});
const TREE_BASE_SCALE = 0.9;
const TOPO_COLORS = Object.freeze([
  [152, 251, 152],
  [144, 238, 144],
  [189, 252, 201],
  [255, 255, 224],
  [255, 218, 185],
  [255, 182, 193],
  [255, 160, 122],
]);

const state = {
  adminUsername: "",
  bounds: { ...DEFAULT_ORCHARD_BOUNDS },
  trees: [],
  legend: [],
  summary: null,
  weather: null,
  selectedTreeId: null,
  hoveredTreeId: null,
  treeMeshes: [],
  autoRotate: true,
  sceneReady: false,
};

const viewport = {
  renderer: null,
  scene: null,
  camera: null,
  controls: null,
  raycaster: new THREE.Raycaster(),
  pointer: new THREE.Vector2(),
  orchardGroup: null,
  terrainMesh: null,
  clock: new THREE.Clock(),
  width: 0,
  height: 0,
};

function disposeSceneNode(node) {
  node.traverse((child) => {
    if (child.geometry) child.geometry.dispose();
    if (child.material) {
      const materials = Array.isArray(child.material) ? child.material : [child.material];
      materials.forEach((material) => {
        const textures = new Set();
        ["map", "emissiveMap", "alphaMap", "normalMap", "roughnessMap", "metalnessMap"].forEach((key) => {
          if (material[key]?.dispose) textures.add(material[key]);
        });
        textures.forEach((texture) => texture.dispose());
        material.dispose();
      });
    }
  });
}

function getCookie(name) {
  const prefix = `${name}=`;
  const part = document.cookie
    .split(";")
    .map((item) => item.trim())
    .find((item) => item.startsWith(prefix));

  return part ? decodeURIComponent(part.slice(prefix.length)) : "";
}

function logout() {
  document.cookie = `${SESSION_COOKIE}=; path=/; max-age=0; samesite=lax`;
  window.location.href = "/";
}

function requestHeaders(withJsonBody) {
  const headers = {};
  if (withJsonBody) headers["Content-Type"] = "application/json";

  const token = getCookie(SESSION_COOKIE);
  if (token) headers["X-Session-Token"] = token;
  return headers;
}

async function postJson(url, body) {
  const withBody = body !== undefined;

  try {
    const response = await fetch(url, {
      method: "POST",
      credentials: "same-origin",
      headers: requestHeaders(withBody),
      body: withBody ? JSON.stringify(body) : undefined,
    });

    const text = await response.text();
    try {
      return { ok: response.ok, data: JSON.parse(text) };
    } catch {
      return { ok: response.ok, data: { raw: text } };
    }
  } catch {
    return { ok: false, data: { error: "网络请求失败" } };
  }
}

function unwrapApiPayload(payload) {
  if (payload && typeof payload === "object" && payload.data != null) {
    return payload.data;
  }
  return payload || {};
}

function escapeHtml(text) {
  return String(text ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function formatDate(unixSeconds) {
  const numeric = Number(unixSeconds);
  if (!Number.isFinite(numeric) || numeric <= 0) return "--";
  return new Date(numeric > 1e12 ? numeric : numeric * 1000).toLocaleString();
}

function formatMetric(value, suffix = "", digits = 1) {
  const numeric = Number(value);
  if (!Number.isFinite(numeric)) return "--";
  return `${numeric.toFixed(digits)}${suffix}`;
}

function statusColor(level, accent) {
  if (accent) return accent;

  switch (level) {
    case "healthy":
      return "#53d37a";
    case "attention":
      return "#f5c86b";
    case "warning":
      return "#ff6a6a";
    case "critical":
      return "#9f7bff";
    default:
      return "#7f96a8";
  }
}

function computeAverages(trees) {
  const samples = trees.filter((tree) => Number.isFinite(tree.temperature) || Number.isFinite(tree.humidity));
  if (samples.length === 0) {
    return { avgTemp: null, avgHumidity: null };
  }

  const tempValues = samples.map((tree) => tree.temperature).filter(Number.isFinite);
  const humidityValues = samples.map((tree) => tree.humidity).filter(Number.isFinite);
  const avgTemp = tempValues.length ? tempValues.reduce((sum, value) => sum + value, 0) / tempValues.length : null;
  const avgHumidity = humidityValues.length ? humidityValues.reduce((sum, value) => sum + value, 0) / humidityValues.length : null;
  return { avgTemp, avgHumidity };
}

function createScene() {
  const canvas = $("orchardSceneCanvas");
  const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.toneMapping = THREE.ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.04;

  const scene = new THREE.Scene();
  scene.fog = new THREE.FogExp2("#7ca7d8", 0.0026);

  const camera = new THREE.PerspectiveCamera(48, 1, 0.1, 800);
  camera.position.set(102, 88, 112);

  const controls = new OrbitControls(camera, renderer.domElement);
  controls.enableDamping = true;
  controls.dampingFactor = 0.05;
  controls.minDistance = 52;
  controls.maxDistance = 240;
  controls.maxPolarAngle = Math.PI * 0.47;
  controls.target.set(0, 10, 0);
  controls.autoRotate = true;
  controls.autoRotateSpeed = 0.55;

  const ambientLight = new THREE.AmbientLight("#f8fbff", 0.5);
  scene.add(ambientLight);

  const hemiLight = new THREE.HemisphereLight("#dbeafe", "#46553c", 1.38);
  scene.add(hemiLight);

  const keyLight = new THREE.DirectionalLight("#fff6d8", 2.2);
  keyLight.position.set(52, 104, 46);
  scene.add(keyLight);

  const fillLight = new THREE.DirectionalLight("#bfdbfe", 0.72);
  fillLight.position.set(-56, 58, -12);
  scene.add(fillLight);

  const rimLight = new THREE.DirectionalLight("#93c5fd", 0.72);
  rimLight.position.set(24, 44, -58);
  scene.add(rimLight);

  const orchardGroup = new THREE.Group();
  orchardGroup.name = "orchard-root";
  scene.add(orchardGroup);

  const groundGrid = new THREE.GridHelper(210, 20, "#60a5fa", "#334155");
  groundGrid.position.y = 0.1;
  groundGrid.material.transparent = true;
  groundGrid.material.opacity = 0.2;
  scene.add(groundGrid);

  const border = new THREE.LineSegments(
    new THREE.EdgesGeometry(new THREE.BoxGeometry(WORLD_SIZE.width, 0.2, WORLD_SIZE.depth)),
    new THREE.LineBasicMaterial({ color: "#93c5fd", transparent: true, opacity: 0.22 })
  );
  border.position.y = 0.2;
  scene.add(border);

  viewport.renderer = renderer;
  viewport.scene = scene;
  viewport.camera = camera;
  viewport.controls = controls;
  viewport.orchardGroup = orchardGroup;

  resizeRenderer();
  renderer.domElement.addEventListener("pointermove", onPointerMove);
  renderer.domElement.addEventListener("click", onCanvasClick);
  window.addEventListener("resize", resizeRenderer);

  state.sceneReady = true;
}

function resizeRenderer() {
  const stage = $("sceneStage");
  if (!stage || !viewport.renderer || !viewport.camera) return;

  const width = stage.clientWidth;
  const height = stage.clientHeight;
  if (!width || !height || (viewport.width === width && viewport.height === height)) return;

  viewport.width = width;
  viewport.height = height;
  viewport.renderer.setSize(width, height, false);
  viewport.camera.aspect = width / height;
  viewport.camera.updateProjectionMatrix();
}

function mapToWorld(rawX, rawY) {
  const bounds = state.bounds || DEFAULT_ORCHARD_BOUNDS;
  const rangeX = Math.max(1, Number(bounds.max_x) - Number(bounds.min_x));
  const rangeY = Math.max(1, Number(bounds.max_y) - Number(bounds.min_y));
  const normalizedX = (Number(rawX || 0) - Number(bounds.min_x)) / rangeX;
  const normalizedY = (Number(rawY || 0) - Number(bounds.min_y)) / rangeY;

  return {
    x: (normalizedX - 0.5) * WORLD_SIZE.width,
    z: (normalizedY - 0.5) * WORLD_SIZE.depth,
  };
}

function hashNoise(x, y) {
  const dot = x * 12.9898 + y * 78.233;
  const sinValue = Math.sin(dot) * 43758.5453;
  return sinValue - Math.floor(sinValue);
}

function valueNoise(x, y) {
  const i = Math.floor(x);
  const j = Math.floor(y);
  const fx = x - i;
  const fy = y - j;
  const u = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
  const v = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);
  const n00 = hashNoise(i, j);
  const n10 = hashNoise(i + 1.0, j);
  const n01 = hashNoise(i, j + 1.0);
  const n11 = hashNoise(i + 1.0, j + 1.0);
  return n00 + u * (n10 - n00) + v * ((n01 + u * (n11 - n01)) - (n00 + u * (n10 - n00)));
}

function fbmNoise(x, y) {
  let value = 0.0;
  let amplitude = 0.5;
  let frequency = 1.0;
  let maxValue = 0.0;

  for (let index = 0; index < 6; index += 1) {
    value += amplitude * valueNoise(x * frequency, y * frequency);
    maxValue += amplitude;
    frequency *= 2.0;
    amplitude *= 0.5;
  }

  return value / maxValue;
}

function topoNoiseAt(x, z) {
  const normalizedX = (x + WORLD_SIZE.width / 2) / WORLD_SIZE.width;
  const normalizedZ = (z + WORLD_SIZE.depth / 2) / WORLD_SIZE.depth;
  const textureX = normalizedX * 512;
  const textureZ = normalizedZ * 512;
  return fbmNoise(textureX * 0.012, textureZ * 0.012);
}

function topoColorAt(value) {
  const numColors = TOPO_COLORS.length;
  const pos = value * (numColors - 1);
  const lo = Math.min(Math.floor(pos), numColors - 2);
  const hi = lo + 1;
  const t = pos - lo;
  const c0 = TOPO_COLORS[lo];
  const c1 = TOPO_COLORS[hi];
  let r = c0[0] + (c1[0] - c0[0]) * t;
  let g = c0[1] + (c1[1] - c0[1]) * t;
  let b = c0[2] + (c1[2] - c0[2]) * t;

  const frac = pos % 1.0;
  const edgeDist = Math.abs(frac - 0.5);
  const glow = Math.max(0.0, (0.2 - edgeDist) / 0.2);
  r = Math.min(255, r + glow * 18);
  g = Math.min(255, g + glow * 28);
  b = Math.min(255, b + glow * 42);

  return new THREE.Color(r / 255, g / 255, b / 255).offsetHSL(0, 0.035, -0.08);
}

function buildTopoTexture(size = 768) {
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d");
  const imageData = ctx.createImageData(size, size);
  const pxData = imageData.data;

  for (let py = 0; py < size; py += 1) {
    for (let px = 0; px < size; px += 1) {
      const color = topoColorAt(fbmNoise(px * 0.012, py * 0.012));
      const offset = (py * size + px) * 4;
      pxData[offset] = Math.round(color.r * 255);
      pxData[offset + 1] = Math.round(color.g * 255);
      pxData[offset + 2] = Math.round(color.b * 255);
      pxData[offset + 3] = 255;
    }
  }

  ctx.putImageData(imageData, 0, 0);

  const texture = new THREE.CanvasTexture(canvas);
  texture.colorSpace = THREE.SRGBColorSpace;
  texture.wrapS = THREE.ClampToEdgeWrapping;
  texture.wrapT = THREE.ClampToEdgeWrapping;
  texture.anisotropy = Math.min(viewport.renderer?.capabilities?.getMaxAnisotropy?.() || 1, 8);
  texture.needsUpdate = true;
  return texture;
}

function terrainInfluenceAt(x, z, trees) {
  if (trees.length === 0) return 0;
  let total = 0;

  trees.forEach((tree) => {
    const dx = x - tree.worldX;
    const dz = z - tree.worldZ;
    const distanceSq = dx * dx + dz * dz;
    const falloff = Math.exp(-distanceSq / 950);
    total += tree.terrainFactor * falloff;
  });

  return total;
}

function groundHeightAt(x, z, trees) {
  const topoValue = topoNoiseAt(x, z);
  const base = (topoValue - 0.5) * 6.8;
  return Math.max(0, 4 + base + terrainInfluenceAt(x, z, trees));
}

function normalizeTreePayload(trees) {
  const terrainValues = trees.map((tree) => Number(tree?.terrain_height || 0));
  const terrainMin = terrainValues.length ? Math.min(...terrainValues) : 0;
  const terrainMax = terrainValues.length ? Math.max(...terrainValues) : 1;
  const terrainRange = Math.max(1, terrainMax - terrainMin);

  return trees.map((tree, index) => {
    const position = tree?.position || {};
    const sensor = tree?.latest_sensor || {};
    const diagnosis = tree?.latest_diagnosis || null;
    const worldPoint = mapToWorld(position.x, position.y);
    const terrainHeight = Number(tree?.terrain_height || 0);
    const terrainFactor = ((terrainHeight - terrainMin) / terrainRange) * 5.2;

    return {
      id: String(tree?.tree_code || tree?.id || `TREE-${index + 1}`),
      dbId: Number(tree?.id || 0),
      treeCode: String(tree?.tree_code || `TREE-${index + 1}`),
      rawX: Number(position.x || 0),
      rawY: Number(position.y || 0),
      terrainHeight,
      terrainFactor,
      worldX: worldPoint.x,
      worldZ: worldPoint.z,
      worldY: 0,
      tagSerialNumber: tree?.tag_serial_number ?? null,
      statusLevel: String(tree?.status?.level || "offline"),
      statusLabel: String(tree?.status?.label || "离线"),
      statusColor: statusColor(tree?.status?.level, tree?.status?.color),
      temperature: Number.isFinite(Number(sensor.temperature)) ? Number(sensor.temperature) : null,
      humidity: Number.isFinite(Number(sensor.humidity)) ? Number(sensor.humidity) : null,
      sampledAt: sensor.sampled_at || null,
      latestDiagnosis: diagnosis,
    };
  });
}

function buildTerrain(trees) {
  const geometry = new THREE.PlaneGeometry(WORLD_SIZE.width, WORLD_SIZE.depth, 72, 72);
  geometry.rotateX(-Math.PI / 2);
  const position = geometry.attributes.position;

  for (let index = 0; index < position.count; index += 1) {
    const x = position.getX(index);
    const z = position.getZ(index);
    position.setY(index, groundHeightAt(x, z, trees));
  }

  geometry.computeVertexNormals();
  const topoTexture = buildTopoTexture();

  const material = new THREE.MeshStandardMaterial({
    color: "#dbc296",
    map: topoTexture,
    emissive: "#10120d",
    emissiveMap: topoTexture,
    emissiveIntensity: 0.01,
    roughness: 0.94,
    metalness: 0,
    flatShading: false,
  });

  const mesh = new THREE.Mesh(geometry, material);
  mesh.receiveShadow = true;
  return mesh;
}

function createTreeMesh(tree) {
  const color = new THREE.Color(tree.statusColor);
  const group = new THREE.Group();
  group.userData.treeId = tree.id;
  group.userData.baseScale = TREE_BASE_SCALE;

  const trunk = new THREE.Mesh(
    new THREE.CylinderGeometry(0.42, 0.58, 4.2, 10),
    new THREE.MeshStandardMaterial({ color: "#7c4a22", emissive: "#2a180b", emissiveIntensity: 0.08, roughness: 0.84, metalness: 0.02 })
  );
  trunk.position.y = 2.1;
  group.add(trunk);

  const canopy = new THREE.Mesh(
    new THREE.SphereGeometry(2.6, 18, 18),
    new THREE.MeshStandardMaterial({ color: color.clone().offsetHSL(-0.01, 0.02, -0.08), emissive: color.clone().multiplyScalar(0.12), emissiveIntensity: 0.18, roughness: 0.68 })
  );
  canopy.position.set(0, 5.4, 0);
  group.add(canopy);

  const canopyTop = new THREE.Mesh(
    new THREE.SphereGeometry(1.8, 18, 18),
    new THREE.MeshStandardMaterial({
      color: color.clone().offsetHSL(-0.004, 0.03, -0.01),
      emissive: color.clone().multiplyScalar(0.1),
      emissiveIntensity: 0.14,
      roughness: 0.62,
    })
  );
  canopyTop.position.set(-0.8, 6.8, 0.5);
  group.add(canopyTop);

  const marker = new THREE.Mesh(
    new THREE.TorusGeometry(3.4, 0.12, 16, 48),
    new THREE.MeshBasicMaterial({ color, transparent: true, opacity: 0.72 })
  );
  marker.rotation.x = Math.PI / 2;
  marker.position.y = 0.24;
  group.add(marker);

  if (tree.statusLevel !== "healthy" && tree.statusLevel !== "offline") {
    const beam = new THREE.Mesh(
      new THREE.CylinderGeometry(0.22, 0.65, 16, 18, 1, true),
      new THREE.MeshBasicMaterial({ color, transparent: true, opacity: 0.14, side: THREE.DoubleSide })
    );
    beam.position.y = 8.2;
    group.add(beam);
  }

  group.position.set(tree.worldX, tree.worldY, tree.worldZ);
  group.userData.tree = tree;
  group.scale.setScalar(TREE_BASE_SCALE);
  return group;
}

function rebuildOrchardScene() {
  if (!viewport.orchardGroup) return;

  while (viewport.orchardGroup.children.length > 0) {
    const child = viewport.orchardGroup.children[0];
    viewport.orchardGroup.remove(child);
    disposeSceneNode(child);
  }

  if (viewport.terrainMesh) {
    viewport.terrainMesh.geometry.dispose();
    viewport.terrainMesh.material.dispose();
    viewport.terrainMesh = null;
  }

  const terrain = buildTerrain(state.trees);
  viewport.orchardGroup.add(terrain);
  viewport.terrainMesh = terrain;

  state.trees.forEach((tree) => {
    tree.worldY = groundHeightAt(tree.worldX, tree.worldZ, state.trees);
    const mesh = createTreeMesh(tree);
    viewport.orchardGroup.add(mesh);
  });

  state.treeMeshes = viewport.orchardGroup.children.filter((item) => item.userData?.treeId);
  updateSelectedTree(state.selectedTreeId || state.trees[0]?.id || null, false);
  $("sceneEmpty").hidden = state.trees.length > 0;
}

function renderLegend() {
  const legendList = $("legendList");
  if (!legendList) return;

  if (!state.legend.length) {
    legendList.innerHTML = '<div class="empty-text">暂无图例数据。</div>';
    return;
  }

  legendList.innerHTML = state.legend.map((item) => `
    <div class="legend-item">
      <div class="legend-item__row">
        <div style="display:flex; align-items:center; gap:10px;">
          <span class="legend-dot" style="color:${escapeHtml(item.color)}; background:${escapeHtml(item.color)};"></span>
          <strong>${escapeHtml(item.label)}</strong>
        </div>
        <span>${Number(item.count || 0)}</span>
      </div>
      <small>状态级别：${escapeHtml(item.level || "unknown")}</small>
    </div>
  `).join("");
}

function renderSummary() {
  const summary = state.summary || {};
  const alertTrees = state.trees.filter((tree) => tree.statusLevel !== "healthy" && tree.statusLevel !== "offline").length;
  const healthyTrees = state.trees.filter((tree) => tree.statusLevel === "healthy").length;
  const { avgTemp, avgHumidity } = computeAverages(state.trees);
  const healthyRate = state.trees.length ? Math.round((healthyTrees / state.trees.length) * 100) : 0;

  $("metricTotalTrees").textContent = String(summary.total_trees ?? state.trees.length ?? 0);
  $("metricOnlineTrees").textContent = String(summary.online_trees ?? 0);
  $("metricAlertTrees").textContent = String(alertTrees);
  $("metricAvgHumidity").textContent = formatMetric(avgHumidity, "%");
  $("overviewUpdatedAt").textContent = summary.last_sampled_at ? `最近采样 ${formatDate(summary.last_sampled_at)}` : "暂无在线采样";

  $("overlayAvgTemp").textContent = formatMetric(avgTemp, "°C");
  $("overlayHealthyRate").textContent = `${healthyRate || 0}%`;
  $("overlayLastSample").textContent = summary.last_sampled_at ? formatDate(summary.last_sampled_at) : "--";

  const statusCounts = state.legend.map((item) => `${item.label}${item.count}`).join(" / ");
  $("sceneTicker").textContent = state.trees.length
    ? `园区在线 ${summary.online_trees || 0} 棵，重点关注 ${alertTrees} 棵，状态分布：${statusCounts}`
    : "暂无可渲染树位，等待后台写入果树坐标数据。";
}

function normalizeWeatherPayload(weather) {
  if (!weather || typeof weather !== "object") {
    return null;
  }

  const current = weather.current && typeof weather.current === "object" ? weather.current : weather;
  const currentUnits = weather.current_units && typeof weather.current_units === "object" ? weather.current_units : {};
  const latitude = Number(weather.latitude);
  const longitude = Number(weather.longitude);

  const temperature = current.temperature ?? current.temperature_2m ?? weather.temperature ?? weather.temperature_2m;
  const humidity = current.humidity ?? current.relative_humidity_2m ?? weather.humidity ?? weather.relative_humidity_2m;
  const windSpeed = current.wind_speed ?? current.wind_speed_10m ?? weather.wind_speed ?? weather.wind_speed_10m;
  const temperatureUnit = current.temperature_unit || weather.temperature_unit || currentUnits.temperature_2m || "°C";
  const humidityUnit = current.humidity_unit || weather.humidity_unit || currentUnits.relative_humidity_2m || "%";
  const windSpeedUnit = current.wind_speed_unit || weather.wind_speed_unit || currentUnits.wind_speed_10m || "km/h";
  const weatherCode = current.weather_code ?? weather.weather_code;
  const description = current.weather_text
    || weather.weather_text
    || current.description
    || weather.description
    || current.weather
    || weather.weather
    || current.text
    || weather.text
    || (weatherCode != null ? `天气代码 ${weatherCode}` : "天气信息已连接");
  const location = weather.city || weather.location || weather.name || "园区属地";
  const locationMeta = Number.isFinite(latitude) && Number.isFinite(longitude)
    ? `坐标 ${latitude.toFixed(2)}, ${longitude.toFixed(2)}`
    : (weather.timezone || "");

  return {
    location,
    locationMeta,
    description,
    temperature,
    temperatureUnit,
    humidity,
    humidityUnit,
    windSpeed,
    windSpeedUnit,
    observedAt: current.time || weather.time || null,
  };
}

function renderWeather() {
  const weather = normalizeWeatherPayload(state.weather);
  if (!weather) {
    $("weatherLocation").textContent = "天气数据不可用";
    $("weatherMeta").textContent = "接口未返回可展示的天气信息，沙盘仍可正常浏览。";
    $("weatherPanel").querySelector(".weather-temp").textContent = "--";
    return;
  }

  const temp = Number.isFinite(Number(weather.temperature))
    ? `${Number(weather.temperature).toFixed(1)}${weather.temperatureUnit || "°C"}`
    : "--";
  const extra = [
    weather.description,
    weather.locationMeta,
    Number.isFinite(Number(weather.humidity)) ? `湿度 ${Number(weather.humidity).toFixed(0)}${weather.humidityUnit || "%"}` : "",
    Number.isFinite(Number(weather.windSpeed)) ? `风速 ${Number(weather.windSpeed).toFixed(1)} ${weather.windSpeedUnit || "km/h"}` : "",
    weather.observedAt ? `观测 ${weather.observedAt}` : "",
  ]
    .filter(Boolean)
    .join(" · ");

  $("weatherPanel").querySelector(".weather-temp").textContent = temp;
  $("weatherLocation").textContent = weather.location;
  $("weatherMeta").textContent = extra;
}

function renderAlerts() {
  const alertList = $("alertList");
  if (!alertList) return;

  const alertTrees = state.trees
    .filter((tree) => tree.statusLevel !== "healthy" && tree.statusLevel !== "offline")
    .sort((left, right) => severityRank(right.statusLevel) - severityRank(left.statusLevel));

  if (!alertTrees.length) {
    alertList.innerHTML = '<div class="empty-text">当前没有需要特别关注的树位。</div>';
    return;
  }

  alertList.innerHTML = alertTrees.map((tree) => `
    <button class="alert-item ${tree.id === state.selectedTreeId ? "is-active" : ""}" type="button" data-tree-id="${escapeHtml(tree.id)}">
      <div class="alert-item__row">
        <div style="display:flex; align-items:center; gap:10px;">
          <span class="legend-dot" style="color:${escapeHtml(tree.statusColor)}; background:${escapeHtml(tree.statusColor)};"></span>
          <strong>${escapeHtml(tree.treeCode)}</strong>
        </div>
        <span>${escapeHtml(tree.statusLabel)}</span>
      </div>
      <div class="alert-item__meta">温度 ${formatMetric(tree.temperature, "°C")} · 湿度 ${formatMetric(tree.humidity, "%")} · 最近采样 ${formatDate(tree.sampledAt)}</div>
    </button>
  `).join("");

  alertList.querySelectorAll("[data-tree-id]").forEach((button) => {
    button.addEventListener("click", () => updateSelectedTree(button.dataset.treeId || null, true));
  });
}

function severityRank(level) {
  switch (level) {
    case "critical":
      return 4;
    case "warning":
      return 3;
    case "attention":
      return 2;
    case "healthy":
      return 1;
    default:
      return 0;
  }
}

function renderTreeDetail(tree) {
  const panel = $("treeDetailPanel");
  if (!panel) return;

  if (!tree) {
    panel.innerHTML = '<div class="empty-text">点击三维沙盘中的树位查看详细信息。</div>';
    return;
  }

  const diagnosisText = tree.latestDiagnosis?.disease_name || tree.latestDiagnosis?.predicted_class || "未发现异常";
  const confidence = Number.isFinite(Number(tree.latestDiagnosis?.confidence))
    ? `${Math.round(Number(tree.latestDiagnosis.confidence) * 100)}%`
    : "--";

  panel.innerHTML = `
    <div class="detail-card">
      <div class="detail-main">
        <div style="display:flex; align-items:center; gap:10px;">
          <span class="detail-dot" style="color:${escapeHtml(tree.statusColor)}; background:${escapeHtml(tree.statusColor)};"></span>
          <div>
            <div class="eyebrow" style="margin:0 0 4px;">${escapeHtml(tree.statusLabel)}</div>
            <strong>${escapeHtml(tree.treeCode)}</strong>
          </div>
        </div>
        <span>ID ${escapeHtml(tree.dbId)}</span>
      </div>
      <dl class="detail-grid">
        <dt>定位坐标</dt>
        <dd>${formatMetric(tree.rawX)} , ${formatMetric(tree.rawY)}</dd>
        <dt>传感温度</dt>
        <dd>${formatMetric(tree.temperature, "°C")}</dd>
        <dt>传感湿度</dt>
        <dd>${formatMetric(tree.humidity, "%")}</dd>
        <dt>地形高程</dt>
        <dd>${formatMetric(tree.terrainHeight, " m")}</dd>
        <dt>标签编号</dt>
        <dd>${tree.tagSerialNumber ?? "--"}</dd>
        <dt>最近采样</dt>
        <dd>${formatDate(tree.sampledAt)}</dd>
      </dl>
      <div class="detail-diagnosis">
        <strong style="font-size:1rem; margin:0 0 8px; display:block;">诊断结果</strong>
        <div>${escapeHtml(diagnosisText)}</div>
        <div style="margin-top:8px; color:var(--text-muted);">置信度 ${confidence} · 诊断时间 ${formatDate(tree.latestDiagnosis?.timestamp)}</div>
      </div>
    </div>
  `;
}

function updateSelectedTree(treeId, focusCamera) {
  state.selectedTreeId = treeId;
  const selectedTree = state.trees.find((tree) => tree.id === treeId) || null;

  state.treeMeshes.forEach((mesh) => {
    const isSelected = mesh.userData?.treeId === treeId;
    const baseScale = Number(mesh.userData?.baseScale) || 1;
    mesh.scale.setScalar(baseScale * (isSelected ? 1.12 : 1));
    mesh.traverse((child) => {
      if (child.material && "opacity" in child.material && child.geometry?.type === "TorusGeometry") {
        child.material.opacity = isSelected ? 1 : 0.72;
      }
    });
  });

  renderTreeDetail(selectedTree);
  renderAlerts();

  if (focusCamera && selectedTree && viewport.controls) {
    viewport.controls.target.lerp(new THREE.Vector3(selectedTree.worldX, selectedTree.worldY + 4, selectedTree.worldZ), 0.85);
  }
}

function setLoading(loading, text = "正在构建三维沙盘...") {
  const loadingEl = $("sceneLoading");
  loadingEl.textContent = text;
  loadingEl.hidden = !loading;
}

function updatePointerFromEvent(event) {
  const rect = viewport.renderer.domElement.getBoundingClientRect();
  viewport.pointer.x = ((event.clientX - rect.left) / rect.width) * 2 - 1;
  viewport.pointer.y = -((event.clientY - rect.top) / rect.height) * 2 + 1;
}

function intersectTreeMeshes(event) {
  if (!viewport.camera || !state.treeMeshes.length) return null;

  updatePointerFromEvent(event);
  viewport.raycaster.setFromCamera(viewport.pointer, viewport.camera);
  const hits = viewport.raycaster.intersectObjects(state.treeMeshes, true);
  const hit = hits.find((item) => item.object.parent?.userData?.treeId || item.object.userData?.treeId);
  if (!hit) return null;
  return hit.object.parent?.userData?.tree || hit.object.userData?.tree || null;
}

function onPointerMove(event) {
  const tree = intersectTreeMeshes(event);
  document.body.style.cursor = tree ? "pointer" : "default";
}

function onCanvasClick(event) {
  const tree = intersectTreeMeshes(event);
  if (!tree) return;
  updateSelectedTree(tree.id, true);
}

function animate() {
  requestAnimationFrame(animate);
  if (!state.sceneReady) return;

  const elapsed = viewport.clock.getElapsedTime();
  viewport.scene.rotation.y = Math.sin(elapsed * 0.08) * 0.012;
  state.treeMeshes.forEach((mesh, index) => {
    const pulse = 1 + Math.sin(elapsed * 1.6 + index * 0.35) * 0.012;
    if (mesh.userData?.treeId !== state.selectedTreeId) {
      const baseScale = Number(mesh.userData?.baseScale) || 1;
      mesh.scale.setScalar(baseScale * pulse);
    }
  });

  viewport.controls.autoRotate = state.autoRotate;
  viewport.controls.update();
  viewport.renderer.render(viewport.scene, viewport.camera);
}

async function validateSession() {
  const response = await postJson("/user/validate");
  if (!response.ok || !response.data?.valid) {
    return null;
  }

  return response.data;
}

async function loadOverview() {
  if (!state.adminUsername) {
    throw new Error("未获取到用户身份");
  }

  const response = await postJson("/user/orchard/overview");

  if (!response.ok) {
    throw new Error(response.data?.error || response.data?.message || "获取园区监测数据失败");
  }

  const payload = unwrapApiPayload(response.data);
  state.bounds = {
    ...DEFAULT_ORCHARD_BOUNDS,
    ...(payload.coordinate_range || {}),
  };
  state.legend = Array.isArray(payload.legend) ? payload.legend : [];
  state.summary = payload.summary || null;
  state.weather = payload.weather || null;
  state.trees = normalizeTreePayload(Array.isArray(payload.trees) ? payload.trees : []);
}

function renderUnauthorized(message) {
  setLoading(false);
  $("sceneEmpty").hidden = false;
  $("sceneEmpty").innerHTML = `
    <h3>无法进入 3D 大屏</h3>
    <p>${escapeHtml(message)}</p>
    <p><a class="app-nav__link" href="/" style="display:inline-flex; margin-top:10px;">前往登录</a></p>
  `;
}

async function refreshDashboard() {
  setLoading(true, "正在刷新园区监测数据...");

  try {
    await loadOverview();
    rebuildOrchardScene();
    renderLegend();
    renderSummary();
    renderWeather();
    renderAlerts();
    setLoading(false);
  } catch (error) {
    setLoading(false);
    $("sceneEmpty").hidden = false;
    $("sceneEmpty").innerHTML = `<h3>数据加载失败</h3><p>${escapeHtml(error.message || "未知错误")}</p>`;
  }
}

function bindActions() {
  $("btnRefreshScene").addEventListener("click", refreshDashboard);
  $("btnLogout").addEventListener("click", logout);
  $("btnToggleOrbit").addEventListener("click", () => {
    state.autoRotate = !state.autoRotate;
    $("btnToggleOrbit").textContent = state.autoRotate ? "暂停巡航" : "恢复巡航";
  });
}

async function init() {
  try {
    createScene();
    bindActions();
    animate();

    const session = await validateSession();
    if (!session?.username) {
      $("btnLogout").hidden = true;
      $("orchardLoginNav").hidden = false;
      renderUnauthorized("请先登录后再访问园区3D沙盘。");
      return;
    }

    $("orchardAdminNav").hidden = !session.is_admin;
    state.adminUsername = session.username;
    await refreshDashboard();
  } catch (error) {
    renderUnauthorized(error?.message || "当前环境无法初始化 Three.js / WebGL 渲染。请在支持硬件加速的浏览器中打开此页面。");
  }
}

init();
