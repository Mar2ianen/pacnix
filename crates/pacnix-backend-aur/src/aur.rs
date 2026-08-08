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

fn fetch_snapshot(package: &str) -> Result<std::path::PathBuf, String> {
    let dir = build_dir(package);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tarball = dir.join(format!("{package}.tar.gz"));
    let agent = ureq::Agent::new_with_defaults();
    let response = agent
        .get(&snapshot_url(package))
        .call()
        .map_err(|e| format!("AUR snapshot download failed: {e}"))?;
    let bytes = response
        .into_body()
        .read_to_vec()
        .map_err(|e| format!("failed to read AUR snapshot: {e}"))?;
    std::fs::write(&tarball, bytes).map_err(|e| e.to_string())?;
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
        _plan: &TransactionPlan,
        _before: &[InstalledPackage],
        _after: &[InstalledPackage],
    ) -> Result<Vec<InstalledPackage>, String> {
        Ok(Vec::new())
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
