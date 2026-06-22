//! Lint-fixture tests for the myelin-flow data model (P-FLOW-01 / P-197) — the committed-ratchet
//! proof (EI-01 §5) that the FOUR schema gates this prompt names are LIVE, not vacuously green:
//! `forward-only-migration`, `no-untagged-personal-data`, `tenant-predicate`, `no-cross-db`. Each
//! runs the REAL lint ([`myelin_lints`]) over a deliberately-broken (RED) fixture and over a clean
//! (GREEN) fixture, so a regression that lets a destructive/blocking migration / an untagged-PII
//! column / a tenant-less query / a cross-DB reach slip through fails THIS build (defense in depth:
//! the lint at source-scan, the runner at boot).
//!
//! The fixtures live under `tests/fixtures/*.rs.txt` — the SAME `/fixtures/` convention the
//! workspace lint-gate (`myelin-lints/src/bin/lint-gate.rs`) EXCLUDES from the live scan, so the
//! deliberately red samples here do NOT trip the real CI gate over the workspace (they are scanner
//! DATA, not real crate code). The flow schema's actual inline-PII key-ref columns are tagged in
//! `src/schema.rs`; the actual migrations are tenant-first + forward-only in `src/migrations.rs` —
//! these fixtures prove the SCANNERS bite.

use std::path::{Path, PathBuf};

use myelin_lints::{
    flow_determinism, forward_only_migration, no_cross_db, no_untagged_personal_data,
    tenant_predicate,
};

