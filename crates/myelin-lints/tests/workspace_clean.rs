//! The LIVE gate: run ALL TWELVE architecture lints over Myelin's OWN `crates/*/src` tree and
//! fail the build on ANY violation. This is what makes the lints a committed gate, not just a
//! fixture exercise — "an uncommitted gate is no gate" (EI-01 §5). The whole point of the lint
//! ratchet is that the twelve bug-classes are impossible to MERGE, so the lints must run on real
//! code (P-S10 → P-017 shipped the four; P-S11 → P-018 completes the twelve).
//!
//! Documented, LOUD exclusions (never silent skips — EI-01 §4):
//! - `myelin-events/src/relay.rs` — the relay is the ONE legitimate broker-publish component
//!   (it drains the outbox to the broker; everything else emits via OutboxTx). Excluding it from
//!   `no-raw-publish` is correct BY DESIGN; the exclusion is named here, not hidden.
//! - `myelin-lints/**` — this crate's own fixtures/test-helpers and the lint scanners themselves
//!   contain the forbidden tokens as DATA (the strings the scanner looks for). Scanning the lint
//!   crate would flag its own pattern lists. Excluded and named.
//! - `**/tests/**` and `**/fixtures/**` — test fixtures deliberately contain red samples.
//! - `myelin-harness/src/bin/{sub-m0,id-m1,infra,m2}-scorecard.rs` — the band-boundary exit-gate
//!   runners (the SUB-M0 runner, P-S24 → P-039; the Identity M1→M2 runner, P-ID-21 → P-079; the
//!   infra integration runner, Stage 4; the M2 reactive-layer runner, M2 → M3):
//!   CI/test-support ORCHESTRATION tooling in the leaf test-support crate `myelin-harness` (NOT a
//!   production-DAG node, §2.9). They spawn `cargo test`/`cargo run` for each per-feature drill —
//!   the one legitimate host-exec site, exactly analogous to the relay's one broker-publish site.
//!   They are never on a user/agent request path, so the `no-host-exec` sandbox-escape rule (which
//!   guards PLATFORM code) does not apply. Named + LOUD, the lint stays fully live on every
//!   production crate; NOT weakened.
//!
//! The remaining-eight lints (P-S11) are designed to be MARKER-keyed where they target
//! not-yet-existing code (`no-cross-sync-cycle` fires only inside an `@identity-sink` file;
//! `flow-determinism` only inside an `@workflow-body`; `control-plane-pii-free` only on a
//! control-plane-named/marked struct) — so they admit the whole current workspace and tighten the
//! moment the consumer code lands. The token-fingerprint lints (`no-cross-db`, `residency-pin`,
//! `search-requires-acl-filter`, `no-llm-in-platform`, `forward-only-migration`) admit the
//! current substrate because no such call-site exists yet; if one is added it must be clean.

use myelin_lints::engine::run;
use myelin_lints::lints::all_twelve;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/myelin-lints; the workspace root is two levels up.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest dir")
        .to_path_buf()
}

