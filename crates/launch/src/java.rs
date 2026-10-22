//! Java runtime discovery.
//!
//! Vanilla 1.20.1 needs Java 17, 1.21 needs 21, 1.8.9 needs 8, and users rarely
//! know which JDK they have. Discovery scans the usual install roots, probes each
//! candidate with `java -version`, and picks the best fit for the version.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaSource {
    JavaHome,
    Path,
    Registry,
    Scan,
    Manual,
}

impl JavaSource {
    pub fn label(&self) -> &'static str {
        match self {
            JavaSource::JavaHome => "JAVA_HOME",
            JavaSource::Path => "PATH",
            JavaSource::Registry => "注册表",
            JavaSource::Scan => "目录扫描",
            JavaSource::Manual => "手动指定",
        }
    }
}

#[derive(Debug, Clone)]
pub struct JavaRuntime {
    /// `.../bin/java.exe`
    pub path: PathBuf,
    /// Full version as reported by `java -version`, e.g. `21.0.10`.
    pub version: String,
    /// Feature release, e.g. `21` or `8`.
    pub major: u32,
    pub vendor: Option<String>,
    /// `x86_64` / `x86` / `arm64`
    pub arch: String,
    pub source: JavaSource,
}

impl JavaRuntime {
    pub fn home(&self) -> Option<&Path> {
        self.path.parent()?.parent()
    }

    pub fn display(&self) -> String {
        format!(
            "Java {} ({}, {}, {})",
            self.version,
            self.vendor.as_deref().unwrap_or("未知厂商"),
            self.arch,
            self.source.label()
        )
    }
}

/// Discover every Java runtime we can reasonably find.
pub fn detect() -> Vec<JavaRuntime> {
    let mut candidates: Vec<(PathBuf, JavaSource)> = Vec::new();

    if let Ok(home) = std::env::var("JAVA_HOME") {
        push_candidate(
            &mut candidates,
            PathBuf::from(home).join("bin/java.exe"),
            JavaSource::JavaHome,
        );
    }

    for entry in std::env::var("PATH").unwrap_or_default().split(';') {
        if !entry.is_empty() {
            push_candidate(
                &mut candidates,
                Path::new(entry).join("java.exe"),
                JavaSource::Path,
            );
        }
    }

    for home in registry_homes() {
        push_candidate(&mut candidates, home.join("bin/java.exe"), JavaSource::Registry);
    }

    for dir in scan_roots() {
        for home in child_dirs_with_java(&dir) {
            push_candidate(&mut candidates, home.join("bin/java.exe"), JavaSource::Scan);
        }
    }

    let mut runtimes: Vec<JavaRuntime> = Vec::new();
    for (path, source) in candidates {
        if !path.is_file() {
            continue;
        }
        match probe(&path) {
            Ok(mut runtime) => {
                runtime.source = source;
                if !runtimes.iter().any(|r| same_home(r, &runtime)) {
                    runtimes.push(runtime);
                }
            }
            Err(e) => tracing::debug!("not a usable java at {}: {e:#}", path.display()),
        }
    }

    runtimes.sort_by(|a, b| b.major.cmp(&a.major).then(a.version.cmp(&b.version)));
    runtimes
}

/// Probe one specific executable (used by "browse for javaw.exe").
pub fn probe_path(path: &Path) -> Result<JavaRuntime> {
    let mut runtime = probe(path)?;
    runtime.source = JavaSource::Manual;
    Ok(runtime)
}

fn same_home(a: &JavaRuntime, b: &JavaRuntime) -> bool {
    match (a.home(), b.home()) {
        (Some(x), Some(y)) => same_path(x, y),
        _ => same_path(&a.path, &b.path),
    }
}

/// Windows paths are case-insensitive; compare them that way.
fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

fn push_candidate(list: &mut Vec<(PathBuf, JavaSource)>, path: PathBuf, source: JavaSource) {
    if !list.iter().any(|(p, _)| same_path(p, &path)) {
        list.push((path, source));
    }
}

