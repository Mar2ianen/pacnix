// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use pacnix_core::model::{
    Candidate, InstalledPackage, Source, TransactionOperation, TransactionPlan,
};
use pacnix_core::{ExecutionContext, PackageBackend};

use crate::rpc::{self, AurPackage};

pub struct AurBackend;

fn snapshot_url(package_base: &str, url_path: Option<&str>) -> String {
    match url_path {
        Some(path) => format!("https://aur.archlinux.org{path}"),
        None => format!("https://aur.archlinux.org/cgit/aur.git/snapshot/{package_base}.tar.gz"),
    }
}

fn build_dir(package: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("pacnix-aur-{package}"))
}

fn try_download_snapshot(
    tarball: &std::path::Path,
    package_base: &str,
    url_path: Option<&str>,
) -> Result<(), String> {
    let agent = ureq::Agent::new_with_defaults();
    let response = agent
        .get(&snapshot_url(package_base, url_path))
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

fn clone_snapshot(package_base: &str, dir: &std::path::Path) -> Result<(), String> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let status = std::process::Command::new("git")
        .args(["clone", "--depth", "1", "--single-branch"])
        .arg(format!("https://aur.archlinux.org/{package_base}.git"))
        .arg(dir)
        .status()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !status.success() {
        return Err(format!("failed to clone AUR repository {package_base}"));
    }
    if !dir.join("PKGBUILD").exists() {
        return Err(format!("AUR repository {package_base} has no PKGBUILD"));
    }
    Ok(())
}

fn fetch_snapshot(
    package: &str,
    package_base: &str,
    url_path: Option<&str>,
) -> Result<std::path::PathBuf, String> {
    let dir = build_dir(package);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tarball = dir.join(format!("{package_base}.tar.gz"));
    let mut last_err = None;
    for attempt in 1..=3 {
        match try_download_snapshot(&tarball, package_base, url_path) {
            Ok(()) => {
                last_err = None;
                break;
            }
            Err(e) => {
                last_err = Some(e);
                if attempt < 3 {
                    std::thread::sleep(std::time::Duration::from_millis(500 * attempt as u64));
                }
            }
        }
    }
    if last_err.is_some() {
        return match clone_snapshot(package_base, &dir) {
            Ok(()) => Ok(dir),
            Err(e) => Err(format!(
                "{}; git clone fallback: {e}",
                last_err.unwrap_or_default()
            )),
        };
    }
    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("--strip-components=1")
        .current_dir(&dir)
        .status()
        .map_err(|e| format!("failed to run tar: {e}"))?;
    if !status.success() {
        return Err(format!(
            "failed to extract AUR snapshot {package_base} for {package}"
        ));
    }
    std::fs::remove_file(&tarball).ok();
    if !dir.join("PKGBUILD").exists() {
        return Err(format!("AUR snapshot {package_base} has no PKGBUILD"));
    }
    Ok(dir)
}

fn current_arch() -> String {
    std::process::Command::new("uname")
        .arg("-m")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "x86_64".to_string())
}

fn parse_srcinfo_deps(srcinfo: &str, arch: &str, package: &str) -> Vec<String> {
    let mut deps: Vec<String> = Vec::new();
    let mut current_package: Option<&str> = None;
    for line in srcinfo.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("pkgname = ") {
            current_package = Some(rest.trim());
            continue;
        }
        for kind in ["depends", "makedepends", "checkdepends"] {
            let Some(rest) = line.strip_prefix(kind) else {
                continue;
            };
            let rest = match rest.strip_prefix(&format!("_{arch}")) {
                Some(r) => r,
                None => rest,
            };
            let Some(rest) = rest.strip_prefix(" = ") else {
                continue;
            };
            let name = rest
                .split(['>', '<', '=', '!'])
                .next()
                .unwrap_or(rest)
                .trim();
            if name.is_empty() {
                continue;
            }
            let in_section = match current_package {
                None => true,
                Some(pkg) => pkg == package,
            };
            if !in_section {
                continue;
            }
            if !deps.iter().any(|d| d == name) {
                deps.push(name.to_string());
            }
        }
    }
    deps
}

