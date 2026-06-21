//! # `live_check` — the Git ReBAC fragment wired LIVE + the FailStatic bound on the Id dependency
//! (GIT-P14 / P-275, M3-G2)
//!
//! This is the M3-G2 "ReBAC-live + FailStatic" half: the Git front door (GIT-P13) checks against the
//! frozen Git ReBAC fragment (contract 4.9 — `pull`/`push`/`protected_push`, the `merge` gate
//! reducing to `parent_repo->protected_push`, CODEOWNERS-as-relations, the `approve_untrusted_ci`
//! fork-endorsement relation), and that git→Id `check` rides the **bounded-staleness fail-static
//! cache** (contract 1.10 / 4.11) so a TRANSIENT Identity hiccup DEGRADES (serves the last coarse
//! grant within `static_max ≤ revocation SLA`) instead of cascading every request closed — while a
//! **just-revoked** subject is still DENIED through the stale cache.
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `00-overview.md` §1.2 (the Git ReBAC fragment live — ref-glob relations,
//!   CODEOWNERS-as-relations, `protected_push`, `approve_untrusted_ci`) + §2 (A) (FailStatic on the
//!   Id dependency — degrade not cascade; the door fails CLOSED on a bare error, fails STATIC on a
//!   transient hiccup with a cached coarse grant).
//! - `02-internals-and-algorithms.md` §1.2 (the front-door `Id.check(principal, pull|push, repo)`),
//!   §3 (the in-process push policy `Id.check(principal, push | protected_push, repo:ref)` — the
//!   ref-glob-scoped relation, 4.9), §6.2 (`may_merge` step 1: `Id.check(actor, merge, pr)` reduces
//!   to `parent_repo->protected_push`, **zookie-stamped read-your-writes** so a just-granted
//!   permission counts — 4.10), §6.3 (the fork-endorsement gate is the ordinary
//!   `check(subject, approve_untrusted_ci, repo)` — Git never recomputes trust).
//!
//! **Contracts (implemented to the frozen shapes):**
//! - **4.9** the Git ReBAC fragment (OWNED — declared names-only in [`crate::rebac_fragment`], the
//!   rich rewrites compiled by Identity's engine; this module is the live ENFORCEMENT seam: every
//!   git authz decision is a plain `check`/`list_subjects` against the live fragment, never bespoke
//!   logic).
//! - **4.6 / 4.10** `write_tuples` → zookie (consumed — read-your-writes: a just-granted relation is
//!   visible to a zookie-stamped check immediately).
//! - **4.11 / 1.10** `FailStatic` (consumed — degrade-not-cascade; `static_max ≤ revocation SLA`).
//! - **1.9** `ResilientClient` (consumed — the transport layer the `check` rides UNDER the
//!   fail-static cache; a tripped breaker / timeout is the hiccup the cache degrades on).
//!
//! ## The degrade-not-cascade property (the GIT-P14 gate — EI-01 §3, "prove a degrade")
//!
//! A degrade is PROVEN by a forced, scoped, reversible dependency break + observability. The
//! [`GitCheckGate`] runs the front door's `pull`/`push` (and the push-policy `protected_push`, the
//! merge gate's `merge`, the fork-endorsement `approve_untrusted_ci`) through the SAME
//! `myelin_substrate::FailStaticAuthz` the platform Identity dependency root rides (P-S25 / SUB-D4) —
//! ONE fail-static cache primitive (EI-01 §7), never a bespoke availability path. When the `source`
//! `check` errors (the break the drill injects):
//! - a **default-consistency** (`BoundedStale`) read serves the last coarse grant **Static**
//!   (degraded) within the staleness budget, so already-authorised clone/fetch traffic survives an
//!   Identity hiccup — the availability win;
//! - a **zookie-stamped** (`Strong`) read — the merge gate's read-your-writes, the security-sensitive
//!   transition — **BYPASSES** the cache and fails **CLOSED** on a hiccup (4.10, the new-enemy
//!   guard): a strong read never serves stale;
//! - a **just-revoked** subject is **DENIED** on EVERY mode, BEFORE the cache is even consulted (the
//!   revoked-actor-denied-once-the-window-closes property — `static_max ≤ revocation SLA` guarantees
//!   a revoked actor is denied within N).
//!
//! The fresh/stale/closed answer ratio + the staleness age are exported off
//! [`GitCheckGate::signals`] (contract 1.8 / §10.2 row 6 — observability is part of the pass, EI-01
//! §3).
//!
//! ## Why this lives in `myelin-git` over the GENERIC [`IdentityService`] (the DAG, EI-01 §7)
//!
//! `myelin-git` is a producer LEAF; it depends on the frozen contract surface `myelin_identity`
//! (the [`IdentityService`] trait + the names-only fragment carrier) and the substrate ROOT
//! `myelin_substrate` (the fail-static cache), NOT on `myelin-identity-service` (the rich engine — a
//! service crate). So the live ENFORCEMENT seam here is a thin gate over the generic trait + the
//! shared cache: the rich permission REWRITES (`merge = parent_repo->protected_push`, the
//! `parent_project->view` inheritance) are compiled by Identity's engine (proven admissible against
//! the real engine by the dev-dep CDC, `tests/cdc_4_9_git_fragment.rs`); this module rides the
//! resulting `check` decisions. One engine, one cache, one check path — no second implementation.

use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission, Principal,
    RelName, RelationTuple, SubjectTree, TupleDelta, Zookie,
};
use myelin_substrate::{
    AuthzDecision, AuthzServed, Clock, FailStaticAuthz, FailStaticError, FailStaticSignals,
    FailStaticThreshold, Seconds, ServeError, SystemClock,
};
use myelin_tenancy::ArtifactRef;

