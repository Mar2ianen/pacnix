// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use std::collections::{HashMap, HashSet};

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
    if let Some(dir) = std::env::var_os("PACNIX_AUR_BUILD_DIR") {
        let base = std::path::PathBuf::from(dir);
        return base.join(format!("pacnix-aur-{package}"));
    }
    let cache = match std::env::var_os("XDG_CACHE_HOME") {
        Some(dir) if !dir.is_empty() => std::path::PathBuf::from(dir),
        _ => {
            let home = std::env::var_os("HOME").unwrap_or_default();
            std::path::PathBuf::from(home).join(".cache")
        }
    };
    cache.join("pacnix").join("aur").join(package)
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

/// Strips a version constraint from a dependency string, e.g.
/// `gcc>=13` -> `gcc`, `libx11` -> `libx11`.
fn dep_name(dep: &str) -> String {
    dep.split(['>', '<', '=', '!'])
        .next()
        .unwrap_or(dep)
        .trim()
        .to_string()
}

/// Keeps packages whose version is newer than what `installed` reports,
/// skipping names pacman does not know (e.g. packages removed by hand).
fn filter_outdated(
    packages: Vec<AurPackage>,
    installed: &dyn Fn(&str) -> Option<String>,
) -> Vec<AurPackage> {
    packages
        .into_iter()
        .filter(|pkg| {
            installed(&pkg.name).is_some_and(|current| {
                pacnix_core::vercmp(&current, pkg.version.as_deref().unwrap_or_default()) < 0
            })
        })
        .collect()
}

/// All build-relevant dependency names of a package: runtime `depends`,
/// `makedepends` and `checkdepends`, deduplicated and constraint-stripped.
fn dep_names_of(pkg: &AurPackage) -> Vec<String> {
    let mut deps: Vec<String> = Vec::new();
    for raw in pkg
        .depends
        .iter()
        .chain(pkg.make_depends.iter())
        .chain(pkg.check_depends.iter())
    {
        let name = dep_name(raw);
        if !name.is_empty() && !deps.iter().any(|d| d == &name) {
            deps.push(name);
        }
    }
    deps
}

type InfoFn = dyn Fn(&[String]) -> Result<Vec<AurPackage>, String>;

