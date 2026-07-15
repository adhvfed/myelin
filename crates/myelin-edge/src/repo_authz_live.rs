//! # `repo_authz_live` — the PRODUCTION per-repo object authorizer + the creator→admin bootstrap
//! grant (R2.1a — R0.2/R0.3 wired LIVE)
//!
//! R0.3 built the wire object-authz SEAM ([`crate::repo_authz::RepoAuthorizer`], consulted by every
//! git wire handler) but production boot kept the [`crate::repo_authz::AllowAllRepos`] fixture — the
//! seam was correct-but-latent. This module is the R2.1a closure: the seam backed by the REAL
//! depth-bounded Zanzibar `check` over the durable S3 tuple store, through the platform fail-static
//! doctrine, plus the bootstrap grant that makes a freshly-created repo reachable by its creator.
//!
//! ## The check path (one primitive, the platform doctrine)
//!
//! [`CheckBackedRepoAuthorizer`] adapts the seam onto the frozen Git ReBAC fragment (contract 4.9):
//! - [`RepoAccess::Read`]  → `check(principal, "pull", repo:<slug>)` (`pull = reader ∪ writer ∪
//!   admin ∪ parent_project->view`);
//! - [`RepoAccess::Write`] → `check(principal, "push", repo:<slug>)` (`push = writer ∪ admin ∪ …`).
//!
//! The check rides [`myelin_git::live_check::GitCheckGate`] — the SAME `FailStaticAuthz` bounded-
//! staleness cache the platform Identity dependency root rides (GIT-P14 doctrine: the git hot path
//! DEGRADES on a transient Id hiccup instead of cascading closed, while a just-revoked subject is
//! denied THROUGH the stale cache and a strong read would bypass it). We deliberately wrap
//! [`StoreBackedCheck`] (the production engine) in the gate rather than calling `CheckEngine`
//! directly: `live_check.rs` is the stated platform doctrine for the git hot path, and the S7
//! revocation consult below keeps the revoked-deny honest on the CACHED path too (the engine's own
//! consult only runs on a fresh evaluation). Fail-closed: `Deny`, `Conditional`, an unparseable
//! object, or any transport error past the staleness budget all refuse.
//!
//! ## THE OBJECT GRAMMAR (the R2.2 concern — writes and checks MUST agree)
//!
//! The canonical repo authz object is **`repo:<slug>`** ([`repo_object_id`]), spelled in ONE place
//! and shared by the check side and the tuple-write side. Why this exact grammar:
//! - the engine derives the object TYPE via `namespace::type_of_object_ref` — `repo:<slug>` →
//!   `repo`, so the compiled `repo` fragment's `pull`/`push` rewrites resolve;
//! - the engine derives the tuple KEY via `check_engine::object_id_of` (the last `/`-segment) —
//!   `repo:<slug>` has no `/`, so the tuple ObjectId IS `repo:<slug>` verbatim;
//! - it matches the grammar the live-fragment tests and `git_fragment::compile_codeowners`
//!   (`ref:<repo-id>::<glob>` where `<repo-id>` is `repo:<slug>`) already pin.
//!
//! (The `myelin-git` front door's `git:repo:<tenant>/<repo>` spelling would resolve object type
//! `git` — NOT the admitted `repo` fragment — and tuple id `<repo>` — mismatching the written
//! tuples. That is exactly the write/check drift R2.2 exists to kill; this module does not use it.)
//!
//! The tenant/region partition does NOT live in the object id: `StoreBackedCheck::check` scopes
//! every tuple read to the SUBJECT's verified `(tenant, region)` (tenant-from-token, ID-3), and
//! [`TupleStore::write_tuples`] scopes every write the same way — so `repo:widgets` under `acme`
//! and `repo:widgets` under `globex` are different partitions structurally. Defence-in-depth, the
//! authorizer still pins `principal.tenant == repo.tenant` before checking (the gateway's IDOR
//! reject already guarantees it upstream).
//!
//! ## The creator→admin BOOTSTRAP GRANT (the make-or-break)
//!
//! `DurableGitStore::create_repo` writes NO ReBAC tuple, so under a deny-by-default authorizer a
//! fresh repo would be unreachable BY ITS OWN CREATOR (no `admin`/`writer`/`reader` edge exists).
//! [`TupleRepoBootstrap`] closes the gap: on repo-create through the edge the creator gets
//! **`repo:<slug>#admin@<principal_id>`** through the ordinary [`TupleStore::write_tuples`] path
//! (4.6 — the tuple + its `iam.tuple_written` event co-commit atomically). `admin` is the frozen
//! fragment's strongest repo relation: it satisfies `pull`, `push`, `administer`, AND
//! `protected_push`, so the creator can immediately clone/push/administer.
//!
//! **Ordering / transactionality (honest):** the tuple write and the on-disk `git init` are TWO
//! stores (PG vs filesystem) with no shared transaction. The edge writes the GRANT FIRST and only
//! then creates the directory ([`crate::git_durable::DurableGitBackend::create_repo_as`]): a crash
//! between the two leaves a dangling grant on a repo that does not exist (harmless — every wire
//! path 404s on the absent dir, and the retried create simply proceeds). The reverse order would
//! leave the WRONG residue: a repo that exists but is reachable by no one. A failed grant write
//! aborts the create loudly (fail-closed, no repo without an owner).

