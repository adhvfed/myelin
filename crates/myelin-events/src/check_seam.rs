//! # `check_seam` — the Git↔CI check-seam carriage (EB-24 / P-144)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §4.12 (the Git↔CI check seam —
//! what the Bus carries) + `00-reconciliation-decisions.md` X-1 (the most load-bearing
//! cross-subsystem seam). **Contracts:** 5.9 CARRIAGE (the Bus's narrow half) + 9.4 CONSUMED (the
//! durable `ci.result` wait). **Drill:** D-11 (check-seam ordering, X-1).
//!
//! ## The Bus's role is NARROW (and only narrow) — what this module owns vs. does NOT own
//! The X-1 seam (`ci.check.updated` per-context facts + the `ci.result` rollup the merge queue
//! waits on) is OWNED by **CI** (producer) + **Git** (gate). The Bus's role is *additive and
//! narrow* — it carries two new event flows and provides one durable wait substrate. Concretely:
//!
//! **The Bus OWNS (this module):**
//! 1. **Envelope conformance.** [`check_updated_draft`] builds the canonical [`EventEnvelope`]
//!    draft for a `ci.check.updated` fact with the §4.12 `subject` token grammar
//!    (`myelin://<tenant>/git/commit/<repo-id>:<oid>#check-<context>`) and the
//!    **`aggregate = commit:<repo-id>:<oid>`** key (so all checks for one commit share an ordering
//!    partition). The CI-owned `CheckStatus` rides in `payload` as an OPAQUE
//!    [`serde_json::Value`] — the Bus does NOT name its fields.
//! 2. **Per-aggregate ordering on `(repo, commit_oid)`.** [`CheckSeamOrder`] ingests
//!    `ci.check.updated` envelopes that may arrive **interleaved across contexts** and
//!    **late/out-of-`seq`**, and exposes them **per-aggregate ordered** by the envelope's outbox
//!    `seq` (the `UNIQUE(aggregate, seq)` order == state-change order, §2.2/§4.12). This is the
//!    ordering substrate Git's `run_attempt` supersession rule RELIES ON (the Bus guarantees the
//!    order; it does NOT evaluate the rule).
//! 3. **The durable `wait_for_signal("ci.result", idem_key)` substrate.** [`CiResultWaitSubstrate`]
//!    is the Bus's narrow half of the 9.4 durable-signal wait the merge-queue workflow parks on:
//!    a [`CiResult`] signal delivered (possibly **doubly**, at-least-once) for an `idem_key`
//!    **wakes the waiter EXACTLY ONCE** (idempotent on `idem_key`). The real durable-workflow
//!    engine is `myelin-flow` (downstream, contract 9.1 — the named floor); this is the *substrate*
//!    its `wait_for_signal` resolves through, exactly the DAG-respecting seam pattern the
//!    Automation engine's [`crate`]-external `DurableExecutor` uses.
//!
//! **The Bus does NOT own (CI/Git, contract 5.9 / X-1):**
//! - the `CheckStatus` field shape (opaque `payload`);
//! - the `(commit_oid, context)` last-writer-wins / `run_attempt` supersession (it is *Git's*
//!   projection — this module only guarantees the per-aggregate ORDER the rule needs, and proves a
//!   stale lower-attempt re-delivery is DROPPABLE because the order is preserved);
//! - `trust_tier`/fork-endorsement gating;
//! - the merge gate.
//!
//! "A shaping, not a new engine." (§4.12.)
//!
//! ## Bands / floors
//! - The **consumer** (Git's `check_status` projection, built from the [`crate::consumer`]
//!   idempotent template over this ordered carriage) lands EB-26 / P-246 (M3).
//! - The **producer** (CI emits `ci.check.updated` + the rollup `ci.result`) lands EB-27 (M4),
//!   which makes the seam END-TO-END.
//! - The real `myelin-flow` durable engine behind `wait_for_signal` is P-FLOW-04 (the named floor);
//!   this module is its in-cell *signal substrate* (the idempotent wake on `idem_key`), unchanged
//!   in shape when the real engine lands.

use crate::taxonomy::new_tokens::{CI_CHECK_UPDATED, CI_RESULT};
use crate::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventType, SubjectComponent,
    Visibility,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// 1. Envelope conformance — the ci.check.updated subject + aggregate grammar
// ---------------------------------------------------------------------------

/// A validated canonical Git commit root used by the check seam.
///
/// `myelin-events` cannot depend on `myelin-refs` because Refs is above Events in the crate DAG.
/// This narrow boundary therefore validates the exact Git commit-root form it consumes. Callers
/// that already depend on Refs should parse the root there first, then cross this boundary. Once
/// constructed, every check/result subject and aggregate is derived from this value rather than
/// from independently supplied strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckCommit {
    root: ArtifactRef,
    repo_id: String,
    commit_oid: String,
    encoded_repo_id: SubjectComponent,
    encoded_commit_oid: SubjectComponent,
}

impl CheckCommit {
    /// Derive a canonical commit root from a validated Git repository root and immutable object id.
    pub fn from_repo_root(
        repo_root: &ArtifactRef,
        commit_oid: &str,
    ) -> Result<Self, CheckSeamError> {
        let rest = repo_root
            .0
            .strip_prefix("myelin://")
            .ok_or(CheckSeamError::InvalidRepoRoot)?;
        if rest.contains('#') {
            return Err(CheckSeamError::InvalidRepoRoot);
        }
        let segments: Vec<&str> = rest.split('/').collect();
        if segments.len() != 4
            || segments[0].is_empty()
            || segments[1] != "git"
            || segments[2] != "repo"
            || segments[3].is_empty()
        {
            return Err(CheckSeamError::InvalidRepoRoot);
        }
        let encoded_repo =
            SubjectComponent::parse(segments[3]).map_err(|_| CheckSeamError::InvalidRepoRoot)?;
        let encoded_oid =
            SubjectComponent::encode(commit_oid).map_err(|_| CheckSeamError::InvalidCommitRoot)?;
        Self::parse(&ArtifactRef(format!(
            "myelin://{}/git/commit/{}:{commit_oid}",
            segments[0],
            encoded_repo.as_str(),
            commit_oid = encoded_oid.as_str()
        )))
    }

