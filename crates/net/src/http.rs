//! A small HTTP client with per-request mirror fallback, retries and warm-up.

use crate::mirror::SourcePool;
use anyhow::{anyhow, Context, Result};
use serde::de::DeserializeOwned;
use std::time::Duration;

/// Sent on every request; Mojang asks launchers to identify themselves.
pub const USER_AGENT: &str = concat!(
    "PCL-RS/",
    env!("CARGO_PKG_VERSION"),
    " (Minecraft launcher; +https://github.com/mc-githu/pcl-rs)"
);

#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
    sources: SourcePool,
    attempts: u32,
}

impl HttpClient {
    pub fn new(sources: SourcePool) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            // A dead mirror must fail fast, not stall the whole install.
            .connect_timeout(Duration::from_secs(10))
            // Per-read timeout, so a stalling CDN is caught mid-body.
            .read_timeout(Duration::from_secs(45))
            .timeout(Duration::from_secs(600))
            .pool_max_idle_per_host(16)
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            client,
            sources,
            attempts: 3,
        })
    }

    pub fn sources(&self) -> &SourcePool {
        &self.sources
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn attempts(mut self, attempts: u32) -> Self {
        self.attempts = attempts.max(1);
        self
    }

    /// Probe every endpoint once, in parallel, and seed the latency table.
    ///
    /// Without this the first request of a process always pays the primary
    /// endpoint's latency, even when another mirror would answer far faster.
    pub async fn warm_up(&self, probe_url: &str) -> Vec<(String, Option<Duration>)> {
        let probes = self.sources.endpoints().iter().map(|endpoint| {
            let url = endpoint.rewrite(probe_url);
            let name = endpoint.name.clone();
            async move {
                let started = std::time::Instant::now();
                let result = self
                    .client
                    .head(&url)
                    .timeout(Duration::from_secs(8))
                    .send()
                    .await;
                (name, result, started.elapsed())
            }
        });

        let results = futures::future::join_all(probes).await;
        let mut report = Vec::new();
        for (name, result, elapsed) in results {
            match result {
                Ok(response) => {
                    // HEAD may legitimately be rejected; the round trip still
                    // tells us how fast this endpoint is.
                    tracing::debug!("warm-up {name}: HTTP {}", response.status());
                    self.sources.note_success(&name, elapsed);
                    report.push((name, Some(elapsed)));
                }
                Err(e) => {
                    tracing::debug!("warm-up probe {name} failed: {e}");
                    self.sources.note_failure(&name);
                    report.push((name, None));
                }
            }
        }
        if !report.is_empty() {
            let summary = report
                .iter()
                .map(|(name, latency)| match latency {
                    Some(d) => format!("{name} {}ms", d.as_millis()),
                    None => format!("{name} 不可用"),
                })
                .collect::<Vec<_>>()
                .join(", ");
            tracing::info!("线路测速 -> {summary}");
        }
        report
    }

    /// Fetch once from the single best URL, no mirror fallback.
    pub async fn get_bytes_direct(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .with_context(|| format!("GET {url}"))?;
        Ok(resp.bytes().await?.to_vec())
    }

    /// Fetch a small resource, trying mirrors in latency order with retries.
    pub async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let mut last_err: Option<anyhow::Error> = None;

        for attempt in 0..self.attempts {
            if attempt > 0 {
                tokio::time::sleep(backoff(attempt)).await;
            }
            for (name, candidate) in self.sources.candidates(url) {
                let started = std::time::Instant::now();
                match self.get_bytes_direct(&candidate).await {
                    Ok(bytes) => {
                        self.sources.note_success(&name, started.elapsed());
                        tracing::debug!(
                            "GET {} via {} -> {} bytes in {:?}",
                            url,
                            name,
                            bytes.len(),
                            started.elapsed()
                        );
                        return Ok(bytes);
                    }
                    Err(e) => {
                        self.sources.note_failure(&name);
                        tracing::warn!("GET {candidate} via {name} failed: {e:#}");
                        last_err = Some(e);
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow!("no mirror available for {url}")))
            .with_context(|| format!("all sources failed for {url}"))
    }

    pub async fn get_text(&self, url: &str) -> Result<String> {
        let bytes = self.get_bytes(url).await?;
        String::from_utf8(bytes).with_context(|| format!("{url} is not valid UTF-8"))
    }

    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let bytes = self.get_bytes(url).await?;
        serde_json::from_slice(&bytes).with_context(|| format!("decoding JSON from {url}"))
    }

    /// Fetch JSON with custom headers (Modrinth / CurseForge APIs).
    pub async fn get_json_with_headers<T: DeserializeOwned>(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<T> {
        let mut request = self.client.get(url);
        for (k, v) in headers {
            request = request.header(*k, *v);
        }
        let resp = request
            .send()
            .await
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .with_context(|| format!("GET {url}"))?;
        Ok(resp.json::<T>().await?)
    }

    // -----------------------------------------------------------------------
    // Raw helpers for authentication flows.
    //
    // Auth requests deliberately bypass the mirror pool: credentials must never
    // be proxied through a third-party CDN.
    // -----------------------------------------------------------------------

    /// POST a JSON body, returning `(status, decoded JSON)` even on 4xx so OAuth
    /// error codes stay readable.
    pub async fn post_json_value(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<(reqwest::StatusCode, serde_json::Value)> {
        let resp = self
            .client
            .post(url)
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        decode(resp).await
    }

    /// POST an `application/x-www-form-urlencoded` body.
    pub async fn post_form_value(
        &self,
        url: &str,
        form: &[(&str, String)],
    ) -> Result<(reqwest::StatusCode, serde_json::Value)> {
        let resp = self
            .client
            .post(url)
            .form(form)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        decode(resp).await
    }

    /// GET with an optional bearer token.
    pub async fn get_json_value(
        &self,
        url: &str,
        bearer: Option<&str>,
    ) -> Result<(reqwest::StatusCode, serde_json::Value)> {
        let mut request = self.client.get(url);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let resp = request.send().await.with_context(|| format!("GET {url}"))?;
        decode(resp).await
    }
}

async fn decode(resp: reqwest::Response) -> Result<(reqwest::StatusCode, serde_json::Value)> {
    let status = resp.status();
    let text = resp.text().await?;
    let value = serde_json::from_str(&text).unwrap_or_else(|_| {
        serde_json::json!({ "raw": text.chars().take(400).collect::<String>() })
    });
    Ok((status, value))
}

pub(crate) fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(400u64 * attempt as u64)
}
