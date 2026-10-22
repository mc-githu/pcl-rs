/* PCL-RS 前端。
 * 布局参考 PCL：左侧导航 + 页面内分栏；下载页统一承载「游戏本体 / Mod / …」，
 * Mod 搜索必须先选定实例（用它的 MC 版本与加载器过滤）。
 * 在浏览器中直接打开时会退化为演示数据，便于调整界面。 */

const tauri = window.__TAURI__;
const invoke = tauri?.core?.invoke ?? demoInvoke;
const listen = tauri?.event?.listen ?? (async () => () => {});
const isDesktop = Boolean(tauri);

const $ = (id) => document.getElementById(id);
const status = (text) => { $("statusText").textContent = text; };
const logLine = (line) => {
  const view = $("logView");
  view.textContent += line + "\n";
  view.scrollTop = view.scrollHeight;
  if (view.textContent.length > 200_000) view.textContent = view.textContent.slice(-150_000);
};
const mb = (bytes) => (bytes / 1048576).toFixed(1);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) =>
  ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

const state = {
  settings: null,
  accounts: [],
  instances: [],
  selected: null, // { id, gameVersion, loader }
  versions: [],
  versionsLoaded: false,
};

/* ------------------------------------------------------------------ 导航 */
document.querySelectorAll(".nav-item").forEach((btn) => {
  btn.addEventListener("click", () => {
    document.querySelectorAll(".nav-item").forEach((b) => b.classList.toggle("active", b === btn));
    const page = btn.dataset.page;
    document.querySelectorAll(".page").forEach((p) => p.classList.toggle("active", p.id === `page-${page}`));
    if (page === "download") {
      loadVersions();
      renderModScope();
    }
    if (page === "instances") loadInstances();
    if (page === "online") refreshTerracotta();
    if (page === "settings") fillSettings();
  });
});

document.querySelectorAll("#downloadSubnav .subnav-item").forEach((btn) => {
  btn.addEventListener("click", () => {
    document.querySelectorAll("#downloadSubnav .subnav-item").forEach((b) => b.classList.toggle("active", b === btn));
    const sub = btn.dataset.sub;
    const isPlaceholder = !["game", "mod"].includes(sub);
    document.querySelectorAll(".subpage").forEach((p) => p.classList.remove("active"));
    if (isPlaceholder) {
      $("placeholderTitle").textContent = btn.textContent.trim();
      $("sub-placeholder").classList.add("active");
    } else {
      $(`sub-${sub}`).classList.add("active");
    }
    if (sub === "mod") renderModScope();
    if (sub === "game") loadVersions();
  });
});

/* ------------------------------------------------------------------ 设置 */
function sourceLabel(source) {
  return { cn: "BMCLAPI → 官方", global: "官方 → BMCLAPI", official: "仅官方" }[source] ?? source;
}

async function loadSettings() {
  state.settings = await invoke("get_settings");
  $("statusSource").textContent = `线路: ${sourceLabel(state.settings.source)}`;
}

function fillSettings() {
  const s = state.settings;
  if (!s) return;
  $("setGameDir").value = s.gameDir ?? "";
  $("setMemory").value = s.memoryMb ?? 4096;
  $("setWidth").value = s.width ?? 854;
  $("setHeight").value = s.height ?? 480;
  $("setFullscreen").checked = Boolean(s.fullscreen);
  $("setSource").value = s.source ?? "cn";
  $("setCfKey").value = s.curseforgeKey ?? "";
  $("setJavaPath").value = s.javaPath ?? "";
}

$("saveSettings").addEventListener("click", async () => {
  state.settings = {
    ...state.settings,
    gameDir: $("setGameDir").value.trim(),
    memoryMb: Number($("setMemory").value) || 4096,
    width: Number($("setWidth").value) || 854,
    height: Number($("setHeight").value) || 480,
    fullscreen: $("setFullscreen").checked,
    source: $("setSource").value,
    curseforgeKey: $("setCfKey").value.trim(),
    javaPath: $("setJavaPath").value.trim() || null,
  };
  state.settings = await invoke("save_settings", { settings: state.settings });
  fillSettings();
  $("statusSource").textContent = `线路: ${sourceLabel(state.settings.source)}`;
  status("设置已保存");

  // 目录可能变了：重新读取实例并重置选择
  state.selected = null;
  state.versionsLoaded = false;
  await loadInstances();
});

