//! # `check_status` — the X-1 Git↔CI CheckStatus **consumer contract** (GIT-P6 / P-232)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md`
//! §1.1 (the X-1 consumer: apply `run_attempt` supersession into the `check_status` projection),
//! `00-overview.md` §0.1 Δ1/Δ2/Δ3 (the frozen `CheckStatus` fact keyed `(commit_oid, context)`, the
//! `required`-set policy is Git's, the untrusted-fork-is-neutral-until-endorsed rule).
//! **Reconciliation:** `05-refined-shared-systems-architecture/00-reconciliation-decisions.md` X-1
//! (the most load-bearing cross-subsystem seam — the frozen `CheckStatus` shape + the monotonic
//! `run_attempt` supersession + the merge gate). **Contract:** index row **5.9** (the Git↔CI
//! CheckStatus seam — CI produces, Git is the consumer + gate).
//!
//! ## What this prompt (GIT-P6 / P-232) ships — and what it deliberately does NOT
//! This is the **DECLARED, COMPILING, NOT-YET-LIVE consumer seam module**. It declares — as a
//! compiling contract surface against the M2-frozen 5.9 shape:
//!
//! 1. **The typed [`CheckStatus`] consumer view** — the Git-side decode of the `CheckStatus` fact CI
//!    carries OPAQUE over the Bus (`myelin_events::check_seam` carries it as a `serde_json::Value`;
//!    Git is the consumer that names + interprets the fields). Keyed half: `(commit_oid, context)`.
//! 2. **The [`CheckStatusRow`] projection-table schema** keyed `(tenant, commit_oid, context)` — the
//!    Git-owned mirror the merge gate reads (exactly ONE current row per key). The DDL is
//!    [`CHECK_STATUS_PROJECTION_DDL`]; no migration is RUN here (the live store + migration is the
//!    GIT-P20 consumer leg).
//! 3. **The monotonic `run_attempt` supersession rule** — [`supersedes`] / [`CheckStatusProjection`]:
//!    an incoming fact supersedes the stored row IFF its `run_attempt >= stored.run_attempt`
//!    (re-run supersession is monotonic on the attempt COUNTER, never on wall-clock `completed_at` —
//!    clocks are not authority). A *lower* attempt arriving late is dropped (the at-least-once
//!    transport makes this drop mandatory).
//! 4. **The `required`-set policy shape** — [`RequiredSetPolicy`] / [`gate_outcome`]: Git's
//!    branch-protection policy decides WHICH contexts gate a target ref (CI reports facts, Git
//!    decides which facts gate, X-1 / Δ1). An `untrusted_fork` success is **neutral for gating**
//!    until endorsed/re-run-trusted (Δ3 — the poisoned-pipeline defence).
//!
//! ## The consumer leg is now LIVE (EB-26 / P-246, M3) — and what is still a FLOOR
//! As of **EB-26 (P-246, M3)** the consumer leg is **WIRED**: [`CheckStatusConsumer`] is an
//! idempotent [`myelin_events::EventHandler`] over the Bus's per-aggregate-ordered `ci.check.updated`
//! carriage (the §4.2 idempotent template — idempotent on `event_id`, applying the monotonic
//! `run_attempt` supersession). The Bus's narrow carriage half (envelope conformance + per-aggregate
//! ordering + the durable `ci.result` wait substrate) lives in `myelin_events::check_seam` (EB-24) —
//! this module is the GIT CONSUMER half that decodes that ordered carriage; it does NOT re-define the
//! Bus carriage (EI-01 §7: extend/reconcile, never duplicate — the opaque payload
//! `myelin_events::check_seam::OrderedCheck::check_status` decodes to exactly the [`CheckStatus`]
//! declared here).
//!
//! **FLOOR (named — VISION §3 / roadmap §5 seam-floor register).** Two legs remain:
//! 1. **The real CI PRODUCER** (CI emits `ci.check.updated` + the rollup `ci.result`) lands
//!    **EB-27/M4** — it makes the seam END-TO-END (the **M4 co-gate GIT-D10 / CI-D8**). In M3 the
//!    consumer is proven against a synthetic `ci.check.updated` emitter (the carriage drill fixture).
//! 2. **The store-backed projection** — the real `check_status` table + the migration + the same-tx
//!    `consumer_dedup` write — is the data-layer follow-on; here the projection is the in-memory
//!    [`CheckStatusProjection`] (the SEMANTICS the live store implements byte-for-byte).
//!
//! ## Acyclic-by-construction (EI-02 §3)
//! Git **never synchronously calls CI**. It reads its OWN [`CheckStatusProjection`] (a mirror of CI's
//! facts), and reads [`TrustTier`] off the fact — it never recomputes trust (CI stamps it from run
//! provenance + the `read & !is_untrusted_fork` ABAC edge, X-1). CI emits, Git reads — the dependency
//! is one-way.

use myelin_events::taxonomy::new_tokens::CI_CHECK_UPDATED;
use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use myelin_tenancy::{ArtifactRef, TenantId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// 1. The frozen CheckStatus fact (the consumer view) — contract 5.9 / X-1
// ---------------------------------------------------------------------------

/// A content-addressed git commit OID — the immutable commit a check ran against. Half of the
/// projection key `(commit_oid, context)`. An opaque, PII-free identifier (the sha bytes), never a
/// payload. Git owns this id grammar (the commit sha is already its stable canonical key — Δ7).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GitOid(pub String);

/// The `CheckContext` — the KEY half of `(commit_oid, context)` (X-1). `{provider, name}`, e.g.
/// `{ci, "build"}`, `{ci, "test/unit"}`, `{external, "sonarcloud"}`. The pair `(commit_oid, context)`
/// is the merge-gate truth key; the projection holds exactly one current row per `(tenant, key)`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CheckContext {
    /// `ci` for a Myelin-CI-produced check, `external` for a third-party status (e.g. sonarcloud).
    pub provider: CheckProvider,
    /// The context name (`build`, `test/unit`, …) — a PII-free identifier, never log bytes.
    pub name: String,
}

