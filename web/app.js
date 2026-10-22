/* PCL-RS 前端逻辑。
 * 通过 Tauri 的全局 API 调用 Rust 命令；在纯浏览器中打开时退化为演示数据，
 * 方便快速检查界面布局。 */

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

let settings = null;
let accounts = [];
let selectedAccount = null;

document.querySelectorAll(".nav-item").forEach((btn) => {
  btn.addEventListener("click", () => {
    document.querySelectorAll(".nav-item").forEach((b) => b.classList.toggle("active", b === btn));
    const page = btn.dataset.page;
    document.querySelectorAll(".page").forEach((p) => p.classList.toggle("active", p.id === `page-${page}`));
    if (page === "download") loadVersions();
    if (page === "instances") loadInstalled();
    if (page === "settings") fillSettings();
  });
});

async function loadSettings() {
  settings = await invoke("get_settings");
  $("statusSource").textContent = `线路: ${sourceLabel(settings.source)}`;
}

function sourceLabel(source) {
  return { cn: "BMCLAPI → 官方", global: "官方 → BMCLAPI", official: "仅官方" }[source] ?? source;
}

function fillSettings() {
  if (!settings) return;
  $("setGameDir").value = settings.gameDir ?? "";
  $("setMemory").value = settings.memoryMb ?? 4096;
  $("setWidth").value = settings.width ?? 854;
  $("setHeight").value = settings.height ?? 480;
  $("setFullscreen").checked = Boolean(settings.fullscreen);
  $("setSource").value = settings.source ?? "cn";
  $("setCfKey").value = settings.curseforgeKey ?? "";
  $("setJavaPath").value = settings.javaPath ?? "";
}

$("saveSettings").addEventListener("click", async () => {
  settings = {
    ...settings,
    gameDir: $("setGameDir").value.trim(),
    memoryMb: Number($("setMemory").value) || 4096,
    width: Number($("setWidth").value) || 854,
    height: Number($("setHeight").value) || 480,
    fullscreen: $("setFullscreen").checked,
    source: $("setSource").value,
    curseforgeKey: $("setCfKey").value.trim(),
    javaPath: $("setJavaPath").value.trim() || null,
  };
  settings = await invoke("save_settings", { settings });
  fillSettings();
  $("statusSource").textContent = `线路: ${sourceLabel(settings.source)}`;
  status("设置已保存");
});

$("scanJava").addEventListener("click", async () => {
  const list = await invoke("detect_java");
  $("javaList").innerHTML = list.length
    ? list.map((j) => `<div class="java-item">Java ${j.major} · ${j.vendor ?? "未知厂商"} · ${j.arch} · ${j.display}</div>`).join("")
    : `<div class="java-item">未找到 Java，请安装 JDK 17 或 21</div>`;
});

async function loadInstalled() {
  const installed = await invoke("list_installed");
  const select = $("launchVersion");
  const previous = select.value;
  select.innerHTML = installed.length
    ? installed.map((v) => `<option value="${v.id}">${v.id}${v.hasJar ? "" : "（缺少 jar）"}</option>`).join("")
    : `<option value="">（尚未安装任何版本）</option>`;
  if (previous && installed.some((v) => v.id === previous)) select.value = previous;

  $("instanceList").innerHTML = installed.length
    ? installed
        .map(
          (v) => `<div class="version-item">
            <span class="id">${v.id}</span>
            <span class="meta">${v.hasJar ? "客户端完整" : "缺少客户端 jar"}${v.javaMajor ? ` · 需要 Java ${v.javaMajor}` : ""}</span>
            <button class="btn ghost" data-verify="${v.id}">校验</button>
            <button class="btn" data-install="${v.id}">补全/更新</button>
          </div>`
        )
        .join("")
    : `<div class="version-item"><span class="meta">还没有安装任何版本，去「下载」页面安装一个吧</span></div>`;

  document.querySelectorAll("[data-verify]").forEach((btn) =>
    btn.addEventListener("click", () => verifyVersion(btn.dataset.verify))
  );
  document.querySelectorAll("[data-install]").forEach((btn) =>
    btn.addEventListener("click", () => installVersion(btn.dataset.install))
  );
}