$("browseGameDir").addEventListener("click", async () => {
  const picked = await invoke("pick_game_dir");
  if (picked) {
    $("setGameDir").value = picked;
    status(`已选择目录：${picked}（记得点「保存设置」）`);
  }
});

$("defaultGameDir").addEventListener("click", async () => {
  const dir = await invoke("default_game_dir");
  if (dir) {
    $("setGameDir").value = dir;
    status(`已填入默认目录：${dir}（记得点「保存设置」）`);
  }
});

$("scanJava").addEventListener("click", async () => {
  const list = await invoke("detect_java");
  $("javaList").innerHTML = list.length
    ? list.map((j) => `<div class="java-item">Java ${j.major} · ${esc(j.vendor ?? "未知厂商")} · ${esc(j.arch)} · ${esc(j.display)}</div>`).join("")
    : `<div class="java-item">未找到 Java，请安装 JDK 17 或 21</div>`;
});

/* ------------------------------------------------------------------ 实例 */
async function loadInstances() {
  state.instances = await invoke("list_installed");

  // 主页与下载页共用的选择器
  const options = state.instances
    .map((v) => `<option value="${esc(v.id)}">${esc(v.id)}${v.hasJar ? "" : "（缺少 jar）"}</option>`)
    .join("");
  $("launchVersion").innerHTML = options || `<option value="">（尚未安装任何版本）</option>`;
  $("instanceSelect").innerHTML =
    `<option value="">未选择</option>` + options;

  const keep = state.selected?.id;
  if (keep && state.instances.some((v) => v.id === keep)) {
    $("instanceSelect").value = keep;
    $("launchVersion").value = keep;
  } else if (state.instances.length) {
    $("instanceSelect").value = state.instances[0].id;
    $("launchVersion").value = state.instances[0].id;
    await selectInstance(state.instances[0].id);
  } else {
    state.selected = null;
    renderModScope();
  }

  $("instanceList").innerHTML = state.instances.length
    ? state.instances
        .map(
          (v, i) => `<div class="version-item enter" style="animation-delay:${i * 22}ms">
            <span class="id">${esc(v.id)}</span>
            <span class="meta">${v.hasJar ? "客户端完整" : "缺少客户端 jar"}${v.javaMajor ? ` · 需要 Java ${v.javaMajor}` : ""}</span>
            <button class="btn ghost" data-verify="${esc(v.id)}">校验</button>
            <button class="btn" data-install="${esc(v.id)}">补全</button>
            <button class="btn ghost" data-pick="${esc(v.id)}">选为下载目标</button>
          </div>`
        )
        .join("")
    : `<div class="empty"><div class="empty-icon">📦</div><p>还没有安装任何版本</p><p class="muted">去「下载 → 游戏本体」安装一个吧</p></div>`;

  bindInstallButtons($("instanceList"));
  $("instanceList").querySelectorAll("[data-verify]").forEach((btn) =>
    btn.addEventListener("click", () => verifyVersion(btn.dataset.verify))
  );
  $("instanceList").querySelectorAll("[data-pick]").forEach((btn) =>
    btn.addEventListener("click", async () => {
      $("instanceSelect").value = btn.dataset.pick;
      await selectInstance(btn.dataset.pick);
      status(`下载目标已切换为 ${btn.dataset.pick}`);
    })
  );
}

async function selectInstance(id) {
  if (!id) {
    state.selected = null;
    renderModScope();
    return;
  }
  try {
    const info = await invoke("version_info", { id });
    state.selected = { id, gameVersion: info.gameVersion, loader: info.loader };
  } catch (e) {
    state.selected = { id, gameVersion: id, loader: "vanilla" };
  }
  renderModScope();
}

$("instanceSelect").addEventListener("change", async (e) => {
  await selectInstance(e.target.value);
  if (e.target.value) $("launchVersion").value = e.target.value;
});