impl CheckContext {
    /// A Myelin-CI context (`{ci, name}`).
    pub fn ci(name: impl Into<String>) -> CheckContext {
        CheckContext {
            provider: CheckProvider::Ci,
            name: name.into(),
        }
    }

    /// An external-status context (`{external, name}`) — e.g. a third-party scanner.
    pub fn external(name: impl Into<String>) -> CheckContext {
        CheckContext {
            provider: CheckProvider::External,
            name: name.into(),
        }
    }
}

/// The producer class of a [`CheckContext`] (frozen 5.9): `ci` (Myelin CI) or `external` (a
/// third-party status). Serialises `snake_case` so the projection column + the decoded fact agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckProvider {
    /// A check produced by a Myelin CI run.
    Ci,
    /// An external/third-party status (e.g. sonarcloud), surfaced as a check.
    External,
}

/// The check lifecycle state (frozen 5.9): the closed set
/// `queued | in_progress | success | failure | error | neutral | cancelled`. Only `success` (with an
/// acceptable trust posture) can satisfy a `required` context; everything else fails/blocks/neutralises
/// the gate. Serialises `snake_case`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    /// The check is queued (not yet running) — not a terminal state; the gate treats it as pending.
    Queued,
    /// The check is running — pending.
    InProgress,
    /// The check passed — the ONLY state that can satisfy a `required` context (with trust).
    Success,
    /// The check failed (a test/build failure) — blocks a `required` context.
    Failure,
    /// The check errored (infra/runner fault, distinct from a clean failure) — blocks.
    Error,
    /// Explicitly neutral — recorded, does not satisfy and does not block.
    Neutral,
    /// The check was cancelled — does not satisfy.
    Cancelled,
}

impl CheckState {
    /// Is this a SUCCESS (the only state that — with an acceptable trust posture — satisfies a
    /// `required` context)? `Neutral` is NOT a success (it is recorded but never gating).
    pub fn is_success(self) -> bool {
        matches!(self, CheckState::Success)
    }

    /// Is this a terminal state (the check has reached a verdict)? `queued`/`in_progress` are not.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            CheckState::Success
                | CheckState::Failure
                | CheckState::Error
                | CheckState::Neutral
                | CheckState::Cancelled
        )
    }
}

/// The trust tier (frozen 5.9): `trusted | untrusted_fork` — **stamped by CI** from the run's
/// provenance + the `read & !is_untrusted_fork` ABAC edge. **Git reads it off the fact and never
/// recomputes it** (X-1). An `untrusted_fork` success is NEUTRAL for gating until a maintainer
/// endorses the run via `check(subject, approve_untrusted_ci, repo)` OR the context is re-run
/// `trusted` (Δ3 — the poisoned-pipeline defence). Serialises `snake_case`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    /// The run executed trusted code (a non-fork PR, or an endorsed/re-run-trusted fork run).
    Trusted,
    /// The run executed untrusted contributor code (a fork PR) — its success cannot self-satisfy a
    /// `required` context; it is neutral until endorsed/re-run-trusted.
    UntrustedFork,
}

/// A humanised reference (template_key + args) — the Notif-humanised summary (NOTIF-1 / contract
/// 7.3). The `CheckStatus.summary` is a `HumanisedRef`, **never a raw string**, so the PR checks
/// panel never renders a CI-supplied raw `"build failed"` — it renders a `(template_key, args)`
/// humanised at the backend. Git carries this opaque (the keys are CI's `summary` vocabulary) and
/// hands it to Notif's `humanise`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanisedRef {
    /// The humanisation template key (e.g. `ci.check.failure`). Notif resolves it; Git never renders
    /// a raw string for the checks panel (NOTIF-1).
    pub template_key: String,
    /// The template args (`name → value`) the humanisation fills. PII-free identifiers/labels.
    pub args: BTreeMap<String, String>,
}

/// A wall-clock timestamp column on the projection (the started/completed columns). RFC3339 string —
/// **not the supersession authority** (the `run_attempt` counter is; clocks are not authority, X-1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timestamp(pub String);

/// **The frozen 5.9 `CheckStatus` fact — the Git CONSUMER view.** CI is the producer/owner; this is
/// the typed Git-side decode of the struct CI carries OPAQUE in the `ci.check.updated` payload
/// (`myelin_events::check_seam` carries it as a `serde_json::Value`; Git names + interprets it).
///
/// Keyed `(commit_oid, context)`. The projection holds exactly one current row per `(tenant, key)`;
/// an incoming fact supersedes the stored row by monotonic [`run_attempt`](CheckStatus::run_attempt)
/// (see [`supersedes`]). This is references-not-payloads: `run` / `details_ref` are `ArtifactRef`s
/// (the producing run + the jump-to-failure sub-anchor), never log bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckStatus {
    /// The partition key (from token, EI-02 §1) — every projection row is tenant-scoped.
    pub tenant: TenantId,
    /// The repo the check ran in (`myelin://<tenant>/git/repo/<id>`).
    pub repo: ArtifactRef,
    /// The content-addressed commit the check ran against — the KEY half of `(commit_oid, context)`.
    pub commit_oid: GitOid,
    /// The check context — the other KEY half. `(commit_oid, context)` is the merge-gate truth key.
    pub context: CheckContext,
    /// The check lifecycle state.
    pub state: CheckState,
    /// Does this context block the merge gate for its target ref? **This is CI's REPORT of its own
    /// view; the AUTHORITY on which contexts gate is GIT's [`RequiredSetPolicy`]** (Δ1 — CI reports,
    /// Git decides). Carried for completeness; the gate consults Git's policy, not this bool.
    pub required: bool,
    /// The producing CI run (`myelin://<tenant>/ci/run/<id>`) — for supersession provenance +
    /// drill-down. A reference, never the run's bytes.
    pub run: ArtifactRef,
    /// The monotonically-increasing attempt counter per `(commit_oid, context)`. **A higher attempt
    /// supersedes a lower one** (the supersession authority — NOT wall-clock). A late lower attempt
    /// is dropped (the at-least-once transport makes the drop mandatory).
    pub run_attempt: u32,
    /// The trust tier — **stamped by CI, read by Git, never recomputed** (X-1). Gates whether a
    /// success can self-satisfy a `required` context (Δ3).
    pub trust_tier: TrustTier,
    /// The jump-to-failure sub-anchor (`myelin://<tenant>/ci/run/<id>#step-<n>`, OQ-D). Git renders
    /// it as a link into CI's run view — it NEVER reads CI's DB to resolve it (§2 of the glue doc).
    pub details_ref: ArtifactRef,
    /// The Notif-humanised summary — a `(template_key, args)` pair, **never a raw string** (NOTIF-1).
    pub summary: HumanisedRef,
    /// When the run started (RFC3339). Not the supersession authority.
    pub started_at: Timestamp,
    /// When the run completed, if it has (RFC3339). Not the supersession authority.
    pub completed_at: Option<Timestamp>,
    /// The reserve/settle bookend (11.7): a check is **not "final" until `cost_settled = true`** — the
    /// merge gate may treat a not-yet-settled check as still-in-progress (the X-1 cost-gate field).
    pub cost_settled: bool,
}

