//! **CT-004b (M4): the LIVE `ci-dispatch.trigger` bus consumer — the seam that makes CI dispatch
//! actually FIRE on a push.**
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §1 (trigger → dispatch: match → dedup → trust-stamp → resolve → reserve/start) + §7.4 (the
//! authored `.myelin/ci.*` config). This module is the CENSUS GAP the shell + the pure cores left
//! open: CI-P6 shipped the `consumer_dedup` ledger SHAPE, CI-P10 shipped the matcher/dedup/trust
//! cores ([`crate::dispatch`]), CI-P11 shipped the resolve/reserve cores ([`crate::resolve`]), and
//! CT-004b (the parser, [`crate::config`]) turned a config FILE into a [`CiDefinition`]. NOTHING wired
//! them onto the bus: `dispatch_app_spec` registered `consumers: Vec::new()`, so a push triggered NO
//! run. This module closes that: a live [`EventHandler`] that, on a matching `git.ref.updated` (+ the
//! PR triggers), reads the pushed repo's `.myelin/ci.*` at `new_oid`, parses + resolves it, and
//! persists the DURABLE reserve/start bundle.
//!
//! ## The pipeline (in [`CiTriggerHandler::handle`], idempotent on `event_id`)
//! 1. **Subject-match** `git.ref.updated` / `git.pr.opened` / `git.pr.synchronized` — a non-CI event
//!    is a clean no-op ([`DispatchOutcome::Skip`]).
//! 2. **Read** `.myelin/ci.toml` (then `.json`) at `new_oid` via the [`GitConfigReader`] seam. ABSENT
//!    ⇒ a clean "no pipeline armed" skip (NOT an error). PRESENT ⇒ [`parse_ci_config`].
//! 3. A [`CiConfigError`] is a **fail-closed, SURFACED skip** — a malformed config does NOT crash the
//!    consumer, does NOT start a run, and is surfaced (a loud structured log via
//!    [`SkipReason::ConfigError`], never a silent swallow — see the surfacing decision below).
//! 4. **Compile** the armed trigger ([`compile_trigger`]) and ask: does THIS event match? No ⇒ skip.
//! 5. **Stamp trust** ([`stamp_trust`]) — the [`TrustStamp`] rides the run + every check (X-1).
//! 6. **Resolve** the CAS snapshot ([`resolve_snapshot`]) — a [`ResolveError`] (e.g. a floating tag)
//!    is a fail-closed surfaced skip.
//! 7. **Reserve/start** ([`reserve_and_start`]) → the ATOMIC bundle, persisted through the
//!    [`ReserveStore`] seam (the `ci_run` row + `ci.run.started` + the first queued
//!    `ci.check.updated` per context).
//!
//! ## The config-error SURFACING decision (the prompt DoD)
//! A [`CiConfigError`] (or a [`ResolveError`]) is surfaced as a **loud, structured
//! [`DispatchOutcome::Skip`] carrying the typed error**, and the handler returns
//! [`HandleOutcome::Done`] (the message is ACKed — a malformed config is NOT poison to retry, and
//! NOT a crash). This is "prefer surfacing over silent": the skip is observable (the returned
//! outcome + the log), never a swallowed error, but it does NOT manufacture a spurious
//! `ci.check.updated{config_error}` — a config error is not tied to a `(commit, context)` check
//! seam (there is no armed run, so no context), so a synthetic check would fabricate a gate signal.
//! The RICHER surface (a repo-level `ci.config.rejected` notification for `myelin ci validate`) is
//! the named follow-on; this chunk surfaces the typed skip.
//!
//! ## What is DURABLE here vs the NAMED floors (be honest)
//! - **Durable, dispatch-owned (shipped LIVE):** the `ci.run.started` + queued `ci.check.updated`
//!   events, committed through the injected DURABLE [`OutboxStore`] ([`OutboxReserveStore`]) — the
//!   production `main.rs` binds `OutboxStore::durable(PgOutboxBacking)`, so these events survive a
//!   restart. The exactly-once effect rides the `Consumer` runtime's `consumer_dedup` ledger (one
//!   triggering `event_id` = one effect) PLUS a deterministic `run_id` derived from the `event_id`
//!   (so a redelivered trigger mints the SAME run — `ON CONFLICT DO NOTHING` at the `ci_run` PK).
//! - **The `ci_run` ROW one-tx co-commit — SHIPPED LIVE (CT-004d.2 chunk 4).** The durable run-of-record
//!   ROW now co-commits with the dedup mark in PRODUCTION: [`CoCommitReserveStore`] writes the `ci_run`
//!   row on the consumer's co-commit `HandlerTx` connection via
//!   [`myelin_ci_controlplane::CiRunStore::co_commit_insert`] (which downcasts
//!   `tx.connection::<sqlx::PgConnection>()` INSIDE ci-controlplane, where `sqlx` is nameable — the leaf
//!   ci-dispatch crate only threads the type-erased `tx` through). So the ROW + the mark commit together
//!   (runtime `Done`) or roll back together (`Retry`/failure) — a crash between leaves NEITHER, a
//!   redelivery re-runs and lands both exactly once (`ON CONFLICT (tenant_id, run_id) DO NOTHING`). The
//!   `ci_run` table is owned by `myelin-ci-controlplane` (its `CREATE_CI_RUN_DDL`); both CI mains create
//!   it at boot via the shared `ci_durable_migrations()` writer subset. The writer lives in
//!   ci-controlplane (NOT `myelin-storage`) because ci-dispatch already depends on it (acyclic — the
//!   controlplane is a terminal leaf), so no new dependency edge is introduced.
//! - **H1 (be brutally honest about the EVENTS — they stay ABSORB, unchanged):** the co-emitted
//!   `ci.run.started` + queued `ci.check.updated` EVENTS still commit through the DURABLE [`OutboxStore`]
//!   in its OWN outbox transaction — SEPARATE from the mark's co-commit tx (this leaf crate cannot name
//!   `sqlx` outside `--features integration` to ride the connection for the OUTBOX rows, and forcing them
//!   there was H1's REJECTED path). A crash between the outbox commit and the mark's commit makes the
//!   redelivery re-emit the SAME deterministic ids; the commit uses
//!   [`OutboxTransaction::commit_absorb`] (`ON CONFLICT (event_id) DO NOTHING` + payload verify) so the
//!   re-emit is ABSORBED, NOT rejected into a `Retry` LIVELOCK (the H1 bug). **This chunk co-commits the
//!   ROW with the mark; the EVENTS remain absorb-idempotent** — the honest split. The IDEAL all-in-one-tx
//!   co-commit (ROW + BOTH events + mark) is still proven in the integration test's `CoCommitReserveStore`
//!   (which CAN name `sqlx` in a test), as the aspirational shape; production ships the row-co-commit +
//!   events-absorb split above.
//! - **`DurableExecutor::start` (CT-004c/CT-004d — OUT OF SCOPE):** this consumer STOPS at the durable
//!   reserve bundle. It does NOT call [`myelin_flow::StartSpec`]'s executor, does NOT register /
//!   run the `ci.pipeline` body (`CI_PIPELINE_WF_TYPE`), and does NOT touch the scheduler/runner. The
//!   `wf_run_id` the `ci_run` row carries is PRE-MINTED here (deterministic from the run) so
//!   CT-004c/d starts the workflow with it; the ACTUAL start + the pipeline EXECUTION is that chunk.
//! - **Cross-service delivery (deploy floor):** the git service emits `git.ref.updated` to ITS
//!   outbox/NATS; whether this consumer receives it cross-cell over the structured
//!   `evt.<tenant>.git.*` subject is a deploy-substrate floor. The integration test proves the
//!   CONSUMER end-to-end by INJECTING a real `git.ref.updated` envelope (with a real repo + a
//!   digest-pinned `.myelin/ci.toml` at `new_oid`) — the real cross-service NATS hop is named.
//! - **The repo→(project, pipeline) registry (floor):** the `ci_run` row's `project_id`/`pipeline_id`
//!   are deterministic placeholders derived from the repo ref here; the real registry that maps a
//!   pushed repo to its CI project/pipeline is the named follow-on.

use std::sync::{Arc, Mutex, OnceLock};

use myelin_events::{
    Actor, EmitContextBase, EventEnvelope, EventHandler, EventId, HandleOutcome, IdMinter,
    OutboxStore, SubjectPattern,
};
use myelin_storage::BlobStore;
use myelin_tenancy::TenantId;

use crate::config::{parse_versioned_ci_config, CiConfigError, ConfigFormat};
use crate::dispatch::{
    compile_trigger, stamp_trust, OnTrigger, RunProvenance, TrustStamp, TRIGGER_CONSUMER,
};
use crate::resolve::{
    reserve_and_start, resolve_versioned_snapshot, snapshot_ref, CheckContext, ResolveError,
    RunFacts, StartHandoff, VersionedCiDefinition,
};

/// A canonical, readable git storage root validated before broker intake is constructed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthoritativeGitRoot(std::path::PathBuf);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRootError(pub String);

impl std::fmt::Display for GitRootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "authoritative git root is unavailable: {}", self.0)
    }
}

impl std::error::Error for GitRootError {}

impl AuthoritativeGitRoot {
    pub fn validate(path: impl AsRef<std::path::Path>) -> Result<Self, GitRootError> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(GitRootError(format!("{} is not an absolute path", path.display())));
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| GitRootError(format!("{}: {error}", path.display())))?;
        if !canonical.is_dir() {
            return Err(GitRootError(format!("{} is not a directory", canonical.display())));
        }
        std::fs::read_dir(&canonical)
            .map_err(|error| GitRootError(format!("{} is not readable: {error}", canonical.display())))?;
        Ok(Self(canonical))
    }

    pub fn as_path(&self) -> &std::path::Path {
        &self.0
    }
}

// =================================================================================================
// 1. The consumer subject whitelist (rule 3: a WHITELIST, never `*`).
// =================================================================================================

/// The `ci-dispatch.trigger` consumer's subject whitelist (contract 2.4 / BUS-3: a whitelist, NEVER
/// `*`). The in-process `serve` harness routes on the envelope's `subject` `ArtifactRef`
/// (`myelin://<tenant>/git/ref/<repo>:<ref>` for a push); a single `&'static` prefix cannot pin the
/// tenant (it is the first path segment), so this coarse prefix is the TRANSPORT PREFILTER and the
/// PRECISE arming is done in [`CiTriggerHandler::handle`] (the exact `event.type` match via
/// [`compile_trigger`] — a non-git event is an O(1) no-op skip, never processed). It is deliberately
/// NOT `*`/`>`/empty, so [`myelin_events::consumer::Subscription::bind`] accepts it and the
/// head-of-line-block guard holds.
///
/// **Named deploy floor:** the production per-`(tenant, subsystem)` NATS routing key is the
/// structured subject `evt.<tenant>.git.*` (`myelin_events::partition`); the cross-cell stream that
/// filters git events for this consumer is the deploy-substrate follow-on. The prefilter here is the
/// in-process/ArtifactRef form the `dispatch_app_spec` harness uses today.
pub fn ci_trigger_subjects() -> &'static [SubjectPattern] {
    // Peer-review finding 2026-07-16 #12: this was `SubjectPattern(String::new())` — an EMPTY prefix,
    // and `Subscription::matches` is `subject.starts_with(&p.0)`, so an empty pattern MATCHES EVERY
    // subject (match-all) — directly contradicting the doc above ("deliberately NOT */>/empty"). Any
    // router iterating `EventHandler::subjects()` would treat it as a firehose subscription. Fixed to
    // the SAME bounded `myelin://` prefix the live `Subscription` binds (`CI_TRIGGER_SUBJECT_STRS`),
    // built via `OnceLock` (a non-empty `SubjectPattern` holds a `String`, not const-constructible in a
    // plain `static` — the `myelin_git::check_status` precedent). The precise arming stays `handle`'s
    // O(1) type match; this only bounds the transport-level whitelist so it is never over-broad.
    static SUBJECTS: OnceLock<Vec<SubjectPattern>> = OnceLock::new();
    SUBJECTS
        .get_or_init(|| {
            CI_TRIGGER_SUBJECT_STRS
                .iter()
                .map(|s| SubjectPattern((*s).to_string()))
                .collect()
        })
        .as_slice()
}

/// The `&str` subjects the live [`myelin_events::consumer::Subscription`] binds — the SOURCE OF TRUTH
/// [`ci_trigger_subjects`] maps into `SubjectPattern`s, and the borrow the `Subscription::bind`
/// constructor takes. `myelin://` is the bounded (non-`*`) transport prefix; the arming is `handle`'s type match.
pub const CI_TRIGGER_SUBJECT_STRS: &[&str] = &["myelin://"];

