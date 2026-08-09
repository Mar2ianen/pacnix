// SPDX-License-Identifier: MIT OR GPL-3.0-or-later

use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use pacnix_backend_alpm::AlpmBackend;
use pacnix_backend_aur::AurBackend;
use pacnix_backend_nix::NixBackend;
use pacnix_core::{
    BackendPlan, Candidate, Command, ExecutionBatch, ExecutionContext, Executor, InstalledPackage,
    PackageBackend, Privilege, RankedCandidate, ResolutionDecision, Resolver, Source, Storage,
    TargetSpec, TransactionPlan,
};

mod config;

const VERBS: &[&str] = &[
    "install", "remove", "search", "info", "list", "upgrade", "sync",
];

struct CliOptions {
    dry_run: bool,
    noconfirm: bool,
    privilege: Option<Vec<String>>,
}

fn parse(args: &[String]) -> Result<(Command, CliOptions), String> {
    let mut dry_run = false;
    let mut noconfirm = false;
    let mut privilege = None;
    let mut deprecated = Vec::new();
    let mut filtered: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--privilege" {
            i += 1;
            let value = args
                .get(i)
                .ok_or("--privilege requires a command (e.g. sudo, pkexec, doas)")?;
            privilege = Some(split_words(value));
        } else if let Some(value) = arg.strip_prefix("--privilege=") {
            privilege = Some(split_words(value));
        } else {
            match arg.as_str() {
                "--dry-run" => dry_run = true,
                "--noconfirm" => noconfirm = true,
                "--execute" | "-E" => deprecated.push(arg.clone()),
                _ => filtered.push(arg.clone()),
            }
        }
        i += 1;
    }
    if !deprecated.is_empty() {
        eprintln!(
            "pacnix: warning: {} is ignored: mutations run by default now",
            deprecated.join(", ")
        );
    }
    let first = filtered.first().ok_or("no command given")?;
    let command = match first.as_str() {
        "-S" => Command::Install(to_targets(&filtered[1..])),
        "-Syu" => Command::Upgrade,
        "-Q" => Command::ListInstalled,
        "-Ss" => Command::Search(filtered[1..].join(" ")),
        "-Qi" => Command::Info(TargetSpec {
            query: filtered[1..].join(" "),
        }),
        "-R" => Command::Remove(to_targets(&filtered[1..])),
        verb if VERBS.contains(&verb) => {
            let mut rest = filtered[1..].to_vec();
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
    Ok((
        command,
        CliOptions {
            dry_run,
            noconfirm,
            privilege,
        },
    ))
}

fn desc_field<'a>(desc: &'a str, field: &str) -> Option<&'a str> {
    let lines: Vec<&str> = desc.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if *line == field && i + 1 < lines.len() {
            return Some(lines[i + 1].trim());
        }
    }
    None
}

/// Binaries shipped by an installed pacman package, read from the local db
/// (`usr/bin/...` entries in `/var/lib/pacman/local/<pkg>/files`).
fn installed_binaries(package: &str) -> Vec<String> {
    let local = std::path::Path::new("/var/lib/pacman/local");
    let Ok(entries) = std::fs::read_dir(local) else {
        return Vec::new();
    };
    let mut binaries = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let Ok(desc) = std::fs::read_to_string(dir.join("desc")) else {
            continue;
        };
        if desc_field(&desc, "%NAME%") != Some(package) {
            continue;
        }
        let Ok(files) = std::fs::read_to_string(dir.join("files")) else {
            return binaries;
        };
        binaries = files
            .lines()
            .map(|line| line.trim_start_matches('/'))
            .filter(|line| line.starts_with("usr/bin/") && !line.ends_with('/'))
            .map(|line| line.trim_start_matches("usr/bin/").to_string())
            .collect();
        break;
    }
    binaries
}