fn fixture(name: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

/// `forward-only-migration` (contract 1.5 / §9) REJECTS a destructive (`down`) + blocking-`ALTER`
/// migration and ADMITS the expand step (nullable add + `CREATE INDEX CONCURRENTLY`). The flow
/// journal is the source of truth; a destructive/blocking migration over it is the silent-data-loss
/// floor (EI-01 §2 — silent data loss outranks every feature).
#[test]
fn forward_only_migration_rejects_destructive_admits_expand() {
    let red = fixture("forward_only_migration.flow.red.rs.txt");
    assert!(
        !forward_only_migration().run(&red).is_empty(),
        "a down/blocking-ALTER migration must be REJECTED by forward-only-migration (§9, the data-loss floor)"
    );
    let green = fixture("forward_only_migration.flow.green.rs.txt");
    assert!(
        forward_only_migration().run(&green).is_empty(),
        "the expand step (nullable add + CONCURRENTLY index) must be ADMITTED by forward-only-migration"
    );
}

/// `no-untagged-personal-data` (contract 1.6 / gdpr §2.1) REJECTS an untagged inline-PII result
/// body and ADMITS the same column once `#[personal_data(...)]`-tagged with the canonical multi-line
/// six-tag `CryptoShred(subject_dek)` form the flow schema uses on its `result_key_ref` /
/// `payload_key_ref` crypto-shred locators. A regression that dropped the tag-tracking would leave
/// an un-erasable inline-PII result.
#[test]
fn no_untagged_personal_data_rejects_untagged_admits_tagged() {
    let red = fixture("no_untagged_personal_data.flow.red.rs.txt");
    assert!(
        !no_untagged_personal_data().run(&red).is_empty(),
        "an untagged inline-PII column (`message_body`) must be REJECTED by no-untagged-personal-data"
    );
    let green = fixture("no_untagged_personal_data.flow.green.rs.txt");
    assert!(
        no_untagged_personal_data().run(&green).is_empty(),
        "a #[personal_data(...)]-tagged column must be ADMITTED (the flow schema's CryptoShred tag shape)"
    );
}

/// `tenant-predicate` (ID-3 / EI-02 §1) REJECTS a tenant-less query and ADMITS a tenant-scoped one —
/// the no-cross-tenant-query-path floor every flow table's tenant-first PK enforces. A regression
/// that admitted a tenant-less query over `workflow_run` is the IDOR (F2) bug class.
#[test]
fn tenant_predicate_rejects_tenantless_admits_tenant_scoped() {
    let red = fixture("tenant_predicate.flow.red.rs.txt");
    assert!(
        !tenant_predicate().run(&red).is_empty(),
        "a tenant-less query must be REJECTED by tenant-predicate (ID-3, the IDOR floor)"
    );
    let green = fixture("tenant_predicate.flow.green.rs.txt");
    assert!(
        tenant_predicate().run(&green).is_empty(),
        "a tenant-scoped query must be ADMITTED by tenant-predicate"
    );
}

/// `no-cross-db` (ADR-01 / EI-02 §8) REJECTS a reach into a sibling service's internal storage
/// module and ADMITS coupling over the frozen contract surface. The flow engine is Postgres-EMBEDDED
/// in its OWN DB (architecture §2, one DB per service); a cross-DB reach is the coupling bug class.
#[test]
fn no_cross_db_rejects_storage_reach_admits_contract_coupling() {
    let red = fixture("no_cross_db.flow.red.rs.txt");
    assert!(
        !no_cross_db().run(&red).is_empty(),
        "a reach into a sibling service's internal store must be REJECTED by no-cross-db (ADR-01)"
    );
    let green = fixture("no_cross_db.flow.green.rs.txt");
    assert!(
        no_cross_db().run(&green).is_empty(),
        "coupling over the frozen contract surface (ArtifactRef/TenantId) must be ADMITTED by no-cross-db"
    );
}

// ---------------------------------------------------------------------------------------------
// `flow-determinism` (contract 1.6 / index 9.2, architecture §10.3) — P-FLOW-08 / P-200.
//
// THIS prompt's deliverable: the flow-determinism lint's RED + GREEN fixtures expressed against
// the REAL `WfCtx` surface (P-FLOW-04 / P-199). The committed-ratchet proof (EI-01 §5) is a PAIR
// of facts, both mechanically asserted here (loud, inverted-safe — never `|| true`):
//   1. the RED fixture (raw clock/RNG/IO in a `@workflow-body`) is REJECTED by the lint, AND it
//      would not compile against `WfCtx` (it reads `SystemTime::now()`/`rand::random()` etc.);
//   2. the GREEN fixture (the same logic via `ctx.now()`/`ctx.rand()`/`ctx.activity(..)`) is
//      ADMITTED by the lint AND COMPILES against the real `myelin_flow::WfCtx` — proven by the
//      `include!` compile-pass below, so "the green fixture compiles" is an artifact, not a claim.
// ---------------------------------------------------------------------------------------------

/// **The lint REJECTS the raw-clock/RNG/IO red fixture and ADMITS the WfCtx-routed green fixture.**
/// The fingerprint scanner ([`flow_determinism`], the §2.11 lint) fires on a raw non-deterministic
/// read inside a `@workflow-body` (the non-deterministic-replay bug class) and stays silent when
/// every read flows through the deterministic `WfCtx` surface. A regression that let a raw
/// `SystemTime::now()` into a workflow body slip past would fail THIS assertion (the gate bites).
#[test]
fn flow_determinism_rejects_raw_nondeterminism_admits_wfctx() {
    let red = fixture("flow_determinism.flow.red.rs.txt");
    let violations = flow_determinism().run(&red);
    assert!(
        !violations.is_empty(),
        "a workflow body reading SystemTime/RNG/IO outside WfCtx must be REJECTED by \
         flow-determinism (the non-deterministic-replay floor, index 9.2/§10.3)"
    );
    // The lint must catch EVERY raw read (clock + rng + sleep + uuid), not just the first — a
    // single-hit scanner would let three of the four diverge silently.
    assert!(
        violations.len() >= 4,
        "all four raw non-deterministic reads (SystemTime::now / rand:: / tokio::time::sleep / \
         Uuid::new_v4) must each be flagged, got {}: {violations:?}",
        violations.len()
    );

    let green = fixture("flow_determinism.flow.green.rs.txt");
    assert!(
        flow_determinism().run(&green).is_empty(),
        "the same logic expressed via ctx.now()/ctx.rand()/ctx.activity(..) must be ADMITTED by \
         flow-determinism (it reads no clock/RNG/IO outside the deterministic WfCtx surface)"
    );
}

/// **The GREEN fixture COMPILES against the real `myelin_flow::WfCtx`.** This `include!`s the exact
/// green-fixture source into the test crate, so a build of this test crate is a real `rustc`
/// compile of the green workflow body against the frozen `WfCtx` surface (`now`/`rand`/`activity`).
/// If the fixture ever drifted off the real surface (a renamed method, a changed signature), THIS
/// test crate would fail to COMPILE — the "green admits" half of the gate is an artifact, not a
/// claim. (The RED fixture is deliberately NOT included: it would not compile, which is its point.)
mod green_compiles {
    include!("fixtures/flow_determinism.flow.green.rs.txt");

    /// Reference the included workflow fn so it is type-checked (not dead-code-eliminated before
    /// `rustc` checks its body against `WfCtx`).
    #[test]
    fn flow_determinism_green_fixture_compiles_against_real_wfctx() {
        // A function pointer to the included body forces its full type-check against the real
        // `WfCtx` surface. Calling it would require a live outbox/journal (that is the WfCtx unit
        // tests' job); compiling it is THIS test's proof.
        let _body: fn(&mut myelin_flow::WfCtx) = nightly_digest_workflow;
        let _ = make_error();
    }
}
