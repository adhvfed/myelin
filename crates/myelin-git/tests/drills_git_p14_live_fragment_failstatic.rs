//! # GIT-P14 (P-275, M3-G2) — the live ReBAC fragment + the FailStatic degrade, end-to-end
//!
//! The chained drill the prompt requires: **grant a relation → read-your-writes within the zookie →
//! break Id → assert degrade (not cascade) + just-revoked-denied**, proven against the **REAL**
//! Identity engine ([`StoreBackedCheck`] over the `with_core_hierarchy` cell schema with the Git
//! fragment admitted), NOT a stub. The [`myelin_git::live_check::GitCheckGate`] wraps the real engine
//! behind a forced, scoped, reversible dependency-break injector ([`BreakableId`]) so the degrade is
//! PROVEN by a real failure + observability (EI-01 §3), not asserted.
//!
//! This file carries the **drill** (the chained e2e) + the **CDC pairs** for contract rows 4.9 (the
//! Git fragment live — enforced at the check) and 4.11 (the FailStatic bound — degrade-not-cascade;
//! `static_max ≤ revocation SLA`). The fragment-evaluation + fail-static path is mandatory-core; the
//! cargo-mutants mutation floor over `live_check.rs` is named in the prompt report.

use std::cell::Cell;

use myelin_events::{OutboxStore, Timestamp};
use myelin_git::live_check::{is_allow, perm, GitCheckGate};
use myelin_identity::{
    Consistency, Decision, IdentityService, ObjectId, Permission, Principal, PrincipalId,
    PrincipalKind, RelName, RelationTuple, SubjectTree, TupleDelta, Zookie,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_substrate::{AuthzServed, FailStaticThreshold, TestClock};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

// ───────────────────────────── the dependency-break injector (a real engine, breakable) ──────────

/// A thin [`IdentityService`] wrapper over the REAL [`StoreBackedCheck`] engine with a forced,
/// reversible **break** toggle — the scoped dependency break the degrade drill injects (EI-01 §3,
/// the P-S03 `DependencyBreaker` pattern). When `broken`, every `check`/`list_subjects` returns a
/// transport-style `Unavailable` (the transient Identity hiccup the fail-static cache degrades on);
/// when not broken, it delegates to the authoritative engine. The `list_subjects` ABI method needs a
/// `(tenant, region)` scope the trait cannot carry, so it delegates to `list_subjects_in` over the
/// fixed test scope.
struct BreakableId {
    inner: StoreBackedCheck,
    scope: TenantScope,
    broken: Cell<bool>,
}

impl BreakableId {
    fn new(inner: StoreBackedCheck, scope: TenantScope) -> Self {
        Self { inner, scope, broken: Cell::new(false) }
    }
    fn set_broken(&self, on: bool) {
        self.broken.set(on);
    }
}

impl IdentityService for BreakableId {
    fn authenticate(&self, c: &myelin_identity::Credential) -> myelin_identity::Result<Principal> {
        self.inner.authenticate(c)
    }
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
        caveat: Option<&myelin_identity::CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        if self.broken.get() {
            // The forced break: the authoritative engine is unreachable (a transient hiccup).
            return Err(myelin_identity::AuthzError::Unavailable("forced Id break (drill)".into()));
        }
        self.inner.check(subject, permission, object, at, caveat)
    }
    fn list_objects(
        &self,
        subject: &Principal,
        permission: &Permission,
        ty: &myelin_identity::ObjectType,
        at: &Consistency,
    ) -> myelin_identity::Result<myelin_identity::ListObjectsResult> {
        self.inner.list_objects(subject, permission, ty, at)
    }
    fn list_subjects(
        &self,
        object: &ObjectId,
        permission: &Permission,
        at: &Consistency,
    ) -> myelin_identity::Result<SubjectTree> {
        if self.broken.get() {
            return Err(myelin_identity::AuthzError::Unavailable("forced Id break (drill)".into()));
        }
        // The ABI trait method cannot carry the (tenant, region) scope; delegate to the scoped
        // `list_subjects_in` over the fixed test scope (the CODEOWNERS Expand path).
        Ok(self.inner.list_subjects_in(&self.scope, object, permission, at))
    }
    fn explain(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ObjectId,
        at: &Consistency,
    ) -> myelin_identity::Result<myelin_identity::RewriteTrace> {
        self.inner.explain(subject, permission, object, at)
    }
    fn delegation(
        &self,
        agent: &Principal,
        trigger: &Principal,
    ) -> myelin_identity::Result<myelin_identity::EffectivePolicy> {
        self.inner.delegation(agent, trigger)
    }
    fn write_tuples(
        &self,
        deltas: &[TupleDelta],
        precondition: Option<&myelin_identity::Precondition>,
    ) -> myelin_identity::Result<Zookie> {
        self.inner.write_tuples(deltas, precondition)
    }
    fn mint_run_token(
        &self,
        a: &PrincipalId,
        r: &myelin_identity::RunId,
        d: &myelin_identity::DelegationCaveats,
        t: &myelin_identity::FailStaticBound,
    ) -> myelin_identity::Result<myelin_identity::RunToken> {
        self.inner.mint_run_token(a, r, d, t)
    }
    fn revoke(&self, t: &myelin_identity::RevokeTarget) -> myelin_identity::Result<()> {
        self.inner.revoke(t)
    }
    fn resolve_pseudonym(
        &self,
        s: &PrincipalId,
        t: &TenantId,
    ) -> myelin_identity::Result<String> {
        self.inner.resolve_pseudonym(s, t)
    }
    fn erase(&self, s: &PrincipalId) -> myelin_identity::Result<()> {
        self.inner.erase(s)
    }
    fn admit_fragment(
        &self,
        f: &myelin_identity::NamespaceFragment,
    ) -> myelin_identity::Result<myelin_identity::FragmentAdmit> {
        self.inner.admit_fragment(f)
    }
}

