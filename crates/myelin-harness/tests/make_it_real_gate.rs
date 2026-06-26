//! The make-it-real evidence gate fail-closed proof tests (MR-005, the prompt's required
//! tamper-fixture meta-test). The analogue of MR-004's red fixture: it proves the gate actually
//! bites. A property does not exist until a test forces the failure (EI-01 §3), so each attack —
//! tamper, stale, missing, green-without-attestation — is exercised against a real fixture and
//! asserted to leave the gate RED, while a fully-attested fixture is asserted GREEN.

use myelin_harness::make_it_real::{
    AttestedScorecard, GateProblem, RowAttestation, DEFAULT_MAX_AGE_DAYS,
};
use myelin_harness::scorecard::{Band, RowResult, Scorecard};

const TODAY: &str = "2026-06-26";

/// A fully-attested make-it-real scorecard: every required row recorded as an attested PASS over
/// a fabricated-but-real captured output. This is the all-green baseline the gate accepts.
fn fully_attested(date: &str) -> AttestedScorecard {
    let mut card = Scorecard::new(Band::MakeItReal);
    for row in Band::MakeItReal.required_rows() {
        let argv: Vec<String> = row.proof_command.iter().map(|s| s.to_string()).collect();
        let output = format!("exit=0\n{} ran green\n", row.id).into_bytes();
        let att = RowAttestation::compute(row.id, &argv, date, &output);
        card.record(RowResult::pass_attested(
            row.id,
            format!("[{date}] PASS attested"),
            date,
            att,
        ));
    }
    AttestedScorecard::from_scorecard(&card, date)
}

#[test]
fn fully_attested_fixture_is_green() {
    let manifest = fully_attested(TODAY);
    let verdict = manifest.validate(TODAY, DEFAULT_MAX_AGE_DAYS);
    assert!(
        verdict.is_green(),
        "a fully fresh, hash-valid, attested scorecard must be GREEN; problems: {:?}",
        verdict.problems
    );
    // And it round-trips through the JSON manifest unchanged (the on-disk artifact is faithful).
    let reparsed = AttestedScorecard::from_json(&manifest.to_json()).expect("valid JSON");
    assert_eq!(reparsed, manifest);
    assert!(reparsed.validate(TODAY, DEFAULT_MAX_AGE_DAYS).is_green());
}

/// ATTACK 1 (the keystone): a hand-tampered scorecard whose recorded OUTPUT bytes were changed
/// (the digest edited) without re-attesting → the recomputed hash no longer matches → the gate
/// reds the row as a TAMPER, it does NOT silently pass.
#[test]
fn changed_output_bytes_is_a_tamper_not_a_silent_pass() {
    let mut manifest = fully_attested(TODAY);
    let target = &mut manifest.rows[0];
    let original = target.attestation.as_ref().unwrap().hash.clone();
    // Hand-edit the recorded output digest (as if someone changed what the command "printed").
    target.attestation.as_mut().unwrap().output_digest =
        blake3::hash(b"forged green output").to_hex().to_string();
    // The stored hash is unchanged — it still attests the ORIGINAL output.
    assert_eq!(target.attestation.as_ref().unwrap().hash, original);

    let verdict = manifest.validate(TODAY, DEFAULT_MAX_AGE_DAYS);
    assert!(!verdict.is_green(), "a changed-output row must RED the gate");
    assert!(
        verdict.problems.iter().any(|p| matches!(
            p,
            GateProblem::Tampered { id, .. } if id == &manifest.rows[0].id
        )),
        "the changed-output row must surface as a TAMPER, not pass silently: {:?}",
        verdict.problems
    );
}

/// ATTACK 2: a verdict flipped to PASS without binding evidence — the JSON row is marked
/// `passed: true` but carries no attestation → the gate reds it as a tamper (a PASS must prove it).
#[test]
fn verdict_flipped_to_pass_without_attestation_is_red() {
    let mut manifest = fully_attested(TODAY);
    // Simulate the hand-edit: drop the attestation but leave passed = true.
    manifest.rows[1].attestation = None;
    let verdict = manifest.validate(TODAY, DEFAULT_MAX_AGE_DAYS);
    assert!(!verdict.is_green());
    assert!(
        verdict.problems.iter().any(|p| matches!(
            p,
            GateProblem::Tampered { id, .. } if id == &manifest.rows[1].id
        )),
        "a PASS with no attestation must red as a tamper: {:?}",
        verdict.problems
    );
}