    /// Validate `myelin://<tenant>/git/commit/<repo-id>:<oid>` with no sub-anchor.
    pub fn parse(root: &ArtifactRef) -> Result<Self, CheckSeamError> {
        let rest = root
            .0
            .strip_prefix("myelin://")
            .ok_or(CheckSeamError::InvalidCommitRoot)?;
        if rest.contains('#') {
            return Err(CheckSeamError::InvalidCommitRoot);
        }
        let mut segments = rest.split('/');
        let tenant = segments.next().unwrap_or_default();
        let subsystem = segments.next().unwrap_or_default();
        let type_ = segments.next().unwrap_or_default();
        let id = segments.next().unwrap_or_default();
        if tenant.is_empty()
            || subsystem != "git"
            || type_ != "commit"
            || id.is_empty()
            || segments.next().is_some()
        {
            return Err(CheckSeamError::InvalidCommitRoot);
        }
        let (encoded_repo_id, encoded_commit_oid) = id
            .split_once(':')
            .ok_or(CheckSeamError::InvalidCommitRoot)?;
        let encoded_repo_id = SubjectComponent::parse(encoded_repo_id)
            .map_err(|_| CheckSeamError::InvalidCommitRoot)?;
        let encoded_commit_oid = SubjectComponent::parse(encoded_commit_oid)
            .map_err(|_| CheckSeamError::InvalidCommitRoot)?;
        Ok(Self {
            root: root.clone(),
            repo_id: encoded_repo_id.decode(),
            commit_oid: encoded_commit_oid.decode(),
            encoded_repo_id,
            encoded_commit_oid,
        })
    }

    /// The validated canonical Git commit root.
    pub fn root(&self) -> &ArtifactRef {
        &self.root
    }

    /// The stable repository id carried by the commit root.
    pub fn repo_id(&self) -> &str {
        &self.repo_id
    }

    /// The immutable commit object id carried by the commit root.
    pub fn commit_oid(&self) -> &str {
        &self.commit_oid
    }
}

/// The `(repo, commit_oid)` aggregate key for the check seam (§4.12). ALL `ci.check.updated`
/// events for one commit share this aggregate, so they are per-aggregate ordered regardless of
/// which `context` they belong to — the ordering partition Git's supersession rule rests on.
///
/// PII-free identifiers (a repo ref + a git OID); never a payload. The Bus carries this opaque —
/// it does not parse the repo ref's internals (Refs owns the `ArtifactRef` grammar, contract 5.7).
pub fn check_aggregate(commit: &CheckCommit) -> AggregateKey {
    AggregateKey(format!(
        "commit:{}:{}",
        commit.encoded_repo_id.as_str(),
        commit.encoded_commit_oid.as_str()
    ))
}

/// The `ci.check.updated` envelope subject (§4.12 / X-1): the canonical Git commit root with a
/// `#check-<context>` sub-anchor. The subject identifies the *per-context* fact; the aggregate
/// above is the per-commit ordering partition the contexts share.
pub fn check_subject(commit: &CheckCommit, context: &str) -> Result<ArtifactRef, CheckSeamError> {
    if context.is_empty()
        || context
            .chars()
            .any(|c| c == '#' || c == '%' || c.is_whitespace() || c.is_control())
    {
        return Err(CheckSeamError::InvalidContext);
    }
    Ok(ArtifactRef(format!("{}#check-{context}", commit.root.0)))
}

/// Build the canonical [`EventDraft`] for a `ci.check.updated` fact. The Bus owns ENVELOPE
/// CONFORMANCE — the `type_`, `subject` token grammar, and the `(repo, commit_oid)` aggregate — and
/// carries the CI-owned `CheckStatus` as an **opaque** payload (references-not-payloads: the
/// CI-owned struct is small + PII-free, carrying `run`/`details_ref` `ArtifactRef`s, never log
/// bytes). The Bus does NOT name a single `CheckStatus` field — `check_status` is passed through.
///
/// This is the producer-side helper CI uses (EB-27/M4); shipping it here in M2 pins the envelope
/// shape the M3 consumer leg (Git) and the D-11 ordering drill assert against.
pub fn check_updated_draft(
    commit: &CheckCommit,
    context: &str,
    check_status: serde_json::Value,
) -> Result<EventDraft, CheckSeamError> {
    Ok(EventDraft {
        type_: EventType(CI_CHECK_UPDATED.to_string()),
        subject: check_subject(commit, context)?,
        aggregate: check_aggregate(commit),
        payload: check_status,
        // The CheckStatus is references-not-payloads (run/details_ref refs, not log bytes); the
        // check fact is platform-controller data (CI is the processor of tenant code, but the
        // *fact that a check ran* is controller metadata — matches the seed taxonomy classing).
        data_role: DataRole::Controller,
        // A check fact drives the always-visible PR-checks UI — internal to the repo's members.
        visibility: Visibility::Internal,
        // No inline PII (references-not-payloads).
        contains_personal_data: false,
        pii_key_ref: None,
    })
}

// ---------------------------------------------------------------------------
// 2. Per-aggregate ordering on (repo, commit_oid) — the D-11 substrate
// ---------------------------------------------------------------------------

/// One ingested `ci.check.updated` envelope, reduced to the fields the Bus's ordering substrate
/// reasons about: its outbox `seq` (the `UNIQUE(aggregate, seq)` order key), the per-context
/// `subject`, and the opaque payload. The Bus does NOT crack the payload — Git does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderedCheck {
    /// The outbox `seq` within the aggregate — the linearisation key (§2.2). State-change order
    /// == outbox order == this `seq` (the `UNIQUE(aggregate, seq)` invariant).
    pub seq: u64,
    /// The per-context fact subject (canonical commit root plus `#check-<context>`).
    pub subject: ArtifactRef,
    /// The opaque CI-owned `CheckStatus` (the Bus carries it; Git interprets it).
    pub check_status: serde_json::Value,
}

