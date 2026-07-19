//! **CI Trigger & Dispatch: definition resolution → content-addressed snapshot + the reserve/start
//! handoff (CI-P11 / P-354, M4).**
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §1 steps 4–5 (definition resolution → CAS snapshot, then the reserve+start handoff) + §7.4 (the
//! config grammar: a declarative JSON-schema'd core, the bounded `QueryAst` for expressions, and the
//! **sandboxed dynamic-generation escape hatch** for programmatic fan-out — "run code to compute the
//! pipeline" inherits the SAME sandbox isolation as any other untrusted code, NO privileged
//! config-eval path); `03-events-contracts-and-glue.md` §1.2 (`ci.run.started` carries
//! `trust_tier` / `trigger_kind` / the CAS snapshot ref) + §1.1 / §4 (the first `ci.check.updated`
//! `{state: queued}` per context, the X-1 seam) + §3 (every `ci.*` event drafted into the outbox in
//! the SAME tx as the state change — `OutboxTx::emit`, contract 2.2, no `publish_now`).
//!
//! **Contracts consumed (implemented to the FROZEN shapes; never re-defined):**
//! - **11.2** [`myelin_storage::BlobStore`] — the resolved DAG is written as a T2 content-addressed
//!   CAS blob (BLAKE3 address = the run's reproducible, auditable definition). REUSED, not a new
//!   store (EI-01 §7).
//! - **9.1** [`myelin_flow::DurableExecutor::start`] / [`myelin_flow::StartSpec`] — the reserve+start
//!   handoff hands `StartSpec{ wf_type: "ci.pipeline", input: [snapshot_ref], .. }` to the durable
//!   executor (the workflow BODY is CI-P15 / `myelin_flow::ci_pipeline`; here dispatch only starts
//!   it). The `idem_key` makes a re-delivered trigger ONE run, never two.
//! - **2.2** [`myelin_events::OutboxTx::emit`] — the ONLY sanctioned emit path: this module BUILDS
//!   the `ci.run.started` + the queued-`ci.check.updated` [`EventDraft`]s; the live consumer emits
//!   them via the outbox in the SAME tx as the `ci_run` write (no `publish_now`, the `no-raw-publish`
//!   lint). The drafts are constructed here so the atomic bundle is one testable unit.
//!
//! ## The two halves this module owns
//!
//! ### 1. Definition resolution → the content-addressed snapshot (the supply-chain floor)
//! [`resolve_snapshot`] reads a parsed `.myelin/ci.*` [`CiDefinition`], validates it (a non-empty,
//! acyclic job DAG), **expands the matrix DETERMINISTICALLY** (lexicographic over the sorted
//! axis-key→value cross-product, so the snapshot is byte-identical for the same input — the
//! reproducibility floor, VISION §3), and **resolves every image reference TO A DIGEST, FAIL-CLOSED
//! on a floating tag** (`alpine:3` is REJECTED; `alpine@sha256:<hex>` passes — the
//! poisoned-pipeline-execution supply-chain control, EI-01 §3 / arch 02 §5.3). It then serialises the
//! resolved DAG to **canonical JSON** and writes it as a CAS blob (T2, contract 11.2). The returned
//! [`ResolvedSnapshot`] + its [`ContentHash`] address ARE the run's reproducible definition — IDENTICAL
//! to the `myelin ci plan` output (shift-left: `validate`/`plan` are pure, no runner spend).
//!
//! The digest-pin check REUSES the already-frozen [`myelin_ci_sandbox::ImageRef::digest_pinned`]
//! rule — NOT a second digest grammar. **The actual tag→digest registry resolution is CI-P23**
//! (`digest-pin-or-fail-closed` + sigstore verify, supply-chain trust CI-D4); CI-P11 enforces the
//! fail-closed half at PLAN time (an un-digested reference never reaches a snapshot — 0 floating tags
//! pinned). State this floor.
//!
//! ### 2. The reserve+start handoff (atomic, one tx)
//! [`reserve_and_start`] takes the snapshot ref + the run facts (the trust stamp from CI-P10's
//! [`crate::dispatch::stamp_trust`], the trigger kind, the per-context check seam) and produces a
//! [`StartHandoff`]: the [`StartSpec`] for the `ci.pipeline` workflow (9.1) AND the atomic bundle the
//! live consumer writes in ONE transaction — the [`CiRunWrite`] row + the [`EventDraft`]s for
//! `ci.run.started` and the first `ci.check.updated{state: queued}` per context (via the outbox,
//! 2.2). The atomicity invariant ([`StartHandoff::is_atomic_bundle`]) is the GATE: a row with NO
//! queued check, or a `ci.run.started` with NO ci_run row, is a partial run — REFUSED by construction
//! (the bundle is one value the consumer commits together).
//!
//! ## FLOOR named (the prompt DoD)
//! - **The sandboxed dynamic-generation escape hatch** (arch 02 §7.4): [`CiDefinition`] models a
//!   generator as [`JobDef::kind`] = [`JobKind::Generate`], but [`resolve_snapshot`] currently
//!   refuses it. The shared execution contract cannot ingest and re-resolve emitted fragments yet,
//!   so persisting an apparently executable generator plan would be dishonest. Generator execution,
//!   fragment ingestion, and re-dispatch remain the named CI-P15 follow-on; there is no privileged
//!   config-evaluation path in the meantime.
//! - **The tag→digest registry resolution** (the real lookup + sigstore verify): CI-P23 (CI-D4).
//!   CI-P11 enforces the fail-closed PLAN-time half only.
//!
//! ## DB-free by default
//! `cargo build`/`cargo test --workspace` stay DB-free: the resolver uses the in-memory
//! [`myelin_storage::FsBlobStore`]-compatible `BlobStore` trait (the unit tests drive an in-memory
//! blob store), and the reserve/start uses the in-memory [`FlowExecutor`](myelin_flow::FlowExecutor).
//! The CAS-snapshot round-trip against the LIVE dev-stack object store is the named integration test.

use std::collections::{BTreeMap, BTreeSet};

use myelin_ci_sandbox::ImageRef;
use myelin_events::{AggregateKey, EventId};
use myelin_events::{ArtifactRef, DataRole, EventDraft, EventType, Visibility};
use myelin_storage::{BlobStore, ContentHash};
use myelin_tenancy::TenantId;

pub use myelin_ci_controlplane::{
    CiExecutionRequestV1, ResolvedJobV1, ResolvedJobV2, ResolvedRunPlanV1, ResolvedRunPlanV2,
    VersionedResolvedRunPlan as VersionedResolvedSnapshot,
};
/// Legacy V1 job alias retained for source compatibility.
pub type ResolvedJob = ResolvedJobV1;
/// Legacy V1 snapshot alias retained for source compatibility.
pub type ResolvedSnapshot = ResolvedRunPlanV1;

/// Dispatch compatibility helpers for the shared resolved-plan wire.
pub trait ResolvedSnapshotExt {
    /// Whether the authored plan contains a dynamic generator node.
    fn has_dynamic_generation(&self) -> bool;
}

impl ResolvedSnapshotExt for ResolvedSnapshot {
    fn has_dynamic_generation(&self) -> bool {
        self.jobs.iter().any(|job| job.is_generator)
    }
}

impl ResolvedSnapshotExt for VersionedResolvedSnapshot {
    fn has_dynamic_generation(&self) -> bool {
        match self {
            VersionedResolvedSnapshot::V1(plan) => plan.jobs.iter().any(|job| job.is_generator),
            VersionedResolvedSnapshot::V2(plan) => plan.jobs.iter().any(|job| job.is_generator),
        }
    }
}

use myelin_ci_sandbox::events::CI_RUN_STARTED;

use crate::dispatch::{OnTrigger, TrustStamp};

// =================================================================================================
// 1. The parsed `.myelin/ci.*` definition (the config-as-code core, arch 02 §7.4).
// =================================================================================================

/// The kind of a job in a CI definition (arch 02 §7.4). Two kinds matter at resolution:
/// - [`JobKind::Normal`] — an ordinary build/test/deploy step running a digest-pinned image.
/// - [`JobKind::Generate`] — the **sandboxed dynamic-generation escape hatch**: a job that *emits* a
///   pipeline fragment for genuinely programmatic fan-out. It is a NORMAL job in every other respect
///   (digest-pinned image, runs on the CI-P3 runner, the SAME sandbox as any untrusted code — NO
///   privileged config-eval path). The type reserves the authored contract, while
///   [`resolve_snapshot`] refuses generator plans until CI-P15 supplies sandbox execution, fragment
///   ingestion, and re-dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    /// An ordinary build/test/deploy job.
    Normal,
    /// The dynamic-generation escape hatch: a job that emits a pipeline fragment (programmatic
    /// fan-out), running in the same sandbox as any untrusted code (arch 02 §7.4).
    Generate,
}

