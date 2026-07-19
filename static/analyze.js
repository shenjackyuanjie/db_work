(() => {
  const SESSION_COOKIE = "session_token";

  const els = {
    uploadZone: document.getElementById("uploadZone"),
    imageInput: document.getElementById("imageInput"),
    previewImg: document.getElementById("previewImg"),
    analyzeBtn: document.getElementById("analyzeBtn"),
    errorBox: document.getElementById("errorBox"),
    errorText: document.getElementById("errorText"),
    loginBadge: document.getElementById("loginBadge"),
    loginHint: document.getElementById("loginHint"),
    logoutBtn: document.getElementById("logout-btn"),
    loginNavLink: document.getElementById("loginNavLink"),
    adminNavLink: document.getElementById("adminNavLink"),
    progressWrapper: document.getElementById("progressWrapper"),
    progressBar: document.getElementById("progressBar"),
    progressText: document.getElementById("progressText"),
    result: {
      isHealthy: document.getElementById("isHealthy"),
      diseaseName: document.getElementById("diseaseName"),
      severity: document.getElementById("severity"),
      confidence: document.getElementById("confidence"),
      treatment: document.getElementById("treatment"),
      prevention: document.getElementById("prevention"),
      qualityWarning: document.getElementById("qualityWarning"),
      duration: document.getElementById("duration"),
      tps: document.getElementById("tps"),
      rawResponse: document.getElementById("rawResponse")
    }
  };

  function getCookie(name) {
    const prefix = `${name}=`;
    const part = document.cookie.split(";").map((item) => item.trim()).find((item) => item.startsWith(prefix));
    return part ? decodeURIComponent(part.slice(prefix.length)) : "";
  }

  function getToken() {
    return getCookie(SESSION_COOKIE);
  }

  function updateLoginUI() {
    const token = getToken();
    if (token) {
      els.loginBadge.textContent = "已登录";
      els.loginBadge.className = "badge success";
      els.loginNavLink.hidden = true;
      els.loginHint.classList.add("hidden");
      els.logoutBtn.classList.remove("hidden");
      els.analyzeBtn.disabled = !els.imageInput.files.length;
      return;
    }

    els.loginBadge.textContent = "未登录";
    els.loginBadge.className = "badge";
    els.loginNavLink.hidden = false;
    els.adminNavLink.hidden = true;
    els.loginHint.classList.remove("hidden");
    els.logoutBtn.classList.add("hidden");
    els.analyzeBtn.disabled = true;
  }

  async function refreshSessionNavigation() {
    const token = getToken();
    if (!token) return;

    try {
      const response = await fetch("/user/validate", {
        method: "POST",
        credentials: "same-origin",
        headers: { "X-Session-Token": token },
      });
      const data = await response.json().catch(() => ({}));
      if (!response.ok || !data.valid) {
        els.loginBadge.textContent = "会话失效";
        els.loginBadge.className = "badge warning";
        els.loginNavLink.hidden = false;
        els.logoutBtn.classList.add("hidden");
        els.adminNavLink.hidden = true;
        els.analyzeBtn.disabled = true;
        return;
      }

      els.adminNavLink.hidden = !data.is_admin;
    } catch {
      els.adminNavLink.hidden = true;
    }
  }

  function setError(msg) {
    if (!msg) {
      els.errorBox.classList.add("hidden");
      return;
    }

    els.errorBox.classList.remove("hidden");
    els.errorText.textContent = msg;
  }

  function setBadge(el, text, type = "default") {
    el.innerHTML = `<span class="badge ${type}">${text}</span>`;
  }

  function resetResult() {
    Object.values(els.result).forEach((el) => {
      if (el.tagName !== "PRE") {
        el.textContent = "-";
      }
    });
    els.result.rawResponse.textContent = "// 等待分析...";
  }

  function updatePreview(file) {
    if (!file) {
      els.previewImg.src = "";
      els.uploadZone.classList.remove("has-image");
      return;
    }

    const url = URL.createObjectURL(file);
    els.previewImg.onload = () => URL.revokeObjectURL(url);
    els.previewImg.src = url;
    els.uploadZone.classList.add("has-image");
  }

  function handleFileSelect() {
    setError("");
    resetResult();
    updatePreview(els.imageInput.files[0]);
    updateLoginUI();
  }

  function fileToDataUrl(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onerror = () => reject(new Error("读取图片失败"));
      reader.onload = () => resolve(reader.result);
      reader.readAsDataURL(file);
    });
  }

  function setAnalyzeLoading(isLoading) {
    els.analyzeBtn.disabled = isLoading;
    els.analyzeBtn.classList.toggle("loading", isLoading);
    if (!isLoading) {
      updateLoginUI();
    }
  }

  function startProgressIndicator() {
    els.progressWrapper.classList.add("show");
    els.progressBar.style.width = "0%";
    els.progressText.textContent = "正在识别，请稍候... (预计 60 秒)";

    let progress = 0;
    const totalTime = 60000;
    const intervalTime = 100;
    const step = (intervalTime / totalTime) * 100;

    return setInterval(() => {
      if (progress < 95) {
        progress += step;
        els.progressBar.style.width = `${progress}%`;
        const remaining = Math.max(0, Math.ceil((totalTime * (1 - progress / 100)) / 1000));
        els.progressText.textContent = `正在识别，请稍候... (预计剩余 ${remaining} 秒)`;
      }
    }, intervalTime);
  }

  function stopProgressIndicator(timer, completed) {
    clearInterval(timer);
    if (!completed) {
      els.progressWrapper.classList.remove("show");
      return;
    }

    els.progressBar.style.width = "100%";
    els.progressText.textContent = "识别完成！";
    setTimeout(() => {
      els.progressWrapper.classList.remove("show");
    }, 1500);
  }

  async function handleAnalyze() {
    setError("");
    resetResult();

    const token = getToken();
    if (!token) {
      setError("未登录，请先登录");
      return;
    }

    const file = els.imageInput.files[0];
    if (!file) {
      setError("请先选择图片");
      return;
    }

    setAnalyzeLoading(true);
    const progressTimer = startProgressIndicator();

    try {
      const imageDataUrl = await fileToDataUrl(file);
      const resp = await fetch("/api/citrus-disease-v2", {
        method: "POST",
        credentials: "same-origin",
        headers: {
          "Content-Type": "application/json",
          "X-Session-Token": token,
        },
        body: JSON.stringify({ image: imageDataUrl }),
      });

      const data = await resp.json();
      els.result.rawResponse.textContent = JSON.stringify(data, null, 2);

      if (!resp.ok || data.code !== 200) {
        throw new Error(data.message || data.error || "识别失败");
      }

      const result = data.data || {};
      const predictedClass = result.predicted_class || "";
      const isHealthy = result.is_healthy === true || predictedClass === "健康果树";
      const isFruitTree = predictedClass !== "非果树";

      if (!isFruitTree) {
        setBadge(els.result.isHealthy, "非果树", "warning");
      } else {
        setBadge(els.result.isHealthy, isHealthy ? "健康" : "患病", isHealthy ? "success" : "danger");
      }

      els.result.diseaseName.textContent = result.disease_name || predictedClass || "无";
      els.result.severity.textContent = result.severity || (isFruitTree ? "-" : "无需分级");

      if (typeof result.confidence === "number") {
        els.result.confidence.textContent = `${result.confidence.toFixed(2)}%`;
      }

      els.result.treatment.textContent = result.treatment_suggestion || "暂无建议";
      els.result.prevention.textContent = result.preventive_measures || "暂无建议";
      els.result.qualityWarning.textContent = result.image_quality_warning || "无";
      els.result.duration.textContent = "-";
      els.result.tps.textContent = result.stage || "-";

      stopProgressIndicator(progressTimer, true);
    } catch (err) {
      stopProgressIndicator(progressTimer, false);
      setError(err.message || "请求服务器失败，请检查网络或后端配置");
    } finally {
      setAnalyzeLoading(false);
    }
  }

  function logout() {
    document.cookie = `${SESSION_COOKIE}=; path=/; max-age=0; samesite=lax`;
    setTimeout(() => location.reload(), 80);
  }

  function bindUploadEvents() {
    els.uploadZone.addEventListener("click", () => els.imageInput.click());
    els.uploadZone.addEventListener("dragover", (event) => {
      event.preventDefault();
      els.uploadZone.classList.add("dragover");
    });
    els.uploadZone.addEventListener("dragleave", () => {
      els.uploadZone.classList.remove("dragover");
    });
    els.uploadZone.addEventListener("drop", (event) => {
      event.preventDefault();
      els.uploadZone.classList.remove("dragover");
      if (event.dataTransfer.files.length) {
        els.imageInput.files = event.dataTransfer.files;
        handleFileSelect();
      }
    });
    els.imageInput.addEventListener("change", handleFileSelect);
  }

  function bindActions() {
    els.analyzeBtn.addEventListener("click", handleAnalyze);
    els.logoutBtn.addEventListener("click", logout);
  }

  async function initPage() {
    bindUploadEvents();
    bindActions();
    updateLoginUI();
    await refreshSessionNavigation();
  }

  void initPage();
})();
