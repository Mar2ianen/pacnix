// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use pacnix_core::model::{Candidate, InstalledPackage, Provenance, Source};
use serde::Deserialize;

#[derive(Deserialize)]
struct ProfileList {
    #[serde(rename = "elements")]
    elements: serde_json::Map<String, serde_json::Value>,
}

/// A single element of `nix profile list --json` with the fields needed to
/// identify it for upgrades: the element name (key), the original flake URL,
/// the locked URL (embedding the pinned revision) and the store paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileElement {
    pub name: String,
    pub original_url: Option<String>,
    pub locked_url: Option<String>,
    pub store_path: Option<String>,
    pub attr_path: Option<String>,
}

#[derive(Deserialize)]
struct SearchHit {
    #[serde(default)]
    pname: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// Parses `nix profile list --json` into the raw elements, keeping the
/// flake URLs that `outdated` needs to detect newer revisions.
pub fn parse_profile_elements(output: &str) -> Result<Vec<ProfileElement>, String> {
    let parsed: ProfileList = serde_json::from_str(output)
        .map_err(|e| format!("bad `nix profile list --json` output: {e}"))?;
    let mut elements = Vec::new();
    for (name, value) in &parsed.elements {
        let store_path = value
            .get("storePaths")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let original_url = value
            .get("originalUrl")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let locked_url = value
            .get("url")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let attr_path = value
            .get("attrPath")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        elements.push(ProfileElement {
            name: name.clone(),
            original_url,
            locked_url,
            store_path,
            attr_path,
        });
    }
    Ok(elements)
}

/// Extracts the pinned revision of a `nix flake metadata <url> --json`
/// output, used to compare against the revision in the locked element URL.
pub fn flake_locked_rev(output: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct Metadata {
        locked: Locked,
    }
    #[derive(Deserialize)]
    struct Locked {
        #[serde(default)]
        rev: Option<String>,
    }
    let parsed: Metadata = serde_json::from_str(output)
        .map_err(|e| format!("bad `nix flake metadata --json` output: {e}"))?;
    parsed
        .locked
        .rev
        .ok_or_else(|| "nix flake metadata: locked revision missing".to_string())
}

/// Splits the pinned revision out of a locked element URL, e.g.
/// `github:owner/repo/abc123?narHash=...` -> `abc123`. URLs without a
/// revision component (`github:owner/repo`) return None.
pub fn locked_rev_of(url: &str) -> Option<String> {
    let (before, rev) = url.rsplit_once('/')?;
    if !before.contains('/') {
        return None;
    }
    let rev = rev.split('?').next()?.trim();
    if rev.is_empty() || rev.contains(':') {
        return None;
    }
    Some(rev.to_string())
}

pub fn parse_profile_list(output: &str) -> Result<Vec<InstalledPackage>, String> {
    let mut pkgs = Vec::new();
    for element in parse_profile_elements(output)? {
        let store = element.store_path.as_deref();
        let source = element.original_url.as_deref().unwrap_or("nixpkgs");
        let backend_ref = match store {
            Some(path) => path.to_string(),
            None => format!("{source}#{}", element.name),
        };
        pkgs.push(InstalledPackage {
            source: Source::Nix,
            backend_ref,
            name: element.name,
            version: store.and_then(split_version),
            scope: element.attr_path,
            installed_at: None,
            provenance: Provenance::Unknown,
        });
    }
    Ok(pkgs)
}

fn split_version(store_path: &str) -> Option<String> {
    let component = store_path.rsplit('/').next()?;
    let (_, version) = component.rsplit_once('-')?;
    if version.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        Some(version.to_string())
    } else {
        None
    }
}
pub fn parse_search(output: &str) -> Result<Vec<Candidate>, String> {
    let parsed: std::collections::BTreeMap<String, SearchHit> =
        serde_json::from_str(output).map_err(|e| format!("bad `nix search --json` output: {e}"))?;
    let mut candidates = Vec::new();
    for (full_attr, hit) in parsed {
        let Some(name) = hit
            .pname
            .or_else(|| full_attr.rsplit('.').next().map(|s| s.to_string()))
        else {
            continue;
        };
        candidates.push(Candidate {
            source: Source::Nix,
            provider: "nixpkgs".to_string(),
            backend_ref: format!("nixpkgs#{full_attr}"),
            name,
            version: hit.version,
            description: hit.description,
            package_base: None,
            url_path: None,
        });
    }
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_profile_list_with_multiple_elements() {
        let fixture = r#"{
            "elements": {
                "ayugram-desktop": {
                    "active": true,
                    "attrPath": "packages.x86_64-linux.default",
                    "originalUrl": "github:Mar2ianen/ayugram-desktop",
                    "priority": 5,
                    "storePaths": ["/nix/store/4jzf58snfrpy30fv70cvlvxj8vhbv0za-ayugram-desktop-7.0.4"],
                    "url": "github:Mar2ianen/ayugram-desktop/abc"
                },
                "helium-more": {
                    "active": true,
                    "attrPath": "packages.x86_64-linux.helium",
                    "originalUrl": "github:oxcl/nix-flake-helium-browser",
                    "priority": 5,
                    "storePaths": ["/nix/store/0zpnsj2iilszyxpn8962l1ciddz0yhv5-helium-0.15.2.1"]
                }
            },
            "version": 3
        }"#;
        let pkgs = parse_profile_list(fixture).unwrap();
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].name, "ayugram-desktop");
        assert_eq!(pkgs[0].version.as_deref(), Some("7.0.4"));
        assert_eq!(
            pkgs[0].backend_ref,
            "/nix/store/4jzf58snfrpy30fv70cvlvxj8vhbv0za-ayugram-desktop-7.0.4"
        );
        assert_eq!(pkgs[1].name, "helium-more");
        assert_eq!(pkgs[1].version.as_deref(), Some("0.15.2.1"));
        assert_eq!(
            pkgs[1].scope.as_deref(),
            Some("packages.x86_64-linux.helium")
        );
    }

    #[test]
    fn parses_search_output() {
        let fixture = r#"{
            "legacyPackages.x86_64-linux.ripgrep": {
                "pname": "ripgrep",
                "version": "14.1.1",
                "description": "A utility that combines the usability of The Silver Searcher"
            }
        }"#;
        let candidates = parse_search(fixture).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "ripgrep");
        assert_eq!(candidates[0].provider, "nixpkgs");
        assert_eq!(
            candidates[0].backend_ref,
            "nixpkgs#legacyPackages.x86_64-linux.ripgrep"
        );
        assert_eq!(candidates[0].version.as_deref(), Some("14.1.1"));
    }

    #[test]
    fn search_keeps_nested_attr_installable() {
        let fixture = r#"{
            "legacyPackages.x86_64-linux.gnome3.vala": {
                "pname": "vala",
                "version": "0.56.0",
                "description": null
            }
        }"#;
        let candidates = parse_search(fixture).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].backend_ref,
            "nixpkgs#legacyPackages.x86_64-linux.gnome3.vala"
        );
        assert_eq!(candidates[0].name, "vala");
    }

    #[test]
    fn search_rejects_malformed_json() {
        assert!(parse_search("not json").is_err());
    }

    #[test]
    fn store_path_version_needs_leading_digit() {
        assert_eq!(
            split_version("/nix/store/abc-foo-1.2.3"),
            Some("1.2.3".into())
        );
        assert_eq!(split_version("/nix/store/abc-foo-latest"), None);
    }

    #[test]
    fn profile_elements_keep_flake_urls_and_attr_path() {
        let fixture = r#"{
            "elements": {
                "ayugram-desktop": {
                    "active": true,
                    "attrPath": "packages.x86_64-linux.default",
                    "originalUrl": "github:Mar2ianen/ayugram-desktop",
                    "priority": 5,
                    "storePaths": ["/nix/store/4jzf58snfrpy30fv70cvlvxj8vhbv0za-ayugram-desktop-7.0.4"],
                    "url": "github:Mar2ianen/ayugram-desktop/cdbb75c?narHash=sha256-x"
                }
            },
            "version": 3
        }"#;
        let elements = parse_profile_elements(fixture).unwrap();
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].name, "ayugram-desktop");
        assert_eq!(
            elements[0].original_url.as_deref(),
            Some("github:Mar2ianen/ayugram-desktop")
        );
        assert_eq!(
            elements[0].locked_url.as_deref(),
            Some("github:Mar2ianen/ayugram-desktop/cdbb75c?narHash=sha256-x")
        );
        assert_eq!(
            elements[0].attr_path.as_deref(),
            Some("packages.x86_64-linux.default")
        );
        assert_eq!(
            elements[0].store_path.as_deref(),
            Some("/nix/store/4jzf58snfrpy30fv70cvlvxj8vhbv0za-ayugram-desktop-7.0.4")
        );
    }

    #[test]
    fn locked_rev_of_splits_revision_from_url() {
        assert_eq!(
            locked_rev_of("github:owner/repo/cdbb75c?narHash=sha256-x"),
            Some("cdbb75c".into())
        );
        assert_eq!(
            locked_rev_of("github:owner/repo/cdbb75c"),
            Some("cdbb75c".into())
        );
        assert_eq!(locked_rev_of("github:owner/repo"), None);
        assert_eq!(locked_rev_of("flake:nixpkgs"), None);
    }

    #[test]
    fn flake_locked_rev_parses_metadata_json() {
        let fixture = r#"{
            "description": "x",
            "locked": {
                "rev": "934c50afe8c33cdd6d403691937bd955a2d1b334",
                "type": "github"
            }
        }"#;
        assert_eq!(
            flake_locked_rev(fixture).unwrap(),
            "934c50afe8c33cdd6d403691937bd955a2d1b334"
        );
        assert!(flake_locked_rev(r#"{"locked": {}}"#).is_err());
        assert!(flake_locked_rev("not json").is_err());
    }
}
