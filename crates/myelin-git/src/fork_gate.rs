//! # `fork_gate` — the fork / trust-tier endorsement gate (the poisoned-pipeline defence, GIT-P22 / P-284, M3)
//!
//! **The poisoned-pipeline defence.** A check whose `trust_tier = untrusted_fork` (a PR from a fork, or
//! any run that executed untrusted contributor code) is **recorded but cannot satisfy a `required`
//! context by itself** — it is **neutral for gating** until a maintainer endorses the run via
//! `check(subject, approve_untrusted_ci, repo)` (the frozen ReBAC relation, contract 4.9) OR the context
//! is re-run trusted (a higher-attempt trusted fact supersedes the fork fact). **A fork PR must never
//! turn its own required gate green by running attacker-controlled CI config** (EI-02 §1 — the classic
//! poisoned-pipeline-execution attack; external-insights/01 §2: supply-chain is a non-negotiability).
//!
//! This module ships the TWO halves the merge gate ([`crate::merge_gate`]) consumed as explicit inputs
//! in GIT-P21 (P-282) and the FLOOR named there ("the LIVE `approve_untrusted_ci` fork-endorsement
//! resolution + the `fork:<pr_id>` cache confinement is GIT-P22"):
//!
//! 1. **The LIVE endorsement RESOLVER** ([`EndorsementResolver`]) — given the merge gate's required
//!    contexts + the PR head's [`crate::check_status::CheckStatusProjection`], it identifies the
//!    contexts that need endorsement (a CURRENT `untrusted_fork` success) and runs the maintainer's
//!    `check(subject, approve_untrusted_ci, repo)` through the LIVE [`crate::live_check::GitCheckGate`]
//!    (the GIT-P14 / P-275 `fork_endorsement_check`, a `Strong` zookie-stamped read-your-writes check).
//!    It PRODUCES the `endorsed_contexts: Vec<CheckContext>` set [`crate::merge_gate::evaluate_merge_gate`]
//!    consumes — the seam that closes the GIT-P21 floor. Endorsement is an ordinary RELATION check,
//!    **never bespoke trust logic** (Git reads `trust_tier` OFF the fact, never recomputes it — X-1).
//!
//! 2. **The `fork:<pr_id>` trust-scoped cache confinement** ([`TrustScope`] + [`ScopedCache`]) — the
//!    storage-tier half of the defence (contract 11.2 C4 / reconciliation §8). A fork-PR run's cache
//!    writes are confined to the `fork:<pr_id>` scope: a scope-key convention over the per-tenant
//!    [`myelin_storage::Cache`] so an `UntrustedFork` write **cannot reach the trusted cache scope** —
//!    it cannot poison a later trusted run by planting a value the trusted run would read. The scope is
//!    DERIVED from the run's trust posture, never caller-chosen, so a fork run is STRUCTURALLY unable to
//!    write a trusted-scope key.
//!
//! **Owning architecture:**
//! `planning/04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md` §6.3
//! (the fork / trust-tier gate — `untrusted_fork` neutral until endorsed; fork cache confined to
//! `fork:<pr_id>`); `00-overview.md` §0.1 Δ3 (untrusted-fork neutral-until-endorsed). **Reconciliation:**
//! `00-reconciliation-decisions.md` X-1 (the seam) + §8 (the trust-scoped cache namespaces — an
//! `UntrustedFork` write cannot reach the trusted cache scope). **Contracts:** index rows **5.9**
//! (fork-endorsement — an `untrusted_fork` success is neutral until `check(subject, approve_untrusted_ci,
//! repo)`), **11.2 C4** (the `fork:<pr_id>` trust-scoped cache), **4.9** (the `approve_untrusted_ci`
//! relation, wired live in GIT-P14).
//!
//! ## Acyclic-by-construction (EI-02 §3 / EI-01 §7)
//! Git **never synchronously calls CI**. The resolver reads Git's OWN projection (a mirror of CI's
//! facts) + runs a `check` against the LIVE ReBAC fragment (Identity) — it does NOT call CI to ask "is
//! this endorsed". The `endorsed_contexts` set it produces is fed to the merge gate, which REUSES the
//! per-context gate logic ([`crate::check_status::is_acceptable_satisfaction`]) — this module does not
//! re-define the gate, it produces its missing input (the GIT-P21 floor).
//!
//! ## FLOORS named (per the prompt)
//! - **The merge queue (the durable workflow that serialises the gate, parks on the rollup `ci.result`,
//!   exactly-once merge) is GIT-P23** — this resolver is the SYNCHRONOUS endorsement step the queue's
//!   per-PR workflow calls; the durable serialisation is GIT-P23.
//! - **The real CI PRODUCER** (CI emits the `untrusted_fork`-stamped `ci.check.updated`) lands EB-27/M4
//!   — the seam goes end-to-end at the **M4 co-gate GIT-D10 / CI-D8**. Here the fork facts are the
//!   synthetic producer's (the carriage drill fixture).