// =================================================================================================
// 2. The git config-read seam (read `.myelin/ci.*` at the pushed ref).
// =================================================================================================

/// Why reading `.myelin/ci.*` at the pushed ref failed (a TRANSPORT/backend failure — distinct from
/// "the file is simply absent", which is `Ok(None)`, a clean skip). A read error is fail-closed:
/// the consumer does NOT start a run it cannot prove the definition of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitReadError {
    /// Backend, repository, or exact commit is temporarily unavailable. Redeliver.
    Unavailable(String),
    /// The request/config object is permanently invalid (for example over the byte limit). DLQ.
    Invalid(String),
}

impl GitReadError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, GitReadError::Unavailable(_))
    }
}

impl std::fmt::Display for GitReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitReadError::Unavailable(message) => write!(f, "git config backing unavailable: {message}"),
            GitReadError::Invalid(message) => write!(f, "invalid git config read: {message}"),
        }
    }
}

impl std::error::Error for GitReadError {}

/// **The seam that reads ONE file from a repo at a ref (the myelin-git read backend, abstracted).**
/// The consumer calls it through [`resolve_ci_config`] (which tries `.myelin/ci.toml` then
/// `.json`). `Ok(None)` == the path is absent at that ref (a clean skip, NOT an error); `Ok(Some)` ==
/// the raw file bytes. The production adapter ([`DurableGitConfigReader`]) wraps
/// `myelin_git::durable`'s `read_blob_at_path`; a unit test uses [`MapGitConfigReader`].
pub trait GitConfigReader: Send + Sync {
    /// Read `path` at `oid` in `(tenant, region, repo)`. `Ok(None)` iff the path is absent at that
    /// ref (a clean "no file" skip); `Err` iff the backend read itself failed (fail-closed).
    fn read_repo_file(
        &self,
        tenant: &str,
        region: &str,
        repo: &str,
        oid: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, GitReadError>;
    fn read_repo_file_bounded(
        &self, tenant: &str, region: &str, repo: &str, oid: &str, path: &str, maximum_bytes: usize,
    ) -> Result<Option<Vec<u8>>, GitReadError> {
        let bytes = self.read_repo_file(tenant, region, repo, oid, path)?;
        if bytes.as_ref().is_some_and(|bytes| bytes.len() > maximum_bytes) {
            return Err(GitReadError::Invalid(format!("{path}@{oid} exceeds the {maximum_bytes}-byte config limit")));
        }
        Ok(bytes)
    }
}

/// The `.myelin/ci.*` paths tried, in priority order: TOML (the primary authored surface, arch 02
/// §7.4) then JSON (the JSON-Schema'd core). YAML is deferred (no workspace YAML dep — see
/// [`ConfigFormat`]); a repo authoring `.myelin/ci.yaml` therefore reads as "no config" here (a clean
/// skip), the SAME named-defer the parser records.
const CI_CONFIG_CANDIDATES: &[(&str, ConfigFormat)] = &[
    (".myelin/ci.toml", ConfigFormat::Toml),
    (".myelin/ci.json", ConfigFormat::Json),
];

/// **Resolve the pushed repo's `.myelin/ci.*` at `oid`** — try `.myelin/ci.toml`, then
/// `.myelin/ci.json`. `Ok(None)` iff NEITHER exists (a clean "no pipeline armed" skip, NOT an error);
/// `Ok(Some((bytes, format)))` for the first present candidate; `Err` iff a backend read failed.
pub fn resolve_ci_config(
    reader: &dyn GitConfigReader,
    tenant: &str,
    region: &str,
    repo: &str,
    oid: &str,
) -> Result<Option<(Vec<u8>, ConfigFormat)>, GitReadError> {
    for (path, format) in CI_CONFIG_CANDIDATES {
        if let Some(bytes) = reader.read_repo_file_bounded(tenant, region, repo, oid, path, crate::config::MAX_CI_CONFIG_BYTES)? {
            return Ok(Some((bytes, *format)));
        }
    }
    Ok(None)
}

// =================================================================================================
// 3. The reserve-persistence seam (persist the atomic bundle DURABLY).
// =================================================================================================

/// The extra `ci_run`-row facts the reserve bundle needs beyond the [`crate::resolve::CiRunWrite`]
/// (the columns the `CREATE_CI_RUN_DDL` requires but the pure resolve core does not model): the
/// residency region, the `(project, pipeline)` ids, the pre-minted `wf_run_id`, and the correlation.
///
/// `project_id`/`pipeline_id` are DETERMINISTIC PLACEHOLDERS from the repo ref (the real
/// repo→pipeline registry is the named floor). `wf_run_id` is PRE-MINTED here (deterministic from the
/// run) — CT-004c/d starts the `ci.pipeline` workflow WITH it (the `DurableExecutor::start` is out of
/// scope). `correlation_id` is the triggering envelope's (real provenance).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReserveFacts {
    /// The residency region (`ci_run.region`) — from the triggering envelope.
    pub region: String,
    /// The CI project id (`ci_run.project_id`, uuid) — a deterministic placeholder from the repo ref
    /// (the repo→project registry is the named floor).
    pub project_id: String,
    /// The CI pipeline id (`ci_run.pipeline_id`, uuid) — a deterministic placeholder (named floor).
    pub pipeline_id: String,
    /// The workflow run id (`ci_run.wf_run_id`, uuid) — PRE-MINTED here; CT-004c/d starts the
    /// workflow with it (`DurableExecutor::start` is out of scope).
    pub wf_run_id: String,
    /// The correlation id (`ci_run.correlation_id`) — the triggering envelope's correlation.
    pub correlation_id: String,
    pub repo_ref: String,
    pub commit_oid: String,
}

/// A fully-armed run ready to persist: the atomic [`StartHandoff`] bundle + the extra `ci_run` facts
/// + the tenant/actor the durable write is scoped to. This is the value a [`ReserveStore`] commits.
#[derive(Clone, Debug)]
pub struct ArmedRun {
    /// The atomic reserve/start bundle (the `ci_run` row + `ci.run.started` + the queued checks).
    pub handoff: StartHandoff,
    /// The extra `ci_run`-row facts (region / ids / correlation).
    pub reserve: ReserveFacts,
    /// The tenant the durable write is partitioned under.
    pub tenant: TenantId,
    /// The acting principal (the pushing pseudonym) — the `ci_run.triggered_by` provenance.
    pub actor: Actor,
    /// The triggering envelope's ambient emit context (tenant/region/actor/clock) the co-committed
    /// events derive their causality + partition from.
    pub emit_ctx: EmitContextBase,
}

/// Why persisting the reserve bundle failed (fail-closed — surfaced, never swallowed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReserveError(pub String);

impl std::fmt::Display for ReserveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "reserve/start persistence failed: {}", self.0)
    }
}

impl std::error::Error for ReserveError {}

/// **The seam that DURABLY persists the atomic reserve/start bundle.** Two shapes, both idempotent:
///
/// - **True co-commit (the target / proven path):** the reserve bundle rides the consumer-runtime
///   co-commit connection ([`HandlerTx::connection`] → the SAME `sqlx` transaction the dedup mark is
///   in), so the `ci_run` ROW + `ci.run.started` + queued `ci.check.updated` + the dedup mark ALL
///   commit or ALL roll back in ONE tx. A crash rolls back EVERYTHING; a redelivery re-runs cleanly.
///   The integration test's `PgReserveStore` implements this (it can name `sqlx`). This is the honest
///   #7 shape and the one the H1 probe proves.
/// - **Separate-tx outbox + ABSORB (the production floor):** [`OutboxReserveStore`] runs in the
///   production leaf crate, which cannot name `sqlx` outside `--features integration`, so it CANNOT
///   ride the type-erased co-commit connection. It commits the events through its own DURABLE
///   [`OutboxStore`] in a SEPARATE tx, using [`OutboxTransaction::commit_absorb`] so a crash-window
///   redelivery (mark not yet committed) re-emits the SAME deterministic ids and is ABSORBED — no
///   `Err`-into-`Retry` LIVELOCK (the H1 bug). The `ci_run` ROW is the named `myelin-storage`-backing
///   floor here (not written by this path yet). Wiring the true co-commit into production (a
///   `myelin-storage` `ci_run` writer riding the co-commit connection) is the named follow-on.
///
/// A unit test uses [`RecordingReserveStore`]. The trait is SYNC to match the [`EventHandler::handle`]
/// body (the durable impls bridge to async at their own boundary, the `PgOutboxBacking` idiom).
pub trait ReserveStore: Send + Sync {
    /// Persist the armed run's atomic bundle. Idempotent on the run identity (a redelivered trigger
    /// mints the SAME `run_id` + the SAME deterministic event ids). `tx` is the consumer-runtime
    /// co-commit handle (#7/MR-023b): an impl that can name `sqlx` (the integration `PgReserveStore`)
    /// downcasts `tx.connection::<sqlx::PgConnection>()` and writes the bundle on the SAME tx as the
    /// dedup mark (true co-commit); the production [`OutboxReserveStore`] ignores `tx` and rides its
    /// own DURABLE outbox with [`OutboxTransaction::commit_absorb`] (absorb-mode idempotency).
    fn persist(
        &self,
        armed: &ArmedRun,
        tx: &mut myelin_events::HandlerTx<'_>,
    ) -> Result<(), ReserveError>;
}

/// **The production reserve store: co-commit the reserve bundle's EVENTS through the DURABLE
/// [`OutboxStore`] (dispatch-owned, survives restart).** Emits `ci.run.started` then the queued
/// `ci.check.updated` per context via the sanctioned [`OutboxTx::emit`] path (co-committed in ONE
/// outbox transaction — `emit`-iff-`commit`); `main.rs` binds `OutboxStore::durable(PgOutboxBacking)`
/// so the rows are durable. The `ci_run` ROW one-tx co-commit is the named `myelin-storage`-backing
/// floor (this leaf crate has no `sqlx` outside `--features integration`); it is proven against live
/// PG in the integration test's `PgReserveStore`.
pub struct OutboxReserveStore {
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
}

impl OutboxReserveStore {
    /// Build the store over the service's DURABLE outbox (the `main.rs`
    /// `OutboxStore::durable(PgOutboxBacking)`) + the shared ULID minter.
    pub fn new(outbox: OutboxStore, minter: Arc<dyn IdMinter>) -> OutboxReserveStore {
        OutboxReserveStore { outbox, minter }
    }
}

impl ReserveStore for OutboxReserveStore {
    fn persist(
        &self,
        armed: &ArmedRun,
        _tx: &mut myelin_events::HandlerTx<'_>,
    ) -> Result<(), ReserveError> {
        // **The production separate-tx floor (events-only).** This leaf crate cannot name `sqlx`
        // outside `--features integration`, so it CANNOT ride the type-erased co-commit connection
        // (`_tx`) — it commits the events through its OWN durable outbox in a separate tx. This store
        // does NOT write the ci_run ROW (it only stages a NOTE); the store that co-commits the durable
        // run-of-record ROW on `_tx` (atomic with the dedup mark) is [`CoCommitReserveStore`] below
        // (CT-004d.2 chunk 4). Kept as the events-only absorb path (its livelock proof is
        // `h1_production_outbox_absorb_closes_the_livelock`).
        let mut tx = self
            .outbox
            .begin(Arc::clone(&self.minter), armed.emit_ctx.clone());
        tx.stage_state_change(format!(
            "ci_run {} reserved (queued) — the durable ROW co-commit is CoCommitReserveStore (chunk 4)",
            armed.handoff.run_write.run_id
        ));
        emit_reserve_events(&mut tx, armed)?;
        // **H1 — ABSORB-mode commit (not the reject-arm `commit`).** A crash-window redelivery (the
        // dedup mark not yet committed) re-runs this whole method and re-emits the SAME deterministic
        // ids; `commit_absorb` `ON CONFLICT (event_id) DO NOTHING`s the byte-identical re-emit instead
        // of `Err`ing → `Retry` → the UNBOUNDED LIVELOCK the reject-arm `commit` caused (H1). The events
        // stay present exactly once; a divergent-payload collision still rejects.
        tx.commit_absorb()
            .map_err(|e| ReserveError(format!("outbox commit_absorb: {e:?}")))
    }
}