$("refreshInstalled").addEventListener("click", loadInstances);
$("reloadInstances").addEventListener("click", loadInstances);

/* ------------------------------------------------------------------ 版本列表 */
async function loadVersions(force = false) {
  if (state.versionsLoaded && !force) return;
  status("正在获取版本清单…");
  const versions = await invoke("list_versions", { includeSnapshots: $("showSnapshots").checked });
  state.versions = versions;
  state.versionsLoaded = true;
  renderVersions();
  status(`共 ${versions.length} 个版本`);
}

function renderVersions() {
  const filter = $("versionSearch").value.trim().toLowerCase();
  const rows = state.versions.filter((v) => !filter || v.id.toLowerCase().includes(filter));
  if (!rows.length) {
    $("versionList").innerHTML = `<div class="empty"><p>没有匹配的版本</p></div>`;
    return;
  }
  $("versionList").innerHTML = rows
    .map(
      (v, i) => `<div class="version-item enter" style="animation-delay:${Math.min(i, 20) * 18}ms">
        <span class="id">${esc(v.id)}</span>
        <span class="badge ${v.kind === "release" ? "ok" : "warn"}">${v.kind === "release" ? "正式版" : "快照"}</span>
        <span class="meta">${esc((v.releaseTime ?? "").slice(0, 10))}</span>
        ${v.installed ? `<span class="badge ok">已安装</span>` : ""}
        <button class="btn ${v.installed ? "ghost" : "primary"}" data-install="${esc(v.id)}">${v.installed ? "补全" : "安装"}</button>
      </div>`
    )
    .join("");
  bindInstallButtons($("versionList"));
}

function bindInstallButtons(root) {
  root.querySelectorAll("[data-install]").forEach((btn) =>
    btn.addEventListener("click", () => installVersion(btn.dataset.install))
  );
}

$("versionSearch").addEventListener("input", renderVersions);
$("showSnapshots").addEventListener("change", () => {
  state.versionsLoaded = false;
  loadVersions(true);
});
$("reloadVersions").addEventListener("click", () => {
  state.versionsLoaded = false;
  loadVersions(true);
});

/* ------------------------------------------------------------------ 下载进度 */
listen("install-progress", (event) => {
  const p = event.payload;
  $("dlProgress").classList.remove("hidden");
  $("dlBar").style.width = `${(p.fraction * 100).toFixed(1)}%`;
  $("dlText").textContent = `${(p.fraction * 100).toFixed(1)}% · ${p.filesDone}/${p.filesTotal} 文件 · ${mb(p.bytesDone)}/${mb(p.bytesTotal)} MB ${p.current ? "· " + p.current : ""}`;

  $("launchProgress").classList.remove("hidden");
  $("launchBar").style.width = $("dlBar").style.width;
  $("launchText").textContent = $("dlText").textContent;
});

async function installVersion(id) {
  if (!id) return;
  status(`正在安装 ${id}…`);
  try {
    await invoke("install_version", { id, verify: false });
    status(`${id} 安装完成`);
    logLine(`[PCL-RS] ${id} 安装完成`);
    await loadInstances();
    state.versions = state.versions.map((v) => (v.id === id ? { ...v, installed: true } : v));
    renderVersions();
  } catch (e) {
    status(`安装失败: ${e}`);
    logLine(`[错误] ${e}`);
    $("dlProgress").classList.add("hidden");
  }
}

async function verifyVersion(id) {
  status(`正在校验 ${id}…`);
  const bad = await invoke("verify_version", { id });
  if (!bad.length) {
    status(`${id} 校验通过`);
  } else {
    status(`${id} 有 ${bad.length} 个文件损坏/缺失`);
    bad.slice(0, 20).forEach((p) => logLine(`[缺失] ${p}`));
  }
}