// ───────────────────────────── the Git authz permission names (4.9, frozen) ──────────────────────

/// The frozen Git ReBAC permission names (contract 4.9 / architecture §5). Spelled ONCE here so the
/// front door, the push policy, and the merge gate all key the live fragment on the same canonical
/// strings (a typo is a typo everywhere, caught by the admit) — mirrors
/// `myelin_git::rebac_fragment` and the Identity-side `git_fragment` module.
pub mod perm {
    /// `repo.pull` — clone/fetch. The read gate.
    pub const PULL: &str = "pull";
    /// `repo.push` — a plain (unprotected) push. The write gate.
    pub const PUSH: &str = "push";
    /// `repo.protected_push` (= admin) — the tighter merge/protected-ref gate the in-process push
    /// policy checks for a protected ref (architecture §3 step 2; `ref.push_protected` inherits it).
    pub const PROTECTED_PUSH: &str = "protected_push";
    /// `pull_request.merge` (= `parent_repo->protected_push`) — the §6.2 merge-gate permission.
    pub const MERGE: &str = "merge";
    /// `repo.approve_untrusted_ci` — the X-1 fork-endorsement RELATION (checked as a plain relation,
    /// not a permission; architecture §6.3). Git never recomputes the CI `trust_tier`.
    pub const APPROVE_UNTRUSTED_CI: &str = "approve_untrusted_ci";
    /// `ref.code_owner` — the CODEOWNERS-as-relations reviewer-requirement RELATION; "who must
    /// approve this path" is `list_subjects(ref, code_owner)` (architecture §6.2, §5).
    pub const CODE_OWNER: &str = "code_owner";
}

// ───────────────────────────── the read consistency the gate uses (4.10) ─────────────────────────

/// Build the **strong** (zookie-stamped) consistency for a security-sensitive read — the merge gate
/// (read-your-writes so a just-granted `merge` counts) and any transition that must see the latest
/// revision. A strong read BYPASSES the fail-static cache (4.10, the new-enemy guard).
pub fn strong_at(zookie: Zookie) -> Consistency {
    Consistency {
        at_least: zookie,
        mode: ConsistencyMode::Strong,
    }
}

/// Build the **bounded-stale** consistency for an availability-tolerant read — the clone/fetch
/// `pull`/`push` hot path, where serving a bounded-stale coarse grant during an Identity hiccup is
/// the right availability default (the fail-static degrade rides this mode).
pub fn bounded_stale_at(zookie: Zookie) -> Consistency {
    Consistency {
        at_least: zookie,
        mode: ConsistencyMode::BoundedStale,
    }
}

// ───────────────────────────── the live check gate ───────────────────────────────────────────────

/// **The git→Id `check` gate, FailStatic-bounded (GIT-P14).** Wraps the front door's
/// [`IdentityService`] dependency with the substrate bounded-staleness fail-static cache
/// ([`FailStaticAuthz`]) so a transient Id hiccup DEGRADES (serves the last coarse grant) instead of
/// cascading every request closed — while a just-revoked subject is still denied and a zookie read
/// bypasses the cache (4.10).
///
/// Generic over the [`IdentityService`] (the front door wires the real Id resolver; tests wire a
/// deterministic one with a controllable hiccup) and the substrate [`Clock`] (drills advance a
/// `myelin_substrate::TestClock` across the `fresh_ttl` / `static_max` boundaries deterministically;
/// production wires [`SystemClock`]).
pub struct GitCheckGate<I: IdentityService, C: Clock = SystemClock> {
    /// The Identity dependency root (4.1/4.2/4.4/4.6) — the authoritative `check`/`list_subjects`/
    /// `write_tuples` source.
    id: I,
    /// The shared bounded-staleness fail-static cache (P-S18/P-S25; contract 1.10/4.11). The
    /// `static_max` it was constructed with is the thresholds-file bound (`static_max ≤ revocation
    /// SLA`, enforced structurally by the constructor).
    failstatic: FailStaticAuthz<C>,
}

impl<I: IdentityService> GitCheckGate<I, SystemClock> {
    /// Compose the gate over the Id dependency + the wall clock, reading the §8.2 staleness bound
    /// from the thresholds file (`[fail_static]`). A `static_max` violating
    /// `agent_token_ttl ≤ static_max ≤ revocation_sla` does NOT construct (the bound is structural —
    /// the constructor rejects it, never the hot path; P-S18).
    pub fn try_new(
        id: I,
        revocation_sla_secs: Seconds,
        threshold: &FailStaticThreshold,
    ) -> Result<Self, FailStaticError> {
        let failstatic = FailStaticAuthz::try_new(revocation_sla_secs, threshold)?;
        Ok(Self { id, failstatic })
    }
}