use crate::check_status::{
    is_acceptable_satisfaction, CheckContext, CheckKey, CheckStatusProjection, CheckStatusRow,
    GitOid, TrustTier,
};
use crate::live_check::{is_allow, GitCheckGate};
use myelin_identity::{IdentityService, Principal, Zookie};
use myelin_storage::{Cache, CacheError};
use myelin_substrate::Clock;
use myelin_tenancy::{ArtifactRef, TenantId};
use std::time::Duration;

// ===========================================================================
// PART A — the LIVE endorsement resolver (produces the merge gate's input)
// ===========================================================================

/// **Why a required context needs an endorsement decision (the resolver's classification).** Only a
/// CURRENT `untrusted_fork` SUCCESS is a candidate for endorsement — every other posture is already
/// decided by the merge gate's pure logic (a trusted success satisfies; a non-success / missing context
/// blocks regardless of endorsement). Surfacing this loudly keeps the resolver from running a redundant
/// (and security-sensitive) `check` for a context the gate would block anyway.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndorsementNeed {
    /// A CURRENT `untrusted_fork` success — this is the ONLY context an endorsement can flip. The
    /// resolver runs `check(subject, approve_untrusted_ci, repo)` for it.
    NeedsEndorsement,
    /// The context does not need (and cannot use) an endorsement: it is missing, not-green, or already
    /// trusted. The resolver skips the `check` (it changes nothing).
    NotApplicable,
}

/// Classify whether a required `context` for `head_oid` NEEDS an endorsement decision — i.e. its CURRENT
/// projection row is an `untrusted_fork` success that is not (yet) endorsed. This is the pure predicate
/// that gates the (security-sensitive, latency-bearing) live `check`: only an un-endorsed fork success
/// can be flipped by an endorsement, so the resolver runs `approve_untrusted_ci` for EXACTLY these.
///
/// Reads `trust_tier` OFF the fact (never recomputes it). A missing/not-green/already-trusted context is
/// [`EndorsementNeed::NotApplicable`] — endorsing it would change nothing (a fork must clear CI first;
/// a trusted success already satisfies).
pub fn endorsement_need(
    projection: &CheckStatusProjection,
    head_oid: &GitOid,
    context: &CheckContext,
) -> EndorsementNeed {
    let key = CheckKey { commit_oid: head_oid.clone(), context: context.clone() };
    match projection.current(&key) {
        Some(row) => endorsement_need_for_row(row),
        None => EndorsementNeed::NotApplicable,
    }
}

/// The [`endorsement_need`] decision over a single CURRENT row — exported so the live-store gate (which
/// fetches the row from Postgres) applies the IDENTICAL classification (the DB path and the in-memory
/// path can never drift). A row needs endorsement IFF it is a SUCCESS with `trust_tier = untrusted_fork`
/// (an un-endorsed fork success — the only thing an endorsement flips).
pub fn endorsement_need_for_row(row: &CheckStatusRow) -> EndorsementNeed {
    if row.state.is_success() && row.trust_tier == TrustTier::UntrustedFork {
        EndorsementNeed::NeedsEndorsement
    } else {
        EndorsementNeed::NotApplicable
    }
}

/// **The endorsing actor's context for a fork-endorsement resolution.** Bundles the four fields the
/// `approve_untrusted_ci` check needs (the maintainer subject, the repo object, the read-your-writes
/// zookie fence, and the revocation consult) so the resolver entry points carry ONE context value
/// instead of four positional args — and so the call site declares "this is the maintainer endorsing
/// against this repo at this fence" in one place. The `subject` is ALWAYS the trusted maintainer
/// clicking "endorse" — never the fork author (a fork author is not granted `approve_untrusted_ci`, so
/// the check Denies; the type does not — and cannot — let a fork author masquerade as the endorser).
pub struct Endorser<'a> {
    /// The maintainer clicking "endorse" — the `subject` of the `approve_untrusted_ci` check.
    pub subject: &'a Principal,
    /// The repo the endorsement relation is checked on (`check(subject, approve_untrusted_ci, repo)`).
    pub repo: &'a ArtifactRef,
    /// The read-your-writes fence — a just-granted `approve_untrusted_ci` tuple is visible to the
    /// `Strong` endorsement read stamped with this zookie (4.10).
    pub zookie: Zookie,
    /// The front door's revocation consult — a just-revoked maintainer cannot endorse, even on a stale
    /// cache (the new-enemy guard; a stale ALLOW never overrides a revoke).
    pub subject_revoked: bool,
}

