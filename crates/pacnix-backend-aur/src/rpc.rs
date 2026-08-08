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

#[derive(Debug, Deserialize)]
pub struct AurPackage {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Version")]
    pub version: Option<String>,
    #[serde(rename = "Description")]
    pub description: Option<String>,
    #[serde(rename = "URLPath")]
    pub url_path: Option<String>,
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
            name: p.name,
            version: p.version,
            description: p.description,
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
}
