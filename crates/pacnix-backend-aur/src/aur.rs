// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use pacnix_core::model::{
    Candidate, InstalledPackage, Source, TransactionOperation, TransactionPlan,
};
use pacnix_core::PackageBackend;

use crate::rpc::{self, AurPackage};

pub struct AurBackend;

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
            backend_ref: format!("aur/{}", target.name),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_queries() {
        assert_eq!(urlencode("hiddify"), "hiddify");
        assert_eq!(urlencode("foo bar"), "foo%20bar");
        assert_eq!(urlencode("foo/bar"), "foo%2Fbar");
    }
}