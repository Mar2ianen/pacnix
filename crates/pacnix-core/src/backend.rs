// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use crate::model::{Candidate, InstalledPackage, Source, TransactionOperation, TransactionPlan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionContext {
    pub privilege: Option<Vec<String>>,
}

impl ExecutionContext {
    pub fn build_command(&self, program: &str) -> std::process::Command {
        match &self.privilege {
            None => std::process::Command::new(program),
            Some(argv) if argv.is_empty() => std::process::Command::new(program),
            Some(argv) => {
                let mut command = std::process::Command::new(&argv[0]);
                command.args(&argv[1..]);
                command.arg(program);
                command
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
    fn plan_remove(&self, target: &InstalledPackage) -> Result<TransactionPlan, String>;
    fn plan_upgrade(&self, target: &InstalledPackage) -> Result<TransactionPlan, String>;
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
        let (program, args) = prog(&ExecutionContext { privilege: None }.build_command("pacman"));
        assert_eq!(program, "pacman");
        assert!(args.is_empty());
    }

    #[test]
    fn command_wraps_sudo() {
        let (program, args) = prog(
            &ExecutionContext {
                privilege: Some(vec!["sudo".into()]),
            }
            .build_command("pacman"),
        );
        assert_eq!(program, "sudo");
        assert_eq!(args, vec!["pacman"]);
    }

    #[test]
    fn command_wraps_gui_pkexec_with_flags() {
        let (program, args) = prog(
            &ExecutionContext {
                privilege: Some(vec!["pkexec".into(), "--user".into(), "root".into()]),
            }
            .build_command("pacman"),
        );
        assert_eq!(program, "pkexec");
        assert_eq!(args, vec!["--user", "root", "pacman"]);
    }
}
