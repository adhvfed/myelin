//! # The LIVE per-repo object-authorization seam — the R0.3 wire gate backed by the real Identity
//! `check` (R2.1a: the R0.3 seed made real)
//!
//! [`crate::repo_authz`] declares the [`RepoAuthorizer`] trait + the fixtures (`AllowAllRepos` /
//! `DenyAllRepos` / `GrantBackedRepos`) that proved the wire seam is load-bearing. This module is the
//! **production** implementation the R0.3 module's `TODO(R2)` promised: a [`RepoAuthorizer`] that
//! resolves every `(principal, repo, access)` wire decision through the SAME doctrinal Git check path
//! the front door / merge gate use — [`GitCheckGate`] over a live [`IdentityService`] (the
//! `StoreBackedCheck` engine over the durable ReBAC tuple store, with the frozen Git fragment
//! admitted). **One engine, one cache, one check path — never a bespoke authz path** (git-hosting
//! §1.2 / contract 4.9 / 4.11).
//!
//! ## The mapping (the whole adapter)
//! - [`RepoAccess::Read`] → the frozen `repo.pull` permission ([`perm::PULL`]); `Write` → `repo.push`
//!   ([`perm::PUSH`]). The Git fragment compiles `pull = reader ∪ writer ∪ admin ∪ …` and `push =
//!   writer ∪ admin ∪ …`, so an `admin` grant satisfies both (the bootstrap-grant creator can
//!   immediately clone AND push).
//! - The repo object is the type-prefixed **`repo:<slug>`** [`ArtifactRef`] — the EXACT grammar the
//!   Git fragment / `live_check` / the bootstrap grant key on (see [`repo_object_ref`]). The check
//!   scope is the VERIFIED principal's `(tenant, region)` (tenant-from-token), so the slug alone is
//!   unambiguous inside the tenant partition (there is no cross-tenant tuple path — ID-D3); the wire's
//!   `repo_loc` already pins the repo's region to the token's region, so the grant written at
//!   create-repo and the check at the wire read the SAME `(tenant, region)` partition.
//! - The read is **bounded-stale** ([`GitCheckGate::front_door_check`]) — the clone/fetch/push hot path
//!   degrades (serves the last coarse grant) on a transient Identity hiccup instead of cascading closed;
//!   a just-revoked subject is still DENIED through the stale cache (the `subject_revoked` consult,
//!   derived exactly as `StoreBackedCheck::check`'s own S7 consult does).
//! - **Fail-closed:** only an `Allow` (fresh OR coarse-stale) admits; Deny / Conditional / a fail-closed
//!   error all map to `false` — the wire then returns a 0-leak 404 (read) or a 403 (write).
//!
//! ## The bootstrap grant (why a fresh repo is reachable at all)
//! Repo creation writes NO ReBAC tuples on its own, so under this deny-by-default authorizer every repo
//! would be born unreachable. [`TupleStoreGrantWriter`] is the create-path seam that writes the
//! creator→admin tuple (`repo:<slug>#admin@<principal>`) so the creator can immediately pull/push
//! (admin ⊇ pull+push). It writes through [`TupleStore::write_tuples`] (contract 4.6 — never raw SQL),
//! fail-loud: if the grant write fails the create-repo call fails (grant-first, so a repo is never born
//! on disk without its grant — no orphaned unreachable repo).

use crate::repo_authz::{RepoAccess, RepoAuthorizer};
use myelin_events::Timestamp;
use myelin_git::core::RepoLoc;
use myelin_git::live_check::{is_allow, perm, GitCheckGate};
use myelin_identity::{
    IdentityService, ObjectId, Permission, Principal, RelName, RelationTuple, RevokeTarget, TupleDelta,
    Zookie,
};
use myelin_identity_service::{RevocationStore, TupleStore};
use myelin_storage::TenantScope;
use myelin_tenancy::ArtifactRef;

