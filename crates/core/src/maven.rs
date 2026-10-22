//! Maven coordinate parsing, as used by `libraries[].name` in version JSON.
//!
//! Format: `group:artifact:version[:classifier][@extension]`, e.g.
//! `org.lwjgl:lwjgl:3.3.2`, `org.lwjgl:lwjgl:3.3.2:natives-windows`.

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MavenName {
    pub group: String,
    pub artifact: String,
    pub version: String,
    pub classifier: Option<String>,
    pub extension: String,
}

impl MavenName {
    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(Error::Maven(raw.to_string()));
        }

        let (body, extension) = match raw.rsplit_once('@') {
            Some((body, ext)) => (body, ext.to_string()),
            None => (raw, "jar".to_string()),
        };

        let parts: Vec<&str> = body.split(':').collect();
        let (group, artifact, version, classifier) = match parts.as_slice() {
            [g, a, v] => (*g, *a, *v, None),
            [g, a, v, c] => (*g, *a, *v, Some(c.to_string())),
            _ => return Err(Error::Maven(raw.to_string())),
        };

        if group.is_empty() || artifact.is_empty() || version.is_empty() {
            return Err(Error::Maven(raw.to_string()));
        }

        Ok(MavenName {
            group: group.to_string(),
            artifact: artifact.to_string(),
            version: version.to_string(),
            classifier,
            extension,
        })
    }

    /// Path of the artifact inside a maven repository root.
    pub fn path(&self) -> String {
        let file = match &self.classifier {
            Some(c) => format!("{}-{}-{}.{}", self.artifact, self.version, c, self.extension),
            None => format!("{}-{}.{}", self.artifact, self.version, self.extension),
        };
        format!(
            "{}/{}/{}/{}",
            self.group_path(),
            self.artifact,
            self.version,
            file
        )
    }

    pub fn group_path(&self) -> String {
        self.group.replace('.', "/")
    }

    pub fn is_native(&self) -> bool {
        self.classifier.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain() {
        let m = MavenName::parse("org.lwjgl:lwjgl:3.3.2").unwrap();
        assert_eq!(m.path(), "org/lwjgl/lwjgl/3.3.2/lwjgl-3.3.2.jar");
        assert!(!m.is_native());
    }

    #[test]
    fn parses_classifier() {
        let m = MavenName::parse("org.lwjgl:lwjgl:3.3.2:natives-windows@jar").unwrap();
        assert_eq!(
            m.path(),
            "org/lwjgl/lwjgl/3.3.2/lwjgl-3.3.2-natives-windows.jar"
        );
        assert!(m.is_native());
    }

    #[test]
    fn parses_forge_style() {
        let m = MavenName::parse("net.minecraftforge:forge:1.20.1-47.2.0:universal").unwrap();
        assert_eq!(m.version, "1.20.1-47.2.0");
        assert!(m.path().ends_with("forge-1.20.1-47.2.0-universal.jar"));
    }

    #[test]
    fn rejects_garbage() {
        assert!(MavenName::parse("not-a-coordinate").is_err());
        assert!(MavenName::parse("").is_err());
    }
}