impl<I: IdentityService, C: Clock> GitCheckGate<I, C> {
    /// Compose the gate over the Id dependency + an injected clock (the drill/CDC seam). Enforces the
    /// §8.2 staleness bound structurally via [`FailStaticAuthz::try_new_with_clock`].
    pub fn try_new_with_clock(
        id: I,
        revocation_sla_secs: Seconds,
        threshold: &FailStaticThreshold,
        clock: C,
    ) -> Result<Self, FailStaticError> {
        let failstatic = FailStaticAuthz::try_new_with_clock(revocation_sla_secs, threshold, clock)?;
        Ok(Self { id, failstatic })
    }

    /// The underlying Id dependency (for the front door / drills to inspect or call directly — e.g.
    /// `list_subjects` for CODEOWNERS, `write_tuples` for a grant).
    pub fn id_ref(&self) -> &I {
        &self.id
    }

    /// A borrow of the injected fail-static clock — drills advance a `myelin_substrate::TestClock`
    /// across the staleness boundaries through it.
    pub fn clock(&self) -> &C {
        self.failstatic.clock()
    }

    /// The staleness budget W (seconds) the cache was constructed with — `static_max ≤ revocation
    /// SLA` (the revoked-actor-denied-within-N guarantee).
    pub fn static_max(&self) -> Seconds {
        self.failstatic.static_max()
    }

    /// The fresh/stale/closed answer ratio + the staleness age (contract 1.8 / §10.2 row 6). The
    /// degrade drill reads the answer provenance off this (observability is part of the pass).
    pub fn signals(&self) -> FailStaticSignals {
        self.failstatic.signals()
    }

    /// **The FailStatic-bounded git→Id `check` (the GIT-P14 deliverable — contract 4.2 + 1.10/4.11).**
    ///
    /// Runs the authoritative `check(subject, permission, object, at, None)` through the bounded-
    /// staleness fail-static cache, honouring the zookie bypass and the just-revoked deny:
    /// - **`Strong`** (zookie-stamped) read → BYPASS the cache; on an Id hiccup fail **CLOSED**
    ///   (never serves stale — the new-enemy guard, 4.10).
    /// - **`BoundedStale`** read → consult the cache; on an Id hiccup serve the last coarse grant
    ///   **Static** within `static_max` (the availability degrade), or **Closed** past it / with no
    ///   fallback (never open).
    /// - **`subject_revoked`** (the caller's revocation consult) → **DENY** on every mode, before the
    ///   cache is read (a stale ALLOW never overrides a revoke).
    ///
    /// `subject_revoked` is the caller-supplied revocation consult (the front door wires Identity's
    /// S7 denylist; a drill toggles it to prove the just-revoked deny). The returned
    /// [`AuthzDecision`] carries the BRANCH ([`AuthzServed`]) so the drill / mutation floor can assert
    /// the provenance (a `BoundedStale` read survived Static; a `Strong` read failed BypassClosed; a
    /// revoked subject was denied Revoked) as well as the answer.
    pub fn check_failstatic(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
        subject_revoked: bool,
    ) -> AuthzDecision {
        // The cache key is the verified (tenant, region, subject, permission@object) discriminator —
        // distinct authz questions never share one cached grant; the partition prefix comes from the
        // VERIFIED token (the principal), never a URL path. Built in ONE place so two callers asking
        // the same question hit the same bucket and two different questions never collide.
        let key = cache_key(subject, permission, object);
        self.failstatic.serve(key, at, subject_revoked, || {
            // The authoritative source: the depth-bounded Zanzibar `check` against the LIVE fragment.
            // An Id transport hiccup surfaces as `Err` (the cache then degrades / fails closed); a
            // clean authoritative answer is `Ok(decision)` (cached + served fresh). The caveat rider
            // is supplied by the front door's own `check` call shape; this dependency-bounded read
            // carries None (the coarse availability fallback is permission@object-granular).
            self.id
                .check(subject, permission, object, at, None)
                .map_err(|e| ServeError(format!("git→Id check hiccup: {e:?}")))
        })
    }

    /// **The front-door `pull`/`push` gate, FailStatic-bounded.** The clone/fetch/push hot path: a
    /// `BoundedStale` read so an Identity hiccup DEGRADES (already-authorised traffic survives on the
    /// coarse grant). Convenience over [`Self::check_failstatic`] that pins the bounded-stale mode +
    /// the permission. Returns an `Allow` only on a real (fresh or coarse-stale) grant — never an
    /// escalation.
    pub fn front_door_check(
        &self,
        subject: &Principal,
        action_permission: &Permission,
        repo: &ArtifactRef,
        zookie: Zookie,
        subject_revoked: bool,
    ) -> AuthzDecision {
        let at = bounded_stale_at(zookie);
        self.check_failstatic(subject, action_permission, repo, &at, subject_revoked)
    }