/// The frozen Git repo object-type name (§5; mirrors `myelin_git::rebac_fragment` +
/// `myelin_identity_service::git_fragment::object_types::REPO` + the `live_check` tests). Spelled here
/// so the wire adapter keys the SAME `repo:<id>` object grammar the fragment + the bootstrap grant use.
const REPO_OBJECT_TYPE: &str = "repo";

/// The frozen `repo.admin` relation name — the bootstrap grant the creator receives. `pull = reader ∪
/// writer ∪ admin` and `push = writer ∪ admin`, so `admin` confers both (git-hosting §5). Mirrors the
/// `admin` relation declared in `git_fragment::repo_fragment`.
const REPO_ADMIN_RELATION: &str = "admin";

/// **The repo object [`ArtifactRef`] the wire check keys on — `repo:<slug>`.** The type-prefixed grammar
/// the Git fragment resolves (`type_of_object_ref("repo:widgets") == "repo"`) and the bootstrap grant
/// writes against. The slug alone (not tenant-qualified) is unambiguous: the check reads ONLY the
/// verified `(tenant, region)` partition (ID-D3), so two tenants' identically-named repos never collide.
pub fn repo_object_ref(repo: &RepoLoc) -> ArtifactRef {
    ArtifactRef(format!("{REPO_OBJECT_TYPE}:{}", repo.repo))
}

/// The `repo:<slug>#admin@<principal>` object-id string the bootstrap grant writes (the same object id
/// [`repo_object_ref`] builds — so the grant written at create-repo and the check at the wire agree
/// byte-for-byte).
fn repo_object_id(repo: &RepoLoc) -> String {
    format!("{REPO_OBJECT_TYPE}:{}", repo.repo)
}

// ───────────────────────────── the LIVE wire object-authz adapter ────────────────────────────────

/// **The production [`RepoAuthorizer`] — the R0.3 wire seam backed by the real Identity `check`
/// (R2.1a).** Wraps the doctrinal [`GitCheckGate`] (the FailStatic-bounded git→Id check) so every wire
/// `(principal, repo, access)` decision resolves through the SAME engine + cache the front door /
/// merge gate use — no bespoke authz path. Generic over the [`IdentityService`] so production wires the
/// durable `StoreBackedCheck::with_pg` and a DB-free test wires an in-memory `StoreBackedCheck::new`.
pub struct GitCheckRepoAuthorizer<I: IdentityService + Send + Sync> {
    /// The FailStatic-bounded git→Id check gate (the one doctrinal check path).
    gate: GitCheckGate<I>,
    /// The S7 revocation denylist — consulted to derive `subject_revoked` exactly as
    /// `StoreBackedCheck::check` does (a just-revoked subject is denied even through the stale cache).
    revocations: RevocationStore,
}

impl<I: IdentityService + Send + Sync> GitCheckRepoAuthorizer<I> {
    /// Compose the live wire authorizer over the git→Id check gate + the S7 revocation store. In
    /// production the `revocations` is the SAME durable denylist the gate's `StoreBackedCheck` consults
    /// (one revocation oracle); a test passes an in-memory `RevocationStore` it can revoke into.
    pub fn new(gate: GitCheckGate<I>, revocations: RevocationStore) -> Self {
        Self { gate, revocations }
    }
}

