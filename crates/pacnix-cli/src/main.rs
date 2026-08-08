// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use std::path::PathBuf;

use pacnix_backend_alpm::AlpmBackend;
use pacnix_backend_aur::AurBackend;
use pacnix_backend_nix::NixBackend;
use pacnix_core::{
    Candidate, Command, Executor, InstalledPackage, PackageBackend, RankedCandidate,
    ResolutionDecision, Resolver, Storage, TargetSpec, TransactionPlan,
};

const VERBS: &[&str] = &[
    "install", "remove", "search", "info", "list", "upgrade", "sync",
];

fn parse(args: &[String]) -> Result<(Command, bool), String> {
    let (execute, args): (bool, Vec<String>) = {
        let rest: Vec<String> = args.to_vec();
        let execute = rest.iter().any(|a| a == "--execute" || a == "-E");
        let filtered = rest
            .into_iter()
            .filter(|a| a != "--execute" && a != "-E")
            .collect();
        (execute, filtered)
    };
    let first = args.first().ok_or("no command given")?;
    let command = match first.as_str() {
        "-S" => Command::Install(to_targets(&args[1..])),
        "-Syu" => Command::Upgrade,
        "-Q" => Command::ListInstalled,
        "-Ss" => Command::Search(args[1..].join(" ")),
        "-Qi" => Command::Info(TargetSpec {
            query: args[1..].join(" "),
        }),
        "-R" => Command::Remove(to_targets(&args[1..])),
        verb if VERBS.contains(&verb) => {
            let mut rest = args[1..].to_vec();
            if let Some(pos) = rest.iter().position(|a| a == "--") {
                rest = rest[pos + 1..].to_vec();
            }
            match verb {
                "install" => Command::Install(to_targets(&rest)),
                "remove" => Command::Remove(to_targets(&rest)),
                "search" => Command::Search(rest.join(" ")),
                "info" => Command::Info(
                    to_targets(&rest)
                        .first()
                        .cloned()
                        .ok_or("info requires a target")?,
                ),
                "list" => Command::ListInstalled,
                "upgrade" => Command::Upgrade,
                "sync" => Command::Sync,
                _ => unreachable!(),
            }
        }
        other => return Err(format!("unknown command: {other}")),
    };
    Ok((command, execute))
}

fn to_targets(args: &[String]) -> Vec<TargetSpec> {
    args.iter()
        .map(|a| TargetSpec { query: a.clone() })
        .collect()
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
    let (command, execute) = match parse(&args) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("pacnix: {e}");
            std::process::exit(1);
        }
    };

    let resolver = Resolver::new(default_registry());
    match open_storage() {
        Ok(storage) => run(&resolver, &storage, command, execute),
        Err(e) => {
            eprintln!("pacnix: storage: {e}");
            std::process::exit(1);
        }
    }
}

