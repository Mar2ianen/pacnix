// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use crate::model::{Candidate, InstalledPackage, Source, TransactionOperation, TransactionPlan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Privilege {
    /// Run commands directly as the current user.
    Direct,
    /// Elevate via the given argv prefix, e.g. `["sudo-rs"]` or
    /// `["pkexec", "--user", "root"]`.
    Elevate(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionContext {
    pub privilege: Privilege,
}

impl ExecutionContext {
    /// Builds a command that executes `program`, optionally wrapped in the
    /// privilege argv. `Elevate` with an empty argv is rejected so a
    /// privileged plan can never silently run unprivileged.
    pub fn build_command(&self, program: &str) -> Result<std::process::Command, String> {
        match &self.privilege {
            Privilege::Direct => Ok(std::process::Command::new(program)),
            Privilege::Elevate(argv) if argv.is_empty() => Err(format!(
                "privilege argv must name a command, got empty for {program}"
            )),
            Privilege::Elevate(argv) => {
                let mut command = std::process::Command::new(&argv[0]);
                command.args(&argv[1..]);
                command.arg(program);
                Ok(command)
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UpgradeImpact {
    pub entries: Vec<UpgradeImpactEntry>,
}

#[derive(Debug, Clone)]
pub struct UpgradeImpactEntry {
    pub name: String,
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
}

pub trait PackageBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn source(&self) -> Source;
    fn search(&self, query: &str) -> Result<Vec<Candidate>, String>;
    fn installed(&self) -> Result<Vec<InstalledPackage>, String>;
    fn plan_install(&self, target: &Candidate) -> Result<TransactionPlan, String>;
    /// Plans the target together with its non-repository dependencies,
    /// deps-first. Backends without dependency graphs (e.g. alpm) install the
    /// target alone; backends like aur override this with recursive expansion.
    fn plan_install_chain(&self, target: &Candidate) -> Result<Vec<TransactionPlan>, String> {
        Ok(vec![self.plan_install(target)?])
    }
    fn plan_remove(&self, target: &InstalledPackage) -> Result<TransactionPlan, String>;
    fn plan_upgrade(&self, target: &InstalledPackage) -> Result<TransactionPlan, String>;
    /// Candidates among `installed` with a newer version available. Defaults
    /// to none: alpm covers the repos through `plan_upgrade_all`, only
    /// backends with an external version source (aur) override this.
    fn outdated(&self, _installed: &[String]) -> Result<Vec<Candidate>, String> {
        Ok(Vec::new())
    }
    /// Plans upgrades of the given targets together, deps-first and without
    /// duplicates. Default: one plan per target via `plan_install_chain`.
    fn plan_upgrade_chain(&self, targets: &[Candidate]) -> Result<Vec<TransactionPlan>, String> {
        let mut plans = Vec::new();
        for target in targets {
            plans.extend(self.plan_install_chain(target)?);
        }
        Ok(plans)
    }
    fn plan_upgrade_all(&self) -> Result<TransactionPlan, String>;
    fn install_size_estimate(&self, _target: &Candidate) -> Result<Option<u64>, String> {
        Ok(None)
    }
    fn remove_size_estimate(&self, _target: &InstalledPackage) -> Result<Option<u64>, String> {
        Ok(None)
    }
    fn upgrade_impact_estimate(
        &self,
        _plan: &TransactionPlan,
    ) -> Result<Option<UpgradeImpact>, String> {
        Ok(None)
    }
    fn execute_operation(
        &self,
        op: &TransactionOperation,
        ctx: &ExecutionContext,
    ) -> Result<(), String>;
    fn receipt_instances(
        &self,
        plan: &TransactionPlan,
        before: &[InstalledPackage],
        after: &[InstalledPackage],
    ) -> Result<Vec<InstalledPackage>, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prog(cmd: &std::process::Command) -> (String, Vec<String>) {
        (
            cmd.get_program().to_string_lossy().into_owned(),
            cmd.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
        )
    }

    #[test]
    fn command_runs_directly_without_privilege() {
        let (program, args) = prog(
            &ExecutionContext {
                privilege: Privilege::Direct,
            }
            .build_command("pacman")
            .unwrap(),
        );
        assert_eq!(program, "pacman");
        assert!(args.is_empty());
    }

    #[test]
    fn command_wraps_sudo() {
        let (program, args) = prog(
            &ExecutionContext {
                privilege: Privilege::Elevate(vec!["sudo".into()]),
            }
            .build_command("pacman")
            .unwrap(),
        );
        assert_eq!(program, "sudo");
        assert_eq!(args, vec!["pacman"]);
    }

    #[test]
    fn command_wraps_gui_pkexec_with_flags() {
        let (program, args) = prog(
            &ExecutionContext {
                privilege: Privilege::Elevate(vec![
                    "pkexec".into(),
                    "--user".into(),
                    "root".into(),
                ]),
            }
            .build_command("pacman")
            .unwrap(),
        );
        assert_eq!(program, "pkexec");
        assert_eq!(args, vec!["--user", "root", "pacman"]);
    }

    #[test]
    fn empty_elevate_argv_is_rejected() {
        let err = ExecutionContext {
            privilege: Privilege::Elevate(Vec::new()),
        }
        .build_command("pacman")
        .unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }
}