/// **The per-aggregate ordering substrate (§4.12 / D-11).** Carries `ci.check.updated` events for
/// ONE aggregate `(repo, commit_oid)` and exposes them **per-aggregate ordered by outbox `seq`**,
/// no matter what arrival INTERLEAVING (across contexts) or LATENESS (a re-delivered lower `seq`
/// after a higher one) the at-least-once transport produced.
///
/// This is the Bus's *narrow* guarantee: a well-defined order on the partition. It does NOT
/// evaluate the supersession rule — it PROVES the rule is well-defined (a stale lower-`run_attempt`
/// re-delivery is droppable *because* the aggregate order is preserved, so Git's monotonic
/// supersession is deterministic regardless of physical arrival order).
#[derive(Debug, Default)]
pub struct CheckSeamOrder {
    /// The aggregate this order is for (`commit:<repo-id>:<oid>`). One [`CheckSeamOrder`] per aggregate
    /// — cross-aggregate order is explicitly NOT promised (§2.2).
    aggregate: String,
    /// `seq → OrderedCheck`. A `BTreeMap` keyed on the outbox `seq` so iteration is ALWAYS in
    /// per-aggregate order, regardless of insertion (arrival) order — the ordering guarantee made
    /// structural. A re-delivered identical `seq` is an idempotent no-op (at-least-once dedup at
    /// the ordering layer; the consumer's `consumer_dedup` ledger is the other half).
    by_seq: BTreeMap<u64, OrderedCheck>,
}

impl CheckSeamOrder {
    /// A fresh per-aggregate order for `(repo, commit_oid)`.
    pub fn new(commit: &CheckCommit) -> CheckSeamOrder {
        CheckSeamOrder {
            aggregate: check_aggregate(commit).0,
            by_seq: BTreeMap::new(),
        }
    }

    /// The aggregate key string this order is keyed on (`commit:<repo-id>:<oid>`).
    pub fn aggregate(&self) -> &str {
        &self.aggregate
    }

    /// Ingest a delivered `ci.check.updated` envelope at its outbox `seq`. Returns `true` if this
    /// was a NEW `seq` (admitted into the order) and `false` if it was a **re-delivery of a `seq`
    /// already seen** (an at-least-once duplicate — absorbed, no reordering). The envelope MUST
    /// belong to this aggregate (the type + aggregate are checked); a mismatch is rejected so a
    /// foreign event can never silently corrupt the partition order.
    pub fn ingest(&mut self, env: &EventEnvelope, seq: u64) -> Result<bool, CheckSeamError> {
        if env.type_.0 != CI_CHECK_UPDATED {
            return Err(CheckSeamError::WrongType(env.type_.0.clone()));
        }
        if env.aggregate.0 != self.aggregate {
            return Err(CheckSeamError::WrongAggregate {
                expected: self.aggregate.clone(),
                got: env.aggregate.0.clone(),
            });
        }
        match self.by_seq.entry(seq) {
            std::collections::btree_map::Entry::Occupied(_) => {
                // A re-delivery of a seq already in the order — at-least-once duplicate. The order
                // is unchanged (idempotent); this is the "stale re-delivery is droppable" property
                // at the ordering layer.
                Ok(false)
            }
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(OrderedCheck {
                    seq,
                    subject: env.subject.clone(),
                    check_status: env.payload.clone(),
                });
                Ok(true)
            }
        }
    }

    /// The checks for this aggregate, **in per-aggregate `seq` order** — the order Git's
    /// supersession rule consumes. Iteration is always ascending `seq` (the `BTreeMap` guarantee),
    /// so an interleaved/late ingest order has NO effect on the consumed order.
    pub fn in_order(&self) -> Vec<OrderedCheck> {
        self.by_seq.values().cloned().collect()
    }

    /// The contiguous per-aggregate `seq`s observed (for the D-11 ordering-gap assertion). A GAP
    /// (a missing `seq` below the high-water mark) means an op is in flight, NOT lost — the
    /// at-least-once transport redelivers it; the drill asserts the gap closes to 0.
    pub fn observed_seqs(&self) -> Vec<u64> {
        self.by_seq.keys().copied().collect()
    }

    /// The number of MISSING `seq`s in `[1, high_water]` — the ordering-health gap the D-11 drill
    /// reads (asserted `== 0` once every in-flight op has been delivered). `0` ⇒ a contiguous,
    /// fully-ordered partition.
    pub fn ordering_gap(&self) -> u64 {
        match self.by_seq.keys().next_back() {
            None => 0,
            Some(&high) => high - self.by_seq.len() as u64,
        }
    }
}

/// The Bus rejects a malformed ingest into the check-seam order — a foreign type or a foreign
/// aggregate. Loud, never silent: a wrong event can never silently corrupt the partition order
/// (the order Git's supersession rule depends on).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckSeamError {
    /// A check producer did not provide a canonical, root-only Git repository ArtifactRef.
    InvalidRepoRoot,
    /// A check target was not a canonical, root-only Git commit ArtifactRef.
    InvalidCommitRoot,
    /// A check context could not be represented as one canonical `check-` sub-anchor.
    InvalidContext,
    /// A ci.result payload named a different commit than its canonical subject root.
    ResultCommitMismatch,
    /// The envelope's `type_` is not `ci.check.updated`.
    WrongType(String),
    /// The envelope's aggregate is not the one this [`CheckSeamOrder`] partitions.
    WrongAggregate { expected: String, got: String },
}

impl std::fmt::Display for CheckSeamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckSeamError::InvalidRepoRoot => {
                write!(f, "invalid canonical Git repository root")
            }
            CheckSeamError::InvalidCommitRoot => {
                write!(f, "invalid canonical Git commit root")
            }
            CheckSeamError::InvalidContext => {
                write!(f, "invalid canonical CI check context")
            }
            CheckSeamError::ResultCommitMismatch => {
                write!(f, "ci.result commit does not match its canonical root")
            }
            CheckSeamError::WrongType(t) => {
                write!(f, "not a ci.check.updated event: type_={t}")
            }
            CheckSeamError::WrongAggregate { expected, got } => {
                write!(f, "wrong aggregate: expected {expected}, got {got}")
            }
        }
    }
}

