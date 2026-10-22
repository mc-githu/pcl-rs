//! Concurrent, resumable, checksum-verified downloader.
//!
//! Guarantees that matter for a launcher:
//! * never trust a mirror — check SHA-1 before a file lands in place,
//! * never lose progress — partial data lives in `<name>.part` and resumes with a
//!   `Range` request,
//! * never stall the UI — aggregated progress is pushed at a fixed cadence.

use crate::http::backoff;
use crate::mirror::SourcePool;
use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use sha1::{Digest, Sha1};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

/// One file to download.
#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub dest: PathBuf,
    /// Expected SHA-1 (lowercase hex). When present the file is rejected on mismatch.
    pub sha1: Option<String>,
    /// Expected size in bytes, used for cheap "already installed" checks.
    pub size: Option<u64>,
    /// Short description such as `library org.lwjgl:lwjgl`.
    pub label: String,
}

impl DownloadTask {
    pub fn new(url: impl Into<String>, dest: impl Into<PathBuf>) -> Self {
        let url = url.into();
        Self {
            label: url.rsplit('/').next().unwrap_or(&url).to_string(),
            url,
            dest: dest.into(),
            sha1: None,
            size: None,
        }
    }

    pub fn sha1(mut self, sha1: impl Into<String>) -> Self {
        self.sha1 = Some(sha1.into());
        self
    }

    pub fn size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
}

/// Aggregated progress snapshot delivered to the UI.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub failed: u64,
    pub current: Option<String>,
    pub finished: bool,
}

impl Progress {
    /// 0.0..=1.0, falling back to per-file progress when sizes are unknown.
    pub fn fraction(&self) -> f32 {
        if self.bytes_total > 0 {
            (self.bytes_done as f32 / self.bytes_total as f32).clamp(0.0, 1.0)
        } else if self.files_total > 0 {
            (self.files_done as f32 / self.files_total as f32).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

pub type ProgressSink = Arc<dyn Fn(Progress) + Send + Sync>;

#[derive(Debug, Clone, Default)]
pub struct DownloadSummary {
    pub downloaded: u64,
    pub skipped: u64,
    pub failed: u64,
    pub bytes: u64,
    pub elapsed: Duration,
}

#[derive(Default)]
struct Counters {
    files_done: AtomicU64,
    bytes_done: AtomicU64,
    failed: AtomicU64,
    current: Mutex<String>,
}

impl Counters {
    fn snapshot(&self, files_total: u64, bytes_total: u64, finished: bool) -> Progress {
        let current = self.current.lock().map(|c| c.clone()).ok();
        Progress {
            files_done: self.files_done.load(Ordering::Relaxed),
            files_total,
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            bytes_total,
            failed: self.failed.load(Ordering::Relaxed),
            current: current.filter(|c| !c.is_empty()),
            finished,
        }
    }
}

#[derive(Clone)]
pub struct Downloader {
    client: reqwest::Client,
    sources: SourcePool,
    concurrency: usize,
    attempts: u32,
    strict: bool,
}

impl Downloader {
    pub fn new(client: reqwest::Client, sources: SourcePool) -> Self {
        Self {
            client,
            sources,
            concurrency: 16,
            attempts: 3,
            strict: false,
        }
    }

    pub fn concurrency(mut self, n: usize) -> Self {
        self.concurrency = n.clamp(1, 64);
        self
    }

    pub fn attempts(mut self, n: u32) -> Self {
        self.attempts = n.max(1);
        self
    }

    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Download every task, skipping files already present and valid.
    pub async fn run(
        &self,
        tasks: Vec<DownloadTask>,
        sink: Option<ProgressSink>,
    ) -> Result<DownloadSummary> {
        let started = Instant::now();
        let files_total = tasks.len() as u64;
        let bytes_total: u64 = tasks.iter().filter_map(|t| t.size).sum();
        let counters = Arc::new(Counters::default());

        let finished = Arc::new(AtomicBool::new(false));
        let ticker = sink.as_ref().map(|sink| {
            let sink = sink.clone();
            let counters = counters.clone();
            let finished = finished.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(150));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    interval.tick().await;
                    let done = finished.load(Ordering::Relaxed);
                    sink(counters.snapshot(files_total, bytes_total, done));
                    if done {
                        break;
                    }
                }
            })
        });

