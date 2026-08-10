// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub privilege: Option<PrivilegeConfig>,
    #[serde(default)]
    pub nix: Option<NixConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NixConfig {
    /// Nix profile path to operate on; when absent the default profile is
    /// used and never mixed with custom ones.
    #[serde(default)]
    pub profile: Option<String>,
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
///
/// Empty argv is rejected at every level: for a privileged plan it must be
/// an actual command, otherwise execution would silently fall back to a
/// direct root-less run while the preflight claims elevation is in place.
pub fn configured_privilege(
    flag: &Option<Vec<String>>,
    config: &Config,
) -> Result<Vec<String>, String> {
    configured_privilege_from(
        flag,
        std::env::var("PACNIX_PRIVILEGE").ok().as_deref(),
        config,
        std::env::var_os("PATH").as_deref(),
    )
}

/// Pure variant of [`configured_privilege`] with injected env and PATH, so
/// tests never mutate process-global environment.
pub fn configured_privilege_from(
    flag: &Option<Vec<String>>,
    env_privilege: Option<&str>,
    config: &Config,
    path: Option<&std::ffi::OsStr>,
) -> Result<Vec<String>, String> {
    if let Some(argv) = flag {
        return reject_empty("--privilege", argv.clone());
    }
    if let Some(value) = env_privilege {
        return reject_empty("PACNIX_PRIVILEGE", split_words(value));
    }
    if let Some(command) = config.privilege.as_ref().and_then(|p| p.command.clone()) {
        let argv = PrivilegeCommand::argv(command);
        if argv.is_empty() {
            eprintln!(
                "pacnix: warning: empty privilege.command in config; falling back to detection"
            );
            return Ok(match detect_privilege_in(path) {
                Some(tool) => vec![tool.to_string()],
                None => vec!["sudo".to_string()],
            });
        }
        return Ok(argv);
    }
    match detect_privilege_in(path) {
        Some(tool) => Ok(vec![tool.to_string()]),
        None => Ok(vec!["sudo".to_string()]),
    }
}

fn reject_empty(source: &str, argv: Vec<String>) -> Result<Vec<String>, String> {
    if argv.is_empty() {
        Err(format!(
            "{source} must name a privilege command (e.g. sudo, pkexec, doas)"
        ))
    } else {
        Ok(argv)
    }
}

fn detect_privilege_in(path: Option<&std::ffi::OsStr>) -> Option<&'static str> {
    const CANDIDATES: &[&str] = &["sudo-rs", "sudo", "pkexec", "doas"];
    CANDIDATES
        .iter()
        .copied()
        .find(|candidate| find_in_path_env(candidate, path))
}

fn find_in_path_env(program: &str, path: Option<&std::ffi::OsStr>) -> bool {
    let Some(path) = path else {
        return false;
    };
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .any(|candidate| candidate.is_file() && is_executable(&candidate))
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
        let argv = configured_privilege(&None, &config).unwrap();
        assert_eq!(argv, vec!["sudo-rs"]);
    }

    #[test]
    fn parses_config_with_array_command() {
        let config: Config =
            toml::from_str(r#"privilege = { command = ["sudo-rs", "-E"] }"#).unwrap();
        let argv = configured_privilege(&None, &config).unwrap();
        assert_eq!(argv, vec!["sudo-rs", "-E"]);
    }

    #[test]
    fn flag_beats_config() {
        let config: Config = toml::from_str(r#"privilege = { command = "sudo-rs" }"#).unwrap();
        let argv = configured_privilege(&Some(vec!["pkexec".to_string()]), &config).unwrap();
        assert_eq!(argv, vec!["pkexec"]);
    }

    #[test]
    fn empty_flag_is_rejected() {
        let config = Config::default();
        let err = configured_privilege(&Some(Vec::new()), &config).unwrap_err();
        assert!(err.contains("--privilege"), "got: {err}");
    }

    #[test]
    fn empty_env_is_rejected() {
        let config = Config::default();
        let result = configured_privilege_from(&None, Some("   "), &config, None);
        assert!(result.is_err(), "empty env must be rejected");
        assert!(result.unwrap_err().contains("PACNIX_PRIVILEGE"));
        let ok = configured_privilege_from(&None, Some("sudo-rs"), &config, None).unwrap();
        assert_eq!(ok, vec!["sudo-rs"]);
    }

    #[test]
    fn defaults_to_detected_tool() {
        let config = Config::default();
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
        let argv = configured_privilege_from(
            &None,
            None,
            &config,
            Some(std::ffi::OsStr::new(dir.to_string_lossy().as_ref())),
        )
        .unwrap();
        assert_eq!(argv[0], "sudo-rs", "detection must find sudo-rs in PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_prefers_sudo_rs_without_touching_env() {
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
        let combined = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap());
        assert_eq!(
            detect_privilege_in(Some(std::ffi::OsStr::new(&combined))),
            Some("sudo-rs")
        );
        let empty: &std::ffi::OsStr = std::ffi::OsStr::new("");
        assert_eq!(detect_privilege_in(Some(empty)), None, "no tool -> none");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn split_words_handles_quoted_free_form() {
        assert_eq!(split_words("sudo-rs -E"), vec!["sudo-rs", "-E"]);
        assert_eq!(split_words("pkexec"), vec!["pkexec"]);
    }
}
