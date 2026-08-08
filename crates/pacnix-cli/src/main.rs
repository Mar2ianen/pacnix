// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use pacnix_core::{
    Candidate, Command, InstalledPackage, Interaction, PackageBackend, Resolver, Source,
    TargetSpec, TransactionPlan,
};

const VERBS: &[&str] = &["install", "remove", "search", "info", "list", "upgrade", "sync"];

fn parse(args: &[String]) -> Result<Command, String> {
    let first = args.first().ok_or("no command given")?;
    match first.as_str() {
        "-S" => Ok(Command::Install(to_targets(&args[1..]))),
        "-Syu" => Ok(Command::Upgrade),
        "-Q" => Ok(Command::ListInstalled),
        "-Ss" => Ok(Command::Search(args[1..].join(" "))),
        "-Qi" => Ok(Command::Info(TargetSpec {
            query: args[1..].join(" "),
        })),
        "-R" => Ok(Command::Remove(to_targets(&args[1..]))),
        verb if VERBS.contains(&verb) => {
            let mut rest = args[1..].to_vec();
            if let Some(pos) = rest.iter().position(|a| a == "--") {
                rest = rest[pos + 1..].to_vec();
            }
            match verb {
                "install" => Ok(Command::Install(to_targets(&rest))),
                "remove" => Ok(Command::Remove(to_targets(&rest))),
                "search" => Ok(Command::Search(rest.join(" "))),
                "info" => Ok(Command::Info(
                    to_targets(&rest)
                        .first()
                        .cloned()
                        .ok_or("info requires a target")?,
                )),
                "list" => Ok(Command::ListInstalled),
                "upgrade" => Ok(Command::Upgrade),
                "sync" => Ok(Command::Sync),
                _ => unreachable!(),
            }
        }
        other => Err(format!("unknown command: {other}")),
    }
}

fn to_targets(args: &[String]) -> Vec<TargetSpec> {
    args.iter().map(|a| TargetSpec { query: a.clone() }).collect()
}

struct BackendStub {
    source: Source,
}

impl PackageBackend for BackendStub {
    fn name(&self) -> &'static str {
        "stub"
    }
    fn source(&self) -> Source {
        self.source.clone()
    }
    fn search(&self, _query: &str) -> Result<Vec<Candidate>, String> {
        Ok(Vec::new())
    }
    fn installed(&self) -> Result<Vec<InstalledPackage>, String> {
        Ok(Vec::new())
    }
    fn plan_install(&self, _target: &TargetSpec) -> Result<TransactionPlan, String> {
        Err("backend not implemented".into())
    }
    fn plan_remove(&self, _target: &InstalledPackage) -> Result<TransactionPlan, String> {
        Err("backend not implemented".into())
    }
    fn plan_upgrade(&self, _target: &InstalledPackage) -> Result<TransactionPlan, String> {
        Err("backend not implemented".into())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pacnix: {e}");
            std::process::exit(1);
        }
    };

    let backends: Vec<&dyn PackageBackend> = vec![
        &BackendStub {
            source: Source::Alpm,
        },
        &BackendStub { source: Source::Aur },
        &BackendStub { source: Source::Nix },
    ];
    {
        let resolver = Resolver::new(backends);
        run(&resolver, command);
    }
}

fn run(resolver: &Resolver<'_>, command: Command) {
    match command {
        Command::Search(query) => {
            let candidates = resolver.resolve(&query).unwrap_or_default();
            if candidates.is_empty() {
                println!("nothing found for: {query}");
            }
            for c in &candidates {
                println!("{}/{}", c.source_name(), c.name);
            }
        }
        Command::ListInstalled => println!("no installed packages (backends not implemented)"),
        other => {
            let interaction = match other {
                Command::Install(t) => Interaction::SelectCandidate(
                    t.iter().map(|t| Candidate {
                        source: Source::Alpm,
                        provider: "extra".into(),
                        name: t.query.clone(),
                        version: None,
                        description: None,
                    })
                    .collect(),
                ),
                _ => return,
            };
            match interaction {
                Interaction::SelectCandidate(c) => {
                    println!("resolve candidates ({}):", c.len());
                    for (i, cand) in c.iter().enumerate() {
                        println!("  {}) {}/{}", i + 1, cand.source_name(), cand.name);
                    }
                }
                Interaction::Confirm(p) => println!("plan: {} {}", p.backend_ref, p.name),
                Interaction::RequestPrivilege(_) => println!("need privilege"),
            }
            println!("(Phase 0 skeleton: backends not implemented yet)");
        }
    }
}