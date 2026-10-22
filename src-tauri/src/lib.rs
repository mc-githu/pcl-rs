//! Tauri command surface for PCL-RS.
//!
//! The frontend (`web/`) is deliberately framework-free: `tauri.conf.json` points
//! `frontendDist` at plain files, so the desktop app builds without a Node
//! toolchain. Every command here is also reachable through the `pcl` CLI, which
//! keeps the GUI a thin layer over tested library code.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;

use pcl_auth::{Account, AccountKind, AccountStore, MicrosoftAuth};
use pcl_core::GamePaths;
use pcl_install::{InstallOptions, Installer};
use pcl_launch::args::LaunchOptions;
use pcl_launch::java;
use pcl_launch::launcher::Launcher;
use pcl_mods::{CurseForge, Loader, Modrinth, SearchQuery};
use pcl_net::{DownloadTask, Downloader, HttpClient, Progress, SourcePool};

pub mod terracotta;

pub use terracotta::{TerracottaInstall, TerracottaStatus};

// ---------------------------------------------------------------------------
// View models (camelCase for the JS side)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub game_dir: String,
    pub memory_mb: u32,
    pub source: String,
    pub java_path: Option<String>,
    pub curseforge_key: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            game_dir: GamePaths::default_root().display().to_string(),
            memory_mb: 4096,
            source: "cn".to_string(),
            java_path: None,
            curseforge_key: None,
            width: 854,
            height: 480,
            fullscreen: false,
        }
    }
}

impl Settings {
    fn store_path(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(&self.game_dir)
            .join("pcl-rs")
            .join("settings.json")
    }

    fn load() -> Self {
        let mut settings = Settings::default();
        if let Ok(text) = std::fs::read_to_string(settings.store_path()) {
            if let Ok(loaded) = serde_json::from_str::<Settings>(&text) {
                settings = loaded;
            }
        }
        settings
    }

    fn save(&self) -> std::io::Result<()> {
        let path = self.store_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default())
    }

    fn paths(&self) -> GamePaths {
        GamePaths::new(&self.game_dir)
    }

    fn source_pool(&self) -> SourcePool {
        match self.source.as_str() {
            "official" => SourcePool::official(),
            "global" => SourcePool::global(),
            _ => SourcePool::cn(),
        }
    }
}

pub struct AppState {
    settings: Mutex<Settings>,
    terracotta: terracotta::TerracottaState,
    /// 与 Terracotta 内嵌 HTTP API 通信的端口（集成模式）。
    tc: terracotta::Integration,
}

impl AppState {
    fn new() -> Self {
        Self {
            settings: Mutex::new(Settings::load()),
            terracotta: terracotta::TerracottaState::default(),
            tc: terracotta::Integration::default(),
        }
    }