/// ATTACK 2b: reusing a VALID attestation from one row on another row (stealing a green). The
/// hash binds the row id, so the recomputed hash for the new id will not match → tamper.
#[test]
fn stealing_another_rows_attestation_is_a_tamper() {
    let mut manifest = fully_attested(TODAY);
    let stolen = manifest.rows[0].attestation.clone();
    manifest.rows[1].attestation = stolen; // row[1] now carries row[0]'s attestation
    let verdict = manifest.validate(TODAY, DEFAULT_MAX_AGE_DAYS);
    assert!(
        verdict.problems.iter().any(|p| matches!(
            p,
            GateProblem::Tampered { id, .. } if id == &manifest.rows[1].id
        )),
        "an attestation bound to a different row id must red as a tamper: {:?}",
        verdict.problems
    );
}

/// ATTACK 3: a stale-date fixture (a row attested long ago) → RED. Evidence that a floor was real
/// a year ago is not evidence it is real now.
#[test]
fn stale_dated_row_is_red() {
    // Attest everything on an old date, then validate as of TODAY.
    let manifest = fully_attested("2025-01-01");
    let verdict = manifest.validate(TODAY, DEFAULT_MAX_AGE_DAYS);
    assert!(!verdict.is_green(), "a far-past attestation must RED the gate");
    assert!(
        verdict
            .problems
            .iter()
            .all(|p| matches!(p, GateProblem::Stale { .. })),
        "every row should red as STALE (the attestations are hash-valid but old): {:?}",
        verdict.problems
    );
    // Sanity: with a window wide enough to cover the gap, the same manifest is green (freshness is
    // the only thing being tested here, not the attestation).
    assert!(manifest.validate(TODAY, 100_000).is_green());
}

/// ATTACK 4: a missing required row → RED (the drop-a-row half of the ratchet, now over the
/// attested manifest). Proven for EVERY required row id (you cannot game the gate by omitting any).
#[test]
fn dropping_any_required_row_reds_the_gate() {
    for dropped in Band::MakeItReal.required_rows() {
        let mut manifest = fully_attested(TODAY);
        manifest.rows.retain(|r| r.id != dropped.id);
        let verdict = manifest.validate(TODAY, DEFAULT_MAX_AGE_DAYS);
        assert!(
            verdict
                .problems
                .iter()
                .any(|p| matches!(p, GateProblem::Missing(id) if id == dropped.id)),
            "dropping required row {} must surface as MISSING and red the gate: {:?}",
            dropped.id,
            verdict.problems
        );
    }
}

/// The gate is RED BY DEFAULT: an empty manifest (no rows recorded) reds with every required row
/// missing. This is the fails-closed floor — nothing recorded means nothing proven.
#[test]
fn empty_manifest_is_red_by_default() {
    let empty = AttestedScorecard {
        band: Band::MakeItReal.to_string(),
        generated_on: TODAY.to_string(),
        rows: vec![],
    };
    let verdict = empty.validate(TODAY, DEFAULT_MAX_AGE_DAYS);
    assert!(!verdict.is_green());
    assert_eq!(
        verdict.problems.len(),
        Band::MakeItReal.required_rows().len(),
        "every required row must be MISSING in an empty manifest"
    );
}

/// A claimed-not-proven row (a real RED drill, recorded honestly) reds the gate — the expected
/// state over the incomplete spine (MR-009/010/011/012/013 not yet landed).
#[test]
fn claimed_not_proven_row_reds_the_gate() {
    let mut card = Scorecard::new(Band::MakeItReal);
    for (i, row) in Band::MakeItReal.required_rows().into_iter().enumerate() {
        if i == 0 {
            card.record(RowResult::claimed_not_proven(
                row.id,
                "drill read RED — the floor is not yet real",
                TODAY,
            ));
        } else {
            let argv: Vec<String> = row.proof_command.iter().map(|s| s.to_string()).collect();
            let att = RowAttestation::compute(row.id, &argv, TODAY, b"green");
            card.record(RowResult::pass_attested(row.id, "[d] PASS", TODAY, att));
        }
    }
    let manifest = AttestedScorecard::from_scorecard(&card, TODAY);
    let verdict = manifest.validate(TODAY, DEFAULT_MAX_AGE_DAYS);
    assert!(!verdict.is_green());
    assert!(verdict
        .problems
        .iter()
        .any(|p| matches!(p, GateProblem::NotProven { .. })));
}