// ───────────────────────────── fixtures ──────────────────────────────────────────────────────────

fn scope(tenant: &str) -> TenantScope {
    let admin = Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&admin, Region("fr-par".into()))
}

fn subject(id: &str, tenant: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region("fr-par".into());
    p
}

fn add(object: &str, relation: &str, subj: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subj.into()),
        caveat: None,
    })
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

const REVOCATION_SLA: u64 = 300;

/// Admit the Git fragment into a fresh real engine + return it seeded with `tuples`.
fn engine_with_git_fragment(scope: &TenantScope, tuples: &[TupleDelta]) -> StoreBackedCheck {
    let store = TupleStore::new(OutboxStore::new());
    let admin = subject("p-admin", scope.tenant().as_str());
    store
        .write_tuples(
            scope,
            &admin,
            tuples,
            None,
            None,
            Timestamp("2026-06-21T00:00:00Z".into()),
        )
        .expect("seed tuples");
    let svc = StoreBackedCheck::new(store);
    for admit in svc.admit_git_fragment() {
        assert!(
            matches!(admit, myelin_identity::FragmentAdmit::Admitted { .. }),
            "the Git fragment admits into the live cell schema: {admit:?}"
        );
    }
    svc
}

// ───────────────────────────── CDC 4.9: the Git fragment is LIVE (enforced at the check) ──────────

