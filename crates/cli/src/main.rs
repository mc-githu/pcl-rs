//! `pcl` — the PCL-RS command line.
//!
//! Every GUI action has a CLI equivalent, which keeps the launcher scriptable
//! (CI can install a version and boot it headless) and makes the core testable
//! without a window.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use pcl_auth::{Account, AccountStore, MicrosoftAuth};
use pcl_core::GamePaths;
use pcl_install::{InstallOptions, Installer};
use pcl_launch::args::LaunchOptions;
use pcl_launch::java;
use pcl_launch::launcher::Launcher;
use pcl_mods::{CurseForge, Loader, Modrinth, SearchQuery};
use pcl_net::{DownloadTask, Downloader, HttpClient, Progress, SourcePool};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "pcl",
    version,
    about = "PCL-RS — 用 Rust 重写的 Minecraft 启动器（CLI）",
    long_about = "PCL-RS 命令行：安装、校验、启动 Minecraft，管理账户与 Mod。\n\
                  下载走 CDN 加速线路（BMCLAPI）并自动回退到官方源。"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// 不显示下载进度
    #[arg(long, global = true)]
    quiet: bool,
}

#[derive(Subcommand)]
enum Command {
    /// 安装一个 Minecraft 版本（下载缺失的文件）
    Install {
        version: String,
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = SourceArg::Cn)]
        source: SourceArg,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        no_assets: bool,
        #[arg(long, default_value_t = 16)]
        concurrency: usize,
    },

    /// 列出所有可安装版本（或本地已安装版本）
    List {
        #[arg(long)]
        installed: bool,
        #[arg(long)]
        dir: Option<PathBuf>,
    },

    /// 启动一个版本（缺失文件会自动补全）
    Launch {
        version: String,
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        username: Option<String>,
        #[arg(long)]
        ms: bool,
        #[arg(long, default_value_t = 4096)]
        memory: u32,
        #[arg(long)]
        width: Option<u32>,
        #[arg(long)]
        height: Option<u32>,
        #[arg(long)]
        fullscreen: bool,
        #[arg(long = "jvm-arg")]
        jvm_args: Vec<String>,
        #[arg(long = "game-arg")]
        game_args: Vec<String>,
        #[arg(long)]
        java: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = SourceArg::Cn)]
        source: SourceArg,
        #[arg(long)]
        dry_run: bool,
    },

    /// 校验已安装版本的完整性（重新计算 SHA-1）
    Verify {
        version: String,
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = SourceArg::Cn)]
        source: SourceArg,
    },

    /// 显示版本的详细信息
    Info {
        version: String,
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = SourceArg::Cn)]
        source: SourceArg,
    },

    /// 列出本机可用的 Java
    Java {
        #[arg(long)]
        json: bool,
    },

    /// 账户管理
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },

    /// 搜索 / 下载 Mod（Modrinth 与 CurseForge 官方 API）
    Mods {
        #[command(subcommand)]
        command: ModsCommand,
    },
}

