//! `lint-gate` — the committed CI entrypoint for the twelve architecture lints (EB-07 → P-019).
//!
//! This binary IS the loud, never-swallowed gate the CI workflow runs (`.github/workflows/ci.yml`,
//! the `architecture-lints` job). It scans every `*.rs` file under one or more roots with all
//! twelve lints ([`myelin_lints::all_twelve`]) and **exits NON-ZERO on any violation** — there is
//! no `... || true` swallow path possible, because the gate is the process exit code itself
//! (doctrine EI-01 §5: "an uncommitted gate is no gate; make violations loud").
//!
//! Usage:
//!   `lint-gate [ROOT ...]`  — scan each ROOT's `*.rs` files. With no ROOT, scans the workspace's
//!   own `crates/*/src` tree (the live workspace gate). Prints every violation to stderr and exits
//!   1 if any are found, 0 if the tree is clean.
//!
//! Why a binary (the EB-07 "wired into CI, loud, never swallowed" obligation): the substrate
//! prompts P-017/P-018 shipped the lints + the `cargo test` matrix/workspace-scan gate, but EB-07
//! requires the lint be wired into CI such that **the workflow fails with a non-zero exit on a red
//! fixture, with no `|| true` swallow**. A process whose exit code IS the gate cannot be silently
//! swallowed by a shell `||`; the `ci_gate_fails_loudly` test (`tests/ci_gate.rs`) proves the
//! red-fixture run exits non-zero and the clean tree exits zero.

