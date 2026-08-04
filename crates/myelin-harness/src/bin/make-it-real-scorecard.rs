use myelin_harness::make_it_real::{
    output_bytes, AttestedScorecard, RowAttestation, DEFAULT_MAX_AGE_DAYS,
};
use myelin_harness::scorecard::{today_iso, Band, RowResult, Scorecard};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let refresh_thresholds = match parse_args() {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let date = today_iso();
    let mut card = Scorecard::new(Band::MakeItReal);

    println!(
        "== make-it-real evidence gate ({date}) - RED BY DEFAULT; running + attesting every \
         required row =="
    );
    for row in Band::MakeItReal.required_rows() {
        print!("  {} … ", row.id);
        match run_and_capture(row.id, row.proof_command) {
            Ok(output) => {
                let argv: Vec<String> = row.proof_command.iter().map(|s| s.to_string()).collect();
                let att = RowAttestation::compute(row.id, &argv, &date, &output);
                let proof = format!(
                    "[{date}] PASS `cargo {}` (attested blake3:{})",
                    row.proof_command.join(" "),
                    &att.hash[..12]
                );
                println!("PASS [attested {}]", &att.hash[..12]);
                card.record(RowResult::pass_attested(row.id, proof, &date, att));
            }
            Err(reason) => {
                println!("RED - {reason}");
                card.record(RowResult::claimed_not_proven(row.id, reason, &date));
            }
        }
    }

    let manifest = AttestedScorecard::from_scorecard(&card, &date);
    if let Err(code) = write_artifact("make-it-real.json", &manifest.to_json()) {
        return code;
    }
    if let Err(code) = write_artifact("make-it-real.md", &manifest.render_markdown()) {
        return code;
    }
    println!("\nattested manifest + markdown written to testing/scorecards/");

    let verdict = manifest.validate(&date, DEFAULT_MAX_AGE_DAYS);
    if verdict.is_green() {
        if refresh_thresholds {
            if let Err(message) = refresh_thresholds_as_of(&date) {
                eprintln!("FATAL: {message}");
                return ExitCode::FAILURE;
            }
            println!("thresholds.toml as_of refreshed to {date} after the green live proof run");
        }
        println!("\nGATE: GREEN - every make-it-real row is fresh, hash-valid, attested PASS.");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "\nGATE: RED - the spine cannot claim production-real ({} problem(s); red by default):",
            verdict.problems.len()
        );
        for p in &verdict.problems {
            eprintln!("  {p}");
        }
        ExitCode::FAILURE
    }
}

fn run_and_capture(id: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new(env!("CARGO"))
        .args(args)
        .output()
        .map_err(|e| format!("could not spawn `cargo {}`: {e}", args.join(" ")))?;
    if !out.stdout.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&out.stdout));
    }
    if !out.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
    }
    let code = out.status.code().unwrap_or(-1);
    if !out.status.success() {
        Err(format!(
            "`cargo {}` exited non-zero ({code}) - the drill read RED (this floor is not yet real)",
            args.join(" ")
        ))
    } else {
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if combined.contains("MR009-SKIP") || combined.contains("SKIP: dev Postgres unreachable") {
            return Err(format!(
                "`cargo {}` exited zero but SKIPPED its live proof - skipped evidence is RED",
                args.join(" ")
            ));
        }
        for marker in required_output_markers(id) {
            if !combined.contains(marker) {
                return Err(format!(
                    "`cargo {}` exited zero but omitted required evidence marker `{marker}` - a \
                     vacuous or partial green is RED",
                    args.join(" ")
                ));
            }
        }
        Ok(output_bytes(code, &out.stdout, &out.stderr))
    }
}

fn required_output_markers(id: &str) -> &'static [&'static str] {
    match id {
        "MR-004" | "MR-012" => {
            &["the_production_graph_absence_ratchet_equals_the_committed_baseline ... ok"]
        }
        "MR-009" => &[
            "family=IDENTITY",
            "family=REVOCATION",
            "family=EVENTS",
            "family=CONTROL-PLANE",
            "family=KMS",
            "3-INSTANCE CONSISTENCY",
        ],
        "MR-010" => &[
            "oidc::tests::negative_alg_none_is_rejected ... ok",
            "saml::tests::xsw_1_forged_sibling_assertion_is_rejected ... ok",
            "webauthn::tests::negative_forged_signature_by_a_different_key_is_rejected ... ok",
            "ssh_auth::tests::negative_forged_signature_by_a_different_key_is_rejected ... ok",
        ],
        "MR-011" => &[
            "capability_crypto::tests::negative_forged_token_by_non_anchor_key_is_refused ... ok",
            "OK: a revoked machine/capability token stays denied across a fresh store instance",
        ],
        "MR-013" => &[
            "[MR-013] PASS  A=transaction-scoped-RLS-isolation",
            "[MR-013] PASS  B=no-GUC-bleed",
            "[MR-013] PASS  C=region-fail-fast",
        ],
        _ => &[],
    }
}

fn parse_args() -> Result<bool, String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [] => Ok(false),
        [flag] if flag == "--refresh-thresholds-as-of" => Ok(true),
        _ => Err(
            "usage: make-it-real-scorecard [--refresh-thresholds-as-of] (unknown arguments refused)"
                .to_string(),
        ),
    }
}

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .parent()
        .and_then(|path| path.parent())
        .unwrap_or(&crate_dir)
        .to_path_buf()
}

fn refresh_thresholds_as_of(date: &str) -> Result<(), String> {
    let path = workspace_root().join("thresholds.toml");
    let current = std::fs::read_to_string(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let mut found = 0_u8;
    let mut refreshed = String::with_capacity(current.len());
    for line in current.lines() {
        if line.starts_with("as_of = ") {
            found = found.saturating_add(1);
            refreshed.push_str(&format!("as_of = \"{date}\"\n"));
        } else {
            refreshed.push_str(line);
            refreshed.push('\n');
        }
    }
    if found != 1 {
        return Err(format!(
            "{} must contain exactly one top-level `as_of =` row; found {found}",
            path.display()
        ));
    }
    std::fs::write(&path, refreshed)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn write_artifact(name: &str, body: &str) -> Result<(), ExitCode> {
    let dir = workspace_root().join("testing").join("scorecards");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("FATAL: could not create {}: {e}", dir.display());
        return Err(ExitCode::FAILURE);
    }
    let path = dir.join(name);
    if let Err(e) = std::fs::write(&path, body) {
        eprintln!("FATAL: could not write {}: {e}", path.display());
        return Err(ExitCode::FAILURE);
    }
    Ok(())
}