/* ------------------------------------------------------------------ Mod（依赖所选实例） */
function renderModScope() {
  const hint = $("modScopeHint");
  const filters = $("modFilters");

  if (!state.selected) {
    hint.className = "scoped-hint warn";
    hint.textContent = "请先在右上角「目标实例」里选择一个已安装的版本——Mod 需要按它的游戏版本与加载器过滤。";
    filters.innerHTML = "";
    $("modSearch").disabled = true;
    $("modList").innerHTML = `<div class="empty"><div class="empty-icon">🧩</div><p>尚未选择实例</p><p class="muted">选择后才能搜索与该版本兼容的 Mod</p></div>`;
    return;
  }

  const { id, gameVersion, loader } = state.selected;
  hint.className = "scoped-hint";
  hint.textContent = `当前目标：${id} —— Mod 将下载到该实例的 mods 目录。`;
  filters.innerHTML = `
    <span class="chip">Minecraft ${esc(gameVersion)}</span>
    <span class="chip">${esc(loader === "vanilla" ? "原版（无加载器）" : loader)}</span>
    ${loader === "vanilla" ? `<span class="chip grey">原版实例通常装不了 Mod，建议先装 Fabric/Forge 实例</span>` : ""}
  `;
  $("modSearch").disabled = false;
  if (!$("modList").dataset.filled) {
    $("modList").innerHTML = `<div class="empty"><p class="muted">输入关键词搜索 Mod</p></div>`;
  }
}

$("modSearch").addEventListener("click", async () => {
  if (!state.selected) return;
  const source = $("modSource").value;
  const query = $("modQuery").value.trim();
  status("正在搜索 Mod…");
  $("modList").innerHTML = `<div class="empty"><p class="muted">搜索中…</p></div>`;
  try {
    const results = await invoke("search_mods", {
      source,
      query,
      gameVersion: state.selected.gameVersion,
      loader: state.selected.loader === "vanilla" ? null : state.selected.loader,
      limit: 20,
    });
    $("modList").dataset.filled = "1";
    $("modList").innerHTML = results.length
      ? results
          .map(
            (m, i) => `<div class="version-item enter" style="animation-delay:${Math.min(i, 20) * 18}ms">
              ${m.iconUrl ? `<img class="icon" src="${esc(m.iconUrl)}" alt="" loading="lazy" />` : `<span class="icon"></span>`}
              <span class="id">${esc(m.name)}</span>
              <span class="meta">${esc(m.summary ?? "")}</span>
              <span class="badge grey">${esc(m.source)}</span>
              <button class="btn primary" data-mod="${esc(m.id)}">下载</button>
            </div>`
          )
          .join("")
      : `<div class="empty"><p>没有找到兼容 ${esc(state.selected.gameVersion)} 的 Mod</p></div>`;

    $("modList").querySelectorAll("[data-mod]").forEach((btn) =>
      btn.addEventListener("click", () => downloadMod(source, btn.dataset.mod))
    );
    status(`找到 ${results.length} 个 Mod`);
  } catch (e) {
    $("modList").innerHTML = `<div class="empty"><p>搜索失败：${esc(e)}</p></div>`;
    status(`搜索失败: ${e}`);
  }
});

async function downloadMod(source, project) {
  if (!state.selected) return;
  status("正在解析并下载 Mod…");
  try {
    const dest = await invoke("download_mod", {
      source,
      project,
      gameVersion: state.selected.gameVersion,
      loader: state.selected.loader === "vanilla" ? null : state.selected.loader,
    });
    status(`已下载到 ${dest}`);
    logLine(`[Mod] ${dest}`);
  } catch (e) {
    status(`下载失败: ${e}`);
    logLine(`[错误] ${e}`);
  }
}

/* ------------------------------------------------------------------ 启动 */
$("launchBtn").addEventListener("click", async () => {
  const id = $("launchVersion").value;
  if (!id) {
    status("请先安装一个版本");
    return;
  }
  const username = $("offlineName").value.trim() || null;
  $("launchBtn").disabled = true;
  status(`正在准备启动 ${id}…`);
  $("launchProgress").classList.remove("hidden");
  try {
    const pid = await invoke("launch_game", { id, username, memoryMb: null, fullscreen: false, width: null, height: null });
    status(`游戏已启动 (PID ${pid})`);
    logLine(`[PCL-RS] 启动 ${id}，PID ${pid}`);
  } catch (e) {
    status(`启动失败: ${e}`);
    logLine(`[错误] ${e}`);
  } finally {
    $("launchBtn").disabled = false;
  }
});