impl CheckStatus {
    /// The projection key `(commit_oid, context)` for this fact — the merge-gate truth key. The
    /// projection holds exactly one current row per `(tenant, key)`.
    pub fn key(&self) -> CheckKey {
        CheckKey {
            commit_oid: self.commit_oid.clone(),
            context: self.context.clone(),
        }
    }
}

/// The `(commit_oid, context)` projection key — the merge-gate truth key (X-1). The
/// `check_status` projection holds **exactly one current row per `(tenant, key)`** (last-writer-wins
/// by monotonic `run_attempt`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CheckKey {
    /// The commit the check ran against.
    pub commit_oid: GitOid,
    /// The check context.
    pub context: CheckContext,
}

// ---------------------------------------------------------------------------
// 2. The check_status projection-table schema (keyed (commit_oid, context))
// ---------------------------------------------------------------------------

/// **The Git-owned `check_status` projection-table schema** keyed `(tenant, commit_oid, context)` —
/// the mirror the merge gate reads (Δ1). Exactly ONE current row per key (last-writer-wins by
/// monotonic `run_attempt`). This is the row SHAPE the GIT-P20 consumer leg materialises; no
/// migration is RUN here (the live table + the idempotent consumer is GIT-P20 — the seam-floor).
///
/// The row mirrors the [`CheckStatus`] fact 1:1 plus the `run_attempt` column the supersession rule
/// reads. `required_by_policy` is **Git's** computed gating decision (from [`RequiredSetPolicy`]),
/// distinct from the CI-reported `required` bool on the fact (Δ1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckStatusRow {
    /// The partition key — every projection row is tenant-scoped.
    pub tenant: TenantId,
    /// The commit half of the key.
    pub commit_oid: GitOid,
    /// The context half of the key.
    pub context: CheckContext,
    /// The current state.
    pub state: CheckState,
    /// The producing run.
    pub run: ArtifactRef,
    /// The attempt counter of the CURRENT row — the supersession high-water mark per key.
    pub run_attempt: u32,
    /// The trust tier of the current row (read off the fact, never recomputed).
    pub trust_tier: TrustTier,
    /// The jump-to-failure sub-anchor.
    pub details_ref: ArtifactRef,
    /// The humanised summary.
    pub summary: HumanisedRef,
    /// The cost-settled bookend.
    pub cost_settled: bool,
}

impl CheckStatusRow {
    /// Materialise a projection row from a decoded [`CheckStatus`] fact (the consumer's apply step).
    pub fn from_fact(fact: &CheckStatus) -> CheckStatusRow {
        CheckStatusRow {
            tenant: fact.tenant.clone(),
            commit_oid: fact.commit_oid.clone(),
            context: fact.context.clone(),
            state: fact.state,
            run: fact.run.clone(),
            run_attempt: fact.run_attempt,
            trust_tier: fact.trust_tier,
            details_ref: fact.details_ref.clone(),
            summary: fact.summary.clone(),
            cost_settled: fact.cost_settled,
        }
    }

    /// The `(commit_oid, context)` key of this row.
    pub fn key(&self) -> CheckKey {
        CheckKey {
            commit_oid: self.commit_oid.clone(),
            context: self.context.clone(),
        }
    }
}

/// **The DDL for the `check_status` projection table** (Δ1 / X-1) — keyed `(tenant, commit_oid,
/// context_provider, context_name)`, exactly one current row per key. DECLARED here (the contract
/// surface); the migration is **RUN in GIT-P20** (the named seam-floor — no live store here). The
/// `run_attempt` column is the supersession high-water mark; `trust_tier` is read off the fact.
///
/// This is a DERIVED projection (contract 2.6/11.5): rebuilt from CI's facts, never restored — the
/// `replay` path asks the Bus to `reindex` `ci.check.updated` for the scope (the glue doc §4).
pub const CHECK_STATUS_PROJECTION_DDL: &str = "\
CREATE TABLE check_status (\
  tenant            text  NOT NULL,\
  commit_oid        text  NOT NULL,\
  context_provider  text  NOT NULL,\
  context_name      text  NOT NULL,\
  state             text  NOT NULL,\
  run_ref           text  NOT NULL,\
  run_attempt       bigint NOT NULL,\
  trust_tier        text  NOT NULL,\
  details_ref       text  NOT NULL,\
  summary_key       text  NOT NULL,\
  summary_args      jsonb NOT NULL,\
  cost_settled      boolean NOT NULL,\
  required_by_policy boolean NOT NULL,\
  PRIMARY KEY (tenant, commit_oid, context_provider, context_name))";