/// Expands the AUR-only dependency graph of `roots`, deps-first (every root
/// itself comes last, roots in the order given). `info(names)` returns the
/// AUR packages among the given names; repository packages are simply absent.
/// It is called in one batch per level, so a many-package upgrade stays a
/// handful of requests. `installed(name)` reports whether pacman already
/// satisfies it. A dependency cycle fails the expansion instead of looping.
fn expand_chain(
    roots: &[String],
    info: &InfoFn,
    installed: &dyn Fn(&str) -> bool,
) -> Result<Vec<AurPackage>, String> {
    let mut levels: Vec<Vec<AurPackage>> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut frontier: Vec<(String, Vec<String>)> = roots
        .iter()
        .map(|root| {
            seen.insert(root.clone());
            (root.clone(), Vec::new())
        })
        .collect();
    while !frontier.is_empty() {
        let names: Vec<String> = frontier.iter().map(|(name, _)| name.clone()).collect();
        let mut by_name: HashMap<String, AurPackage> = HashMap::new();
        for chunk in names.chunks(50) {
            for pkg in info(chunk)? {
                by_name.insert(pkg.name.clone(), pkg);
            }
        }
        let mut next: Vec<(String, Vec<String>)> = Vec::new();
        let mut level: Vec<AurPackage> = Vec::new();
        for (name, path) in frontier {
            let Some(pkg) = by_name.get(&name).cloned() else {
                continue;
            };
            level.push(pkg);
            for dep in dep_names_of(by_name.get(&name).unwrap()) {
                if installed(&dep) {
                    continue;
                }
                if let Some(pos) = path.iter().position(|p| p == &dep) {
                    let mut cycle: Vec<&str> = path[pos..].iter().map(String::as_str).collect();
                    cycle.push(name.as_str());
                    cycle.push(dep.as_str());
                    return Err(format!("dependency cycle: {}", cycle.join(" -> ")));
                }
                if seen.contains(&dep) {
                    continue;
                }
                seen.insert(dep.clone());
                let mut child_path = path.clone();
                child_path.push(name.clone());
                next.push((dep, child_path));
            }
        }
        if !level.is_empty() {
            levels.push(level);
        }
        frontier = next;
    }
    let mut order: Vec<AurPackage> = Vec::new();
    for level in levels.into_iter().rev() {
        order.extend(level);
    }
    Ok(order)
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

impl AurBackend {
    /// Shared walk for installs and upgrades: expands the AUR-only graph of
    /// `roots` and plans each node in dependency order.
    fn expand_and_plan(&self, roots: &[String]) -> Result<Vec<TransactionPlan>, String> {
        let chain = expand_chain(roots, &|names| rpc::info_by_name(names), &|name| {
            installed_desc(name).ok().flatten().is_some()
        })?;
        if chain.is_empty() {
            return Err("not found in AUR".into());
        }
        let mut plans = Vec::new();
        for pkg in chain {
            let candidate = rpc::to_candidates(vec![pkg]).remove(0);
            plans.push(self.plan_install(&candidate)?);
        }
        Ok(plans)
    }
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

    fn outdated(&self, installed: &[String]) -> Result<Vec<Candidate>, String> {
        let mut found: Vec<AurPackage> = Vec::new();
        for chunk in installed.chunks(50) {
            found.extend(rpc::info_by_name(chunk)?);
        }
        let outdated = filter_outdated(found, &|name| {
            installed_desc(name)
                .ok()
                .flatten()
                .and_then(|(_, version, _)| version)
        });
        Ok(rpc::to_candidates(outdated))
    }

    fn plan_upgrade_chain(&self, targets: &[Candidate]) -> Result<Vec<TransactionPlan>, String> {
        let roots: Vec<String> = targets.iter().map(|t| t.name.clone()).collect();
        self.expand_and_plan(&roots)
    }

    fn plan_install_chain(&self, target: &Candidate) -> Result<Vec<TransactionPlan>, String> {
        self.expand_and_plan(std::slice::from_ref(&target.name))
    }

    fn plan_upgrade_all(&self) -> Result<TransactionPlan, String> {
        Err("aur: upgrade all not implemented yet".into())
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
                let mut command = ctx.build_command("pacman")?;
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
                let status = ctx
                    .build_command("pacman")?
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
            "hiddify",
            "build dir must live under a persistent cache, not tmpfs"
        );
        assert!(build_dir("hiddify").starts_with(std::env::var("HOME").unwrap()));
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

    fn pkg(name: &str, deps: &[&str]) -> AurPackage {
        AurPackage {
            name: name.to_string(),
            version: Some("1.0-1".to_string()),
            description: None,
            url_path: None,
            package_base: Some(name.to_string()),
            depends: deps.iter().map(|d| d.to_string()).collect(),
            make_depends: Vec::new(),
            check_depends: Vec::new(),
        }
    }

    fn names(packages: &[AurPackage]) -> Vec<String> {
        packages.iter().map(|p| p.name.clone()).collect()
    }

    #[test]
    fn dep_name_strips_version_constraints() {
        assert_eq!(dep_name("gcc>=13"), "gcc");
        assert_eq!(dep_name("libx11"), "libx11");
        assert_eq!(dep_name("libpng!=1.2"), "libpng");
        assert_eq!(dep_name("  curl<2 "), "curl");
    }

    #[test]
    fn expand_chain_orders_deps_first() {
        let table = [
            pkg("a", &["b"]),
            pkg("b", &["c"]),
            pkg("c", &[]),
            pkg("d", &[]),
        ];
        let find = move |names: &[String]| {
            Ok(table
                .iter()
                .filter(|p| names.iter().any(|n| n == &p.name))
                .cloned()
                .collect::<Vec<_>>())
        };
        let chain = expand_chain(&["a".to_string()], &find, &|_| false).unwrap();
        assert_eq!(names(&chain), vec!["c", "b", "a"]);
    }

    #[test]
    fn expand_chain_dedupes_diamond() {
        let table = [
            pkg("a", &["b", "c"]),
            pkg("b", &["d"]),
            pkg("c", &["d"]),
            pkg("d", &[]),
        ];
        let find = move |names: &[String]| {
            Ok(table
                .iter()
                .filter(|p| names.iter().any(|n| n == &p.name))
                .cloned()
                .collect::<Vec<_>>())
        };
        let chain = expand_chain(&["a".to_string()], &find, &|_| false).unwrap();
        let order = names(&chain);
        assert_eq!(order.last().unwrap(), "a", "target must be installed last");
        assert_eq!(order.iter().filter(|n| n.as_str() == "d").count(), 1);
        assert_eq!(order.len(), 4);
    }

    #[test]
    fn expand_chain_reports_cycles() {
        let table = [pkg("a", &["b"]), pkg("b", &["a"])];
        let find = move |names: &[String]| {
            Ok(table
                .iter()
                .filter(|p| names.iter().any(|n| n == &p.name))
                .cloned()
                .collect::<Vec<_>>())
        };
        let err = expand_chain(&["a".to_string()], &find, &|_| false).unwrap_err();
        assert!(err.contains("a -> b -> a"), "got: {err}");
    }

    #[test]
    fn expand_chain_skips_installed_and_repo_deps() {
        let table = [pkg("a", &["b", "c"]), pkg("c", &[])];
        let find = move |names: &[String]| {
            Ok(table
                .iter()
                .filter(|p| names.iter().any(|n| n == &p.name))
                .cloned()
                .collect::<Vec<_>>())
        };
        let chain = expand_chain(&["a".to_string()], &find, &|name| name == "b").unwrap();
        assert_eq!(names(&chain), vec!["c", "a"]);
    }

    #[test]
    fn expand_chain_propagates_network_errors() {
        let err = expand_chain(
            &["a".to_string()],
            &|_| Err("network down".to_string()),
            &|_| false,
        )
        .unwrap_err();
        assert!(err.contains("network down"), "got: {err}");
    }

    #[test]
    fn aur_plan_chain_puts_target_last() {
        let table = [pkg("a", &["b"]), pkg("b", &[])];
        let find = move |names: &[String]| {
            Ok(table
                .iter()
                .filter(|p| names.iter().any(|n| n == &p.name))
                .cloned()
                .collect::<Vec<_>>())
        };
        let chain = expand_chain(&["a".to_string()], &find, &|_| false).unwrap();
        assert_eq!(chain.last().unwrap().name, "a");
        assert_eq!(chain[0].name, "b");
    }

    #[test]
    fn expand_chain_multi_root_shares_deps() {
        let table = [pkg("a", &["b"]), pkg("c", &["b"]), pkg("b", &[])];
        let find = move |names: &[String]| {
            Ok(table
                .iter()
                .filter(|p| names.iter().any(|n| n == &p.name))
                .cloned()
                .collect::<Vec<_>>())
        };
        let roots = ["a".to_string(), "c".to_string()];
        let chain = expand_chain(&roots, &find, &|_| false).unwrap();
        let order = names(&chain);
        assert_eq!(
            order.iter().filter(|n| n.as_str() == "b").count(),
            1,
            "shared dep must be planned exactly once"
        );
        let pos_b = order.iter().position(|n| n == "b").unwrap();
        let pos_a = order.iter().position(|n| n == "a").unwrap();
        let pos_c = order.iter().position(|n| n == "c").unwrap();
        assert!(
            pos_b < pos_a && pos_b < pos_c,
            "dep before both roots: {order:?}"
        );
    }

    #[test]
    fn filter_outdated_keeps_only_newer_versions() {
        let mut newest = pkg("newer", &[]);
        newest.version = Some("2.0-1".to_string());
        let equal = pkg("equal", &[]);
        let unknown = pkg("unknown", &[]);
        let table = vec![newest, equal, unknown];
        let installed = |name: &str| match name {
            "newer" => Some("1.0-1".to_string()),
            "equal" => Some("1.0-1".to_string()),
            _ => None,
        };
        let outdated = filter_outdated(table, &installed);
        assert_eq!(names(&outdated), vec!["newer"]);
    }
}