/// **The LIVE fork-endorsement resolver (GIT-P22 — the GIT-P21 floor closed).** Produces the
/// `endorsed_contexts` set [`crate::merge_gate::evaluate_merge_gate`] consumes, by running the
/// maintainer's `check(subject, approve_untrusted_ci, repo)` through the LIVE [`GitCheckGate`] for each
/// required context whose CURRENT row is an un-endorsed `untrusted_fork` success.
///
/// **A fork can never self-green its required gate**: the endorsement check is a `check` against `repo`'s
/// `approve_untrusted_ci` relation — a relation only a MAINTAINER holds (an ordinary contributor / the
/// fork author is not granted it). The fork author submitting their own PR is NOT the `subject` of this
/// check; the subject is the trusted maintainer who clicks "endorse". So the resolver structurally cannot
/// produce an endorsement a fork author triggered.
///
/// Generic over the [`IdentityService`] (the real Id resolver in prod; a stub in drills) + the substrate
/// [`Clock`] — exactly the [`GitCheckGate`] seam (one engine, one cache, one check path; no second
/// implementation).
pub struct EndorsementResolver<'g, I: IdentityService, C: Clock> {
    /// The LIVE git→Id check gate (GIT-P14) — the `approve_untrusted_ci` resolution rides its
    /// `fork_endorsement_check` (a `Strong` zookie-stamped read-your-writes check that BYPASSES the
    /// fail-static cache, so a just-granted endorsement counts immediately).
    gate: &'g GitCheckGate<I, C>,
}

