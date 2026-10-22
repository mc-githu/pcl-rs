//! CDN mirror support with adaptive endpoint selection.
//!
//! Mojang serves metadata and files from a handful of fixed hosts. In mainland
//! China those are commonly slow or unreachable, so launchers rewrite them onto a
//! mirror that follows the exact same path layout (BMCLAPI).
//!
//! A [`SourcePool`] holds several [`Endpoint`]s. Each endpoint rewrites a URL and
//! the downloader walks the pool until one succeeds. The pool also records
//! latency and failures per endpoint, so the fastest mirror is tried first and a
//! mirror that keeps timing out is demoted instead of costing every request a
//! connect timeout.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Canonical (official) version manifest used as the entry point.
pub const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// BMCLAPI, the community CDN that mirrors Mojang content path-for-path.
pub const BMCLAPI_BASE: &str = "https://bmclapi2.bangbang93.com";

const MOJANG_HOSTS: &[&str] = &[
    "https://piston-meta.mojang.com",
    "https://launchermeta.mojang.com",
    "https://launcher.mojang.com",
    "https://piston-data.mojang.com",
    "https://resources.download.minecraft.net",
    "https://libraries.minecraft.net",
    "https://files.minecraftforge.net/maven",
    "https://maven.minecraftforge.net",
    "https://maven.fabricmc.net",
    "https://maven.quiltmc.org/repository/release",
    "https://meta.fabricmc.net",
    "https://meta.quiltmc.org",
    "https://api.minecraftservices.com",
];

/// BMCLAPI path layout differs for two hosts; everything else is 1:1.
const BMCLAPI_RULES: &[(&str, &str)] = &[
    ("https://piston-meta.mojang.com", BMCLAPI_BASE),
    ("https://launchermeta.mojang.com", BMCLAPI_BASE),
    ("https://launcher.mojang.com", BMCLAPI_BASE),
    ("https://piston-data.mojang.com", BMCLAPI_BASE),
    ("https://resources.download.minecraft.net", "https://bmclapi2.bangbang93.com/assets"),
    ("https://libraries.minecraft.net", "https://bmclapi2.bangbang93.com/maven"),
    ("https://files.minecraftforge.net/maven", "https://bmclapi2.bangbang93.com/maven"),
    ("https://maven.minecraftforge.net", "https://bmclapi2.bangbang93.com/maven"),
    ("https://maven.fabricmc.net", "https://bmclapi2.bangbang93.com/maven"),
    ("https://maven.quiltmc.org/repository/release", "https://bmclapi2.bangbang93.com/maven"),
    ("https://meta.fabricmc.net", "https://bmclapi2.bangbang93.com/fabric-meta"),
    ("https://meta.quiltmc.org", "https://bmclapi2.bangbang93.com/quilt-meta"),
    ("https://api.minecraftservices.com", "https://bmclapi2.bangbang93.com/minecraft-services"),
];

/// One source of game files.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub name: String,
    rules: Vec<(String, String)>,
}

impl Endpoint {
    /// The origin, unmodified.
    pub fn official() -> Self {
        Self {
            name: "Official (Mojang)".to_string(),
            rules: Vec::new(),
        }
    }

    /// BMCLAPI CDN mirror.
    pub fn bmclapi() -> Self {
        Self {
            name: "BMCLAPI (CDN)".to_string(),
            rules: BMCLAPI_RULES
                .iter()
                .map(|(f, t)| (f.to_string(), t.to_string()))
                .collect(),
        }
    }

    /// A user supplied mirror base URL.
    pub fn custom(name: impl Into<String>, base: &str) -> Self {
        let base = base.trim_end_matches('/');
        let mut rules: Vec<(String, String)> = MOJANG_HOSTS
            .iter()
            .map(|host| (host.to_string(), base.to_string()))
            .collect();
        // Mirrors keep the `/assets` and `/maven` segments of BMCLAPI.
        for (from, to) in BMCLAPI_RULES {
            rules.retain(|(f, _)| f != from);
            let suffix = to.trim_start_matches(BMCLAPI_BASE);
            rules.push((from.to_string(), format!("{base}{suffix}")));
        }
        Self {
            name: name.into(),
            rules,
        }
    }