/// One job in a parsed CI definition (arch 02 §7.4). `image` is the RAW reference as authored — it
/// may be a floating tag (`alpine:3`) at parse time; [`resolve_snapshot`] is where it is resolved to
/// a digest fail-closed. `needs` is the DAG edge set (the names this job depends on).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobDef {
    /// The job name (unique within the definition — the DAG node id).
    pub name: String,
    /// The RAW image reference as authored (may be a floating tag — resolved fail-closed later).
    pub image: String,
    /// Exact executable argv. Dispatch never infers a shell or fallback command.
    pub command: Vec<String>,
    /// The names this job depends on (the DAG edges — every name must exist in the definition).
    pub needs: Vec<String>,
    /// The job kind — [`JobKind::Generate`] marks the sandboxed dynamic-generation escape hatch.
    pub kind: JobKind,
    /// The optional matrix axes (axis key → the ordered list of values). Empty == a single job
    /// instance. The cross-product is expanded DETERMINISTICALLY in [`resolve_snapshot`].
    pub matrix: BTreeMap<String, Vec<String>>,
}

impl JobDef {
    /// A normal (non-matrix, non-generate) job over `image`.
    pub fn normal(
        name: impl Into<String>,
        image: impl Into<String>,
        command: impl IntoIterator<Item = impl Into<String>>,
    ) -> JobDef {
        JobDef {
            name: name.into(),
            image: image.into(),
            command: command.into_iter().map(Into::into).collect(),
            needs: Vec::new(),
            kind: JobKind::Normal,
            matrix: BTreeMap::new(),
        }
    }

    /// Mark this job's `needs` (the DAG edges).
    pub fn with_needs(mut self, needs: impl IntoIterator<Item = impl Into<String>>) -> JobDef {
        self.needs = needs.into_iter().map(Into::into).collect();
        self
    }

    /// Mark a matrix axis on this job (axis key → ordered values).
    pub fn with_matrix(mut self, key: impl Into<String>, values: Vec<String>) -> JobDef {
        self.matrix.insert(key.into(), values);
        self
    }

    /// Mark this job as the dynamic-generation escape hatch ([`JobKind::Generate`]).
    pub fn as_generator(mut self) -> JobDef {
        self.kind = JobKind::Generate;
        self
    }
}

/// A parsed `.myelin/ci.*` definition (arch 02 §7.4 config-as-code core). The triggering event
/// (`on:`) compiles to the ONE `QueryAst` (CI-P10's [`compile_trigger`](crate::dispatch::compile_trigger));
/// the jobs are the DAG [`resolve_snapshot`] content-addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiDefinition {
    /// The armed trigger (the `on:` block — compiled to the one `QueryAst`, CI-P10).
    pub on: OnTrigger,
    /// The jobs (the DAG). MUST be non-empty + acyclic — validated in [`resolve_snapshot`].
    pub jobs: Vec<JobDef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiPlanContract {
    V1,
    V2(CiExecutionRequestV1),
}

/// A parsed definition paired with its authored wire contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionedCiDefinition {
    /// Versioned authored request contract. V2 still carries no server launch authority.
    pub contract: CiPlanContract,
    /// The armed trigger.
    pub on: OnTrigger,
    /// The authored DAG.
    pub jobs: Vec<JobDef>,
}

impl VersionedCiDefinition {
    pub fn v1(on: OnTrigger, jobs: Vec<JobDef>) -> Self {
        Self { contract: CiPlanContract::V1, on, jobs }
    }

    pub fn v2(on: OnTrigger, execution: CiExecutionRequestV1, jobs: Vec<JobDef>) -> Self {
        Self { contract: CiPlanContract::V2(execution), on, jobs }
    }
}

// =================================================================================================
// 2. Resolution → the content-addressed snapshot.
// =================================================================================================

/// Why a definition fails to resolve (fail-closed — arch 02 §7.4 / EI-01 §3). LOUD, never silently
/// coerced into a degraded snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// The definition has no jobs — an empty pipeline is rejected (nothing to run).
    EmptyDefinition,
    /// A job's image is NOT digest-pinned — a FLOATING TAG is rejected fail-closed (the
    /// supply-chain control; the real tag→digest registry resolution is CI-P23). Carries the
    /// offending job + reference for a self-describing error.
    FloatingTag {
        /// The job whose image was a floating tag.
        job: String,
        /// The un-digested reference that was refused.
        reference: String,
    },
    /// Two jobs share a name — the DAG node ids must be unique.
    DuplicateJob(String),
    /// A `needs` edge names a job that does not exist in the definition (a dangling DAG edge).
    UnknownNeed {
        /// The job carrying the dangling edge.
        job: String,
        /// The non-existent name it depends on.
        need: String,
    },
    /// A job names itself as a dependency.
    SelfNeed(String),
    /// A job repeats the same dependency instead of declaring a set.
    DuplicateNeed { job: String, need: String },
    /// The job DAG has a cycle (it is not a DAG) — a run could never make progress.
    Cyclic,
    /// The CAS blob write failed (the snapshot could not be content-addressed) — surfaced, never
    /// swallowed (the snapshot is the run's definition; no snapshot ⇒ no start).
    BlobWrite(myelin_storage::BlobError),
    /// **The matrix cross-product exceeds [`MAX_TOTAL_MATRIX_INSTANCES`] (peer-review finding
    /// 2026-07-16 #10 — resource-limit fail-closed).** `expand_matrix` materializes the full axis
    /// cross-product; an unbounded config (e.g. ~8 axes × ~10 values) would OOM the dispatch consumer
    /// on a SINGLE untrusted push. The instance count is computed with SATURATING arithmetic BEFORE any
    /// expansion, so an astronomical product is refused without allocating it. Carries the offending
    /// count + the cap for a self-describing error.
    MatrixTooLarge {
        /// The total (or running) resolved-instance count that tripped the cap (saturating).
        count: usize,
        /// The enforced ceiling ([`MAX_TOTAL_MATRIX_INSTANCES`]).
        cap: usize,
    },
    /// The resolved versioned plan violated the shared execution-boundary contract.
    InvalidPlan(String),
    /// Two expanded jobs resolved to the same concrete machine-token node name.
    ConcreteNameCollision(String),
}