// ---------------------------------------------------------------------------
// 3. The monotonic run_attempt supersession rule (X-1)
// ---------------------------------------------------------------------------

/// **The monotonic `run_attempt` supersession rule (X-1).** An `incoming` fact supersedes the
/// `stored` row IFF `incoming.run_attempt >= stored.run_attempt`. Supersession is monotonic on the
/// attempt COUNTER, **never on wall-clock `completed_at`** (clocks are not authority). A *lower*
/// attempt arriving late returns `false` (dropped) — the at-least-once transport makes the drop
/// mandatory (a re-delivered stale fact must not clobber a newer one).
///
/// `>=` (not `>`) because a re-delivery of the SAME attempt is idempotent — applying it again is a
/// no-op overwrite with identical data (the consumer's `consumer_dedup` ledger is the other half).
pub fn supersedes(incoming_attempt: u32, stored_attempt: u32) -> bool {
    incoming_attempt >= stored_attempt
}

/// **The `check_status` projection — the Git-owned consumer mirror.** Holds exactly ONE current row
/// per `(commit_oid, context)` (tenant-scoped), applying the monotonic [`supersedes`] rule on every
/// incoming fact. This is the in-memory shape of the projection the merge gate reads; the LIVE,
/// store-backed, idempotent-on-`event_id` consumer is GIT-P20 (the seam-floor) — this declares the
/// supersession SEMANTICS the live consumer implements, proven by the unit drills below.
#[derive(Debug, Default, Clone)]
pub struct CheckStatusProjection {
    /// `(commit_oid, context) → current row`. A `BTreeMap` so iteration is deterministic (for the
    /// gate scan + tests). One current row per key (last-writer-wins by `run_attempt`).
    rows: BTreeMap<CheckKey, CheckStatusRow>,
}

impl CheckStatusProjection {
    /// A fresh, empty projection.
    pub fn new() -> CheckStatusProjection {
        CheckStatusProjection::default()
    }

    /// **Apply one decoded [`CheckStatus`] fact** under the monotonic supersession rule. Returns the
    /// [`ApplyOutcome`] (loud, never a silent drop): `Superseded` if the fact became the current row,
    /// `DroppedStale` if it was a late lower-attempt re-delivery (the row is unchanged). This is the
    /// SEMANTICS the GIT-P20 live consumer implements over the ordered Bus carriage.
    pub fn apply(&mut self, fact: &CheckStatus) -> ApplyOutcome {
        let key = fact.key();
        match self.rows.get(&key) {
            Some(stored) if !supersedes(fact.run_attempt, stored.run_attempt) => {
                // A late LOWER attempt — dropped (the stored newer row wins). Mandatory under
                // at-least-once delivery: a stale re-delivery must never clobber a newer fact.
                ApplyOutcome::DroppedStale {
                    incoming_attempt: fact.run_attempt,
                    current_attempt: stored.run_attempt,
                }
            }
            _ => {
                // No stored row, or the incoming attempt is >= the stored one — it becomes current.
                self.rows.insert(key, CheckStatusRow::from_fact(fact));
                ApplyOutcome::Superseded {
                    current_attempt: fact.run_attempt,
                }
            }
        }
    }

    /// The current row for a `(commit_oid, context)` key, if any.
    pub fn current(&self, key: &CheckKey) -> Option<&CheckStatusRow> {
        self.rows.get(key)
    }

    /// All current rows for a commit (across contexts) — the set the merge gate scans for a target
    /// commit. Deterministic order (the `BTreeMap` guarantee).
    pub fn rows_for_commit<'a>(
        &'a self,
        commit_oid: &'a GitOid,
    ) -> impl Iterator<Item = &'a CheckStatusRow> + 'a {
        self.rows
            .iter()
            .filter(move |(k, _)| &k.commit_oid == commit_oid)
            .map(|(_, v)| v)
    }

    /// The number of current rows (one per `(commit_oid, context)` key).
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Is the projection empty?
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// The outcome of applying a [`CheckStatus`] fact to the [`CheckStatusProjection`] — a loud, typed
/// distinction between a fact that became the current row and a stale lower-attempt re-delivery that
/// was dropped (never a silent drop; the drop is observable so the GIT-P20 consumer + drills assert
/// it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The fact superseded (or seeded) the current row — its `run_attempt` is now the high-water mark.
    Superseded {
        /// The attempt counter that is now current for the key.
        current_attempt: u32,
    },
    /// A late LOWER-attempt fact — dropped; the stored (newer) row is unchanged.
    DroppedStale {
        /// The (lower) attempt of the dropped incoming fact.
        incoming_attempt: u32,
        /// The (higher) attempt of the row that stayed current.
        current_attempt: u32,
    },
}

// ---------------------------------------------------------------------------
// 4. The required-set policy shape (Git decides which contexts gate) — Δ1/Δ3
// ---------------------------------------------------------------------------

/// **Git's branch-protection `required`-set policy (Δ1 / X-1).** The set of contexts that MUST be
/// green (with an acceptable trust posture) for a merge into a target ref. **This is Git's policy —
/// CI reports facts, Git decides which facts gate.** Declared here as the policy SHAPE the merge gate
/// (GIT-P20) evaluates; the live branch-protection store + the per-ref ruleset is GIT-P20.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RequiredSetPolicy {
    /// The contexts that gate the target ref. A merge is blocked unless every one has a CURRENT row
    /// with `state = success` AND an acceptable trust posture (trusted or fork-endorsed, Δ3).
    pub required_contexts: Vec<CheckContext>,
}

impl RequiredSetPolicy {
    /// A policy requiring exactly the given contexts.
    pub fn requiring(required_contexts: Vec<CheckContext>) -> RequiredSetPolicy {
        RequiredSetPolicy { required_contexts }
    }