impl std::error::Error for CheckSeamError {}

// ---------------------------------------------------------------------------
// 3. The ci.result rollup signal + the durable wait_for_signal substrate
// ---------------------------------------------------------------------------

/// The overall rollup verdict of a `ci.result` signal (§4.12). The Bus carries this opaque (it is
/// CI-derived); the closed `success`/`failure` set matches the contract-9.4 / X-1 signal payload
/// (`overall: success|failure`). The merge-queue workflow reads it; the Bus does not interpret it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiOverall {
    /// All required contexts passed (the merge queue may proceed — *Git* decides "required").
    Success,
    /// At least one required context failed/errored (Git dequeues the PR with a reason).
    Failure,
}

/// **The `ci.result` rollup signal payload** (§4.12 / X-1 / contract 9.4):
/// `{ commit_oid, overall, contexts, idem_token }`. CI-DERIVED and distinct from the per-context
/// `ci.check.updated` events — the single signal the merge-queue durable workflow's
/// `wait_for_signal` parks on. The Bus carries it as the signal payload + provides the idempotent
/// wake substrate; it does NOT decide the merge (Git does).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiResult {
    /// The commit the rollup is for.
    pub commit_oid: String,
    /// The CI-derived overall verdict.
    pub overall: CiOverall,
    /// The per-context names rolled up (PII-free context identifiers, never payload bytes).
    pub contexts: Vec<String>,
    /// The idempotency token CI mints for this rollup — the `idem_key` the merge-queue workflow's
    /// `wait_for_signal("ci.result", idem_key=<merge_attempt_id>)` keys on. A double-delivery of
    /// the SAME `idem_token` is ONE wake (contract 9.1 idempotency, OQ-F).
    pub idem_token: String,
}

/// The `ci.result` rollup signal subject (§4.12 / X-1): the bare canonical Git commit root. The
/// rollup and its checks therefore share the same commit identity and aggregate partition.
pub fn ci_result_subject(commit: &CheckCommit) -> ArtifactRef {
    commit.root.clone()
}

/// **Build the canonical [`EventDraft`] for the `ci.result` rollup signal (the PRODUCER leg, EB-27/
/// M4).** CI derives the rollup from the per-context `ci.check.updated` facts and emits it **via the
/// outbox** (BUS-2) on the same `(repo, commit_oid)` aggregate, so the rollup is per-aggregate
/// ordered *after* the checks it rolls up. The merge-queue durable workflow waits on it via
/// `wait_for_signal("ci.result", idem_key=<merge_attempt_id>)` (contract 9.4); the Bus carries the
/// signal opaque + provides the idempotent wake substrate ([`CiResultWaitSubstrate`]).
///
/// The Bus owns ONLY the envelope conformance (`type_ = ci.result`, the commit-anchored aggregate,
/// the signal payload shape `{commit_oid, overall, contexts, idem_token}`). It does NOT decide the
/// `overall` verdict (CI derives it from required-context status) nor the merge (Git's gate). The
/// `aggregate` deliberately matches [`check_aggregate`] so the rollup linearises after its checks on
/// the one per-commit partition (§2.2/§4.12).
pub fn ci_result_draft(
    commit: &CheckCommit,
    result: &CiResult,
) -> Result<EventDraft, CheckSeamError> {
    if commit.commit_oid != result.commit_oid {
        return Err(CheckSeamError::ResultCommitMismatch);
    }
    Ok(EventDraft {
        type_: EventType(CI_RESULT.to_string()),
        subject: ci_result_subject(commit),
        aggregate: check_aggregate(commit),
        // The rollup signal payload `{commit_oid, overall, contexts, idem_token}` — PII-free
        // (references-not-payloads: context identifiers + a commit OID, never log bytes).
        payload: serde_json::to_value(result).expect("CiResult serialises (closed shape)"),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    })
}

/// **Derive the `ci.result` rollup from the per-aggregate-ordered checks (the PRODUCER's roll-up,
/// EB-27/M4).** Given the CURRENT per-context status (after Git/CI's `run_attempt` supersession has
/// collapsed each context to one row) and the set of `required` contexts the merge gate enforces,
/// compute the `overall` verdict: [`CiOverall::Success`] iff EVERY required context succeeded,
/// [`CiOverall::Failure`] otherwise (a required context failed/errored/missing). The Bus offers this
/// as a deterministic helper the CI producer uses to mint the rollup it emits via [`ci_result_draft`]
/// — it does NOT decide *which* contexts are required (Git's gate does; the producer passes the set).
///
/// `current` maps a context name → did-it-succeed (the post-supersession truth). `required` is the
/// gate's required-context set. `idem_token` is the merge-attempt idempotency key CI mints. The
/// verdict is pure/deterministic (same inputs → same rollup), so a re-derivation after a redelivery
/// is byte-identical (the idempotent wake then absorbs the duplicate).
pub fn rollup_ci_result(
    commit_oid: &str,
    current: &BTreeMap<String, bool>,
    required: &[String],
    idem_token: &str,
) -> CiResult {
    // Success iff every REQUIRED context is present AND succeeded; a missing required context is a
    // failure-to-gate (never an implicit pass — the gate stays closed until CI reports it).
    let overall = if required
        .iter()
        .all(|ctx| current.get(ctx).copied().unwrap_or(false))
    {
        CiOverall::Success
    } else {
        CiOverall::Failure
    };
    // The rolled-up context set is the required gate set, in deterministic (sorted) order so the
    // rollup is byte-stable across re-derivations (the idempotent-wake precondition).
    let mut contexts: Vec<String> = required.to_vec();
    contexts.sort();
    CiResult {
        commit_oid: commit_oid.to_string(),
        overall,
        contexts,
        idem_token: idem_token.to_string(),
    }
}