fn run(resolver: &Resolver, storage: &Storage, command: Command, execute: bool) {
    match command {
        Command::Search(query) => {
            let (ranked, errors) = resolver.resolve_ranked(&query);
            for err in &errors {
                eprintln!("pacnix: {}: {}", err.backend, err.message);
            }
            if ranked.is_empty() {
                println!("nothing found for: {query}");
            } else {
                print_ranked(&ranked);
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
                let mark = match &pkg.provenance {
                    pacnix_core::Provenance::SyncKnown => String::new(),
                    pacnix_core::Provenance::Foreign => " (foreign)".to_string(),
                    pacnix_core::Provenance::PacnixInstalled { source } => {
                        format!(" (via pacnix: {source})")
                    }
                    pacnix_core::Provenance::Unknown => " (unknown)".to_string(),
                };
                println!(
                    "{} {}{}",
                    pkg.name,
                    pkg.version.as_deref().unwrap_or("-"),
                    mark
                );
            }
        }
        Command::Install(targets) => {
            for target in &targets {
                let preference = storage.alias(&target.query).ok().flatten();
                let decision = resolver.resolve_with_preference(
                    &target.query,
                    preference.as_ref().map(|(s, r)| (s.as_str(), r.as_str())),
                );
                match decision {
                    ResolutionDecision::Selected(ranked) => {
                        let cand = &ranked.candidate;
                        if let Err(e) = storage.remember_alias(
                            &target.query,
                            cand.source.as_str(),
                            &cand.backend_ref,
                        ) {
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
                                if execute {
                                    execute_plan(storage, backend, &plan);
                                }
                            }
                            Err(e) => eprintln!("pacnix: {}: {e}", backend.name()),
                        }
                    }
                    ResolutionDecision::Ambiguous(ranked) => {
                        println!("select candidate for {}:", target.query);
                        for (i, ranked) in ranked.iter().enumerate() {
                            let cand = &ranked.candidate;
                            println!(
                                "  {}) {}/{}  [{}]",
                                i + 1,
                                cand.provider,
                                cand.name,
                                reasons(ranked)
                            );
                        }
                        if execute {
                            eprintln!("pacnix: nothing executed: pick a candidate");
                        }
                    }
                    ResolutionDecision::NotFound { errors } => {
                        for err in &errors {
                            eprintln!("pacnix: {}: {}", err.backend, err.message);
                        }
                        eprintln!("pacnix: nothing found for: {}", target.query);
                    }
                }
            }
        }
        Command::Remove(targets) => {
            for target in &targets {
                let mut found = collect_installed(resolver);
                found.retain(|p| p.name == target.query);
                if found.is_empty() {
                    eprintln!("pacnix: not installed: {}", target.query);
                    continue;
                }
                if found.len() > 1 {
                    eprintln!(
                        "pacnix: {} has multiple installed instances ({})",
                        target.query,
                        found
                            .iter()
                            .map(|p| p.source.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    continue;
                }
                let pkg = &found[0];
                let backend = select_backend_by_source(resolver, pkg.source.clone());
                match backend.plan_remove(pkg) {
                    Ok(plan) => {
                        println!(
                            "plan: remove {} via {} ({} operations)",
                            plan.name,
                            backend.name(),
                            plan.operations.len()
                        );
                        if execute {
                            execute_plan(storage, backend, &plan);
                        }
                    }
                    Err(e) => eprintln!("pacnix: {}: {e}", backend.name()),
                }
            }
        }
        Command::Upgrade => {
            let installed = collect_installed(resolver);
            let mut planned = 0;
            for pkg in &installed {
                let backend = select_backend_by_source(resolver, pkg.source.clone());
                match backend.plan_upgrade(pkg) {
                    Ok(plan) => {
                        println!(
                            "plan: upgrade {} -> {} ({} operations)",
                            plan.name,
                            backend.name(),
                            plan.operations.len()
                        );
                        planned += 1;
                        if execute {
                            execute_plan(storage, backend, &plan);
                        }
                    }
                    Err(e) => eprintln!("pacnix: {}: {e}", backend.name()),
                }
            }
            if planned == 0 {
                println!("nothing to upgrade");
            }
        }
        Command::Sync => {
            let generation = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let mut count = 0;
            let mut errors = Vec::new();
            let mut scanned: Vec<String> = Vec::new();
            for backend in resolver.backends() {
                let pkgs = match backend.installed() {
                    Ok(pkgs) => pkgs,
                    Err(e) => {
                        errors.push(format!("{}: {e}", backend.name()));
                        continue;
                    }
                };
                let mut backend_ok = true;
                for pkg in &pkgs {
                    let mut pkg = pkg.clone();
                    if pkg.provenance == pacnix_core::Provenance::Foreign
                        && pkg.installed_at.is_some()
                    {
                        if let Ok(Some(source)) = storage.known_source_for(
                            &pkg.name,
                            pkg.source.as_str(),
                            &pkg.backend_ref,
                            pkg.version.as_deref(),
                            pkg.installed_at,
                        ) {
                            pkg.provenance = pacnix_core::Provenance::PacnixInstalled { source };
                        }
                    } else if pkg.provenance == pacnix_core::Provenance::Unknown
                        && pkg.installed_at.is_none()
                    {
                        if let Ok(Some(source)) = storage.known_source_for(
                            &pkg.name,
                            pkg.source.as_str(),
                            &pkg.backend_ref,
                            pkg.version.as_deref(),
                            None,
                        ) {
                            pkg.provenance = pacnix_core::Provenance::PacnixInstalled { source };
                        }
                    }
                    if let Err(e) = storage.upsert_instance_with_generation(&pkg, generation) {
                        backend_ok = false;
                        errors.push(format!("{}: storage: {e}", backend.name()));
                    }
                }
                if backend_ok {
                    scanned.push(backend.name().to_string());
                }
                count += pkgs.len();
            }
            match storage.sweep_stale_instances(generation, &scanned) {
                Ok(removed) => {
                    if removed > 0 {
                        println!("reconcile: removed {removed} stale instances");
                    }
                }
                Err(e) => errors.push(format!("storage: {e}")),
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
    select_backend_by_source(resolver, candidate.source.clone())
}

fn select_backend_by_source(
    resolver: &Resolver,
    source: pacnix_core::Source,
) -> &dyn PackageBackend {
    resolver
        .backends()
        .iter()
        .find(|b| b.source() == source)
        .expect("backend for source must be registered")
        .as_ref()
}

fn collect_installed(resolver: &Resolver) -> Vec<InstalledPackage> {
    let mut found: Vec<InstalledPackage> = Vec::new();
    for backend in resolver.backends() {
        match backend.installed() {
            Ok(mut pkgs) => found.append(&mut pkgs),
            Err(e) => eprintln!("pacnix: {}: {e}", backend.name()),
        }
    }
    found
}

fn execute_plan(storage: &Storage, backend: &dyn PackageBackend, plan: &TransactionPlan) {
    let ctx = pacnix_core::ExecutionContext {
        use_sudo: plan.requires_privilege,
    };
    match Executor::new(storage).execute(plan, backend, &ctx) {
        Ok(receipts) => {
            println!("exec: {} ok", plan.name);
            for receipt in &receipts {
                println!(
                    "receipt: {} from {} ({})",
                    receipt.package_name, receipt.source_ref, receipt.installed_backend_ref
                );
            }
        }
        Err(e) => eprintln!("pacnix: {e}"),
    }
}

fn reasons(ranked: &RankedCandidate) -> String {
    ranked
        .reasons
        .iter()
        .map(|r| r.label())
        .collect::<Vec<_>>()
        .join("; ")
}

fn print_ranked(ranked: &[RankedCandidate]) {
    for (i, entry) in ranked.iter().enumerate() {
        let cand = &entry.candidate;
        println!("{}) {}/{}", i + 1, cand.provider, cand.name);
        println!("    [{}]", reasons(entry));
        if let Some(d) = &cand.description {
            println!("    {d}");
        }
    }
}