    /// Does this policy gate on `context`?
    pub fn requires(&self, context: &CheckContext) -> bool {
        self.required_contexts.contains(context)
    }
}

/// Is a current row an **acceptable satisfaction** of a `required` context (Δ3, the poisoned-pipeline
/// defence)? A `success` satisfies IFF its trust posture is acceptable: `trusted`, OR
/// `untrusted_fork` that has been ENDORSED (`fork_endorsed = true`, the maintainer
/// `check(subject, approve_untrusted_ci, repo)` flow — X-1). An un-endorsed `untrusted_fork` success
/// is **neutral for gating** (recorded, but cannot self-satisfy — a fork must not turn its own gate
/// green by running attacker-controlled CI config).
pub fn is_acceptable_satisfaction(row: &CheckStatusRow, fork_endorsed: bool) -> bool {
    if !row.state.is_success() {
        return false;
    }
    match row.trust_tier {
        TrustTier::Trusted => true,
        // An untrusted-fork success is neutral until endorsed (or re-run trusted, which flips the
        // tier to Trusted on a later fact — handled by supersession).
        TrustTier::UntrustedFork => fork_endorsed,
    }
}

/// **The merge-gate outcome for a target commit (X-1 / Δ1 / Δ3) — the consumer's gate evaluation.**
/// Given the [`RequiredSetPolicy`] for the target ref + the current projection rows for the commit +
/// the set of fork-endorsed contexts (the maintainer endorsements via `approve_untrusted_ci`),
/// returns the [`GateOutcome`]. **Git reads its OWN projection — it never synchronously calls CI**
/// (EI-02 §3, acyclic). This is the gate LOGIC the GIT-P20 merge gate fires; here it is the declared,
/// proven shape (no live merge, no event consumer wired — the seam-floor).
pub fn gate_outcome(
    policy: &RequiredSetPolicy,
    projection: &CheckStatusProjection,
    commit_oid: &GitOid,
    endorsed_contexts: &[CheckContext],
) -> GateOutcome {
    let mut unmet: Vec<CheckContext> = Vec::new();
    for ctx in &policy.required_contexts {
        let key = CheckKey {
            commit_oid: commit_oid.clone(),
            context: ctx.clone(),
        };
        match projection.current(&key) {
            None => unmet.push(ctx.clone()),
            Some(row) => {
                let endorsed = endorsed_contexts.contains(ctx);
                if !is_acceptable_satisfaction(row, endorsed) {
                    unmet.push(ctx.clone());
                }
            }
        }
    }
    if unmet.is_empty() {
        GateOutcome::AllRequiredGreen
    } else {
        GateOutcome::Blocked { unmet }
    }
}

/// The merge-gate outcome (the Git-owned decision "may this PR merge?"). Loud + typed: either all
/// required contexts are satisfied, or the specific unmet contexts are surfaced (humanised into the
/// PR checks panel by Notif — never a raw string).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateOutcome {
    /// Every required context has a current `success` row with an acceptable trust posture — the gate
    /// is GREEN (the merge may proceed; the merge queue's `ci.result` wait is the runtime half).
    AllRequiredGreen,
    /// At least one required context is missing / not-green / un-endorsed-fork — the gate BLOCKS.
    Blocked {
        /// The specific contexts that are not satisfied (missing, failing, or un-endorsed fork).
        unmet: Vec<CheckContext>,
    },
}

// ---------------------------------------------------------------------------
// 5. THE LIVE CONSUMER LEG (EB-26 / P-246, M3) — Git's check_status projection
//    wired as an idempotent EventHandler over the Bus's per-aggregate-ordered carriage.
// ---------------------------------------------------------------------------

/// The subject whitelist for the check-status consumer — exactly `ci.check.updated` (rule 3 — NEVER
/// `*`). Git consumes ONLY this token from CI; it never subscribes to a wildcard (a poison/slow CI
/// subject must never head-of-line-block git's other consumers). Pinned to the NAMED Bus token. A
/// `const` `SubjectPattern` cannot embed an owned `String`, so the whitelist is built once into a
/// `'static` slice the [`EventHandler::subjects`] signature requires (a one-time `OnceLock` init).
fn check_status_subjects() -> &'static [SubjectPattern] {
    use std::sync::OnceLock;
    static SUBJECTS: OnceLock<Vec<SubjectPattern>> = OnceLock::new();
    SUBJECTS
        .get_or_init(|| vec![SubjectPattern(CI_CHECK_UPDATED.to_string())])
        .as_slice()
}

/// **The LIVE check-seam consumer leg (EB-26 / P-246, M3).** Git's `check_status` projection wired as
/// an idempotent [`EventHandler`] over the Bus's per-aggregate-ordered `ci.check.updated` carriage
/// (the §4.2 idempotent consumer template, §4.12 / contract 5.9). This is the consumer half of the
/// X-1 seam going LIVE in M3 (the producer half — CI's real emit — lands EB-27/M4, making the seam
/// end-to-end).
///
/// What the runtime around it guarantees (the Bus's [`myelin_events::consumer::Consumer`] template,
/// EB-05) and what THIS handler guarantees:
/// - **Idempotent on `event_id`** — the Bus consumer runtime's `consumer_dedup` ledger skips a
///   redelivered `event_id` (rule 1), so `handle` is invoked at most once per event; AND this
///   handler's `apply` is ITSELF idempotent (a re-applied same-attempt fact is a no-op overwrite,
///   [`supersedes`] is `>=`) — belt and braces, so even a dedup-ledger miss never double-effects.
/// - **Per-aggregate ordered on `(repo, commit_oid)`** — the Bus delivers the per-context facts in
///   per-aggregate `seq` order ([`myelin_events::check_seam::CheckSeamOrder`]), so the monotonic
///   `run_attempt` supersession this handler applies is well-defined regardless of physical arrival
///   order (a late lower-attempt re-delivery is [`ApplyOutcome::DroppedStale`], never a clobber).
/// - **The Bus's role stays NARROW** — it carries the envelope + the order; this handler is the
///   GIT-OWNED projection logic (decode the opaque payload → apply supersession). The Bus does not
///   name a `CheckStatus` field; Git decodes it here.
///
/// The projection is interior-mutable (a `Mutex`) because [`EventHandler::handle`] takes `&self` (the
/// Bus runtime owns the handler and may deliver concurrently); the lock is held only for the
/// O(1) apply. The live store-backed projection (the real `check_status` table + the same-tx dedup
/// write) is the data-layer follow-on; the SEMANTICS — idempotent-on-`event_id`, per-aggregate
/// ordered, monotonic supersession — are exactly what this handler proves.
#[derive(Debug, Default)]
pub struct CheckStatusConsumer {
    /// The Git-owned projection the handler mutates (one current row per `(commit_oid, context)`).
    projection: Mutex<CheckStatusProjection>,
    /// The count of facts that became current (superseded/seeded the row) — for the carriage drill's
    /// telemetry assertion.
    applied: Mutex<u64>,
    /// The count of late lower-attempt facts dropped as stale — the supersession's observable half.
    dropped_stale: Mutex<u64>,
}

