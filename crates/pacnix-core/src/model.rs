// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    Alpm,
    Aur,
    Nix,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Alpm => "alpm",
            Source::Aur => "aur",
            Source::Nix => "nix",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub source: Source,
    pub provider: String,
    pub backend_ref: String,
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
pub enum Provenance {
    Unknown,
    SyncKnown,
    Foreign,
    PacnixInstalled { source: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReceipt {
    pub package_name: String,
    pub installed_backend: String,
    pub installed_backend_ref: String,
    pub source: String,
    pub source_ref: String,
    pub version: Option<String>,
    pub installed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    pub source: Source,
    pub backend_ref: String,
    pub name: String,
    pub version: Option<String>,
    pub scope: Option<String>,
    pub installed_at: Option<i64>,
    pub provenance: Provenance,
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
    FetchAurSource { package: String },
    InstallPackage { package: String },
    RemovePackage { package: String },
    UpgradePackage { package: String },
    ProfileInstall { profile: String, attr: String },
    ProfileRemove { profile: String, attr: String },
    ProfileUpgrade { profile: String, element: String },
    SystemUpgrade { system: String },
}
