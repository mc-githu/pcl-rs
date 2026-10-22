//! Asset index handling: content-addressed objects plus legacy layouts.

use anyhow::{Context, Result};
use pcl_core::{AssetIndex, GamePaths};
use std::path::Path;

/// Base URL Mojang publishes asset objects under (rewritten to the CDN later).
pub const ASSET_BASE: &str = "https://resources.download.minecraft.net";

/// `assets/objects/<2>/<hash>` URL for an object hash.
pub fn object_url(hash: &str) -> String {
    let prefix = &hash[..2.min(hash.len())];
    format!("{ASSET_BASE}/{prefix}/{hash}")
}

/// Parse an asset index file from disk.
pub fn load_index(path: &Path) -> Result<AssetIndex> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Legacy versions address assets by *name* under `assets/virtual/<id>` or
/// `resources/`; this materialises the hashed objects into that tree.
pub fn materialize_virtual(paths: &GamePaths, index: &AssetIndex, asset_id: &str) -> Result<usize> {
    let target_root = if index.map_to_resources {
        paths.root().join("resources")
    } else {
        paths.virtual_dir(asset_id)
    };

    let mut written = 0usize;
    for (name, object) in &index.objects {
        let source = paths.object_file(&object.hash);
        if !source.is_file() {
            tracing::warn!("missing asset object {} for {name}", object.hash);
            continue;
        }
        let target = target_root.join(name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if std::fs::metadata(&target)
            .map(|m| m.len() == object.size)
            .unwrap_or(false)
        {
            continue;
        }
        std::fs::copy(&source, &target)
            .with_context(|| format!("copying asset {name} -> {}", target.display()))?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_url_shape() {
        assert_eq!(
            object_url("abcdef1234567890"),
            "https://resources.download.minecraft.net/ab/abcdef1234567890"
        );
    }
}