fn prebuilt_variant(resolver: &Resolver, exact: &str) -> Option<String> {
    let (ranked, _errors) = resolver.resolve_ranked(exact);
    ranked
        .into_iter()
        .filter(|r| r.candidate.source == Source::Aur)
        .map(|r| r.candidate.name)
        .find(|name| match name.strip_prefix(exact) {
            Some(rest) => matches!(rest, "-bin" | "-bins" | "-static"),
            None => false,
        })
}

fn split_words(value: &str) -> Vec<String> {
    value.split_whitespace().map(str::to_string).collect()
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
    let (command, opts) = match parse(&args) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("pacnix: {e}");
            std::process::exit(1);
        }
    };
    let resolver = Resolver::new(default_registry());
    match open_storage() {
        Ok(storage) => run(&resolver, &storage, command, &opts),
        Err(e) => {
            eprintln!("pacnix: storage: {e}");
            std::process::exit(1);
        }
    }
}

fn run(resolver: &Resolver, storage: &Storage, command: Command, opts: &CliOptions) {
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
                    pacnix_core::Provenance::Inferred { source } => {
                        format!(" (inferred: {source})")
                    }
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
        Command::Install(targets) => run_install(resolver, storage, &targets, opts),
        Command::Remove(targets) => run_remove(resolver, storage, &targets, opts),
        Command::Upgrade => run_upgrade(resolver, storage, opts),
        Command::Sync => {
            let (count, removed) = reconcile(resolver, storage);
            if removed > 0 {
                println!("reconcile: removed {removed} stale instances");
            }
            println!("reconcile: {count} instances");
        }
        _ => {
            println!("(Phase 0 skeleton: not implemented yet)");
        }
    }
}

struct PlannedInstall<'a> {
    query: String,
    candidate: Candidate,
    backend: &'a dyn PackageBackend,
    plan: TransactionPlan,
    /// True only for the user-requested target; dependency plans in an AUR
    /// chain are auxiliary and never alias-remembered.
    is_target: bool,
}