/// **The durable `wait_for_signal("ci.result", idem_key)` substrate (the Bus's narrow 9.4 half).**
///
/// The merge-queue is a durable workflow (`myelin-flow`, contract 9.1) that
/// `wait_for_signal("ci.result", idem_key=<merge_attempt_id>)` — it holds NO runtime while CI runs
/// for hours and wakes when the rollup arrives. The Bus's role is the *signal substrate*: a
/// [`CiResult`] delivered (possibly **doubly**, by the at-least-once transport) for an `idem_key`
/// **wakes the waiter EXACTLY ONCE**. `DurableExecutor::signal` is idempotent on `idem_key` (X-1 /
/// OQ-F: a double-delivery is one wake, not two).
///
/// This is NOT the durable engine (that is `myelin-flow`, the named floor P-FLOW-04). It is the
/// in-cell substrate the engine's `wait_for_signal` resolves through — the idempotent delivery
/// keyed on `idem_key`, modelled here exactly as the engine will see it. A waiter `park`s an
/// `idem_key`; a `deliver` of a [`CiResult`] for that `idem_key` resolves it once; a redelivery is
/// a no-op return of the already-delivered result.
#[derive(Debug, Default)]
pub struct CiResultWaitSubstrate {
    /// `idem_key → CiResult` — the delivered (and thus resolved) results, keyed on the idempotency
    /// key the workflow waits on. A re-delivery finds the key already present and is a no-op (one
    /// wake). A `BTreeMap` so the state is deterministic/ordered for tests.
    delivered: BTreeMap<String, CiResult>,
    /// `idem_key → wake_count` — how many times each waiter was WOKEN. The substrate's correctness
    /// invariant is `wake_count <= 1` for every key (a double-delivered signal wakes once). The
    /// D-11 / unit drill asserts this is exactly `1` after a doubly-delivered `ci.result`.
    wakes: BTreeMap<String, u32>,
}

impl CiResultWaitSubstrate {
    /// The signal NAME the merge-queue workflow waits on
    /// (`wait_for_signal("ci.result", idem_key)`) — pinned to the NAMED `ci.result` token
    /// ([`CI_RESULT`]), never a literal, so the workflow and the substrate agree by construction.
    pub const SIGNAL_NAME: &'static str = CI_RESULT;

    /// A fresh substrate with no parked waits.
    pub fn new() -> CiResultWaitSubstrate {
        CiResultWaitSubstrate::default()
    }

    /// **`wait_for_signal("ci.result", idem_key)`** — park a wait on `idem_key`. Returns the
    /// already-delivered [`CiResult`] IF the signal arrived before the park (the at-least-once
    /// transport can deliver before the workflow re-leases), `None` if the wait is genuinely
    /// pending. Parking is the workflow holding NO runtime (contract 9.4) — modelled here as the
    /// substrate remembering the key it must resolve.
    ///
    /// A park is idempotent: re-parking an already-resolved key returns the SAME result and does
    /// NOT increment the wake count (one wake per key, ever).
    pub fn wait_for_signal(&mut self, idem_key: &str) -> Option<CiResult> {
        self.delivered.get(idem_key).cloned()
    }

    /// **Deliver a `ci.result` signal** for its `idem_token` (= the `idem_key` a workflow waits
    /// on). Returns [`WakeOutcome::Woke`] the FIRST time a given `idem_key` is delivered (the waiter
    /// wakes once) and [`WakeOutcome::Duplicate`] on every subsequent re-delivery of the SAME
    /// `idem_key` (the at-least-once double-delivery is absorbed — one wake, not two). The
    /// substrate is idempotent on `idem_key` by construction (X-1 / contract 9.1 / OQ-F).
    pub fn deliver(&mut self, result: CiResult) -> WakeOutcome {
        let key = result.idem_token.clone();
        if self.delivered.contains_key(&key) {
            // At-least-once double-delivery of the same rollup — one wake, not two.
            return WakeOutcome::Duplicate;
        }
        self.delivered.insert(key.clone(), result);
        *self.wakes.entry(key).or_insert(0) += 1;
        WakeOutcome::Woke
    }

    /// How many times the waiter on `idem_key` was woken. The substrate's invariant is `<= 1`; the
    /// drill asserts it is exactly `1` after a (single or doubly-delivered) `ci.result`.
    pub fn wake_count(&self, idem_key: &str) -> u32 {
        self.wakes.get(idem_key).copied().unwrap_or(0)
    }

    /// Has the wait on `idem_key` been resolved (a `ci.result` delivered)?
    pub fn is_resolved(&self, idem_key: &str) -> bool {
        self.delivered.contains_key(idem_key)
    }
}