listen("game-log", (event) => logLine(event.payload));
listen("game-exit", (event) => {
  status(`游戏已退出，退出码 ${event.payload}`);
  logLine(`[PCL-RS] 游戏退出，退出码 ${event.payload}`);
});
listen("game-started", (event) => logLine(`[PCL-RS] 进程已创建，PID ${event.payload}`));

/* ------------------------------------------------------------------ 账户 */
async function loadAccounts() {
  state.accounts = await invoke("list_accounts");
  const list = $("accountList");
  list.innerHTML = state.accounts.length
    ? state.accounts
        .map(
          (a) => `<div class="account-item ${a.selected ? "selected" : ""}" data-uuid="${esc(a.uuid)}">
            <span>${esc(a.username)}</span>
            <span class="kind">${a.kind === "microsoft" ? "微软" : "离线"}${a.expired ? " · 已过期" : ""}</span>
          </div>`
        )
        .join("")
    : `<div class="account-item"><span>暂无账户</span><span class="kind">添加离线或微软登录</span></div>`;

  list.querySelectorAll("[data-uuid]").forEach((el) =>
    el.addEventListener("click", async () => {
      await invoke("select_account", { uuid: el.dataset.uuid });
      await loadAccounts();
    })
  );

  const selected = state.accounts.find((a) => a.selected) ?? state.accounts[0] ?? null;
  $("accountChip").textContent = selected ? selected.username : "未登录";
}

$("addOffline").addEventListener("click", async () => {
  const name = $("newOfflineName").value.trim();
  if (!name) return;
  await invoke("login_offline", { name });
  $("newOfflineName").value = "";
  await loadAccounts();
  status(`已添加离线账户 ${name}`);
});

$("msLogin").addEventListener("click", async () => {
  status("正在请求微软设备码…");
  try {
    await invoke("login_microsoft");
    await loadAccounts();
    $("msCode").classList.add("hidden");
    status("微软账户登录成功");
  } catch (e) {
    status(`登录失败: ${e}`);
  }
});

listen("ms-code", (event) => {
  const { userCode, verificationUri } = event.payload;
  const box = $("msCode");
  box.classList.remove("hidden");
  box.innerHTML = `请打开 <a href="${esc(verificationUri)}" target="_blank">${esc(verificationUri)}</a>，输入代码 <b>${esc(userCode)}</b>，完成后本窗口会自动继续。`;
  status("等待在浏览器中完成登录…");
});

/* ------------------------------------------------------------------ 联机（陶瓦联机 · 房间码） */
let tcPort = null;
let tcTimer = null;

const TC_STATE_TEXT = {
  "waiting": "待机",
  "host-scanning": "正在扫描本机 Minecraft…",
  "host-starting": "正在创建房间…",
  "host-ok": "房间已就绪",
  "guest-connecting": "正在连接房主…",
  "guest-starting": "正在建立连接…",
  "guest-ok": "已加入房间",
  "exception": "出错了",
};

function stopTcPolling() {
  if (tcTimer) {
    clearInterval(tcTimer);
    tcTimer = null;
  }
}

function startTcPolling() {
  stopTcPolling();
  tcTimer = setInterval(refreshTerracottaState, 900);
}

function setTcBadge(text, cls) {
  const badge = $("tcBadge");
  badge.textContent = text;
  badge.className = `badge ${cls}`;
}

async function refreshTerracotta() {
  // 1) 安装状态
  try {
    const s = await invoke("terracotta_status");
    $("tcInfo").innerHTML = `
      <div><span>安装</span><b>${s.installed ? "已安装" + (s.version ? " v" + esc(s.version) : "") : "未安装"}</b></div>
      <div><span>路径</span><span>${esc(s.exe ?? s.dir)}</span></div>
    `;
    if (!s.installed) setTcBadge("未安装", "grey");
    $("tcStart").disabled = !s.installed;
    $("tcHost").disabled = !s.installed;
    $("tcJoin").disabled = !s.installed;
  } catch (e) {
    logLine(`[联机] 安装状态检测失败: ${e}`);
  }
  // 2) 服务状态
  await refreshTerracottaState();
}