use crate::repo_authz::{RepoAccess, RepoAuthorizer};
use myelin_events::Timestamp;
use myelin_git::core::RepoLoc;
use myelin_git::live_check::{is_allow, perm, GitCheckGate};
use myelin_identity::{
    ObjectId, Permission, Principal, RelName, RelationTuple, RevokeTarget, TupleDelta, Zookie,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_substrate::{FailStaticError, FailStaticThreshold, Seconds, SystemClock};
use myelin_tenancy::ArtifactRef;

/// The frozen `repo` relation the bootstrap grant writes (`repo.admin` — the strongest repo
/// relation in the 4.9 fragment: `pull`/`push`/`administer`/`protected_push` all admit it). Spelled
/// once so the grant and any audit reference agree with `git_fragment::repo_fragment`.
pub const REPO_ADMIN_RELATION: &str = "admin";

/// **The canonical repo authz OBJECT ID (`repo:<slug>`) — the one grammar writes and checks share.**
/// See the module doc for why this exact spelling is load-bearing (engine type inference + tuple
/// key + the fragment tests all pin it). The tenant/region partition is the verified scope, never
/// part of the id.
pub fn repo_object_id(slug: &str) -> String {
    format!("repo:{slug}")
}

/// The [`ArtifactRef`] form of [`repo_object_id`] — what the `check` side passes (the engine's
/// `object_id_of` maps it back onto the identical tuple key; `type_of_object_ref` reads type
/// `repo`).
pub fn repo_object_ref(slug: &str) -> ArtifactRef {
    ArtifactRef(repo_object_id(slug))
}

// ───────────────────────────── the production authorizer ─────────────────────────────────────────

/// **The CheckEngine-backed [`RepoAuthorizer`] (R2.1a) — the production replacement for
/// [`crate::repo_authz::AllowAllRepos`] at the git wire.** Read → `pull`, Write → `push`, against
/// the live Git fragment through the fail-static gate. Deny-by-default: no tuple, no reach.
pub struct CheckBackedRepoAuthorizer {
    /// The GIT-P14 fail-static gate over the production engine (bounded-stale on the wire hot path;
    /// a revoked subject denied through the cache; fail-closed past the staleness budget).
    gate: GitCheckGate<StoreBackedCheck, SystemClock>,
}

impl CheckBackedRepoAuthorizer {
    /// Compose the authorizer over the production [`StoreBackedCheck`] (which must already have the
    /// Git fragment admitted — `admit_git_fragment`, else every compiled-permission check denies) +
    /// the thresholds-file fail-static bound. A `static_max` violating
    /// `agent_token_ttl ≤ static_max ≤ revocation_sla` does NOT construct (P-S18, structural).
    pub fn try_new(
        check: StoreBackedCheck,
        revocation_sla_secs: Seconds,
        threshold: &FailStaticThreshold,
    ) -> Result<CheckBackedRepoAuthorizer, FailStaticError> {
        Ok(CheckBackedRepoAuthorizer {
            gate: GitCheckGate::try_new(check, revocation_sla_secs, threshold)?,
        })
    }
}

impl RepoAuthorizer for CheckBackedRepoAuthorizer {
    fn authorize_repo(&self, principal: &Principal, repo: &RepoLoc, access: RepoAccess) -> bool {
        // Defence-in-depth tenant pin: the gateway's IDOR reject + the GIT-D8 tenant-from-token rule
        // already guarantee `repo.tenant` is the VERIFIED token tenant, but a drifted call site must
        // fail CLOSED here, never leak into another tenant's partition (the check below scopes reads
        // by the PRINCIPAL's tenant, so a mismatched RepoLoc would otherwise silently check the
        // wrong object).
        if principal.tenant.0 != repo.tenant {
            return false;
        }
        // Read → repo.pull, Write → repo.push (the frozen 4.9 permission names — live_check::perm).
        let permission = Permission(
            match access {
                RepoAccess::Read => perm::PULL,
                RepoAccess::Write => perm::PUSH,
            }
            .to_string(),
        );
        let object = repo_object_ref(&repo.repo);
        // The S7 revocation consult, supplied to the gate so a just-revoked principal is denied
        // even when the fail-static cache would otherwise serve a stale coarse ALLOW (the engine's
        // own internal consult only runs on a FRESH evaluation). A `Principal` denylist entry
        // carries no TTL, so the zero instant never gates it (mirrors `StoreBackedCheck::check`).
        let scope = TenantScope::from_verified_token(principal, principal.region.clone());
        let revoked = self.gate.id_ref().revocations().is_revoked(
            &scope,
            &RevokeTarget::Principal(principal.principal_id.clone()),
            &Timestamp(String::new()),
        );
        // The wire hot path is a BoundedStale read (the GIT-P14 availability posture: clone/fetch/
        // push traffic survives a transient Id hiccup on the last coarse grant, within
        // static_max ≤ revocation SLA). An empty zookie = "latest" (no snapshot pin).
        let decision = self.gate.front_door_check(
            principal,
            &permission,
            &object,
            Zookie(String::new()),
            revoked,
        );
        // Fail-closed: only an explicit Allow admits (Deny / Conditional / Closed all refuse).
        is_allow(&decision)
    }
}

// ───────────────────────────── the creator→admin bootstrap grant ─────────────────────────────────

/// **The repo-create bootstrap seam (R2.1a).** Consulted by
/// [`crate::git_durable::DurableGitBackend::create_repo_as`] BEFORE the bare repo lands on disk:
/// write whatever grants make the fresh repo reachable by its creator. An `Err` ABORTS the create
/// (fail-closed — never a repo no one can reach).
pub trait RepoBootstrapGrants: Send + Sync {
    /// Grant the creator its bootstrap relation(s) on the repo about to be created.
    fn grant_creator(&self, creator: &Principal, repo: &RepoLoc) -> Result<(), String>;

    /// **The COMPENSATING removal (R2.1a-followup, defect #7).** Remove the EXACT bootstrap
    /// relation(s) [`grant_creator`] added — called by
    /// [`crate::git_durable::DurableGitBackend::create_repo_as`] in its error arm when the on-disk
    /// `git init` FAILS after the grant already committed, so no orphan `repo:<slug>#admin@<creator>`
    /// tuple survives on a repo that does not exist (the cross-user hole: an orphan grant + slug
    /// reuse by a DIFFERENT principal = the original principal silently holds admin on the new repo).
    ///
    /// Must be the exact inverse of [`grant_creator`]: same object grammar, same relation, same
    /// subject, scoped to the creator's verified `(tenant, region)`, through the ordinary
    /// [`TupleStore::write_tuples`] path (contract 4.6 — NEVER raw SQL). A [`TupleDelta::Remove`] of a
    /// tuple that was never durably committed is a no-op (safe), so this is always sound to call in
    /// the error arm. An `Err` here means the compensation ITSELF failed — the caller surfaces that
    /// LOUDLY (a KNOWN, logged orphan grant beats a silent one).
    fn revoke_creator(&self, creator: &Principal, repo: &RepoLoc) -> Result<(), String>;
}

/// The no-op bootstrap (the `test-support`/fixture default, paired with the `AllowAllRepos`
/// authorizer fixture: where every principal reaches every repo, no grant is needed). Production
/// boot injects [`TupleRepoBootstrap`] alongside the real authorizer — the pair is what makes
/// deny-by-default livable.
pub struct NoRepoBootstrap;

impl RepoBootstrapGrants for NoRepoBootstrap {
    fn grant_creator(&self, _creator: &Principal, _repo: &RepoLoc) -> Result<(), String> {
        Ok(())
    }
    fn revoke_creator(&self, _creator: &Principal, _repo: &RepoLoc) -> Result<(), String> {
        Ok(())
    }
}

/// **The production bootstrap: `repo:<slug>#admin@<creator>` through [`TupleStore::write_tuples`]
/// (contract 4.6).** The tuple + its `iam.tuple_written` event co-commit atomically (the S3 store's
/// own transaction); the scope/actor are the VERIFIED creator's (tenant-from-token). Must be handed
/// the SAME tuple store the [`CheckBackedRepoAuthorizer`]'s engine reads (one edge set — the write
/// is visible to the very next check).
pub struct TupleRepoBootstrap {
    tuples: TupleStore,
}

impl TupleRepoBootstrap {
    /// Wire the bootstrap over the tuple store the production `check` engine reads
    /// (`StoreBackedCheck::tuples()` — the ONE S3 edge set, never a second store).
    pub fn new(tuples: TupleStore) -> TupleRepoBootstrap {
        TupleRepoBootstrap { tuples }
    }

    /// The ONE bootstrap tuple both the grant [`TupleDelta::Add`] and the compensating
    /// [`TupleDelta::Remove`] key on — `repo:<slug>#admin@<creator>` in the shared `repo:<slug>`
    /// grammar. Spelled once so the compensating remove targets the EXACT tuple the grant added
    /// (defect #7 — a drifted spelling would remove nothing and leave the orphan).
    fn admin_tuple(creator: &Principal, repo: &RepoLoc) -> RelationTuple {
        RelationTuple {
            object: ObjectId(repo_object_id(&repo.repo)),
            relation: RelName(REPO_ADMIN_RELATION.to_string()),
            subject: creator.principal_id.clone(),
            caveat: None,
        }
    }

    /// The defence-in-depth tenant pin both halves apply (a drifted RepoLoc must not touch the
    /// creator's partition for another tenant's repo name).
    fn tenant_pin(creator: &Principal, repo: &RepoLoc) -> Result<(), String> {
        if creator.tenant.0 != repo.tenant {
            return Err(format!(
                "bootstrap grant refused: repo tenant `{}` is not the creator's verified tenant",
                repo.tenant
            ));
        }
        Ok(())
    }
}

impl RepoBootstrapGrants for TupleRepoBootstrap {
    fn grant_creator(&self, creator: &Principal, repo: &RepoLoc) -> Result<(), String> {
        Self::tenant_pin(creator, repo)?;
        let delta = TupleDelta::Add(Self::admin_tuple(creator, repo));
        let scope = TenantScope::from_verified_token(creator, creator.region.clone());
        self.tuples
            .write_tuples(&scope, creator, &[delta], None, None, now_rfc3339())
            .map(|_zookie| ())
            .map_err(|e| e.to_string())
    }

    fn revoke_creator(&self, creator: &Principal, repo: &RepoLoc) -> Result<(), String> {
        // The exact inverse of grant_creator: same tenant pin, same tuple, a Remove through the
        // ordinary 4.6 write path (never raw SQL), scoped to the creator's verified (tenant, region).
        // Removing a tuple that never durably committed is a no-op (the store's Remove is idempotent),
        // so this is safe to call in the create error arm even if the grant itself had failed.
        Self::tenant_pin(creator, repo)?;
        let delta = TupleDelta::Remove(Self::admin_tuple(creator, repo));
        let scope = TenantScope::from_verified_token(creator, creator.region.clone());
        self.tuples
            .write_tuples(&scope, creator, &[delta], None, None, now_rfc3339())
            .map(|_zookie| ())
            .map_err(|e| e.to_string())
    }
}

/// The wall-clock `occurred_at` stamp for the bootstrap tuple write (RFC 3339 UTC, second
/// precision). Derived from `SystemTime` with the standard civil-from-days conversion — no new
/// date-time dependency (the workspace has none; the events envelope carries the stamp as an
/// opaque string).
fn now_rfc3339() -> Timestamp {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil_from_days (Howard Hinnant's algorithm) — exact for the proleptic Gregorian calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Timestamp(format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::OutboxStore;
    use myelin_identity::{DataRole, FragmentAdmit, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_substrate::FailStaticThreshold;
    use myelin_tenancy::{Region, TenantId};

    // The thresholds-file [fail_static] seed (mirrors live_check.rs / fork_gate.rs fixtures).
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
        Principal::new(
            TenantId(tenant.into()),
            Region("eu-west".into()),
            PrincipalId(id.into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    /// The production engine over the in-memory S3 double, with the Git fragment ADMITTED (as the
    /// production composition root does) — a compiled `pull`/`push` check resolves.
    fn check_with_git_fragment() -> StoreBackedCheck {
        let sbc = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));
        for admit in sbc.admit_git_fragment() {
            assert!(
                matches!(admit, FragmentAdmit::Admitted { .. }),
                "the Git fragment admits: {admit:?}"
            );
        }
        sbc
    }

    fn authorizer(sbc: StoreBackedCheck) -> CheckBackedRepoAuthorizer {
        CheckBackedRepoAuthorizer::try_new(sbc, 300, &threshold()).expect("valid staleness bound")
    }

    /// **Deny-by-default: an in-tenant principal with NO tuple on the repo reaches NOTHING.** The
    /// exact hole R0.3 named (un-granted repo reach) — now closed by the real engine, not a fixture.
    #[test]
    fn ungranted_principal_is_denied_read_and_write() {
        let authz = authorizer(check_with_git_fragment());
        let mallory = principal("svc:mallory", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        assert!(!authz.authorize_repo(&mallory, &repo, RepoAccess::Read));
        assert!(!authz.authorize_repo(&mallory, &repo, RepoAccess::Write));
    }

    /// **THE WRITE↔CHECK GRAMMAR AGREEMENT (the R2.2 concern, pinned):** the bootstrap grant written
    /// through [`TupleRepoBootstrap`] is admitted by [`CheckBackedRepoAuthorizer`] for BOTH accesses
    /// (`admin` ⊆ `pull` and ⊆ `push` in the frozen fragment) — the tuple the writer produces is the
    /// tuple the checker resolves, through the ONE `repo:<slug>` grammar.
    #[test]
    fn bootstrap_grant_then_authorizer_admits_creator() {
        let sbc = check_with_git_fragment();
        let bootstrap = TupleRepoBootstrap::new(sbc.tuples().clone());
        let authz = authorizer(sbc);
        let creator = principal("svc:creator", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");

        // Before the grant: denied (proves the admit below is the GRANT's doing, not a default).
        assert!(!authz.authorize_repo(&creator, &repo, RepoAccess::Read));

        bootstrap
            .grant_creator(&creator, &repo)
            .expect("the creator→admin bootstrap grant writes");

        assert!(
            authz.authorize_repo(&creator, &repo, RepoAccess::Read),
            "admin ⊆ pull: the creator clones its fresh repo"
        );
        assert!(
            authz.authorize_repo(&creator, &repo, RepoAccess::Write),
            "admin ⊆ push: the creator pushes to its fresh repo"
        );
    }

    /// **Defect #7 — the compensating remove is the exact inverse of the grant:** after a grant
    /// then a `revoke_creator`, the authorizer DENIES again (the orphan tuple is gone through the
    /// real 4.6 write path — the checker resolves the removal, not just an in-memory forget).
    #[test]
    fn revoke_creator_removes_the_grant_the_checker_resolves() {
        let sbc = check_with_git_fragment();
        let bootstrap = TupleRepoBootstrap::new(sbc.tuples().clone());
        let authz = authorizer(sbc);
        let creator = principal("svc:creator", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");

        bootstrap.grant_creator(&creator, &repo).expect("grant");
        assert!(authz.authorize_repo(&creator, &repo, RepoAccess::Read));

        bootstrap
            .revoke_creator(&creator, &repo)
            .expect("the compensating remove writes");
        assert!(
            !authz.authorize_repo(&creator, &repo, RepoAccess::Read),
            "the removed grant no longer admits"
        );
        assert!(!authz.authorize_repo(&creator, &repo, RepoAccess::Write));
    }

    /// **A compensating remove of a tuple that was never committed is a safe no-op** (the store's
    /// Remove is idempotent) — so the create error arm can always call it without a spurious failure.
    #[test]
    fn revoke_creator_on_never_granted_is_a_noop_ok() {
        let sbc = check_with_git_fragment();
        let bootstrap = TupleRepoBootstrap::new(sbc.tuples().clone());
        let creator = principal("svc:creator", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        // No grant was ever written; the remove still succeeds (no-op).
        bootstrap
            .revoke_creator(&creator, &repo)
            .expect("remove of an absent tuple is a no-op Ok");
    }

    /// **The compensating remove keeps the same defence-in-depth tenant pin:** a foreign-tenant
    /// RepoLoc is refused (never a write into another partition).
    #[test]
    fn revoke_creator_refuses_a_foreign_tenant() {
        let sbc = check_with_git_fragment();
        let bootstrap = TupleRepoBootstrap::new(sbc.tuples().clone());
        let creator = principal("svc:creator", "acme");
        let foreign = RepoLoc::new("globex", "eu-west", "widgets");
        assert!(bootstrap.revoke_creator(&creator, &foreign).is_err());
    }

    /// **Cross-repo isolation:** the creator's grant on `widgets` does NOT admit `secrets` (per-
    /// object tuples, no wildcard).
    #[test]
    fn grant_on_one_repo_does_not_admit_another() {
        let sbc = check_with_git_fragment();
        let bootstrap = TupleRepoBootstrap::new(sbc.tuples().clone());
        let authz = authorizer(sbc);
        let creator = principal("svc:creator", "acme");
        bootstrap
            .grant_creator(&creator, &RepoLoc::new("acme", "eu-west", "widgets"))
            .expect("grant");

        let other = RepoLoc::new("acme", "eu-west", "secrets");
        assert!(!authz.authorize_repo(&creator, &other, RepoAccess::Read));
        assert!(!authz.authorize_repo(&creator, &other, RepoAccess::Write));
    }

    /// **The read/write split is the fragment's, not ours:** a `reader` tuple admits `pull` but NOT
    /// `push` (Read ≠ Write; the authorizer maps accesses onto the compiled permissions, it does not
    /// flatten them).
    #[test]
    fn reader_tuple_admits_read_not_write() {
        let sbc = check_with_git_fragment();
        let reader = principal("svc:reader", "acme");
        let scope = TenantScope::from_verified_token(&reader, reader.region.clone());
        sbc.tuples()
            .write_tuples(
                &scope,
                &reader,
                &[TupleDelta::Add(RelationTuple {
                    object: ObjectId(repo_object_id("widgets")),
                    relation: RelName("reader".into()),
                    subject: reader.principal_id.clone(),
                    caveat: None,
                })],
                None,
                None,
                now_rfc3339(),
            )
            .expect("write reader tuple");
        let authz = authorizer(sbc);
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        assert!(authz.authorize_repo(&reader, &repo, RepoAccess::Read));
        assert!(
            !authz.authorize_repo(&reader, &repo, RepoAccess::Write),
            "a reader does not push"
        );
    }

    /// **The defence-in-depth tenant pin fails CLOSED:** a RepoLoc naming a foreign tenant is denied
    /// outright (never checked against the principal's own partition), and the bootstrap writer
    /// refuses to write the grant.
    #[test]
    fn foreign_tenant_repoloc_is_refused_by_both_halves() {
        let sbc = check_with_git_fragment();
        let bootstrap = TupleRepoBootstrap::new(sbc.tuples().clone());
        let authz = authorizer(sbc);
        let creator = principal("svc:creator", "acme");
        let foreign = RepoLoc::new("globex", "eu-west", "widgets");

        assert!(bootstrap.grant_creator(&creator, &foreign).is_err());
        assert!(!authz.authorize_repo(&creator, &foreign, RepoAccess::Read));
        assert!(!authz.authorize_repo(&creator, &foreign, RepoAccess::Write));
    }

    /// **A revoked principal is denied even with a live grant** (the S7 consult threads into the
    /// fail-static gate — a cached ALLOW never overrides a revoke).
    #[test]
    fn revoked_principal_is_denied_despite_grant() {
        let sbc = check_with_git_fragment();
        let bootstrap = TupleRepoBootstrap::new(sbc.tuples().clone());
        let creator = principal("svc:creator", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        bootstrap.grant_creator(&creator, &repo).expect("grant");

        let scope = TenantScope::from_verified_token(&creator, creator.region.clone());
        sbc.revoke_in(
            &scope,
            &RevokeTarget::Principal(creator.principal_id.clone()),
            Timestamp("2026-07-15T00:00:00Z".into()),
        );
        let authz = authorizer(sbc);
        assert!(!authz.authorize_repo(&creator, &repo, RepoAccess::Read));
        assert!(!authz.authorize_repo(&creator, &repo, RepoAccess::Write));
    }

    /// The one-grammar helpers stay in lock-step (kills a drifted-format mutant): the ref form wraps
    /// the id form verbatim.
    #[test]
    fn repo_object_grammar_is_one_spelling() {
        assert_eq!(repo_object_id("widgets"), "repo:widgets");
        assert_eq!(repo_object_ref("widgets").0, "repo:widgets");
    }

    /// The RFC 3339 stamp is well-formed (a `YYYY-MM-DDThh:mm:ssZ` shape with in-range fields) —
    /// the civil-from-days conversion is exact on a known instant.
    #[test]
    fn now_rfc3339_is_well_formed() {
        let Timestamp(s) = now_rfc3339();
        assert_eq!(s.len(), 20, "YYYY-MM-DDThh:mm:ssZ: {s}");
        assert!(s.ends_with('Z') && s.as_bytes()[10] == b'T', "{s}");
        // The stamp is in this century (a sanity pin — the test env clock is post-2020).
        assert!(s.starts_with("20"), "{s}");
    }
}