/// **CDC 4.9 — the live Git fragment is ENFORCED at the front-door check (0 unauthorized admitted).**
/// A repo `admin` gets `pull` through the real engine + the fail-static gate; an outsider is denied.
/// This is the contract-4.9 consumer↔provider pair reified: Git (the consumer) gates an action ONLY
/// on a resolved grant; Identity (the provider) resolves the Git permission through the four userset
/// operators. The gate (the GitCheckGate) is the live ENFORCEMENT seam.
#[test]
fn cdc_4_9_live_fragment_is_enforced_at_the_check() {
    let s = scope("acme");
    let svc = engine_with_git_fragment(&s, &[add("repo:core", "admin", "p:alice")]);
    let gate = GitCheckGate::try_new_with_clock(
        BreakableId::new(svc, s.clone()),
        REVOCATION_SLA,
        &threshold(),
        TestClock::at(1_000),
    )
    .expect("valid staleness bound");

    let repo = ArtifactRef("repo:core".into());
    let pull = Permission(perm::PULL.into());

    // alice (a real repo admin) pulls — the live fragment resolves pull = reader∪writer∪admin∪….
    let d = gate.front_door_check(&subject("p:alice", "acme"), &pull, &repo, Zookie(String::new()), false);
    assert!(is_allow(&d), "a repo admin pulls through the LIVE fragment (0 unauthorized denied)");

    // an outsider is denied (fail-closed — no resolved grant).
    let d = gate.front_door_check(&subject("p:bob", "acme"), &pull, &repo, Zookie(String::new()), false);
    assert!(!is_allow(&d), "an outsider is denied (0 unauthorized action admitted)");
}

/// **CDC 4.9 — the X-1 fork-endorsement relation is a plain `check` (not bespoke logic).** A
/// maintainer with the `approve_untrusted_ci` relation endorses; an outsider does not — proven
/// through the live engine via the fail-static gate's `fork_endorsement_check`.
#[test]
fn cdc_4_9_fork_endorsement_relation_is_enforced_live() {
    let s = scope("acme");
    let svc = engine_with_git_fragment(&s, &[add("repo:core", "approve_untrusted_ci", "p:maint")]);
    let gate = GitCheckGate::try_new_with_clock(
        BreakableId::new(svc, s.clone()),
        REVOCATION_SLA,
        &threshold(),
        TestClock::at(1_000),
    )
    .expect("valid");
    let repo = ArtifactRef("repo:core".into());

    // The zookie is the read-your-writes fence; an empty zookie reads at latest (no prior write to
    // fence on — the endorsement relation was seeded, not written through this gate's `grant_relation`).
    let d = gate.fork_endorsement_check(&subject("p:maint", "acme"), &repo, Zookie(String::new()), false);
    assert!(is_allow(&d), "a maintainer with approve_untrusted_ci endorses (X-1, live)");
    let d = gate.fork_endorsement_check(&subject("p:bob", "acme"), &repo, Zookie(String::new()), false);
    assert!(!is_allow(&d), "an outsider cannot endorse (X-1, fail-closed)");
}

// ───────────────────────────── the CHAINED e2e (the GIT-P14 gate) ────────────────────────────────

