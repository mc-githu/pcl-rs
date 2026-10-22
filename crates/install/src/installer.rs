//! The installer: resolves a version, plans every file it needs and materialises
//! a runnable installation.
//!
//! The plan/execute split matters: the GUI can show "N files / M bytes to
//! download" before anything is touched, and the CLI can print the same plan with
//! `--dry-run`.

use anyhow::{anyhow, bail, Context, Result};
use pcl_core::{
    rules::{evaluate, RuleContext},
    AssetIndex, DownloadInfo, GamePaths, Library, MavenName, VersionDetails, VersionManifest,
    VersionSummary,
};
use pcl_net::{DownloadTask, Downloader, HttpClient, ProgressSink, SourcePool, MANIFEST_URL};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::assets;
use crate::natives::extract_natives;

/// Legacy maven repository used when a library has no `downloads` block.
const LIBRARY_BASE: &str = "https://libraries.minecraft.net/";

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub download_client: bool,
    pub download_assets: bool,
    /// Re-hash every existing file instead of trusting matching sizes.
    pub verify_existing: bool,
    pub concurrency: usize,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            download_client: true,
            download_assets: true,
            verify_existing: false,
            concurrency: 16,
        }
    }
}

/// A version that is ready to launch.
#[derive(Debug, Clone)]
pub struct InstalledVersion {
    pub id: String,
    pub details: VersionDetails,
    pub version_json: PathBuf,
    pub client_jar: PathBuf,
    /// Libraries then the client jar, in classpath order.
    pub classpath: Vec<PathBuf>,
    pub natives_dir: PathBuf,
    pub asset_index: Option<String>,
    pub asset_index_file: Option<PathBuf>,
    pub log_config: Option<PathBuf>,
    pub downloaded: u64,
    pub skipped: u64,
}

pub struct Installer {
    paths: GamePaths,
    http: HttpClient,
    downloader: Downloader,
    manifest_cache: Mutex<Option<Arc<VersionManifest>>>,
    options: InstallOptions,
}

impl Installer {
    pub fn new(paths: GamePaths, sources: SourcePool) -> Result<Self> {
        let http = HttpClient::new(sources.clone())?;
        let downloader = Downloader::new(http.client().clone(), sources);
        Ok(Self {
            paths,
            http,
            downloader,
            manifest_cache: Mutex::new(None),
            options: InstallOptions::default(),
        })
    }

    pub fn with_options(mut self, options: InstallOptions) -> Self {
        self.downloader = self.downloader.concurrency(options.concurrency);
        self.options = options;
        self
    }

    pub fn paths(&self) -> &GamePaths {
        &self.paths
    }

    pub fn http(&self) -> &HttpClient {
        &self.http
    }

    pub fn options(&self) -> &InstallOptions {
        &self.options
    }

    /// Probe every mirror once so the fastest one is used from the first byte.
    pub async fn warm_up(&self) -> Vec<(String, Option<std::time::Duration>)> {
        self.http.warm_up(MANIFEST_URL).await
    }

    /// Fetch (and memoise) `version_manifest_v2.json`.
    pub async fn manifest(&self) -> Result<Arc<VersionManifest>> {
        if let Some(cached) = self.manifest_cache.lock().ok().and_then(|c| c.clone()) {
            return Ok(cached);
        }
        let manifest: VersionManifest = self
            .http
            .get_json(MANIFEST_URL)
            .await
            .context("fetching the Minecraft version manifest")?;
        tracing::info!(
            "version manifest: {} versions, latest release {}",
            manifest.versions.len(),
            manifest.latest.release
        );
        let manifest = Arc::new(manifest);
        if let Ok(mut cache) = self.manifest_cache.lock() {
            *cache = Some(manifest.clone());
        }
        Ok(manifest)
    }

    pub async fn list_versions(&self) -> Result<Vec<VersionSummary>> {
        Ok(self.manifest().await?.versions.clone())
    }

    async fn fetch_details(&self, url: &str) -> Result<VersionDetails> {
        self.http
            .get_json::<VersionDetails>(url)
            .await
            .with_context(|| format!("fetching version JSON {url}"))
    }