impl CheckStatusConsumer {
    /// A fresh consumer with an empty projection.
    pub fn new() -> CheckStatusConsumer {
        CheckStatusConsumer::default()
    }

    /// Decode the opaque `ci.check.updated` payload (the `serde_json::Value` the Bus carries OPAQUE,
    /// `myelin_events::check_seam::OrderedCheck::check_status`) into the typed Git [`CheckStatus`]
    /// consumer view. A malformed payload is a LOUD [`Reason`] (the handler dead-letters it — never a
    /// silent drop, never the wrong shape into the projection).
    pub fn decode(payload: &serde_json::Value) -> Result<CheckStatus, Reason> {
        serde_json::from_value(payload.clone()).map_err(|e| {
            Reason(format!(
                "ci.check.updated payload is not a valid CheckStatus fact: {e}"
            ))
        })
    }

    /// Bind the opaque producer fact back to the envelope provenance before it can become Git-owned
    /// projection state. All comparisons use canonical typed derivation; payload strings never get
    /// to choose a tenant, subject, or ordering partition independently.
    fn validate_provenance(ev: &EventEnvelope, fact: &CheckStatus) -> Result<(), Reason> {
        let invalid = || {
            Reason("ci.check.updated envelope provenance does not match payload".into())
        };
        if fact.context.provider != CheckProvider::Ci || fact.tenant != ev.tenant {
            return Err(invalid());
        }
        let repo = myelin_refs::parse_scoped(&fact.repo.0).map_err(|_| invalid())?;
        if repo.artifact_ref != fact.repo
            || repo.subsystem != "git"
            || repo.type_ != "repo"
            || repo.sub.is_some()
            || repo.tenant != fact.tenant
        {
            return Err(invalid());
        }
        let commit = myelin_events::check_seam::CheckCommit::from_repo_root(
            &repo.artifact_ref,
            &fact.commit_oid.0,
        )
        .map_err(|_| invalid())?;
        let expected_subject =
            myelin_events::check_seam::check_subject(&commit, &fact.context.name)
                .map_err(|_| invalid())?;
        let expected_aggregate = myelin_events::check_seam::check_aggregate(&commit);
        if ev.subject != expected_subject || ev.aggregate != expected_aggregate {
            return Err(invalid());
        }
        Ok(())
    }