fn run_install(resolver: &Resolver, storage: &Storage, targets: &[TargetSpec], opts: &CliOptions) {
    if targets.is_empty() {
        eprintln!("pacnix: install requires at least one target");
        return;
    }
    println!(":: Resolving...");
    let mut planned: Vec<PlannedInstall> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for target in targets {
        let preference = storage.alias(&target.query).ok().flatten();
        let decision = resolver.resolve_with_preference(
            &target.query,
            preference.as_ref().map(|(s, r)| (s.as_str(), r.as_str())),
        );
        let ranked = match decision {
            ResolutionDecision::Selected(ranked) => ranked,
            ResolutionDecision::Ambiguous(ranked) => {
                match select_candidate(&target.query, &ranked, opts) {
                    Some(idx) => ranked[idx].clone(),
                    None => {
                        failed.push(format!("aborted selection for: {}", target.query));
                        continue;
                    }
                }
            }
            ResolutionDecision::NotFound { errors } => {
                for err in &errors {
                    eprintln!("pacnix: {}: {}", err.backend, err.message);
                }
                failed.push(format!("nothing found for: {}", target.query));
                continue;
            }
        };
        println!(
            ":: Selected {}/{}",
            ranked.candidate.provider, ranked.candidate.name
        );
        let backend = select_backend(resolver, &ranked.candidate);
        match backend.plan_install_chain(&ranked.candidate) {
            Ok(plans) => {
                let last = plans.len().saturating_sub(1);
                for (idx, plan) in plans.into_iter().enumerate() {
                    planned.push(PlannedInstall {
                        query: target.query.clone(),
                        candidate: ranked.candidate.clone(),
                        backend,
                        plan,
                        is_target: idx == last,
                    });
                }
            }
            Err(e) => failed.push(format!("{}: {e}", backend.name())),
        }
    }
    if !failed.is_empty() {
        for f in &failed {
            eprintln!("pacnix: {f}");
        }
        eprintln!("pacnix: aborting: resolution failed");
        return;
    }
    print_install_summary(&planned);
    if opts.dry_run {
        println!(":: (dry run: nothing executed, nothing written)");
        return;
    }
    let privilege = match config::configured_privilege(&opts.privilege, &config::load()) {
        Ok(argv) => argv,
        Err(e) => {
            eprintln!("pacnix: {e}");
            return;
        }
    };
    if planned.iter().any(|p| p.plan.requires_privilege) && !acquire_privilege(&privilege) {
        return;
    }
    if !confirm(opts, ":: Proceed with installation? [Y/n]") {
        eprintln!("pacnix: aborted");
        return;
    }
    println!(":: Installing...");
    let reports = {
        let batch = ExecutionBatch {
            plans: planned
                .iter()
                .map(|p| BackendPlan {
                    backend: p.backend,
                    plan: &p.plan,
                    ctx: ExecutionContext {
                        privilege: if p.plan.requires_privilege {
                            Privilege::Elevate(privilege.clone())
                        } else {
                            Privilege::Direct
                        },
                    },
                })
                .collect(),
        };
        Executor::new(storage).execute_batch(&batch)
    };
    report_outcomes(&reports);
    let mut ok = true;
    for (report, item) in reports.iter().zip(planned.iter()) {
        if report.error.is_none() {
            if !report.receipts.is_empty() {
                println!(":: Installed {}", item.plan.name);
                let mut bins: Vec<String> = Vec::new();
                for receipt in &report.receipts {
                    if receipt.installed_backend == "alpm" {
                        bins.extend(installed_binaries(&receipt.package_name));
                    }
                }
                bins.dedup();
                if !bins.is_empty() {
                    println!("   bin: {}", bins.join(", "));
                }
            }
            if item.is_target {
                if let Err(e) = storage.remember_alias(
                    &item.query,
                    item.candidate.source.as_str(),
                    &item.candidate.backend_ref,
                ) {
                    eprintln!("pacnix: storage: {e}");
                }
            }
        } else {
            ok = false;
            if item.is_target && item.candidate.source == Source::Aur {
                if let Some(bin) = prebuilt_variant(resolver, &item.candidate.name) {
                    eprintln!(
                        "pacnix: hint: a prebuilt variant is available: try `pacnix -S {bin}`"
                    );
                }
            }
        }
    }
    if ok {
        println!(":: Reconciling authoritative state...");
        let (_count, removed) = reconcile(resolver, storage);
        if removed > 0 {
            println!(":: Reconciled (removed {removed} stale instances)");
        } else {
            println!(":: Reconciled");
        }
    } else {
        println!(":: partial failure: some lanes failed; run `pacnix sync` to reconcile");
    }
}

struct PlannedRemoval<'a> {
    pkg: InstalledPackage,
    backend: &'a dyn PackageBackend,
    plan: TransactionPlan,
}

