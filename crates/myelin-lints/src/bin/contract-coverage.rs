//! `contract-coverage` — the committed CI entrypoint for the contract-coverage meta-gate
//! (P-S21 → P-037).
//!
//! This binary IS the loud, never-swallowed meta-gate the CI workflow runs (alongside `lint-gate`).
//! It reconciles the authoritative contract-index row set
//! (`planning/05-refined-shared-systems-architecture/contract-index.md`) against the coverage
//! manifest (`contract-coverage.toml` at the workspace root), verifying every `covered` row's
//! named provider+consumer CDC file exists on disk and carries both sides, and that every
//! `deferred` row names its landing prompt. It **exits NON-ZERO on any falsely-claimed or dropped
//! row** — the gate is the process exit code itself, so there is no `... || true` swallow path
//! (doctrine EI-01 §5: "an uncommitted gate is no gate; make violations loud").
//!
//! Usage:
//!   `contract-coverage`                       — scan the workspace (default paths).
//!   `contract-coverage --index P --manifest Q` — scan with explicit paths (used by the self-test
//!                                                to point at a red fixture).

use myelin_lints::coverage::{
    parse_contract_index_rows, parse_manifest, scan, workspace_root, FsCdc,
};
use std::path::PathBuf;
use std::process::ExitCode;

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() -> ExitCode {
    let root = workspace_root();
    let args: Vec<String> = std::env::args().skip(1).collect();

    let index_path = arg_value(&args, "--index")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            root.join("planning/05-refined-shared-systems-architecture/contract-index.md")
        });
    let manifest_path = arg_value(&args, "--manifest")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("contract-coverage.toml"));

    let index_src = match std::fs::read_to_string(&index_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "contract-coverage: FAIL — cannot read contract-index `{}`: {e}",
                index_path.display()
            );
            return ExitCode::FAILURE;
        }
    };
    let manifest_src = match std::fs::read_to_string(&manifest_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "contract-coverage: FAIL — cannot read coverage manifest `{}`: {e}",
                manifest_path.display()
            );
            return ExitCode::FAILURE;
        }
    };

    let rows = parse_contract_index_rows(&index_src);
    if rows.is_empty() {
        eprintln!(
            "contract-coverage: FAIL — parsed 0 contract rows from `{}` (the row parser drifted \
             from the contract-index table shape).",
            index_path.display()
        );
        return ExitCode::FAILURE;
    }
    let manifest = match parse_manifest(&manifest_src) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("contract-coverage: FAIL — malformed coverage manifest: {e}");
            return ExitCode::FAILURE;
        }
    };

    let cdc = FsCdc {
        workspace_root: root,
    };
    let report = scan(&rows, &manifest, &cdc);

    if report.is_green() {
        eprintln!(
            "contract-coverage: OK — {} contract rows reconciled ({} covered with a verified \
             provider+consumer CDC pair, {} deferred with a named landing prompt), 0 \
             falsely-claimed.",
            report.rows_checked, report.covered, report.deferred
        );
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "contract-coverage: FAIL — {} contract row(s) lie about coverage (loud, never \
             swallowed — ship the missing CDC pair or mark the row deferred with its landing \
             prompt; NEVER weaken the gate):",
            report.errors.len()
        );
        for err in &report.errors {
            eprintln!("  {err}");
        }
        ExitCode::FAILURE
    }
}