    /// Load a version and flatten its `inheritsFrom` chain (Forge/Fabric/Quilt).
    pub async fn resolve(&self, id: &str) -> Result<VersionDetails> {
        let manifest = self.manifest().await?;
        let summary = manifest
            .versions
            .iter()
            .find(|v| v.id == id)
            .ok_or_else(|| anyhow!("unknown version `{id}` (not in the Mojang manifest)"))?;

        let mut chain = vec![self.fetch_details(&summary.url).await?];
        let mut seen = vec![id.to_string()];

        loop {
            let parent_id = match chain.last().and_then(|d| d.inherits_from.clone()) {
                Some(p) if !p.is_empty() => p,
                _ => break,
            };
            if seen.contains(&parent_id) {
                bail!("circular `inheritsFrom` chain involving `{parent_id}`");
            }
            if chain.len() >= 8 {
                bail!("`inheritsFrom` chain for `{id}` is too deep");
            }
            let parent_summary = manifest
                .versions
                .iter()
                .find(|v| v.id == parent_id)
                .ok_or_else(|| anyhow!("parent version `{parent_id}` is not in the manifest"))?;
            chain.push(self.fetch_details(&parent_summary.url).await?);
            seen.push(parent_id);
        }

        let mut merged = chain.pop().expect("chain is never empty");
        while let Some(child) = chain.pop() {
            merged = merged.merge_child(child);
        }
        if merged.id != id {
            merged.id = id.to_string();
        }
        Ok(merged)
    }

    /// Build the download plan for an already resolved version.
    pub fn plan(&self, details: &VersionDetails, options: &InstallOptions) -> Result<InstallPlan> {
        self.plan_with_context(details, options, &RuleContext::host())
    }

    pub fn plan_with_context(
        &self,
        details: &VersionDetails,
        options: &InstallOptions,
        ctx: &RuleContext,
    ) -> Result<InstallPlan> {
        let mut plan = InstallPlan::default();

        // --- client jar -----------------------------------------------------
        let jar_id = details.jar_id().to_string();
        plan.client_jar = self.paths.version_jar(&jar_id);
        if options.download_client {
            if let Some(client) = details.client_download() {
                plan.tasks.push(download_task(
                    client,
                    plan.client_jar.clone(),
                    format!("client jar {jar_id}"),
                ));
            }
        }

        // --- libraries ------------------------------------------------------
        for lib in &details.libraries {
            if !evaluate(lib.rules.as_deref(), ctx) {
                continue;
            }
            let maven = match MavenName::parse(&lib.name) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("skipping library with bad coordinate `{}`: {e}", lib.name);
                    continue;
                }
            };

            let excludes = lib
                .extract
                .as_ref()
                .map(|e| e.exclude.clone())
                .unwrap_or_default();

            // Legacy `natives` map: {"windows": "natives-windows-${arch}"}
            let mapped_classifier = lib
                .natives
                .as_ref()
                .and_then(|n| n.get(&ctx.os_name))
                .map(|classifier| classifier.replace("${arch}", arch_bits(&ctx.os_arch)));

            if let Some(classifier) = mapped_classifier {
                let task = self.classifier_task(lib, &maven, &classifier);
                plan.natives.push(NativeJob {
                    jar: task.dest.clone(),
                    excludes: excludes.clone(),
                });
                plan.tasks.push(task);
            }

            let is_native_entry = maven
                .classifier
                .as_deref()
                .map(|c| c.starts_with("natives"))
                .unwrap_or(false);

            if is_native_entry && lib.natives.is_none() {
                // 1.13+: natives are ordinary classifier artifacts.
                let task = self.artifact_task(lib, &maven);
                plan.natives.push(NativeJob {
                    jar: task.dest.clone(),
                    excludes,
                });
                plan.tasks.push(task);
                continue;
            }