fn probe(path: &Path) -> Result<JavaRuntime> {
    let output = Command::new(path)
        .arg("-version")
        .output()
        .with_context(|| format!("running {}", path.display()))?;

    // `java -version` writes to stderr on every JDK ever shipped.
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let text = if stderr.trim().is_empty() { stdout } else { stderr };

    if text.trim().is_empty() {
        anyhow::bail!("`{} -version` produced no output", path.display());
    }

    let (major, version) = parse_version(&text).ok_or_else(|| {
        anyhow::anyhow!(
            "cannot parse java version from: {}",
            text.lines().next().unwrap_or("").trim()
        )
    })?;

    Ok(JavaRuntime {
        path: path.to_path_buf(),
        version,
        major,
        vendor: parse_vendor(&text),
        arch: parse_arch(&text),
        source: JavaSource::Manual,
    })
}

/// Parse `openjdk version "21.0.10"` and the legacy `java version "1.8.0_402"`.
pub fn parse_version(text: &str) -> Option<(u32, String)> {
    let quoted = text
        .lines()
        .find(|line| line.contains("version"))
        .and_then(|line| {
            let start = line.find('"')?;
            let rest = &line[start + 1..];
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        });

    let raw = quoted.or_else(|| {
        text.split(|c: char| c.is_whitespace() || c == '"')
            .find(|token| {
                token.chars().next().is_some_and(|c| c.is_ascii_digit()) && token.contains('.')
            })
            .map(|t| t.to_string())
    })?;

    let cleaned = raw.trim_start_matches(|c: char| !c.is_ascii_digit());
    let mut parts = cleaned.split(|c| c == '.' || c == '_' || c == '-' || c == '+');

    let first: u32 = parts.next()?.parse().ok()?;
    let major = if first == 1 {
        parts.next()?.parse().ok()?
    } else {
        first
    };

    Some((major, cleaned.to_string()))
}

fn parse_vendor(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let vendor = if lower.contains("microsoft") {
        "Microsoft"
    } else if lower.contains("temurin") || lower.contains("adoptium") || lower.contains("adoptopenjdk") {
        "Eclipse Temurin"
    } else if lower.contains("zulu") {
        "Azul Zulu"
    } else if lower.contains("graalvm") {
        "GraalVM"
    } else if lower.contains("corretto") {
        "Amazon Corretto"
    } else if lower.contains("semeru") || lower.contains("ibm") {
        "IBM Semeru"
    } else if lower.contains("liberica") {
        "BellSoft Liberica"
    } else if lower.contains("openjdk") {
        "OpenJDK"
    } else if lower.contains("oracle") {
        "Oracle"
    } else {
        return None;
    };
    Some(vendor.to_string())
}

fn parse_arch(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if lower.contains("64-bit") || lower.contains("x86_64") || lower.contains("amd64") {
        "x86_64".to_string()
    } else if lower.contains("aarch64") || lower.contains("arm64") {
        "arm64".to_string()
    } else {
        "x86".to_string()
    }
}

/// Pick the best runtime for a version requiring `required_major`.
///
/// Preference: exact major, then the smallest major above the requirement (newer
/// JDKs sometimes break old mods), then the largest below it.
pub fn pick(runtimes: &[JavaRuntime], required_major: Option<u32>) -> Option<JavaRuntime> {
    let Some(required) = required_major else {
        return runtimes.first().cloned();
    };

    if let Some(exact) = runtimes.iter().find(|r| r.major == required) {
        return Some(exact.clone());
    }
    if let Some(newer) = runtimes
        .iter()
        .filter(|r| r.major > required)
        .min_by_key(|r| r.major)
    {
        return Some(newer.clone());
    }
    runtimes
        .iter()
        .filter(|r| r.major < required)
        .max_by_key(|r| r.major)
        .cloned()
}