impl<I: IdentityService + Send + Sync> RepoAuthorizer for GitCheckRepoAuthorizer<I> {
    fn authorize_repo(&self, principal: &Principal, repo: &RepoLoc, access: RepoAccess) -> bool {
        // Read → pull, Write → push (the frozen 4.9 permission names). One place maps the access.
        let permission = match access {
            RepoAccess::Read => Permission(perm::PULL.to_string()),
            RepoAccess::Write => Permission(perm::PUSH.to_string()),
        };
        let object = repo_object_ref(repo);

        // The S7 revocation consult — derived exactly as `StoreBackedCheck::check` does (P-ID-14): the
        // scope is the SUBJECT's own verified (tenant, region), the target is the principal, the `now`
        // is the zero instant (a principal denylist entry carries no TTL). A revoked subject is then
        // denied through the fail-static cache (a cached ALLOW never overrides a revoke).
        let scope = TenantScope::from_verified_token(principal, principal.region.clone());
        let target = RevokeTarget::Principal(principal.principal_id.clone());
        let subject_revoked = self
            .revocations
            .is_revoked(&scope, &target, &Timestamp(String::new()));

        // The bounded-stale front-door check (the clone/fetch/push hot path degrades, never cascades).
        // The wire has no read-your-writes token, so the empty/current zookie — the bounded-stale
        // callers' convention. Only a real Allow (fresh or coarse-stale) admits; everything else
        // (Deny / Conditional / a fail-closed hiccup past the window) is false — fail-closed.
        let decision =
            self.gate
                .front_door_check(principal, &permission, &object, Zookie(String::new()), subject_revoked);
        is_allow(&decision)
    }
}

// ───────────────────────────── the create-path bootstrap-grant seam ──────────────────────────────

/// **The create-repo bootstrap-grant seam.** Repo creation writes no ReBAC tuples on its own, so under
/// the deny-by-default [`GitCheckRepoAuthorizer`] a fresh repo would be born unreachable. An impl of
/// this trait, injected into the durable git backend's create path, writes the creator→admin grant so
/// the creator can immediately pull/push. Absent (no seam injected) = no write (DB-free tests
/// unchanged); present + the write FAILS = create-repo FAILS (fail-loud — never an orphaned repo).
pub trait RepoGrantWriter: Send + Sync {
    /// Write the `repo:<slug>#admin@<principal>` bootstrap grant. Returns `Err(reason)` on any write
    /// failure so the create path can fail loud (the repo is not created — no unreachable orphan).
    fn grant_repo_admin(&self, principal: &Principal, repo: &RepoLoc) -> Result<(), String>;
}

/// **The production bootstrap-grant writer over the durable ReBAC tuple store (contract 4.6).** Writes
/// the creator→admin tuple through [`TupleStore::write_tuples`] — the SAME atomic, outbox-co-committing
/// write path Identity's grants use, never raw SQL. Holds a [`TupleStore`] cloned from (or over the same
/// durable `rebac_tuple` backing as) the `StoreBackedCheck` the wire authorizer checks against, so the
/// grant is visible to the very next wire `check`.
pub struct TupleStoreGrantWriter {
    tuples: TupleStore,
}

impl TupleStoreGrantWriter {
    /// Wrap the durable (or in-memory-test) tuple store the creator→admin grant is written into.
    pub fn new(tuples: TupleStore) -> Self {
        Self { tuples }
    }
}

impl RepoGrantWriter for TupleStoreGrantWriter {
    fn grant_repo_admin(&self, principal: &Principal, repo: &RepoLoc) -> Result<(), String> {
        // The write scope is the creator's own verified (tenant, region) (never a path — the
        // tenant-predicate floor); the grant lands in that partition, the same one the wire check reads.
        let scope = TenantScope::from_verified_token(principal, principal.region.clone());
        let delta = TupleDelta::Add(RelationTuple {
            object: ObjectId(repo_object_id(repo)),
            relation: RelName(REPO_ADMIN_RELATION.to_string()),
            subject: principal.principal_id.clone(),
            caveat: None,
        });
        self.tuples
            .write_tuples(&scope, principal, &[delta], None, None, grant_occurred_at())
            .map(|_zookie| ())
            .map_err(|e| format!("bootstrap creator→admin grant write failed: {e}"))
    }
}

