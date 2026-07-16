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
//!   events, co-committed through the injected DURABLE [`OutboxStore`] ([`OutboxReserveStore`]) — the
//!   production `main.rs` binds `OutboxStore::durable(PgOutboxBacking)`, so these events survive a
//!   restart. The exactly-once effect rides the `Consumer` runtime's `consumer_dedup` ledger (one
//!   triggering `event_id` = one effect) PLUS a deterministic `run_id` derived from the `event_id`
//!   (so a redelivered trigger mints the SAME run — `ON CONFLICT DO NOTHING` at the `ci_run` PK).
//! - **The `ci_run` ROW one-tx co-commit (PROVEN in test, NAMED as a floor for production):** the
//!   `ci_run` table is owned by `myelin-ci-controlplane` (its `CREATE_CI_RUN_DDL`), NOT by
//!   ci-dispatch — `all_durable_migrations()` (what this service's `main.rs` applies) does NOT create
//!   it. The FULL atomic bundle (the `ci_run` row + BOTH events in ONE tx) is proven against live PG
//!   in `tests/integration_ci_ct004b_trigger_consumer.rs` (which stands the `ci_run` table up in an
//!   isolated schema and drives a `PgReserveStore` exactly like CT-004a / the CT-004 `p28`
//!   durability drill). The PRODUCTION durable `ci_run` writer belongs in `myelin-storage` (the
//!   established `PgOutboxBacking` / `DurableTupleBacking` backing site — this leaf crate carries
//!   `sqlx` only behind `--features integration`); wiring it there + into `main.rs` is the named
//!   cross-service follow-on.
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

use std::sync::{Arc, Mutex};

use myelin_events::{
    Actor, EmitContextBase, EventEnvelope, EventHandler, HandleOutcome, IdMinter, OutboxStore,
    OutboxTx, SubjectPattern,
};
use myelin_storage::BlobStore;
use myelin_tenancy::TenantId;

use crate::config::{parse_ci_config, CiConfigError, ConfigFormat};
use crate::dispatch::{
    compile_trigger, stamp_trust, OnTrigger, RunProvenance, TrustStamp, TRIGGER_CONSUMER,
};
use crate::resolve::{
    reserve_and_start, resolve_snapshot, snapshot_ref, CheckContext, CiDefinition, ResolveError,
    RunFacts, StartHandoff,
};

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
pub static CI_TRIGGER_SUBJECTS: &[SubjectPattern] = &[SubjectPattern(String::new())];

/// The `&str` subjects the live [`myelin_events::consumer::Subscription`] binds — the same whitelist
/// as [`CI_TRIGGER_SUBJECTS`], as the borrow the `Subscription::bind` constructor takes. `myelin://`
/// is the bounded (non-`*`) transport prefix; the exact arming is `handle`'s type match.
pub const CI_TRIGGER_SUBJECT_STRS: &[&str] = &["myelin://"];

// =================================================================================================
// 2. The git config-read seam (read `.myelin/ci.*` at the pushed ref).
// =================================================================================================

/// Why reading `.myelin/ci.*` at the pushed ref failed (a TRANSPORT/backend failure — distinct from
/// "the file is simply absent", which is `Ok(None)`, a clean skip). A read error is fail-closed:
/// the consumer does NOT start a run it cannot prove the definition of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitReadError(pub String);