#[derive(Subcommand)]
enum AccountCommand {
    Offline {
        name: String,
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    Ms {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        client_id: Option<String>,
    },
    List {
        #[arg(long)]
        dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ModsCommand {
    Search {
        query: String,
        #[arg(long, value_enum, default_value_t = ModSourceArg::Modrinth)]
        source: ModSourceArg,
        #[arg(long)]
        version: Option<String>,
        #[arg(long, value_enum)]
        loader: Option<LoaderArg>,
        #[arg(long, default_value_t = 10)]
        limit: u32,
        #[arg(long)]
        api_key: Option<String>,
    },
    Download {
        project: String,
        #[arg(long)]
        version: String,
        #[arg(long, value_enum, default_value_t = ModSourceArg::Modrinth)]
        source: ModSourceArg,
        #[arg(long, value_enum)]
        loader: Option<LoaderArg>,
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        api_key: Option<String>,
    },
}

#[derive(Copy, Clone, ValueEnum)]
enum SourceArg {
    /// 官方源
    Official,
    /// CDN 加速（BMCLAPI）+ 官方回退
    Cn,
    /// 官方优先 + CDN 回退
    Global,
}

#[derive(Copy, Clone, ValueEnum)]
enum ModSourceArg {
    Modrinth,
    #[value(name = "curseforge")]
    Curseforge,
}

#[derive(Copy, Clone, ValueEnum)]
enum LoaderArg {
    Fabric,
    Quilt,
    Forge,
    #[value(name = "neoforge")]
    Neoforge,
}

impl LoaderArg {
    fn to_loader(self) -> Loader {
        match self {
            LoaderArg::Fabric => Loader::Fabric,
            LoaderArg::Quilt => Loader::Quilt,
            LoaderArg::Forge => Loader::Forge,
            LoaderArg::Neoforge => Loader::NeoForge,
        }
    }
}

fn sources(arg: SourceArg) -> SourcePool {
    match arg {
        SourceArg::Official => SourcePool::official(),
        SourceArg::Cn => SourcePool::cn(),
        SourceArg::Global => SourcePool::global(),
    }
}

fn game_paths(dir: Option<PathBuf>) -> GamePaths {
    GamePaths::new(dir.unwrap_or_else(GamePaths::default_root))
}

fn progress_sink(quiet: bool) -> Option<pcl_net::ProgressSink> {
    if quiet {
        return None;
    }
    let last_percent = Arc::new(AtomicU64::new(u64::MAX));
    Some(Arc::new(move |p: Progress| {
        let percent = (p.fraction() * 1000.0) as u64;
        if percent == last_percent.load(Ordering::Relaxed) && !p.finished {
            return;
        }
        last_percent.store(percent, Ordering::Relaxed);
        let current: String = p.current.as_deref().unwrap_or("").chars().take(40).collect();
        eprint!(
            "\r  {:>5.1}%  {:>4}/{:>4} 文件  {:>7.1}/{:>7.1} MB  {:<40}",
            p.fraction() * 100.0,
            p.files_done,
            p.files_total,
            p.bytes_done as f64 / 1_048_576.0,
            p.bytes_total as f64 / 1_048_576.0,
            current,
        );
        if p.finished {
            eprintln!();
        }
    }))
}

#[tokio::main]
async fn main() -> Result<()> {
    enable_utf8_console();

    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    match cli.command {
        Command::Install {
            version,
            dir,
            source,
            dry_run,
            no_assets,
            concurrency,
        } => {
            let paths = game_paths(dir);
            let options = InstallOptions {
                download_assets: !no_assets,
                concurrency,
                ..Default::default()
            };
            let installer =
                Installer::new(paths.clone(), sources(source))?.with_options(options.clone());
            println!("游戏目录: {}", paths.root().display());
            println!("下载线路: {}", installer.http().sources().names().join(" -> "));
            warm_up(&installer, cli.quiet).await;

            if dry_run {
                let (details, plan) = installer.plan_full(&version, &options).await?;
                println!();
                println!(
                    "版本 {} ({})",
                    details.id,
                    details.kind.clone().unwrap_or_default()
                );
                println!("主类: {}", details.main_class.clone().unwrap_or_default());
                println!(
                    "需要 Java: {}",
                    details
                        .required_java_major()
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "未声明".into())
                );
                println!(
                    "计划下载: {} 个文件, {:.1} MB",
                    plan.tasks.len(),
                    plan.total_bytes() as f64 / 1_048_576.0
                );
                println!("类路径条目: {}", plan.classpath.len());
                return Ok(());
            }

            let installed = installer
                .install(&version, &options, progress_sink(cli.quiet))
                .await?;
            println!(
                "完成: {} 下载 {} 个文件, 跳过 {} 个\n  客户端: {}\n  类路径: {} 项",
                installed.id,
                installed.downloaded,
                installed.skipped,
                installed.client_jar.display(),
                installed.classpath.len()
            );
        }

        Command::List { installed, dir } => {
            let paths = game_paths(dir);
            if installed {
                let mut found = Vec::new();
                if let Ok(entries) = std::fs::read_dir(paths.versions_dir()) {
                    for entry in entries.flatten() {
                        let id = entry.file_name().to_string_lossy().to_string();
                        if paths.version_json(&id).is_file() {
                            let jar = paths.version_jar(&id).is_file();
                            found.push(format!("{id}{}", if jar { "" } else { "  (缺少 jar)" }));
                        }
                    }
                }
                found.sort();
                if found.is_empty() {
                    println!("没有已安装的版本（目录: {}）", paths.versions_dir().display());
                } else {
                    for line in found {
                        println!("{line}");
                    }
                }
            } else {
                let installer = Installer::new(paths, SourcePool::cn())?;
                warm_up(&installer, cli.quiet).await;
                let manifest = installer.manifest().await?;
                println!(
                    "最新正式版: {}   最新快照: {}",
                    manifest.latest.release, manifest.latest.snapshot
                );
                for v in manifest.versions.iter().take(40) {
                    println!(
                        "{:<18} {:<9} {}",
                        v.id,
                        v.kind,
                        v.release_time.clone().unwrap_or_default()
                    );
                }
                println!("... 共 {} 个版本", manifest.versions.len());
            }
        }

        Command::Launch {
            version,
            dir,
            username,
            ms,
            memory,
            width,
            height,
            fullscreen,
            jvm_args,
            game_args,
            java: java_path,
            source,
            dry_run,
        } => {
            let paths = game_paths(dir);
            let account = resolve_account(&paths, username, ms).await?;
            let offline_login = account.is_offline();
            println!(
                "账户: {} ({})",
                account.username,
                if offline_login { "离线" } else { "微软" }
            );

            let installer = Installer::new(paths.clone(), sources(source))?;
            warm_up(&installer, cli.quiet).await;
            let installed = installer
                .install(&version, &InstallOptions::default(), progress_sink(cli.quiet))
                .await?;

            let required = installed.details.required_java_major();
            let runtime = match java_path {
                Some(path) => java::probe_path(&path)?,
                None => {
                    let all = java::detect();
                    if all.is_empty() {
                        bail!("未找到任何 Java，请安装 JDK 17/21 或用 --java 指定");
                    }
                    java::pick(&all, required)
                        .ok_or_else(|| anyhow::anyhow!("没有满足 Java {required:?} 的运行时"))?
                }
            };
            println!("Java: {} -> {}", runtime.display(), runtime.path.display());

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
                memory_mb: memory,
                extra_jvm_args: jvm_args,
                extra_game_args: game_args,
                fullscreen,
                width,
                height,
                launcher_name: "PCL-RS".to_string(),
                launcher_version: env!("CARGO_PKG_VERSION").to_string(),
            }
            .build()?;

            if dry_run {
                println!("{}\n(cwd: {})", spec.display(), spec.cwd.display());
                return Ok(());
            }

            print_precheck(offline_login, cli.quiet);

            let launcher = Launcher::new(paths.logs_dir());
            let handle = launcher.launch(&spec, &[], None).await?;
            println!(
                "已启动 Minecraft (PID {})，日志: {}",
                handle.pid,
                handle.log.path.display()
            );
            let code = handle.wait().await?;
            println!("游戏退出，退出码 {code}");
        }

        Command::Verify {
            version,
            dir,
            source,
        } => {
            let paths = game_paths(dir);
            let installer = Installer::new(paths, sources(source))?;
            warm_up(&installer, cli.quiet).await;
            let results = installer.verify(&version).await?;
            let bad: Vec<_> = results.iter().filter(|(_, ok)| !ok).collect();
            println!(
                "校验 {}: {} 个文件, 损坏/缺失 {} 个",
                version,
                results.len(),
                bad.len()
            );
            for (path, _) in bad.iter().take(20) {
                println!("  ! {}", path.display());
            }
        }

        Command::Info {
            version,
            dir,
            source,
        } => {
            let paths = game_paths(dir);
            let installer = Installer::new(paths, sources(source))?;
            warm_up(&installer, cli.quiet).await;
            let details = installer.resolve(&version).await?;
            let (_, plan) = installer
                .plan_full(&version, &InstallOptions::default())
                .await?;
            println!(
                "版本: {} ({})",
                details.id,
                details.kind.clone().unwrap_or_default()
            );
            println!("主类: {}", details.main_class.clone().unwrap_or_default());
            println!(
                "继承: {}",
                details.inherits_from.clone().unwrap_or_else(|| "-".into())
            );
            println!(
                "资源索引: {}",
                details.assets.clone().unwrap_or_else(|| "-".into())
            );
            println!("需要 Java: {:?}", details.required_java_major());
            println!("库数量: {}", details.libraries.len());
            println!(
                "待下载: {} 个文件 / {:.1} MB",
                plan.tasks.len(),
                plan.total_bytes() as f64 / 1_048_576.0
            );
            println!("发布时间: {}", details.release_time.clone().unwrap_or_default());
        }

        Command::Java { json } => {
            let runtimes = java::detect();
            if json {
                let items: Vec<_> = runtimes
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "path": r.path.display().to_string(),
                            "version": r.version,
                            "major": r.major,
                            "vendor": r.vendor,
                            "arch": r.arch,
                            "source": r.source.label(),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else if runtimes.is_empty() {
                println!("未找到 Java");
            } else {
                for r in &runtimes {
                    println!("{:<9} {:<14} {}", format!("Java {}", r.major), r.version, r.display());
                }
            }
        }

        Command::Account { command } => match command {
            AccountCommand::Offline { name, dir } => {
                let paths = game_paths(dir);
                let path = AccountStore::default_path(paths.root());
                let mut store = AccountStore::load(&path)?;
                let account = Account::offline(&name);
                println!("离线账户: {} ({})", account.username, account.uuid);
                store.upsert(account);
                store.save(&path)?;
                println!("已保存到 {}", path.display());
            }
            AccountCommand::Ms { dir, client_id } => {
                let paths = game_paths(dir);
                let path = AccountStore::default_path(paths.root());
                let client = HttpClient::new(SourcePool::official())?;
                let auth = match client_id {
                    Some(id) => MicrosoftAuth::with_client_id(client, id),
                    None => MicrosoftAuth::new(client),
                };
                let account = auth
                    .login(|flow| {
                        println!(
                            "\n请在浏览器打开 {} 并输入代码: {}\n",
                            flow.verification_uri, flow.user_code
                        );
                    })
                    .await?;
                println!("登录成功: {} ({})", account.username, account.uuid);
                let mut store = AccountStore::load(&path)?;
                store.upsert(account);
                store.save(&path)?;
                println!("已保存到 {}", path.display());
            }
            AccountCommand::List { dir } => {
                let paths = game_paths(dir);
                let store = AccountStore::load(&AccountStore::default_path(paths.root()))?;
                if store.accounts.is_empty() {
                    println!("没有已保存的账户");
                }
                for account in &store.accounts {
                    let mark = if store.selected.as_deref() == Some(account.uuid.as_str()) {
                        "*"
                    } else {
                        " "
                    };
                    println!(
                        "{mark} {:<16} {:<34} {:?}",
                        account.username, account.uuid, account.kind
                    );
                }
            }
        },

        Command::Mods { command } => match command {
            ModsCommand::Search {
                query,
                source,
                version,
                loader,
                limit,
                api_key,
            } => {
                let client = HttpClient::new(SourcePool::official())?;
                let mut q = SearchQuery::new(query).with_limit(limit);
                q.game_version = version;
                q.loader = loader.map(LoaderArg::to_loader);

                let results = match source {
                    ModSourceArg::Modrinth => Modrinth::new(client).search(&q).await?,
                    ModSourceArg::Curseforge => {
                        CurseForge::new(client, api_key.or_else(curseforge_key).unwrap_or_default())
                            .search(&q)
                            .await?
                    }
                };

                println!("共 {} 个结果:\n", results.total);
                for project in &results.projects {
                    println!(
                        "{:<32} {:<24} {:>10}  {}",
                        project.name,
                        project.slug,
                        project.downloads,
                        project.summary.chars().take(60).collect::<String>()
                    );
                }
            }
            ModsCommand::Download {
                project,
                version,
                source,
                loader,
                dir,
                api_key,
            } => {
                let paths = game_paths(dir);
                let client = HttpClient::new(SourcePool::official())?;
                let loader = loader.map(LoaderArg::to_loader);

                let file = match source {
                    ModSourceArg::Modrinth => Modrinth::new(client)
                        .versions(&project, Some(&version), loader)
                        .await?
                        .into_iter()
                        .next(),
                    ModSourceArg::Curseforge => {
                        CurseForge::new(client, api_key.or_else(curseforge_key).unwrap_or_default())
                            .files(&project, Some(&version), loader)
                            .await?
                            .into_iter()
                            .next()
                    }
                };

                let Some(file) = file else {
                    bail!("没有找到与 {version} 兼容的 Mod 文件");
                };

                println!("选中文件: {} ({})", file.file_name, file.display_name);
                let Some(url) = file.download_url.clone() else {
                    println!(
                        "该文件作者关闭了第三方下载，请在浏览器中手动下载: {}",
                        if matches!(source, ModSourceArg::Curseforge) {
                            format!("https://www.curseforge.com/minecraft/mc-mods/{project}")
                        } else {
                            format!("https://modrinth.com/mod/{project}")
                        }
                    );
                    return Ok(());
                };

                let dest = paths.mods_dir().join(&file.file_name);
                let client = HttpClient::new(SourcePool::official())?;
                let downloader = Downloader::new(client.client().clone(), SourcePool::official());
                let task = DownloadTask::new(url, dest.clone()).label(file.file_name.clone());
                let summary = downloader.run(vec![task], progress_sink(cli.quiet)).await?;
                println!(
                    "已下载到 {} （下载 {}，跳过 {}）",
                    dest.display(),
                    summary.downloaded,
                    summary.skipped
                );
            }
        },
    }

    Ok(())
}

fn curseforge_key() -> Option<String> {
    std::env::var("PCL_CURSEFORGE_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
}

/// Windows consoles default to the OEM code page (GBK on zh-CN), which mangles
/// the Chinese output this CLI prints.
#[cfg(windows)]
fn enable_utf8_console() {
    const CP_UTF8: u32 = 65001;
    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleOutputCP(code_page: u32) -> i32;
        fn SetConsoleCP(code_page: u32) -> i32;
    }
    // SAFETY: plain Win32 calls with constant arguments.
    unsafe {
        SetConsoleOutputCP(CP_UTF8);
        SetConsoleCP(CP_UTF8);
    }
}

#[cfg(not(windows))]
fn enable_utf8_console() {}

/// Warm up the mirror pool and print the measured latencies.
async fn warm_up(installer: &Installer, quiet: bool) {
    let report = installer.warm_up().await;
    if quiet {
        return;
    }
    let parts: Vec<String> = report
        .iter()
        .map(|(name, latency)| match latency {
            Some(d) => format!("{name} {}ms", d.as_millis()),
            None => format!("{name} 不可用"),
        })
        .collect();
    println!("线路测速: {}", parts.join(" | "));
}

/// 启动前的正版购买劝导与赞助劝导。
///
/// PCL 的使用指南要求：实现了 Minecraft 启动功能的衍生作品，应给出与
/// `McLaunchPrecheck` 一致的劝导。离线启动给出提示；微软账户启动视为已持有正版。
fn print_precheck(is_offline: bool, quiet: bool) {
    if quiet || !is_offline {
        return;
    }
    println!(
        r#"
+--------------------------------------------------------------+
|  提示：你正在以离线模式启动 Minecraft
|
|  如果你还没有购买 Minecraft，请支持正版：
|    https://www.minecraft.net/zh-hans/store/minecraft-java-bedrock-edition-pc
|  本体与资源均由官方源 / 授权 CDN 下载，PCL-RS 不附带游戏文件。
|
|  PCL 原作者：龙腾猫跃   https://github.com/Meloong-Git/PCL
|  欢迎赞助支持 PCL：https://afdian.com/a/LTCat
|  PCL-RS 是第三方独立实现，与 PCL 官方无关。
+--------------------------------------------------------------+
"#
    );
}

/// Pick the account to launch with: explicit username > `--ms` saved account >
/// saved selection > environment username.
async fn resolve_account(paths: &GamePaths, username: Option<String>, ms: bool) -> Result<Account> {
    if let Some(name) = username {
        return Ok(Account::offline(&name));
    }

    let store_path = AccountStore::default_path(paths.root());
    let store = AccountStore::load(&store_path)?;

    if ms {
        let account = store
            .accounts
            .iter()
            .find(|a| !a.is_offline())
            .cloned()
            .context("没有已保存的微软账户，请先运行 `pcl account ms`")?;
        if account.is_expired() && account.refresh_token.is_some() {
            println!("Token 已过期，正在刷新...");
            let client = HttpClient::new(SourcePool::official())?;
            let auth = MicrosoftAuth::new(client);
            let token = auth.refresh(&account).await?;
            return auth.authenticate(&token).await;
        }
        return Ok(account);
    }

    if let Some(account) = store.selected_account() {
        return Ok(account.clone());
    }

    let fallback = std::env::var("USERNAME").unwrap_or_else(|_| "Player".to_string());
    Ok(Account::offline(&fallback))
}