fn run_remove(resolver: &Resolver, storage: &Storage, targets: &[TargetSpec], opts: &CliOptions) {
    if targets.is_empty() {
        eprintln!("pacnix: remove requires at least one target");
        return;
    }
    let all_installed = collect_installed(resolver);
    let mut planned: Vec<PlannedRemoval> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for target in targets {
        let found: Vec<&InstalledPackage> = all_installed
            .iter()
            .filter(|p| p.name == target.query)
            .collect();
        if found.is_empty() {
            failed.push(format!("not installed: {}", target.query));
            continue;
        }
        let pkg = if found.len() > 1 && !opts.noconfirm && std::io::stdin().is_terminal() {
            match select_instance(&target.query, &found) {
                Some(p) => p,
                None => {
                    failed.push(format!("cannot select instance for: {}", target.query));
                    continue;
                }
            }
        } else {
            found[0].clone()
        };
        let backend = select_backend_by_source(resolver, pkg.source.clone());
        match backend.plan_remove(&pkg) {
            Ok(plan) => planned.push(PlannedRemoval { pkg, backend, plan }),
            Err(e) => failed.push(format!("{}: {e}", backend.name())),
        }
    }
    if !failed.is_empty() {
        for f in &failed {
            eprintln!("pacnix: {f}");
        }
        eprintln!("pacnix: aborting");
        return;
    }
    print_removal_summary(&planned);
    if opts.dry_run {
        println!(":: (dry run: nothing executed, nothing written)");
        return;
    }
    let privilege = match config::configured_privilege(&opts.privilege, &config::load()) {
        Ok(argv) => argv,
        Err(e) => {
            eprintln!("pacnix: {e}");
            return;
        }
    };
    if planned.iter().any(|p| p.plan.requires_privilege) && !acquire_privilege(&privilege) {
        return;
    }
    if !confirm(opts, ":: Proceed with removal? [Y/n]") {
        eprintln!("pacnix: aborted");
        return;
    }
    println!(":: Removing...");
    let reports = execute_plans(
        storage,
        &planned
            .iter()
            .map(|p| (p.backend, &p.plan))
            .collect::<Vec<_>>(),
        &privilege,
    );
    report_outcomes(&reports);
    if reports.iter().all(|r| r.error.is_none()) {
        for item in &planned {
            println!(":: Removed {}", item.pkg.name);
        }
        println!(":: Reconciling authoritative state...");
        let (_count, removed) = reconcile(resolver, storage);
        if removed > 0 {
            println!(":: Reconciled (removed {removed} stale instances)");
        } else {
            println!(":: Reconciled");
        }
    } else {
        println!(":: partial failure; run `pacnix sync` to reconcile");
    }
}

struct PlannedUpgrade<'a> {
    backend: &'a dyn PackageBackend,
    plan: TransactionPlan,
}

fn run_upgrade(resolver: &Resolver, storage: &Storage, opts: &CliOptions) {
    println!(":: Checking for updates...");
    let mut planned: Vec<PlannedUpgrade> = Vec::new();
    for backend in resolver.backends() {
        match backend.plan_upgrade_all() {
            Ok(plan) => planned.push(PlannedUpgrade {
                backend: backend.as_ref(),
                plan,
            }),
            Err(e) => eprintln!("pacnix: {}: {}", backend.name(), e),
        }
    }
    if planned.is_empty() {
        println!("nothing to upgrade");
        return;
    }
    print_upgrade_summary(&planned);
    if opts.dry_run {
        println!(":: (dry run: nothing executed, nothing written)");
        return;
    }
    let privilege = match config::configured_privilege(&opts.privilege, &config::load()) {
        Ok(argv) => argv,
        Err(e) => {
            eprintln!("pacnix: {e}");
            return;
        }
    };
    if planned.iter().any(|p| p.plan.requires_privilege) && !acquire_privilege(&privilege) {
        return;
    }
    if !confirm(opts, ":: Proceed with upgrade? [Y/n]") {
        eprintln!("pacnix: aborted");
        return;
    }
    println!(":: Upgrading...");
    let pairs: Vec<(&dyn PackageBackend, &TransactionPlan)> =
        planned.iter().map(|p| (p.backend, &p.plan)).collect();
    let reports = execute_plans(storage, &pairs, &privilege);
    report_outcomes(&reports);
    if reports.iter().all(|r| r.error.is_none()) {
        println!(":: Reconciling authoritative state...");
    }
    reconcile(resolver, storage);
    println!(":: Reconciled");
}

fn execute_plans(
    storage: &Storage,
    pairs: &[(&dyn PackageBackend, &TransactionPlan)],
    privilege: &[String],
) -> Vec<pacnix_core::BackendReport> {
    let batch = ExecutionBatch {
        plans: pairs
            .iter()
            .map(|(backend, plan)| BackendPlan {
                backend: *backend,
                plan,
                ctx: ExecutionContext {
                    privilege: if plan.requires_privilege {
                        Privilege::Elevate(privilege.to_vec())
                    } else {
                        Privilege::Direct
                    },
                },
            })
            .collect(),
    };
    Executor::new(storage).execute_batch(&batch)
}

