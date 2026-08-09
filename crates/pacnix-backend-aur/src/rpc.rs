// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use pacnix_core::model::{Candidate, Source};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct RpcResponse {
    pub version: u32,
    #[serde(rename = "type")]
    pub kind: String,
    pub resultcount: u32,
    pub results: Vec<AurPackage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AurPackage {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Version")]
    pub version: Option<String>,
    #[serde(rename = "Description")]
    pub description: Option<String>,
    #[serde(rename = "URLPath")]
    pub url_path: Option<String>,
    #[serde(rename = "PackageBase")]
    pub package_base: Option<String>,
    #[serde(rename = "Depends", default)]
    pub depends: Vec<String>,
    #[serde(rename = "MakeDepends", default)]
    pub make_depends: Vec<String>,
    #[serde(rename = "CheckDepends", default)]
    pub check_depends: Vec<String>,
}

pub fn info_by_name(names: &[String]) -> Result<Vec<AurPackage>, String> {
    let mut url = String::from("https://aur.archlinux.org/rpc/v5/info?");
    for name in names {
        url.push_str("arg[]=");
        url.push_str(name);
        url.push('&');
    }
    let agent = ureq::Agent::new_with_defaults();
    let body = agent
        .get(&url)
        .call()
        .map_err(|e| format!("AUR RPC failed: {e}"))?
        .into_body()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    info_from_json(&body)
}

pub fn info_from_json(json: &str) -> Result<Vec<AurPackage>, String> {
    let response: RpcResponse = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if response.kind == "error" {
        return Err("AUR RPC error response".into());
    }
    Ok(response.results)
}

pub fn existing_names(names: &[String]) -> Result<Vec<String>, String> {
    let mut existing = Vec::new();
    for chunk in names.chunks(50) {
        existing.extend(info_by_name(chunk)?.into_iter().map(|p| p.name));
    }
    existing.sort();
    existing.dedup();
    Ok(existing)
}

pub fn search_from_json(json: &str) -> Result<Vec<AurPackage>, String> {
    let response: RpcResponse = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if response.kind == "error" {
        return Err("AUR RPC error response".into());
    }
    Ok(response.results)
}

pub fn to_candidates(packages: Vec<AurPackage>) -> Vec<Candidate> {
    packages
        .into_iter()
        .map(|p| Candidate {
            source: Source::Aur,
            provider: "aur".to_string(),
            backend_ref: format!("aur/{}", p.name),
            name: p.name,
            version: p.version,
            description: p.description,
            package_base: p.package_base,
            url_path: p.url_path,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rpc_response() {
        let json = r#"{
            "version": 5,
            "type": "search",
            "resultcount": 2,
            "results": [
                {"Name": "hiddify", "Version": "1.0-1", "Description": "Proxy", "URLPath": "/cgit/aur.git/snapshot/hiddify.tar.gz"},
                {"Name": "hiddify-bin", "Version": "1.0-2", "Description": null, "URLPath": "/cgit/aur.git/snapshot/hiddify-bin.tar.gz"}
            ]
        }"#;
        let packages = search_from_json(json).unwrap();
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].name, "hiddify");
        assert_eq!(packages[0].description.as_deref(), Some("Proxy"));
        assert!(packages[1].description.is_none());
    }

    #[test]
    fn rejects_error_response() {
        let json = r#"{"version": 5, "type": "error", "resultcount": 0, "results": []}"#;
        assert!(search_from_json(json).is_err());
    }

    #[test]
    fn parses_info_dependencies() {
        let json = r#"{
            "version": 5,
            "type": "info",
            "resultcount": 1,
            "results": [{
                "Name": "foo",
                "Version": "1.0-1",
                "Description": null,
                "URLPath": "/cgit/aur.git/snapshot/foo.tar.gz",
                "PackageBase": "foo",
                "Depends": ["libx11", "gcc>=13"],
                "MakeDepends": ["cmake"],
                "CheckDepends": ["python"]
            }]
        }"#;
        let packages = info_from_json(json).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].depends, vec!["libx11", "gcc>=13"]);
        assert_eq!(packages[0].make_depends, vec!["cmake"]);
        assert_eq!(packages[0].check_depends, vec!["python"]);
    }

    #[test]
    fn missing_dep_fields_default_to_empty() {
        let json = r#"{
            "version": 5,
            "type": "search",
            "resultcount": 1,
            "results": [{"Name": "bar", "Version": "1.0-1", "Description": null, "URLPath": null, "PackageBase": "bar"}]
        }"#;
        let packages = search_from_json(json).unwrap();
        assert!(packages[0].depends.is_empty());
        assert!(packages[0].make_depends.is_empty());
        assert!(packages[0].check_depends.is_empty());
    }
}
