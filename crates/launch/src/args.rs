//! Command line construction.
//!
//! Built from the version JSON rather than hard-coded: Mojang moved from
//! `minecraftArguments` (a single template string) to rule-guarded
//! `arguments.game` / `arguments.jvm` arrays in 1.13, and both shapes must work.

use anyhow::{bail, Result};
use pcl_auth::Account;
use pcl_core::{
    rules::{evaluate, RuleContext},
    Argument, ArgumentValue, VersionDetails,
};
use std::collections::HashMap;
use std::path::PathBuf;

/// Everything needed to turn a version into a process invocation.
pub struct LaunchOptions {
    pub details: VersionDetails,
    /// `java.exe` (console variant, so stdout can be captured).
    pub java: PathBuf,
    pub game_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub libraries_dir: PathBuf,
    pub natives_dir: PathBuf,
    pub classpath: Vec<PathBuf>,
    pub account: Account,
    pub version_type: String,
    pub assets_index: Option<String>,
    pub log_config: Option<PathBuf>,
    pub memory_mb: u32,
    pub extra_jvm_args: Vec<String>,
    pub extra_game_args: Vec<String>,
    pub fullscreen: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub launcher_name: String,
    pub launcher_version: String,
}

pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

impl CommandSpec {
    /// Quoted, shell-ready rendering for logs and bug reports.
    pub fn display(&self) -> String {
        let mut out = quote(&self.program.to_string_lossy());
        for arg in &self.args {
            out.push(' ');
            out.push_str(&quote(arg));
        }
        out
    }
}

fn quote(value: &str) -> String {
    if value.contains(' ') || value.contains('\t') {
        format!("\"{value}\"")
    } else {
        value.to_string()
    }
}

impl LaunchOptions {
    pub fn build(&self) -> Result<CommandSpec> {
        let ctx = RuleContext::host();
        let main_class = self
            .details
            .main_class
            .clone()
            .unwrap_or_else(|| "net.minecraft.client.main.Main".to_string());

        let classpath = std::env::join_paths(&self.classpath)
            .map_err(|e| anyhow::anyhow!("building classpath: {e}"))?
            .to_string_lossy()
            .to_string();

        let replacements = self.replacements(classpath);

        let mut jvm: Vec<String> = Vec::new();
        if self.memory_mb > 0 {
            jvm.push(format!("-Xmx{}M", self.memory_mb));
            jvm.push(format!("-Xms{}M", (self.memory_mb / 4).max(128)));
        }

        if let Some(log_arg) = self.log4j_argument() {
            jvm.push(log_arg);
        }

        let mut has_classpath = false;
        let mut has_library_path = false;

        if let Some(arguments) = &self.details.arguments {
            for argument in &arguments.jvm {
                for value in expand(argument, &ctx, &replacements) {
                    if value == "-cp" || value == "-classpath" {
                        has_classpath = true;
                    }
                    if value.starts_with("-Djava.library.path") {
                        has_library_path = true;
                    }
                    jvm.push(value);
                }
            }
        }

        if !has_library_path {
            jvm.push(format!("-Djava.library.path={}", self.natives_dir.display()));
        }
        if !has_classpath {
            jvm.push("-cp".to_string());
            jvm.push(replacements.get("classpath").cloned().unwrap_or_default());
        }

        jvm.push(format!("-Dminecraft.launcher.brand={}", self.launcher_name));
        jvm.push(format!("-Dminecraft.launcher.version={}", self.launcher_version));
        jvm.extend(self.extra_jvm_args.iter().cloned());

        // --- game arguments -------------------------------------------------
        let mut game: Vec<String> = Vec::new();
        if let Some(arguments) = &self.details.arguments {
            for argument in &arguments.game {
                game.extend(expand(argument, &ctx, &replacements));
            }
        } else if let Some(legacy) = &self.details.minecraft_arguments {
            for token in legacy.split_whitespace() {
                game.push(substitute(token, &replacements));
            }
        }

        if self.fullscreen {
            game.push("--fullscreen".to_string());
        }
        if let (Some(width), Some(height)) = (self.width, self.height) {
            game.push("--width".to_string());
            game.push(width.to_string());
            game.push("--height".to_string());
            game.push(height.to_string());
        }
        game.extend(self.extra_game_args.iter().cloned());

        if game.is_empty() {
            bail!("version has neither `arguments.game` nor `minecraftArguments`");
        }

        let mut args = jvm;
        args.push(main_class);
        args.extend(game);

        Ok(CommandSpec {
            program: self.java.clone(),
            args,
            cwd: self.game_dir.clone(),
        })
    }