/// **The hard ceiling on the total number of matrix-expanded job instances a single CI definition may
/// resolve to (peer-review finding #10 — DoS floor).** A push whose config would fan out past this is
/// refused fail-closed BEFORE the cross-product is materialized (the count is computed with saturating
/// multiplication). 1024 is far above any legitimate pipeline (the workspace's largest real matrix is a
/// few dozen instances) and far below anything that pressures the consumer's memory.
pub const MAX_TOTAL_MATRIX_INSTANCES: usize = 1024;

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::EmptyDefinition => {
                write!(f, "the CI definition has no jobs — nothing to run")
            }
            ResolveError::FloatingTag { job, reference } => write!(
                f,
                "job `{job}`: image `{reference}` is a FLOATING TAG — rejected fail-closed (resolve \
                 every reference to a digest `@<algo>:<hex>`; the tag→digest registry resolution is \
                 CI-P23). 0 un-digested references reach a snapshot."
            ),
            ResolveError::DuplicateJob(name) => {
                write!(f, "duplicate job name `{name}` — DAG node ids must be unique")
            }
            ResolveError::UnknownNeed { job, need } => write!(
                f,
                "job `{job}` needs `{need}`, which is not a job in the definition (dangling DAG edge)"
            ),
            ResolveError::SelfNeed(job) => write!(f, "job `{job}` depends on itself"),
            ResolveError::DuplicateNeed { job, need } => write!(f, "job `{job}` repeats dependency `{need}`"),
            ResolveError::Cyclic => write!(f, "the job DAG has a cycle — it is not a DAG"),
            ResolveError::BlobWrite(e) => {
                write!(f, "the CAS snapshot blob write failed: {e} (no snapshot ⇒ no start)")
            }
            ResolveError::MatrixTooLarge { count, cap } => write!(
                f,
                "the matrix cross-product resolves to {count} job instances, over the {cap} ceiling — \
                 rejected fail-closed (a push cannot fan out unbounded; raise the config's matrix or \
                 split the pipeline)"
            ),
            ResolveError::InvalidPlan(detail) => write!(f, "invalid resolved CI plan: {detail}"),
            ResolveError::ConcreteNameCollision(name) => write!(f, "matrix expansion produced duplicate concrete job name `{name}`"),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Render a deterministic machine-token instance name for a matrix assignment. The full authored
/// job name and sorted assignment are length-framed into a BLAKE3 identity, retaining a bounded
/// human-readable job prefix without allowing distinct assignments to alias.
fn instance_name(job: &str, assignment: &BTreeMap<String, String>) -> String {
    myelin_ci_controlplane::derive_concrete_job_name(job, assignment)
}

/// Expand one job's matrix into its deterministic cross-product of axis assignments. The axes are
/// taken in SORTED key order (a `BTreeMap`), and each axis's values in their authored order; the
/// cross-product is built so the result list is byte-identical for the same input. A job with no
/// matrix yields exactly one empty assignment.
fn expand_matrix(matrix: &BTreeMap<String, Vec<String>>) -> Vec<BTreeMap<String, String>> {
    let mut out: Vec<BTreeMap<String, String>> = vec![BTreeMap::new()];
    // `BTreeMap` gives the axes in sorted key order → the cross-product nesting is deterministic.
    for (axis, values) in matrix {
        let mut next = Vec::with_capacity(out.len() * values.len().max(1));
        for base in &out {
            for v in values {
                let mut a = base.clone();
                a.insert(axis.clone(), v.clone());
                next.push(a);
            }
        }
        if !values.is_empty() {
            out = next;
        }
    }
    out
}

/// Validate the job DAG is acyclic + every `needs` edge resolves (Kahn's algorithm — a topological
/// order exists iff the graph is a DAG). Returns the first structural error.
fn validate_dag(jobs: &[JobDef]) -> Result<(), ResolveError> {
    let names: BTreeSet<&str> = jobs.iter().map(|j| j.name.as_str()).collect();
    // Every `needs` edge must point at an existing job.
    for j in jobs {
        let mut seen_needs = BTreeSet::new();
        for need in &j.needs {
            if need == &j.name { return Err(ResolveError::SelfNeed(j.name.clone())); }
            if !seen_needs.insert(need.as_str()) {
                return Err(ResolveError::DuplicateNeed { job: j.name.clone(), need: need.clone() });
            }
            if !names.contains(need.as_str()) {
                return Err(ResolveError::UnknownNeed {
                    job: j.name.clone(),
                    need: need.clone(),
                });
            }
        }
    }
    // Kahn: repeatedly remove a node with no unresolved in-edge; if any remain, there is a cycle.
    let mut indeg: BTreeMap<&str, usize> = jobs
        .iter()
        .map(|j| (j.name.as_str(), j.needs.len()))
        .collect();
    let mut queue: Vec<&str> = indeg
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(&n, _)| n)
        .collect();
    let mut removed = 0usize;
    while let Some(n) = queue.pop() {
        removed += 1;
        // Any job that needs `n` loses one in-edge.
        for j in jobs {
            if j.needs.iter().any(|d| d == n) {
                let e = indeg.get_mut(j.name.as_str()).expect("name indexed");
                *e -= 1;
                if *e == 0 {
                    queue.push(j.name.as_str());
                }
            }
        }
    }
    if removed != jobs.len() {
        return Err(ResolveError::Cyclic);
    }
    Ok(())
}

fn valid_machine_token(value: &str, maximum: usize) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else { return false; };
    value.len() <= maximum && first.is_ascii_alphanumeric()
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn validate_authored_tokens(jobs: &[JobDef]) -> Result<(), ResolveError> {
    use myelin_ci_controlplane::run_plan::{MAX_JOB_NAME_BYTES, MAX_MATRIX_AXES, MAX_MATRIX_KEY_BYTES, MAX_MATRIX_VALUE_BYTES};
    for job in jobs {
        if !valid_machine_token(&job.name, MAX_JOB_NAME_BYTES) {
            return Err(ResolveError::InvalidPlan(format!("authored job name `{}` is not a bounded machine token", job.name)));
        }
        if job.matrix.len() > MAX_MATRIX_AXES {
            return Err(ResolveError::InvalidPlan(format!("job `{}` declares more than {MAX_MATRIX_AXES} matrix axes", job.name)));
        }
        for (axis, values) in &job.matrix {
            if !valid_machine_token(axis, MAX_MATRIX_KEY_BYTES) || values.is_empty() {
                return Err(ResolveError::InvalidPlan(format!("job `{}` matrix axis `{axis}` is invalid or empty", job.name)));
            }
            if values.iter().any(|value| !valid_machine_token(value, MAX_MATRIX_VALUE_BYTES)) {
                return Err(ResolveError::InvalidPlan(format!("job `{}` has an invalid matrix value for `{axis}`", job.name)));
            }
        }
    }
    Ok(())
}