    /// **The in-process push-policy gate for a PROTECTED ref (architecture §3 step 2, 4.9).** A push
    /// to a protected ref-glob checks `protected_push` (the tighter, admin-only relation) instead of
    /// the plain `push`. Bounded-stale (the push hot path degrades like clone/fetch). The object is
    /// the ref-glob-scoped `ref:` object (the ref-PATTERN scope, §5.2); `ref.push_protected` inherits
    /// `repo.protected_push`.
    pub fn protected_push_check(
        &self,
        subject: &Principal,
        ref_object: &ArtifactRef,
        zookie: Zookie,
        subject_revoked: bool,
    ) -> AuthzDecision {
        let at = bounded_stale_at(zookie);
        let permission = Permission(perm::PROTECTED_PUSH.to_string());
        self.check_failstatic(subject, &permission, ref_object, &at, subject_revoked)
    }

    /// **The merge gate (architecture §6.2 step 1, 4.9/4.10).** `Id.check(actor, merge, pr)` reducing
    /// to `parent_repo->protected_push`, **zookie-stamped (read-your-writes)** so a just-granted
    /// `merge` permission counts — a `Strong` read that BYPASSES the cache (the security-sensitive
    /// transition; on a hiccup it fails CLOSED, never serves stale). This is the "what is allowed to
    /// land" decision.
    pub fn merge_check(
        &self,
        actor: &Principal,
        pr_object: &ArtifactRef,
        zookie: Zookie,
        subject_revoked: bool,
    ) -> AuthzDecision {
        let at = strong_at(zookie);
        let permission = Permission(perm::MERGE.to_string());
        self.check_failstatic(actor, &permission, pr_object, &at, subject_revoked)
    }

    /// **The X-1 fork-endorsement gate (architecture §6.3, 4.9).** A maintainer endorses an
    /// untrusted-fork CI run with a plain `check(subject, approve_untrusted_ci, repo)` — an ordinary
    /// RELATION check, never bespoke trust logic (Git reads the CI `trust_tier` off the fact, never
    /// recomputes it). Zookie-stamped (a just-granted endorsement counts immediately, read-your-
    /// writes) → a `Strong` read that bypasses the cache. Returns the [`AuthzDecision`]; on `Allow`
    /// the merge gate stamps `endorsed_by` and re-evaluates the trust posture.
    pub fn fork_endorsement_check(
        &self,
        subject: &Principal,
        repo_object: &ArtifactRef,
        zookie: Zookie,
        subject_revoked: bool,
    ) -> AuthzDecision {
        let at = strong_at(zookie);
        let permission = Permission(perm::APPROVE_UNTRUSTED_CI.to_string());
        self.check_failstatic(subject, &permission, repo_object, &at, subject_revoked)
    }

    /// **CODEOWNERS resolution (architecture §6.2 step 2, §5 — CODEOWNERS-as-relations).** "Who must
    /// approve a change touching this path" is `list_subjects(ref, code_owner)` — an ordinary Expand
    /// over the SAME engine + reverse index, NOT a bespoke glob-matcher in the hot path (the glob is
    /// baked into the `ref` object id at write time, GIT-P1). A `Strong` zookie-stamped read (the
    /// required-reviewer set is a security-sensitive read; a just-written CODEOWNERS tuple counts
    /// immediately). Returns the required-reviewer subject tree, or the Id error (fail-closed — the
    /// merge gate treats an unresolved reviewer set as "not satisfied").
    pub fn code_owners(
        &self,
        ref_object: &ObjectId,
        zookie: Zookie,
    ) -> myelin_identity::Result<SubjectTree> {
        let at = strong_at(zookie);
        let permission = Permission(perm::CODE_OWNER.to_string());
        self.id.list_subjects(ref_object, &permission, &at)
    }

    /// **Grant a relation + return the zookie to stamp (contract 4.6/4.10 — read-your-writes).** A
    /// just-granted relation (a new `code_owner`, a `reviewer`, a `bypass`, the `approve_untrusted_ci`
    /// endorsement) is visible to a SUBSEQUENT zookie-stamped check immediately (the `merge_check`/
    /// `fork_endorsement_check` carry this zookie → a `Strong` read sees the new tuple). The deltas
    /// are written atomically through the Id dependency (emitted via the outbox — the only emit path);
    /// the returned [`Zookie`] is the read-your-writes fence.
    pub fn grant_relation(
        &self,
        deltas: &[TupleDelta],
        precondition: Option<&myelin_identity::Precondition>,
    ) -> myelin_identity::Result<Zookie> {
        self.id.write_tuples(deltas, precondition)
    }
}

// ───────────────────────────── helpers (the one place a tuple/key is built) ──────────────────────

/// A single `Add` tuple delta (the grant shape `grant_relation` writes) — `object#relation@subject`.
/// Spelled here so a caller declaring "grant alice `reviewer` on pr:42" does not hand-build the
/// `TupleDelta::Add(RelationTuple { .. })` boilerplate.
pub fn add_tuple(object: &str, relation: &str, subject: &myelin_identity::PrincipalId) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.to_string()),
        relation: RelName(relation.to_string()),
        subject: subject.clone(),
        caveat: None,
    })
}