/// **THE CHAINED DRILL (GIT-P14, the M3-G2 gate): grant → read-your-writes → break Id → degrade
/// (not cascade) + just-revoked-denied.** Proven against the REAL engine end-to-end.
///
/// 1. **grant a relation** — alice is made a repo admin via the real `TupleStore::write_tuples`
///    (the grant returns a zookie fence).
/// 2. **read-your-writes within the zookie** — a check stamped with that zookie sees the just-granted
///    grant immediately (alice pulls). (Contract 4.10.)
/// 3. **break Id** — the forced, scoped, reversible dependency break (the engine becomes
///    unreachable).
/// 4. **assert DEGRADE, not cascade** — a bounded-stale read serves the last coarse grant STATIC
///    (already-authorised traffic survives), within `static_max ≤ revocation SLA`.
/// 5. **assert just-revoked-DENIED** — even with a cached ALLOW + the Id still broken, a revoked
///    subject is denied THROUGH the stale cache (the cached ALLOW never overrides a revoke).
#[test]
fn chained_grant_read_your_writes_break_degrade_revoked_denied() {
    let s = scope("acme");
    // The grant is via the real engine's TupleStore; build the store first so we can write to it,
    // then wrap it. We seed alice's admin grant DIRECTLY (step 1, the real write_tuples path).
    let store = TupleStore::new(OutboxStore::new());
    let admin = subject("p-admin", "acme");

    // 1. GRANT a relation (alice → repo admin) — the real atomic write returns the zookie fence.
    let zookie = store
        .write_tuples(
            &s,
            &admin,
            &[add("repo:core", "admin", "p:alice")],
            None,
            None,
            Timestamp("2026-06-21T00:00:00Z".into()),
        )
        .expect("grant alice admin");
    assert!(!zookie.0.is_empty(), "the grant returns a read-your-writes zookie fence");

    let svc = StoreBackedCheck::new(store);
    for admit in svc.admit_git_fragment() {
        assert!(matches!(admit, myelin_identity::FragmentAdmit::Admitted { .. }));
    }
    let gate = GitCheckGate::try_new_with_clock(
        BreakableId::new(svc, s.clone()),
        REVOCATION_SLA,
        &threshold(),
        TestClock::at(1_000),
    )
    .expect("valid");

    let repo = ArtifactRef("repo:core".into());
    let pull = Permission(perm::PULL.into());
    let alice = subject("p:alice", "acme");

    // 2. READ-YOUR-WRITES within the zookie — the just-granted admin grant is visible NOW. The
    //    front-door read (bounded-stale) caches the coarse ALLOW (and is fresh — the engine is up).
    let d = gate.front_door_check(&alice, &pull, &repo, zookie.clone(), false);
    assert!(is_allow(&d), "read-your-writes: the just-granted admin pulls immediately (4.10)");
    assert_eq!(d.served, AuthzServed::Fresh, "served fresh from the live engine");

    // 3. BREAK Id (the forced, reversible, scoped dependency break).
    gate.id_ref().set_broken(true);

    // 4. DEGRADE, not cascade — just past fresh_ttl, inside static_max: the bounded-stale read serves
    //    the last coarse grant STATIC (the availability win), NOT a fail-closed cascade.
    gate.clock().advance(31);
    let d = gate.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false);
    assert!(d.is_degraded(), "the BoundedStale read DEGRADES (served STATIC) during the Id break");
    assert!(is_allow(&d), "the degraded answer is the cached ALLOW — already-authorised survives");

    // observability is part of the pass (EI-01 §3): a stale answer was observed; its age ≤ static_max.
    let sig = gate.signals();
    assert!(sig.stale >= 1, "the degrade is observable (fresh/stale/closed ratio signal)");
    assert!(
        sig.last_staleness_secs <= gate.static_max(),
        "staleness age ≤ static_max ≤ revocation SLA (a degrade never outlives the bound)"
    );

    // 5. JUST-REVOKED DENIED — alice is revoked. Even with a cached ALLOW + the Id still broken (so a
    //    fresh re-check is impossible), the revocation consult denies her THROUGH the stale cache.
    let d = gate.front_door_check(&alice, &pull, &repo, Zookie(String::new()), /*revoked*/ true);
    assert_eq!(d.served, AuthzServed::Revoked, "a revoked subject is denied through the cache");
    assert!(!is_allow(&d), "the cached ALLOW does NOT override the revoke (0 stale escalation)");

    // 6. RECOVER (reversible) → the next read is fresh again (the break left no cascade behind).
    gate.id_ref().set_broken(false);
    let d = gate.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false);
    assert_eq!(d.served, AuthzServed::Fresh, "recovered → fresh again (the degrade was bounded)");
}

// ───────────────────────────── CDC 4.11: the FailStatic bound (degrade-not-cascade) ──────────────

