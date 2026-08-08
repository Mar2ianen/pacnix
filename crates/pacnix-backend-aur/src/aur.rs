// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use pacnix_core::model::{
    Candidate, InstalledPackage, Source, TransactionOperation, TransactionPlan,
};
use pacnix_core::{ExecutionContext, PackageBackend};

use crate::rpc::{self, AurPackage};

pub struct AurBackend;

fn snapshot_url(package: &str) -> String {
    format!("https://aur.archlinux.org/cgit/aur.git/snapshot/{package}.tar.gz")
}

fn build_dir(package: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("pacnix-aur-{package}"))
}

fn try_download_snapshot(tarball: &std::path::Path, package: &str) -> Result<(), String> {
    let agent = ureq::Agent::new_with_defaults();
    let response = agent
        .get(&snapshot_url(package))
        .call()
        .map_err(|e| format!("AUR snapshot download failed: {e}"))?;
    let bytes = response
        .into_body()
        .read_to_vec()
        .map_err(|e| format!("failed to read AUR snapshot: {e}"))?;
    if bytes.len() < 2 || bytes[0] != 0x1f || bytes[1] != 0x8b {
        return Err(format!(
            "AUR snapshot download failed: not a gzip archive ({} bytes)",
            bytes.len()
        ));
    }
    std::fs::write(tarball, bytes).map_err(|e| e.to_string())
}

fn clone_snapshot(package: &str, dir: &std::path::Path) -> Result<(), String> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let status = std::process::Command::new("git")
        .args(["clone", "--depth", "1", "--single-branch"])
        .arg(format!("https://aur.archlinux.org/{package}.git"))
        .arg(dir)
        .status()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !status.success() {
        return Err(format!("failed to clone AUR repository {package}"));
    }
    if !dir.join("PKGBUILD").exists() {
        return Err(format!("AUR repository {package} has no PKGBUILD"));
    }
    Ok(())
}

fn fetch_snapshot(package: &str) -> Result<std::path::PathBuf, String> {
    let dir = build_dir(package);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tarball = dir.join(format!("{package}.tar.gz"));
    let mut last_err = String::new();
    for attempt in 1..=3 {
        match try_download_snapshot(&tarball, package) {
            Ok(()) => break,
            Err(e) => {
                last_err = e;
                if attempt < 3 {
                    std::thread::sleep(std::time::Duration::from_millis(500 * attempt as u64));
                }
            }
        }
    }
    if !last_err.is_empty() {
        match clone_snapshot(package, &dir) {
            Ok(()) => return Ok(dir),
            Err(e) => return Err(format!("{last_err}; git clone fallback: {e}")),
        }
    }
    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("--strip-components=1")
        .current_dir(&dir)
        .status()
        .map_err(|e| format!("failed to run tar: {e}"))?;
    if !status.success() {
        return Err(format!("failed to extract AUR snapshot {package}"));
    }
    std::fs::remove_file(&tarball).ok();
    if !dir.join("PKGBUILD").exists() {
        return Err(format!("AUR snapshot {package} has no PKGBUILD"));
    }
    Ok(dir)
}

fn build_package(package: &str, dir: &std::path::Path) -> Result<(), String> {
    let status = std::process::Command::new("makepkg")
        .args(["--noconfirm", "--syncdeps", "--needed"])
        .current_dir(dir)
        .status()
        .map_err(|e| format!("failed to run makepkg: {e}"))?;
    if !status.success() {
        return Err(format!("makepkg failed for {package} (status {status})"));
    }
    Ok(())
}

fn built_artifact(dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            name.ends_with(".pkg.tar.zst")
                || name.ends_with(".pkg.tar.xz")
                || name.ends_with(".pkg.tar")
        })
        .max()
        .ok_or_else(|| "no built package artifact found".into())
}

fn search_rpc(query: &str) -> Result<Vec<AurPackage>, String> {
    let url = format!(
        "https://aur.archlinux.org/rpc/v5/search/{}?by=name-desc",
        urlencode(query)
    );
    let agent = ureq::Agent::new_with_defaults();
    let body = agent
        .get(&url)
        .call()
        .map_err(|e| format!("AUR RPC failed: {e}"))?
        .into_body()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    rpc::search_from_json(&body)
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

fn installed_desc(package: &str) -> Result<Option<(String, Option<String>, i64)>, String> {
    let local = std::path::Path::new("/var/lib/pacman/local");
    for entry in std::fs::read_dir(local).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let desc_path = dir.join("desc");
        let desc = match std::fs::read_to_string(&desc_path) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let (name, version) = parse_desc_fields(&desc);
        if name.as_deref() == Some(package) {
            let installed_at = std::fs::metadata(&desc_path)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            return Ok(Some((
                name.unwrap_or_else(|| package.to_string()),
                version,
                installed_at,
            )));
        }
    }
    Ok(None)
}

