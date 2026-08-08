// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use pacnix_backend_alpm::AlpmBackend;
use pacnix_backend_aur::AurBackend;
use pacnix_backend_nix::NixBackend;
use pacnix_core::{
    Candidate, Command, Interaction, InstalledPackage, PackageBackend, Resolver, TargetSpec,
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

fn default_registry() -> Vec<Box<dyn PackageBackend>> {
    vec![
        Box::new(AlpmBackend),
        Box::new(AurBackend),
        Box::new(NixBackend),
    ]
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

    let resolver = Resolver::new(default_registry());
    run(&resolver, command);
}

fn run(resolver: &Resolver, command: Command) {
    match command {
        Command::Search(query) => {
            let result = resolver.resolve(&query);
            for err in &result.backend_errors {
                eprintln!("pacnix: {}: {}", err.backend, err.message);
            }
            if result.candidates.is_empty() {
                println!("nothing found for: {query}");
                return;
            }
            for c in &result.candidates {
                println!("{}/{}", c.provider, c.name);
                if let Some(d) = &c.description {
                    println!("    {d}");
                }
            }
        }
        Command::ListInstalled => {
            let mut found: Vec<InstalledPackage> = Vec::new();
            for backend in resolver.backends() {
                match backend.installed() {
                    Ok(mut pkgs) => found.append(&mut pkgs),
                    Err(e) => eprintln!("pacnix: {}: {e}", backend.name()),
                }
            }
            for pkg in &found {
                println!("{} {}", pkg.name, pkg.version.as_deref().unwrap_or("-"));
            }
        }
        Command::Install(targets) => {
            for target in &targets {
                let result = resolver.resolve(&target.query);
                for err in &result.backend_errors {
                    eprintln!("pacnix: {}: {}", err.backend, err.message);
                }
                if result.candidates.is_empty() {
                    eprintln!("pacnix: nothing found for: {}", target.query);
                    continue;
                }
                let interaction = if result.candidates.len() == 1 {
                    Interaction::Confirm(
                        select_backend(resolver, &result.candidates[0]).plan_install(&result.candidates[0])
                            .unwrap_or_else(|e| {
                                eprintln!("pacnix: {e}");
                                std::process::exit(1);
                            }),
                    )
                } else {
                    Interaction::SelectCandidate(result.candidates)
                };
                match interaction {
                    Interaction::SelectCandidate(candidates) => {
                        println!("select candidate for {}:", target.query);
                        for (i, cand) in candidates.iter().enumerate() {
                            println!("  {}) {}/{}", i + 1, cand.provider, cand.name);
                        }
                    }
                    Interaction::Confirm(plan) => {
                        println!(
                            "plan: {} {} operations={}",
                            plan.backend_ref,
                            plan.name,
                            plan.operations.len()
                        );
                    }
                    Interaction::RequestPrivilege(_) => println!("need privilege"),
                }
            }
        }
        _ => {
            println!("(Phase 0 skeleton: not implemented yet)");
        }
    }
}

fn select_backend<'a>(resolver: &'a Resolver, candidate: &Candidate) -> &'a dyn PackageBackend {
    resolver
        .backends()
        .iter()
        .find(|b| b.source() == candidate.source)
        .expect("backend for candidate source must be registered")
        .as_ref()
}