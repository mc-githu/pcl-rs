//! Shared types across mod sources.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectSource {
    Modrinth,
    CurseForge,
}

impl ProjectSource {
    pub fn label(&self) -> &'static str {
        match self {
            ProjectSource::Modrinth => "Modrinth",
            ProjectSource::CurseForge => "CurseForge",
        }
    }
}

/// Mod loader, in the spelling both APIs use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Loader {
    Vanilla,
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

impl Loader {
    pub fn as_str(&self) -> &'static str {
        match self {
            Loader::Vanilla => "minecraft",
            Loader::Fabric => "fabric",
            Loader::Quilt => "quilt",
            Loader::Forge => "forge",
            Loader::NeoForge => "neoforge",
        }
    }

    /// CurseForge `modLoaderType` enum value.
    pub fn curseforge_id(&self) -> u32 {
        match self {
            Loader::Forge => 1,
            Loader::Fabric => 4,
            Loader::Quilt => 5,
            Loader::NeoForge => 6,
            Loader::Vanilla => 0,
        }
    }

    pub fn parse(value: &str) -> Option<Loader> {
        match value.to_ascii_lowercase().as_str() {
            "fabric" => Some(Loader::Fabric),
            "quilt" => Some(Loader::Quilt),
            "forge" => Some(Loader::Forge),
            "neoforge" => Some(Loader::NeoForge),
            "minecraft" | "vanilla" => Some(Loader::Vanilla),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseType {
    Release,
    Beta,
    Alpha,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepKind {
    Required,
    Optional,
    Incompatible,
    Embedded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModDependency {
    pub kind: DepKind,
    pub project_id: Option<String>,
    pub version_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModProject {
    pub source: ProjectSource,
    /// Slug on Modrinth, numeric id on CurseForge.
    pub id: String,
    pub slug: String,
    pub name: String,
    pub summary: String,
    pub author: Option<String>,
    pub downloads: u64,
    pub icon_url: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModFile {
    pub source: ProjectSource,
    pub project_id: String,
    pub version_id: String,
    pub display_name: String,
    pub file_name: String,
    /// `None` when the author disabled third-party downloads (CurseForge).
    pub download_url: Option<String>,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub release_type: ReleaseType,
    pub dependencies: Vec<ModDependency>,
}

impl ModFile {
    pub fn requires_manual_download(&self) -> bool {
        self.download_url.is_none()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub game_version: Option<String>,
    pub loader: Option<Loader>,
    pub limit: u32,
    pub offset: u32,
}

impl SearchQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            game_version: None,
            loader: None,
            limit: 20,
            offset: 0,
        }
    }

    pub fn for_version(mut self, version: impl Into<String>) -> Self {
        self.game_version = Some(version.into());
        self
    }

    pub fn with_loader(mut self, loader: Loader) -> Self {
        self.loader = Some(loader);
        self
    }

    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = limit.clamp(1, 100);
        self
    }

    pub fn with_offset(mut self, offset: u32) -> Self {
        self.offset = offset;
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchResults {
    pub projects: Vec<ModProject>,
    pub total: u64,
    pub offset: u32,
}