            // Ordinary jar, goes on the classpath.
            if lib.downloads.as_ref().and_then(|d| d.artifact.as_ref()).is_some()
                || maven.classifier.is_none()
            {
                let task = self.artifact_task(lib, &maven);
                plan.classpath.push(task.dest.clone());
                plan.tasks.push(task);
            }
        }

        // --- log4j configuration -------------------------------------------
        if options.download_assets {
            if let Some(logging) = details.logging.as_ref().and_then(|l| l.client.as_ref()) {
                let dest = self.paths.log_config(&logging.file.id);
                plan.log_config = Some(dest.clone());
                let mut task =
                    DownloadTask::new(logging.file.url.clone(), dest).label(format!(
                        "log config {}",
                        logging.file.id
                    ));
                task.sha1 = logging.file.sha1.clone();
                task.size = logging.file.size;
                plan.tasks.push(task);
            }
        }

        Ok(plan)
    }

    /// Fetch and cache the asset index, then append one task per object.
    async fn append_asset_tasks(
        &self,
        details: &VersionDetails,
        plan: &mut InstallPlan,
    ) -> Result<Option<AssetIndex>> {
        let Some(index_ref) = &details.asset_index else {
            return Ok(None);
        };

        let index_file = self.paths.index_file(&index_ref.id);
        let bytes = self
            .http
            .get_bytes(&index_ref.url)
            .await
            .with_context(|| format!("fetching asset index {}", index_ref.id))?;

        if let Some(expected) = &index_ref.sha1 {
            let actual = pcl_net::sha1_file_from_bytes(&bytes);
            if !actual.eq_ignore_ascii_case(expected) {
                bail!(
                    "asset index {} is corrupt (sha1 {actual} != {expected})",
                    index_ref.id
                );
            }
        }

        if let Some(parent) = index_file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&index_file, &bytes).await?;

        let index: AssetIndex = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing asset index {}", index_ref.id))?;

        plan.asset_index = Some(index_ref.id.clone());
        plan.asset_index_file = Some(index_file);

        if self.options.download_assets {
            for (name, object) in &index.objects {
                plan.tasks.push(
                    DownloadTask::new(assets::object_url(&object.hash), self.paths.object_file(&object.hash))
                        .sha1(object.hash.clone())
                        .size(object.size)
                        .label(format!("asset {name}")),
                );
            }
        }

        Ok(Some(index))
    }

    /// Resolve, plan and include asset objects without downloading them.
    pub async fn plan_full(
        &self,
        id: &str,
        options: &InstallOptions,
    ) -> Result<(VersionDetails, InstallPlan)> {
        let details = self.resolve(id).await?;
        let mut plan = self.plan(&details, options)?;
        self.append_asset_tasks(&details, &mut plan).await?;
        plan.dedupe();
        Ok((details, plan))
    }

    /// Download everything and finish the installation.
    pub async fn install(
        &self,
        id: &str,
        options: &InstallOptions,
        sink: Option<ProgressSink>,
    ) -> Result<InstalledVersion> {
        let details = self.resolve(id).await?;
        self.paths.ensure_base_dirs()?;

        let version_dir = self.paths.version_dir(&details.id);
        tokio::fs::create_dir_all(&version_dir).await?;
        let version_json = self.paths.version_json(&details.id);

        // Persist the merged profile first: a failed download leaves a version
        // that `pcl install` can repair instead of an invisible half-state.
        let json = serde_json::to_vec_pretty(&details)?;
        tokio::fs::write(&version_json, &json).await?;

        let mut plan = self.plan(&details, options)?;
        let index = self.append_asset_tasks(&details, &mut plan).await?;
        plan.dedupe();

        let client_jar = plan.client_jar.clone();
        let mut classpath = plan.classpath.clone();
        let natives_dir = self.paths.natives_dir(&details.id);
        let log_config = plan.log_config.clone();
        let asset_index = plan.asset_index.clone();
        let asset_index_file = plan.asset_index_file.clone();
        let native_jobs = plan.natives.clone();
        let task_count = plan.tasks.len();

        tracing::info!("installing {}: {} files to fetch", details.id, task_count);
        let downloader = self.downloader.clone().strict(options.verify_existing);
        let summary = downloader.run(plan.tasks, sink).await?;

        // --- natives --------------------------------------------------------
        if !native_jobs.is_empty() {
            let jobs = native_jobs;
            let dest = natives_dir.clone();
            tokio::task::spawn_blocking(move || -> Result<()> {
                std::fs::create_dir_all(&dest)?;
                for job in jobs {
                    if !job.jar.is_file() {
                        tracing::warn!("native jar {} is missing", job.jar.display());
                        continue;
                    }
                    extract_natives(&job.jar, &dest, &job.excludes)?;
                }
                Ok(())
            })
            .await
            .context("native extraction task panicked")??;
        }

        // --- legacy virtual assets -----------------------------------------
        if let Some(index) = &index {
            if index.virtual_ || index.map_to_resources {
                let paths = self.paths.clone();
                let index = index.clone();
                let asset_id = asset_index.clone().unwrap_or_default();
                tokio::task::spawn_blocking(move || {
                    assets::materialize_virtual(&paths, &index, &asset_id)
                })
                .await
                .context("virtual asset task panicked")??;
            }
        }

        classpath.push(client_jar.clone());

        Ok(InstalledVersion {
            id: details.id.clone(),
            details,
            version_json,
            client_jar,
            classpath,
            natives_dir,
            asset_index,
            asset_index_file,
            log_config,
            downloaded: summary.downloaded,
            skipped: summary.skipped,
        })
    }

    /// Re-hash every file of an installed version (the "verify integrity" action).
    pub async fn verify(&self, id: &str) -> Result<Vec<(PathBuf, bool)>> {
        let (_, plan) = self.plan_full(id, &self.options).await?;
        self.downloader.verify(&plan.tasks).await
    }

    fn artifact_task(&self, lib: &Library, maven: &MavenName) -> DownloadTask {
        let fallback_dest = self.paths.libraries_dir().join(maven.path());
        let ArtifactLike {
            dest,
            url,
            sha1,
            size,
        } = self.artifact_like(lib, maven, None, fallback_dest);
        let mut task = DownloadTask::new(url, dest).label(format!("library {}", lib.name));
        task.sha1 = sha1;
        task.size = size;
        task
    }

    fn classifier_task(&self, lib: &Library, maven: &MavenName, classifier: &str) -> DownloadTask {
        let mut native = maven.clone();
        native.classifier = Some(classifier.to_string());
        let fallback_dest = self.paths.libraries_dir().join(native.path());
        let ArtifactLike {
            dest,
            url,
            sha1,
            size,
        } = self.artifact_like(lib, maven, Some(classifier), fallback_dest);
        let mut task = DownloadTask::new(url, dest).label(format!("natives {}", lib.name));
        task.sha1 = sha1;
        task.size = size;
        task
    }

    fn artifact_like(
        &self,
        lib: &Library,
        maven: &MavenName,
        classifier: Option<&str>,
        fallback_dest: PathBuf,
    ) -> ArtifactLike {
        let artifact = match classifier {
            Some(name) => lib
                .downloads
                .as_ref()
                .and_then(|d| d.classifiers.as_ref())
                .and_then(|map| map.get(name)),
            None => lib.downloads.as_ref().and_then(|d| d.artifact.as_ref()),
        };

        match artifact {
            Some(a) => ArtifactLike {
                dest: a
                    .path
                    .as_ref()
                    .map(|p| self.paths.libraries_dir().join(p))
                    .unwrap_or(fallback_dest),
                url: a.url.clone(),
                sha1: a.sha1.clone(),
                size: a.size,
            },
            None => {
                let base = lib.url.clone().unwrap_or_else(|| LIBRARY_BASE.to_string());
                let mut coordinates = maven.clone();
                coordinates.classifier = classifier.map(|c| c.to_string());
                ArtifactLike {
                    dest: fallback_dest,
                    url: format!("{}/{}", base.trim_end_matches('/'), coordinates.path()),
                    sha1: None,
                    size: None,
                }
            }
        }
    }
}