/// The `occurred_at` stamped on the bootstrap-grant's `iam.tuple_written` event. The substrate clock
/// injection is the production composition-root's job; a fixed RFC-3339 stamp is sufficient here (the
/// edge does not drain the outbox — the relay does), mirroring the edge git backend's `emit_ctx`
/// convention. The stamp does not affect the grant's visibility to `check` (that is keyed on the tuple
/// edge + the verified partition, not the event time).
fn grant_occurred_at() -> Timestamp {
    Timestamp("2026-07-15T00:00:00Z".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::OutboxStore;
    use myelin_identity::{FragmentAdmit, PrincipalId, PrincipalKind};
    use myelin_identity_service::StoreBackedCheck;
    use myelin_substrate::FailStaticThreshold;
    use myelin_tenancy::{Region, TenantId};

    const REGION: &str = "eu-west";
    const REVOCATION_SLA: u64 = 300;

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

    fn principal(id: &str, tenant: &str) -> Principal {
        let mut p = Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Service,
            TenantId(tenant.into()),
        );
        p.region = Region(REGION.into());
        p
    }

    fn scope_of(p: &Principal) -> TenantScope {
        TenantScope::from_verified_token(p, p.region.clone())
    }

    fn add(object: &str, relation: &str, subj: &str) -> TupleDelta {
        TupleDelta::Add(RelationTuple {
            object: ObjectId(object.into()),
            relation: RelName(relation.into()),
            subject: PrincipalId(subj.into()),
            caveat: None,
        })
    }

    fn admit(svc: &StoreBackedCheck) {
        for a in svc.admit_git_fragment() {
            assert!(
                matches!(a, FragmentAdmit::Admitted { .. }),
                "the Git fragment admits into the live cell schema: {a:?}"
            );
        }
    }

    /// A live in-memory `StoreBackedCheck` seeded with `tuples` + the Git fragment admitted (the DB-free
    /// entry the task names — `StoreBackedCheck::new(TupleStore::new(outbox))`).
    fn check_with(scope: &TenantScope, tuples: &[TupleDelta]) -> StoreBackedCheck {
        let store = TupleStore::new(OutboxStore::new());
        let seed_admin = principal("seed-admin", scope.tenant().as_str());
        store
            .write_tuples(scope, &seed_admin, tuples, None, None, grant_occurred_at())
            .expect("seed tuples");
        let svc = StoreBackedCheck::new(store);
        admit(&svc);
        svc
    }

    fn adapter(
        check: StoreBackedCheck,
        revocations: RevocationStore,
    ) -> GitCheckRepoAuthorizer<StoreBackedCheck> {
        let gate =
            GitCheckGate::try_new(check, REVOCATION_SLA, &threshold()).expect("valid staleness bound");
        GitCheckRepoAuthorizer::new(gate, revocations)
    }

    fn repo_loc(tenant: &str, slug: &str) -> RepoLoc {
        RepoLoc::new(tenant, REGION, slug)
    }

    /// **Allow→true, un-granted→false; the un-granted-repo-reach hole is closed.** An `admin` grant maps
    /// to Allow on BOTH accesses (admin ⊇ pull+push); an in-tenant principal with no grant is denied
    /// both; a different repo is not reachable off the grant.
    #[test]
    fn admin_grant_allows_both_ungranted_denies_both() {
        let alice = principal("p:alice", "acme");
        let a = adapter(
            check_with(&scope_of(&alice), &[add("repo:widgets", "admin", "p:alice")]),
            RevocationStore::new(),
        );
        let widgets = repo_loc("acme", "widgets");
        assert!(a.authorize_repo(&alice, &widgets, RepoAccess::Read), "admin ⊇ pull");
        assert!(a.authorize_repo(&alice, &widgets, RepoAccess::Write), "admin ⊇ push");

        let mallory = principal("p:mallory", "acme");
        assert!(
            !a.authorize_repo(&mallory, &widgets, RepoAccess::Read),
            "an in-tenant principal with NO grant is denied (0-leak read)"
        );
        assert!(!a.authorize_repo(&mallory, &widgets, RepoAccess::Write));

        let secrets = repo_loc("acme", "secrets");
        assert!(
            !a.authorize_repo(&alice, &secrets, RepoAccess::Read),
            "a different repo is not reachable off the widgets grant"
        );
    }

    /// **Read→PULL, Write→PUSH.** A `reader` grant confers pull (Read admits) but NOT push (Write denied)
    /// — proving the access→permission mapping is exactly reader⊂pull / writer⊂push, not flattened.
    #[test]
    fn read_grant_maps_to_pull_only_write_needs_push() {
        let reader = principal("p:reader", "acme");
        let a = adapter(
            check_with(&scope_of(&reader), &[add("repo:widgets", "reader", "p:reader")]),
            RevocationStore::new(),
        );
        let widgets = repo_loc("acme", "widgets");
        assert!(
            a.authorize_repo(&reader, &widgets, RepoAccess::Read),
            "a reader grant confers pull (Read→PULL)"
        );
        assert!(
            !a.authorize_repo(&reader, &widgets, RepoAccess::Write),
            "a reader grant does NOT confer push (Write→PUSH, the stricter access)"
        );
    }

    /// **A `writer` grant confers both** (push = writer ∪ admin; pull = reader ∪ writer ∪ admin).
    #[test]
    fn writer_grant_confers_pull_and_push() {
        let dev = principal("p:dev", "acme");
        let a = adapter(
            check_with(&scope_of(&dev), &[add("repo:widgets", "writer", "p:dev")]),
            RevocationStore::new(),
        );
        let widgets = repo_loc("acme", "widgets");
        assert!(a.authorize_repo(&dev, &widgets, RepoAccess::Read));
        assert!(a.authorize_repo(&dev, &widgets, RepoAccess::Write));
    }

    /// **A revoked subject is denied even WITH an admin grant** (the S7 consult denies through the
    /// fail-static cache — a stale/fresh ALLOW never overrides a revoke).
    #[test]
    fn revoked_subject_is_denied_even_with_a_grant() {
        let alice = principal("p:alice", "acme");
        let s = scope_of(&alice);
        let revocations = RevocationStore::new();
        let a = adapter(
            check_with(&s, &[add("repo:widgets", "admin", "p:alice")]),
            revocations.clone(),
        );
        let widgets = repo_loc("acme", "widgets");
        assert!(
            a.authorize_repo(&alice, &widgets, RepoAccess::Read),
            "pre-revocation: the admin grant admits"
        );

        revocations.revoke(
            &s,
            &RevokeTarget::Principal(alice.principal_id.clone()),
            grant_occurred_at(),
        );
        assert!(
            !a.authorize_repo(&alice, &widgets, RepoAccess::Read),
            "a revoked subject is denied even with a grant (Read)"
        );
        assert!(
            !a.authorize_repo(&alice, &widgets, RepoAccess::Write),
            "…and Write"
        );
    }

    /// **The bootstrap grant makes a fresh repo usable.** A repo with no tuples is unreachable; writing
    /// the creator→admin grant through the SAME tuple store the check reads flips it to pull+push-able.
    #[test]
    fn bootstrap_grant_writer_makes_a_fresh_repo_pull_and_push_able() {
        let creator = principal("p:creator", "acme");
        let store = TupleStore::new(OutboxStore::new());
        // The check reads the SAME store the grant-writer writes into (a shared Arc inner).
        let check = StoreBackedCheck::new(store.clone());
        admit(&check);
        let a = adapter(check, RevocationStore::new());
        let widgets = repo_loc("acme", "widgets");

        assert!(
            !a.authorize_repo(&creator, &widgets, RepoAccess::Read),
            "no grant yet → the fresh repo is unreachable (deny-by-default)"
        );

        TupleStoreGrantWriter::new(store)
            .grant_repo_admin(&creator, &widgets)
            .expect("bootstrap creator→admin grant");

        assert!(
            a.authorize_repo(&creator, &widgets, RepoAccess::Read),
            "the bootstrap grant makes the repo pull-able"
        );
        assert!(
            a.authorize_repo(&creator, &widgets, RepoAccess::Write),
            "…and push-able (admin ⊇ push)"
        );
    }
}
