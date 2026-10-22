//! Rule evaluation (the `rules` arrays guarding libraries, arguments and assets).

use crate::model::{Rule, RuleAction};
use serde::{Deserialize, Serialize};

/// Runtime feature flags exposed to `arguments` rules.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Features {
    #[serde(default)]
    pub is_demo_user: bool,
    #[serde(default)]
    pub has_custom_resolution: bool,
    #[serde(default)]
    pub has_quick_plays_support: bool,
    #[serde(default)]
    pub is_quick_play_singleplayer: bool,
    #[serde(default)]
    pub is_quick_play_multiplayer: bool,
    #[serde(default)]
    pub is_quick_play_realms: bool,
}

/// Everything a rule can test against on the current machine.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RuleContext {
    /// `windows` | `osx` | `linux`
    pub os_name: String,
    /// `x86` | `x86_64` | `arm64`
    pub os_arch: String,
    pub os_version: String,
    pub features: Features,
}

impl Default for RuleContext {
    fn default() -> Self {
        Self::host()
    }
}

impl RuleContext {
    pub fn host() -> Self {
        Self {
            os_name: std::env::consts::OS.replace("macos", "osx"),
            os_arch: match std::env::consts::ARCH {
                "x86_64" => "x86_64",
                "aarch64" => "arm64",
                "x86" => "x86",
                other => other,
            }
            .to_string(),
            // The only rules that test `os.version` are macOS specific.
            os_version: String::new(),
            features: Features::default(),
        }
    }
}

impl Features {
    fn get(&self, key: &str) -> bool {
        match key {
            "is_demo_user" => self.is_demo_user,
            "has_custom_resolution" => self.has_custom_resolution,
            "has_quick_plays_support" => self.has_quick_plays_support,
            "is_quick_play_singleplayer" => self.is_quick_play_singleplayer,
            "is_quick_play_multiplayer" => self.is_quick_play_multiplayer,
            "is_quick_play_realms" => self.is_quick_play_realms,
            _ => false,
        }
    }
}

impl Rule {
    pub fn matches(&self, ctx: &RuleContext) -> bool {
        if let Some(os) = &self.os {
            if let Some(name) = &os.name {
                if !name.eq_ignore_ascii_case(&ctx.os_name) {
                    return false;
                }
            }
            if let Some(arch) = &os.arch {
                if !arch.eq_ignore_ascii_case(&ctx.os_arch) {
                    return false;
                }
            }
            if let Some(version) = &os.version {
                if !matches_version_pattern(version, &ctx.os_version) {
                    return false;
                }
            }
        }

        if let Some(features) = &self.features {
            for (key, want) in features {
                if ctx.features.get(key) != *want {
                    return false;
                }
            }
        }

        true
    }
}

/// `os.version` is a Java-style regex; only a tiny prefix subset is supported.
fn matches_version_pattern(pattern: &str, actual: &str) -> bool {
    if actual.is_empty() {
        return false;
    }
    let pattern = pattern.trim_start_matches('^');
    actual.starts_with(&pattern[..pattern.len().min(actual.len())])
}

/// Evaluate a rules list: no rules means allowed, otherwise the last match wins.
pub fn evaluate(rules: Option<&[Rule]>, ctx: &RuleContext) -> bool {
    let Some(rules) = rules else { return true };
    if rules.is_empty() {
        return true;
    }

    let mut allowed = false;
    for rule in rules {
        if rule.matches(ctx) {
            allowed = rule.action == RuleAction::Allow;
        }
    }
    allowed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> RuleContext {
        RuleContext {
            os_name: "windows".into(),
            os_arch: "x86_64".into(),
            os_version: String::new(),
            features: Features::default(),
        }
    }

    fn rules(json: &str) -> Vec<Rule> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn no_rules_allows() {
        assert!(evaluate(None, &ctx()));
        assert!(evaluate(Some(&[]), &ctx()));
    }

    #[test]
    fn os_allow_rule() {
        assert!(evaluate(
            Some(&rules(r#"[{"action":"allow","os":{"name":"windows"}}]"#)),
            &ctx()
        ));
        assert!(!evaluate(
            Some(&rules(r#"[{"action":"allow","os":{"name":"osx"}}]"#)),
            &ctx()
        ));
    }

    #[test]
    fn disallow_wins_when_later() {
        let r = rules(
            r#"[{"action":"allow","os":{"name":"windows"}},{"action":"disallow","os":{"name":"windows"}}]"#,
        );
        assert!(!evaluate(Some(&r), &ctx()));
    }

    #[test]
    fn arch_rules() {
        assert!(!evaluate(
            Some(&rules(r#"[{"action":"allow","os":{"arch":"x86"}}]"#)),
            &ctx()
        ));
        assert!(evaluate(
            Some(&rules(r#"[{"action":"allow","os":{"arch":"x86_64"}}]"#)),
            &ctx()
        ));
    }

    #[test]
    fn feature_rules() {
        let r = rules(r#"[{"action":"allow","features":{"is_demo_user":true}}]"#);
        assert!(!evaluate(Some(&r), &ctx()));
        let mut c = ctx();
        c.features.is_demo_user = true;
        assert!(evaluate(Some(&r), &c));
    }
}
