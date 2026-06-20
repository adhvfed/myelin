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
//! - `myelin-harness/src/bin/{sub-m0,id-m1,infra}-scorecard.rs` — the band-boundary exit-gate
//!   runners (the SUB-M0 runner, P-S24 → P-039; the Identity M1→M2 runner, P-ID-21 → P-079; the
//!   infra integration runner, Stage 4):
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
    "myelin-events/src/firehose.rs", // the EPHEMERAL firehose transport (EB-21/P-141): `firehose::publish` is the frozen contract-3.5/§5.5 method NAME for a DIFFERENT seam from the durable bus — §4.3 "the durable bus carries only pointer events" while the firehose carries ephemeral frames over its own publish/subscribe/resume API (a references-not-payloads pointer, not an outbox-emitted durable event). NAMED, LOUD (see firehose.rs).
    "myelin-harness/src/bin/sub-m0-scorecard.rs", // the SUB-M0 exit-gate runner: the one legitimate host-exec site (CI orchestration).
    "myelin-harness/src/bin/id-m1-scorecard.rs", // the Identity M1→M2 exit-gate runner (P-079): same legitimate host-exec site (CI orchestration).
    "myelin-harness/src/bin/infra-scorecard.rs", // the infra integration exit-gate runner (Stage 4): same legitimate host-exec site (spawns `cargo test --features integration` per drill).
    "myelin-lints/", // this crate: scanners + fixtures carry the tokens as data.
    "/tests/",       // test fixtures deliberately contain red samples.
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