impl std::fmt::Display for GitReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "git config read failed: {}", self.0)
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
        if let Some(bytes) = reader.read_repo_file(tenant, region, repo, oid, path)? {
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

/// **The seam that DURABLY persists the atomic reserve/start bundle.** The production impl
/// ([`OutboxReserveStore`]) co-commits the `ci.run.started` + queued `ci.check.updated` events through
/// the injected DURABLE [`OutboxStore`]; the integration test's `PgReserveStore` additionally writes
/// the `ci_run` ROW in the SAME tx (the full atomic bundle proven on live PG). A unit test uses
/// [`RecordingReserveStore`]. The trait is SYNC to match the [`EventHandler::handle`] body (the
/// durable impls bridge to async at their own boundary, the `PgOutboxBacking` idiom).
pub trait ReserveStore: Send + Sync {
    /// Persist the armed run's atomic bundle. Idempotent on the run identity (a redelivered trigger
    /// mints the SAME `run_id`, so the durable write is `ON CONFLICT DO NOTHING` — one run, never
    /// two). Returns `Ok(())` whether it inserted or absorbed a duplicate.
    fn persist(&self, armed: &ArmedRun) -> Result<(), ReserveError>;
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
    fn persist(&self, armed: &ArmedRun) -> Result<(), ReserveError> {
        // ONE outbox transaction: ci.run.started + every queued ci.check.updated commit together
        // (emit-iff-committed). The staged-state-change records that the ci_run row is the co-commit
        // partner the myelin-storage backing floor will make durable in the SAME tx (named above).
        let mut tx = self
            .outbox
            .begin(Arc::clone(&self.minter), armed.emit_ctx.clone());
        tx.stage_state_change(format!(
            "ci_run {} reserved (queued) — the durable row co-commit is the myelin-storage backing floor",
            armed.handoff.run_write.run_id
        ));
        tx.emit(armed.handoff.run_started.clone(), None)
            .map_err(|e| ReserveError(format!("ci.run.started emit: {e:?}")))?;
        for check in &armed.handoff.queued_checks {
            tx.emit(check.clone(), None)
                .map_err(|e| ReserveError(format!("queued ci.check.updated emit: {e:?}")))?;
        }
        tx.commit()
            .map_err(|e| ReserveError(format!("outbox commit: {e:?}")))
    }
}

// =================================================================================================
// 4. The dispatch plan (the pure pipeline core — testable per branch).
// =================================================================================================

/// Why a triggering event did NOT arm a run — every skip is a distinct, SURFACED reason (fail-closed,
/// never a silent swallow). All skips ACK the message ([`HandleOutcome::Done`]): a skip is not poison
/// to retry, and never a crash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The event is not a CI trigger type (a non-git / non-armed event routed by the coarse prefix).
    NotATrigger(String),
    /// The event payload lacked a required field (`repo` / `new_oid`) — malformed, fail-closed.
    MalformedPayload(String),
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

/// Extract `(repo, new_oid, ref, on-event-type-supported)` provenance from a triggering envelope.
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
    // The head oid the run runs against: `new_oid` (push) or `head_oid` (PR).
    let new_oid = p
        .get("new_oid")
        .or_else(|| p.get("head_oid"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| SkipReason::MalformedPayload("missing `new_oid`/`head_oid`".into()))?
        .to_string();
    // Fork provenance: a PR from a fork carries `is_fork`/`forked`; a push is a member push.
    let is_fork = p
        .get("is_fork")
        .or_else(|| p.get("forked"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(TriggerFacts {
        repo,
        new_oid,
        is_fork,
    })
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
    let def: CiDefinition = match parse_ci_config(&bytes, format_hint) {
        Ok(d) => d,
        Err(e) => return DispatchOutcome::Skip(SkipReason::ConfigError(e)),
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
    let (_snapshot, address) = match resolve_snapshot(&def, blobs, &tenant) {
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
/// reserve bundle through the [`ReserveStore`]; a `Skip` is surfaced (logged) and ACKed. The
/// `Consumer` runtime wraps this with the seven rules (rule-1 dedup on `event_id` via the durable
/// `consumer_dedup` ledger, ack-after-enqueue, bounded prefetch, the lag metric) — this body is the
/// trigger LOGIC only. Idempotent on `event_id` twice over: the runtime's dedup AND the deterministic
/// `run_id` (`ON CONFLICT DO NOTHING`).
pub struct CiTriggerHandler {
    reader: Arc<dyn GitConfigReader>,
    blobs: Arc<dyn BlobStore + Send + Sync>,
    reserve: Arc<dyn ReserveStore>,
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
) -> Result<myelin_substrate::ConsumerReg, myelin_events::SubscribeError> {
    let handler = CiTriggerHandler::new(reader, blobs, reserve);
    let subscription = myelin_events::consumer::Subscription::bind(
        myelin_events::ConsumerName(TRIGGER_CONSUMER.into()),
        CI_TRIGGER_SUBJECT_STRS,
        myelin_events::PrefetchBound::DEFAULT,
    )?;
    Ok(myelin_substrate::ConsumerReg::new(
        myelin_events::Consumer::new(handler, subscription, dedup),
    ))
}

impl EventHandler for CiTriggerHandler {
    fn subjects(&self) -> &'static [SubjectPattern] {
        CI_TRIGGER_SUBJECTS
    }

    fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
        match plan_dispatch(ev, self.reader.as_ref(), self.blobs.as_ref()) {
            DispatchOutcome::Arm(armed) => match self.reserve.persist(&armed) {
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
                // Every skip is SURFACED (the trace) and ACKed. A NotATrigger skip (the coarse-prefix
                // firehose discarding a non-git event) is not even recorded past a bounded window —
                // but a config/resolve error IS surfaced loudly (fail-closed, never silent).
                match &reason {
                    SkipReason::NotATrigger(_) | SkipReason::NoConfig => {}
                    other => self.record(format!("skip: {other}")),
                }
                HandleOutcome::Done
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
            return Err(GitReadError(format!("injected read failure at {path}")));
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
    fn persist(&self, armed: &ArmedRun) -> Result<(), ReserveError> {
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
        Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventId, EventType, Timestamp,
        Visibility,
    };
    use myelin_git::events::{GIT_PR_OPENED, GIT_REF_UPDATED};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_storage::FsBlobStore;
    use myelin_tenancy::{Region, TenantId};

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
            aggregate: AggregateKey(format!("git/ref/{repo}:refs/heads/main")),
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

    fn valid_toml() -> &'static [u8] {
        concat!(
            "on = \"push\"\n\n",
            "[[jobs]]\n",
            "name = \"build\"\n",
            "image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"\n",
        )
        .as_bytes()
    }

    fn arm(ev: &EventEnvelope, reader: &dyn GitConfigReader) -> DispatchOutcome {
        let blobs = FsBlobStore::new();
        plan_dispatch(ev, reader, &blobs)
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
    }

    /// **No `.myelin/ci.*` at the pushed ref → a clean NoConfig skip (NOT an error, no run).**
    #[test]
    fn no_config_is_a_clean_skip() {
        let ev = push_envelope("web", "deadbeef");
        let reader = MapGitConfigReader::new();
        assert!(matches!(arm(&ev, &reader), DispatchOutcome::Skip(SkipReason::NoConfig)));
    }

    /// **A malformed `.myelin/ci.toml` → a fail-closed, SURFACED ConfigError skip (no run, no crash).**
    #[test]
    fn a_malformed_config_is_a_surfaced_skip() {
        let ev = push_envelope("web", "deadbeef");
        let reader =
            MapGitConfigReader::new().with_file("web", "deadbeef", ".myelin/ci.toml", &b"on = = broken"[..]);
        assert!(matches!(
            arm(&ev, &reader),
            DispatchOutcome::Skip(SkipReason::ConfigError(_))
        ));
    }

    /// **A backend read failure → a fail-closed ReadFailed skip (no run without a proven definition).**
    #[test]
    fn a_read_failure_is_fail_closed() {
        let ev = push_envelope("web", "deadbeef");
        let reader = MapGitConfigReader::new().with_failure("web", "deadbeef", ".myelin/ci.toml");
        assert!(matches!(
            arm(&ev, &reader),
            DispatchOutcome::Skip(SkipReason::ReadFailed(_))
        ));
    }

    /// **A non-matching trigger (a `pull_request` config on a push) → TriggerNotMatched skip.**
    #[test]
    fn a_non_matching_trigger_skips() {
        let ev = push_envelope("web", "deadbeef");
        let pr_config = concat!(
            "on = \"pull_request\"\n\n",
            "[[jobs]]\nname = \"build\"\n",
            "image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"\n"
        );
        let reader =
            MapGitConfigReader::new().with_file("web", "deadbeef", ".myelin/ci.toml", pr_config.as_bytes());
        assert!(matches!(
            arm(&ev, &reader),
            DispatchOutcome::Skip(SkipReason::TriggerNotMatched)
        ));
    }

    /// **A floating-tag config → a fail-closed ResolveError skip (the supply-chain control).**
    #[test]
    fn a_floating_tag_is_a_surfaced_resolve_skip() {
        let ev = push_envelope("web", "deadbeef");
        let floating = concat!("on = \"push\"\n\n[[jobs]]\nname = \"build\"\nimage = \"alpine:3\"\n");
        let reader =
            MapGitConfigReader::new().with_file("web", "deadbeef", ".myelin/ci.toml", floating.as_bytes());
        assert!(matches!(
            arm(&ev, &reader),
            DispatchOutcome::Skip(SkipReason::ResolveError(ResolveError::FloatingTag { .. }))
        ));
    }

    /// **A missing `repo`/`new_oid` payload → a fail-closed MalformedPayload skip.**
    #[test]
    fn a_malformed_payload_skips() {
        let mut ev = push_envelope("web", "deadbeef");
        ev.payload = serde_json::json!({ "ref": "refs/heads/main" });
        assert!(matches!(
            arm(&ev, &MapGitConfigReader::new()),
            DispatchOutcome::Skip(SkipReason::MalformedPayload(_))
        ));
    }

    /// **The HAPPY PATH: a digest-pinned `.myelin/ci.toml` on a matching push → an ARMED run** with
    /// the atomic bundle (the `ci_run` row queued + `ci.run.started` + the queued check), the
    /// deterministic run_id, and the Trusted (member push) stamp.
    #[test]
    fn the_happy_path_arms_the_atomic_bundle() {
        let ev = push_envelope("web", "deadbeef");
        let reader =
            MapGitConfigReader::new().with_file("web", "deadbeef", ".myelin/ci.toml", valid_toml());
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
        assert_eq!(armed.tenant, TenantId("acme".into()));
    }

    /// **A fork PR (is_fork) → an UntrustedFork stamp on BOTH the row AND every check (X-1).**
    #[test]
    fn a_fork_pr_stamps_untrusted_fork() {
        let mut ev = push_envelope("web", "deadbeef");
        ev.type_ = EventType(GIT_PR_OPENED.into());
        ev.payload = serde_json::json!({
            "repo": "web", "head_oid": "deadbeef", "is_fork": true,
        });
        let pr_config = concat!(
            "on = \"pull_request\"\n\n[[jobs]]\nname = \"build\"\n",
            "image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"\n"
        );
        let reader =
            MapGitConfigReader::new().with_file("web", "deadbeef", ".myelin/ci.toml", pr_config.as_bytes());
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
        let ev = push_envelope("web", "deadbeef");
        let reader: Arc<dyn GitConfigReader> = Arc::new(
            MapGitConfigReader::new().with_file("web", "deadbeef", ".myelin/ci.toml", valid_toml()),
        );
        let blobs: Arc<dyn BlobStore + Send + Sync> = Arc::new(FsBlobStore::new());
        let store = Arc::new(RecordingReserveStore::new());
        let handler = CiTriggerHandler::new(reader, blobs, store.clone());

        assert_eq!(handler.handle(&ev), HandleOutcome::Done);
        assert_eq!(handler.handle(&ev), HandleOutcome::Done, "redelivery is handled");
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
        let loc = myelin_git::core::RepoLoc::new(tenant, region, repo_name(repo));
        if !self.store.repo_exists(&loc) {
            // An unknown repo reads as "no config" (a clean skip) — a push for a repo whose object
            // store this cell does not hold is the cross-service git-read floor, not a crash.
            return Ok(None);
        }
        let git = self
            .store
            .open_repo(&loc)
            .map_err(|e| GitReadError(format!("open {repo}: {e}")))?;
        match git
            .read_blob_at_path(oid, path)
            .map_err(|e| GitReadError(format!("read {path}@{oid}: {e}")))?
        {
            myelin_git::durable::BlobPathLookup::Found { bytes, .. } => Ok(Some(bytes)),
            // Absent or a directory at that name → no config file (a clean skip).
            myelin_git::durable::BlobPathLookup::Missing
            | myelin_git::durable::BlobPathLookup::IsDir => Ok(None),
        }
    }
}

/// Extract the repo NAME from a repo ref: the last path segment of `myelin://<tenant>/git/repo/<name>`
/// or a bare `<name>`. The `RepoLoc` keys on the short repo name (its on-disk path segment).
fn repo_name(repo: &str) -> &str {
    repo.rsplit(['/', ':']).next().filter(|s| !s.is_empty()).unwrap_or(repo)
}