/// **Resolve a parsed [`CiDefinition`] to a content-addressed [`ResolvedSnapshot`] + write it as a
/// T2 CAS blob (contract 11.2).** The full arch 02 §1.4 / §7.4 path:
///   1. **validate** — a non-empty, unique-named, acyclic job DAG (every `needs` resolves);
///   2. **resolve every image to a digest, FAIL-CLOSED** — a floating tag is REJECTED
///      ([`ResolveError::FloatingTag`]); 0 un-digested references reach the snapshot (the
///      supply-chain control, the real tag→digest registry resolution is CI-P23);
///   3. **expand the matrix DETERMINISTICALLY** — the sorted-axis cross-product, instance names
///      rendered in sorted axis order (reproducible);
///   4. **content-address** — serialise the resolved DAG to canonical JSON + `put` it into the
///      tenant's `BlobStore` (BLAKE3 address = the snapshot ref).
///
/// Returns the `(snapshot, address)` — the address is the `definition_snapshot` ref the `ci_run`
/// row + `ci.run.started` carry. The snapshot is IDENTICAL to the `myelin ci plan` output
/// (shift-left, no runner spend).
pub fn resolve_versioned_snapshot(
    def: &VersionedCiDefinition,
    blobs: &dyn BlobStore,
    tenant: &TenantId,
) -> Result<(VersionedResolvedSnapshot, ContentHash), ResolveError> {
    if def.jobs.is_empty() {
        return Err(ResolveError::EmptyDefinition);
    }
    validate_authored_tokens(&def.jobs)?;
    // Unique job names (DAG node ids).
    let mut seen = BTreeSet::new();
    for j in &def.jobs {
        if !seen.insert(j.name.as_str()) {
            return Err(ResolveError::DuplicateJob(j.name.clone()));
        }
    }
    validate_dag(&def.jobs)?;

    // FINDING #10 — the matrix-expansion DoS floor: compute the TOTAL instance count with SATURATING
    // multiplication BEFORE materializing anything, and refuse fail-closed past MAX_TOTAL_MATRIX_INSTANCES.
    // Each job contributes the product of its axis value-counts (a non-matrix job = 1); an axis is
    // non-empty by parse-validation. An untrusted push whose config would fan out to millions of
    // instances is rejected without ever allocating the cross-product.
    let mut total_instances: usize = 0;
    for j in &def.jobs {
        let job_instances = j
            .matrix
            .values()
            .fold(1usize, |acc, vals| acc.saturating_mul(vals.len().max(1)));
        total_instances = total_instances.saturating_add(job_instances);
        if total_instances > MAX_TOTAL_MATRIX_INSTANCES {
            return Err(ResolveError::MatrixTooLarge {
                count: total_instances,
                cap: MAX_TOTAL_MATRIX_INSTANCES,
            });
        }
    }

    // Resolve + expand. Collect into a Vec, then sort by instance name for the deterministic order.
    let mut resolved: Vec<ResolvedJobV2> = Vec::new();
    for j in &def.jobs {
        // DIGEST-PIN-OR-REJECT (fail-closed). REUSES the frozen ImageRef::digest_pinned rule — NOT a
        // second digest grammar. The real tag→digest registry resolution is CI-P23; here a
        // non-digest-pinned reference is REFUSED (0 floating tags reach a snapshot).
        let image = ImageRef {
            reference: j.image.clone(),
        };
        if !image.digest_pinned() {
            return Err(ResolveError::FloatingTag {
                job: j.name.clone(),
                reference: j.image.clone(),
            });
        }
        for assignment in expand_matrix(&j.matrix) {
            resolved.push(ResolvedJobV2 {
                stage: j.name.clone(),
                name: instance_name(&j.name, &assignment),
                image: j.image.clone(),
                command: j.command.clone(),
                needs: Vec::new(),
                is_generator: j.kind == JobKind::Generate,
                matrix_key: assignment,
            });
        }
    }
    let concrete_by_authored: BTreeMap<&str, Vec<String>> = def.jobs.iter().map(|job| {
        let mut names: Vec<_> = expand_matrix(&job.matrix).iter()
            .map(|assignment| instance_name(&job.name, assignment)).collect();
        names.sort(); names.dedup(); (job.name.as_str(), names)
    }).collect();
    for (job, authored) in resolved.iter_mut().zip(def.jobs.iter().flat_map(|job| {
        std::iter::repeat_n(job, expand_matrix(&job.matrix).len())
    })) {
        job.needs = authored.needs.iter()
            .flat_map(|need| concrete_by_authored[need.as_str()].iter().cloned()).collect();
        job.needs.sort(); job.needs.dedup();
    }
    // Deterministic order — the reproducibility floor (the same input → byte-identical snapshot).
    resolved.sort_by(|a, b| a.name.cmp(&b.name));
    for pair in resolved.windows(2) {
        if pair[0].name == pair[1].name { return Err(ResolveError::ConcreteNameCollision(pair[0].name.clone())); }
    }

    let snapshot = match &def.contract {
        CiPlanContract::V1 => VersionedResolvedSnapshot::V1(ResolvedRunPlanV1 {
            schema_version: myelin_ci_controlplane::RUN_PLAN_SCHEMA_V1,
            jobs: resolved.into_iter().map(|job| ResolvedJobV1 {
                name: job.name,
                image: job.image,
                command: job.command,
                needs: job.needs,
                is_generator: job.is_generator,
                matrix_key: job.matrix_key,
            }).collect(),
        }),
        CiPlanContract::V2(execution) => VersionedResolvedSnapshot::V2(ResolvedRunPlanV2 {
            schema_version: myelin_ci_controlplane::RUN_PLAN_SCHEMA_V2,
            execution: execution.clone(),
            jobs: resolved,
        }),
    };
    let bytes = snapshot.canonical_bytes().map_err(|error| ResolveError::InvalidPlan(error.to_string()))?;
    let address = blobs
        .put(tenant, &bytes)
        .map_err(ResolveError::BlobWrite)?;
    Ok((snapshot, address))
}

/// Resolve through the byte-identical legacy V1 source API.
pub fn resolve_snapshot(
    def: &CiDefinition,
    blobs: &dyn BlobStore,
    tenant: &TenantId,
) -> Result<(ResolvedSnapshot, ContentHash), ResolveError> {
    let versioned = VersionedCiDefinition::v1(def.on.clone(), def.jobs.clone());
    let (snapshot, address) = resolve_versioned_snapshot(&versioned, blobs, tenant)?;
    let VersionedResolvedSnapshot::V1(snapshot) = snapshot else {
        unreachable!("legacy definitions always resolve to V1")
    };
    Ok((snapshot, address))
}

/// The `ArtifactRef` form of a CAS snapshot address — the references-not-payloads handle the
/// `StartSpec.input` + the `ci_run.definition_snapshot` carry (never the snapshot bytes). The grammar
/// is `myelin://<tenant>/ci/snapshot/<algo>:<hex>` (a Refs-rooted ref; CI references it, Refs owns the
/// grammar, contract 5.7).
pub fn snapshot_ref(tenant: &TenantId, address: &ContentHash) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/ci/snapshot/{}",
        tenant.0,
        address.to_multihash_string()
    ))
}

// =================================================================================================
// 3. The reserve+start handoff (atomic, one tx — arch 02 §1.5 / §3).
// =================================================================================================

/// A check context the run reports (arch 03 §1.1 — `CheckContext = {provider, name}`). The first
/// `ci.check.updated{state: queued}` is emitted per context at start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckContext {
    /// The context name (e.g. `build`, `test/unit`).
    pub name: String,
}

impl CheckContext {
    /// A `ci`-provider check context of `name`.
    pub fn ci(name: impl Into<String>) -> CheckContext {
        CheckContext { name: name.into() }
    }
}