let versionsLoaded = false;
async function loadVersions(force = false) {
  if (versionsLoaded && !force) return;
  status("正在获取版本清单…");
  const includeSnapshots = $("showSnapshots").checked;
  const versions = await invoke("list_versions", { includeSnapshots });
  versionsLoaded = true;
  renderVersions(versions);
  status(`共 ${versions.length} 个版本`);
  window.__versions = versions;
}

function renderVersions(versions) {
  const filter = $("versionSearch").value.trim().toLowerCase();
  const rows = versions.filter((v) => !filter || v.id.toLowerCase().includes(filter));
  $("versionList").innerHTML = rows
    .map(
      (v) => `<div class="version-item">
        <span class="id">${v.id}</span>
        <span class="badge ${v.kind === "release" ? "ok" : "warn"}">${v.kind === "release" ? "正式版" : "快照"}</span>
        <span class="meta">${(v.releaseTime ?? "").slice(0, 10)}</span>
        ${v.installed ? `<span class="badge ok">已安装</span>` : ""}
        <button class="btn ${v.installed ? "ghost" : "primary"}" data-install="${v.id}">${v.installed ? "补全" : "安装"}</button>
      </div>`
    )
    .join("");
  document.querySelectorAll("[data-install]").forEach((btn) =>
    btn.addEventListener("click", () => installVersion(btn.dataset.install))
  );
}

$("versionSearch").addEventListener("input", () => renderVersions(window.__versions ?? []));
$("showSnapshots").addEventListener("change", () => {
  versionsLoaded = false;
  loadVersions(true);
});
$("reloadVersions").addEventListener("click", () => {
  versionsLoaded = false;
  loadVersions(true);
});
$("reloadInstances").addEventListener("click", loadInstalled);
$("refreshInstalled").addEventListener("click", loadInstalled);

listen("install-progress", (event) => {
  const p = event.payload;
  const bar = $("dlBar");
  const text = $("dlText");
  $("dlProgress").classList.remove("hidden");
  bar.style.width = `${(p.fraction * 100).toFixed(1)}%`;
  const mb = (bytes) => (bytes / 1048576).toFixed(1);
  text.textContent = `${(p.fraction * 100).toFixed(1)}% · ${p.filesDone}/${p.filesTotal} 文件 · ${mb(p.bytesDone)}/${mb(p.bytesTotal)} MB ${p.current ? "· " + p.current : ""}`;

  const lbar = $("launchBar");
  if (lbar) {
    $("launchProgress").classList.remove("hidden");
    lbar.style.width = bar.style.width;
    $("launchText").textContent = text.textContent;
  }
});

async function installVersion(id) {
  if (!id) return;
  status(`正在安装 ${id}…`);
  try {
    await invoke("install_version", { id, verify: false });
    status(`${id} 安装完成`);
    logLine(`[PCL-RS] ${id} 安装完成`);
    await loadInstalled();
    versionsLoaded = false;
  } catch (e) {
    status(`安装失败: ${e}`);
    logLine(`[错误] ${e}`);
    $("dlProgress").classList.add("hidden");
  }
}

async function verifyVersion(id) {
  status(`正在校验 ${id}…`);
  const bad = await invoke("verify_version", { id });
  if (bad.length === 0) {
    status(`${id} 校验通过`);
  } else {
    status(`${id} 有 ${bad.length} 个文件损坏/缺失`);
    bad.slice(0, 20).forEach((p) => logLine(`[缺失] ${p}`));
  }
}

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
    const pid = await invoke("launch_game", {
      id,
      username,
      memoryMb: null,
      fullscreen: false,
      width: null,
      height: null,
    });
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