async function refreshTerracottaState() {
  if (tcPort === null) {
    // 可能已经在运行（之前启动过）：尝试直接读
    try {
      const port = await invoke("launch_terracotta");
      tcPort = port;
    } catch {
      setTcBadge("未启动", "grey");
      $("tcRoomBox").classList.add("hidden");
      $("tcMembers").innerHTML = "";
      return;
    }
  }

  try {
    const s = await invoke("terracotta_state");
    const state = s.state ?? "waiting";
    const room = s.room ?? "";

    setTcBadge(TC_STATE_TEXT[state] ?? state, state.endsWith("-ok") ? "ok" : state === "exception" ? "warn" : "");

    if (room) {
      $("tcRoomBox").classList.remove("hidden");
      $("tcRoomCode").textContent = room;
      $("tcRoomHint").textContent = state === "host-ok"
        ? "把这个码发给朋友，让他们在「加入房间」里粘贴即可。"
        : "房间正在建立，稍候房间码即可使用。";
    } else {
      $("tcRoomBox").classList.add("hidden");
    }

    if (s.url) {
      $("tcInfo").insertAdjacentHTML("beforeend", `<div><span>直连地址</span><code>${esc(s.url)}</code></div>`);
    }
    if (s.difficulty) {
      $("tcInfo").insertAdjacentHTML("beforeend", `<div><span>NAT 难度</span><b>${esc(s.difficulty)}</b></div>`);
    }
    if (s.type !== undefined && state === "exception") {
      $("tcMembers").innerHTML = `<div class="empty"><p>连接失败（类型 ${esc(s.type)}），请检查房间码或网络。</p></div>`;
    } else if (Array.isArray(s.profiles)) {
      $("tcMembers").innerHTML = s.profiles
        .map((p) => `<div class="member"><span class="member-name">${esc(p.name ?? "未知")}</span><span class="badge grey">${esc(p.kind ?? "")}</span></div>`)
        .join("");
    }
  } catch (e) {
    // 服务已退出
    tcPort = null;
    setTcBadge("已停止", "grey");
    $("tcRoomBox").classList.add("hidden");
    stopTcPolling();
  }
}

$("tcInstall").addEventListener("click", async () => {
  $("tcProgress").classList.remove("hidden");
  $("tcBar").style.width = "10%";
  $("tcText").textContent = "正在准备…";
  $("tcInstall").disabled = true;
  try {
    const r = await invoke("install_terracotta");
    $("tcBar").style.width = "100%";
    $("tcText").textContent = `安装完成：${r.asset}（v${r.version}）`;
    status(`陶瓦联机 v${r.version} 安装完成`);
    await refreshTerracotta();
  } catch (e) {
    $("tcText").textContent = `安装失败：${e}`;
    status(`陶瓦联机安装失败: ${e}`);
  } finally {
    $("tcInstall").disabled = false;
  }
});

$("tcStart").addEventListener("click", async () => {
  status("正在启动陶瓦联机服务…");
  try {
    tcPort = await invoke("launch_terracotta");
    status(`联机服务已就绪（端口 ${tcPort}）`);
    logLine(`[联机] Terracotta 已启动，端口 ${tcPort}`);
    startTcPolling();
    await refreshTerracottaState();
  } catch (e) {
    status(`启动失败: ${e}`);
    logLine(`[联机] 启动失败: ${e}`);
  }
});

$("tcHost").addEventListener("click", async () => {
  const player = $("tcPlayer").value.trim() || "PCL-RS 房主";
  try {
    await invoke("terracotta_host", { player });
    status("已请求创建房间：先在 Minecraft 里「对局域网开放」，扫描到后会自动生成房间码");
    startTcPolling();
  } catch (e) {
    status(`创建房间失败: ${e}`);
  }
});

$("tcJoin").addEventListener("click", async () => {
  const room = $("tcRoom").value.trim();
  if (!room) {
    status("请先粘贴房主的房间码");
    return;
  }
  const player = $("tcPlayer").value.trim() || "PCL-RS 玩家";
  try {
    await invoke("terracotta_join", { room, player });
    status("正在加入房间…");
    startTcPolling();
  } catch (e) {
    status(`加入失败: ${e}`);
  }
});

