// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub privilege: Option<PrivilegeConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrivilegeConfig {
    #[serde(default)]
    pub command: Option<PrivilegeCommand>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PrivilegeCommand {
    Single(String),
    Many(Vec<String>),
}

impl PrivilegeCommand {
    pub fn argv(self) -> Vec<String> {
        match self {
            PrivilegeCommand::Single(cmd) => split_words(&cmd),
            PrivilegeCommand::Many(argv) => argv,
        }
    }
}

fn split_words(value: &str) -> Vec<String> {
    value.split_whitespace().map(str::to_string).collect()
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PACNIX_CONFIG") {
        return Some(PathBuf::from(path));
    }
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => {
            let home = std::env::var_os("HOME")?;
            PathBuf::from(home).join(".config")
        }
    };
    Some(base.join("pacnix").join("config.toml"))
}

/// Loads the config file; a missing file is fine, a broken one is ignored
/// with a warning and yields the defaults.
pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Config::default(),
    };
    match toml::from_str(&text) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("pacnix: warning: ignoring broken config {}: {err}", path.display());
            Config::default()
        }
    }
}

/// Privilege argv with fallback precedence: `--privilege` flag, then
/// `PACNIX_PRIVILEGE` env, then `privilege.command` from the config file,
/// then plain `sudo`.
pub fn configured_privilege(
    flag: &Option<Vec<String>>,
    config: &Config,
) -> Vec<String> {
    if let Some(argv) = flag {
        return argv.clone();
    }
    if let Ok(value) = std::env::var("PACNIX_PRIVILEGE") {
        return split_words(&value);
    }
    if let Some(argv) = config
        .privilege
        .as_ref()
        .and_then(|p| p.command.clone())
        .map(PrivilegeCommand::argv)
    {
        return argv;
    }
    vec!["sudo".to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_with_string_command() {
        let config: Config = toml::from_str(r#"privilege = { command = "sudo-rs" }"#).unwrap();
        let argv = configured_privilege(&None, &config);
        assert_eq!(argv, vec!["sudo-rs"]);
    }

    #[test]
    fn parses_config_with_array_command() {
        let config: Config =
            toml::from_str(r#"privilege = { command = ["sudo-rs", "-E"] }"#).unwrap();
        let argv = configured_privilege(&None, &config);
        assert_eq!(argv, vec!["sudo-rs", "-E"]);
    }

    #[test]
    fn flag_beats_config() {
        let config: Config = toml::from_str(r#"privilege = { command = "sudo-rs" }"#).unwrap();
        let argv = configured_privilege(
            &Some(vec!["pkexec".to_string()]),
            &config,
        );
        assert_eq!(argv, vec!["pkexec"]);
    }

    #[test]
    fn defaults_to_sudo() {
        let config = Config::default();
        let argv = configured_privilege(&None, &config);
        assert_eq!(argv, vec!["sudo"]);
    }

    #[test]
    fn split_words_handles_quoted_free_form() {
        assert_eq!(split_words("sudo-rs -E"), vec!["sudo-rs", "-E"]);
        assert_eq!(split_words("pkexec"), vec!["pkexec"]);
    }
}