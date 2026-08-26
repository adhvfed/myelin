use myelin_lints::coverage::{
    parse_contract_index_rows, parse_frontend_contracts, parse_manifest, scan,
    scan_frontend_contracts, workspace_root, FsArtifacts,
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
        .unwrap_or_else(|| root.join("contracts/contract-index.md"));
    let manifest_path = arg_value(&args, "--manifest")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("contract-coverage.toml"));

    let index_src = match std::fs::read_to_string(&index_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "contract-coverage: FAIL - cannot read contract-index `{}`: {e}",
                index_path.display()
            );
            return ExitCode::FAILURE;
        }
    };
    let manifest_src = match std::fs::read_to_string(&manifest_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "contract-coverage: FAIL - cannot read coverage manifest `{}`: {e}",
                manifest_path.display()
            );
            return ExitCode::FAILURE;
        }
    };

    let rows = parse_contract_index_rows(&index_src);
    if rows.is_empty() {
        eprintln!(
            "contract-coverage: FAIL - parsed 0 contract rows from `{}` (the row parser drifted \
             from the contract-index table shape).",
            index_path.display()
        );
        return ExitCode::FAILURE;
    }
    let manifest = match parse_manifest(&manifest_src) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("contract-coverage: FAIL - malformed coverage manifest: {e}");
            return ExitCode::FAILURE;
        }
    };
    let frontend = match parse_frontend_contracts(&manifest_src) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("contract-coverage: FAIL - malformed frontend coverage registry: {e}");
            return ExitCode::FAILURE;
        }
    };

    let artifacts = FsArtifacts {
        workspace_root: root,
    };
    let report = scan(&rows, &manifest, &artifacts);
    let frontend_errors = scan_frontend_contracts(&frontend, &artifacts);

    if report.is_green() && frontend_errors.is_empty() {
        eprintln!(
            "contract-coverage: OK - {} contract rows reconciled ({} covered by existing CDC \
             test files, {} deferred with a named landing prompt), {} frontend contract(s) \
             registered with structurally-valid shared vectors.",
            report.rows_checked,
            report.covered,
            report.deferred,
            frontend.len()
        );
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "contract-coverage: FAIL - {} contract coverage registry violation(s):",
            report.errors.len() + frontend_errors.len()
        );
        for err in &report.errors {
            eprintln!("  {err}");
        }
        for err in &frontend_errors {
            eprintln!("  {err}");
        }
        ExitCode::FAILURE
    }
}
