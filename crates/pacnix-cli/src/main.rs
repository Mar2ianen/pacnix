// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use std::path::PathBuf;

use pacnix_backend_alpm::AlpmBackend;
use pacnix_backend_aur::AurBackend;
use pacnix_backend_nix::NixBackend;
use pacnix_core::{
    Candidate, Command, Interaction, InstalledPackage, PackageBackend, Resolver, Storage,
    TargetSpec,
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

fn open_storage() -> Result<Storage, String> {
    let dir: PathBuf = std::env::var_os("PACNIX_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
                .join(".local/state/pacnix")
        });
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Storage::open(&dir.join("pacnix.db").to_string_lossy())
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
    match open_storage() {
        Ok(storage) => run(&resolver, &storage, command),
        Err(e) => {
            eprintln!("pacnix: storage: {e}");
            std::process::exit(1);
        }
    }
}

fn run(resolver: &Resolver, storage: &Storage, command: Command) {
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
                if let Err(e) = storage.upsert_instance(pkg) {
                    eprintln!("pacnix: storage: {e}");
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
                if result.candidates.len() == 1 {
                    let cand = &result.candidates[0];
                    if let Err(e) = storage.remember_alias(&target.query, &cand.provider, &cand.name) {
                        eprintln!("pacnix: storage: {e}");
                    }
                    let backend = select_backend(resolver, cand);
                    match backend.plan_install(cand) {
                        Ok(plan) => {
                            println!(
                                "plan: install {} from {}/{} ({} operations, privilege: {})",
                                plan.name,
                                cand.provider,
                                cand.name,
                                plan.operations.len(),
                                plan.requires_privilege
                            );
                        }
                        Err(e) => eprintln!("pacnix: {}: {e}", backend.name()),
                    }
                    continue;
                }
                match Interaction::SelectCandidate(result.candidates) {
                    Interaction::SelectCandidate(candidates) => {
                        println!("select candidate for {}:", target.query);
                        for (i, cand) in candidates.iter().enumerate() {
                            println!("  {}) {}/{}", i + 1, cand.provider, cand.name);
                        }
                    }
                    _ => unreachable!(),
                }
            }
        }
        Command::Sync => {
            let mut count = 0;
            let mut errors = Vec::new();
            for backend in resolver.backends() {
                match backend.installed() {
                    Ok(pkgs) => {
                        for pkg in &pkgs {
                            if let Err(e) = storage.upsert_instance(pkg) {
                                errors.push(format!("{}: storage: {e}", backend.name()));
                            }
                        }
                        count += pkgs.len();
                    }
                    Err(e) => errors.push(format!("{}: {e}", backend.name())),
                }
            }
            for e in &errors {
                eprintln!("pacnix: {e}");
            }
            println!("reconcile: {count} instances");
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