/// Documented exclusions (path substrings). A file whose path contains any of these is skipped
/// for a NAMED, by-design reason (see the module docs). Loud: the list is in source, reviewed.
const EXCLUDED_SUBSTRINGS: &[&str] = &[
    "myelin-events/src/relay.rs", // the one legitimate broker-publish component (by design).
    "myelin-storage/src/pgrelay.rs", // the OLTP-co-located relay (Stage 2): same legitimate broker-publish role as relay.rs (BUS-2); outbox queries are relay-internal, not tenant-store.
    "myelin-storage/src/pg_migrator.rs", // the race-safe LIVE MIGRATION DRIVER (Stage 2, the P-S12 floor): `PgMigrator::apply`/`with_migration_lock` issue SCHEMA/INFRA statements only — `pg_advisory_lock`/`unlock`, the GLOBAL schema-version table `myelin_applied_migration` (no tenant column → nothing to bind), a `pg_locks` probe — NONE tenant-store queries (same non-tenant-store posture as pgrelay.rs). The migration DDL is run via `conn.execute(&str)`, not a `sqlx::query` builder, so the driver adds no tenant-store query of its own; pg.rs stays FULLY linted. NAMED, LOUD (see pg_migrator.rs), not weakened.
    "myelin-events/src/firehose.rs", // the EPHEMERAL firehose transport (EB-21/P-141): `firehose::publish` is the frozen contract-3.5/§5.5 method NAME for a DIFFERENT seam from the durable bus — §4.3 "the durable bus carries only pointer events" while the firehose carries ephemeral frames over its own publish/subscribe/resume API (a references-not-payloads pointer, not an outbox-emitted durable event). NAMED, LOUD (see firehose.rs).
    "myelin-knowledge/src/transport.rs", // the KNOWLEDGE COLLAB TRANSPORT (KN-P07/P-297): `CollabTransport::send_op`/`publish_presence` call `firehose.publish` (the EPHEMERAL collab op-stream + presence the arch sites on the firehose, §4.3/§2.1/ADR-04.5) — a references-not-payloads pointer, not an outbox-emitted durable event. Knowledge's DURABLE emit (coalesced knowledge.doc.updated via OutboxTx::emit) lives in emit.rs and stays FULLY linted. NAMED, LOUD (see transport.rs); same posture as firehose.rs.
    "myelin-ci-controlplane/src/log_pipeline.rs", // the CI LOG PIPELINE (CI-P20/P-363): `LogPipeline::ship_line` calls `firehose.publish(stream=ci-log, scope=run:<id>, frame)` (the EPHEMERAL CI log live-tail the arch sites on the firehose, 02 §7.1 / event-bus §4.3 "CI is the heaviest firehose producer") — a references-not-payloads byte-range pointer, not an outbox-emitted durable event. CI's DURABLE log emit (the COALESCED `ci.log.available` pointer via OutboxTx::emit) is the BUFFERED `LogAvailablePointer::to_draft` EventDraft the caller emits through the outbox (no raw `.publish(`). NAMED, LOUD (see log_pipeline.rs); same posture as transport.rs/firehose.rs.
    "myelin-harness/src/bin/sub-m0-scorecard.rs", // the SUB-M0 exit-gate runner: the one legitimate host-exec site (CI orchestration).
    "myelin-harness/src/bin/id-m1-scorecard.rs", // the Identity M1→M2 exit-gate runner (P-079): same legitimate host-exec site (CI orchestration).
    "myelin-harness/src/bin/infra-scorecard.rs", // the infra integration exit-gate runner (Stage 4): same legitimate host-exec site (spawns `cargo test --features integration` per drill).
    "myelin-harness/src/bin/m2-scorecard.rs", // the M2 reactive-shared-layer exit-gate runner (M2→M3): same legitimate host-exec site (spawns `cargo test`/`cargo run` per drill; AG-D4 with MYELIN_REQUIRE_KVM=1 so a real microVM must boot).
    "myelin-harness/src/bin/m3-scorecard.rs", // the M3 producer-subsystems exit-gate runner (M3→M4): same legitimate host-exec site (spawns `cargo test`/`cargo run` per Git+Knowledge drill, incl. the GIT-D10/D11-int + KN-D5/D7/D9/D10 `--features integration` rows against the live stack).
    "myelin-ci-sandbox/src/firecracker.rs", // the Firecracker default backend (CI-P2 → P-237): the ONE legitimate VMM-spawn site — spawning `firecracker --no-api --config-file` IS how the unified-sandbox boundary is CREATED (the seam's enforcement mechanism), not a bypass of it. Exactly analogous to the relay's broker-publish site. NAMED, LOUD (see firecracker.rs); the routing split (mutation→EffectApi) is unweakened.
    "myelin-ci-sandbox/src/gvisor.rs", // the gVisor `runsc` named-second backend (CI-P2 → P-237): the ONE legitimate runtime-spawn site — same posture as firecracker.rs (the seam's mechanism, not a bypass). NAMED, LOUD.
    "myelin-lints/",                   // this crate: scanners + fixtures carry the tokens as data.
    "/tests/",                         // test fixtures deliberately contain red samples.
    "/fixtures/",
];

fn is_excluded(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    EXCLUDED_SUBSTRINGS.iter().any(|ex| s.contains(ex))
}

/// Recursively collect every `*.rs` file under `crates/*/src`.
fn rust_source_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let crates_dir = root.join("crates");
    let entries = std::fs::read_dir(&crates_dir).expect("crates/ must exist");
    for crate_entry in entries.flatten() {
        let src = crate_entry.path().join("src");
        if src.is_dir() {
            collect_rs(&src, &mut out);
        }
    }
    out
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn the_twelve_lints_are_clean_over_the_workspace_source() {
    let root = workspace_root();
    let lints = all_twelve();
    let mut all_violations = Vec::new();
    let mut scanned = 0usize;

    for file in rust_source_files(&root) {
        if is_excluded(&file) {
            continue;
        }
        let src = std::fs::read_to_string(&file).expect("readable source file");
        if let Err(violations) = run(&lints, &src) {
            for v in violations {
                all_violations.push(format!("{}: {v}", file.display()));
            }
        }
        scanned += 1;
    }

    // Sanity: we actually scanned the tree (a 0-file run would be a vacuous green — the
    // un-wired-gate failure mode EI-01 §5 warns about).
    assert!(
        scanned >= 8,
        "expected to scan the workspace src tree (>= 8 files), scanned {scanned}"
    );

    assert!(
        all_violations.is_empty(),
        "the twelve architecture lints found violations in workspace source \
         (loud, never swallowed — fix the code, do not weaken the lint):\n{}",
        all_violations.join("\n")
    );
}