fn report_outcomes(reports: &[pacnix_core::BackendReport]) {
    for report in reports {
        if let Some(e) = &report.error {
            eprintln!("pacnix: {}: {}", report.backend, e);
            continue;
        }
        for receipt in &report.receipts {
            println!(
                "receipt: {} from {} ({})",
                receipt.package_name, receipt.source_ref, receipt.installed_backend_ref
            );
        }
    }
}

fn acquire_privilege(privilege: &[String]) -> bool {
    println!(":: Acquiring privilege...");
    let program = privilege.first().map(String::as_str).unwrap_or("sudo");
    if program == "sudo" || program == "sudo-rs" {
        let mut command = std::process::Command::new(program);
        command.args(&privilege[1..]);
        command.arg("-v");
        match command.status() {
            Ok(s) if s.success() => return true,
            Ok(_) => {
                eprintln!("pacnix: privilege acquisition failed ({program})");
                return false;
            }
            Err(e) => {
                eprintln!("pacnix: {program} unavailable: {e}");
                return false;
            }
        }
    }
    if config::find_in_path(program).is_none() {
        eprintln!("pacnix: privilege command not found in PATH: {program}");
        return false;
    }
    println!(":: Privilege tool: {program} (prompt on first privileged call)");
    true
}

fn confirm(opts: &CliOptions, prompt: &str) -> bool {
    if opts.noconfirm {
        return true;
    }
    if !std::io::stdin().is_terminal() {
        eprintln!("pacnix: stdin is not a terminal; use --noconfirm to proceed");
        return false;
    }
    print!("{prompt} ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => false,
        Ok(_) => matches!(line.trim().to_lowercase().as_str(), "" | "y" | "yes"),
        Err(_) => false,
    }
}

fn select_candidate(query: &str, ranked: &[RankedCandidate], opts: &CliOptions) -> Option<usize> {
    if ranked.is_empty() {
        return None;
    }
    if opts.noconfirm || !std::io::stdin().is_terminal() {
        return Some(0);
    }
    println!(":: Multiple providers found for {query}:");
    for (i, entry) in ranked.iter().enumerate() {
        let cand = &entry.candidate;
        println!("{}) {}/{}", i + 1, cand.provider, cand.name);
        println!("   [{}]", reasons(entry));
        if let Some(d) = &cand.description {
            println!("   {d}");
        }
    }
    print!("Enter a selection (default=1): ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return Some(0);
    }
    let choice = line.trim();
    if choice.is_empty() {
        return Some(0);
    }
    choice
        .parse::<usize>()
        .ok()
        .map(|n| n - 1)
        .filter(|n| *n < ranked.len())
}

fn select_instance(query: &str, found: &[&InstalledPackage]) -> Option<InstalledPackage> {
    println!(":: Multiple installed instances for {query}:");
    for (i, p) in found.iter().enumerate() {
        println!("   {}) {}  {}", i + 1, p.source.as_str(), p.backend_ref);
    }
    print!("Enter a selection (default=1): ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return Some(found[0].clone());
    }
    let choice = line.trim();
    if choice.is_empty() {
        return Some(found[0].clone());
    }
    choice
        .parse::<usize>()
        .ok()
        .map(|n| n - 1)
        .filter(|n| *n < found.len())
        .map(|n| found[n].clone())
}