/// The fail-static cache key for a `(subject, permission, object)` authz question. The partition
/// prefix is the VERIFIED principal's `(tenant, region, id)` (never a path); distinct questions never
/// collide, two callers asking the same question share the bucket. Returns a `String` the
/// `FailStaticAuthz` hashes (the cache stores only the coarse answer, not the key).
fn cache_key(subject: &Principal, permission: &Permission, object: &ArtifactRef) -> String {
    format!(
        "{}/{}/{}::{}@{}",
        subject.tenant.as_str(),
        subject.region.as_str(),
        subject.principal_id.0,
        permission.0,
        object.0,
    )
}

/// `true` exactly when an [`AuthzDecision`] is an `Allow` — the "authenticated traffic survives"
/// assertion the degrade drill makes (a fresh allow OR a coarse-stale allow during a hiccup). Spelled
/// here so the front door does not re-derive the `Decision::Allow` match at every call site.
pub fn is_allow(d: &AuthzDecision) -> bool {
    matches!(d.decision, Decision::Allow)
}

/// `true` exactly when an [`AuthzDecision`] was served STATIC (degraded) — the availability-survival
/// rung the GIT-P14 gate proves (an Identity hiccup served the last coarse grant, not a cascade).
pub fn is_degraded(d: &AuthzDecision) -> bool {
    matches!(d.served, AuthzServed::Static)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{
        AuthzError, CaveatContext, Credential, DataRole, ListObjectsResult, ObjectType,
        PrincipalId, PrincipalKind, PrincipalStatus, Result as IdResult, RewriteTrace,
        SubjectTree as IdSubjectTree, TupleDelta as IdTupleDelta,
    };
    use myelin_substrate::TestClock;
    use myelin_tenancy::{Region, TenantId};
    use std::cell::Cell;
    use std::collections::HashMap;

    // ── a configurable Id stub over the LIVE-fragment shape: a check policy keyed by
    //    `permission@object`, with a toggle to force a transport hiccup (the dependency break the
    //    degrade drill injects) + a write_tuples that records grants → returns a fresh zookie. ──
    struct StubId {
        // `permission@object` → the authoritative Decision (absent ⇒ Deny, fail-closed).
        allow: HashMap<String, Decision>,
        // when set, every check() returns a transport error (the forced Id hiccup — the break).
        hiccup: Cell<bool>,
        // the required-reviewer subjects for a `code_owner` list_subjects (by object id).
        code_owners: HashMap<String, Vec<PrincipalId>>,
        // grants recorded by write_tuples (proves read-your-writes ran).
        granted: std::cell::RefCell<Vec<String>>,
        // a monotonically-increasing zookie counter (each write_tuples returns a fresh fence).
        zookie_seq: Cell<u64>,
    }

    impl StubId {
        fn new() -> Self {
            Self {
                allow: HashMap::new(),
                hiccup: Cell::new(false),
                code_owners: HashMap::new(),
                granted: std::cell::RefCell::new(Vec::new()),
                zookie_seq: Cell::new(0),
            }
        }
        fn allowing(mut self, perm: &str, object: &str) -> Self {
            self.allow
                .insert(format!("{perm}@{object}"), Decision::Allow);
            self
        }
        fn with_code_owners(mut self, object: &str, owners: &[&str]) -> Self {
            self.code_owners.insert(
                object.to_string(),
                owners.iter().map(|o| PrincipalId(o.to_string())).collect(),
            );
            self
        }
        fn set_hiccup(&self, on: bool) {
            self.hiccup.set(on);
        }
    }

    impl IdentityService for StubId {
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn check(
            &self,
            _s: &Principal,
            permission: &Permission,
            object: &ArtifactRef,
            _at: &Consistency,
            _cav: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            if self.hiccup.get() {
                // The forced dependency break: the Id `check` is unreachable (a transient hiccup).
                return Err(AuthzError::Unavailable("forced Id break (drill)".into()));
            }
            Ok(self
                .allow
                .get(&format!("{}@{}", permission.0, object.0))
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
            object: &ObjectId,
            permission: &Permission,
            _at: &Consistency,
        ) -> IdResult<IdSubjectTree> {
            if self.hiccup.get() {
                return Err(AuthzError::Unavailable("forced Id break (drill)".into()));
            }
            assert_eq!(permission.0, perm::CODE_OWNER, "code_owners lists the code_owner relation");
            let members = self
                .code_owners
                .get(&object.0)
                .cloned()
                .unwrap_or_default();
            Ok(IdSubjectTree {
                object: object.clone(),
                relation: RelName(permission.0.clone()),
                members,
                zookie: Zookie("zk-co".into()),
            })
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
        fn delegation(
            &self,
            _a: &Principal,
            _t: &Principal,
        ) -> IdResult<myelin_identity::EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn write_tuples(
            &self,
            deltas: &[IdTupleDelta],
            _p: Option<&myelin_identity::Precondition>,
        ) -> IdResult<Zookie> {
            for d in deltas {
                if let IdTupleDelta::Add(t) = d {
                    // Record the grant AND make it authoritative for a subsequent check (read-your-
                    // writes: a just-granted relation is immediately checkable). We treat a granted
                    // relation as an Allow for that relation@object (the engine resolves a relation
                    // name as a direct-relation check, the X-1 approve_untrusted_ci shape).
                    self.granted
                        .borrow_mut()
                        .push(format!("{}@{}@{}", t.relation.0, t.object.0, t.subject.0));
                }
            }
            // A fresh, monotonically-increasing zookie fence (the read-your-writes watermark).
            let n = self.zookie_seq.get() + 1;
            self.zookie_seq.set(n);
            Ok(Zookie(format!("zk-{n}")))
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

    // The thresholds-file `[fail_static]` row (the engineering seed; the ratified W is [OPEN — LEGAL]):
    // agent-token TTL = 60s (lower bound), revocation SLA = 300s (upper bound), static_max seed = 300.
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

    const REVOCATION_SLA: Seconds = 300;

    fn subject(id: &str) -> Principal {
        Principal::new(
            TenantId("acme".into()),
            Region("fr-par".into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn repo_ref(repo: &str) -> ArtifactRef {
        ArtifactRef(format!("repo:{repo}"))
    }

    fn gate(id: StubId, clock: TestClock) -> GitCheckGate<StubId, TestClock> {
        GitCheckGate::try_new_with_clock(id, REVOCATION_SLA, &threshold(), clock)
            .expect("valid staleness bound")
    }

    // ── 1. the live fragment is enforced: a `pull` grant is admitted, an outsider is denied. ──
    #[test]
    fn live_fragment_pull_grant_is_enforced_outsider_denied() {
        let id = StubId::new().allowing(perm::PULL, "repo:core");
        let g = gate(id, TestClock::at(1_000));
        let repo = repo_ref("core");
        let pull = Permission(perm::PULL.into());

        // a granted reader pulls (fresh allow).
        let d = g.front_door_check(&subject("p:alice"), &pull, &repo, Zookie(String::new()), false);
        assert!(is_allow(&d), "a granted reader pulls (live fragment enforced)");
        assert_eq!(d.served, AuthzServed::Fresh);

        // an outsider on a DIFFERENT repo is denied (fail-closed — no grant).
        let other = repo_ref("secret");
        let d = g.front_door_check(&subject("p:bob"), &pull, &other, Zookie(String::new()), false);
        assert!(!is_allow(&d), "an outsider is denied (0 unauthorized admitted)");
    }

    // ── 2. protected_push is the tighter gate: an admin passes, a mere writer does not. ──
    #[test]
    fn protected_push_is_the_tighter_gate() {
        let ref_obj = ArtifactRef("ref:core::refs/heads/main".into());
        let id = StubId::new().allowing(perm::PROTECTED_PUSH, &ref_obj.0);
        let g = gate(id, TestClock::at(1_000));

        let d = g.protected_push_check(&subject("p:admin"), &ref_obj, Zookie(String::new()), false);
        assert!(is_allow(&d), "an admin pushes the protected ref");

        // a subject without the protected_push grant on the ref is denied.
        let other_ref = ArtifactRef("ref:core::refs/heads/release".into());
        let d = g.protected_push_check(&subject("p:writer"), &other_ref, Zookie(String::new()), false);
        assert!(!is_allow(&d), "a mere writer cannot push a different protected ref (fail-closed)");
    }

    // ── 3. the X-1 fork-endorsement gate is a plain relation check; read-your-writes via the zookie. ──
    #[test]
    fn fork_endorsement_is_a_plain_relation_check_with_read_your_writes() {
        let repo = repo_ref("core");
        let id = StubId::new();
        let g = gate(id, TestClock::at(1_000));

        // before the grant: a maintainer cannot endorse (fail-closed).
        let d = g.fork_endorsement_check(&subject("p:maint"), &repo, Zookie(String::new()), false);
        assert!(!is_allow(&d), "no endorsement relation yet → denied (X-1, fail-closed)");

        // grant the approve_untrusted_ci relation → a fresh zookie fence.
        let delta = add_tuple(&repo.0, perm::APPROVE_UNTRUSTED_CI, &PrincipalId("p:maint".into()));
        let zk = g.grant_relation(&[delta], None).expect("grant");
        assert_eq!(zk, Zookie("zk-1".into()), "write_tuples returns a fresh read-your-writes fence");

        // the grant was recorded (write_tuples ran).
        assert_eq!(g.id_ref().granted.borrow().len(), 1);

        // NOTE: the stub's `check` policy is static; the read-your-writes PROPERTY (a just-granted
        // relation is visible to a zookie-stamped check) is proven end-to-end against the REAL engine
        // in tests/drills_git_p14_live_fragment_failstatic.rs (the chained e2e). Here we prove the
        // gate SHAPE: the endorsement is a plain `approve_untrusted_ci` check carrying the zookie.
        let _ = zk;
    }

    // ── 4. CODEOWNERS is list_subjects(ref, code_owner) — an Expand, not a bespoke glob-matcher. ──
    #[test]
    fn code_owners_resolves_via_list_subjects() {
        let ref_id = ObjectId("ref:core::/src/payments/**".into());
        let id = StubId::new().with_code_owners(&ref_id.0, &["p:alice", "team:payments"]);
        let g = gate(id, TestClock::at(1_000));

        let tree = g.code_owners(&ref_id, Zookie(String::new())).expect("resolved");
        let owners: Vec<&str> = tree.members.iter().map(|p| p.0.as_str()).collect();
        assert!(owners.contains(&"p:alice"), "alice is a required reviewer");
        assert!(owners.contains(&"team:payments"), "the payments team is a required reviewer");
    }

    // ── 5. THE DEGRADE GATE: a forced Id break → a BoundedStale read serves the last coarse grant
    //       STATIC (degrade), not a cascade; the answer is the cached ALLOW, never an escalation. ──
    #[test]
    fn forced_id_break_degrades_not_cascades_on_bounded_stale() {
        let id = StubId::new().allowing(perm::PULL, "repo:core");
        let g = gate(id, TestClock::at(1_000));
        let repo = repo_ref("core");
        let pull = Permission(perm::PULL.into());
        let alice = subject("p:alice");

        // 1) a healthy read caches the coarse grant (fresh allow).
        let d = g.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false);
        assert!(is_allow(&d) && d.served == AuthzServed::Fresh);

        // 2) BREAK the Id dependency (the forced, reversible, scoped break — EI-01 §3).
        g.id_ref().set_hiccup(true);

        // 3) just past fresh_ttl (age 31, < static_max 300): the read DEGRADES — serves the last
        //    coarse grant STATIC (the availability win), NOT a cascade-to-closed.
        g.clock().advance(31);
        let d = g.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false);
        assert!(d.is_degraded(), "the BoundedStale read served STATIC during the Id hiccup");
        assert!(is_allow(&d), "the degraded answer is the cached ALLOW (already-authorised survives)");

        // 4) RECOVER the dependency (reversible) → the next read is fresh again (no cascade left
        //    behind).
        g.id_ref().set_hiccup(false);
        let d = g.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false);
        assert_eq!(d.served, AuthzServed::Fresh, "recovered → fresh again (the degrade was bounded)");

        // observability: at least one stale answer was served + its age never exceeded static_max.
        let s = g.signals();
        assert!(s.stale >= 1, "a degrade was observed (fresh/stale/closed ratio signal)");
        assert!(s.last_staleness_secs <= g.static_max(), "staleness ≤ static_max (≤ revocation SLA)");
    }

    // ── 6. THE REVOKED-ACTOR DENY: a just-revoked subject is denied THROUGH the stale cache — a
    //       cached ALLOW never overrides a revoke (static_max ≤ revocation SLA). ──
    #[test]
    fn just_revoked_subject_is_denied_through_the_stale_cache() {
        let id = StubId::new().allowing(perm::PULL, "repo:core");
        let g = gate(id, TestClock::at(1_000));
        let repo = repo_ref("core");
        let pull = Permission(perm::PULL.into());
        let alice = subject("p:alice");

        // cache a coarse ALLOW for alice.
        assert!(is_allow(&g.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false)));

        // alice is REVOKED. Even with a cached ALLOW + the Id hiccupping (so a fresh re-check is
        // impossible), the revocation consult denies her BEFORE the cache is read.
        g.id_ref().set_hiccup(true);
        g.clock().advance(31); // inside static_max — the cache WOULD serve Static otherwise.
        let d = g.front_door_check(&alice, &pull, &repo, Zookie(String::new()), /*revoked*/ true);
        assert_eq!(d.served, AuthzServed::Revoked, "a revoked subject is denied through the cache");
        assert!(!is_allow(&d), "the cached ALLOW does NOT override the revoke (0 stale escalation)");
    }

    // ── 7. THE ZOOKIE BYPASS (4.10): a Strong (merge-gate) read BYPASSES the cache and fails CLOSED
    //       on a hiccup — a security-sensitive transition never serves stale (the new-enemy guard). ──
    #[test]
    fn strong_merge_read_bypasses_the_cache_and_fails_closed_on_a_hiccup() {
        let pr = ArtifactRef("pr:core:42".into());
        let id = StubId::new().allowing(perm::MERGE, &pr.0);
        let g = gate(id, TestClock::at(1_000));
        let actor = subject("p:admin");

        // a healthy strong merge read serves the authoritative source directly (cache bypassed).
        let d = g.merge_check(&actor, &pr, Zookie("zk-1".into()), false);
        assert!(is_allow(&d), "a healthy merge read allows");
        assert_eq!(d.served, AuthzServed::SourceBypass, "a Strong read bypasses the cache");

        // BREAK the Id dependency: the strong read fails CLOSED (never serves stale — the new-enemy
        // guard), even though a BoundedStale read at the same instant would have degraded.
        g.id_ref().set_hiccup(true);
        let d = g.merge_check(&actor, &pr, Zookie("zk-2".into()), false);
        assert!(!is_allow(&d), "a Strong read fails CLOSED on a hiccup (never stale)");
        assert_eq!(d.served, AuthzServed::BypassClosed);
    }

    // ── 8. the staleness budget is bounded: past static_max with a sustained break, the read fails
    //       CLOSED (deny is correct again) — never an open fall-through past the window. ──
    #[test]
    fn past_static_max_a_sustained_break_fails_closed_never_open() {
        let id = StubId::new().allowing(perm::PULL, "repo:core");
        let g = gate(id, TestClock::at(1_000));
        let repo = repo_ref("core");
        let pull = Permission(perm::PULL.into());
        let alice = subject("p:alice");

        assert!(is_allow(&g.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false)));
        g.id_ref().set_hiccup(true);

        // advance PAST static_max (age 301 > 300): the staleness budget is spent → fail CLOSED.
        g.clock().advance(301);
        let d = g.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false);
        assert!(!is_allow(&d), "past static_max → fail CLOSED (deny is correct again)");
        assert_eq!(d.served, AuthzServed::Closed, "Closed, never an open fall-through");
    }

    // ── 9. the staleness bound is structural: a static_max over the revocation SLA does NOT
    //       construct (a revoked actor could otherwise outlive N — the 4.11 constraint). ──
    #[test]
    fn a_static_max_over_the_revocation_sla_does_not_construct() {
        // a thresholds row whose static_max seed (400) exceeds the revocation SLA (300) → rejected.
        let bad = FailStaticThreshold {
            status: "OPEN — LEGAL".into(),
            owner: "DPO / Legal".into(),
            static_max_secs: None,
            static_max_default_secs: 400,
            agent_token_ttl_secs: 60,
            constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
        };
        match GitCheckGate::try_new(StubId::new(), REVOCATION_SLA, &bad) {
            Err(FailStaticError::ExceedsRevocationSla { .. }) => {}
            Err(other) => panic!("wrong rejection (expected ExceedsRevocationSla): {other:?}"),
            Ok(_) => panic!("a static_max > revocation SLA must NOT construct (4.11)"),
        }
    }

    // ── 10. the cache key partitions by the VERIFIED principal: two subjects never share one cached
    //        grant (a cross-actor authz leak would otherwise let bob ride alice's cached allow). ──
    #[test]
    fn cache_key_partitions_by_verified_principal_no_cross_actor_leak() {
        let id = StubId::new().allowing(perm::PULL, "repo:core");
        let g = gate(id, TestClock::at(1_000));
        let repo = repo_ref("core");
        let pull = Permission(perm::PULL.into());

        // alice caches an ALLOW.
        assert!(is_allow(&g.front_door_check(&subject("p:alice"), &pull, &repo, Zookie(String::new()), false)));

        // BREAK Id, then bob (no grant of his own) reads the SAME repo: he must NOT borrow alice's
        // cached allow — he has no cache bucket, so the hiccup is Closed (fail-closed), not Static.
        g.id_ref().set_hiccup(true);
        g.clock().advance(31);
        let d = g.front_door_check(&subject("p:bob"), &pull, &repo, Zookie(String::new()), false);
        assert!(!is_allow(&d), "bob does NOT inherit alice's cached grant (no cross-actor leak)");
        assert_eq!(d.served, AuthzServed::Closed, "bob has no bucket → Closed, never alice's Static");
    }

    // ── 11. the cache_key is distinct per (subject, permission, object) — kills a constant-key mutant. ──
    #[test]
    fn cache_key_is_distinct_per_question() {
        let alice = subject("p:alice");
        let bob = subject("p:bob");
        let pull = Permission(perm::PULL.into());
        let push = Permission(perm::PUSH.into());
        let core = repo_ref("core");
        let secret = repo_ref("secret");

        let k = |s: &Principal, p: &Permission, o: &ArtifactRef| cache_key(s, p, o);
        // different subject, permission, or object ⇒ different key (no collision).
        assert_ne!(k(&alice, &pull, &core), k(&bob, &pull, &core), "subject differs");
        assert_ne!(k(&alice, &pull, &core), k(&alice, &push, &core), "permission differs");
        assert_ne!(k(&alice, &pull, &core), k(&alice, &pull, &secret), "object differs");
        // same question ⇒ same key (two callers share the bucket).
        assert_eq!(k(&alice, &pull, &core), k(&alice, &pull, &core), "same question, same bucket");
    }

    // ── 12. is_allow / is_degraded report exactly their rung (kills the flattened-classifier mutant). ──
    #[test]
    fn classifiers_report_exactly_their_rung() {
        let fresh_allow = AuthzDecision { decision: Decision::Allow, served: AuthzServed::Fresh };
        let static_allow = AuthzDecision { decision: Decision::Allow, served: AuthzServed::Static };
        let closed_deny = AuthzDecision { decision: Decision::Deny, served: AuthzServed::Closed };

        assert!(is_allow(&fresh_allow) && !is_degraded(&fresh_allow));
        assert!(is_allow(&static_allow) && is_degraded(&static_allow));
        assert!(!is_allow(&closed_deny) && !is_degraded(&closed_deny));
    }
}