#[derive(Default)]
pub struct InstallPlan {
    pub tasks: Vec<DownloadTask>,
    pub classpath: Vec<PathBuf>,
    pub natives: Vec<NativeJob>,
    pub client_jar: PathBuf,
    pub asset_index: Option<String>,
    pub asset_index_file: Option<PathBuf>,
    pub log_config: Option<PathBuf>,
}

impl InstallPlan {
    /// Several asset names can share one content-addressed object, and merged
    /// loader profiles can repeat a library; downloading the same destination
    /// twice in parallel would race, so keep the first task per path.
    pub fn dedupe(&mut self) {
        let mut seen = HashSet::new();
        self.tasks.retain(|task| seen.insert(task.dest.clone()));

        let mut seen_classpath = HashSet::new();
        self.classpath.retain(|path| seen_classpath.insert(path.clone()));

        let mut seen_natives = HashSet::new();
        self.natives.retain(|job| seen_natives.insert(job.jar.clone()));
    }

    pub fn total_bytes(&self) -> u64 {
        self.tasks.iter().filter_map(|t| t.size).sum()
    }
}

#[derive(Debug, Clone)]
pub struct NativeJob {
    pub jar: PathBuf,
    pub excludes: Vec<String>,
}

struct ArtifactLike {
    dest: PathBuf,
    url: String,
    sha1: Option<String>,
    size: Option<u64>,
}

fn download_task(info: &DownloadInfo, dest: PathBuf, label: String) -> DownloadTask {
    let mut task = DownloadTask::new(info.url.clone(), dest).label(label);
    task.sha1 = info.sha1.clone();
    task.size = info.size;
    task
}

fn arch_bits(os_arch: &str) -> &'static str {
    match os_arch {
        "x86" => "32",
        _ => "64",
    }
}
