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
            eprintln!(
                "pacnix: warning: ignoring broken config {}: {err}",
                path.display()
            );
            Config::default()
        }
    }
}

/// Privilege argv with fallback precedence: `--privilege` flag, then
/// `PACNIX_PRIVILEGE` env, then `privilege.command` from the config file,
/// then an auto-detected available tool (sudo-rs, sudo, pkexec, doas).
pub fn configured_privilege(flag: &Option<Vec<String>>, config: &Config) -> Vec<String> {
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
    detect_privilege()
}

/// Picks the first privilege tool present in PATH. Preference order matters:
/// sudo-rs, then plain sudo, then GUI pkexec, then doas.
fn detect_privilege() -> Vec<String> {
    const CANDIDATES: &[&str] = &["sudo-rs", "sudo", "pkexec", "doas"];
    for candidate in CANDIDATES {
        if find_in_path(candidate).is_some() {
            return vec![candidate.to_string()];
        }
    }
    vec!["sudo".to_string()]
}

pub fn find_in_path(program: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file() && is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
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
        let argv = configured_privilege(&Some(vec!["pkexec".to_string()]), &config);
        assert_eq!(argv, vec!["pkexec"]);
    }

    #[test]
    fn defaults_to_detected_tool() {
        let config = Config::default();
        let argv = configured_privilege(&None, &config);
        assert_eq!(argv, detect_privilege(), "fallback must detect a tool");
        assert!(
            ["sudo-rs", "sudo", "pkexec", "doas"].contains(&argv[0].as_str()),
            "detected tool must be a known privilege tool"
        );
    }

    #[test]
    fn detect_prefers_sudo_rs() {
        let path = std::env::var_os("PATH").unwrap();
        let dir = std::env::temp_dir().join(format!("pacnix-priv-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for tool in ["sudo-rs", "sudo", "pkexec"] {
            let f = dir.join(tool);
            std::fs::write(&f, "").unwrap();
            let mut perms = std::fs::metadata(&f).unwrap().permissions();
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
            std::fs::set_permissions(&f, perms).unwrap();
        }
        std::env::set_var(
            "PATH",
            format!("{}:{}", dir.display(), path.to_string_lossy()),
        );
        assert_eq!(detect_privilege(), vec!["sudo-rs"]);
        std::env::set_var("PATH", &path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn split_words_handles_quoted_free_form() {
        assert_eq!(split_words("sudo-rs -E"), vec!["sudo-rs", "-E"]);
        assert_eq!(split_words("pkexec"), vec!["pkexec"]);
    }
}
