//! Mod discovery and download.
//!
//! PCL-RS talks to the two ecosystems mod authors actually publish to:
//! * **Modrinth** — open API, no key required,
//! * **CurseForge** — official API, requires a free API key.
//!
//! No third-party aggregator or scraper is involved: every download URL comes
//! straight from the project's own API response, and files whose authors opted out
//! of third-party downloads are reported instead of guessed.

pub mod curseforge;
pub mod modrinth;
pub mod types;

pub use curseforge::CurseForge;
pub use modrinth::Modrinth;
pub use types::{
    DepKind, Loader, ModDependency, ModFile, ModProject, ProjectSource, ReleaseType, SearchQuery,
    SearchResults,
};
