//! Core domain models for PCL-RS.
//!
//! This crate is dependency-light and free of I/O policy: it only describes
//! *what* a Minecraft version is (metadata, rules, maven coordinates, on-disk
//! layout) so that the downloader, installer, launcher and GUI share one source
//! of truth.

pub mod error;
pub mod maven;
pub mod model;
pub mod paths;
pub mod rules;
pub mod uuid;

pub use error::{Error, Result};
pub use maven::MavenName;
pub use model::{
    Argument, ArgumentValue, AssetIndex, AssetIndexRef, AssetObject, DownloadInfo, JavaVersion,
    Library, LibraryArtifact, LibraryDownloads, Logging, LoggingClient, LoggingFile, OsRule, Rule,
    RuleAction, VersionDetails, VersionManifest, VersionSummary,
};
pub use paths::GamePaths;
pub use rules::{Features, RuleContext};