fn build_dependencies(dir: &std::path::Path, package: &str) -> Result<Vec<String>, String> {
    let output = std::process::Command::new("makepkg")
        .args(["--printsrcinfo"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to run makepkg --printsrcinfo: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "makepkg --printsrcinfo failed (status {})",
            output.status
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_srcinfo_deps(&text, &current_arch(), package))
}

fn build_package(package: &str, dir: &std::path::Path) -> Result<(), String> {
    let status = std::process::Command::new("makepkg")
        .args(["--noconfirm", "--needed"])
        .current_dir(dir)
        .status()
        .map_err(|e| format!("failed to run makepkg: {e}"))?;
    if !status.success() {
        return Err(format!("makepkg failed for {package} (status {status})"));
    }
    Ok(())
}

fn artifact_pkgname(path: &std::path::Path) -> Result<Option<String>, String> {
    let output = std::process::Command::new("tar")
        .args(["-xOf"])
        .arg(path)
        .arg(".PKGINFO")
        .output()
        .map_err(|e| format!("failed to inspect artifact {path:?}: {e}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text
        .lines()
        .find_map(|line| line.trim().strip_prefix("pkgname = "))
        .map(|name| name.trim().to_string()))
}

fn built_artifact(dir: &std::path::Path, package: &str) -> Result<std::path::PathBuf, String> {
    for path in std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            name.ends_with(".pkg.tar.zst")
                || name.ends_with(".pkg.tar.xz")
                || name.ends_with(".pkg.tar")
        })
    {
        if artifact_pkgname(&path)? == Some(package.to_string()) {
            return Ok(path);
        }
    }
    Err(format!("no built artifact for requested package {package}"))
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
        let (name, version, installed_at) = parse_desc_fields(&desc);
        if name.as_deref() == Some(package) {
            let Some(installed_at) = installed_at else {
                continue;
            };
            return Ok(Some((
                name.unwrap_or_else(|| package.to_string()),
                version,
                installed_at,
            )));
        }
    }
    Ok(None)
}