/// The outcome of delivering a `ci.result` to the [`CiResultWaitSubstrate`] — a loud, typed
/// distinction between a real wake and an absorbed at-least-once duplicate (never a silent drop;
/// the duplicate is observable so a drill can assert "exactly one wake").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeOutcome {
    /// The first delivery for this `idem_key` — the waiter woke (exactly once).
    Woke,
    /// A re-delivery of a `idem_key` already resolved — absorbed (one wake total), not a second.
    Duplicate,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Actor, CorrelationId, DataRole as DR, EventId, Timestamp, Visibility as Vis};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    /// A stub CI system principal (the producer of check facts).
    fn ci_actor() -> Actor {
        Actor(Principal::stub(
            PrincipalId("ci".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        ))
    }

    fn commit(repo_id: &str, commit_oid: &str) -> CheckCommit {
        CheckCommit::parse(&ArtifactRef(format!(
            "myelin://acme/git/commit/{repo_id}:{commit_oid}"
        )))
        .expect("canonical test commit")
    }

    /// Build a `ci.check.updated` envelope for `(repo, commit_oid, context)` at a given outbox
    /// `seq`, carrying an opaque `run_attempt`-stamped CheckStatus payload (the Bus does not
    /// interpret it; the test stamps `run_attempt` so the assertions can read it back).
    fn check_env(
        repo_id: &str,
        commit_oid: &str,
        context: &str,
        run_attempt: u64,
        state: &str,
    ) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("evt-{context}-{run_attempt}")),
            type_: EventType(CI_CHECK_UPDATED.to_string()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: ci_actor(),
            subject: check_subject(&commit(repo_id, commit_oid), context).unwrap(),
            aggregate: check_aggregate(&commit(repo_id, commit_oid)),
            causation_id: None,
            correlation_id: CorrelationId(format!("corr-{commit_oid}")),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DR::Controller,
            visibility: Vis::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            // The CI-owned CheckStatus — OPAQUE to the Bus; the test stamps run_attempt + state.
            payload: serde_json::json!({ "context": context, "run_attempt": run_attempt, "state": state }),
        }
    }

    /// The envelope `subject` + `aggregate` follow the §4.12 grammar exactly.
    #[test]
    fn envelope_conformance_subject_and_aggregate_grammar() {
        let commit = commit("core", "abc123");
        let draft = check_updated_draft(
            &commit,
            "build",
            serde_json::json!({ "state": "success", "run_attempt": 1 }),
        )
        .unwrap();
        assert_eq!(draft.type_.0, "ci.check.updated");
        assert_eq!(
            draft.subject.0, "myelin://acme/git/commit/core:abc123#check-build",
            "subject = canonical commit root plus check sub-anchor"
        );
        assert_eq!(
            draft.aggregate.0, "commit:core:abc123",
            "aggregate = (repo, commit_oid) — all contexts for one commit share it"
        );
        // The Bus carries the CheckStatus OPAQUE — it round-trips untouched.
        assert_eq!(draft.payload["run_attempt"], 1);
        assert!(!draft.contains_personal_data, "references-not-payloads");
    }

    #[test]
    fn encoded_repository_delimiters_round_trip_and_form_a_stream_subject() {
        let repo_root =
            ArtifactRef("myelin://acme/git/repo/repo%2Ewith%3Adelimiter%25value".into());
        let commit = CheckCommit::from_repo_root(&repo_root, "blake3:dead.beef").unwrap();
        assert_eq!(commit.repo_id(), "repo.with:delimiter%value");
        assert_eq!(commit.commit_oid(), "blake3:dead.beef");
        assert_eq!(
            commit.root().0,
            "myelin://acme/git/commit/repo%2Ewith%3Adelimiter%25value:blake3%3Adead%2Ebeef"
        );
        let draft = check_updated_draft(&commit, "build", serde_json::json!({})).unwrap();
        let envelope = EventEnvelope {
            event_id: EventId("encoded-commit".into()),
            type_: draft.type_,
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: ci_actor(),
            subject: draft.subject,
            aggregate: draft.aggregate,
            causation_id: None,
            correlation_id: CorrelationId("encoded-commit".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DR::Controller,
            visibility: Vis::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload: draft.payload,
        };
        crate::StreamSubject::of(&envelope).expect("encoded aggregate is transport-safe");

        for malformed in [
            "myelin://acme/git/repo/repo.with",
            "myelin://acme/git/repo/repo%2ewith",
            "myelin://acme/git/repo/repo%41",
        ] {
            assert!(
                CheckCommit::from_repo_root(&ArtifactRef(malformed.into()), "deadbeef").is_err()
            );
        }
    }

    /// All contexts for one commit share the SAME aggregate (the per-commit ordering partition);
    /// a different commit is a different aggregate (cross-aggregate order not promised).
    #[test]
    fn all_contexts_of_a_commit_share_one_aggregate() {
        let a_build = check_aggregate(&commit("core", "deadbeef"));
        let a_test = check_aggregate(&commit("core", "deadbeef"));
        let a_other_commit = check_aggregate(&commit("core", "cafef00d"));
        assert_eq!(
            a_build, a_test,
            "build + test of one commit → one aggregate"
        );
        assert_ne!(
            a_build, a_other_commit,
            "a different commit → a different aggregate"
        );
    }

    /// **D-11 core: interleaved + LATE arrivals stay per-aggregate ordered.** Emit
    /// `ci.check.updated` for one `(repo, commit_oid)` across contexts AND re-run attempts, deliver
    /// them out of `seq` order (interleaved + a late lower `seq`), and assert the consumed order is
    /// the per-aggregate `seq` order — so Git's `run_attempt` supersession is well-defined.
    #[test]
    fn interleaved_and_late_arrivals_stay_per_aggregate_ordered() {
        let mut order = CheckSeamOrder::new(&commit("core", "deadbeef"));

        // The outbox assigned these seqs (state-change order): build#1=1, test#1=2, build#2=3
        // (a re-run of build), test#2=4. They ARRIVE interleaved + out of order.
        let build1 = check_env("core", "deadbeef", "build", 1, "failure");
        let test1 = check_env("core", "deadbeef", "test", 1, "success");
        let build2 = check_env("core", "deadbeef", "build", 2, "success"); // a re-run
        let test2 = check_env("core", "deadbeef", "test", 2, "success");

        // Deliver them in a SCRAMBLED arrival order: 3, 1, 4, 2 (interleaved across contexts; the
        // higher build re-run arrives before the lower one).
        assert!(order.ingest(&build2, 3).unwrap());
        assert!(order.ingest(&build1, 1).unwrap());
        assert!(order.ingest(&test2, 4).unwrap());
        assert!(order.ingest(&test1, 2).unwrap());

        // The CONSUMED order is the per-aggregate seq order (1,2,3,4) — NOT the arrival order.
        let seqs: Vec<u64> = order.in_order().iter().map(|c| c.seq).collect();
        assert_eq!(
            seqs,
            vec![1, 2, 3, 4],
            "consumed in per-aggregate seq order, not arrival order"
        );
        assert_eq!(order.ordering_gap(), 0, "contiguous: no gap, fully ordered");

        // Because the order is preserved, Git's monotonic run_attempt supersession is well-defined:
        // build's run_attempt sequence over the order is [1 (seq1), 2 (seq3)] — monotonic; the
        // CURRENT build status is the highest run_attempt (the re-run, success). The Bus does NOT
        // evaluate this — it GUARANTEES the order that makes it deterministic.
        let build_attempts: Vec<u64> = order
            .in_order()
            .iter()
            .filter(|c| c.subject.0.ends_with("check-build"))
            .map(|c| c.check_status["run_attempt"].as_u64().unwrap())
            .collect();
        assert_eq!(
            build_attempts,
            vec![1, 2],
            "build attempts appear in monotonic order"
        );
    }

    /// **D-11: a stale lower-attempt re-delivery is DROPPABLE — the aggregate order is preserved.**
    /// The at-least-once transport re-delivers an already-seen `seq`; it is absorbed (no
    /// reordering, no duplicate in the consumed order), so Git can drop the stale re-delivery and
    /// its supersession stays deterministic.
    #[test]
    fn stale_redelivery_is_droppable_order_preserved() {
        let mut order = CheckSeamOrder::new(&commit("core", "deadbeef"));
        let build1 = check_env("core", "deadbeef", "build", 1, "failure");
        let build2 = check_env("core", "deadbeef", "build", 2, "success");

        assert!(order.ingest(&build1, 1).unwrap(), "first build is new");
        assert!(order.ingest(&build2, 2).unwrap(), "the re-run is new");

        // The at-least-once transport RE-DELIVERS the stale lower attempt (seq 1) AFTER the higher
        // one. The ordering layer absorbs it (no-op) — the consumed order is unchanged.
        assert!(
            !order.ingest(&build1, 1).unwrap(),
            "the stale re-delivery is a duplicate, absorbed"
        );

        let seqs: Vec<u64> = order.in_order().iter().map(|c| c.seq).collect();
        assert_eq!(
            seqs,
            vec![1, 2],
            "order preserved across the stale re-delivery (droppable)"
        );
        assert_eq!(order.ordering_gap(), 0);
    }

    /// The ordering gap reads the count of in-flight (not-yet-delivered) seqs below the high-water
    /// mark — `0` means a contiguous, fully-ordered partition; a positive gap means ops are in
    /// flight (NOT lost — the at-least-once transport redelivers them; the D-11 drill asserts the
    /// gap closes to 0).
    #[test]
    fn ordering_gap_counts_in_flight_seqs() {
        let mut order = CheckSeamOrder::new(&commit("core", "deadbeef"));
        // seqs 1 and 3 delivered; 2 is still in flight.
        order
            .ingest(&check_env("core", "deadbeef", "build", 1, "success"), 1)
            .unwrap();
        order
            .ingest(&check_env("core", "deadbeef", "lint", 1, "success"), 3)
            .unwrap();
        assert_eq!(order.ordering_gap(), 1, "seq 2 in flight → gap of 1");
        // the in-flight op arrives — the gap closes (0 lost).
        order
            .ingest(&check_env("core", "deadbeef", "test", 1, "success"), 2)
            .unwrap();
        assert_eq!(
            order.ordering_gap(),
            0,
            "every op delivered → contiguous, 0 gap"
        );
        assert_eq!(order.observed_seqs(), vec![1, 2, 3]);
    }

    /// A foreign type or aggregate is REJECTED — a wrong event can never silently corrupt the
    /// partition order Git's supersession rule depends on.
    #[test]
    fn ingest_rejects_foreign_type_and_aggregate() {
        let mut order = CheckSeamOrder::new(&commit("core", "deadbeef"));

        let mut wrong_type = check_env("core", "deadbeef", "build", 1, "success");
        wrong_type.type_ = EventType("git.ref.updated".into());
        assert!(matches!(
            order.ingest(&wrong_type, 1),
            Err(CheckSeamError::WrongType(_))
        ));

        let wrong_agg = check_env("core", "cafef00d", "build", 1, "success");
        assert!(matches!(
            order.ingest(&wrong_agg, 1),
            Err(CheckSeamError::WrongAggregate { .. })
        ));
    }

    /// **The wait_for_signal substrate wakes EXACTLY ONCE on a doubly-delivered ci.result.** The
    /// at-least-once transport delivers the same rollup twice for one `idem_key`; the waiter wakes
    /// once (contract 9.1 idempotency on `idem_key`, X-1 / OQ-F).
    #[test]
    fn wait_for_signal_wakes_exactly_once_on_double_delivery() {
        let mut sub = CiResultWaitSubstrate::new();
        let idem = "merge-attempt-42";

        // The merge-queue workflow parks: wait_for_signal("ci.result", idem_key) — pending.
        assert_eq!(
            sub.wait_for_signal(idem),
            None,
            "no signal yet → genuinely pending"
        );
        assert!(!sub.is_resolved(idem));

        let result = CiResult {
            commit_oid: "deadbeef".into(),
            overall: CiOverall::Success,
            contexts: vec!["build".into(), "test".into()],
            idem_token: idem.into(),
        };

        // CI delivers the rollup — the waiter WOKE.
        assert_eq!(
            sub.deliver(result.clone()),
            WakeOutcome::Woke,
            "first delivery wakes"
        );
        // The at-least-once transport DOUBLY delivers the SAME rollup — absorbed, NOT a second wake.
        assert_eq!(
            sub.deliver(result.clone()),
            WakeOutcome::Duplicate,
            "re-delivery is one wake"
        );
        // And a THIRD redelivery is still absorbed.
        assert_eq!(sub.deliver(result.clone()), WakeOutcome::Duplicate);

        assert_eq!(
            sub.wake_count(idem),
            1,
            "EXACTLY ONE wake on a doubly-delivered ci.result"
        );
        assert!(sub.is_resolved(idem));
        // A subsequent park returns the delivered result (the workflow re-leases + reads it).
        assert_eq!(sub.wait_for_signal(idem), Some(result));
    }

    /// Distinct `idem_key`s (distinct merge attempts) wake independently — the substrate is keyed
    /// per merge_attempt_id, so one merge attempt's signal does not wake another's waiter.
    #[test]
    fn distinct_idem_keys_wake_independently() {
        let mut sub = CiResultWaitSubstrate::new();
        let r1 = CiResult {
            commit_oid: "c1".into(),
            overall: CiOverall::Success,
            contexts: vec!["build".into()],
            idem_token: "attempt-1".into(),
        };
        let r2 = CiResult {
            commit_oid: "c2".into(),
            overall: CiOverall::Failure,
            contexts: vec!["build".into()],
            idem_token: "attempt-2".into(),
        };
        assert_eq!(sub.deliver(r1), WakeOutcome::Woke);
        assert_eq!(sub.deliver(r2), WakeOutcome::Woke);
        assert_eq!(sub.wake_count("attempt-1"), 1);
        assert_eq!(sub.wake_count("attempt-2"), 1);
        assert_eq!(
            sub.wake_count("attempt-3"),
            0,
            "an unparked key has no wake"
        );
    }

    /// The substrate's signal name is the NAMED `ci.result` token (the workflow waits on this).
    #[test]
    fn signal_name_is_the_ci_result_token() {
        assert_eq!(CiResultWaitSubstrate::SIGNAL_NAME, "ci.result");
    }

    /// The `ci.result` payload serialises to the §4.12 frozen shape
    /// `{ commit_oid, overall, contexts, idem_token }` with `overall` in `snake_case`.
    #[test]
    fn ci_result_payload_shape() {
        let result = CiResult {
            commit_oid: "deadbeef".into(),
            overall: CiOverall::Success,
            contexts: vec!["build".into(), "test".into()],
            idem_token: "merge-7".into(),
        };
        let v = serde_json::to_value(&result).unwrap();
        assert_eq!(v["commit_oid"], "deadbeef");
        assert_eq!(
            v["overall"], "success",
            "overall is snake_case success|failure"
        );
        assert_eq!(v["contexts"], serde_json::json!(["build", "test"]));
        assert_eq!(v["idem_token"], "merge-7");
        // Round-trip.
        let back: CiResult = serde_json::from_value(v).unwrap();
        assert_eq!(back, result);
    }

    /// **The PRODUCER leg: the `ci.result` rollup draft follows the §4.12 grammar.** CI emits the
    /// rollup on the SAME `(repo, commit_oid)` aggregate as the checks it rolls up, so the rollup
    /// linearises after them on the one per-commit partition; the subject is the `ci-result` `#sub`.
    #[test]
    fn ci_result_draft_follows_the_grammar() {
        let result = CiResult {
            commit_oid: "abc123".into(),
            overall: CiOverall::Success,
            contexts: vec!["build".into(), "test".into()],
            idem_token: "merge-7".into(),
        };
        let commit = commit("core", "abc123");
        let draft = ci_result_draft(&commit, &result).unwrap();
        assert_eq!(draft.type_.0, "ci.result");
        assert_eq!(
            draft.subject.0, "myelin://acme/git/commit/core:abc123",
            "ci.result uses the canonical commit root (there is no ci-result sub kind)"
        );
        assert_eq!(
            draft.aggregate.0, "commit:core:abc123",
            "the rollup shares the per-commit aggregate so it linearises after its checks"
        );
        assert_eq!(
            draft.aggregate,
            check_aggregate(&commit),
            "the rollup aggregate IS the checks' aggregate"
        );
        assert!(!draft.contains_personal_data, "references-not-payloads");
        // The payload round-trips to the frozen signal shape.
        let back: CiResult = serde_json::from_value(draft.payload).unwrap();
        assert_eq!(back, result);
    }

    /// **The PRODUCER's rollup derivation: success iff EVERY required context succeeded.** The Bus
    /// offers a deterministic verdict helper; a missing or failing required context closes the gate
    /// (never an implicit pass). The Bus does NOT decide WHICH contexts are required.
    #[test]
    fn rollup_ci_result_is_success_iff_all_required_pass() {
        let required = vec!["build".to_string(), "test".to_string()];

        // All required succeeded → Success.
        let mut current = BTreeMap::new();
        current.insert("build".to_string(), true);
        current.insert("test".to_string(), true);
        current.insert("lint".to_string(), false); // non-required failure → irrelevant
        let r = rollup_ci_result("abc123", &current, &required, "merge-1");
        assert_eq!(r.overall, CiOverall::Success);
        assert_eq!(
            r.contexts,
            vec!["build".to_string(), "test".to_string()],
            "the rolled-up set is the required gate set, sorted (byte-stable)"
        );

        // A required context failed → Failure.
        let mut current = BTreeMap::new();
        current.insert("build".to_string(), true);
        current.insert("test".to_string(), false);
        let r = rollup_ci_result("abc123", &current, &required, "merge-1");
        assert_eq!(r.overall, CiOverall::Failure);

        // A required context MISSING (CI hasn't reported it) → the gate stays closed (Failure).
        let mut current = BTreeMap::new();
        current.insert("build".to_string(), true);
        let r = rollup_ci_result("abc123", &current, &required, "merge-1");
        assert_eq!(
            r.overall,
            CiOverall::Failure,
            "a missing required context never implicitly passes"
        );
    }

    /// **The rollup is DETERMINISTIC (the idempotent-wake precondition).** Re-deriving the rollup
    /// from the same inputs is byte-identical, so a re-delivery of the same rollup carries the same
    /// `idem_token` + payload → the substrate absorbs it as one wake.
    #[test]
    fn rollup_ci_result_is_deterministic() {
        let required = vec!["test".to_string(), "build".to_string()]; // unsorted input
        let mut current = BTreeMap::new();
        current.insert("build".to_string(), true);
        current.insert("test".to_string(), true);
        let a = rollup_ci_result("abc123", &current, &required, "merge-1");
        let b = rollup_ci_result("abc123", &current, &required, "merge-1");
        assert_eq!(a, b, "same inputs → byte-identical rollup");
        assert_eq!(
            a.contexts,
            vec!["build".to_string(), "test".to_string()],
            "contexts always sorted regardless of input order"
        );
    }
}