        let results: Vec<Result<Outcome>> = futures::stream::iter(tasks.into_iter().map(|task| {
            let counters = &counters;
            async move { self.process(task, counters).await }
        }))
        .buffer_unordered(self.concurrency)
        .collect()
        .await;

        finished.store(true, Ordering::Relaxed);
        if let Some(ticker) = ticker {
            let _ = ticker.await;
        } else if let Some(sink) = &sink {
            sink(counters.snapshot(files_total, bytes_total, true));
        }

        let mut summary = DownloadSummary {
            elapsed: started.elapsed(),
            ..Default::default()
        };
        let mut failures: Vec<String> = Vec::new();
        for result in results {
            match result {
                Ok(Outcome::Downloaded(bytes)) => {
                    summary.downloaded += 1;
                    summary.bytes += bytes;
                }
                Ok(Outcome::Skipped) => summary.skipped += 1,
                Err(e) => {
                    summary.failed += 1;
                    failures.push(format!("{e:#}"));
                }
            }
        }

        if !failures.is_empty() {
            let shown = failures.iter().take(5).cloned().collect::<Vec<_>>().join("\n  ");
            return Err(anyhow!(
                "{} of {} files failed to download:\n  {}",
                summary.failed,
                files_total,
                shown
            ));
        }

        Ok(summary)
    }

    async fn process(&self, task: DownloadTask, counters: &Counters) -> Result<Outcome> {
        if let Some(parent) = task.dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("creating {}", parent.display()))?;
        }

        if self.is_up_to_date(&task).await {
            counters.files_done.fetch_add(1, Ordering::Relaxed);
            if let Some(size) = task.size {
                counters.bytes_done.fetch_add(size, Ordering::Relaxed);
            }
            return Ok(Outcome::Skipped);
        }

        if let Ok(mut current) = counters.current.lock() {
            *current = task.label.clone();
        }

        let bytes = self.fetch_with_retries(&task, counters).await;

        match &bytes {
            Ok(_) => {
                counters.files_done.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                counters.failed.fetch_add(1, Ordering::Relaxed);
                counters.files_done.fetch_add(1, Ordering::Relaxed);
            }
        }

        bytes.map(Outcome::Downloaded)
    }

    async fn is_up_to_date(&self, task: &DownloadTask) -> bool {
        let Ok(meta) = tokio::fs::metadata(&task.dest).await else {
            return false;
        };
        if meta.len() == 0 && task.size.unwrap_or(0) > 0 {
            return false;
        }

        // Fast path: a matching size is trusted unless strict mode is on.
        if !self.strict {
            if let Some(size) = task.size {
                return meta.len() == size;
            }
        }

        match &task.sha1 {
            Some(expected) => verify_sha1(&task.dest, expected).await.unwrap_or(false),
            None => meta.len() > 0,
        }
    }

    async fn fetch_with_retries(&self, task: &DownloadTask, counters: &Counters) -> Result<u64> {
        let mut last_err: Option<anyhow::Error> = None;

        for attempt in 0..self.attempts {
            if attempt > 0 {
                tokio::time::sleep(backoff(attempt)).await;
            }
            for (name, url) in self.sources.candidates(&task.url) {
                let started = Instant::now();
                match self.fetch_to(&url, task, counters).await {
                    Ok(bytes) => {
                        self.sources.note_success(&name, started.elapsed());
                        return Ok(bytes);
                    }
                    Err(e) => {
                        self.sources.note_failure(&name);
                        tracing::warn!("download {} via {} failed: {e:#}", task.label, name);
                        last_err = Some(e);
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow!("no source for {}", task.url)))
            .with_context(|| format!("downloading {}", task.label))
    }

    async fn fetch_to(&self, url: &str, task: &DownloadTask, counters: &Counters) -> Result<u64> {
        let part = part_path(&task.dest);
        let existing = tokio::fs::metadata(&part)
            .await
            .map(|m| m.len())
            .unwrap_or(0);

        let mut request = self.client.get(url);
        if existing > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
        }

        let response = request
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;

        let status = response.status();
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(anyhow!("range not satisfiable for {url}, restarting"));
        }
        if !status.is_success() {
            return Err(anyhow!("HTTP {status} for {url}"));
        }

        let resuming = status == reqwest::StatusCode::PARTIAL_CONTENT;
        let mut file = if resuming {
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(&part)
                .await
                .with_context(|| format!("opening {}", part.display()))?
        } else {
            tokio::fs::File::create(&part)
                .await
                .with_context(|| format!("creating {}", part.display()))?
        };

        let mut written = 0u64;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("reading body of {url}"))?;
            file.write_all(&chunk)
                .await
                .with_context(|| format!("writing {}", part.display()))?;
            written += chunk.len() as u64;
            counters
                .bytes_done
                .fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        file.flush().await?;
        drop(file);

        if let Some(expected) = &task.sha1 {
            if !verify_sha1(&part, expected).await? {
                let _ = tokio::fs::remove_file(&part).await;
                return Err(anyhow!(
                    "checksum mismatch for {} (expected {expected})",
                    task.label
                ));
            }
        } else if let Some(size) = task.size {
            let len = tokio::fs::metadata(&part).await?.len();
            if len != size {
                let _ = tokio::fs::remove_file(&part).await;
                return Err(anyhow!(
                    "size mismatch for {} (expected {size}, got {len})",
                    task.label
                ));
            }
        }

        replace_file(&part, &task.dest).await?;
        Ok(written)
    }

    /// Hash every file of a finished install (the "verify" action).
    pub async fn verify(&self, tasks: &[DownloadTask]) -> Result<Vec<(PathBuf, bool)>> {
        let mut out = Vec::with_capacity(tasks.len());
        for task in tasks {
            let exists = tokio::fs::metadata(&task.dest).await.is_ok();
            let ok = if !exists {
                false
            } else {
                match &task.sha1 {
                    Some(expected) => verify_sha1(&task.dest, expected).await.unwrap_or(false),
                    None => true,
                }
            };
            out.push((task.dest.clone(), ok));
        }
        Ok(out)
    }
}