impl<'g, I: IdentityService, C: Clock> EndorsementResolver<'g, I, C> {
    /// Compose the resolver over the live check gate.
    pub fn new(gate: &'g GitCheckGate<I, C>) -> EndorsementResolver<'g, I, C> {
        EndorsementResolver { gate }
    }

    /// **Resolve the endorsed-context set for a PR head** (the merge gate's `endorsed_contexts` input).
    /// For each required context whose CURRENT projection row is an un-endorsed `untrusted_fork` success
    /// ([`EndorsementNeed::NeedsEndorsement`]), run the maintainer's `approve_untrusted_ci` check against
    /// `repo`. A context is ENDORSED IFF the check is an `Allow`. Returns the contexts the merge gate may
    /// treat as endorsed.
    ///
    /// `endorser` is the maintainer clicking "endorse" (the `subject` of the check — never the fork
    /// author). `zookie` is the read-your-writes fence (a just-granted `approve_untrusted_ci` tuple is
    /// visible to this `Strong` read). `subject_revoked` is the front door's revocation consult (a
    /// just-revoked maintainer cannot endorse, even on a stale cache).
    ///
    /// **0 forks green their own gate**: a context is added to the result ONLY on a maintainer `Allow`;
    /// a `Deny` (the fork author is not a maintainer) leaves it OUT, so the merge gate keeps blocking it
    /// (the neutral-for-gating rule holds). The endorsement is per-CONTEXT (a maintainer endorsing one
    /// run does not blanket-endorse a later re-pushed head — the projection row's identity is the
    /// `(head_oid, context)` key, and a new push mints a new head → a fresh un-endorsed fork fact).
    pub fn resolve_endorsed(
        &self,
        required: &[CheckContext],
        projection: &CheckStatusProjection,
        head_oid: &GitOid,
        endorser: &Endorser<'_>,
    ) -> Vec<CheckContext> {
        let mut endorsed = Vec::new();
        for ctx in required {
            // Only run the (security-sensitive) check for a context an endorsement can actually flip.
            if endorsement_need(projection, head_oid, ctx) == EndorsementNeed::NeedsEndorsement {
                let decision = self.gate.fork_endorsement_check(
                    endorser.subject,
                    endorser.repo,
                    endorser.zookie.clone(),
                    endorser.subject_revoked,
                );
                if is_allow(&decision) {
                    endorsed.push(ctx.clone());
                }
            }
        }
        endorsed
    }

    /// **Is a single required context satisfiable AFTER the endorsement resolution?** Convenience over
    /// the per-context pieces: classify the need; if it needs endorsement, run the check; then ask the
    /// REUSED [`is_acceptable_satisfaction`] (the merge gate's own predicate) whether the row now
    /// satisfies. Returns `true` IFF the context is green-and-current with an acceptable trust posture
    /// (trusted, or an endorsed fork). The single-context primitive a per-PR merge-queue step calls.
    pub fn context_satisfied(
        &self,
        projection: &CheckStatusProjection,
        head_oid: &GitOid,
        context: &CheckContext,
        endorser: &Endorser<'_>,
    ) -> bool {
        let key = CheckKey { commit_oid: head_oid.clone(), context: context.clone() };
        let Some(row) = projection.current(&key) else {
            return false; // a missing required context never satisfies (fail-closed).
        };
        let endorsed = match endorsement_need_for_row(row) {
            EndorsementNeed::NeedsEndorsement => {
                let d = self.gate.fork_endorsement_check(
                    endorser.subject,
                    endorser.repo,
                    endorser.zookie.clone(),
                    endorser.subject_revoked,
                );
                is_allow(&d)
            }
            EndorsementNeed::NotApplicable => false,
        };
        is_acceptable_satisfaction(row, endorsed)
    }
}

// ===========================================================================
// PART B — the fork:<pr_id> trust-scoped cache confinement (11.2 C4 / §8)
// ===========================================================================

/// **A trust scope for a cache write/read (contract 11.2 C4 / reconciliation §8 — the poisoned-cache
/// defence).** Every cache key a CI run touches is confined to its trust scope:
/// - [`TrustScope::Trusted`] — a trusted run (a non-fork PR, or an endorsed/re-run-trusted fork) reads
///   and writes the `trusted:` scope.
/// - [`TrustScope::Fork`] — an `untrusted_fork` run is confined to `fork:<pr_id>:` — it can read and
///   write ONLY its OWN PR-scoped namespace.
///
/// **The invariant (the defence):** an `UntrustedFork` write can NEVER produce a `trusted:`-scoped key,
/// so it cannot plant a value a later trusted run would read (the classic poisoned-cache attack). The
/// scope is DERIVED from the run's trust posture ([`TrustScope::for_run`]), never caller-chosen — a fork
/// run is STRUCTURALLY unable to address the trusted scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustScope {
    /// A trusted run — reads/writes the `trusted:` scope.
    Trusted,
    /// An `untrusted_fork` run — confined to `fork:<pr_id>:` (it shares the upstream's object DB but
    /// its CACHE is isolated to this PR; recon §8, arch §6.3). Carries the PR id the scope is keyed on.
    Fork {
        /// The PR id the fork run's cache is confined to (`fork:<pr_id>`). A PII-free identifier (the
        /// PR number / id), never a payload.
        pr_id: String,
    },
}

impl TrustScope {
    /// **Derive the trust scope for a run from its [`TrustTier`] (the structural confinement).** A
    /// `trusted` run gets [`TrustScope::Trusted`]; an `untrusted_fork` run gets
    /// [`TrustScope::Fork`] keyed on its `pr_id`. This is the ONLY way a scope is minted from a run —
    /// the trust tier comes OFF the fact (CI-stamped, never recomputed), so a fork run cannot
    /// caller-choose the trusted scope.
    pub fn for_run(trust_tier: TrustTier, pr_id: &str) -> TrustScope {
        match trust_tier {
            TrustTier::Trusted => TrustScope::Trusted,
            TrustTier::UntrustedFork => TrustScope::Fork { pr_id: pr_id.to_string() },
        }
    }

    /// The scope KEY PREFIX — `trusted` or `fork:<pr_id>`. Every cached key is `"<prefix>:<key>"`, so a
    /// fork's keys live in a disjoint keyspace from the trusted keys (the confinement is a prefix
    /// partition, exactly the recon §8 scope-key convention over the per-tenant cache). PII-free.
    pub fn key_prefix(&self) -> String {
        match self {
            TrustScope::Trusted => "trusted".to_string(),
            TrustScope::Fork { pr_id } => format!("fork:{pr_id}"),
        }
    }

    /// `true` IFF this is the trusted scope. The confinement-witness predicate (a fork scope is never
    /// trusted — the unit drill asserts `for_run(UntrustedFork, ..)` is never trusted).
    pub fn is_trusted(&self) -> bool {
        matches!(self, TrustScope::Trusted)
    }
}