/// **Emit the reserve bundle's two co-emitted EVENTS (`ci.run.started` + the queued `ci.check.updated`
/// per context) into an open outbox tx, with DETERMINISTIC ids (the absorb-mode idempotency anchor).**
/// The shared emit both [`OutboxReserveStore`] and [`CoCommitReserveStore`] use so the id derivation is
/// authored ONCE (no drift). The caller `commit_absorb`s the tx.
///
/// **Peer-review #8: DETERMINISTIC co-emitted event ids.** The `ci.run.started` + each queued
/// `ci.check.updated` id is derived from the (deterministic) `run_id` + the event's stable subject, so
/// a REDELIVERED trigger re-emits the SAME ids and `commit_absorb` `ON CONFLICT DO NOTHING`s them.
///
/// **H3 (peer-review #7) — the check id includes the run_id.** The check subject is
/// `repo#commit-<oid>/check-<context>` with NO run_id; two DISTINCT triggers on the same
/// (repo, commit, context) mint DISTINCT runs but would have minted the SAME check id with DIFFERENT
/// payloads (a collision). Seeding with the run_id (`evt:<run_id>:<subject>`) makes a redelivery of the
/// SAME run still dedup while distinct runs diverge. `run.started` already carries the run_id in its
/// subject (`ci/run/<run_id>`), so it is per-run unique already.
fn emit_reserve_events(
    tx: &mut myelin_events::OutboxTransaction,
    armed: &ArmedRun,
) -> Result<(), ReserveError> {
    let started_id = EventId(deterministic_uuid(&format!(
        "evt:{}",
        armed.handoff.run_started.subject.0
    )));
    tx.emit_with_id(started_id, armed.handoff.run_started.clone(), None)
        .map_err(|e| ReserveError(format!("ci.run.started emit: {e:?}")))?;
    for check in &armed.handoff.queued_checks {
        let check_id = check_event_id(&armed.handoff.run_write.run_id, &check.subject.0);
        tx.emit_with_id(check_id, check.clone(), None)
            .map_err(|e| ReserveError(format!("queued ci.check.updated emit: {e:?}")))?;
    }
    Ok(())
}

/// **Map an [`ArmedRun`] to the durable [`CiRunInsert`] the [`CiRunStore`] writes.** The mapping lives
/// HERE (ci-controlplane cannot name `ArmedRun` — that edge would be a cycle). Every NOT-NULL `ci_run`
/// column is set from the atomic bundle; `state = "queued"` (the reserve state). Repository,
/// commit, and triggering pseudonym provenance come from the authoritative trigger envelope, never
/// authored config. `cause_event_id` carries the triggering event identity.
pub fn ci_run_insert_from_armed(armed: &ArmedRun) -> myelin_ci_controlplane::CiRunInsert {
    let rw = &armed.handoff.run_write;
    myelin_ci_controlplane::CiRunInsert {
        tenant_id: armed.tenant.0.clone(),
        region: armed.reserve.region.clone(),
        run_id: rw.run_id.clone(),
        project_id: armed.reserve.project_id.clone(),
        pipeline_id: armed.reserve.pipeline_id.clone(),
        wf_run_id: armed.reserve.wf_run_id.clone(),
        definition_snapshot: rw.definition_snapshot.0.clone(),
        trigger_kind: rw.trigger_kind.clone(),
        trust_tier: rw.trust_tier.clone(),
        state: rw.state.clone(),
        correlation_id: armed.reserve.correlation_id.clone(),
        cause_event_id: Some(rw.cause_event_id.clone()),
        repo_ref: Some(armed.reserve.repo_ref.clone()),
        commit_oid: Some(armed.reserve.commit_oid.clone()),
        triggered_by: Some(armed.actor.0.principal_id.0.clone()),
    }
}

/// **The PRODUCTION reserve store that co-commits the durable `ci_run` ROW with the dedup mark
/// (CT-004d.2 chunk 4 — the run-of-record writer the CT-004b integration test proved).** On `persist`:
///
/// 1. **CO-COMMIT the `ci_run` ROW on the consumer's co-commit `HandlerTx` connection** via
///    [`CiRunStore::co_commit_insert`] (which downcasts `tx.connection::<sqlx::PgConnection>()` inside
///    ci-controlplane, where `sqlx` is nameable). The row rides the SAME `sqlx` transaction the dedup
///    mark is in, so the run-of-record + the mark commit together (runtime `Done`) or roll back together
///    (`Retry`/failure) — the load-bearing atomicity invariant. `ON CONFLICT (tenant_id, run_id) DO
///    NOTHING` makes a redelivery land the row exactly once. A missing co-commit connection is
///    fail-closed (`Retry`), never a write outside the mark's tx.
/// 2. **EMIT the co-emitted EVENTS through the DURABLE outbox in ABSORB mode (the honest #7 H1 split).**
///    The `ci.run.started` + queued `ci.check.updated` events go through the outbox (which owns its OWN
///    pool — this leaf crate cannot name `sqlx` there), `commit_absorb`ed so the deterministic re-emit
///    of a crash-window redelivery is ABSORBED, not a livelock. The ROW co-commits; the EVENTS are
///    absorb-idempotent. Forcing the events onto the external connection was H1's REJECTED path.
///
/// **The crash consistency (why the split is safe):** the events are absorb-idempotent on their
/// deterministic ids and the row is `ON CONFLICT`-idempotent on its deterministic `run_id`, so ANY
/// interleaving of the two commits under a crash converges to exactly ONE run + its events on
/// redelivery. The row + mark atomicity means a run-of-record is NEVER durably present without its
/// dedup mark (or vice-versa); the events being present without the row (order-2-before-1 crash) is
/// self-healing (the redelivery writes the row + re-absorbs the events).
///
/// **The ONE non-auto-self-healing state (adversarial-verify LOW, 2026-07-17):** if the handler PANICS
/// AFTER the events `commit_absorb` but before the runtime commits the row+mark, the #7 H2 panic path
/// rolls back the co-commit tx (no row, no mark) and dead-letters TERMINALLY (acked) — so the queued
/// `ci.check.updated` events stay durable with NO `ci_run` row until the durable-DLQ (#7b) poison is
/// replayed. Direction is SAFE (the merge gate BLOCKS on a queued/absent required context, never
/// admits — a run-of-record can't be silently missing in a way that green-lights a merge); it simply
/// does not converge without operator replay. Named, not silently skipped.
///
/// Out of scope (named): `DurableExecutor::start` (chunk 3), the `ci.pipeline` body (chunk 2), the
/// scheduler/runner (chunk 5). This store writes the reserve run-of-record + emits the reserve events.
pub struct CoCommitReserveStore {
    ci_run: myelin_ci_controlplane::CiRunStore,
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
    rt: tokio::runtime::Handle,
}

impl CoCommitReserveStore {
    /// Build the production reserve store from the durable `ci_run` writer (over the CI OLTP pool), the
    /// service's DURABLE outbox (the `main.rs` `OutboxStore::durable(PgOutboxBacking)`), the shared ULID
    /// minter, and the serve runtime handle (bridges the async co-commit `sqlx` write to the sync
    /// `persist` body — the `PgOutboxBacking` idiom).
    pub fn new(
        ci_run: myelin_ci_controlplane::CiRunStore,
        outbox: OutboxStore,
        minter: Arc<dyn IdMinter>,
        rt: tokio::runtime::Handle,
    ) -> CoCommitReserveStore {
        CoCommitReserveStore {
            ci_run,
            outbox,
            minter,
            rt,
        }
    }
}

impl ReserveStore for CoCommitReserveStore {
    fn persist(
        &self,
        armed: &ArmedRun,
        tx: &mut myelin_events::HandlerTx<'_>,
    ) -> Result<(), ReserveError> {
        // (1) CO-COMMIT the run-of-record ROW on the dedup mark's tx (fail-closed on no co-commit conn).
        let row = ci_run_insert_from_armed(armed);
        self.ci_run
            .co_commit_insert(tx, &row, &self.rt)
            .map_err(|e| ReserveError(format!("ci_run co-commit: {e}")))?;
        // (2) EMIT the events through the DURABLE outbox in ABSORB mode (the honest #7 H1 split — the
        // events stay absorb-idempotent; the ROW above co-committed with the mark).
        let mut otx = self
            .outbox
            .begin(Arc::clone(&self.minter), armed.emit_ctx.clone());
        emit_reserve_events(&mut otx, armed)?;
        otx.commit_absorb()
            .map_err(|e| ReserveError(format!("outbox commit_absorb: {e:?}")))
    }
}

// =================================================================================================
// 4. The dispatch plan (the pure pipeline core — testable per branch).
// =================================================================================================

/// Why a triggering event did NOT arm a run — every skip is a distinct, SURFACED reason (fail-closed,
/// never a silent swallow). Clean/structural skips ACK; unavailable backing retries; permanently
/// invalid Git reads dead-letter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The event is not a CI trigger type (a non-git / non-armed event routed by the coarse prefix).
    NotATrigger(String),
    /// The event payload lacked a required field (`repo` / `new_oid`) — malformed, fail-closed.
    MalformedPayload(String),
    /// Envelope provenance is contradictory or invalid. This is permanent poison, never an
    /// authored-config skip: subject/aggregate/payload disagreement reaches the durable DLQ.
    InvalidProvenance(String),
    /// A backend read of `.myelin/ci.*` failed (fail-closed — no run without a proven definition).
    ReadFailed(GitReadError),
    /// NO `.myelin/ci.*` at the pushed ref — a clean "no pipeline armed" skip (NOT an error).
    NoConfig,
    /// The `.myelin/ci.*` was present but malformed — SURFACED, fail-closed (no run, no crash).
    ConfigError(CiConfigError),
    /// The config parsed, but its armed trigger does NOT match THIS event (e.g. a `pull_request`
    /// config on a push).
    TriggerNotMatched,
    /// The definition failed to resolve (a floating tag / cycle / dangling need) — SURFACED,
    /// fail-closed (the supply-chain control; no un-digested reference reaches a run).
    ResolveError(ResolveError),
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkipReason::NotATrigger(t) => write!(f, "not a CI trigger event: `{t}`"),
            SkipReason::MalformedPayload(m) => write!(f, "malformed trigger payload: {m}"),
            SkipReason::InvalidProvenance(m) => write!(f, "invalid trigger provenance: {m}"),
            SkipReason::ReadFailed(e) => write!(f, "{e}"),
            SkipReason::NoConfig => write!(f, "no `.myelin/ci.*` at the pushed ref — no pipeline armed"),
            SkipReason::ConfigError(e) => write!(f, "malformed `.myelin/ci.*` (fail-closed): {e}"),
            SkipReason::TriggerNotMatched => {
                write!(f, "the armed trigger does not match this event — no run")
            }
            SkipReason::ResolveError(e) => write!(f, "the definition failed to resolve (fail-closed): {e}"),
        }
    }
}

/// The outcome of planning a dispatch for one triggering event: either an ARMED run (the atomic
/// bundle ready to persist) or a SURFACED skip (with the typed reason). This is the pure pipeline —
/// [`CiTriggerHandler::handle`] persists an `Arm` through the [`ReserveStore`] and logs a `Skip`.
#[derive(Debug)]
pub enum DispatchOutcome {
    /// The event armed a run — the atomic bundle to persist.
    Arm(Box<ArmedRun>),
    /// The event did not arm a run — the surfaced reason.
    Skip(SkipReason),
}

/// Canonical repository, immutable commit, and fork provenance from a triggering envelope.
struct TriggerFacts {
    repo: String,
    new_oid: String,
    is_fork: bool,
}

