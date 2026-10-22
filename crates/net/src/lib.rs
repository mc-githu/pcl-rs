//! Networking for PCL-RS.
//!
//! * [`mirror`] — URL rewriting so every Mojang endpoint can be served from a CDN
//!   mirror (BMCLAPI), with automatic fallback to the origin and adaptive
//!   latency-based ordering.
//! * [`download`] — a concurrent, resumable, checksum-verified downloader that
//!   reports aggregated progress (the GUI drives its progress bar from it).

pub mod download;
pub mod http;
pub mod mirror;

pub use download::{
    sha1_file, sha1_file_from_bytes, DownloadSummary, DownloadTask, Downloader, Progress,
    ProgressSink,
};
pub use http::{HttpClient, USER_AGENT};
pub use mirror::{Endpoint, SourcePool, MANIFEST_URL};

/// Official Modrinth API root.
pub const MODRINTH_API: &str = "https://api.modrinth.com/v2";
/// Official CurseForge API root.
pub const CURSEFORGE_API: &str = "https://api.curseforge.com/v1";