/// The `ci_run` row the reserve+start writes (arch 01 §3.1) — the thin index over the myelin-flow
/// workflow run, written in the SAME tx as the outbox drafts. The PII surface (`triggered_by`) is a
/// pseudonym subject (contract 4.8); this struct carries the non-PII fields the dispatch sets at
/// start (`state: queued`). The live table is `myelin_ci_controlplane::migrations::CREATE_CI_RUN_DDL`;
/// this is the value the consumer INSERTs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiRunWrite {
    /// The run id (an opaque uuid string) — the `ci_run` PK half.
    pub run_id: String,
    /// The content-addressed definition snapshot ref (the CAS blob, 11.2) — the reproducible
    /// definition the run runs.
    pub definition_snapshot: ArtifactRef,
    /// The trigger kind, as the `ci_run.trigger_kind` CHECK token (`push`/`pull_request`/…).
    pub trigger_kind: String,
    /// The stamped trust tier, as the `ci_run.trust_tier` CHECK token (`trusted`/`untrusted_fork`/
    /// `self_hosted`). The SAME value stamped onto every `ci.check.updated.trust_tier` (X-1).
    pub trust_tier: String,
    /// The lifecycle state at start — always `queued` (the reserve+start writes the row queued).
    pub state: String,
    /// The triggering `event_id` (the cause — the `ci_run.cause_event_id` provenance).
    pub cause_event_id: String,
}

/// The complete reserve+start handoff (arch 02 §1.5). Produced by [`reserve_and_start`]; the live
/// consumer (a) calls `DurableExecutor::start(self.start_spec)` (9.1) and (b) commits the atomic
/// bundle — the [`CiRunWrite`] row + the [`EventDraft`]s — via the outbox in ONE tx (2.2). Holding
/// the three together makes the atomicity invariant ([`Self::is_atomic_bundle`]) testable: there is
/// no partial run.
#[derive(Clone, Debug)]
pub struct StartHandoff {
    /// The `DurableExecutor::start` spec for the `ci.pipeline` workflow (9.1) — `input` is the
    /// references-not-payloads snapshot ref; `idem_key` makes a re-delivered trigger ONE run.
    pub start_spec: StartSpec,
    /// The `ci_run` row to INSERT (state = queued).
    pub run_write: CiRunWrite,
    /// The `ci.run.started` draft (carries trust_tier / trigger_kind / the CAS snapshot ref, §1.2).
    pub run_started: EventDraft,
    /// The first `ci.check.updated{state: queued}` draft PER context (X-1, §1.1 / §4).
    pub queued_checks: Vec<EventDraft>,
}

// Re-export so callers do not need a second `use` of the frozen flow type to read the handoff.
pub use myelin_flow::StartSpec;

/// The registered `ci.pipeline` workflow type (contract 9.1 — the body the durable executor drives;
/// the body itself is CI-P15 / `myelin_flow::ci_pipeline`). Dispatch only NAMES it at `start`.
pub const CI_PIPELINE_WF_TYPE: &str = "ci.pipeline";

impl StartHandoff {
    /// **The atomicity invariant (the prompt GATE): the bundle is all-or-nothing — a `ci_run` row
    /// state=`queued`, a `ci.run.started` event, AND at least one queued `ci.check.updated` per
    /// context, all present together.** The live consumer commits these in ONE tx via the outbox
    /// (2.2); this method makes the "no partial run" property assertable: a row with no queued check,
    /// or a started event with no row, would FAIL this — by construction it never does, because the
    /// handoff is one value built atomically.
    pub fn is_atomic_bundle(&self) -> bool {
        let row_queued = self.run_write.state == "queued";
        let started_is_run_started = self.run_started.type_.0 == CI_RUN_STARTED;
        let has_queued_check =
            !self.queued_checks.is_empty() && self.queued_checks.iter().all(is_queued_check_draft);
        // The snapshot ref the row carries IS the input the workflow starts on (no divergence).
        let snapshot_matches =
            self.start_spec.input.first() == Some(&self.run_write.definition_snapshot);
        row_queued && started_is_run_started && has_queued_check && snapshot_matches
    }
}

/// True iff a draft is a `ci.check.updated` carrying `state: "queued"`.
fn is_queued_check_draft(d: &EventDraft) -> bool {
    d.type_.0 == myelin_ci_sandbox::events::CI_CHECK_UPDATED
        && d.payload.get("state").and_then(|s| s.as_str()) == Some("queued")
}

/// Map an [`OnTrigger`] to its `ci_run.trigger_kind` CHECK token (the `CREATE_CI_RUN_DDL` CHECK set:
/// `push`/`pull_request`/`issue_transition`/`manual`/`agent`/`schedule`).
fn trigger_kind_token(on: &OnTrigger) -> &'static str {
    match on {
        OnTrigger::Push => "push",
        OnTrigger::PullRequest => "pull_request",
        OnTrigger::IssueTransitioned => "issue_transition",
        OnTrigger::Manual => "manual",
        OnTrigger::Schedule => "schedule",
        OnTrigger::Agent => "agent",
    }
}

/// Map the stamped [`TrustStamp`]'s job tier to its `ci_run.trust_tier` CHECK token
/// (`trusted`/`untrusted_fork`/`self_hosted`).
fn trust_tier_token(stamp: &TrustStamp) -> &'static str {
    use crate::dispatch::TrustTier;
    match stamp.job_tier {
        TrustTier::Trusted => "trusted",
        TrustTier::UntrustedFork => "untrusted_fork",
        TrustTier::SelfHosted => "self_hosted",
    }
}

/// The check-seam subject `repo#commit-<oid>/check-<context>` (X-1, arch 03 §4.12) — REUSES the
/// frozen `myelin_events::check_seam::check_subject` so the subject grammar is byte-identical to what
/// Git's gate consumes (no drift).
fn check_subject(repo: &str, commit_oid: &str, context: &str) -> ArtifactRef {
    myelin_events::check_seam::check_subject(repo, commit_oid, context)
}

/// The facts the reserve+start handoff needs about the run (provenance the snapshot does not carry):
/// the repo + commit the checks key on, the per-context list, and the pseudonym actor.
#[derive(Clone, Debug)]
pub struct RunFacts {
    /// The opaque run id (a uuid string).
    pub run_id: String,
    /// The repo ref the check seam keys on (X-1: `repo#commit-<oid>/check-<context>`).
    pub repo_ref: String,
    /// The commit oid the run ran against (the CheckStatus key half).
    pub commit_oid: String,
    /// The check contexts the run reports (one queued `ci.check.updated` each).
    pub contexts: Vec<CheckContext>,
    /// The triggering `event_id` (the cause provenance — `cause_event_id` + the `idem_key` derivation).
    pub cause_event_id: EventId,
}

