//! On-disk layout of a Minecraft installation, matching the vanilla launcher
//! (`.minecraft`) so existing installations are picked up unchanged.

use crate::error::IoResultExt;
use crate::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct GamePaths {
    root: PathBuf,
}

impl GamePaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `%APPDATA%\.minecraft` (or the platform equivalent).
    pub fn default_root() -> PathBuf {
        if let Ok(dir) = std::env::var("APPDATA") {
            return PathBuf::from(dir).join(".minecraft");
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".minecraft");
        }
        PathBuf::from(".minecraft")
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    pub fn version_dir(&self, id: &str) -> PathBuf {
        self.versions_dir().join(id)
    }

    pub fn version_json(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.json"))
    }

    pub fn version_jar(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.jar"))
    }

    pub fn natives_dir(&self, id: &str) -> PathBuf {
        self.version_dir(id).join("natives")
    }

    pub fn libraries_dir(&self) -> PathBuf {
        self.root.join("libraries")
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn indexes_dir(&self) -> PathBuf {
        self.assets_dir().join("indexes")
    }

    pub fn objects_dir(&self) -> PathBuf {
        self.assets_dir().join("objects")
    }

    pub fn virtual_dir(&self, asset_id: &str) -> PathBuf {
        self.assets_dir().join("virtual").join(asset_id)
    }

    pub fn index_file(&self, asset_id: &str) -> PathBuf {
        self.indexes_dir().join(format!("{asset_id}.json"))
    }

    /// Content-addressed asset object: `objects/<first 2 hex chars>/<hash>`.
    pub fn object_file(&self, hash: &str) -> PathBuf {
        let prefix = &hash[..2.min(hash.len())];
        self.objects_dir().join(prefix).join(hash)
    }

    /// `assets/log_configs/<id>.xml`
    pub fn log_config(&self, id: &str) -> PathBuf {
        self.assets_dir()
            .join("log_configs")
            .join(format!("{id}.xml"))
    }

    pub fn mods_dir(&self) -> PathBuf {
        self.root.join("mods")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Create the directories the installer writes into.
    pub fn ensure_base_dirs(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.versions_dir(),
            self.libraries_dir(),
            self.assets_dir(),
            self.indexes_dir(),
            self.objects_dir(),
        ] {
            std::fs::create_dir_all(&dir).at(&dir)?;
        }
        Ok(())
    }

    pub fn is_installed(&self, id: &str) -> bool {
        self.version_json(id).is_file() && self.version_jar(id).is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Built with `join` so the assertions hold on every platform.
    #[test]
    fn layout_matches_vanilla() {
        let root = PathBuf::from("mc");
        let p = GamePaths::new(root.clone());

        assert_eq!(
            p.version_json("1.20.1"),
            root.join("versions").join("1.20.1").join("1.20.1.json")
        );
        assert_eq!(
            p.version_jar("1.20.1"),
            root.join("versions").join("1.20.1").join("1.20.1.jar")
        );
        assert_eq!(
            p.object_file("abcdef0123"),
            root.join("assets").join("objects").join("ab").join("abcdef0123")
        );
        assert_eq!(
            p.index_file("5"),
            root.join("assets").join("indexes").join("5.json")
        );
        assert_eq!(
            p.natives_dir("1.20.1"),
            root.join("versions").join("1.20.1").join("natives")
        );
    }
}