fn print_install_summary(planned: &[PlannedInstall]) {
    println!("\n:: Packages to install");
    let mut by_backend: Vec<(String, Vec<&PlannedInstall>)> = Vec::new();
    for item in planned {
        let key = item.backend.name().to_uppercase();
        match by_backend.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(item),
            None => by_backend.push((key, vec![item])),
        }
    }
    for (backend, items) in &by_backend {
        println!("{backend} ({})", items.len());
        let sizes: Vec<Option<u64>> = items
            .iter()
            .map(|i| i.backend.install_size_estimate(&i.candidate).ok().flatten())
            .collect();
        let total: u64 = sizes.iter().flatten().sum();
        for (item, size) in items.iter().zip(sizes.iter()) {
            match size {
                Some(b) => println!(
                    "  {}/{} ({} disk)",
                    item.candidate.provider,
                    item.plan.name,
                    human_size(*b)
                ),
                None => println!("  {}/{}", item.candidate.provider, item.plan.name),
            }
        }
        if sizes.iter().all(Option::is_some) && !sizes.is_empty() {
            println!("  Total: {} disk", human_size(total));
        } else if sizes.iter().any(Option::is_some) {
            println!("  Known subtotal: {} disk", human_size(total));
        }
    }
    println!();
}

fn human_size(bytes: u64) -> String {
    let value = bytes as f64;
    let units = ["B", "KiB", "MiB", "GiB"];
    let mut unit = 0;
    let mut amount = value;
    while amount >= 1024.0 && unit < units.len() - 1 {
        amount /= 1024.0;
        unit += 1;
    }
    format!("{amount:.2} {}", units[unit])
}

fn print_removal_summary(planned: &[PlannedRemoval]) {
    println!("\n:: Packages to remove");
    let sizes: Vec<Option<u64>> = planned
        .iter()
        .map(|i| i.backend.remove_size_estimate(&i.pkg).ok().flatten())
        .collect();
    let total: u64 = sizes.iter().flatten().sum();
    for (item, size) in planned.iter().zip(sizes.iter()) {
        match size {
            Some(b) => println!(
                "  {} ({} freed) via {}",
                item.pkg.name,
                human_size(*b),
                item.backend.name()
            ),
            None => println!("  {} via {}", item.pkg.name, item.backend.name()),
        }
    }
    if sizes.iter().all(Option::is_some) && !sizes.is_empty() {
        println!("  Total: {} freed", human_size(total));
    } else if sizes.iter().any(Option::is_some) {
        println!("  Known subtotal: {} freed", human_size(total));
    }
    println!();
}

fn print_upgrade_summary(planned: &[PlannedUpgrade]) {
    println!("\n:: Packages to upgrade");
    let mut deltas: Vec<Option<i64>> = Vec::new();
    let mut all_available = true;
    for item in planned {
        println!("  {} via {}", item.plan.name, item.backend.name());
        match item.backend.upgrade_impact_estimate(&item.plan) {
            Ok(Some(impact)) => {
                for e in &impact.entries {
                    match (e.old_size, e.new_size) {
                        (Some(old), Some(new)) => {
                            let d = new as i64 - old as i64;
                            deltas.push(Some(d));
                            println!(
                                "    {}: {} -> {} (Δ{}{} disk)",
                                e.name,
                                human_size(old),
                                human_size(new),
                                if d > 0 { "+" } else { "" },
                                human_size(d.unsigned_abs())
                            );
                        }
                        (None, Some(new)) => {
                            deltas.push(None);
                            println!("    {}: {} disk new", e.name, human_size(new));
                        }
                        (Some(old), None) => {
                            deltas.push(None);
                            println!("    {}: {} disk old", e.name, human_size(old));
                        }
                        _ => deltas.push(None),
                    }
                }
            }
            Ok(None) => all_available = false,
            Err(e) => {
                all_available = false;
                eprintln!("pacnix: upgrade impact ({}): {e}", item.backend.name());
            }
        }
    }
    let any_known = deltas.iter().any(Option::is_some);
    let all_delta_known = deltas.iter().all(Option::is_some) && !deltas.is_empty();
    if let Some(total) = signed_total(&deltas) {
        if all_available && all_delta_known {
            println!(
                "  Total: {}{} disk",
                if total < 0 { "-" } else { "+" },
                human_size(total.unsigned_abs())
            );
        } else if any_known {
            println!(
                "  Known subtotal: {}{} disk",
                if total < 0 { "-" } else { "+" },
                human_size(total.unsigned_abs())
            );
        }
    }
    if any_known {
        println!("    note: impact estimated from current sync DB; refresh may change it");
    }
    println!();
}