fn parse_desc_fields(desc: &str) -> (Option<String>, Option<String>) {
    let mut name = None;
    let mut version = None;
    let lines: Vec<&str> = desc.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let field = lines[i];
        if field.starts_with('%') && field.ends_with('%') {
            let value = lines.get(i + 1).copied().unwrap_or("").to_string();
            match field {
                "%NAME%" => name = Some(value),
                "%VERSION%" => version = Some(value),
                _ => {}
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    (name, version)
}

impl PackageBackend for AurBackend {
    fn name(&self) -> &'static str {
        "aur"
    }

    fn source(&self) -> Source {
        Source::Aur
    }

    fn search(&self, query: &str) -> Result<Vec<Candidate>, String> {
        let packages = search_rpc(query)?;
        Ok(rpc::to_candidates(packages))
    }

    fn installed(&self) -> Result<Vec<InstalledPackage>, String> {
        Ok(Vec::new())
    }

    fn plan_install(&self, target: &Candidate) -> Result<TransactionPlan, String> {
        Ok(TransactionPlan {
            backend_ref: target.backend_ref.clone(),
            name: target.name.clone(),
            operations: vec![
                TransactionOperation::FetchAurSource {
                    package: target.name.clone(),
                },
                TransactionOperation::InstallPackage {
                    package: target.name.clone(),
                },
            ],
            requires_privilege: true,
        })
    }

    fn plan_remove(&self, target: &InstalledPackage) -> Result<TransactionPlan, String> {
        Ok(TransactionPlan {
            backend_ref: target.backend_ref.clone(),
            name: target.name.clone(),
            operations: vec![TransactionOperation::RemovePackage {
                package: target.name.clone(),
            }],
            requires_privilege: true,
        })
    }

    fn plan_upgrade(&self, target: &InstalledPackage) -> Result<TransactionPlan, String> {
        Ok(TransactionPlan {
            backend_ref: target.backend_ref.clone(),
            name: target.name.clone(),
            operations: vec![TransactionOperation::UpgradePackage {
                package: target.name.clone(),
            }],
            requires_privilege: true,
        })
    }

    fn plan_upgrade_all(&self) -> Result<TransactionPlan, String> {
        Err("aur: upgrade all not implemented yet".into())
    }

    fn receipt_instances(
        &self,
        plan: &TransactionPlan,
        _before: &[InstalledPackage],
        _after: &[InstalledPackage],
    ) -> Result<Vec<InstalledPackage>, String> {
        let mut receipts = Vec::new();
        if let Some((name, version, installed_at)) = installed_desc(&plan.name)? {
            receipts.push(InstalledPackage {
                source: Source::Alpm,
                backend_ref: format!("local/{}", plan.name),
                name,
                version,
                scope: None,
                installed_at: Some(installed_at),
                provenance: pacnix_core::Provenance::Foreign,
            });
        }
        Ok(receipts)
    }

    fn execute_operation(
        &self,
        op: &TransactionOperation,
        ctx: &ExecutionContext,
    ) -> Result<(), String> {
        match op {
            TransactionOperation::FetchAurSource { package } => fetch_snapshot(package).map(|_| ()),
            TransactionOperation::InstallPackage { package } => {
                let dir = build_dir(package);
                if !dir.join("PKGBUILD").exists() {
                    return Err(format!(
                        "{package}: PKGBUILD not fetched yet; install AUR via pacnix install"
                    ));
                }
                build_package(package, &dir)?;
                let artifact = built_artifact(&dir)?;
                let mut command = std::process::Command::new("pacman");
                if ctx.use_sudo {
                    let mut sudo = std::process::Command::new("sudo");
                    sudo.arg("pacman");
                    command = sudo;
                }
                let status = command
                    .args(["-U", "--noconfirm"])
                    .arg(&artifact)
                    .status()
                    .map_err(|e| format!("failed to run pacman -U: {e}"))?;
                if !status.success() {
                    return Err(format!("pacman -U {package} failed with status {status}"));
                }
                Ok(())
            }
            _ => Err(format!("aur: unsupported operation for execution: {op:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_desc_fields() {
        let desc = "%NAME%\nmutt-wizard\n\n%VERSION%\n3.3.1-1\n\n%INSTALLDATE%\n1754702000\n";
        let (name, version) = parse_desc_fields(desc);
        assert_eq!(name.as_deref(), Some("mutt-wizard"));
        assert_eq!(version.as_deref(), Some("3.3.1-1"));
        let (name, version) = parse_desc_fields("%NAME%\nfoo\n%VERSION%-\n");
        assert_eq!(name.as_deref(), Some("foo"));
        assert_eq!(version, None);
    }

    #[test]
    fn snapshot_url_is_predictable() {
        assert_eq!(
            snapshot_url("hiddify"),
            "https://aur.archlinux.org/cgit/aur.git/snapshot/hiddify.tar.gz"
        );
        assert_eq!(
            build_dir("hiddify").file_name().unwrap(),
            "pacnix-aur-hiddify"
        );
    }

    #[test]
    fn urlencode_queries() {
        assert_eq!(urlencode("hiddify"), "hiddify");
        assert_eq!(urlencode("foo bar"), "foo%20bar");
        assert_eq!(urlencode("foo/bar"), "foo%2Fbar");
    }
}