/// **Build the atomic reserve+start handoff (arch 02 §1.5; the prompt's second GATE).** Given the
/// content-addressed `snapshot` ref (from [`resolve_snapshot`]), the CI-P10 trust `stamp`, the armed
/// trigger `on`, and the `facts`, produces the [`StartHandoff`]: the `DurableExecutor::start` spec for
/// the `ci.pipeline` workflow (9.1) + the atomic bundle the consumer commits in ONE tx — the
/// `ci_run` row (state=queued) + `ci.run.started` + the first `ci.check.updated{state: queued}` per
/// context (via the outbox, 2.2).
///
/// The `idem_key` is `<run_id>:<cause_event_id>` so a re-delivered trigger that already minted this
/// run is ONE start, not two (the 9.1 idempotency, paired with CI-P10's dedup ledger). Personal data
/// stays references-not-payloads: the `StartSpec.input` is the snapshot ref, never a body.
pub fn reserve_and_start(
    snapshot: &ArtifactRef,
    stamp: &TrustStamp,
    on: &OnTrigger,
    facts: &RunFacts,
) -> StartHandoff {
    let trigger_kind = trigger_kind_token(on).to_string();
    let trust_tier = trust_tier_token(stamp).to_string();

    // 9.1: the reserve+start spec for the ci.pipeline workflow. input = the snapshot ref
    // (references-not-payloads); idem_key = <run_id>:<cause_event_id> (a re-delivered trigger is ONE
    // run). The reserve bookend (refuse-start-on-exhaustion) is the workflow's first act (CI-P15);
    // dispatch supplies no budget here (the workflow reserves) — left None.
    let start_spec = StartSpec {
        wf_type: CI_PIPELINE_WF_TYPE.to_string(),
        input: vec![snapshot.clone()],
        budget: None,
        idem_key: format!("{}:{}", facts.run_id, facts.cause_event_id.0),
    };

    let run_write = CiRunWrite {
        run_id: facts.run_id.clone(),
        definition_snapshot: snapshot.clone(),
        trigger_kind: trigger_kind.clone(),
        trust_tier: trust_tier.clone(),
        state: "queued".to_string(),
        cause_event_id: facts.cause_event_id.0.clone(),
    };

    // §1.2: ci.run.started carries trust_tier / trigger_kind / the CAS snapshot ref
    // (references-not-payloads — the snapshot is a ref, not the bytes).
    let run_started = EventDraft {
        type_: EventType(CI_RUN_STARTED.to_string()),
        subject: ArtifactRef(format!("ci/run/{}", facts.run_id)),
        aggregate: AggregateKey(format!("ci/run/{}", facts.run_id)),
        payload: serde_json::json!({
            "run": format!("ci/run/{}", facts.run_id),
            "trust_tier": trust_tier,
            "trigger_kind": trigger_kind,
            "definition_snapshot": snapshot.0,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    };

    // §1.1 / §4: the first ci.check.updated{state: queued} per context (X-1). The CheckStatus is the
    // frozen small + PII-free struct; trust_tier is the SAME stamped value (0 divergence). REUSES the
    // frozen check_seam subject + aggregate grammar so Git's gate consumes a byte-identical subject.
    let queued_checks = facts
        .contexts
        .iter()
        .map(|ctx| {
            let check_status = serde_json::json!({
                "repo": facts.repo_ref,
                "commit_oid": facts.commit_oid,
                "context": { "provider": "ci", "name": ctx.name },
                "state": "queued",
                "run": format!("ci/run/{}", facts.run_id),
                "run_attempt": 1,
                "trust_tier": git_trust_token(stamp),
                "cost_settled": false,
            });
            EventDraft {
                type_: EventType(myelin_ci_sandbox::events::CI_CHECK_UPDATED.to_string()),
                subject: check_subject(&facts.repo_ref, &facts.commit_oid, &ctx.name),
                aggregate: myelin_events::check_seam::check_aggregate(
                    &facts.repo_ref,
                    &facts.commit_oid,
                ),
                payload: check_status,
                data_role: DataRole::Controller,
                visibility: Visibility::Internal,
                contains_personal_data: false,
                pii_key_ref: None,
            }
        })
        .collect();

    StartHandoff {
        start_spec,
        run_write,
        run_started,
        queued_checks,
    }
}

/// The `ci.check.updated.trust_tier` token (the 2-way merge-gate projection, X-1):
/// `untrusted_fork` for a fork, `trusted` otherwise (a self-hosted member run is trusted CODE for the
/// gate). REUSES the CI-P10 [`TrustStamp::check_tier`] (the git projection) — never recomputed.
fn git_trust_token(stamp: &TrustStamp) -> &'static str {
    use myelin_git::check_status::TrustTier as GitTrustTier;
    match stamp.check_tier {
        GitTrustTier::Trusted => "trusted",
        GitTrustTier::UntrustedFork => "untrusted_fork",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{stamp_trust, RunProvenance};
    use myelin_storage::FsBlobStore;

    const PINNED: &str = "registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000";
    const PINNED2: &str = "registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000";

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn blobs() -> FsBlobStore {
        // `FsBlobStore::new()` is the in-memory (HashMap-backed) M0 floor — DB-free.
        FsBlobStore::new()
    }

    fn member_stamp() -> TrustStamp {
        stamp_trust(&RunProvenance {
            is_fork: false,
            targets_self_hosted: false,
            read_excludes_fork: true,
        })
    }

    fn fork_stamp() -> TrustStamp {
        stamp_trust(&RunProvenance {
            is_fork: true,
            targets_self_hosted: false,
            read_excludes_fork: false,
        })
    }

    // -------- 1. The digest-pin-or-reject resolver (the supply-chain GATE) --------

    /// **THE floating-tag GATE: a floating-tag reference is REJECTED at resolution (fail-closed); 0
    /// un-digested references reach a snapshot.** The prompt's headline supply-chain control.
    #[test]
    fn a_floating_tag_is_rejected_fail_closed() {
        let def = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![JobDef::normal("build", "alpine:3", ["build"])],
        };
        let err = resolve_snapshot(&def, &blobs(), &tenant())
            .expect_err("a floating tag must be rejected fail-closed");
        assert!(
            matches!(&err, ResolveError::FloatingTag { job, reference }
                if job == "build" && reference == "alpine:3"),
            "the floating tag is refused with its job + reference: {err:?}"
        );
    }

    /// Variants of un-digested references are ALL rejected (a bare name, a `:tag`, an empty digest) —
    /// the fail-closed rule has no gap.
    #[test]
    fn every_undigested_reference_shape_is_rejected() {
        for bad in [
            "alpine",
            "alpine:latest",
            "registry/foo@sha256:",
            "foo@:abc",
        ] {
            let def = CiDefinition {
                on: OnTrigger::Push,
                jobs: vec![JobDef::normal("j", bad, ["run"])],
            };
            assert!(
                matches!(
                    resolve_snapshot(&def, &blobs(), &tenant()),
                    Err(ResolveError::FloatingTag { .. })
                ),
                "the un-digested reference `{bad}` must be rejected fail-closed"
            );
        }
    }

    /// **Peer-review finding #10 — the matrix cross-product is capped (DoS floor).** A config with ~8
    /// axes of 10 values fans out to 10^8 = 100M instances; the count is computed with SATURATING
    /// arithmetic BEFORE expansion, so the push is refused fail-closed with `MatrixTooLarge` and NOTHING
    /// is allocated. A legitimately-sized matrix (well under the ceiling) still resolves.
    #[test]
    fn an_unbounded_matrix_is_refused_before_it_is_materialized() {
        // 8 axes × 10 values each = 10^8 instances — an OOM without the cap.
        let mut job = JobDef::normal("build", PINNED, ["build"]);
        for a in 0..8u32 {
            job = job.with_matrix(
                format!("axis{a}"),
                (0..10u32).map(|v| v.to_string()).collect(),
            );
        }
        let def = CiDefinition { on: OnTrigger::Push, jobs: vec![job] };
        let err = resolve_snapshot(&def, &blobs(), &tenant())
            .expect_err("an astronomical matrix must be refused");
        assert!(
            matches!(err, ResolveError::MatrixTooLarge { count, cap }
                if count > cap && cap == MAX_TOTAL_MATRIX_INSTANCES),
            "the over-cap matrix is refused fail-closed: {err:?}"
        );

        // A legitimate matrix (3 × 4 = 12 instances) still resolves + content-addresses.
        let ok = JobDef::normal("test", PINNED2, ["test"])
            .with_matrix("os", vec!["linux".into(), "mac".into(), "win".into()])
            .with_matrix("v", vec!["1".into(), "2".into(), "3".into(), "4".into()]);
        let def_ok = CiDefinition { on: OnTrigger::Push, jobs: vec![ok] };
        let (snap, _addr) = resolve_snapshot(&def_ok, &blobs(), &tenant())
            .expect("a modestly-sized matrix resolves");
        assert_eq!(snap.jobs.len(), 12, "3×4 expands to 12 instances");
    }

    /// **A digest-pinned definition resolves + content-addresses (the happy path).** Every image is
    /// `@sha256:<hex>`; the snapshot is written as a CAS blob and the returned address round-trips.
    #[test]
    fn a_digest_pinned_definition_resolves_and_content_addresses() {
        let store = blobs();
        let def = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![
                JobDef::normal("build", PINNED, ["build"]),
                JobDef::normal("test", PINNED2, ["test"]).with_needs(["build"]),
            ],
        };
        let (snap, addr) =
            resolve_snapshot(&def, &store, &tenant()).expect("a digest-pinned def resolves");
        assert_eq!(snap.jobs.len(), 2, "two resolved jobs");
        // The CAS blob exists at the returned address + round-trips to the canonical bytes.
        let bytes = store
            .get(&tenant(), &addr)
            .expect("the snapshot blob is present");
        assert_eq!(
            bytes,
            snap.canonical_bytes().unwrap(),
            "the blob IS the snapshot bytes"
        );
        // The address is the BLAKE3 content address of those bytes (content-addressed by construction).
        assert_eq!(addr, ContentHash::blake3(&snap.canonical_bytes().unwrap()));
    }

    // -------- 2. Deterministic matrix expansion --------

    /// **The matrix expands DETERMINISTICALLY: the same definition yields a byte-identical snapshot
    /// (same address) every time, with instance names in sorted axis order.** The reproducibility
    /// floor (VISION §3).
    #[test]
    fn the_matrix_expands_deterministically() {
        let store = blobs();
        let def = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![JobDef::normal("test", PINNED, ["test"])
                .with_matrix("os", vec!["linux".into(), "macos".into()])
                .with_matrix("rust", vec!["stable".into(), "beta".into()])],
        };
        let (snap, addr) = resolve_snapshot(&def, &store, &tenant()).expect("resolves");
        // 2 os × 2 rust = 4 instances.
        assert_eq!(snap.jobs.len(), 4, "the 2×2 matrix expands to 4 instances");
        // Instance names are rendered in SORTED axis order (os before rust) + sorted overall.
        let names: Vec<&str> = snap.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names.len(), 4);
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(names.iter().all(|name| name.starts_with("test--")));
        // Re-resolving the SAME definition yields the SAME content address (byte-identical).
        let (_snap2, addr2) = resolve_snapshot(&def, &blobs(), &tenant()).expect("re-resolves");
        assert_eq!(
            addr, addr2,
            "the same input → the same content address (reproducible)"
        );
        assert_eq!(
            addr.to_multihash_string(),
            "blake3:fc837be9ceab0d70f24be239f1c4c0a5167e95e2904087f5b7443dfe354b858d",
            "the legacy V1 matrix wire stays byte-identical"
        );
    }

    #[test]
    fn command_hash_and_concrete_matrix_fan_in_are_deterministic() {
        let definition = |command: &str| CiDefinition { on: OnTrigger::Push, jobs: vec![
            JobDef::normal("build", PINNED, [command]).with_matrix("os", vec!["linux".into(), "macos".into()]),
            JobDef::normal("test", PINNED2, ["test"]).with_needs(["build"])
                .with_matrix("rust", vec!["stable".into(), "beta".into()]),
        ] };
        let (first, hash) = resolve_snapshot(&definition("build"), &blobs(), &tenant()).unwrap();
        let (_, repeat) = resolve_snapshot(&definition("build"), &blobs(), &tenant()).unwrap();
        let (_, changed) = resolve_snapshot(&definition("build-v2"), &blobs(), &tenant()).unwrap();
        assert_eq!(hash, repeat);
        assert_ne!(hash, changed);
        let builds: Vec<String> = first.jobs.iter().filter(|job| job.name.starts_with("build--"))
            .map(|job| job.name.clone()).collect();
        assert_eq!(builds.len(), 2);
        for test in first.jobs.iter().filter(|job| job.name.starts_with("test--")) {
            assert_eq!(test.needs, builds);
        }
    }

    #[test]
    fn malformed_programmatic_plans_fail_closed_without_name_collisions() {
        let prefix = "a".repeat(70);
        let def = CiDefinition { on: OnTrigger::Push, jobs: vec![
            JobDef::normal(format!("{prefix}x"), PINNED, ["a"]).with_matrix("os", vec!["linux".into()]),
            JobDef::normal(format!("{prefix}y"), PINNED2, ["b"]).with_matrix("os", vec!["linux".into()]),
        ] };
        let (plan, _) = resolve_snapshot(&def, &blobs(), &tenant()).unwrap();
        assert_ne!(plan.jobs[0].name, plan.jobs[1].name);
        let bad = CiDefinition { on: OnTrigger::Push, jobs: vec![JobDef::normal("unicode-雪", PINNED, ["run"])] };
        assert!(matches!(resolve_snapshot(&bad, &blobs(), &tenant()), Err(ResolveError::InvalidPlan(_))));
        let empty = CiDefinition { on: OnTrigger::Push, jobs: vec![JobDef::normal("build", PINNED, std::iter::empty::<String>())] };
        assert!(matches!(resolve_snapshot(&empty, &blobs(), &tenant()), Err(ResolveError::InvalidPlan(_))));
    }

    #[test]
    fn snapshot_ref_is_exact_tenant_scoped_lowercase_blake3() {
        let address = ContentHash::blake3(b"snapshot");
        let reference = snapshot_ref(&tenant(), &address);
        assert_eq!(reference.0, format!("myelin://acme/ci/snapshot/{}", address.to_multihash_string()));
        let digest = reference.0.strip_prefix("myelin://acme/ci/snapshot/blake3:").unwrap();
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }

    /// An empty definition + a structural-DAG defect (a cycle / a dangling need / a dup name) is
    /// rejected — the resolver validates the DAG before content-addressing.
    #[test]
    fn structural_defects_are_rejected() {
        let store = blobs();
        // Empty.
        assert_eq!(
            resolve_snapshot(
                &CiDefinition {
                    on: OnTrigger::Push,
                    jobs: vec![]
                },
                &store,
                &tenant()
            ),
            Err(ResolveError::EmptyDefinition)
        );
        // A dangling need.
        assert!(matches!(
            resolve_snapshot(
                &CiDefinition {
                    on: OnTrigger::Push,
                    jobs: vec![JobDef::normal("a", PINNED, ["a"]).with_needs(["ghost"])],
                },
                &store,
                &tenant()
            ),
            Err(ResolveError::UnknownNeed { need, .. }) if need == "ghost"
        ));
        // A cycle (a→b→a).
        assert_eq!(
            resolve_snapshot(
                &CiDefinition {
                    on: OnTrigger::Push,
                    jobs: vec![
                        JobDef::normal("a", PINNED, ["a"]).with_needs(["b"]),
                        JobDef::normal("b", PINNED, ["b"]).with_needs(["a"]),
                    ],
                },
                &store,
                &tenant()
            ),
            Err(ResolveError::Cyclic)
        );
        // A duplicate job name.
        assert!(matches!(
            resolve_snapshot(
                &CiDefinition {
                    on: OnTrigger::Push,
                    jobs: vec![JobDef::normal("a", PINNED, ["a"]), JobDef::normal("a", PINNED2, ["a"])],
                },
                &store,
                &tenant()
            ),
            Err(ResolveError::DuplicateJob(n)) if n == "a"
        ));
        assert_eq!(
            resolve_snapshot(&CiDefinition { on: OnTrigger::Push,
                jobs: vec![JobDef::normal("a", PINNED, ["a"]).with_needs(["a"])] }, &store, &tenant()),
            Err(ResolveError::SelfNeed("a".into()))
        );
        assert_eq!(
            resolve_snapshot(&CiDefinition { on: OnTrigger::Push, jobs: vec![
                JobDef::normal("a", PINNED, ["a"]),
                JobDef::normal("b", PINNED2, ["b"]).with_needs(["a", "a"]),
            ] }, &store, &tenant()),
            Err(ResolveError::DuplicateNeed { job: "b".into(), need: "a".into() })
        );
    }

    // -------- 3. The dynamic-generation escape-hatch floor (named + hooked) --------

    /// **The dynamic-generation escape hatch is hooked (the named floor): a `Generate` job is a
    /// NORMAL digest-pinned job whose presence the snapshot exposes via `has_dynamic_generation`.**
    /// The generator runs in the SAME sandbox (no privileged config-eval path); the in-sandbox
    /// EXECUTION lands with the runner (CI-P15).
    #[test]
    fn dynamic_generation_is_refused_until_fragment_ingestion_exists() {
        let def = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![JobDef::normal("gen-matrix", PINNED, ["generate"]).as_generator()],
        };
        assert!(matches!(resolve_snapshot(&def, &blobs(), &tenant()), Err(ResolveError::InvalidPlan(_))));
        // A normal definition has no generator.
        let plain = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![JobDef::normal("build", PINNED, ["build"])],
        };
        let (s2, _) = resolve_snapshot(&plain, &blobs(), &tenant()).unwrap();
        assert!(!s2.has_dynamic_generation());
    }

    // -------- 4. The atomic reserve+start handoff (the second GATE) --------

    fn facts() -> RunFacts {
        RunFacts {
            run_id: "run-0001".into(),
            repo_ref: "myelin://acme/git/repo/web".into(),
            commit_oid: "deadbeef".into(),
            contexts: vec![CheckContext::ci("build"), CheckContext::ci("test/unit")],
            cause_event_id: EventId("ev-push-1".into()),
        }
    }

    /// **THE reserve+start GATE: the handoff is an ATOMIC bundle — the `ci_run` row (queued) +
    /// `ci.run.started` + the first queued `ci.check.updated` per context, all present together (one
    /// tx, no partial run).** The snapshot ref the row carries IS the workflow `start` input.
    #[test]
    fn the_reserve_start_handoff_is_an_atomic_bundle() {
        let snap = snapshot_ref(&tenant(), &ContentHash::blake3(b"snap"));
        let handoff = reserve_and_start(&snap, &member_stamp(), &OnTrigger::Push, &facts());

        assert!(
            handoff.is_atomic_bundle(),
            "the row + ci.run.started + the queued checks are one atomic bundle"
        );
        // The ci_run row is queued + carries the snapshot ref + the stamped tier.
        assert_eq!(handoff.run_write.state, "queued");
        assert_eq!(handoff.run_write.definition_snapshot, snap);
        assert_eq!(handoff.run_write.trust_tier, "trusted");
        assert_eq!(handoff.run_write.trigger_kind, "push");
        // ci.run.started carries trust_tier / trigger_kind / the snapshot ref (§1.2).
        assert_eq!(handoff.run_started.type_.0, CI_RUN_STARTED);
        assert_eq!(handoff.run_started.payload["trust_tier"], "trusted");
        assert_eq!(handoff.run_started.payload["definition_snapshot"], snap.0);
        // One queued ci.check.updated PER context (build, test/unit).
        assert_eq!(
            handoff.queued_checks.len(),
            2,
            "one queued check per context"
        );
        for c in &handoff.queued_checks {
            assert_eq!(c.type_.0, myelin_ci_sandbox::events::CI_CHECK_UPDATED);
            assert_eq!(c.payload["state"], "queued");
            assert_eq!(c.payload["run_attempt"], 1);
        }
        // The StartSpec names the ci.pipeline workflow + starts on the snapshot ref.
        assert_eq!(handoff.start_spec.wf_type, CI_PIPELINE_WF_TYPE);
        assert_eq!(handoff.start_spec.input, vec![snap]);
        // The idem_key makes a re-delivered trigger ONE run.
        assert_eq!(handoff.start_spec.idem_key, "run-0001:ev-push-1");
    }

    /// **The stamped trust tier rides BOTH the `ci_run` row AND every queued `ci.check.updated` with
    /// 0 divergence (X-1).** A fork run is `untrusted_fork` on the row (3-way) AND `untrusted_fork` on
    /// every queued check (2-way projection) — the SAME fork verdict.
    #[test]
    fn the_trust_tier_rides_the_row_and_every_check_zero_divergence() {
        let snap = snapshot_ref(&tenant(), &ContentHash::blake3(b"snap"));
        let handoff = reserve_and_start(&snap, &fork_stamp(), &OnTrigger::PullRequest, &facts());
        assert_eq!(
            handoff.run_write.trust_tier, "untrusted_fork",
            "the 3-way row tier"
        );
        assert_eq!(handoff.run_write.trigger_kind, "pull_request");
        for c in &handoff.queued_checks {
            assert_eq!(
                c.payload["trust_tier"], "untrusted_fork",
                "the queued check carries the SAME fork verdict (X-1, 0 divergence)"
            );
        }
        assert!(handoff.is_atomic_bundle());
    }

    /// The queued check subject is the X-1 `repo#commit-<oid>/check-<context>` seam grammar (the
    /// byte-identical Git-consumed subject — REUSES the frozen check_seam helper, no drift).
    #[test]
    fn the_queued_check_subject_is_the_x1_seam_grammar() {
        let snap = snapshot_ref(&tenant(), &ContentHash::blake3(b"snap"));
        let handoff = reserve_and_start(&snap, &member_stamp(), &OnTrigger::Push, &facts());
        let build = &handoff.queued_checks[0];
        assert_eq!(
            build.subject.0, "myelin://acme/git/repo/web#commit-deadbeef/check-build",
            "the X-1 check subject grammar"
        );
        // The aggregate is the per-commit ordering partition (all contexts share it).
        for c in &handoff.queued_checks {
            assert_eq!(
                c.aggregate.0, "myelin://acme/git/repo/web#commit-deadbeef",
                "the per-commit aggregate (the ordering partition the contexts share)"
            );
        }
    }

    /// The trigger kind maps to the FROZEN `ci_run.trigger_kind` CHECK token for every trigger (a
    /// token the live `CREATE_CI_RUN_DDL` CHECK admits — no drift).
    #[test]
    fn trigger_kinds_map_to_the_frozen_check_tokens() {
        let snap = snapshot_ref(&tenant(), &ContentHash::blake3(b"snap"));
        for (on, tok) in [
            (OnTrigger::Push, "push"),
            (OnTrigger::PullRequest, "pull_request"),
            (OnTrigger::IssueTransitioned, "issue_transition"),
            (OnTrigger::Manual, "manual"),
            (OnTrigger::Schedule, "schedule"),
            (OnTrigger::Agent, "agent"),
        ] {
            let h = reserve_and_start(&snap, &member_stamp(), &on, &facts());
            assert_eq!(h.run_write.trigger_kind, tok, "trigger_kind for {on:?}");
        }
    }
}