fn signed_total(deltas: &[Option<i64>]) -> Option<i64> {
    let known: Vec<i64> = deltas.iter().flatten().copied().collect();
    (!known.is_empty()).then(|| known.iter().sum())
}

fn aur_exact_names(
    pkgs_by_backend: &[(&dyn PackageBackend, Option<Vec<InstalledPackage>>)],
) -> HashSet<String> {
    let mut foreign: Vec<String> = Vec::new();
    for (backend, pkgs) in pkgs_by_backend {
        if backend.source() != Source::Alpm {
            continue;
        }
        let Some(pkgs) = pkgs else {
            continue;
        };
        for pkg in pkgs {
            if pkg.provenance == pacnix_core::Provenance::Foreign && pkg.installed_at.is_some() {
                foreign.push(pkg.name.clone());
            }
        }
    }
    if foreign.is_empty() {
        return HashSet::new();
    }
    match pacnix_backend_aur::rpc::existing_names(&foreign) {
        Ok(names) => names.into_iter().collect(),
        Err(e) => {
            eprintln!("pacnix: aur rpc: {e}");
            HashSet::new()
        }
    }
}

fn reconcile(resolver: &Resolver, storage: &Storage) -> (usize, usize) {
    let generation = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut count = 0;
    let mut removed_total = 0;
    let mut errors = Vec::new();
    let mut inferred = 0usize;

    let mut scanned: Vec<(&dyn PackageBackend, Option<Vec<InstalledPackage>>)> = Vec::new();
    for backend in resolver.backends() {
        let backend: &dyn PackageBackend = backend.as_ref();
        match backend.installed() {
            Ok(pkgs) => scanned.push((backend, Some(pkgs))),
            Err(e) => {
                errors.push(format!("{}: {e}", backend.name()));
                scanned.push((backend, None));
            }
        }
    }
    let aur_names = if resolver
        .backends()
        .iter()
        .any(|b| b.source() == Source::Aur)
    {
        aur_exact_names(&scanned)
    } else {
        HashSet::new()
    };
    for (backend, pkgs) in &scanned {
        let Some(pkgs) = pkgs else {
            continue;
        };
        let mut upserts: Vec<InstalledPackage> = Vec::new();
        for pkg in pkgs.iter() {
            let mut pkg = pkg.clone();
            if pkg.provenance == pacnix_core::Provenance::Foreign && pkg.installed_at.is_some() {
                if let Ok(Some(source)) = storage.known_source_for(
                    &pkg.name,
                    pkg.source.as_str(),
                    &pkg.backend_ref,
                    pkg.version.as_deref(),
                    pkg.installed_at,
                ) {
                    pkg.provenance = pacnix_core::Provenance::PacnixInstalled { source };
                } else if backend.source() == Source::Alpm && aur_names.contains(&pkg.name) {
                    pkg.provenance = pacnix_core::Provenance::Inferred {
                        source: "aur".into(),
                    };
                    inferred += 1;
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
            upserts.push(pkg);
            count += 1;
        }
        match storage.upsert_and_sweep(&upserts, generation, backend.name()) {
            Ok(removed) => removed_total += removed,
            Err(e) => errors.push(format!("storage ({}): {e}", backend.name())),
        }
    }
    if inferred > 0 {
        println!("reconcile: inferred {inferred} foreign packages to be aur");
    }
    for e in &errors {
        eprintln!("pacnix: {e}");
    }
    (count, removed_total)
}

fn select_backend<'a>(resolver: &'a Resolver, candidate: &Candidate) -> &'a dyn PackageBackend {
    select_backend_by_source(resolver, candidate.source.clone())
}

fn select_backend_by_source(resolver: &Resolver, source: Source) -> &dyn PackageBackend {
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