    /// Snapshot of the current projection (a clone) — the merge gate reads this. Cloned out under the
    /// lock so the gate scan never races a concurrent apply.
    pub fn projection(&self) -> CheckStatusProjection {
        self.projection
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The number of facts that became current (the supersession high-water advances).
    pub fn applied_count(&self) -> u64 {
        *self.applied.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The number of late lower-attempt facts dropped as stale (the at-least-once supersession drop).
    pub fn dropped_stale_count(&self) -> u64 {
        *self.dropped_stale.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl EventHandler for CheckStatusConsumer {
    /// The `*`-free whitelist: exactly `ci.check.updated` (rule 3). Git consumes only this from CI.
    fn subjects(&self) -> &'static [SubjectPattern] {
        check_status_subjects()
    }

    /// **Apply one delivered `ci.check.updated` to the `check_status` projection** (idempotent on
    /// `event_id` via the runtime; idempotent-by-construction here too). Decodes the opaque payload →
    /// applies the monotonic [`supersedes`] rule. A wrong-type event or a malformed payload is a LOUD
    /// [`HandleOutcome::NonRetryable`] (dead-letter — never silently dropped, never the wrong shape
    /// into the projection). A valid fact is applied and `Done` (the runtime acks + dedup-marks it).
    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        // The handler binds only `ci.check.updated`; a foreign type slipping through the whitelist is
        // a wiring bug — dead-letter it loudly (rule 5), never apply it.
        if ev.type_.0 != CI_CHECK_UPDATED {
            return HandleOutcome::NonRetryable(Reason(format!(
                "check_status consumer received a non-ci.check.updated event: {}",
                ev.type_.0
            )));
        }
        let fact = match Self::decode(&ev.payload) {
            Ok(f) => f,
            Err(reason) => return HandleOutcome::NonRetryable(reason),
        };
        if let Err(reason) = Self::validate_provenance(ev, &fact) {
            return HandleOutcome::NonRetryable(reason);
        }
        let mut proj = self.projection.lock().unwrap_or_else(|e| e.into_inner());
        match proj.apply(&fact) {
            ApplyOutcome::Superseded { .. } => {
                *self.applied.lock().unwrap_or_else(|e| e.into_inner()) += 1;
            }
            ApplyOutcome::DroppedStale { .. } => {
                *self.dropped_stale.lock().unwrap_or_else(|e| e.into_inner()) += 1;
            }
        }
        HandleOutcome::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(key: &str) -> HumanisedRef {
        HumanisedRef {
            template_key: key.into(),
            args: BTreeMap::new(),
        }
    }

    /// Build a decoded CheckStatus fact for a `(commit, context, attempt, state, trust)`.
    fn fact(
        commit: &str,
        ctx: CheckContext,
        attempt: u32,
        state: CheckState,
        trust: TrustTier,
    ) -> CheckStatus {
        CheckStatus {
            tenant: TenantId("acme".into()),
            repo: ArtifactRef("myelin://acme/git/repo/core".into()),
            commit_oid: GitOid(commit.into()),
            context: ctx,
            state,
            required: true,
            run: ArtifactRef("myelin://acme/ci/run/1".into()),
            run_attempt: attempt,
            trust_tier: trust,
            details_ref: ArtifactRef("myelin://acme/ci/run/1#step-3".into()),
            summary: h("ci.check.updated"),
            started_at: Timestamp("2026-06-21T00:00:00Z".into()),
            completed_at: Some(Timestamp("2026-06-21T00:01:00Z".into())),
            cost_settled: true,
        }
    }

    /// The frozen 5.9 shape serialises to exactly the X-1 field set — the compile/shape CDC half.
    #[test]
    fn check_status_serialises_to_the_frozen_5_9_shape() {
        let f = fact(
            "abc123",
            CheckContext::ci("build"),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        );
        let v = serde_json::to_value(&f).unwrap();
        // The KEY half + the closed-set fields.
        assert_eq!(v["commit_oid"], "abc123");
        assert_eq!(v["context"]["provider"], "ci");
        assert_eq!(v["context"]["name"], "build");
        assert_eq!(v["state"], "success");
        assert_eq!(v["trust_tier"], "trusted");
        assert_eq!(v["run_attempt"], 1);
        assert_eq!(v["required"], true);
        assert_eq!(v["cost_settled"], true);
        // references-not-payloads: run + details_ref are ArtifactRefs.
        assert_eq!(v["run"], "myelin://acme/ci/run/1");
        assert_eq!(v["details_ref"], "myelin://acme/ci/run/1#step-3");
        // summary is a (template_key, args) HumanisedRef — never a raw string.
        assert_eq!(v["summary"]["template_key"], "ci.check.updated");
        // Round-trips.
        let back: CheckStatus = serde_json::from_value(v).unwrap();
        assert_eq!(back, f);
    }

    /// **The decode of the Bus's OPAQUE payload is exactly this consumer view** — the consumer half
    /// reconciles with `myelin_events::check_seam`, which carries the CheckStatus as a
    /// `serde_json::Value`. A fact serialised by Git decodes back through `serde_json::Value` (the
    /// shape the Bus carries) into the same typed [`CheckStatus`]. No second struct — the opaque
    /// carriage decodes to THIS (EI-01 §7 reconciliation).
    #[test]
    fn opaque_bus_payload_decodes_to_the_consumer_view() {
        let f = fact(
            "abc123",
            CheckContext::ci("test"),
            2,
            CheckState::Failure,
            TrustTier::Trusted,
        );
        // Git produces the opaque payload shape the Bus carries...
        let opaque: serde_json::Value = serde_json::to_value(&f).unwrap();
        // ...and the consumer decodes that opaque value into the typed view.
        let decoded: CheckStatus = serde_json::from_value(opaque).unwrap();
        assert_eq!(decoded, f);
        assert_eq!(
            decoded.key(),
            CheckKey {
                commit_oid: GitOid("abc123".into()),
                context: CheckContext::ci("test"),
            }
        );
    }

    /// **The monotonic supersession rule** — `>=` supersedes, a lower attempt is dropped.
    #[test]
    fn supersession_is_monotonic_on_run_attempt() {
        assert!(supersedes(2, 1), "a higher attempt supersedes");
        assert!(
            supersedes(1, 1),
            "the same attempt is an idempotent re-apply (>=)"
        );
        assert!(
            !supersedes(1, 2),
            "a LOWER attempt is dropped (stale re-delivery)"
        );
    }

    /// **A late lower-attempt re-delivery is DROPPED** — the projection keeps the newer row.
    #[test]
    fn late_lower_attempt_is_dropped_not_applied() {
        let mut proj = CheckStatusProjection::new();
        let build = CheckContext::ci("build");

        // attempt 1 (failure) lands, then attempt 2 (a re-run, success) supersedes it.
        assert_eq!(
            proj.apply(&fact(
                "c1",
                build.clone(),
                1,
                CheckState::Failure,
                TrustTier::Trusted
            )),
            ApplyOutcome::Superseded { current_attempt: 1 }
        );
        assert_eq!(
            proj.apply(&fact(
                "c1",
                build.clone(),
                2,
                CheckState::Success,
                TrustTier::Trusted
            )),
            ApplyOutcome::Superseded { current_attempt: 2 }
        );

        // The at-least-once transport RE-DELIVERS the stale attempt 1 — it is DROPPED.
        assert_eq!(
            proj.apply(&fact(
                "c1",
                build.clone(),
                1,
                CheckState::Failure,
                TrustTier::Trusted
            )),
            ApplyOutcome::DroppedStale {
                incoming_attempt: 1,
                current_attempt: 2
            }
        );

        // The CURRENT row is still the attempt-2 success (the stale re-delivery did not clobber).
        let key = CheckKey {
            commit_oid: GitOid("c1".into()),
            context: build,
        };
        let row = proj.current(&key).unwrap();
        assert_eq!(row.run_attempt, 2);
        assert_eq!(row.state, CheckState::Success);
    }

    /// **One current row per (commit_oid, context)** — distinct contexts coexist; a re-apply of the
    /// same key supersedes in place (never a duplicate row).
    #[test]
    fn one_current_row_per_key() {
        let mut proj = CheckStatusProjection::new();
        proj.apply(&fact(
            "c1",
            CheckContext::ci("build"),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));
        proj.apply(&fact(
            "c1",
            CheckContext::ci("test"),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));
        assert_eq!(proj.len(), 2, "two distinct contexts → two rows");
        // Re-apply build at a higher attempt — supersedes in place, no new row.
        proj.apply(&fact(
            "c1",
            CheckContext::ci("build"),
            5,
            CheckState::Success,
            TrustTier::Trusted,
        ));
        assert_eq!(
            proj.len(),
            2,
            "supersession is in-place, never a duplicate row"
        );
    }

    /// **The required-set policy gate: all required green ⇒ merge may proceed.**
    #[test]
    fn gate_green_when_all_required_contexts_succeed_trusted() {
        let mut proj = CheckStatusProjection::new();
        let build = CheckContext::ci("build");
        let test = CheckContext::ci("test");
        proj.apply(&fact(
            "c1",
            build.clone(),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));
        proj.apply(&fact(
            "c1",
            test.clone(),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));

        let policy = RequiredSetPolicy::requiring(vec![build, test]);
        assert_eq!(
            gate_outcome(&policy, &proj, &GitOid("c1".into()), &[]),
            GateOutcome::AllRequiredGreen
        );
    }

    /// **A missing or failing required context BLOCKS, surfacing the unmet contexts.**
    #[test]
    fn gate_blocks_on_missing_or_failing_required_context() {
        let mut proj = CheckStatusProjection::new();
        let build = CheckContext::ci("build");
        let test = CheckContext::ci("test");
        // build succeeds; test FAILS; lint is required but MISSING.
        proj.apply(&fact(
            "c1",
            build.clone(),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));
        proj.apply(&fact(
            "c1",
            test.clone(),
            1,
            CheckState::Failure,
            TrustTier::Trusted,
        ));
        let lint = CheckContext::ci("lint");

        let policy = RequiredSetPolicy::requiring(vec![build, test.clone(), lint.clone()]);
        let outcome = gate_outcome(&policy, &proj, &GitOid("c1".into()), &[]);
        match outcome {
            GateOutcome::Blocked { unmet } => {
                assert!(unmet.contains(&test), "the failing context is unmet");
                assert!(unmet.contains(&lint), "the missing context is unmet");
                assert_eq!(unmet.len(), 2);
            }
            GateOutcome::AllRequiredGreen => panic!("must block"),
        }
    }

    /// **Δ3: an un-endorsed untrusted-fork success is NEUTRAL for gating** (the poisoned-pipeline
    /// defence) — the gate blocks until the maintainer endorses (or the context is re-run trusted).
    #[test]
    fn untrusted_fork_success_is_neutral_until_endorsed() {
        let mut proj = CheckStatusProjection::new();
        let build = CheckContext::ci("build");
        // A fork run: success, but trust_tier = untrusted_fork.
        proj.apply(&fact(
            "c1",
            build.clone(),
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));

        let policy = RequiredSetPolicy::requiring(vec![build.clone()]);
        let commit = GitOid("c1".into());

        // UN-endorsed → the fork success cannot self-satisfy → BLOCKED.
        assert_eq!(
            gate_outcome(&policy, &proj, &commit, &[]),
            GateOutcome::Blocked {
                unmet: vec![build.clone()]
            }
        );

        // The maintainer ENDORSES the context (the approve_untrusted_ci flow) → now GREEN.
        assert_eq!(
            gate_outcome(&policy, &proj, &commit, std::slice::from_ref(&build)),
            GateOutcome::AllRequiredGreen
        );
    }

    /// **A re-run under trust_tier = trusted flips the tier via supersession** — the other half of
    /// the Δ3 escape hatch ("approve and run"): a higher-attempt trusted fact supersedes the fork
    /// fact, so the gate goes green WITHOUT an explicit endorsement.
    #[test]
    fn rerun_trusted_supersedes_fork_and_greens_the_gate() {
        let mut proj = CheckStatusProjection::new();
        let build = CheckContext::ci("build");
        proj.apply(&fact(
            "c1",
            build.clone(),
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));
        // The maintainer re-runs the context trusted (attempt 2) — supersedes the fork fact.
        proj.apply(&fact(
            "c1",
            build.clone(),
            2,
            CheckState::Success,
            TrustTier::Trusted,
        ));

        let policy = RequiredSetPolicy::requiring(vec![build]);
        assert_eq!(
            gate_outcome(&policy, &proj, &GitOid("c1".into()), &[]),
            GateOutcome::AllRequiredGreen,
            "re-run trusted greens the gate with no explicit endorsement"
        );
    }

    /// The projection-table DDL is keyed `(tenant, commit_oid, context)` (the X-1 key) and carries
    /// the `run_attempt` supersession column + the trust tier.
    #[test]
    fn projection_ddl_is_keyed_on_commit_oid_and_context() {
        assert!(CHECK_STATUS_PROJECTION_DDL.contains("CREATE TABLE check_status"));
        assert!(CHECK_STATUS_PROJECTION_DDL
            .contains("PRIMARY KEY (tenant, commit_oid, context_provider, context_name)"));
        assert!(CHECK_STATUS_PROJECTION_DDL.contains("run_attempt"));
        assert!(CHECK_STATUS_PROJECTION_DDL.contains("trust_tier"));
    }

    /// A row materialises 1:1 from a fact (the consumer apply step).
    #[test]
    fn row_materialises_from_fact() {
        let f = fact(
            "c1",
            CheckContext::ci("build"),
            3,
            CheckState::Success,
            TrustTier::Trusted,
        );
        let row = CheckStatusRow::from_fact(&f);
        assert_eq!(row.key(), f.key());
        assert_eq!(row.run_attempt, 3);
        assert_eq!(row.trust_tier, TrustTier::Trusted);
        assert!(row.cost_settled);
    }
}