enum Outcome {
    Downloaded(u64),
    Skipped,
}

fn part_path(dest: &Path) -> PathBuf {
    let file_name = dest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    dest.with_file_name(format!("{file_name}.part"))
}

/// Rename, replacing an existing destination (Windows refuses rename onto a file).
async fn replace_file(from: &Path, to: &Path) -> Result<()> {
    if tokio::fs::rename(from, to).await.is_ok() {
        return Ok(());
    }
    let _ = tokio::fs::remove_file(to).await;
    tokio::fs::rename(from, to)
        .await
        .with_context(|| format!("moving {} -> {}", from.display(), to.display()))
}

/// Lowercase hex SHA-1 of an in-memory buffer.
pub fn sha1_file_from_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Lowercase hex SHA-1 of a file (blocking).
pub fn sha1_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

async fn verify_sha1(path: &Path, expected: &str) -> Result<bool> {
    let path = path.to_path_buf();
    let expected = expected.to_ascii_lowercase();
    let hash = tokio::task::spawn_blocking(move || sha1_file(&path))
        .await
        .context("hashing task panicked")??;
    Ok(hash == expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_path_appends_suffix() {
        let p = part_path(Path::new(r"C:\mc\assets\objects\ab\abcd"));
        assert_eq!(p, PathBuf::from(r"C:\mc\assets\objects\ab\abcd.part"));
    }

    #[test]
    fn progress_fraction() {
        let mut p = Progress::default();
        assert_eq!(p.fraction(), 1.0);
        p.files_total = 4;
        p.files_done = 1;
        assert!((p.fraction() - 0.25).abs() < 1e-6);
        p.bytes_total = 100;
        p.bytes_done = 50;
        assert!((p.fraction() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn sha1_of_known_content() {
        let dir = std::env::temp_dir().join("pcl-rs-sha1-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(
            sha1_file(&file).unwrap(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            sha1_file_from_bytes(b"abc"),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