/// **The trust-scoped cache (contract 11.2 C4) — the `fork:<pr_id>` confinement over the per-tenant
/// [`Cache`].** Wraps any [`myelin_storage::Cache`] (the in-memory floor in unit tests; the real
/// `ValkeyCache` behind the `integration` feature — the one-line swap holds, the confinement is a
/// key-prefix convention orthogonal to the backend). Every `get`/`set`/`delete` is prefixed with the
/// run's [`TrustScope`] key prefix, so:
/// - a trusted run reads/writes `<tenant>:trusted:<key>`;
/// - a fork run reads/writes `<tenant>:fork:<pr_id>:<key>`.
///
/// **0 fork writes reach the trusted scope**: a fork run carries a [`TrustScope::Fork`] (derived from
/// its CI-stamped trust tier), so its keys ALWAYS prefix `fork:<pr_id>:` — it physically cannot address
/// a `trusted:`-prefixed key. The unit + CDC drills assert: a fork write, then a trusted READ of the
/// SAME logical key, is a MISS (0 fork writes in the trusted scope).
pub struct ScopedCache<'c, K: Cache> {
    /// The underlying per-tenant cache (the §11.2 backend — fs↔Valkey one-line swap; the confinement is
    /// the prefix this wrapper adds, orthogonal to which backend).
    inner: &'c K,
    /// The trust scope this handle is confined to (derived from the run's trust tier — never
    /// caller-chosen for a fork).
    scope: TrustScope,
}