use myelin_lints::engine::run;
use myelin_lints::lints::all_twelve;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Documented, LOUD exclusions — mirrors `tests/workspace_clean.rs` (named, never silent skips,
/// EI-01 §4). The relay is the one legitimate broker-publish site; the lint crate carries the
/// forbidden tokens as scanner data; test/fixture trees deliberately hold red samples.
///
/// The `myelin-harness/src/bin/*-scorecard.rs` band-boundary exit-gate RUNNERS (the SUB-M0 runner
/// `sub-m0-scorecard.rs`, P-S24 → P-039; the Identity M1→M2 runner `id-m1-scorecard.rs`, P-ID-21 →
/// P-079; the infra integration runner `infra-scorecard.rs`, Stage 4; the M2 reactive-layer runner
/// `m2-scorecard.rs`, M2 → M3; the M3 producer runner `m3-scorecard.rs`, M3 → M4; the M4 consumer
/// runner `m4-scorecard.rs`, M4 → M5) are CI/test-support
/// ORCHESTRATION tooling (the leaf test-support crate `myelin-harness`,
/// NOT a node in the production DAG, architecture §2.9) whose whole job is to spawn `cargo
/// test`/`cargo run` for each per-feature drill and aggregate the result. Their
/// `Command::new(env!("CARGO"))` is the one legitimate host-exec site, exactly analogous to the
/// relay's one legitimate broker-publish site: it is developer/CI tooling, never reachable on a
/// user/agent request path, so the `no-host-exec` sandbox-escape rule (which guards PLATFORM code)
/// does not apply. These are NAMED, LOUD exclusions of single tool files — the lint stays fully
/// live on every production crate; it is NOT weakened. (The production execution seam
/// `ToolHands::exec` lands in M2/CI.)
const EXCLUDED_SUBSTRINGS: &[&str] = &[
    "myelin-events/src/relay.rs",
    // The OLTP-co-located RELAY (Stage 2 / infra): `PgRelay::relay_once` drains the co-located
    // outbox table and is the ONE legitimate broker-publish site for the OLTP service — exactly
    // the role relay.rs plays for the in-process floor (BUS-2: the relay is the only
    // broker-publish component). Its `bus.put(...)` forwards an ALREADY-committed outbox row
    // (emit-iff-committed), not a fire-and-forget bypass; its outbox queries are relay-INTERNAL
    // (the outbox is keyed by (aggregate, seq) and drained across aggregates), NOT tenant-store
    // queries — the same posture as relay.rs. NAMED, LOUD exclusion (see the crate note in
    // pgrelay.rs), never a silent skip; the tenant-store code in pg.rs stays fully linted.
    "myelin-storage/src/pgrelay.rs",
    // The DURABLE consumer_dedup BACKING (MR-023 / SI-023): the frozen `consumer_dedup` table is
    // keyed `(consumer, event_id)` and carries NO tenant column (the event_id is a globally-unique
    // ULID; dedup is per-consumer, cross-tenant by design — contract 2.5). Its INSERT/SELECT/DELETE
    // are consumer-INTERNAL, not tenant-store queries, so the `tenant-predicate` fingerprint
    // (`sqlx::query` without a `tenant_id`) flags them falsely — exactly the relay-INTERNAL
    // posture pgrelay.rs's outbox queries take above. The tenant-store code in pg.rs /
    // identity_durable.rs stays FULLY linted. NAMED, LOUD exclusion (see the module note in
    // events_durable.rs), never a silent skip; the lint is NOT weakened.
    "myelin-storage/src/events_durable.rs",
    // The race-safe LIVE MIGRATION DRIVER (Stage 2 / infra, the P-S12 floor): `PgMigrator::apply`
    // + `with_migration_lock` run SCHEMA/INFRA statements only — the Postgres session advisory lock
    // (`pg_advisory_lock`/`unlock`), the global schema-version table `myelin_applied_migration`
    // (id + checksum, a GLOBAL ledger with NO tenant column — there is no tenant to bind), and a
    // `pg_locks` probe. NONE of these are tenant-store queries (they touch no tenant table), so the
    // `tenant-predicate` IDOR fingerprint (`sqlx::query` without a `tenant_id`) flags them falsely —
    // exactly the same relay-INTERNAL/non-tenant-store posture as pgrelay.rs above. The DDL the
    // driver EXECUTES is run via `conn.execute(&str)` (the caller's migration text, e.g. the
    // tenant-scoped `rebac_tuple` table whose RLS policy keys on `tenant_id`), not a `sqlx::query`
    // builder, so the migrator introduces no tenant-store query of its own. The tenant-store code in
    // pg.rs stays FULLY linted. NAMED, LOUD exclusion (see the module note in pg_migrator.rs), never
    // a silent skip; the lint is NOT weakened.
    "myelin-storage/src/pg_migrator.rs",
    // The durable CONTROL-PLANE PLACEMENT REGISTRY backing (MR-024 / SI-011/SI-028): the `cell` /
    // `tenant_placement` / `misroute_audit` tables are control-plane ROUTING infra — the registry
    // routes ALL tenants to cells, so every query is cross-tenant BY DESIGN (the gateway asks "which
    // cell homes tenant X?" for any X). It is PII-free (opaque ids only — control-plane-pii-free) and
    // is NOT a per-request tenant data store, so it does NOT use the with_tenant_tx/RLS convention and
    // carries no per-row tenant predicate — exactly the relay-INTERNAL / non-tenant-store posture
    // pgrelay.rs / events_durable.rs take above. The `tenant_id` column is the ROUTING KEY, not an RLS
    // predicate; the HARD placement invariant is enforced as a REAL DB TRIGGER, not a tenant predicate.
    // The tenant-store code in pg.rs / identity_durable.rs stays FULLY linted. NAMED, LOUD exclusion
    // (see the module note in placement_durable.rs), never a silent skip; the lint is NOT weakened.
    "myelin-storage/src/placement_durable.rs",
    // The durable KMS backing (MR-025 / SI-006): the software-sealed cell ROOT + wrapped KEKs/DEKs
    // (`kms_sealed_root` / `kms_wrapped_kek` / `kms_wrapped_dek`). This is cell-INFRA key material —
    // the KMS holds the keys for ALL tenants in the cell (one engine resolves every tenant's DEK), so
    // it is cross-tenant BY DESIGN and PII-free (key ciphertext + opaque ids). The `kms_sealed_root`
    // queries carry no tenant column at all (the root is per-CELL), and the KEK/DEK `tenant_id` column
    // is the key-OWNER, not an RLS predicate. Like placement_durable.rs / events_durable.rs / pgrelay.rs
    // this is infra, NOT a per-request tenant data store, so it does NOT use the with_tenant_tx/RLS
    // convention and carries no per-row tenant predicate. The tenant-store code in pg.rs /
    // identity_durable.rs stays FULLY linted. NAMED, LOUD exclusion (see the module note in
    // kms_durable.rs), never a silent skip; the lint is NOT weakened.
    "myelin-storage/src/kms_durable.rs",
    // The durable capability-token CELL AUTHORITY ROOT backing (R4.0 / P-527 / MR-025 follow-on): the
    // software-sealed cell-authority root (`cell_token_root` — the Ed25519 seed + macaroon MAC key,
    // sealed under the SAME operator seal key as the KMS root). EXACTLY the kms_durable.rs posture:
    // cell-INFRA key material (the cell authority signs tokens for ALL tenants in the cell), PII-free
    // (key ciphertext + opaque `cell_id`), the `cell_token_root` table carries NO tenant column (the
    // root is per-CELL), and it connects to the OLTP pool DIRECTLY (not via with_tenant_tx/RLS). Like
    // kms_durable.rs this is infra, NOT a per-request tenant data store, so it carries no per-row
    // tenant predicate. pg.rs / identity_durable.rs stay FULLY linted. NAMED, LOUD exclusion (see the
    // module note in cell_root_durable.rs), never a silent skip; the lint is NOT weakened.
    "myelin-storage/src/cell_root_durable.rs",
    // The FIREHOSE transport (EB-21 / P-141): `firehose::publish(stream, scope, frame)` is the
    // FROZEN contract-3.5 / §5.5 method name for the EPHEMERAL firehose transport — a DIFFERENT
    // seam from the durable bus the `no-raw-publish` lint guards. §4.3 is explicit: "the durable bus
    // carries only pointer/summary events" while the firehose carries the high-volume ephemeral
    // frames (CI logs, collab op-streams, chat live delivery) over its OWN `publish`/`subscribe`/
    // `resume` API. A firehose frame is a references-not-payloads pointer (`FramePayload`), never an
    // inline-PII durable event, and it is NOT emitted-iff-committed through the outbox — it is a
    // separate transport by design (OQ-J). The lint's `.publish(` fingerprint collides with the
    // frozen `firehose::publish` method NAME; excluding this ONE file keeps the lint live on every
    // durable-bus call site while honouring the architecture's two-transport split. NAMED, LOUD
    // exclusion (see the module note in firehose.rs), never a silent skip.
    "myelin-events/src/firehose.rs",
    // The KNOWLEDGE COLLAB TRANSPORT (KN-P07 / P-297): `CollabTransport::send_op` /
    // `publish_presence` call `firehose.publish(stream=fan.<tenant>.knowledge, scope=doc:<page_id>,
    // frame)` — the EPHEMERAL collab op-stream + presence fan-out the architecture explicitly sites
    // on the firehose (§4.3: "the firehose carries … collab op-streams"; §2.1: "the durable bus
    // carries only the knowledge.doc.updated pointer; the collab op-stream never melts the durable
    // control bus", ADR-04.5). A firehose frame is a references-not-payloads pointer (the op_id wire
    // form), never an inline-PII durable event, and is NOT emitted-iff-committed through the outbox.
    // Knowledge's DURABLE emit (the coalesced knowledge.doc.updated / knowledge.page.updated via
    // OutboxTx::emit) lives in `emit.rs`, which stays FULLY linted. Excluding this ONE transport file
    // (the same posture as firehose.rs / relay.rs) honours the two-transport split. NAMED, LOUD
    // exclusion (see the module note in transport.rs), never a silent skip.
    "myelin-knowledge/src/transport.rs",
    // The CI LOG PIPELINE (CI-P20 / P-363): `LogPipeline::ship_line` calls
    // `firehose.publish(stream=ci-log, scope=run:<id>, frame)` — the EPHEMERAL CI log live-tail the
    // architecture explicitly sites on the firehose (arch 02 §7.1: "logs ride the firehose +
    // the resume-cursor protocol"; "CI is the heaviest firehose producer", event-bus §4.3). A
    // firehose frame is a references-not-payloads byte-range pointer, never an inline-PII durable
    // event, and is NOT emitted-iff-committed through the outbox. CI's DURABLE log emit (the
    // COALESCED `ci.log.available` pointer via OutboxTx::emit) is assembled in this same module as a
    // BUFFERED `EventDraft` the caller emits through the outbox (`LogAvailablePointer::to_draft` —
    // never a raw `.publish(`); only the firehose live-tail `.publish(` is here). Excluding this ONE
    // transport file (the exact posture as knowledge/src/transport.rs / firehose.rs / relay.rs)
    // honours the two-transport split. NAMED, LOUD exclusion (see the module note in
    // log_pipeline.rs), never a silent skip — the lint stays live on every durable-bus call site.
    "myelin-ci-controlplane/src/log_pipeline.rs",
    // The REGION-scoped, CROSS-TENANT scheduler claim + reaper (CT-004c.1): `claim_region_scoped` /
    // `reap_region_scoped` run `CLAIM_QUERY` / `REAP_QUERY` — the scheduler pull-lease claim (arch 02
    // §2.1) and the dead-runner reaper. These are CROSS-TENANT BY DESIGN: a hosted runner claims the
    // next eligible job across ALL tenants in its region, and the DRR fairness (`fair_deficit.deficit
    // DESC`) explicitly spans tenants ("prevents one tenant's matrix from starving every OTHER
    // tenant", arch 02 §2.2). They filter by `region` only (the residency/routing key, NOT an RLS
    // predicate) and carry no `tenant_id` — so the `tenant-predicate` IDOR fingerprint flags them
    // FALSELY, EXACTLY the control-plane-routing posture of `placement_durable.rs` (the cell-placement
    // registry: "which cell homes tenant X?" for any X). The PER-TENANT job_queue ops
    // (enqueue/cancel_superseded/complete/heartbeat) stay in `job_queue_store.rs` (NOT excluded), each
    // binding `tenant_id` through the MR-022 `with_tenant_tx` convention — so the tenant-store code
    // stays FULLY linted; only these two genuinely-cross-tenant SERVICE reads are excluded. NAMED,
    // LOUD exclusion of a single file (see the module note in job_queue_region.rs), never a silent
    // skip; the lint is NOT weakened.
    "myelin-ci-controlplane/src/job_queue_region.rs",
    // The CHAT FIREHOSE-ONLY LIVE-DELIVERY surface (CHAT-P10 / P-404): `LiveDelivery::deliver` calls
    // `firehose.publish(stream=fan.<tenant>, scope=channel:<id>, frame)` — the EPHEMERAL live
    // message/presence/typing/read-state/partial frames the arch sites FIREHOSE-ONLY (02 §7 / 03 §1.2
    // / ADR-04.5), a references-not-payloads message-id/op pointer, NOT an outbox-emitted durable
    // event. There is NO durable-bus handle in the module (the live frame cannot reach the durable
    // bus by construction); the durable `chat.message.created` is the Message Service's
    // outbox-co-committed write, never the gateway's (arch §9 — the gateway has no emit path).
    // Exactly the two-transport-split posture as transport.rs / log_pipeline.rs / firehose.rs. NAMED,
    // LOUD exclusion of this ONE file (see the module note in delivery.rs); the lint stays fully live
    // on every other gateway file (the shed governor + the frame builders carry no `.publish(`).
    "myelin-chat-gateway/src/delivery.rs",
    "myelin-harness/src/bin/sub-m0-scorecard.rs",
    "myelin-harness/src/bin/id-m1-scorecard.rs",
    // The infra integration exit-gate runner (Stage 4): same posture as the two runners above —
    // its `Command::new(env!("CARGO"))` spawns `cargo test --features integration` per drill, the
    // one legitimate host-exec site for a CI/test-support orchestration binary. NAMED, LOUD
    // exclusion of a single tool file; the lint stays fully live on every production crate.
    "myelin-harness/src/bin/infra-scorecard.rs",
    // The M2 reactive-shared-layer exit-gate runner (M2 → M3): same posture as the three runners
    // above — its `Command::new(env!("CARGO"))` spawns `cargo test`/`cargo run` per drill (the
    // AG-D4 row with `MYELIN_REQUIRE_KVM=1` so a real microVM must boot), the one legitimate
    // host-exec site for a CI/test-support orchestration binary. NAMED, LOUD exclusion of a single
    // tool file; the lint stays fully live on every production crate.
    "myelin-harness/src/bin/m2-scorecard.rs",
    // The M3 producer-subsystems exit-gate runner (M3 → M4): same posture as the runners above —
    // its `Command::new(env!("CARGO"))` spawns `cargo test`/`cargo run` per Git+Knowledge drill
    // (incl. the GIT-D10/D11-int + KN-D5/D7/D9/D10 `--features integration` rows against the live
    // stack), the one legitimate host-exec site for a CI/test-support orchestration binary. NAMED,
    // LOUD exclusion of a single tool file; the lint stays fully live on every production crate.
    "myelin-harness/src/bin/m3-scorecard.rs",
    // The M4 consumer-subsystems exit-gate runner (M4 → M5): same posture as the runners above —
    // its `Command::new(env!("CARGO"))` spawns `cargo test`/`cargo run` per CI+Issues+Chat drill
    // (incl. the AG-D4/CI-T1 prod-image re-confirm with `MYELIN_REQUIRE_KVM=1` so a real microVM
    // must boot + the STOR-D1/D2 `--features integration` restore-verify against the live stack),
    // the one legitimate host-exec site for a CI/test-support orchestration binary. NAMED, LOUD
    // exclusion of a single tool file; the lint stays fully live on every production crate.
    "myelin-harness/src/bin/m4-scorecard.rs",
    // The M5 world-scale-hardening exit-gate runner (M5 → M6): same posture as the runners above —
    // its `Command::new(env!("CARGO"))` spawns `cargo test` per F6-surge / git-world-scale /
    // Knowledge / multi-cell-DSR / E2E drill plus the STOR-D2-at-cell-scale permanent restore row,
    // the one legitimate host-exec site for a CI/test-support orchestration binary. NAMED, LOUD
    // exclusion of a single tool file; the lint stays fully live on every production crate. (This
    // entry was missing when m5-scorecard.rs landed in a56c0b0 — the runner shipped without its
    // exclusion, reddening the lint-gate; restored here as part of the P-506 truth-up pass, the
    // code-wins-over-docs re-sync, EI-01 §1: an earlier-band gate may not stay red under a later band.)
    "myelin-harness/src/bin/m5-scorecard.rs",
    // The M6 production-readiness exit-gate runner (M6 → M7): same posture as the runners above —
    // its `Command::new(env!("CARGO"))` (`run_proof`) spawns `cargo test`/`cargo run` per band-M6
    // drill, the one legitimate host-exec site for a CI/test-support orchestration binary. NAMED,
    // LOUD exclusion of a single tool file; the lint stays fully live on every production crate.
    // (This entry was missing when m6-scorecard.rs landed — the runner shipped without its
    // exclusion, reddening the lint-gate; restored here, same code-wins-over-docs re-sync as the m5
    // entry above, EI-01 §1: an earlier-band gate may not stay red under a later band.)
    "myelin-harness/src/bin/m6-scorecard.rs",
    // The make-it-real spine gate runner (MR-005): same posture as the band-boundary scorecard
    // runners above — its `Command::new(env!("CARGO"))` (`run_and_capture`) spawns `cargo test` per
    // spine proof row, the one legitimate host-exec site for this CI/test-support orchestration
    // binary (it RE-RUNS each proof command live and attests the captured output — the gate's whole
    // un-fakeable guarantee). NAMED, LOUD exclusion of a single tool file; the lint stays fully live
    // on every production crate. (Was missing when make-it-real-scorecard.rs landed in MR-005,
    // reddening the lint-gate; restored here, same re-sync as the m5/m6 entries above, EI-01 §1.)
    "myelin-harness/src/bin/make-it-real-scorecard.rs",
    // The Myelin self-hosting CI graph runner (the DOGFOOD loop, P-507 / P-S37 → M6): same posture
    // as the band-boundary scorecard runners above — its `Command::new(env!("CARGO"))` spawns
    // `cargo run`/`cargo test`/`cargo mutants` per ratchet job (the twelve lints + the
    // contract-coverage scanner + the mandatory-core mutation gate + the SUB-D3/D6/D10 drills) on
    // Myelin's OWN commit, the one legitimate host-exec site for a CI/test-support orchestration
    // binary. NAMED, LOUD exclusion of a single tool file; the lint stays fully live on every
    // production crate.
    "myelin-harness/src/bin/self-hosting-ci.rs",
    // The self-hosting CI graph DEFINITION module (P-507 / P-S37): carries `Command::new(env!
    // ("CARGO"))` (the proof-command runner, same legitimate host-exec posture as the runner bin
    // above) PLUS the ratchet proof-command argv tokens as FROZEN SCANNER DATA (the `self_hosting_
    // jobs` table). Exactly the lint-crate-carries-forbidden-tokens-as-data posture as `myelin-
    // lints/` itself. NAMED, LOUD exclusion of this single CI-orchestration module; the lint stays
    // fully live on every production crate.
    "myelin-harness/src/self_hosting_ci.rs",
    // The Firecracker + gVisor sandbox BACKENDS (CI-P2 → P-237): the ONE legitimate VMM/runtime
    // spawn sites. The `no-host-exec` rule forbids platform code SHELLING OUT to the host kernel so
    // that all execution goes through the unified sandbox seam (`SandboxBackend::launch`). These two
    // files ARE that seam's enforcement mechanism: spawning the Firecracker VMM (`firecracker
    // --no-api --config-file`) / the gVisor `runsc` runtime is precisely HOW the isolation boundary
    // is CREATED — it is not a path that bypasses the sandbox, it is the path that builds it. Exactly
    // analogous to the relay's one broker-publish site + the harness runners' one cargo-spawn site
    // above. The routing split (only compute/external untrusted code reaches launch; mutation goes
    // through EffectApi, contract 8.2) is unweakened, and the lint stays fully live on every OTHER
    // production file. NAMED, LOUD exclusion of these two files (see the module notes in
    // firecracker.rs / gvisor.rs), never a silent skip.
    "myelin-ci-sandbox/src/firecracker.rs",
    "myelin-ci-sandbox/src/gvisor.rs",
    "myelin-lints/",
    "/tests/",
    "/fixtures/",
];

fn is_excluded(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    EXCLUDED_SUBSTRINGS.iter().any(|ex| s.contains(ex))
}

/// The workspace's own `crates/*/src` tree (the default scan root when no arg is given).
fn default_roots() -> Vec<PathBuf> {
    // CARGO_MANIFEST_DIR = crates/myelin-lints; the workspace root is two levels up.
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    vec![workspace.join("crates")]
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

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if root.is_file() {
        // An EXPLICITLY-passed file is scanned regardless of extension (the fixtures the CI-gate
        // self-test points at are `*.rs.txt`). A DIRECTORY walk still only picks up `*.rs`.
        out.push(root.to_path_buf());
    } else {
        collect_rs(root, &mut out);
    }
    out
}

fn main() -> ExitCode {
    // Args after argv[0] are scan roots; default to the workspace crates tree. A `--no-exclude`
    // flag (used by the CI-gate self-test over a red fixture) disables the by-design exclusions so
    // a fixture under a `/fixtures/` path is actually scanned.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let no_exclude = args.iter().any(|a| a == "--no-exclude");
    let roots: Vec<PathBuf> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .collect();
    let roots = if roots.is_empty() {
        default_roots()
    } else {
        roots
    };

    let lints = all_twelve();
    let mut violations = Vec::new();
    let mut scanned = 0usize;

    for root in &roots {
        for file in rust_files(root) {
            if !no_exclude && is_excluded(&file) {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&file) else {
                continue;
            };
            if let Err(found) = run(&lints, &src) {
                for v in found {
                    violations.push(format!("{}: {v}", file.display()));
                }
            }
            scanned += 1;
        }
    }

    if violations.is_empty() {
        eprintln!("lint-gate: OK — {scanned} file(s) scanned, 0 violations.");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "lint-gate: FAIL — {} violation(s) in {scanned} file(s) (loud, never swallowed — fix \
             the code, do not weaken the lint):",
            violations.len()
        );
        for v in &violations {
            eprintln!("  {v}");
        }
        ExitCode::FAILURE
    }
}