$("tcQuit").addEventListener("click", async () => {
  try {
    await invoke("stop_terracotta");
    tcPort = null;
    stopTcPolling();
    setTcBadge("已停止", "grey");
    $("tcRoomBox").classList.add("hidden");
    $("tcMembers").innerHTML = "";
    status("已退出联机");
  } catch (e) {
    status(`退出失败: ${e}`);
  }
});

$("tcRefresh").addEventListener("click", refreshTerracotta);

$("tcCopyRoom").addEventListener("click", async () => {
  const code = $("tcRoomCode").textContent.trim();
  if (!code || code === "-") return;
  try {
    await navigator.clipboard.writeText(code);
    status(`已复制房间码 ${code}`);
  } catch {
    status("复制失败，请手动选中房间码复制");
  }
});

listen("terracotta-progress", (event) => {
  $("tcProgress").classList.remove("hidden");
  $("tcText").textContent = event.payload;
  $("tcBar").style.width = "55%";
});

/* ------------------------------------------------------------------ 微软登录（浏览器 / 设备码） */
$("msLoginBrowser").addEventListener("click", async () => {
  status("已打开浏览器，请在网页中完成 Microsoft 登录…");
  try {
    const account = await invoke("login_microsoft_browser");
    await loadAccounts();
    status(`登录成功：${account.username}`);
    logLine(`[账户] 微软登录成功：${account.username}`);
  } catch (e) {
    status(`登录失败: ${e}`);
    logLine(`[账户] 浏览器登录失败: ${e}`);
  }
});
/* ------------------------------------------------------------------ 演示数据 */
async function demoInvoke(command) {
  const demo = {
    gameDir: "C:\\Users\\Demo\\.minecraft",
    memoryMb: 4096,
    source: "cn",
    javaPath: null,
    curseforgeKey: "",
    width: 854,
    height: 480,
    fullscreen: false,
  };
  switch (command) {
    case "get_settings":
    case "save_settings":
      return demo;
    case "list_installed":
      return [
        { id: "1.20.1", hasJar: true, javaMajor: 17 },
        { id: "1.20.1-forge-47.2.0", hasJar: true, javaMajor: 17 },
      ];
    case "version_info":
      return { id: "1.20.1", gameVersion: "1.20.1", loader: "fabric", javaMajor: 17, kind: "release" };
    case "list_versions":
      return ["1.21.4", "1.21.1", "1.20.6", "1.20.1", "1.19.2", "1.18.2", "1.16.5", "1.12.2", "1.8.9"].map((id, i) => ({
        id,
        kind: i === 2 ? "snapshot" : "release",
        releaseTime: new Date(Date.now() - i * 86400000 * 40).toISOString(),
        installed: i < 2,
      }));
    case "list_accounts":
      return [{ username: "Steve", uuid: "0".repeat(32), kind: "offline", selected: true, expired: false }];
    case "detect_java":
      return [{ major: 21, vendor: "Microsoft", arch: "x86_64", display: "Java 21.0.10 (Microsoft, x86_64)", path: "C:/jdk/bin/java.exe" }];
    case "terracotta_status":
      return { installed: true, exe: "C:/mc/pcl-rs/tools/terracotta/terracotta.exe", version: "0.4.2", running: false, dir: "C:/mc/pcl-rs/tools/terracotta" };
    case "search_mods":
      return [
        { source: "Modrinth", id: "sodium", slug: "sodium", name: "Sodium", summary: "现代化渲染优化模组", downloads: 12345678, iconUrl: null, url: "https://modrinth.com/mod/sodium" },
        { source: "Modrinth", id: "lithium", slug: "lithium", name: "Lithium", summary: "通用逻辑优化", downloads: 8765432, iconUrl: null, url: "https://modrinth.com/mod/lithium" },
      ];
    default:
      return [];
  }
}

/* ------------------------------------------------------------------ 启动 */
(async function main() {
  if (!isDesktop) $("statusSource").textContent += "（浏览器演示模式）";
  await loadSettings();
  fillSettings();
  await loadAccounts();
  await loadInstances();
  status("就绪");
})();

