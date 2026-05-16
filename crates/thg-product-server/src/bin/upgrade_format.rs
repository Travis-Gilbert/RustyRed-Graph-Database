//! thg-upgrade-format: migrate RedCore on-disk format between versions.
//!
//! Walks each tenant subdirectory under the supplied data dir, reads the
//! existing manifest (if any), applies the migration chain `0 -> 1 -> ... ->
//! CURRENT_FORMAT_VERSION`, then writes a fresh manifest+snapshot pair.
//!
//! Usage:
//!     thg-upgrade-format <data-dir> [--dry-run]
//!
//! Exit codes:
//!     0 — all tenants upgraded (or already current).
//!     1 — at least one tenant refused (locked, malformed, or too-new).
//!     2 — bad CLI arguments.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use thg_core::{
    read_manifest, RedCoreDurability, RedCoreGraphStore, RedCoreOptions, CURRENT_FORMAT_VERSION,
};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: thg-upgrade-format <data-dir> [--dry-run]");
        return ExitCode::from(2);
    }
    let data_dir = PathBuf::from(&args[0]);
    let dry_run = args.iter().any(|a| a == "--dry-run");

    if !data_dir.exists() {
        eprintln!("error: data dir does not exist: {}", data_dir.display());
        return ExitCode::from(2);
    }

    let tenants = match discover_tenants(&data_dir) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("error: discover tenants: {err:?}");
            return ExitCode::from(1);
        }
    };

    if tenants.is_empty() {
        // Treat the top-level dir as a single (un-tenanted) RedCore store.
        return upgrade_one(&data_dir, dry_run);
    }

    let mut had_failure = false;
    for tenant in tenants {
        let result = upgrade_one(&tenant, dry_run);
        if result != ExitCode::SUCCESS {
            had_failure = true;
        }
    }
    if had_failure {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn discover_tenants(data_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    // Production layout: <data_dir>/tenants/<safe>/{manifest.json,graph.snapshot.current,graph.aof}
    // Legacy / single-tenant layout: <data_dir>/{manifest.json,...}
    let mut out = Vec::new();

    let tenants_root = data_dir.join("tenants");
    if tenants_root.is_dir() {
        for entry in fs::read_dir(&tenants_root)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() && tenant_dir_looks_redcore(&path) {
                out.push(path);
            }
        }
        return Ok(out);
    }

    // Fall back to top-level scan for the legacy single-tenant case.
    for entry in fs::read_dir(data_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() && tenant_dir_looks_redcore(&path) {
            out.push(path);
        }
    }
    Ok(out)
}

fn tenant_dir_looks_redcore(dir: &Path) -> bool {
    dir.join("manifest.json").exists() || dir.join("graph.snapshot.current").exists()
}

fn upgrade_one(tenant_dir: &Path, dry_run: bool) -> ExitCode {
    let manifest = match read_manifest(tenant_dir) {
        Ok(m) => m,
        Err(err) => {
            eprintln!(
                "tenant {}: refuse, read manifest failed: {err:?}",
                tenant_dir.display()
            );
            return ExitCode::from(1);
        }
    };

    let from_version = manifest.as_ref().map(|m| m.version).unwrap_or(0);
    if from_version == CURRENT_FORMAT_VERSION {
        println!(
            "tenant {}: already at version {} (no-op)",
            tenant_dir.display(),
            CURRENT_FORMAT_VERSION
        );
        return ExitCode::SUCCESS;
    }
    if from_version > CURRENT_FORMAT_VERSION {
        eprintln!(
            "tenant {}: refuse, on-disk version {from_version} > supported {CURRENT_FORMAT_VERSION}",
            tenant_dir.display()
        );
        return ExitCode::from(1);
    }

    println!(
        "tenant {}: upgrading {from_version} -> {CURRENT_FORMAT_VERSION}{}",
        tenant_dir.display(),
        if dry_run { " (dry-run)" } else { "" }
    );

    if dry_run {
        return ExitCode::SUCCESS;
    }

    // The migration path for v0 -> v1 is trivial: opening with the current
    // binary will read the legacy snapshot, replay any AOF, then write a
    // current-format manifest+snapshot when snapshot_now() is called.
    let options = RedCoreOptions {
        durability: RedCoreDurability::None,
        snapshot_interval_writes: 0,
        strict_acid: false,
    };
    let mut store = match RedCoreGraphStore::open(tenant_dir.to_path_buf(), options) {
        Ok(s) => s,
        Err(err) => {
            eprintln!(
                "tenant {}: refuse, open failed: {err:?}",
                tenant_dir.display()
            );
            return ExitCode::from(1);
        }
    };
    if let Err(err) = store.snapshot_now() {
        eprintln!(
            "tenant {}: refuse, snapshot rewrite failed: {err:?}",
            tenant_dir.display()
        );
        return ExitCode::from(1);
    }
    println!(
        "tenant {}: upgraded to {CURRENT_FORMAT_VERSION}",
        tenant_dir.display()
    );
    ExitCode::SUCCESS
}
