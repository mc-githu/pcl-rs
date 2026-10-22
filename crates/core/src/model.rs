//! Serde models for Mojang's version manifest and version detail JSON.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<VersionSummary>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VersionSummary {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub time: Option<String>,
    #[serde(rename = "releaseTime", default)]
    pub release_time: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(rename = "complianceLevel", default)]
    pub compliance_level: Option<i32>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct VersionDetails {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(rename = "mainClass", default)]
    pub main_class: Option<String>,
    #[serde(default)]
    pub assets: Option<String>,
    #[serde(rename = "assetIndex", default)]
    pub asset_index: Option<AssetIndexRef>,
    #[serde(default)]
    pub downloads: Option<HashMap<String, DownloadInfo>>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(rename = "javaVersion", default)]
    pub java_version: Option<JavaVersion>,
    #[serde(default)]
    pub logging: Option<Logging>,
    #[serde(default)]
    pub arguments: Option<Arguments>,
    /// Pre-1.13 versions describe game arguments as a single template string.
    #[serde(rename = "minecraftArguments", default)]
    pub minecraft_arguments: Option<String>,
    #[serde(rename = "inheritsFrom", default)]
    pub inherits_from: Option<String>,
    /// Forge/Fabric profiles point at the vanilla jar they extend.
    #[serde(rename = "jar", default)]
    pub jar: Option<String>,
    #[serde(rename = "releaseTime", default)]
    pub release_time: Option<String>,
    #[serde(default)]
    pub time: Option<String>,
    #[serde(rename = "minimumLauncherVersion", default)]
    pub minimum_launcher_version: Option<i32>,
    #[serde(rename = "complianceLevel", default)]
    pub compliance_level: Option<i32>,
}

impl VersionDetails {
    /// The version whose client jar must be used (Forge/Fabric inherit it).
    pub fn jar_id(&self) -> &str {
        self.jar.as_deref().unwrap_or(&self.id)
    }

    pub fn client_download(&self) -> Option<&DownloadInfo> {
        self.downloads.as_ref()?.get("client")
    }

    pub fn required_java_major(&self) -> Option<u32> {
        self.java_version.as_ref().map(|j| j.major_version)
    }

    /// Merge a child (Forge/Fabric profile) on top of this vanilla parent.
    ///
    /// The child wins for scalar fields, libraries accumulate (child last), and
    /// argument lists concatenate — exactly what Mojang's launcher does for
    /// `inheritsFrom`.
    pub fn merge_child(self, child: VersionDetails) -> VersionDetails {
        let parent_id = self.id.clone();

        let mut downloads = self.downloads.unwrap_or_default();
        if let Some(child_dl) = child.downloads {
            downloads.extend(child_dl);
        }

        let mut libraries = self.libraries;
        libraries.extend(child.libraries);

        let arguments = match (self.arguments, child.arguments) {
            (None, c) => c,
            (p, None) => p,
            (Some(p), Some(c)) => Some(Arguments {
                game: [p.game, c.game].concat(),
                jvm: [p.jvm, c.jvm].concat(),
            }),
        };

        VersionDetails {
            id: child.id,
            kind: child.kind.or(self.kind),
            main_class: child.main_class.or(self.main_class),
            assets: child.assets.or(self.assets),
            asset_index: child.asset_index.or(self.asset_index),
            downloads: Some(downloads),
            libraries,
            java_version: child.java_version.or(self.java_version),
            logging: child.logging.or(self.logging),
            arguments,
            minecraft_arguments: child.minecraft_arguments.or(self.minecraft_arguments),
            inherits_from: child.inherits_from,
            // A loader profile without its own `jar` still launches the vanilla
            // jar of its parent, never a non-existent `versions/<loader>/<loader>.jar`.
            jar: child.jar.or(self.jar).or(Some(parent_id)),
            release_time: child.release_time.or(self.release_time),
            time: child.time.or(self.time),
            minimum_launcher_version: child
                .minimum_launcher_version
                .or(self.minimum_launcher_version),
            compliance_level: child.compliance_level.or(self.compliance_level),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DownloadInfo {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AssetIndexRef {
    pub id: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(rename = "totalSize", default)]
    pub total_size: Option<u64>,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AssetIndex {
    #[serde(default)]
    pub id: Option<String>,
    pub objects: HashMap<String, AssetObject>,
    #[serde(default)]
    pub map_to_resources: bool,
    #[serde(default)]
    pub virtual_: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JavaVersion {
    #[serde(default)]
    pub component: Option<String>,
    #[serde(rename = "majorVersion")]
    pub major_version: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Logging {
    #[serde(default)]
    pub client: Option<LoggingClient>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoggingClient {
    #[serde(default)]
    pub argument: Option<String>,
    pub file: LoggingFile,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoggingFile {
    pub id: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
}

/// A single entry of `arguments.game` / `arguments.jvm`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Conditional {
        #[serde(default)]
        rules: Vec<Rule>,
        value: ArgumentValue,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ArgumentValue {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: Option<LibraryDownloads>,
    /// Legacy native classifier selector, e.g. `{"windows": "natives-windows-${arch}"}`.
    #[serde(default)]
    pub natives: Option<HashMap<String, String>>,
    #[serde(default)]
    pub rules: Option<Vec<Rule>>,
    #[serde(default)]
    pub extract: Option<Extract>,
    /// Legacy: maven repository root this library comes from.
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LibraryDownloads {
    #[serde(default)]
    pub artifact: Option<LibraryArtifact>,
    #[serde(default)]
    pub classifiers: Option<HashMap<String, LibraryArtifact>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LibraryArtifact {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rule {
    pub action: RuleAction,
    #[serde(default)]
    pub os: Option<OsRule>,
    #[serde(default)]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OsRule {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_version() {
        let raw = r#"{
            "id": "1.20.1",
            "type": "release",
            "mainClass": "net.minecraft.client.main.Main",
            "assets": "5",
            "assetIndex": {"id":"5","sha1":"abc","size":10,"totalSize":100,"url":"https://x"},
            "downloads": {"client": {"sha1":"d","size":2,"url":"https://y"}},
            "libraries": [],
            "javaVersion": {"component":"java-runtime-gamma","majorVersion":17},
            "arguments": {"game": ["--username", {"rules":[{"action":"allow","features":{"is_demo_user":true}}],"value":"--demo"}]}
        }"#;
        let v: VersionDetails = serde_json::from_str(raw).unwrap();
        assert_eq!(v.jar_id(), "1.20.1");
        assert_eq!(v.required_java_major(), Some(17));
        assert_eq!(v.arguments.unwrap().game.len(), 2);
    }

    #[test]
    fn inherits_and_merges() {
        let parent: VersionDetails = serde_json::from_str(
            r#"{"id":"1.20.1","mainClass":"a","libraries":[{"name":"g:a:1"}],"arguments":{"game":["--p"],"jvm":["-X"]}}"#,
        )
        .unwrap();
        let child: VersionDetails = serde_json::from_str(
            r#"{"id":"forge","inheritsFrom":"1.20.1","mainClass":"b","libraries":[{"name":"g:b:2"}],"arguments":{"game":["--c"],"jvm":[]}}"#,
        )
        .unwrap();
        let merged = parent.merge_child(child);
        assert_eq!(merged.id, "forge");
        assert_eq!(merged.main_class.as_deref(), Some("b"));
        assert_eq!(merged.libraries.len(), 2);
        assert_eq!(merged.jar_id(), "1.20.1", "loader must reuse the vanilla jar");
        assert_eq!(merged.arguments.unwrap().game.len(), 2);
    }
}