    fn settings(&self) -> Settings {
        self.settings.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionView {
    pub id: String,
    pub kind: String,
    pub release_time: String,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledView {
    pub id: String,
    pub has_jar: bool,
    pub java_major: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaView {
    pub path: String,
    pub version: String,
    pub major: u32,
    pub vendor: Option<String>,
    pub arch: String,
    pub source: String,
    pub display: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub username: String,
    pub uuid: String,
    pub kind: String,
    pub selected: bool,
    pub expired: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressView {
    pub fraction: f32,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub current: Option<String>,
    pub finished: bool,
}

impl From<Progress> for ProgressView {
    fn from(p: Progress) -> Self {
        Self {
            fraction: p.fraction(),
            files_done: p.files_done,
            files_total: p.files_total,
            bytes_done: p.bytes_done,
            bytes_total: p.bytes_total,
            current: p.current,
            finished: p.finished,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModView {
    pub source: String,
    pub id: String,
    pub slug: String,
    pub name: String,
    pub summary: String,
    pub author: Option<String>,
    pub downloads: u64,
    pub icon_url: Option<String>,
    pub url: String,
}

/// Everything the UI needs to scope downloads to one instance: the Minecraft
/// version it runs and the mod loader it uses.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfoView {
    pub id: String,
    pub game_version: String,
    /// `vanilla` | `fabric` | `quilt` | `forge` | `neoforge`
    pub loader: String,
    pub java_major: Option<u32>,
    pub kind: String,
}

/// Detect the mod loader from a version profile.
///
/// Loader profiles are identified by their libraries first (reliable) and only
/// then by the version id, because users rename profile folders freely.
pub fn detect_loader(details: &pcl_core::VersionDetails) -> String {
    for lib in &details.libraries {
        let name = lib.name.to_ascii_lowercase();
        if name.contains("fabric-loader") || name.contains("net.fabricmc:fabric") {
            return "fabric".to_string();
        }
        if name.contains("quilt-loader") {
            return "quilt".to_string();
        }
        if name.contains("neoforge") {
            return "neoforge".to_string();
        }
        if name.contains("net.minecraftforge:forge") || name.contains("net.minecraftforge:fmlloader") {
            return "forge".to_string();
        }
    }

    let id = details.id.to_ascii_lowercase();
    for (needle, loader) in [
        ("neoforge", "neoforge"),
        ("fabric", "fabric"),
        ("quilt", "quilt"),
        ("forge", "forge"),
    ] {
        if id.contains(needle) {
            return loader.to_string();
        }
    }
    "vanilla".to_string()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

fn progress_sink(app: AppHandle) -> pcl_net::ProgressSink {
    Arc::new(move |progress: Progress| {
        let _ = app.emit("install-progress", ProgressView::from(progress));
    })
}

fn account_store(settings: &Settings) -> AccountStore {
    AccountStore::load(&AccountStore::default_path(settings.paths().root())).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings()
}

/// 打开系统目录选择器，返回选中的游戏目录（`.minecraft` 所在目录）。
///
/// 用插件阻塞式 API 而不是 JS 端 `dialog.open()`：命令走 Rust 侧就不需要额外的
/// capability 权限声明，前端只拿一个字符串。
#[tauri::command]
fn pick_game_dir(app: AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .set_title("选择游戏目录（包含 versions / assets / libraries 的文件夹）")
        .blocking_pick_folder()
        .map(|path| path.to_string())
}

/// 默认游戏目录，用于「使用默认」按钮。
#[tauri::command]
fn default_game_dir() -> String {
    GamePaths::default_root().display().to_string()
}

#[tauri::command]
fn save_settings(state: State<'_, AppState>, settings: Settings) -> Result<Settings, String> {
    settings.save().map_err(err)?;
    if let Ok(mut guard) = state.settings.lock() {
        *guard = settings.clone();
    }
    Ok(settings)
}

#[tauri::command]
async fn list_versions(
    app: AppHandle,
    state: State<'_, AppState>,
    include_snapshots: bool,
) -> Result<Vec<VersionView>, String> {
    let settings = state.settings();
    let installer = Installer::new(settings.paths(), settings.source_pool()).map_err(err)?;
    let _ = installer.warm_up().await;
    let versions = installer.list_versions().await.map_err(err)?;
    let paths = settings.paths();
    let _ = app.emit("versions-loaded", versions.len());

    Ok(versions
        .into_iter()
        .filter(|v| include_snapshots || v.kind == "release")
        .map(|v| VersionView {
            installed: paths.is_installed(&v.id),
            id: v.id,
            kind: v.kind,
            release_time: v.release_time.unwrap_or_default(),
        })
        .collect())
}

#[tauri::command]
fn list_installed(state: State<'_, AppState>) -> Vec<InstalledView> {
    let settings = state.settings();
    let paths = settings.paths();
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(paths.versions_dir()) {
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().to_string();
            let json = paths.version_json(&id);
            if !json.is_file() {
                continue;
            }
            let java_major = std::fs::read_to_string(&json)
                .ok()
                .and_then(|text| serde_json::from_str::<pcl_core::VersionDetails>(&text).ok())
                .and_then(|details| details.required_java_major());
            out.push(InstalledView {
                has_jar: paths.version_jar(&id).is_file(),
                id,
                java_major,
            });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[tauri::command]
async fn install_version(    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    verify: bool,
) -> Result<InstalledView, String> {
    let settings = state.settings();
    let options = InstallOptions {
        verify_existing: verify,
        ..Default::default()
    };
    let installer = Installer::new(settings.paths(), settings.source_pool())
        .map_err(err)?
        .with_options(options.clone());

    let installed = installer
        .install(&id, &options, Some(progress_sink(app)))
        .await
        .map_err(err)?;

    Ok(InstalledView {
        has_jar: installed.client_jar.is_file(),
        id: installed.id,
        java_major: installed.details.required_java_major(),
    })
}

#[tauri::command]
async fn verify_version(state: State<'_, AppState>, id: String) -> Result<Vec<String>, String> {
    let settings = state.settings();
    let installer = Installer::new(settings.paths(), settings.source_pool()).map_err(err)?;
    let results = installer.verify(&id).await.map_err(err)?;
    Ok(results
        .into_iter()
        .filter(|(_, ok)| !ok)
        .map(|(path, _)| path.display().to_string())
        .collect())
}

#[tauri::command]
fn detect_java(state: State<'_, AppState>) -> Vec<JavaView> {
    let settings = state.settings();
    let mut runtimes = java::detect();
    if let Some(custom) = settings.java_path.as_ref().filter(|p| !p.is_empty()) {
        if let Ok(runtime) = java::probe_path(std::path::Path::new(custom)) {
            runtimes.insert(0, runtime);
        }
    }
    runtimes
        .into_iter()
        .map(|r| JavaView {
            display: r.display(),
            path: r.path.display().to_string(),
            version: r.version,
            major: r.major,
            vendor: r.vendor,
            arch: r.arch,
            source: r.source.label().to_string(),
        })
        .collect()
}

#[tauri::command]
async fn launch_game(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    username: Option<String>,
    memory_mb: Option<u32>,
    fullscreen: bool,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<u32, String> {
    let settings = state.settings();
    let paths = settings.paths();

    let account = match username.filter(|u| !u.trim().is_empty()) {
        Some(name) => Account::offline(&name),
        None => account_store(&settings)
            .selected_account()
            .cloned()
            .unwrap_or_else(|| Account::offline("Player")),
    };

    let installer = Installer::new(paths.clone(), settings.source_pool()).map_err(err)?;
    let installed = installer
        .install(&id, &InstallOptions::default(), Some(progress_sink(app.clone())))
        .await
        .map_err(err)?;

    let runtime = match settings.java_path.as_ref().filter(|p| !p.is_empty()) {
        Some(path) => java::probe_path(std::path::Path::new(path)).map_err(err)?,
        None => {
            let all = java::detect();
            java::pick(&all, installed.details.required_java_major())
                .ok_or_else(|| "未找到可用的 Java，请在设置中指定".to_string())?
        }
    };

    let spec = LaunchOptions {
        details: installed.details.clone(),
        java: runtime.path.clone(),
        game_dir: paths.root().to_path_buf(),
        assets_dir: paths.assets_dir(),
        libraries_dir: paths.libraries_dir(),
        natives_dir: installed.natives_dir.clone(),
        classpath: installed.classpath.clone(),
        account,
        version_type: installed
            .details
            .kind
            .clone()
            .unwrap_or_else(|| "release".to_string()),
        assets_index: installed.asset_index.clone(),
        log_config: installed.log_config.clone(),
        memory_mb: memory_mb.unwrap_or(settings.memory_mb),
        extra_jvm_args: Vec::new(),
        extra_game_args: Vec::new(),
        fullscreen: fullscreen || settings.fullscreen,
        width: width.or(Some(settings.width)),
        height: height.or(Some(settings.height)),
        launcher_name: "PCL-RS".to_string(),
        launcher_version: env!("CARGO_PKG_VERSION").to_string(),
    }
    .build()
    .map_err(err)?;

    let launcher = Launcher::new(paths.logs_dir());
    let sink_app = app.clone();
    let handle = launcher
        .launch(
            &spec,
            &[],
            Some(Arc::new(move |line: &str| {
                let _ = sink_app.emit("game-log", line.to_string());
            })),
        )
        .await
        .map_err(err)?;

    let pid = handle.pid;
    let _ = app.emit("game-started", pid);

    let exit_app = app.clone();
    tokio::spawn(async move {
        let code = handle.wait().await.unwrap_or(-1);
        let _ = exit_app.emit("game-exit", code);
    });

    Ok(pid)
}

#[tauri::command]
fn list_accounts(state: State<'_, AppState>) -> Vec<AccountView> {
    let settings = state.settings();
    let store = account_store(&settings);
    store
        .accounts
        .iter()
        .map(|a| AccountView {
            username: a.username.clone(),
            uuid: a.uuid.clone(),
            kind: match a.kind {
                AccountKind::Microsoft => "microsoft".to_string(),
                AccountKind::Offline => "offline".to_string(),
            },
            selected: store.selected.as_deref() == Some(a.uuid.as_str()),
            expired: a.is_expired(),
        })
        .collect()
}

#[tauri::command]
fn login_offline(state: State<'_, AppState>, name: String) -> Result<AccountView, String> {
    if name.trim().is_empty() {
        return Err("用户名不能为空".to_string());
    }
    let settings = state.settings();
    let mut store = account_store(&settings);
    let account = Account::offline(name.trim());
    store.upsert(account.clone());
    store
        .save(&AccountStore::default_path(settings.paths().root()))
        .map_err(err)?;

    Ok(AccountView {
        username: account.username,
        uuid: account.uuid,
        kind: "offline".to_string(),
        selected: true,
        expired: false,
    })
}

/// 浏览器登录：拉起系统浏览器，用户在网页里完成登录后自动回调本机端口。
#[tauri::command]
async fn login_microsoft_browser(state: State<'_, AppState>) -> Result<AccountView, String> {
    let settings = state.settings();
    let client = HttpClient::new(SourcePool::official()).map_err(err)?;
    let auth = MicrosoftAuth::new(client.clone());

    let session = auth.begin_browser_login().await.map_err(err)?;
    pcl_auth::browser::open_browser(&session.authorize_url).map_err(err)?;

    let account = auth
        .complete_browser_login(session, std::time::Duration::from_secs(300))
        .await
        .map_err(err)?;

    let mut store = account_store(&settings);
    store.upsert(account.clone());
    store
        .save(&AccountStore::default_path(settings.paths().root()))
        .map_err(err)?;

    Ok(AccountView {
        username: account.username,
        uuid: account.uuid,
        kind: "microsoft".to_string(),
        selected: true,
        expired: false,
    })
}

#[tauri::command]
async fn login_microsoft(app: AppHandle, state: State<'_, AppState>) -> Result<AccountView, String> {
    let settings = state.settings();
    let client = HttpClient::new(SourcePool::official()).map_err(err)?;
    let auth = MicrosoftAuth::new(client);

    let notify_app = app.clone();
    let account = auth
        .login(move |flow| {
            let _ = notify_app.emit(
                "ms-code",
                serde_json::json!({
                    "userCode": flow.user_code,
                    "verificationUri": flow.verification_uri,
                }),
            );
        })
        .await
        .map_err(err)?;

    let mut store = account_store(&settings);
    store.upsert(account.clone());
    store
        .save(&AccountStore::default_path(settings.paths().root()))
        .map_err(err)?;

    Ok(AccountView {
        username: account.username,
        uuid: account.uuid,
        kind: "microsoft".to_string(),
        selected: true,
        expired: false,
    })
}

#[tauri::command]
fn select_account(state: State<'_, AppState>, uuid: String) -> Result<(), String> {
    let settings = state.settings();
    let mut store = account_store(&settings);
    store.selected = Some(uuid);
    store
        .save(&AccountStore::default_path(settings.paths().root()))
        .map_err(err)
}

#[tauri::command]
async fn search_mods(
    state: State<'_, AppState>,
    source: String,
    query: String,
    game_version: Option<String>,
    loader: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<ModView>, String> {
    let settings = state.settings();
    let client = HttpClient::new(SourcePool::official()).map_err(err)?;

    let mut q = SearchQuery::new(query).with_limit(limit.unwrap_or(20));
    q.game_version = game_version;
    q.loader = loader.as_deref().and_then(Loader::parse);

    let results = if source == "curseforge" {
        CurseForge::new(client, settings.curseforge_key.clone().unwrap_or_default())
            .search(&q)
            .await
            .map_err(err)?
    } else {
        Modrinth::new(client).search(&q).await.map_err(err)?
    };

    Ok(results
        .projects
        .into_iter()
        .map(|p| ModView {
            source: p.source.label().to_string(),
            id: p.id,
            slug: p.slug,
            name: p.name,
            summary: p.summary,
            author: p.author,
            downloads: p.downloads,
            icon_url: p.icon_url,
            url: p.url,
        })
        .collect())
}

#[tauri::command]
async fn download_mod(
    app: AppHandle,
    state: State<'_, AppState>,
    source: String,
    project: String,
    game_version: String,
    loader: Option<String>,
) -> Result<String, String> {
    let settings = state.settings();
    let client = HttpClient::new(SourcePool::official()).map_err(err)?;
    let loader = loader.as_deref().and_then(Loader::parse);

    let file = if source == "curseforge" {
        CurseForge::new(client, settings.curseforge_key.clone().unwrap_or_default())
            .files(&project, Some(&game_version), loader)
            .await
            .map_err(err)?
            .into_iter()
            .next()
    } else {
        Modrinth::new(client)
            .versions(&project, Some(&game_version), loader)
            .await
            .map_err(err)?
            .into_iter()
            .next()
    }
    .ok_or_else(|| "没有找到兼容的 Mod 文件".to_string())?;

    let Some(url) = file.download_url.clone() else {
        return Err(format!(
            "{} 的作者关闭了第三方下载，请到 {} 手动下载",
            file.file_name,
            if source == "curseforge" {
                format!("https://www.curseforge.com/minecraft/mc-mods/{project}")
            } else {
                format!("https://modrinth.com/mod/{project}")
            }
        ));
    };

    let dest = settings.paths().mods_dir().join(&file.file_name);
    let client = HttpClient::new(SourcePool::official()).map_err(err)?;
    let downloader = Downloader::new(client.client().clone(), SourcePool::official());
    let task = DownloadTask::new(url, dest.clone()).label(file.file_name.clone());
    downloader
        .run(vec![task], Some(progress_sink(app)))
        .await
        .map_err(err)?;

    Ok(dest.display().to_string())
}

/// Metadata for the instance currently selected in the download page.
#[tauri::command]
fn version_info(state: State<'_, AppState>, id: String) -> Result<VersionInfoView, String> {
    let settings = state.settings();
    let paths = settings.paths();
    let json = paths.version_json(&id);
    let text = std::fs::read_to_string(&json).map_err(err)?;
    let details: pcl_core::VersionDetails = serde_json::from_str(&text).map_err(err)?;

    // Loader profiles carry the vanilla version in `jar` (or `inheritsFrom`);
    // vanilla profiles are their own game version.
    let game_version = details
        .jar
        .clone()
        .or_else(|| details.inherits_from.clone())
        .unwrap_or_else(|| details.id.clone());

    Ok(VersionInfoView {
        loader: detect_loader(&details),
        game_version,
        java_major: details.required_java_major(),
        kind: details.kind.clone().unwrap_or_else(|| "release".to_string()),
        id: details.id,
    })
}

// ---------------------------------------------------------------------------
// 陶瓦联机（Terracotta）
// ---------------------------------------------------------------------------

#[tauri::command]
fn terracotta_status(state: State<'_, AppState>) -> TerracottaStatus {
    let settings = state.settings();
    let mut status = terracotta::status(settings.paths().root());
    status.running = state.terracotta.running();
    status
}

#[tauri::command]
async fn install_terracotta(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TerracottaInstall, String> {
    let settings = state.settings();
    let dir = settings.paths().root().to_path_buf();

    let emitter = app.clone();
    let result = terracotta::install(&dir, move |stage| {
        let _ = emitter.emit("terracotta-progress", stage.to_string());
    })
    .await
    .map_err(err)?;

    Ok(result)
}

/// 以集成模式启动 Terracotta，返回它内嵌 HTTP 服务的端口。
#[tauri::command]
async fn launch_terracotta(app: AppHandle, state: State<'_, AppState>) -> Result<u16, String> {
    // Alread running? Just reuse the port.
    if let Some(port) = state.tc.port() {
        if terracotta::fetch_state(port).await.is_ok() {
            return Ok(port);
        }
        state.tc.set_port(None);
    }

    let settings = state.settings();
    let game_dir = settings.paths().root().to_path_buf();
    let status = terracotta::status(&game_dir);
    let exe = status
        .exe
        .as_ref()
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "还没有安装陶瓦联机，请先点「一键安装」".to_string())?;

    let port = terracotta::launch_integrated(&game_dir, &exe, &state.tc)
        .await
        .map_err(err)?;
    let _ = app.emit("terracotta-started", port);
    Ok(port)
}

/// 读取房间状态（房间码、成员、直连地址都由它给出）。
#[tauri::command]
async fn terracotta_state(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let port = state
        .tc
        .port()
        .ok_or_else(|| "陶瓦联机尚未启动".to_string())?;
    terracotta::fetch_state(port).await.map_err(err)
}

/// 创建房间（房主）：先让 Terracotta 扫描本机 Minecraft，再进入 host 流程。
#[tauri::command]
async fn terracotta_host(state: State<'_, AppState>, player: String) -> Result<(), String> {
    let port = state
        .tc
        .port()
        .ok_or_else(|| "陶瓦联机尚未启动".to_string())?;
    terracotta::host(port, &player).await.map_err(err)
}

/// 通过房间码加入房间。
#[tauri::command]
async fn terracotta_join(
    state: State<'_, AppState>,
    room: String,
    player: String,
) -> Result<(), String> {
    let port = state
        .tc
        .port()
        .ok_or_else(|| "陶瓦联机尚未启动".to_string())?;
    terracotta::join(port, &room, &player).await.map_err(err)
}

/// 优雅退出 Terracotta。
#[tauri::command]
async fn stop_terracotta(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(port) = state.tc.port() {
        let _ = terracotta::quit(port).await;
    }
    state.tc.set_port(None);
    state.terracotta.stop().map_err(err)
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            pick_game_dir,
            default_game_dir,
            list_versions,
            list_installed,
            version_info,
            install_version,
            verify_version,
            detect_java,
            launch_game,
            list_accounts,
            login_offline,
            login_microsoft,
            select_account,
            search_mods,
            download_mod,
            terracotta_status,
            install_terracotta,
            launch_terracotta,
            terracotta_state,
            terracotta_host,
            terracotta_join,
            stop_terracotta,
            login_microsoft_browser,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PCL-RS");
}