    /// Rewrite `url` for this endpoint; unchanged when no rule applies.
    pub fn rewrite(&self, url: &str) -> String {
        for (from, to) in &self.rules {
            if let Some(rest) = url.strip_prefix(from.as_str()) {
                return format!("{to}{rest}");
            }
        }
        url.to_string()
    }

    pub fn applies_to(&self, url: &str) -> bool {
        self.rules.iter().any(|(from, _)| url.starts_with(from.as_str()))
    }
}

#[derive(Debug, Clone, Copy)]
struct EndpointStat {
    /// Exponentially weighted moving average of time-to-response, in ms.
    ewma_ms: f64,
    consecutive_failures: u32,
    unhealthy_until_ms: u64,
}

impl Default for EndpointStat {
    fn default() -> Self {
        Self {
            ewma_ms: -1.0,
            consecutive_failures: 0,
            unhealthy_until_ms: 0,
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// How long an endpoint stays demoted after repeated failures.
const FAILURE_COOLDOWN_MS: u64 = 90_000;

/// An ordered, self-tuning list of endpoints.
#[derive(Debug, Clone)]
pub struct SourcePool {
    endpoints: Vec<Endpoint>,
    stats: Arc<Mutex<HashMap<String, EndpointStat>>>,
}

impl Default for SourcePool {
    fn default() -> Self {
        Self::cn()
    }
}

impl SourcePool {
    pub fn new(endpoints: Vec<Endpoint>) -> Self {
        assert!(!endpoints.is_empty(), "source pool must not be empty");
        Self {
            endpoints,
            stats: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Origin only.
    pub fn official() -> Self {
        Self::new(vec![Endpoint::official()])
    }

    /// CDN first, origin as fallback — the default for users in mainland China.
    pub fn cn() -> Self {
        Self::new(vec![Endpoint::bmclapi(), Endpoint::official()])
    }

    /// Origin first, CDN as fallback.
    pub fn global() -> Self {
        Self::new(vec![Endpoint::official(), Endpoint::bmclapi()])
    }

    pub fn endpoints(&self) -> &[Endpoint] {
        &self.endpoints
    }

    pub fn names(&self) -> Vec<&str> {
        self.endpoints.iter().map(|e| e.name.as_str()).collect()
    }

    /// All candidate URLs for `url`, de-duplicated, best-first.
    pub fn candidates(&self, url: &str) -> Vec<(String, String)> {
        let now = now_ms();
        let stats = self.stats.lock().ok();

        let mut ordered: Vec<(usize, bool, f64)> = self
            .endpoints
            .iter()
            .enumerate()
            .map(|(index, endpoint)| {
                let stat = stats
                    .as_ref()
                    .and_then(|s| s.get(&endpoint.name).copied())
                    .unwrap_or_default();
                let unhealthy = stat.unhealthy_until_ms > now;
                // Unknown latency sorts in the middle so declared order is kept
                // until real measurements arrive.
                let latency = if stat.ewma_ms < 0.0 { 500.0 } else { stat.ewma_ms };
                (index, unhealthy, latency)
            })
            .collect();

        ordered.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.total_cmp(&b.2)).then(a.0.cmp(&b.0)));

        let mut out: Vec<(String, String)> = Vec::new();
        for (index, _, _) in ordered {
            let endpoint = &self.endpoints[index];
            let rewritten = endpoint.rewrite(url);
            if out.iter().any(|(_, u)| *u == rewritten) {
                continue;
            }
            out.push((endpoint.name.clone(), rewritten));
        }
        out
    }

    /// Record a successful request and how long it took to respond.
    pub fn note_success(&self, endpoint: &str, elapsed: std::time::Duration) {
        let Ok(mut stats) = self.stats.lock() else {
            return;
        };
        let stat = stats.entry(endpoint.to_string()).or_default();
        let sample = elapsed.as_secs_f64() * 1000.0;
        stat.ewma_ms = if stat.ewma_ms < 0.0 {
            sample
        } else {
            stat.ewma_ms * 0.7 + sample * 0.3
        };
        stat.consecutive_failures = 0;
        stat.unhealthy_until_ms = 0;
    }

    /// Record a failure; repeats put the endpoint into a cooldown.
    pub fn note_failure(&self, endpoint: &str) {
        let Ok(mut stats) = self.stats.lock() else {
            return;
        };
        let stat = stats.entry(endpoint.to_string()).or_default();
        stat.consecutive_failures += 1;
        if stat.consecutive_failures >= 2 {
            stat.unhealthy_until_ms = now_ms() + FAILURE_COOLDOWN_MS;
        }
    }

    /// Latency snapshot for the settings UI: `(endpoint, ewma ms, cooling down)`.
    pub fn latency_report(&self) -> Vec<(String, Option<u64>, bool)> {
        let now = now_ms();
        let stats = self.stats.lock().ok();
        self.endpoints
            .iter()
            .map(|endpoint| {
                let stat = stats
                    .as_ref()
                    .and_then(|s| s.get(&endpoint.name).copied())
                    .unwrap_or_default();
                let latency = (stat.ewma_ms >= 0.0).then(|| stat.ewma_ms.round() as u64);
                (endpoint.name.clone(), latency, stat.unhealthy_until_ms > now)
            })
            .collect()
    }

    /// Preferred URL for `url` (for logging / display only).
    pub fn prefer(&self, url: &str) -> String {
        self.endpoints[0].rewrite(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn official_is_identity() {
        let e = Endpoint::official();
        assert_eq!(
            e.rewrite("https://libraries.minecraft.net/org/lwjgl/x.jar"),
            "https://libraries.minecraft.net/org/lwjgl/x.jar"
        );
        assert!(!e.applies_to("https://api.modrinth.com/v2/search"));
    }

    #[test]
    fn bmclapi_rewrites_libraries_and_assets() {
        let e = Endpoint::bmclapi();
        assert_eq!(
            e.rewrite("https://libraries.minecraft.net/org/lwjgl/x.jar"),
            "https://bmclapi2.bangbang93.com/maven/org/lwjgl/x.jar"
        );
        assert_eq!(
            e.rewrite("https://resources.download.minecraft.net/ab/abcd"),
            "https://bmclapi2.bangbang93.com/assets/ab/abcd"
        );
        assert_eq!(
            e.rewrite(MANIFEST_URL),
            "https://bmclapi2.bangbang93.com/mc/game/version_manifest_v2.json"
        );
        assert_eq!(
            e.rewrite("https://api.curseforge.com/v1/mods"),
            "https://api.curseforge.com/v1/mods"
        );
    }

    #[test]
    fn cn_pool_orders_and_dedupes() {
        let pool = SourcePool::cn();
        let c = pool.candidates(MANIFEST_URL);
        assert_eq!(c.len(), 2);
        assert!(c[0].1.starts_with("https://bmclapi2"));
        assert!(c[1].1.starts_with("https://piston-meta"));

        let c = pool.candidates("https://api.modrinth.com/v2/search");
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn adaptive_ordering_prefers_the_faster_endpoint() {
        let pool = SourcePool::cn();
        pool.note_success("Official (Mojang)", Duration::from_millis(50));
        pool.note_success("BMCLAPI (CDN)", Duration::from_millis(4000));

        let c = pool.candidates(MANIFEST_URL);
        assert!(c[0].1.starts_with("https://piston-meta"));
    }

    #[test]
    fn repeated_failures_demote_an_endpoint() {
        let pool = SourcePool::cn();
        pool.note_failure("BMCLAPI (CDN)");
        pool.note_failure("BMCLAPI (CDN)");
        pool.note_success("Official (Mojang)", Duration::from_millis(800));

        let report = pool.latency_report();
        let cdn = report.iter().find(|(n, _, _)| n.starts_with("BMCLAPI")).unwrap();
        assert!(cdn.2, "CDN should be cooling down");

        let c = pool.candidates(MANIFEST_URL);
        assert!(c[0].1.starts_with("https://piston-meta"));
        assert_eq!(c.len(), 2, "still reachable as a last resort");
    }
}