// ---------------------------------------------------------------------------
// Platform scanning
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn registry_homes() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for key in [
        r"HKLM\SOFTWARE\JavaSoft\JDK",
        r"HKLM\SOFTWARE\JavaSoft\JRE",
        r"HKLM\SOFTWARE\JavaSoft\Java Runtime Environment",
        r"HKLM\SOFTWARE\JavaSoft\Java Development Kit",
    ] {
        let Ok(output) = Command::new("reg")
            .args(["query", key, "/s", "/v", "JavaHome"])
            .output()
        else {
            continue;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("JavaHome") {
                if let Some(value) = rest.split("REG_SZ").nth(1) {
                    let value = value.trim();
                    if !value.is_empty() {
                        out.push(PathBuf::from(value));
                    }
                }
            }
        }
    }
    out
}

#[cfg(not(target_os = "windows"))]
fn registry_homes() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(target_os = "windows")]
fn scan_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for base in [
        std::env::var("ProgramFiles").ok(),
        std::env::var("ProgramFiles(x86)").ok(),
        std::env::var("LOCALAPPDATA").ok().map(|p| format!("{p}\\Programs")),
    ]
    .into_iter()
    .flatten()
    {
        for vendor in [
            "Java",
            "Eclipse Adoptium",
            "Eclipse Foundation",
            "Microsoft",
            "Zulu",
            "BellSoft",
            "Amazon Corretto",
            "Semeru",
            "GraalVM",
            "JetBrains",
        ] {
            roots.push(PathBuf::from(&base).join(vendor));
        }
        roots.push(PathBuf::from(&base));
    }
    roots.into_iter().filter(|p| p.is_dir()).collect()
}

#[cfg(not(target_os = "windows"))]
fn scan_roots() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/lib/jvm"),
        PathBuf::from("/Library/Java/JavaVirtualMachines"),
    ]
    .into_iter()
    .filter(|p| p.is_dir())
    .collect()
}

/// Directories that look like a JDK/JRE home (contain `bin/java.exe`).
fn child_dirs_with_java(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|path| path.join("bin/java.exe").is_file() || path.join("bin/java").is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_versions() {
        assert_eq!(
            parse_version(r#"openjdk version "21.0.10" 2026-01-20 LTS"#).unwrap().0,
            21
        );
        assert_eq!(parse_version(r#"openjdk version "17.0.2" 2022-01-18"#).unwrap().0, 17);
    }

    #[test]
    fn parses_legacy_versions() {
        assert_eq!(parse_version(r#"java version "1.8.0_402""#).unwrap().0, 8);
        assert_eq!(parse_version(r#"java version "1.7.0_80""#).unwrap().0, 7);
    }

    #[test]
    fn detects_vendor_and_arch() {
        let text = r#"openjdk version "21.0.10" 2026-01-20 LTS
OpenJDK Runtime Environment Microsoft-12345 (build 21.0.10+7-LTS)
OpenJDK 64-Bit Server VM Microsoft-12345 (build 21.0.10+7-LTS, mixed mode, sharing)"#;
        assert_eq!(parse_vendor(text).as_deref(), Some("Microsoft"));
        assert_eq!(parse_arch(text), "x86_64");
    }

    #[test]
    fn picks_best_runtime() {
        let mk = |major: u32| JavaRuntime {
            path: PathBuf::from(format!("C:/jdk{major}/bin/java.exe")),
            version: format!("{major}.0.1"),
            major,
            vendor: None,
            arch: "x86_64".into(),
            source: JavaSource::Scan,
        };
        let all = vec![mk(21), mk(17), mk(8)];

        assert_eq!(pick(&all, Some(17)).unwrap().major, 17);
        assert_eq!(pick(&all, Some(16)).unwrap().major, 17);
        assert_eq!(pick(&all, Some(25)).unwrap().major, 21);
        assert_eq!(pick(&all, None).unwrap().major, 21);
        assert!(pick(&[], Some(17)).is_none());
    }
}