/// Parse the triggering-event provenance from the envelope payload (arch 02 §1). `git.ref.updated`
/// carries `{repo, ref, new_oid, ...}`; the PR events carry the head oid + a fork flag. Returns the
/// facts, or the malformed-payload reason (fail-closed).
fn trigger_facts(ev: &EventEnvelope) -> Result<TriggerFacts, SkipReason> {
    let p = &ev.payload;
    // `repo` is the repository ref the push landed on (the ci_run repo_ref + the git read key).
    let repo = p
        .get("repo")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| SkipReason::MalformedPayload("missing `repo`".into()))?
        .to_string();
    myelin_git::gix_backend::validate_repo_slug(&repo).map_err(|error| {
        SkipReason::InvalidProvenance(format!("invalid payload repository {repo:?}: {error}"))
    })?;

    // Subject, aggregate, and payload are mutually distrustful provenance inputs. All three must
    // identify the same push/PR before any Git or CAS read is attempted.
    let oid_field = if ev.type_.0 == myelin_git::events::GIT_REF_UPDATED {
        let ref_name = p.get("ref").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
            .ok_or_else(|| SkipReason::MalformedPayload("missing `ref`".into()))?;
        validate_git_ref_name(ref_name)?;
        validate_envelope_provenance(
            ev,
            &format!("myelin://{}/git/ref/{repo}:{ref_name}", ev.tenant.0),
            &format!("{repo}:{ref_name}"),
        )?;
        "new_oid"
    } else {
        let number = p.get("number").and_then(|v| v.as_u64()).filter(|number| *number > 0)
            .ok_or_else(|| SkipReason::InvalidProvenance("PR `number` must be a positive integer".into()))?;
        validate_envelope_provenance(
            ev,
            &format!("myelin://{}/git/pr/{repo}:{number}", ev.tenant.0),
            &format!("git/pr/{repo}:{number}"),
        )?;
        "head_oid"
    };

    // Only an immutable full SHA-1 oid is admitted. Refs, HEAD, revspecs, and abbreviations are
    // permanently invalid and are refused before crossing the Git reader seam.
    let raw_oid = p
        .get(oid_field)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| SkipReason::MalformedPayload(format!("missing `{oid_field}`")))?;
    let new_oid = canonical_commit_oid(raw_oid)?;
    // Fork provenance is trust input, not a best-effort hint. PR producers MUST provide an
    // explicit boolean. Push producers predate the field, so absence remains a member push; any
    // alias they do provide is nevertheless validated and preserved. If canonical + legacy aliases
    // coexist they must agree, otherwise field ordering could choose which trust result wins.
    let requires_fork_evidence = ev.type_.0 == myelin_git::events::GIT_PR_OPENED
        || ev.type_.0 == myelin_git::events::GIT_PR_SYNCHRONIZED;
    let is_fork = parse_fork_evidence(p, requires_fork_evidence)?;
    Ok(TriggerFacts {
        repo,
        new_oid,
        is_fork,
    })
}

fn validate_envelope_provenance(
    ev: &EventEnvelope,
    expected_subject: &str,
    expected_aggregate: &str,
) -> Result<(), SkipReason> {
    if ev.subject.0 != expected_subject {
        return Err(SkipReason::InvalidProvenance(format!(
            "subject/payload provenance mismatch: expected {expected_subject:?}, got {:?}", ev.subject.0
        )));
    }
    if ev.aggregate.0 != expected_aggregate {
        return Err(SkipReason::InvalidProvenance(format!(
            "aggregate/payload provenance mismatch: expected {expected_aggregate:?}, got {:?}", ev.aggregate.0
        )));
    }
    Ok(())
}

/// Validate the canonical `check-ref-format` rules used by a fully-qualified provider ref.
fn validate_git_ref_name(ref_name: &str) -> Result<(), SkipReason> {
    let invalid = !ref_name.starts_with("refs/")
        || ref_name.ends_with('/')
        || ref_name.ends_with('.')
        || ref_name.contains("//")
        || ref_name.contains("..")
        || ref_name.contains("@{")
        || ref_name.contains([':', '\\'])
        || ref_name.split('/').any(|part| {
            part.is_empty() || part.starts_with('.') || part.ends_with(".lock")
        })
        || ref_name.chars().any(|c| c.is_ascii_control() || c.is_ascii_whitespace() || matches!(c, '~' | '^' | '?' | '*' | '['));
    if invalid {
        Err(SkipReason::InvalidProvenance(format!("invalid canonical Git ref {ref_name:?}")))
    } else {
        Ok(())
    }
}

fn canonical_commit_oid(raw: &str) -> Result<String, SkipReason> {
    if raw.len() != 40 || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SkipReason::ReadFailed(GitReadError::Invalid(
            "commit oid must be exactly 40 hexadecimal characters; revspecs and abbreviated ids are refused".into(),
        )));
    }
    Ok(raw.to_ascii_lowercase())
}

/// Parse and reconcile canonical/legacy fork provenance without ever defaulting malformed trust
/// input to a trusted member run. PR events require evidence; push events preserve legacy absence.
fn parse_fork_evidence(payload: &serde_json::Value, required: bool) -> Result<bool, SkipReason> {
    let canonical = payload.get("is_fork");
    let legacy = payload.get("forked");
    let parse = |name: &str, value: &serde_json::Value| {
        value
            .as_bool()
            .ok_or_else(|| SkipReason::MalformedPayload(format!("`{name}` must be a boolean")))
    };

    match (canonical, legacy) {
        (None, None) if required => Err(SkipReason::MalformedPayload(
            "missing explicit boolean fork evidence (`is_fork` or legacy `forked`)".into(),
        )),
        (None, None) => Ok(false),
        (Some(value), None) => parse("is_fork", value),
        (None, Some(value)) => parse("forked", value),
        (Some(canonical), Some(legacy)) => {
            let canonical = parse("is_fork", canonical)?;
            let legacy = parse("forked", legacy)?;
            if canonical != legacy {
                return Err(SkipReason::MalformedPayload(
                    "conflicting fork evidence: `is_fork` and `forked` must agree".into(),
                ));
            }
            Ok(canonical)
        }
    }
}

/// Which [`OnTrigger`] an event TYPE could arm (the reverse of [`OnTrigger::event_types`]). `None`
/// for a non-CI-trigger type (a clean skip). Only `push` / `pull_request` are wired in this chunk;
/// the other triggers (issue/manual/schedule/agent) are recognised but arm through their own
/// producers (named — this consumer subscribes to the git push/PR seam).
fn on_trigger_for_type(event_type: &str) -> Option<OnTrigger> {
    if event_type == myelin_git::events::GIT_REF_UPDATED {
        Some(OnTrigger::Push)
    } else if event_type == myelin_git::events::GIT_PR_OPENED
        || event_type == myelin_git::events::GIT_PR_SYNCHRONIZED
    {
        Some(OnTrigger::PullRequest)
    } else {
        None
    }
}

/// **The DETERMINISTIC `ci.check.updated` event id for a (run, check-subject) pair (H3 — peer-review
/// #7 re-prosecution).** Seeded with the `run_id` so a REDELIVERY of the SAME run (same deterministic
/// run_id) re-mints the SAME id (dedup/absorb), while two DISTINCT runs on the same
/// `(repo, commit, context)` — a re-opened PR on the same head, the same commit on a second ref, two
/// PRs sharing a head — diverge (distinct ids, no collision). The check SUBJECT alone omits the run_id
/// (`repo#commit-<oid>/check-<context>`), so seeding on the subject ONLY (the pre-fix bug) made those
/// distinct runs collide on the same id with DIFFERENT payloads.
///
/// LOW aside (named, not fixed): [`deterministic_uuid`] is a 2×-salted FNV-64 (non-cryptographic) over
/// possibly attacker-authored job/context names; it guards a DEDUP boundary (a collision would merge
/// two runs' checks), not an auth boundary. FNV is fine for the dedup role; if this ever gates trust,
/// swap to a keyed/crypto hash. Documented here, not silently relied upon.
pub fn check_event_id(run_id: &str, check_subject: &str) -> EventId {
    EventId(deterministic_uuid(&format!("evt:{run_id}:{check_subject}")))
}

/// **A deterministic uuid-shaped string from a seed (FNV-1a fill).** The `ci_run` `run_id` /
/// `wf_run_id` / `project_id` / `pipeline_id` columns are `uuid`; deriving them deterministically
/// from the triggering `event_id` (run/wf) or the repo ref (project/pipeline) makes a REDELIVERED
/// trigger mint the SAME ids — the `ON CONFLICT (tenant_id, run_id) DO NOTHING` idempotency guard
/// (exactly-once run under at-least-once delivery), and reproducible test assertions.
pub fn deterministic_uuid(seed: &str) -> String {
    let fill = |salt: u64| -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ salt;
        for b in seed.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    };
    let a = fill(0);
    let b = fill(0x00ff_00ff_00ff_00ff);
    // Render the 16 bytes as a canonical uuid string (8-4-4-4-12).
    let bytes = [a.to_be_bytes(), b.to_be_bytes()].concat();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8],
        bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

/// **The pure dispatch pipeline (arch 02 §1): match → read → parse → trigger-match → trust-stamp →
/// resolve → reserve.** Given a triggering envelope, the git reader, and the tenant's blob store,
/// returns the ARMED bundle or a SURFACED skip. This is the testable core [`CiTriggerHandler::handle`]
/// drives; every branch is a distinct [`SkipReason`] / an `Arm`.
pub fn plan_dispatch(
    ev: &EventEnvelope,
    reader: &dyn GitConfigReader,
    blobs: &dyn BlobStore,
) -> DispatchOutcome {
    // 1. Subject-match: is this a CI trigger type at all?
    let Some(on) = on_trigger_for_type(&ev.type_.0) else {
        return DispatchOutcome::Skip(SkipReason::NotATrigger(ev.type_.0.clone()));
    };

    // 1b. Parse the triggering provenance (repo, head oid, fork flag).
    let facts = match trigger_facts(ev) {
        Ok(f) => f,
        Err(reason) => return DispatchOutcome::Skip(reason),
    };

    let tenant = ev.tenant.clone();
    let region = ev.region.0.clone();

    // 2. Read `.myelin/ci.toml` (then `.json`) at the pushed ref.
    let config = match resolve_ci_config(reader, &tenant.0, &region, &facts.repo, &facts.new_oid) {
        Ok(Some(c)) => c,
        Ok(None) => return DispatchOutcome::Skip(SkipReason::NoConfig),
        Err(e) => return DispatchOutcome::Skip(SkipReason::ReadFailed(e)),
    };
    let (bytes, format) = config;

    // 3. Parse — a CiConfigError is a fail-closed, SURFACED skip (no crash, no run).
    let format_hint = match format {
        ConfigFormat::Toml => ".myelin/ci.toml",
        ConfigFormat::Json => ".myelin/ci.json",
    };
    let def: VersionedCiDefinition = match parse_versioned_ci_config(&bytes, format_hint) {
        Ok(d) => d,
        Err(e) => {
            return DispatchOutcome::Skip(SkipReason::ConfigError(e.into_legacy_surface()))
        }
    };

    // 4. Compile the armed trigger and ask: does THIS event match? The config's `on:` compiles to the
    //    ONE `QueryAst` (CI-P10, contract 3.4 — NOT a CI DSL / CEL); a compile error is a fail-closed
    //    skip. The MATCH DECISION is the TYPE-FAMILY equality: the arrived event type maps (via
    //    [`on_trigger_for_type`]) back to `on`, and the config's armed trigger must be the SAME family
    //    — so a repo whose config is `on = "pull_request"` does NOT arm on a push, and vice-versa.
    //
    //    (`EventMatcher::matches` is the RUN-object visibility gate keyed on a `run` subject — the
    //    authz reverse-index over WHICH runs a viewer may arm; it is NOT the "does this push event's
    //    type match" question, whose subject is a git ref, not a run. The two-halves seam: the type
    //    predicate here, the run-object visibility at the authz layer — named.)
    if let Err(e) = compile_trigger(&def.on) {
        return DispatchOutcome::Skip(SkipReason::MalformedPayload(format!("trigger compile: {e}")));
    }
    if def.on != on || !def.on.event_types().contains(&ev.type_.0.as_str()) {
        return DispatchOutcome::Skip(SkipReason::TriggerNotMatched);
    }

    // 5. Trust-stamp (X-1) — the single evaluation stamped onto BOTH the run + every check.
    //    A member push is Trusted; a fork PR is UntrustedFork. The ReBAC `read & !is_untrusted_fork`
    //    ABAC edge (contract 4.9) that cross-checks the structural flag is the named Identity seam;
    //    here `read_excludes_fork = !is_fork` mirrors the structural provenance fail-closed.
    let provenance = RunProvenance {
        is_fork: facts.is_fork,
        targets_self_hosted: false,
        read_excludes_fork: !facts.is_fork,
    };
    let stamp: TrustStamp = stamp_trust(&provenance);

    // 6. Resolve the CAS snapshot — a ResolveError (floating tag / cycle) is a fail-closed skip.
    let (_snapshot, address) = match resolve_versioned_snapshot(&def, blobs, &tenant) {
        Ok(r) => r,
        Err(e) => return DispatchOutcome::Skip(SkipReason::ResolveError(e)),
    };
    let snapshot = snapshot_ref(&tenant, &address);

    // 7. Reserve/start — build the atomic bundle. The run_id is deterministic from the triggering
    //    event_id (idempotent: a redelivery mints the SAME run). One check context per top-level job.
    let run_id = deterministic_uuid(&format!("run:{}", ev.event_id.0));
    let wf_run_id = deterministic_uuid(&format!("wf:{}", ev.event_id.0));
    let contexts: Vec<CheckContext> = def
        .jobs
        .iter()
        .map(|j| CheckContext::ci(j.name.clone()))
        .collect();
    let run_facts = RunFacts {
        run_id: run_id.clone(),
        repo_ref: facts.repo.clone(),
        commit_oid: facts.new_oid.clone(),
        contexts,
        cause_event_id: ev.event_id.clone(),
    };
    let handoff = reserve_and_start(&snapshot, &stamp, &def.on, &run_facts);

    let reserve = ReserveFacts {
        region: region.clone(),
        project_id: deterministic_uuid(&format!("project:{}", facts.repo)),
        pipeline_id: deterministic_uuid(&format!("pipeline:{}", facts.repo)),
        wf_run_id,
        correlation_id: ev.correlation_id.0.clone(),
        repo_ref: facts.repo,
        commit_oid: facts.new_oid,
    };
    let emit_ctx = EmitContextBase {
        tenant: tenant.clone(),
        region: ev.region.clone(),
        actor: ev.actor.clone(),
        schema_ver: 1,
        occurred_at: ev.occurred_at.clone(),
        recorded_at: ev.recorded_at.clone(),
        caused_by: None,
    };

    DispatchOutcome::Arm(Box::new(ArmedRun {
        handoff,
        reserve,
        tenant,
        actor: ev.actor.clone(),
        emit_ctx,
    }))
}