/// **CDC 4.11 — the FailStatic bound: a zookie (Strong) read BYPASSES the cache + fails CLOSED on a
/// break.** The merge gate's read-your-writes is a security-sensitive transition; it never serves
/// stale (the new-enemy guard, 4.10). Proven through the real engine.
#[test]
fn cdc_4_11_strong_merge_read_bypasses_cache_fails_closed_on_break() {
    let s = scope("acme");
    // alice is a repo admin → pull_request.merge resolves via parent_repo->protected_push (= admin).
    // The PR points at its parent repo via the parent_repo relation.
    let svc = engine_with_git_fragment(
        &s,
        &[
            add("repo:core", "admin", "p:alice"),
            add("pull_request:core:42", "parent_repo", "repo:core#protected_push"),
        ],
    );
    let gate = GitCheckGate::try_new_with_clock(
        BreakableId::new(svc, s.clone()),
        REVOCATION_SLA,
        &threshold(),
        TestClock::at(1_000),
    )
    .expect("valid");
    let pr = ArtifactRef("pull_request:core:42".into());
    let alice = subject("p:alice", "acme");

    // healthy: the strong merge read serves the authoritative engine directly (cache bypassed). The
    // empty zookie reads at latest (the merge gate would carry the grant's zookie for read-your-writes).
    let d = gate.merge_check(&alice, &pr, Zookie(String::new()), false);
    assert_eq!(d.served, AuthzServed::SourceBypass, "a Strong merge read bypasses the cache");
    assert!(is_allow(&d), "alice (admin → protected_push → merge) may merge");

    // BREAK Id: the strong read fails CLOSED (never serves stale) — the new-enemy guard.
    gate.id_ref().set_broken(true);
    let d = gate.merge_check(&alice, &pr, Zookie(String::new()), false);
    assert_eq!(d.served, AuthzServed::BypassClosed, "a Strong read fails CLOSED on a break");
    assert!(!is_allow(&d), "a security-sensitive merge read never serves stale (4.10)");
}

/// **CDC 4.11 — `static_max ≤ revocation SLA` is structural.** A thresholds row whose `static_max`
/// seed exceeds the revocation SLA does NOT construct the gate (a revoked actor could otherwise
/// outlive N). The bound is enforced in the constructor, never the hot path.
#[test]
fn cdc_4_11_static_max_over_revocation_sla_does_not_construct() {
    let s = scope("acme");
    let svc = engine_with_git_fragment(&s, &[]);
    let bad = FailStaticThreshold {
        status: "OPEN — LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 400, // > revocation SLA (300)
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    };
    let built = GitCheckGate::try_new_with_clock(
        BreakableId::new(svc, s.clone()),
        REVOCATION_SLA,
        &bad,
        TestClock::at(0),
    );
    assert!(
        matches!(built, Err(myelin_substrate::FailStaticError::ExceedsRevocationSla { .. })),
        "a static_max > revocation SLA must NOT construct (4.11, the §8.2 bound is structural)"
    );
}

/// **CDC 4.11 — past `static_max` a sustained break fails CLOSED (deny is correct again), never
/// open.** The staleness budget is bounded; past the window the degrade ends in a fail-closed deny,
/// never an open fall-through. Proven through the real engine.
#[test]
fn cdc_4_11_past_static_max_sustained_break_fails_closed() {
    let s = scope("acme");
    let svc = engine_with_git_fragment(&s, &[add("repo:core", "admin", "p:alice")]);
    let gate = GitCheckGate::try_new_with_clock(
        BreakableId::new(svc, s.clone()),
        REVOCATION_SLA,
        &threshold(),
        TestClock::at(1_000),
    )
    .expect("valid");
    let repo = ArtifactRef("repo:core".into());
    let pull = Permission(perm::PULL.into());
    let alice = subject("p:alice", "acme");

    assert!(is_allow(&gate.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false)));
    gate.id_ref().set_broken(true);
    gate.clock().advance(301); // past static_max (300)
    let d = gate.front_door_check(&alice, &pull, &repo, Zookie(String::new()), false);
    assert_eq!(d.served, AuthzServed::Closed, "past static_max → Closed (deny is correct), never open");
    assert!(!is_allow(&d));
}