    fn log4j_argument(&self) -> Option<String> {
        let client = self.details.logging.as_ref()?.client.as_ref()?;
        let template = client.argument.as_ref()?;
        let path = self.log_config.as_ref()?;
        Some(template.replace("${path}", &path.display().to_string()))
    }

    fn replacements(&self, classpath: String) -> HashMap<&'static str, String> {
        let mut map: HashMap<&'static str, String> = HashMap::new();
        let assets_index = self
            .assets_index
            .clone()
            .or_else(|| self.details.assets.clone())
            .unwrap_or_else(|| "legacy".to_string());

        map.insert("auth_player_name", self.account.username.clone());
        map.insert("version_name", self.details.id.clone());
        map.insert("game_directory", self.game_dir.display().to_string());
        map.insert("assets_root", self.assets_dir.display().to_string());
        map.insert(
            "game_assets",
            self.assets_dir.join("virtual").join(&assets_index).display().to_string(),
        );
        map.insert("assets_index_name", assets_index);
        map.insert("auth_uuid", self.account.uuid.clone());
        map.insert("auth_access_token", self.account.access_token.clone());
        map.insert("clientid", self.account.client_id().to_string());
        map.insert("auth_xuid", self.account.xuid.clone().unwrap_or_default());
        map.insert("user_type", self.account.user_type.clone());
        map.insert("user_properties", self.account.user_properties().to_string());
        map.insert("version_type", self.version_type.clone());
        map.insert("natives_directory", self.natives_dir.display().to_string());
        map.insert("launcher_name", self.launcher_name.clone());
        map.insert("launcher_version", self.launcher_version.clone());
        map.insert("classpath", classpath);
        map.insert("classpath_separator", ";".to_string());
        map.insert("library_directory", self.libraries_dir.display().to_string());
        map.insert("resolution_width", self.width.unwrap_or(854).to_string());
        map.insert("resolution_height", self.height.unwrap_or(480).to_string());
        map
    }
}

/// Expand one argument entry (plain string or rule-guarded list) into tokens.
fn expand(
    argument: &Argument,
    ctx: &RuleContext,
    replacements: &HashMap<&'static str, String>,
) -> Vec<String> {
    match argument {
        Argument::Plain(value) => vec![substitute(value, replacements)],
        Argument::Conditional { rules, value } => {
            if !evaluate(Some(rules), ctx) {
                return Vec::new();
            }
            match value {
                ArgumentValue::One(single) => vec![substitute(single, replacements)],
                ArgumentValue::Many(list) => {
                    list.iter().map(|s| substitute(s, replacements)).collect()
                }
            }
        }
    }
}

/// Replace every `${name}` placeholder we know; unknown ones are left intact so
/// they show up in logs instead of silently vanishing.
pub fn substitute(input: &str, replacements: &HashMap<&'static str, String>) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let key = &after[..end];
        match replacements.get(key) {
            Some(value) => out.push_str(value),
            None => {
                out.push_str("${");
                out.push_str(key);
                out.push('}');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&'static str, &str)]) -> HashMap<&'static str, String> {
        pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    #[test]
    fn substitutes_known_and_keeps_unknown() {
        let m = map(&[("auth_player_name", "Steve"), ("version_name", "1.20.1")]);
        assert_eq!(
            substitute("--username ${auth_player_name} --version ${version_name}", &m),
            "--username Steve --version 1.20.1"
        );
        assert_eq!(substitute("${unknown}", &m), "${unknown}");
    }

    #[test]
    fn substitutes_repeated_and_adjacent() {
        let m = map(&[("a", "1"), ("b", "2")]);
        assert_eq!(substitute("${a}${b}-${a}", &m), "12-1");
        assert_eq!(substitute("no placeholders", &m), "no placeholders");
    }

    #[test]
    fn expands_conditional_rules() {
        let ctx = RuleContext::host();
        let m = HashMap::new();
        let demo = Argument::Conditional {
            rules: serde_json::from_str(r#"[{"action":"allow","features":{"is_demo_user":true}}]"#)
                .unwrap(),
            value: ArgumentValue::One("--demo".into()),
        };
        assert!(expand(&demo, &ctx, &m).is_empty());

        let plain = Argument::Plain("--username".into());
        assert_eq!(expand(&plain, &ctx, &m), vec!["--username"]);
    }
}
