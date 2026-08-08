// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use pacnix_core::model::{Candidate, InstalledPackage, Provenance, Source};
use serde::Deserialize;

#[derive(Deserialize)]
struct ProfileList {
    #[serde(rename = "elements")]
    elements: serde_json::Map<String, serde_json::Value>,
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

pub fn parse_profile_list(output: &str) -> Result<Vec<InstalledPackage>, String> {
    let parsed: ProfileList = serde_json::from_str(output)
        .map_err(|e| format!("bad `nix profile list --json` output: {e}"))?;
    let mut pkgs = Vec::new();
    for (name, value) in &parsed.elements {
        let store = value
            .get("storePaths")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str());
        let attr_path = value.get("attrPath").and_then(|v| v.as_str());
        let source = value
            .get("originalUrl")
            .and_then(|v| v.as_str())
            .unwrap_or("nixpkgs");
        let backend_ref = match store {
            Some(path) => path.to_string(),
            None => format!("{source}#{name}"),
        };
        pkgs.push(InstalledPackage {
            source: Source::Nix,
            backend_ref,
            name: name.clone(),
            version: store.and_then(split_version),
            scope: attr_path.map(|a| a.to_string()),
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

pub fn parse_search(output: &str) -> Vec<Candidate> {
    let Ok(parsed) = serde_json::from_str::<std::collections::BTreeMap<String, SearchHit>>(output)
    else {
        return Vec::new();
    };
    parsed
        .into_iter()
        .filter_map(|(full_attr, hit)| {
            let name = hit
                .pname
                .or_else(|| full_attr.rsplit('.').next().map(|s| s.to_string()))?;
            Some(Candidate {
                source: Source::Nix,
                provider: "nixpkgs".to_string(),
                name,
                version: hit.version,
                description: hit.description,
            })
        })
        .collect()
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
        let candidates = parse_search(fixture);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "ripgrep");
        assert_eq!(candidates[0].provider, "nixpkgs");
        assert_eq!(candidates[0].version.as_deref(), Some("14.1.1"));
    }

    #[test]
    fn store_path_version_needs_leading_digit() {
        assert_eq!(
            split_version("/nix/store/abc-foo-1.2.3"),
            Some("1.2.3".into())
        );
        assert_eq!(split_version("/nix/store/abc-foo-latest"), None);
    }
}