// =================================================================================================
// 5. The live EventHandler.
// =================================================================================================

/// **The live `ci-dispatch.trigger` bus consumer (the CT-004b deliverable).** On a matching
/// `git.ref.updated` / PR event it drives [`plan_dispatch`] and, on an `Arm`, persists the atomic
/// reserve bundle through the [`ReserveStore`]; every `Skip` is surfaced, then classified as a
/// clean/structural ACK, transient retry, or permanent dead-letter. The
/// `Consumer` runtime wraps this with the seven rules (rule-1 dedup on `event_id` via the durable
/// `consumer_dedup` ledger, ack-after-enqueue, bounded prefetch, the lag metric) — this body is the
/// trigger LOGIC only. Idempotent on `event_id` twice over: the runtime's dedup AND the deterministic
/// `run_id` (`ON CONFLICT DO NOTHING`).
pub struct CiTriggerHandler {
    reader: Arc<dyn GitConfigReader>,
    blobs: Arc<dyn BlobStore + Send + Sync>,
    reserve: Arc<dyn ReserveStore>,
    expected_region: Option<String>,
    /// The last outcomes (surfaced skips + armed runs), bounded — the observability the drill reads
    /// (a skip is NOT silent). Bounded so it cannot grow unboundedly under a busy consumer.
    trace: Mutex<Vec<String>>,
}

impl CiTriggerHandler {
    /// Build the handler from the three seams: the git config reader, the tenant blob store (the CAS
    /// snapshot sink), and the durable reserve store.
    pub fn new(
        reader: Arc<dyn GitConfigReader>,
        blobs: Arc<dyn BlobStore + Send + Sync>,
        reserve: Arc<dyn ReserveStore>,
    ) -> CiTriggerHandler {
        CiTriggerHandler {
            reader,
            blobs,
            reserve,
            expected_region: None,
            trace: Mutex::new(Vec::new()),
        }
    }

    /// Bind production intake to the configured cell region.
    pub fn for_region(
        reader: Arc<dyn GitConfigReader>,
        blobs: Arc<dyn BlobStore + Send + Sync>,
        reserve: Arc<dyn ReserveStore>,
        expected_region: impl Into<String>,
    ) -> CiTriggerHandler {
        CiTriggerHandler {
            reader,
            blobs,
            reserve,
            expected_region: Some(expected_region.into()),
            trace: Mutex::new(Vec::new()),
        }
    }

    /// The consumer's durable name (the `consumer_dedup` PK half — one stable name for the trigger
    /// leg, so a triggering `event_id` dedups against exactly this consumer's prior effects).
    pub fn consumer_name(&self) -> &'static str {
        TRIGGER_CONSUMER
    }

    /// The bounded outcome trace (the surfaced skips + armed-run ids) — the observability a drill
    /// reads to assert a malformed config was a surfaced skip, not a silent swallow.
    pub fn trace(&self) -> Vec<String> {
        self.trace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn record(&self, line: String) {
        let mut t = self.trace.lock().unwrap_or_else(|e| e.into_inner());
        if t.len() >= 256 {
            t.remove(0);
        }
        t.push(line);
    }
}

/// **Build the registered `ci-dispatch.trigger` [`ConsumerReg`] from the three seams.** The one
/// construction site both `main.rs` (production wiring) and the integration test use: it binds the
/// `*`-free whitelist ([`CI_TRIGGER_SUBJECT_STRS`]), wraps the [`CiTriggerHandler`] in the
/// `Consumer` runtime (rule-1 dedup + ack-after-enqueue + bounded prefetch + the lag metric), and
/// returns the `ConsumerReg` `dispatch_app_spec` carries. The `dedup` ledger is the runtime's
/// exactly-once-effect guard (the durable `consumer_dedup` in production).
pub fn build_trigger_consumer(
    reader: Arc<dyn GitConfigReader>,
    blobs: Arc<dyn BlobStore + Send + Sync>,
    reserve: Arc<dyn ReserveStore>,
    dedup: myelin_events::DedupLedger,
    expected_region: impl Into<String>,
    dead_letters: Arc<dyn myelin_events::DurableDeadLetter>,
) -> Result<myelin_substrate::ConsumerReg, myelin_events::SubscribeError> {
    let handler = CiTriggerHandler::for_region(reader, blobs, reserve, expected_region);
    let subscription = myelin_events::consumer::Subscription::bind(
        myelin_events::ConsumerName(TRIGGER_CONSUMER.into()),
        CI_TRIGGER_SUBJECT_STRS,
        myelin_events::PrefetchBound::DEFAULT,
    )?;
    Ok(myelin_substrate::ConsumerReg::new(
        myelin_events::Consumer::new(handler, subscription, dedup)
            .with_dead_letter_sink(myelin_events::DeadLetterSink::durable(dead_letters)),
    ))
}

impl EventHandler for CiTriggerHandler {
    fn subjects(&self) -> &'static [SubjectPattern] {
        ci_trigger_subjects()
    }

    fn handle(&self, ev: &EventEnvelope, tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        if let Some(expected) = &self.expected_region {
            if ev.region.0 != *expected {
                self.record(format!(
                    "region mismatch: envelope={} configured_cell={expected}",
                    ev.region.0
                ));
                return HandleOutcome::NonRetryable(myelin_events::Reason(format!(
                    "event region {} does not match configured cell region {expected}",
                    ev.region.0
                )));
            }
        }
        match plan_dispatch(ev, self.reader.as_ref(), self.blobs.as_ref()) {
            // Thread the consumer-runtime co-commit handle (`tx`) into the reserve store: the true
            // co-commit impl writes the bundle on the SAME tx as the dedup mark (#7); the production
            // outbox impl ignores it and rides its own absorb-mode outbox (H1).
            DispatchOutcome::Arm(armed) => match self.reserve.persist(&armed, tx) {
                Ok(()) => {
                    self.record(format!(
                        "armed run_id={} repo={} snapshot={}",
                        armed.handoff.run_write.run_id,
                        armed.reserve.correlation_id,
                        armed.handoff.run_write.definition_snapshot.0
                    ));
                    HandleOutcome::Done
                }
                // A durable-write failure is RETRYABLE (a transient DB/broker hiccup): NOT poison,
                // NOT a silent swallow — the message stays pending and redelivers (the dedup +
                // deterministic run_id make the retry exactly-once).
                Err(e) => {
                    self.record(format!("reserve FAILED (retry): {e}"));
                    HandleOutcome::Retry(myelin_events::Backoff { seconds: 5 })
                }
            },
            DispatchOutcome::Skip(reason) => {
                // Every skip is SURFACED. A NotATrigger/NoConfig clean skip is not recorded; backing
                // unavailability retries without committing dedup, permanent Git invalidity poisons,
                // and structural config/resolve skips are terminal ACKs.
                match &reason {
                    SkipReason::NotATrigger(_) | SkipReason::NoConfig => {}
                    other => self.record(format!("skip: {other}")),
                }
                if matches!(&reason, SkipReason::ReadFailed(error) if error.is_retryable()) {
                    HandleOutcome::Retry(myelin_events::Backoff { seconds: 5 })
                } else if matches!(reason, SkipReason::InvalidProvenance(_)) {
                    // The detailed, attacker-controlled provenance stays only in the bounded
                    // in-process trace above; the durable DLQ reason is deliberately PII-free.
                    HandleOutcome::NonRetryable(myelin_events::Reason(
                        "invalid trigger provenance".into(),
                    ))
                } else if let SkipReason::ReadFailed(error) = reason {
                    HandleOutcome::NonRetryable(myelin_events::Reason(error.to_string()))
                } else if matches!(reason, SkipReason::ResolveError(ResolveError::BlobWrite(_))) {
                    HandleOutcome::Retry(myelin_events::Backoff { seconds: 5 })
                } else {
                    HandleOutcome::Done
                }
            }
        }
    }
}

// =================================================================================================
// 6. Test doubles (unit-test-only; DB-free) + the durable git reader adapter.
// =================================================================================================

/// **A DB-free [`GitConfigReader`] test double: a map of `(repo, oid, path) → bytes`.** Absent keys
/// read as `Ok(None)` (a clean skip); a key registered with an error-sentinel yields `Err` (the
/// read-failed branch). Used by the unit tests to drive every pipeline branch DB-free.
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct MapGitConfigReader {
    files: std::collections::HashMap<(String, String, String), Vec<u8>>,
    fail: std::collections::HashSet<(String, String, String)>,
}

#[cfg(any(test, feature = "test-support"))]
impl MapGitConfigReader {
    /// A fresh, empty reader (every read is `Ok(None)` — the no-config skip).
    pub fn new() -> MapGitConfigReader {
        MapGitConfigReader::default()
    }

    /// Register `bytes` at `(repo, oid, path)`.
    pub fn with_file(
        mut self,
        repo: &str,
        oid: &str,
        path: &str,
        bytes: impl Into<Vec<u8>>,
    ) -> MapGitConfigReader {
        self.files
            .insert((repo.into(), oid.into(), path.into()), bytes.into());
        self
    }

    /// Register a backend READ FAILURE at `(repo, oid, path)` (the fail-closed branch).
    pub fn with_failure(mut self, repo: &str, oid: &str, path: &str) -> MapGitConfigReader {
        self.fail.insert((repo.into(), oid.into(), path.into()));
        self
    }
}

#[cfg(any(test, feature = "test-support"))]
impl GitConfigReader for MapGitConfigReader {
    fn read_repo_file(
        &self,
        _tenant: &str,
        _region: &str,
        repo: &str,
        oid: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, GitReadError> {
        let key = (repo.to_string(), oid.to_string(), path.to_string());
        if self.fail.contains(&key) {
            return Err(GitReadError::Unavailable(format!("injected read failure at {path}")));
        }
        Ok(self.files.get(&key).cloned())
    }
}

/// **A DB-free [`ReserveStore`] test double: records the armed runs it was asked to persist.** Used
/// by the unit tests to assert the atomic bundle the consumer produced (the `ci_run` row + the events)
/// WITHOUT a live DB. NOT a durable store — it is a `#[cfg(test)]`-gated recorder, so the
/// `no-in-memory-durable-store` lint (which strips test-gated doubles) does not fire.
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct RecordingReserveStore {
    persisted: Mutex<Vec<ArmedRun>>,
}

#[cfg(any(test, feature = "test-support"))]
impl RecordingReserveStore {
    /// A fresh recorder.
    pub fn new() -> RecordingReserveStore {
        RecordingReserveStore::default()
    }

