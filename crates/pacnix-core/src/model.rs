// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    Alpm,
    Aur,
    Nix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub source: Source,
    pub provider: String,
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
}

impl Candidate {
    pub fn installed_label(&self) -> String {
        format!("{}/{}", self.provider, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    pub source: Source,
    pub backend_ref: String,
    pub name: String,
    pub version: Option<String>,
    pub scope: Option<String>,
    pub installed_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetSpec {
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionPlan {
    pub backend_ref: String,
    pub name: String,
    pub operations: Vec<TransactionOperation>,
    pub requires_privilege: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionOperation {
    InstallPackage { package: String },
    RemovePackage { package: String },
    UpgradePackage { package: String },
    ProfileInstall { profile: String, attr: String },
    ProfileRemove { profile: String, attr: String },
    ProfileUpgrade { profile: String },
}