async function loadAccounts() {
  accounts = await invoke("list_accounts");
  const list = $("accountList");
  list.innerHTML = accounts.length
    ? accounts
        .map(
          (a) => `<div class="account-item ${a.selected ? "selected" : ""}" data-uuid="${a.uuid}">
            <span>${a.username}</span>
            <span class="kind">${a.kind === "microsoft" ? "微软" : "离线"}${a.expired ? " · 已过期" : ""}</span>
          </div>`
        )
        .join("")
    : `<div class="account-item"><span>暂无账户，添加一个离线账户或用微软登录</span></div>`;

  list.querySelectorAll("[data-uuid]").forEach((el) =>
    el.addEventListener("click", async () => {
      await invoke("select_account", { uuid: el.dataset.uuid });
      await loadAccounts();
    })
  );

  selectedAccount = accounts.find((a) => a.selected) ?? accounts[0] ?? null;
  $("accountChip").textContent = selectedAccount ? selectedAccount.username : "未登录";
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
  box.innerHTML = `请在浏览器打开 <a href="${verificationUri}" target="_blank">${verificationUri}</a>，输入代码 <b>${userCode}</b>，完成后本窗口会自动继续。`;
  status("等待在浏览器中完成登录…");
});

$("modSearch").addEventListener("click", async () => {
  const source = $("modSource").value;
  const query = $("modQuery").value.trim();
  const gameVersion = $("modVersion").value.trim() || null;
  const loader = $("modLoader").value || null;
  status("正在搜索 Mod…");
  try {
    const results = await invoke("search_mods", { source, query, gameVersion, loader, limit: 20 });
    $("modList").innerHTML = results.length
      ? results
          .map(
            (m) => `<div class="version-item">
              <span class="id">${m.name}</span>
              <span class="meta">${m.summary ?? ""}</span>
              <span class="badge">${m.source}</span>
              <button class="btn primary" data-mod="${m.id}" data-source="${source}">下载</button>
            </div>`
          )
          .join("")
      : `<div class="version-item"><span class="meta">没有找到结果</span></div>`;

    document.querySelectorAll("[data-mod]").forEach((btn) =>
      btn.addEventListener("click", () => downloadMod(btn.dataset.source, btn.dataset.mod))
    );
    status(`找到 ${results.length} 个 Mod`);
  } catch (e) {
    status(`搜索失败: ${e}`);
  }
});

async function downloadMod(source, project) {
  const gameVersion = $("modVersion").value.trim();
  if (!gameVersion) {
    status("请先填写 MC 版本");
    return;
  }
  const loader = $("modLoader").value || null;
  status("正在解析 Mod 文件…");
  try {
    const dest = await invoke("download_mod", { source, project, gameVersion, loader });
    status(`已下载到 ${dest}`);
    logLine(`[Mod] ${dest}`);
  } catch (e) {
    status(`下载失败: ${e}`);
  }
}

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
    windowWidth: 1120,
    windowHeight: 720,
  };
  switch (command) {
    case "get_settings":
    case "save_settings":
      return demo;
    case "list_installed":
      return [
        { id: "1.20.1", hasJar: true, javaMajor: 17 },
        { id: "1.21.4", hasJar: true, javaMajor: 21 },
      ];
    case "list_versions":
      return ["1.21.4", "1.21.1", "1.20.6", "1.20.1", "1.19.2", "1.18.2", "1.16.5", "1.12.2", "1.8.9"].map(
        (id, i) => ({
          id,
          kind: i === 2 ? "snapshot" : "release",
          releaseTime: new Date(Date.now() - i * 86400000 * 40).toISOString(),
          installed: i < 2,
        })
      );
    case "list_accounts":
      return [{ username: "Steve", uuid: "0".repeat(32), kind: "offline", selected: true, expired: false }];
    case "detect_java":
      return [{ major: 21, vendor: "Microsoft", arch: "x86_64", display: "Java 21.0.10 (Microsoft, x86_64)", path: "C:/jdk/bin/java.exe" }];
    default:
      return [];
  }
}

(async function main() {
  if (!isDesktop) {
    $("statusSource").textContent += "（浏览器演示模式）";
  }
  await loadSettings();
  fillSettings();
  await loadInstalled();
  await loadAccounts();
  status("就绪");
})();