    /// The armed runs persisted so far (the atomic bundles the consumer produced).
    pub fn persisted(&self) -> Vec<ArmedRun> {
        self.persisted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ReserveStore for RecordingReserveStore {
    fn persist(
        &self,
        armed: &ArmedRun,
        _tx: &mut myelin_events::HandlerTx<'_>,
    ) -> Result<(), ReserveError> {
        self.persisted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(armed.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::TrustTier;
    use myelin_events::{
        consumer::{Consumer, ConsumerName, Delivered, Message, PrefetchBound, Subscription},
        Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, DedupLedger, EventId, EventType,
        Timestamp, Visibility,
    };
    use myelin_git::events::{GIT_PR_OPENED, GIT_PR_SYNCHRONIZED, GIT_REF_UPDATED};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_storage::{ContentHash, FsBlobStore};
    use myelin_tenancy::{Region, TenantId};

    const TEST_OID: &str = "dddddddddddddddddddddddddddddddddddddddd";

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("pusher".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    /// A `git.ref.updated` envelope for `repo` pushing `new_oid`, with the given payload extras.
    fn push_envelope(repo: &str, new_oid: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("ev-push-1".into()),
            type_: EventType(GIT_REF_UPDATED.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(principal()),
            subject: ArtifactRef(format!("myelin://acme/git/ref/{repo}:refs/heads/main")),
            aggregate: AggregateKey(format!("{repo}:refs/heads/main")),
            causation_id: None,
            correlation_id: CorrelationId("corr-1".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-07-16T00:00:00Z".into()),
            recorded_at: Timestamp("2026-07-16T00:00:00Z".into()),
            payload: serde_json::json!({
                "repo": repo,
                "ref": "refs/heads/main",
                "new_oid": new_oid,
                "old_oid": "0000000000000000000000000000000000000000",
                "forced": false,
            }),
        }
    }

    fn pr_envelope(event_type: &str, fork_fields: serde_json::Value) -> EventEnvelope {
        let mut ev = push_envelope("web", TEST_OID);
        ev.type_ = EventType(event_type.into());
        ev.subject = ArtifactRef("myelin://acme/git/pr/web:42".into());
        ev.aggregate = AggregateKey("git/pr/web:42".into());
        ev.payload = serde_json::json!({ "repo": "web", "number": 42, "head_oid": TEST_OID });
        ev.payload.as_object_mut().unwrap().extend(fork_fields.as_object().unwrap().clone());
        ev
    }

    fn valid_toml() -> &'static [u8] {
        concat!(
            "on = \"push\"\n\n",
            "[[jobs]]\n",
            "name = \"build\"\n",
            "image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"\n",
            "command = [\"build\"]\n",
        )
        .as_bytes()
    }

    fn valid_v2_toml() -> &'static [u8] {
        concat!(
            "schema_version = 2\n",
            "on = \"push\"\n\n",
            "[execution]\n",
            "profile = \"linux-small-v1\"\n\n",
            "[[jobs]]\n",
            "name = \"build\"\n",
            "image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"\n",
            "command = [\"build\"]\n",
        )
        .as_bytes()
    }

    fn valid_pr_toml() -> &'static [u8] {
        concat!(
            "on = \"pull_request\"\n\n",
            "[[jobs]]\nname = \"build\"\n",
            "image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"\n",
            "command = [\"build\"]\n"
        )
        .as_bytes()
    }

    struct NoConfigRead;

    impl GitConfigReader for NoConfigRead {
        fn read_repo_file(
            &self, _tenant: &str, _region: &str, _repo: &str, _oid: &str, _path: &str,
        ) -> Result<Option<Vec<u8>>, GitReadError> {
            panic!("malformed provenance reached a config read")
        }
    }

    struct NoCasAccess;

    impl BlobStore for NoCasAccess {
        fn put(&self, _tenant: &TenantId, _bytes: &[u8]) -> myelin_storage::blob::Result<ContentHash> {
            panic!("malformed provenance reached CAS")
        }
        fn get(&self, _tenant: &TenantId, _hash: &ContentHash) -> myelin_storage::blob::Result<Vec<u8>> {
            panic!("malformed provenance reached CAS")
        }
        fn head(&self, _tenant: &TenantId, _hash: &ContentHash) -> myelin_storage::blob::Result<myelin_storage::blob::BlobMeta> {
            panic!("malformed provenance reached CAS")
        }
        fn delete(&self, _tenant: &TenantId, _hash: &ContentHash) -> myelin_storage::blob::Result<()> {
            panic!("malformed provenance reached CAS")
        }
    }

    fn assert_malformed_before_side_effects(ev: &EventEnvelope) {
        assert!(matches!(
            plan_dispatch(ev, &NoConfigRead, &NoCasAccess),
            DispatchOutcome::Skip(SkipReason::MalformedPayload(_))
        ));
    }

    fn arm(ev: &EventEnvelope, reader: &dyn GitConfigReader) -> DispatchOutcome {
        let blobs = FsBlobStore::new();
        plan_dispatch(ev, reader, &blobs)
    }

    fn runtime(handler: CiTriggerHandler, ledger: DedupLedger) -> Consumer<CiTriggerHandler> {
        let subscription = Subscription::bind(
            ConsumerName(TRIGGER_CONSUMER.into()),
            CI_TRIGGER_SUBJECT_STRS,
            PrefetchBound::DEFAULT,
        )
        .expect("CI trigger subscription");
        Consumer::new(handler, subscription, ledger)
    }

    fn message(envelope: EventEnvelope) -> Message {
        Message { subject: envelope.subject.0.clone(), envelope }
    }

    fn temp_git_root(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "myelin-ci-dispatch-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create temporary git root");
        root
    }

    #[test]
    fn authoritative_git_root_requires_an_existing_absolute_directory() {
        assert!(matches!(
            AuthoritativeGitRoot::validate("relative/git-root"),
            Err(GitRootError(message)) if message.contains("absolute")
        ));
        let missing = std::env::temp_dir().join(format!("myelin-ci-dispatch-missing-root-{}", std::process::id()));
        assert!(AuthoritativeGitRoot::validate(missing).is_err());

        let root = temp_git_root("validated-root");
        let validated = AuthoritativeGitRoot::validate(&root).expect("valid root");
        assert_eq!(validated.as_path(), root.canonicalize().unwrap());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn region_mismatch_dlqs_before_git_cas_or_reserve_then_records_terminal_tombstone() {
        let reserve = Arc::new(RecordingReserveStore::new());
        let handler = CiTriggerHandler::for_region(
            Arc::new(NoConfigRead), Arc::new(NoCasAccess), reserve.clone(), "us-east",
        );
        let ledger = DedupLedger::new();
        let consumer = runtime(handler, ledger.clone());

        assert!(matches!(
            consumer.deliver(&message(push_envelope("web", TEST_OID))),
            Delivered::DeadLettered(_)
        ));
        assert!(reserve.persisted().is_empty(), "region mismatch has zero reserve effects");
        assert_eq!(ledger.len(), 1, "DLQ persistence precedes the terminal tombstone");
    }

    #[test]
    fn missing_repository_or_exact_commit_retries_with_zero_dedup_or_effect() {
        let root = temp_git_root("unavailable-git");
        let store = myelin_git::durable::DurableGitStore::rooted(&root);
        let loc = myelin_git::core::RepoLoc::new("acme", "fr-par", "web");

        for (event_id, setup_repo) in [("missing-repo", false), ("missing-commit", true)] {
            if setup_repo {
                store.create_repo(&loc).expect("create bare repo");
            }
            let reader: Arc<dyn GitConfigReader> = Arc::new(DurableGitConfigReader::new(
                myelin_git::durable::DurableGitStore::rooted(&root),
            ));
            let reserve = Arc::new(RecordingReserveStore::new());
            let handler = CiTriggerHandler::for_region(
                reader, Arc::new(FsBlobStore::new()), reserve.clone(), "fr-par",
            );
            let ledger = DedupLedger::new();
            let consumer = runtime(handler, ledger.clone());
            let mut event = push_envelope("web", "1111111111111111111111111111111111111111");
            event.event_id = EventId(event_id.into());

            assert_eq!(consumer.deliver(&message(event)), Delivered::Retried(5));
            assert!(reserve.persisted().is_empty());
            assert!(ledger.is_empty(), "Git retry rolls back the dedup mark");
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn existing_commit_without_ci_config_is_a_genuine_acked_no_config() {
        let root = temp_git_root("no-config");
        let store = myelin_git::durable::DurableGitStore::rooted(&root);
        let repo = store
            .create_repo(&myelin_git::core::RepoLoc::new("acme", "fr-par", "web"))
            .expect("create bare repo");
        let (commit, _, _) = repo
            .build_file_commit(
                "refs/heads/main", "README.md", b"no CI definition\n", "seed", "ci", "ci@invalid",
            )
            .expect("seed commit");
        let reserve = Arc::new(RecordingReserveStore::new());
        let handler = CiTriggerHandler::for_region(
            Arc::new(DurableGitConfigReader::new(store)),
            Arc::new(FsBlobStore::new()),
            reserve.clone(),
            "fr-par",
        );
        let ledger = DedupLedger::new();
        let consumer = runtime(handler, ledger.clone());

        assert_eq!(
            consumer.deliver(&message(push_envelope("web", commit.as_str()))),
            Delivered::Acked
        );
        assert!(reserve.persisted().is_empty());
        assert_eq!(ledger.len(), 1, "genuine NoConfig is terminally acknowledged");
        drop(consumer);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn oversized_config_is_terminal_poison_without_business_effect() {
        let reader: Arc<dyn GitConfigReader> = Arc::new(MapGitConfigReader::new().with_file(
            "web", TEST_OID, ".myelin/ci.toml",
            vec![b' '; crate::config::MAX_CI_CONFIG_BYTES + 1],
        ));
        let reserve = Arc::new(RecordingReserveStore::new());
        let handler = CiTriggerHandler::for_region(
            reader, Arc::new(NoCasAccess), reserve.clone(), "fr-par",
        );
        let ledger = DedupLedger::new();
        let consumer = runtime(handler, ledger.clone());

        assert!(matches!(
            consumer.deliver(&message(push_envelope("web", TEST_OID))),
            Delivered::DeadLettered(_)
        ));
        assert!(reserve.persisted().is_empty());
        assert_eq!(ledger.len(), 1, "DLQ persistence precedes the terminal tombstone");
    }

    struct FailingBlobStore;

    impl BlobStore for FailingBlobStore {
        fn put(&self, _tenant: &TenantId, _bytes: &[u8]) -> myelin_storage::blob::Result<ContentHash> {
            Err(myelin_storage::blob::BlobError::MalformedAddress("injected S3 outage".into()))
        }
        fn get(&self, _: &TenantId, _: &ContentHash) -> myelin_storage::blob::Result<Vec<u8>> {
            panic!("CAS read reached during write-failure test")
        }
        fn head(&self, _: &TenantId, _: &ContentHash) -> myelin_storage::blob::Result<myelin_storage::blob::BlobMeta> {
            panic!("CAS head reached during write-failure test")
        }
        fn delete(&self, _: &TenantId, _: &ContentHash) -> myelin_storage::blob::Result<()> {
            panic!("CAS delete reached during write-failure test")
        }
    }

    #[test]
    fn cas_snapshot_write_failure_retries_without_dedup_or_reserve_effect() {
        let reader: Arc<dyn GitConfigReader> = Arc::new(
            MapGitConfigReader::new().with_file("web", TEST_OID, ".myelin/ci.toml", valid_toml()),
        );
        let reserve = Arc::new(RecordingReserveStore::new());
        let handler = CiTriggerHandler::for_region(
            reader, Arc::new(FailingBlobStore), reserve.clone(), "fr-par",
        );
        let ledger = DedupLedger::new();
        let consumer = runtime(handler, ledger.clone());

        assert_eq!(
            consumer.deliver(&message(push_envelope("web", TEST_OID))),
            Delivered::Retried(5)
        );
        assert!(reserve.persisted().is_empty());
        assert!(ledger.is_empty(), "S3 retry rolls back the dedup mark");
    }

    /// **subjects() is a whitelist, NEVER `*` (BUS-3).** The coarse prefix is bounded (non-`*`), so
    /// `Subscription::bind` accepts it and the head-of-line guard holds.
    #[test]
    fn subjects_is_a_whitelist_never_wildcard() {
        let sub = myelin_events::consumer::Subscription::bind(
            myelin_events::ConsumerName(TRIGGER_CONSUMER.into()),
            CI_TRIGGER_SUBJECT_STRS,
            myelin_events::PrefetchBound::DEFAULT,
        );
        assert!(sub.is_ok(), "the CI trigger whitelist binds (never `*`)");
        for s in CI_TRIGGER_SUBJECT_STRS {
            assert_ne!(*s, "*", "no `*` in the whitelist");
            assert_ne!(*s, ">", "no `>` in the whitelist");
            assert!(!s.is_empty(), "no empty (over-broad) subject");
        }
        // Finding #12: the `SubjectPattern` form a router iterating `subjects()` sees must ALSO be
        // bounded — an empty pattern is `starts_with("")` = match-all. Assert non-empty + non-wildcard,
        // and that it mirrors the `&str` whitelist exactly (one source of truth).
        let patterns = ci_trigger_subjects();
        assert!(!patterns.is_empty(), "subjects() is a non-empty whitelist");
        for p in patterns {
            assert!(!p.0.is_empty(), "no EMPTY (match-all) SubjectPattern in subjects() — finding #12");
            assert_ne!(p.0, "*");
            assert_ne!(p.0, ">");
        }
        assert_eq!(
            patterns.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(),
            CI_TRIGGER_SUBJECT_STRS.to_vec(),
            "subjects() mirrors the &str whitelist exactly"
        );
    }

    /// **No `.myelin/ci.*` at the pushed ref → a clean NoConfig skip (NOT an error, no run).**
    #[test]
    fn no_config_is_a_clean_skip() {
        let ev = push_envelope("web", TEST_OID);
        let reader = MapGitConfigReader::new();
        assert!(matches!(arm(&ev, &reader), DispatchOutcome::Skip(SkipReason::NoConfig)));
    }

    /// **A malformed `.myelin/ci.toml` → a fail-closed, SURFACED ConfigError skip (no run, no crash).**
    #[test]
    fn a_malformed_config_is_a_surfaced_skip() {
        let ev = push_envelope("web", TEST_OID);
        let reader =
            MapGitConfigReader::new().with_file("web", TEST_OID, ".myelin/ci.toml", &b"on = = broken"[..]);
        assert!(matches!(
            arm(&ev, &reader),
            DispatchOutcome::Skip(SkipReason::ConfigError(_))
        ));
    }

    /// **A backend read failure → a fail-closed ReadFailed skip (no run without a proven definition).**
    #[test]
    fn a_read_failure_is_fail_closed() {
        let ev = push_envelope("web", TEST_OID);
        let reader = MapGitConfigReader::new().with_failure("web", TEST_OID, ".myelin/ci.toml");
        assert!(matches!(
            arm(&ev, &reader),
            DispatchOutcome::Skip(SkipReason::ReadFailed(_))
        ));
    }

    #[test]
    fn an_oversized_config_is_refused_before_parsing() {
        let ev = push_envelope("web", TEST_OID);
        let reader = MapGitConfigReader::new().with_file(
            "web", TEST_OID, ".myelin/ci.toml", vec![b' '; crate::config::MAX_CI_CONFIG_BYTES + 1],
        );
        assert!(matches!(arm(&ev, &reader), DispatchOutcome::Skip(SkipReason::ReadFailed(_))));
    }

    /// **A non-matching trigger (a `pull_request` config on a push) → TriggerNotMatched skip.**
    #[test]
    fn a_non_matching_trigger_skips() {
        let ev = push_envelope("web", TEST_OID);
        let pr_config = concat!(
            "on = \"pull_request\"\n\n",
            "[[jobs]]\nname = \"build\"\n",
            "image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"\n",
            "command = [\"build\"]\n"
        );
        let reader =
            MapGitConfigReader::new().with_file("web", TEST_OID, ".myelin/ci.toml", pr_config.as_bytes());
        assert!(matches!(
            arm(&ev, &reader),
            DispatchOutcome::Skip(SkipReason::TriggerNotMatched)
        ));
    }

    /// **A floating-tag config → a fail-closed ResolveError skip (the supply-chain control).**
    #[test]
    fn a_floating_tag_is_a_surfaced_resolve_skip() {
        let ev = push_envelope("web", TEST_OID);
        let floating = "on = \"push\"\n\n[[jobs]]\nname = \"build\"\nimage = \"alpine:3\"\ncommand = [\"build\"]\n";
        let reader =
            MapGitConfigReader::new().with_file("web", TEST_OID, ".myelin/ci.toml", floating.as_bytes());
        assert!(matches!(
            arm(&ev, &reader),
            DispatchOutcome::Skip(SkipReason::ResolveError(ResolveError::FloatingTag { .. }))
        ));
    }

    /// **A missing `repo`/`new_oid` payload → a fail-closed MalformedPayload skip.**
    #[test]
    fn a_malformed_payload_skips() {
        let mut ev = push_envelope("web", TEST_OID);
        ev.payload = serde_json::json!({ "ref": "refs/heads/main" });
        assert!(matches!(
            arm(&ev, &MapGitConfigReader::new()),
            DispatchOutcome::Skip(SkipReason::MalformedPayload(_))
        ));
    }

    fn assert_invalid_provenance_is_poison_before_effects(ev: EventEnvelope) {
        let reserve = Arc::new(RecordingReserveStore::new());
        let handler = CiTriggerHandler::for_region(
            Arc::new(NoConfigRead), Arc::new(NoCasAccess), reserve.clone(), "fr-par",
        );
        match handler.handle(&ev, &mut myelin_events::HandlerTx::none()) {
            HandleOutcome::NonRetryable(myelin_events::Reason(reason)) => {
                assert_eq!(reason, "invalid trigger provenance");
                assert!(!reason.contains("ATTACKER_SENTINEL"));
            }
            other => panic!("invalid provenance must be permanent poison, got {other:?}"),
        }
        let ledger = DedupLedger::new();
        let consumer = runtime(handler, ledger.clone());
        assert!(matches!(consumer.deliver(&message(ev)), Delivered::DeadLettered(_)));
        assert!(reserve.persisted().is_empty(), "invalid provenance has zero reserve effects");
        assert_eq!(ledger.len(), 1, "permanent poison records a terminal tombstone after DLQ");
    }

    #[test]
    fn push_subject_aggregate_payload_and_ref_provenance_must_cohere_before_effects() {
        let mut cases = Vec::new();
        let mut wrong_subject_repo = push_envelope("team/web", TEST_OID);
        wrong_subject_repo.subject = ArtifactRef(
            "myelin://acme/git/ref/other/web:refs/heads/main".into(),
        );
        cases.push(wrong_subject_repo);
        let mut wrong_tenant = push_envelope("web", TEST_OID);
        wrong_tenant.subject = ArtifactRef("myelin://other/git/ref/web:refs/heads/main".into());
        cases.push(wrong_tenant);
        let mut wrong_aggregate = push_envelope("web", TEST_OID);
        wrong_aggregate.aggregate = AggregateKey("ATTACKER_SENTINEL:refs/heads/main".into());
        cases.push(wrong_aggregate);
        let mut invalid_repo = push_envelope("web", TEST_OID);
        invalid_repo.payload["repo"] = serde_json::json!("../web");
        cases.push(invalid_repo);
        let mut invalid_ref = push_envelope("web", TEST_OID);
        invalid_ref.payload["ref"] = serde_json::json!("HEAD");
        cases.push(invalid_ref);
        let mut hidden_ref_component = push_envelope("web", TEST_OID);
        hidden_ref_component.payload["ref"] = serde_json::json!("refs/heads/.hidden");
        cases.push(hidden_ref_component);
        for event in cases {
            assert_invalid_provenance_is_poison_before_effects(event);
        }
    }

    #[test]
    fn pr_subject_aggregate_payload_and_number_provenance_must_cohere_before_effects() {
        let mut cases = Vec::new();
        let mut wrong_subject = pr_envelope(GIT_PR_OPENED, serde_json::json!({ "is_fork": false }));
        wrong_subject.subject = ArtifactRef("myelin://acme/git/pr/other:42".into());
        cases.push(wrong_subject);
        let mut wrong_aggregate = pr_envelope(GIT_PR_OPENED, serde_json::json!({ "is_fork": false }));
        wrong_aggregate.aggregate = AggregateKey("git/pr/web:41".into());
        cases.push(wrong_aggregate);
        let mut invalid_repo = pr_envelope(GIT_PR_OPENED, serde_json::json!({ "is_fork": false }));
        invalid_repo.payload["repo"] = serde_json::json!("group//web");
        cases.push(invalid_repo);
        let mut invalid_number = pr_envelope(GIT_PR_OPENED, serde_json::json!({ "is_fork": false }));
        invalid_number.payload["number"] = serde_json::json!(0);
        cases.push(invalid_number);
        for event in cases {
            assert_invalid_provenance_is_poison_before_effects(event);
        }
    }

    #[test]
    fn revspecs_refs_head_abbreviations_and_non_hex_oids_are_permanent_before_git_read() {
        for invalid in ["HEAD", "main", "refs/heads/main", "deadbeef", "ATTACKER_SENTINEL"] {
            let mut event = push_envelope("web", TEST_OID);
            event.payload["new_oid"] = serde_json::json!(invalid);
            assert!(matches!(
                plan_dispatch(&event, &NoConfigRead, &NoCasAccess),
                DispatchOutcome::Skip(SkipReason::ReadFailed(GitReadError::Invalid(_)))
            ));
        }
        let mut event = push_envelope("web", TEST_OID);
        event.payload["new_oid"] = serde_json::json!("ATTACKER_SENTINEL");
        let handler = CiTriggerHandler::for_region(
            Arc::new(NoConfigRead), Arc::new(NoCasAccess),
            Arc::new(RecordingReserveStore::new()), "fr-par",
        );
        let HandleOutcome::NonRetryable(myelin_events::Reason(reason)) =
            handler.handle(&event, &mut myelin_events::HandlerTx::none()) else {
                panic!("invalid oid must be permanent poison");
            };
        assert!(!reason.contains("ATTACKER_SENTINEL"));
    }

    #[test]
    fn uppercase_exact_oid_is_canonicalized_before_git_read_and_persistence() {
        let uppercase = "DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD";
        let reader = MapGitConfigReader::new().with_file(
            "web", TEST_OID, ".myelin/ci.toml", valid_toml(),
        );
        let DispatchOutcome::Arm(armed) = arm(&push_envelope("web", uppercase), &reader) else {
            panic!("uppercase exact oid must canonicalize and arm");
        };
        assert_eq!(armed.reserve.commit_oid, TEST_OID);
    }

    #[test]
    fn durable_reader_preserves_namespaced_repository_slugs() {
        let root = temp_git_root("namespaced-repo");
        let store = myelin_git::durable::DurableGitStore::rooted(&root);
        let repo = store.create_repo(&myelin_git::core::RepoLoc::new(
            "acme", "fr-par", "team/web",
        )).expect("create namespaced bare repo");
        let raw = git2::Repository::open_bare(repo.path()).expect("open namespaced repo");
        let blob = raw.blob(valid_toml()).expect("write CI config blob");
        let mut ci = raw.treebuilder(None).expect("CI tree builder");
        ci.insert("ci.toml", blob, 0o100644).expect("insert config");
        let ci_tree = ci.write().expect("write CI tree");
        let mut root_tree = raw.treebuilder(None).expect("root tree builder");
        root_tree.insert(".myelin", ci_tree, 0o040000).expect("insert .myelin tree");
        let root_tree = raw.find_tree(root_tree.write().expect("write root tree")).unwrap();
        let signature = git2::Signature::now("ci", "ci@invalid").unwrap();
        let commit = raw.commit(
            Some("refs/heads/main"), &signature, &signature, "seed", &root_tree, &[],
        ).expect("seed CI config").to_string();
        let reader = DurableGitConfigReader::new(store);
        let DispatchOutcome::Arm(armed) = arm(
            &push_envelope("team/web", &commit), &reader,
        ) else {
            panic!("the full namespaced slug must select its own repository");
        };
        assert_eq!(armed.reserve.repo_ref, "team/web");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn both_pr_events_refuse_missing_mistyped_or_conflicting_fork_evidence_before_side_effects() {
        let malformed = [
            serde_json::json!({}),
            serde_json::json!({ "is_fork": "false" }),
            serde_json::json!({ "forked": 0 }),
            serde_json::json!({ "is_fork": true, "forked": false }),
            serde_json::json!({ "is_fork": true, "forked": "true" }),
        ];
        for event_type in [GIT_PR_OPENED, GIT_PR_SYNCHRONIZED] {
            for fields in &malformed {
                assert_malformed_before_side_effects(&pr_envelope(event_type, fields.clone()));
            }
        }
    }

    #[test]
    fn both_pr_events_accept_boolean_canonical_legacy_and_equal_alias_evidence() {
        let cases = [
            (serde_json::json!({ "is_fork": false }), "trusted"),
            (serde_json::json!({ "forked": false }), "trusted"),
            (serde_json::json!({ "is_fork": false, "forked": false }), "trusted"),
            (serde_json::json!({ "is_fork": true }), "untrusted_fork"),
            (serde_json::json!({ "forked": true }), "untrusted_fork"),
            (serde_json::json!({ "is_fork": true, "forked": true }), "untrusted_fork"),
        ];
        for event_type in [GIT_PR_OPENED, GIT_PR_SYNCHRONIZED] {
            for (fields, expected_tier) in &cases {
                let ev = pr_envelope(event_type, fields.clone());
                let reader = MapGitConfigReader::new().with_file("web", TEST_OID, ".myelin/ci.toml", valid_pr_toml());
                let DispatchOutcome::Arm(armed) = arm(&ev, &reader) else {
                    panic!("explicit boolean fork evidence must reach the matching PR config");
                };
                assert_eq!(armed.handoff.run_write.trust_tier, *expected_tier);
            }
        }
    }

    #[test]
    fn push_preserves_absence_and_true_but_refuses_mistyped_or_conflicting_fork_evidence() {
        let reader = MapGitConfigReader::new().with_file("web", TEST_OID, ".myelin/ci.toml", valid_toml());
        let DispatchOutcome::Arm(absent) = arm(&push_envelope("web", TEST_OID), &reader) else {
            panic!("legacy push without fork evidence remains compatible");
        };
        assert_eq!(absent.handoff.run_write.trust_tier, "trusted");

        let mut fork = push_envelope("web", TEST_OID);
        fork.payload["is_fork"] = serde_json::json!(true);
        let DispatchOutcome::Arm(fork) = arm(&fork, &reader) else {
            panic!("well-typed push fork evidence must be preserved");
        };
        assert_eq!(fork.handoff.run_write.trust_tier, "untrusted_fork");

        for fields in [
            serde_json::json!({ "is_fork": "false" }),
            serde_json::json!({ "forked": null }),
            serde_json::json!({ "is_fork": false, "forked": true }),
        ] {
            let mut ev = push_envelope("web", TEST_OID);
            ev.payload.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
            assert_malformed_before_side_effects(&ev);
        }
    }

    #[test]
    fn unknown_event_type_remains_not_a_trigger_before_payload_parsing() {
        let mut ev = push_envelope("web", TEST_OID);
        ev.type_ = EventType("git.pr.closed".into());
        ev.payload["is_fork"] = serde_json::json!("not-a-boolean");
        assert!(matches!(
            plan_dispatch(&ev, &NoConfigRead, &NoCasAccess),
            DispatchOutcome::Skip(SkipReason::NotATrigger(t)) if t == "git.pr.closed"
        ));
    }

    /// **The HAPPY PATH: a digest-pinned `.myelin/ci.toml` on a matching push → an ARMED run** with
    /// the atomic bundle (the `ci_run` row queued + `ci.run.started` + the queued check), the
    /// deterministic run_id, and the Trusted (member push) stamp.
    #[test]
    fn the_happy_path_arms_the_atomic_bundle() {
        let ev = push_envelope("web", TEST_OID);
        let reader =
            MapGitConfigReader::new().with_file("web", TEST_OID, ".myelin/ci.toml", valid_toml());
        let DispatchOutcome::Arm(armed) = arm(&ev, &reader) else {
            panic!("the digest-pinned config on a matching push must arm a run");
        };
        // The atomic bundle invariant holds (the prompt GATE).
        assert!(armed.handoff.is_atomic_bundle(), "the reserve bundle is atomic");
        assert_eq!(armed.handoff.run_write.state, "queued");
        assert_eq!(armed.handoff.run_write.trust_tier, "trusted", "a member push is trusted");
        assert_eq!(armed.handoff.run_write.trigger_kind, "push");
        assert_eq!(armed.handoff.run_write.cause_event_id, "ev-push-1");
        // One queued ci.check.updated per top-level job (build).
        assert_eq!(armed.handoff.queued_checks.len(), 1, "one queued check per job");
        // The run_id is deterministic from the event_id (idempotency anchor).
        assert_eq!(armed.handoff.run_write.run_id, deterministic_uuid("run:ev-push-1"));
        assert_eq!(armed.reserve.wf_run_id, deterministic_uuid("wf:ev-push-1"));
        assert_eq!(armed.reserve.correlation_id, "corr-1");
        assert_eq!(armed.reserve.repo_ref, "web");
        assert_eq!(armed.reserve.commit_oid, TEST_OID);
        let insert = ci_run_insert_from_armed(&armed);
        assert_eq!(insert.triggered_by.as_deref(), Some("pusher"));
        assert_eq!(armed.tenant, TenantId("acme".into()));
    }

    #[test]
    fn exact_v2_config_reaches_a_v2_cas_snapshot_through_the_production_consumer() {
        let ev = push_envelope("web", TEST_OID);
        let reader = MapGitConfigReader::new().with_file(
            "web",
            TEST_OID,
            ".myelin/ci.toml",
            valid_v2_toml(),
        );
        let blobs = FsBlobStore::new();
        let DispatchOutcome::Arm(armed) = plan_dispatch(&ev, &reader, &blobs) else {
            panic!("the exact V2 request must arm and persist its requested wire");
        };
        let digest = armed
            .handoff
            .run_write
            .definition_snapshot
            .0
            .rsplit('/')
            .next()
            .unwrap();
        let address = ContentHash::parse(digest).unwrap();
        let bytes = blobs.get(&TenantId("acme".into()), &address).unwrap();
        let decoded = myelin_ci_controlplane::decode_resolved_run_plan(&bytes).unwrap();
        assert!(decoded.as_v2().is_some());
    }

    /// **A fork PR (is_fork) → an UntrustedFork stamp on BOTH the row AND every check (X-1).**
    #[test]
    fn a_fork_pr_stamps_untrusted_fork() {
        let ev = pr_envelope(GIT_PR_OPENED, serde_json::json!({ "is_fork": true }));
        let pr_config = concat!(
            "on = \"pull_request\"\n\n[[jobs]]\nname = \"build\"\n",
            "image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"\n",
            "command = [\"build\"]\n"
        );
        let reader =
            MapGitConfigReader::new().with_file("web", TEST_OID, ".myelin/ci.toml", pr_config.as_bytes());
        let DispatchOutcome::Arm(armed) = arm(&ev, &reader) else {
            panic!("a fork PR with a matching config arms an untrusted-fork run");
        };
        assert_eq!(armed.handoff.run_write.trust_tier, "untrusted_fork");
        assert_eq!(armed.handoff.run_write.trigger_kind, "pull_request");
        for c in &armed.handoff.queued_checks {
            assert_eq!(c.payload["trust_tier"], "untrusted_fork", "X-1: 0 divergence");
        }
        // Cross-check the pure classifier agrees.
        assert_eq!(
            stamp_trust(&RunProvenance { is_fork: true, targets_self_hosted: false, read_excludes_fork: false })
                .job_tier,
            TrustTier::UntrustedFork
        );
    }

    /// **The handler persists an armed run through the ReserveStore + is idempotent on redelivery**
    /// (the SAME event_id → the SAME deterministic run_id → one persisted run, even delivered twice).
    #[test]
    fn the_handler_persists_and_is_idempotent() {
        let ev = push_envelope("web", TEST_OID);
        let reader: Arc<dyn GitConfigReader> = Arc::new(
            MapGitConfigReader::new().with_file("web", TEST_OID, ".myelin/ci.toml", valid_toml()),
        );
        let blobs: Arc<dyn BlobStore + Send + Sync> = Arc::new(FsBlobStore::new());
        let store = Arc::new(RecordingReserveStore::new());
        let handler = CiTriggerHandler::new(reader, blobs, store.clone());

        assert_eq!(handler.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);
        assert_eq!(handler.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done, "redelivery is handled");
        let persisted = store.persisted();
        // The ReserveStore itself is called twice (the runtime's consumer_dedup is what suppresses the
        // second delivery in production; here we prove BOTH calls mint the SAME run_id — the second
        // durable write is an ON CONFLICT DO NOTHING no-op, not a second run).
        assert_eq!(persisted.len(), 2, "the handler ran on both deliveries");
        assert_eq!(
            persisted[0].handoff.run_write.run_id, persisted[1].handoff.run_write.run_id,
            "the redelivery mints the SAME deterministic run_id (exactly-once run)"
        );
        // The surfaced trace shows the armed run (never a silent swallow).
        assert!(handler.trace().iter().any(|l| l.starts_with("armed run_id=")));
    }

    /// **H3: the check event id includes the run_id — distinct runs on the SAME (repo, commit,
    /// context) do NOT collide, and a redelivery of the SAME run re-mints the SAME id.** The check
    /// SUBJECT is run-agnostic (`repo#commit-<oid>/check-<context>`), so seeding on the subject alone
    /// (the pre-fix bug) minted ONE id for two distinct runs with DIFFERENT payloads (a collision).
    #[test]
    fn check_event_id_diverges_across_runs_stable_within_a_run() {
        let subject = "myelin://acme/git/ref/web:refs/heads/main#commit-deadbeef/check-build";
        let run_a = deterministic_uuid("run:ev-A");
        let run_b = deterministic_uuid("run:ev-B");
        // Same run + same subject → SAME id (a redelivery dedups/absorbs).
        assert_eq!(
            check_event_id(&run_a, subject),
            check_event_id(&run_a, subject),
            "same run + subject is stable (redelivery dedups)"
        );
        // DISTINCT runs + same subject → DISTINCT ids (no collision across runs).
        assert_ne!(
            check_event_id(&run_a, subject),
            check_event_id(&run_b, subject),
            "distinct runs on the same (repo, commit, context) must NOT collide (H3)"
        );
    }

    /// **deterministic_uuid is a valid uuid shape + stable per seed + distinct across seeds.**
    #[test]
    fn deterministic_uuid_is_stable_and_shaped() {
        let a = deterministic_uuid("run:ev-1");
        assert_eq!(a, deterministic_uuid("run:ev-1"), "stable per seed");
        assert_ne!(a, deterministic_uuid("run:ev-2"), "distinct across seeds");
        assert_eq!(a.len(), 36, "canonical uuid length");
        assert_eq!(a.matches('-').count(), 4, "canonical uuid dashes");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    }
}

/// **The production [`GitConfigReader`] adapter over the myelin-git durable read backend.** Wraps a
/// `myelin_git::durable::DurableStore` (the on-disk repo store): `read_repo_file` opens the repo at
/// `(tenant, region, repo)` and reads the blob at `oid`:`path` via `read_blob_at_path` (the SAME
/// nested tree/blob navigation the repo-browser uses — never a reimplemented walk). A missing path /
/// a path that resolves to a directory reads as `Ok(None)` (a clean skip); a backend error is
/// `Err(GitReadError)` (fail-closed).
///
/// **Named cross-service floor:** in a split deployment the git repos live in the GIT service's
/// storage, not the dispatch service's — reading blobs cross-service is a git-service read API
/// (in-process `ReadBackend` or an RPC). This adapter is the in-cell/shared-storage form; the
/// cross-cell git-read hop is the deploy follow-on.
pub struct DurableGitConfigReader<P: myelin_git::gix_backend::RepoPathResolver + Send + Sync> {
    store: myelin_git::durable::DurableGitStore<P>,
}

impl<P: myelin_git::gix_backend::RepoPathResolver + Send + Sync> DurableGitConfigReader<P> {
    /// Build the adapter over a durable repo store.
    pub fn new(store: myelin_git::durable::DurableGitStore<P>) -> DurableGitConfigReader<P> {
        DurableGitConfigReader { store }
    }
}

impl<P: myelin_git::gix_backend::RepoPathResolver + Send + Sync> GitConfigReader
    for DurableGitConfigReader<P>
{
    fn read_repo_file(
        &self,
        tenant: &str,
        region: &str,
        repo: &str,
        oid: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, GitReadError> {
        self.read_repo_file_bounded(tenant, region, repo, oid, path, usize::MAX)
    }

    fn read_repo_file_bounded(
        &self, tenant: &str, region: &str, repo: &str, oid: &str, path: &str, maximum_bytes: usize,
    ) -> Result<Option<Vec<u8>>, GitReadError> {
        let loc = myelin_git::core::RepoLoc::new(tenant, region, repo);
        if !self.store.repo_exists(&loc) {
            return Err(GitReadError::Unavailable(format!(
                "repository {repo} is unavailable for tenant={tenant} region={region}"
            )));
        }
        let git = self
            .store
            .open_repo(&loc)
            .map_err(|e| GitReadError::Unavailable(format!("open {repo}: {e}")))?;
        let oid = myelin_git::core::Oid::new(oid);
        match git
            .read_blob_at_commit_oid_bounded(&oid, path, maximum_bytes)
            .map_err(|e| GitReadError::Unavailable(format!("read {path}@{}: {e}", oid.as_str())))?
        {
            myelin_git::durable::BlobPathLookup::Found { bytes, .. } => Ok(Some(bytes)),
            myelin_git::durable::BlobPathLookup::TooLarge { size, maximum } => Err(GitReadError::Invalid(format!("{path}@{} is {size} bytes, above the {maximum}-byte config limit", oid.as_str()))),
            // Absent or a directory at that name → no config file (a clean skip).
            myelin_git::durable::BlobPathLookup::Missing
            | myelin_git::durable::BlobPathLookup::IsDir => Ok(None),
        }
    }
}