impl<'c, K: Cache> ScopedCache<'c, K> {
    /// Open a trust-scoped handle on `inner` confined to `scope`. The `scope` is the run's derived
    /// [`TrustScope`] ([`TrustScope::for_run`]) — a fork run hands a [`TrustScope::Fork`], so this
    /// handle is structurally unable to write a trusted key.
    pub fn new(inner: &'c K, scope: TrustScope) -> ScopedCache<'c, K> {
        ScopedCache { inner, scope }
    }

    /// The trust scope this handle is confined to.
    pub fn scope(&self) -> &TrustScope {
        &self.scope
    }

    /// The fully-scoped backing key for a logical `key` — `<scope_prefix>:<key>`. The ONE place the
    /// prefix is composed, so a `get` and a `set` of the same logical key under the same scope always
    /// agree, and two scopes never collide.
    fn scoped_key(&self, key: &str) -> String {
        format!("{}:{}", self.scope.key_prefix(), key)
    }

    /// Read `key` within this run's trust scope. A trusted handle reads only `trusted:`-prefixed keys; a
    /// fork handle reads only its `fork:<pr_id>:`-prefixed keys — so a fork's write is INVISIBLE to a
    /// trusted read of the same logical key (a clean MISS, the confinement).
    pub fn get(&self, tenant: &TenantId, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        self.inner.get(tenant, &self.scoped_key(key))
    }

    /// Write `key` within this run's trust scope for `ttl`. A fork run writes ONLY into its
    /// `fork:<pr_id>:` scope — it can NEVER write a `trusted:`-prefixed key (the poisoned-cache defence:
    /// the scope is derived from the CI-stamped trust tier, not caller-chosen).
    pub fn set(
        &self,
        tenant: &TenantId,
        key: &str,
        value: &[u8],
        ttl: Duration,
    ) -> Result<(), CacheError> {
        self.inner.set(tenant, &self.scoped_key(key), value, ttl)
    }

    /// Invalidate `key` within this run's trust scope.
    pub fn delete(&self, tenant: &TenantId, key: &str) -> Result<(), CacheError> {
        self.inner.delete(tenant, &self.scoped_key(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check_status::{CheckState, CheckStatus, HumanisedRef, Timestamp};
    use crate::live_check::GitCheckGate;
    use myelin_identity::{
        AuthzError, CaveatContext, Consistency, Credential, Decision, EffectivePolicy,
        IdentityService, ListObjectsResult, ObjectId, ObjectType, Permission, Precondition,
        Principal, PrincipalId, PrincipalKind, RewriteTrace, Result as IdResult, SubjectTree,
        TupleDelta, Zookie,
    };
    use myelin_storage::InMemoryCache;
    use myelin_substrate::{FailStaticThreshold, SystemClock};
    use myelin_tenancy::{Region, TenantId};
    use std::collections::{BTreeMap, HashMap};

    // ───────────────────────── fixtures ─────────────────────────

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
            summary: HumanisedRef { template_key: "ci.check.updated".into(), args: BTreeMap::new() },
            started_at: Timestamp("2026-06-22T00:00:00Z".into()),
            completed_at: Some(Timestamp("2026-06-22T00:01:00Z".into())),
            cost_settled: true,
        }
    }

    fn principal(id: &str) -> Principal {
        let mut p =
            Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, TenantId("acme".into()));
        p.region = Region("fr-par".into());
        p
    }

    fn maintainer() -> Principal {
        principal("maintainer-1")
    }

    const REPO: &str = "myelin://acme/git/repo/core";

    // A minimal IdentityService stub: `approve_untrusted_ci@repo` is Allow IFF the subject is in the
    // endorser allow-list (a maintainer). A fork author is NOT in the list → Deny (cannot self-endorse).
    struct StubId {
        endorsers: HashMap<String, Decision>,
    }
    impl StubId {
        fn new() -> Self {
            Self { endorsers: HashMap::new() }
        }
        fn allowing_endorser(mut self, principal_id: &str, repo: &str) -> Self {
            self.endorsers
                .insert(format!("approve_untrusted_ci@{principal_id}@{repo}"), Decision::Allow);
            self
        }
    }
    impl IdentityService for StubId {
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn check(
            &self,
            s: &Principal,
            permission: &Permission,
            object: &ArtifactRef,
            _at: &Consistency,
            _cav: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            Ok(self
                .endorsers
                .get(&format!("{}@{}@{}", permission.0, s.principal_id.0, object.0))
                .copied()
                .unwrap_or(Decision::Deny))
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _a: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_subjects(
            &self,
            _o: &ObjectId,
            _p: &Permission,
            _a: &Consistency,
        ) -> IdResult<SubjectTree> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ObjectId,
            _a: &Consistency,
        ) -> IdResult<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn write_tuples(
            &self,
            _d: &[TupleDelta],
            _p: Option<&Precondition>,
        ) -> IdResult<Zookie> {
            Ok(Zookie("zk-1".into()))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &myelin_identity::RunId,
            _d: &myelin_identity::DelegationCaveats,
            _t: &myelin_identity::FailStaticBound,
        ) -> IdResult<myelin_identity::RunToken> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn admit_fragment(
            &self,
            _f: &myelin_identity::NamespaceFragment,
        ) -> IdResult<myelin_identity::FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
    }

    fn threshold() -> FailStaticThreshold {
        FailStaticThreshold {
            status: "OPEN — LEGAL".into(),
            owner: "DPO / Legal".into(),
            static_max_secs: None,
            static_max_default_secs: 300,
            agent_token_ttl_secs: 60,
            constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
        }
    }

    fn gate(id: StubId) -> GitCheckGate<StubId, SystemClock> {
        // The resolver uses a Strong (cache-bypass) read, so the cache window is not load-bearing here
        // — only the construction bound must hold (agent_ttl ≤ static_max ≤ revocation-SLA).
        GitCheckGate::try_new(id, 300, &threshold()).expect("gate constructs")
    }

    fn zk() -> Zookie {
        Zookie("zk-merge".into())
    }

    fn repo_ref() -> ArtifactRef {
        ArtifactRef(REPO.into())
    }

    fn endorser<'a>(subject: &'a Principal, repo: &'a ArtifactRef, revoked: bool) -> Endorser<'a> {
        Endorser { subject, repo, zookie: zk(), subject_revoked: revoked }
    }

    // ───────────────────────── PART A: the endorsement resolver ─────────────────────────

    #[test]
    fn endorsement_need_only_flags_un_endorsed_fork_success() {
        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        let build = CheckContext::ci("build");
        // A fork success → needs endorsement.
        proj.apply(&fact("h1", build.clone(), 1, CheckState::Success, TrustTier::UntrustedFork));
        assert_eq!(
            endorsement_need(&proj, &head, &build),
            EndorsementNeed::NeedsEndorsement
        );
        // A trusted success → not applicable (already satisfies).
        let test = CheckContext::ci("test");
        proj.apply(&fact("h1", test.clone(), 1, CheckState::Success, TrustTier::Trusted));
        assert_eq!(endorsement_need(&proj, &head, &test), EndorsementNeed::NotApplicable);
        // A fork FAILURE → not applicable (endorsing a failure changes nothing — fork must clear CI).
        let lint = CheckContext::ci("lint");
        proj.apply(&fact("h1", lint.clone(), 1, CheckState::Failure, TrustTier::UntrustedFork));
        assert_eq!(endorsement_need(&proj, &head, &lint), EndorsementNeed::NotApplicable);
        // A MISSING context → not applicable.
        assert_eq!(
            endorsement_need(&proj, &head, &CheckContext::ci("absent")),
            EndorsementNeed::NotApplicable
        );
    }

    #[test]
    fn maintainer_endorsement_resolves_the_fork_context() {
        // The maintainer holds approve_untrusted_ci@repo → the resolver produces the endorsed context.
        let id = StubId::new().allowing_endorser("maintainer-1", REPO);
        let g = gate(id);
        let resolver = EndorsementResolver::new(&g);

        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        let build = CheckContext::ci("build");
        proj.apply(&fact("h1", build.clone(), 1, CheckState::Success, TrustTier::UntrustedFork));

        let m = maintainer();
        let repo = repo_ref();
        let endorsed = resolver.resolve_endorsed(
            std::slice::from_ref(&build),
            &proj,
            &head,
            &endorser(&m, &repo, false),
        );
        assert_eq!(endorsed, vec![build], "the maintainer endorsement resolves the fork context");
    }

    #[test]
    fn a_non_maintainer_cannot_self_endorse_the_fork_gate() {
        // The fork author (NOT granted approve_untrusted_ci) is the subject → Deny → 0 endorsed
        // contexts → the merge gate keeps blocking (0 forks green their own required gate).
        let id = StubId::new(); // nobody holds the endorsement relation.
        let g = gate(id);
        let resolver = EndorsementResolver::new(&g);

        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        let build = CheckContext::ci("build");
        proj.apply(&fact("h1", build.clone(), 1, CheckState::Success, TrustTier::UntrustedFork));

        // The fork author principal (any non-endorser) tries to endorse.
        let fork_author = principal("fork-author");
        let repo = repo_ref();
        let endorsed = resolver.resolve_endorsed(
            std::slice::from_ref(&build),
            &proj,
            &head,
            &endorser(&fork_author, &repo, false),
        );
        assert!(endorsed.is_empty(), "a non-maintainer cannot self-endorse the fork gate");
    }

    #[test]
    fn resolver_skips_the_check_for_non_fork_contexts() {
        // A trusted success + a missing context: neither needs an endorsement → the resolver produces
        // NO endorsements (it does not spuriously endorse a context the gate already decides).
        let id = StubId::new().allowing_endorser("maintainer-1", REPO);
        let g = gate(id);
        let resolver = EndorsementResolver::new(&g);

        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        let build = CheckContext::ci("build");
        let test = CheckContext::ci("test");
        proj.apply(&fact("h1", build.clone(), 1, CheckState::Success, TrustTier::Trusted));
        // `test` is required but missing.
        let m = maintainer();
        let repo = repo_ref();
        let endorsed = resolver.resolve_endorsed(
            &[build, test],
            &proj,
            &head,
            &endorser(&m, &repo, false),
        );
        assert!(endorsed.is_empty(), "no fork contexts → no endorsements minted");
    }

    #[test]
    fn revoked_maintainer_cannot_endorse() {
        // A just-revoked maintainer (subject_revoked) is denied even on a granted relation — a stale
        // ALLOW never overrides a revoke (the new-enemy guard rides the FailStatic gate).
        let id = StubId::new().allowing_endorser("maintainer-1", REPO);
        let g = gate(id);
        let resolver = EndorsementResolver::new(&g);

        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        let build = CheckContext::ci("build");
        proj.apply(&fact("h1", build.clone(), 1, CheckState::Success, TrustTier::UntrustedFork));

        let m = maintainer();
        let repo = repo_ref();
        let endorsed = resolver.resolve_endorsed(
            std::slice::from_ref(&build),
            &proj,
            &head,
            &endorser(&m, &repo, true), // revoked
        );
        assert!(endorsed.is_empty(), "a revoked maintainer cannot endorse");
    }

    #[test]
    fn context_satisfied_composes_need_check_and_acceptable() {
        let id = StubId::new().allowing_endorser("maintainer-1", REPO);
        let g = gate(id);
        let resolver = EndorsementResolver::new(&g);

        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        let build = CheckContext::ci("build");
        // An un-endorsed fork success: the resolver endorses it (maintainer) → satisfied.
        proj.apply(&fact("h1", build.clone(), 1, CheckState::Success, TrustTier::UntrustedFork));
        let m = maintainer();
        let repo = repo_ref();
        assert!(resolver.context_satisfied(
            &proj, &head, &build, &endorser(&m, &repo, false)
        ));
        // A missing context never satisfies.
        assert!(!resolver.context_satisfied(
            &proj, &head, &CheckContext::ci("absent"), &endorser(&m, &repo, false)
        ));
    }

    // ───────────────────────── PART B: the fork:<pr_id> cache confinement ─────────────────────────

    #[test]
    fn trust_scope_is_derived_from_the_run_never_caller_chosen() {
        // A trusted run → trusted scope; a fork run → fork:<pr_id> scope (NEVER trusted).
        assert_eq!(TrustScope::for_run(TrustTier::Trusted, "42"), TrustScope::Trusted);
        let fork = TrustScope::for_run(TrustTier::UntrustedFork, "42");
        assert_eq!(fork, TrustScope::Fork { pr_id: "42".into() });
        assert!(!fork.is_trusted(), "a fork run is NEVER the trusted scope");
        // The trusted scope IS trusted (the positive half — a mutant returning a constant `false`
        // would pass the fork assertion but fails here).
        assert!(TrustScope::Trusted.is_trusted(), "the trusted scope is trusted");
        assert!(TrustScope::for_run(TrustTier::Trusted, "ignored").is_trusted());
        assert_eq!(fork.key_prefix(), "fork:42");
        assert_eq!(TrustScope::Trusted.key_prefix(), "trusted");
    }

    #[test]
    fn a_fork_write_cannot_reach_the_trusted_scope() {
        // THE POISONED-CACHE DEFENCE (11.2 C4): a fork run writes a key; a trusted run READING the same
        // logical key gets a MISS (0 fork writes in the trusted scope).
        let cache = InMemoryCache::new();
        let tenant = TenantId("acme".into());
        let ttl = Duration::from_secs(60);

        let fork = ScopedCache::new(&cache, TrustScope::for_run(TrustTier::UntrustedFork, "42"));
        fork.set(&tenant, "dep-graph", b"poison", ttl).unwrap();

        // A trusted run reads the SAME logical key — it must NOT see the fork's poison.
        let trusted = ScopedCache::new(&cache, TrustScope::Trusted);
        assert_eq!(
            trusted.get(&tenant, "dep-graph").unwrap(),
            None,
            "a trusted read of a fork-written key is a MISS (0 fork writes in the trusted scope)"
        );

        // The fork run itself reads back its OWN write (confinement isolates, it does not lose data).
        assert_eq!(fork.get(&tenant, "dep-graph").unwrap(), Some(b"poison".to_vec()));
    }

    #[test]
    fn two_forks_are_isolated_from_each_other() {
        // fork:<pr_id> is per-PR: PR 42's cache is invisible to PR 99 (a fork cannot read another
        // fork's scope either).
        let cache = InMemoryCache::new();
        let tenant = TenantId("acme".into());
        let ttl = Duration::from_secs(60);

        let f42 = ScopedCache::new(&cache, TrustScope::for_run(TrustTier::UntrustedFork, "42"));
        let f99 = ScopedCache::new(&cache, TrustScope::for_run(TrustTier::UntrustedFork, "99"));
        f42.set(&tenant, "k", b"v42", ttl).unwrap();
        assert_eq!(f99.get(&tenant, "k").unwrap(), None, "PR 99 cannot read PR 42's fork scope");
        assert_eq!(f42.get(&tenant, "k").unwrap(), Some(b"v42".to_vec()));
    }

    #[test]
    fn a_trusted_write_is_visible_to_a_later_trusted_run() {
        // The confinement does not break the legitimate trusted-scope path: two trusted runs share the
        // trusted scope (a build cache hit across trusted runs is the whole point).
        let cache = InMemoryCache::new();
        let tenant = TenantId("acme".into());
        let ttl = Duration::from_secs(60);

        let t1 = ScopedCache::new(&cache, TrustScope::Trusted);
        t1.set(&tenant, "k", b"shared", ttl).unwrap();
        let t2 = ScopedCache::new(&cache, TrustScope::Trusted);
        assert_eq!(t2.get(&tenant, "k").unwrap(), Some(b"shared".to_vec()));
    }

    #[test]
    fn scoped_delete_invalidates_the_scoped_key() {
        // delete must actually drop the scoped backing key (a mutant that returns Ok(()) without
        // delegating would leave the value readable — this catches it).
        let cache = InMemoryCache::new();
        let tenant = TenantId("acme".into());
        let ttl = Duration::from_secs(60);

        let fork = ScopedCache::new(&cache, TrustScope::for_run(TrustTier::UntrustedFork, "42"));
        fork.set(&tenant, "k", b"v", ttl).unwrap();
        assert_eq!(fork.get(&tenant, "k").unwrap(), Some(b"v".to_vec()));
        fork.delete(&tenant, "k").unwrap();
        assert_eq!(fork.get(&tenant, "k").unwrap(), None, "delete dropped the scoped key");
    }
}