fn parse_desc_fields(desc: &str) -> (Option<String>, Option<String>, Option<i64>) {
    let mut name = None;
    let mut version = None;
    let mut installed_at = None;
    let lines: Vec<&str> = desc.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let field = lines[i];
        if field.starts_with('%') && field.ends_with('%') {
            let value = lines.get(i + 1).copied().unwrap_or("").to_string();
            match field {
                "%NAME%" => name = Some(value),
                "%VERSION%" => version = Some(value),
                "%INSTALLDATE%" => installed_at = value.parse::<i64>().ok(),
                _ => {}
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    (name, version, installed_at)
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
        let package_base = target
            .package_base
            .clone()
            .unwrap_or_else(|| target.name.clone());
        Ok(TransactionPlan {
            backend_ref: target.backend_ref.clone(),
            name: target.name.clone(),
            operations: vec![
                TransactionOperation::FetchAurSource {
                    package: target.name.clone(),
                    package_base: package_base.clone(),
                    url_path: target.url_path.clone(),
                },
                TransactionOperation::InstallAurBuildDeps {
                    package: target.name.clone(),
                },
                TransactionOperation::BuildAurPackage {
                    package: target.name.clone(),
                },
                TransactionOperation::InstallAurPackage {
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
            TransactionOperation::FetchAurSource {
                package,
                package_base,
                url_path,
            } => fetch_snapshot(package, package_base, url_path.as_deref()).map(|_| ()),
            TransactionOperation::InstallAurBuildDeps { package } => {
                let dir = build_dir(package);
                if !dir.join("PKGBUILD").exists() {
                    return Err(format!(
                        "{package}: PKGBUILD not fetched yet; install AUR via pacnix install"
                    ));
                }
                let deps = build_dependencies(&dir, package)?;
                if deps.is_empty() {
                    return Ok(());
                }
                let mut command = std::process::Command::new("pacman");
                if ctx.use_sudo {
                    let mut sudo = std::process::Command::new("sudo");
                    sudo.arg("pacman");
                    command = sudo;
                }
                command
                    .arg("-S")
                    .arg("--needed")
                    .arg("--asdeps")
                    .arg("--noconfirm");
                for dep in &deps {
                    command.arg(dep);
                }
                let status = command
                    .status()
                    .map_err(|e| format!("failed to run pacman -S: {e}"))?;
                if !status.success() {
                    return Err(format!(
                        "installing build deps for {package} failed (status {status})"
                    ));
                }
                Ok(())
            }
            TransactionOperation::BuildAurPackage { package } => {
                let dir = build_dir(package);
                if !dir.join("PKGBUILD").exists() {
                    return Err(format!(
                        "{package}: PKGBUILD not fetched yet; install AUR via pacnix install"
                    ));
                }
                build_package(package, &dir)
            }
            TransactionOperation::InstallAurPackage { package } => {
                let dir = build_dir(package);
                if !dir.join("PKGBUILD").exists() {
                    return Err(format!(
                        "{package}: PKGBUILD not fetched yet; install AUR via pacnix install"
                    ));
                }
                let artifact = built_artifact(&dir, package)?;
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
        let (name, version, installed_at) = parse_desc_fields(desc);
        assert_eq!(name.as_deref(), Some("mutt-wizard"));
        assert_eq!(version.as_deref(), Some("3.3.1-1"));
        assert_eq!(installed_at, Some(1754702000));
        let (name, version, installed_at) = parse_desc_fields("%NAME%\nfoo\n%VERSION%-\n");
        assert_eq!(name.as_deref(), Some("foo"));
        assert_eq!(version, None);
        assert_eq!(installed_at, None);
    }

    #[test]
    fn snapshot_url_uses_url_path_or_base() {
        assert_eq!(
            snapshot_url("hiddify", None),
            "https://aur.archlinux.org/cgit/aur.git/snapshot/hiddify.tar.gz"
        );
        assert_eq!(
            snapshot_url("nx", Some("/cgit/aur.git/snapshot/nx.tar.gz")),
            "https://aur.archlinux.org/cgit/aur.git/snapshot/nx.tar.gz"
        );
        assert_eq!(
            build_dir("hiddify").file_name().unwrap(),
            "pacnix-aur-hiddify"
        );
    }

    #[test]
    fn split_package_uses_base_for_fetch_and_name_for_artifact() {
        let plan = AurBackend
            .plan_install(&Candidate {
                source: Source::Aur,
                provider: "aur".into(),
                backend_ref: "aur/nxproxy".into(),
                name: "nxproxy".into(),
                version: Some("3.5.2-1".into()),
                description: None,
                package_base: Some("nx".into()),
                url_path: Some("/cgit/aur.git/snapshot/nx.tar.gz".into()),
            })
            .unwrap();
        let ops = &plan.operations;
        assert!(matches!(
            &ops[0],
            TransactionOperation::FetchAurSource {
                package,
                package_base,
                url_path,
            } if package == "nxproxy"
                && package_base == "nx"
                && url_path.as_deref() == Some("/cgit/aur.git/snapshot/nx.tar.gz")
        ));
        assert!(
            matches!(&ops[1], TransactionOperation::InstallAurBuildDeps { package } if package == "nxproxy")
        );
        assert!(
            matches!(&ops[2], TransactionOperation::BuildAurPackage { package } if package == "nxproxy")
        );
        assert!(
            matches!(&ops[3], TransactionOperation::InstallAurPackage { package } if package == "nxproxy")
        );
    }

    #[test]
    fn urlencode_queries() {
        assert_eq!(urlencode("hiddify"), "hiddify");
        assert_eq!(urlencode("foo bar"), "foo%20bar");
        assert_eq!(urlencode("foo/bar"), "foo%2Fbar");
    }

    #[test]
    fn srcinfo_includes_dep_kinds_and_arch_variants() {
        let srcinfo = "\
pkgbase = nx
pkgver = 3.5.2
pkgrel = 1
depends = zlib
makedepends = gcc
makedepends_x86_64 = extra-tool
checkdepends = python
checkdepends_x86_64 = ctest
pkgname = nx
depends = any-pkg
pkgname = nxproxy
depends = nx
";
        let deps = parse_srcinfo_deps(srcinfo, "x86_64", "nxproxy");
        for expected in ["zlib", "gcc", "extra-tool", "python", "ctest", "nx"] {
            assert!(
                deps.iter().any(|d| d == expected),
                "{expected} must be collected, got {deps:?}"
            );
        }
        assert!(
            !deps.contains(&"any-pkg".to_string()),
            "deps of another split package section must be excluded"
        );
        assert_eq!(
            deps.iter().filter(|d| d.as_str() == "nx").count(),
            1,
            "nx must appear once (dedup)"
        );
    }

    #[test]
    fn srcinfo_dedupes_across_sections() {
        let srcinfo = "\
pkgname = a
depends = libz
pkgname = b
depends = libz
makedepends_x86_64 = cmake
";
        let deps = parse_srcinfo_deps(srcinfo, "x86_64", "b");
        assert_eq!(deps.iter().filter(|d| d.as_str() == "libz").count(), 1);
    }

    #[test]
    fn built_artifact_requires_matching_pkgname() {
        let dir = std::env::temp_dir().join(format!("pacnix-aur-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let artifact = dir.join("nx-3.5.2-1-x86_64.pkg.tar");
        if artifact.exists() {
            let _ = std::fs::remove_file(&artifact);
        }
        std::fs::write(&artifact, "NOPE".as_bytes()).unwrap();
        let result = built_artifact(&dir, "nxproxy");
        assert!(result.is_err(), ".max() fallback must not be used");
        let _ = std::fs::remove_file(&artifact);
        let _ = std::fs::remove_dir(&dir);
    }
}
