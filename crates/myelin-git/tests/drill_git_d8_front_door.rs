//! # GIT-D8 — the front-door cross-tenant isolation + residency drill (GIT-P13 / P-274, M3-G2)
//!
//! The FIRST RUNNABLE gate (roadmap §6): clone/push works, **authenticated, tenant-isolated,
//! region-pinned, never loses an event**. This file is the quantified GIT-D8 drill plus the
//! chained SSH-clone→push→check-gate→residency-reject e2e and the CDC pairs for the consumed
//! contract rows the front door rides:
//!
//! - **4.1** `authenticate(Credential) → Principal` — every machine-identity kind (SSH pubkey /
//!   deploy-key / PAT / per-job token) resolves a `Principal`; the **tenant comes from the verified
//!   token, never the URL path** (the GIT-D8 invariant).
//! - **4.2** `check(subject, permission, object, at, caveat?) → Decision` — the per-action
//!   `pull`/`push` fail-closed gate.
//! - **12.2** `placement_of(repo) → RepoGitPlacement` (the REAL storage face — region-pinned,
//!   relocatable). The drill wires the ACTUAL [`myelin_storage::GitPackTier`] as the front door's
//!   [`myelin_git::front_door::PlacementResolver`], so the residency reject is proven against the
//!   real 12.2 surface, not a hand-rolled stub.
//!
//! **THE QUANTIFIED GATE (the green artifact):** a token whose tenant ≠ the URL-path tenant →
//! 0 cross-tenant read (the door streams NOTHING + runs NO check against the foreign repo); a route
//! that would leave the region → 0 out-of-region routes admitted (refused at the door).

use myelin_git::core::{
    Backend, BlameHunk, DiffLine, GitCore, GitCoreError, GitOp, Maintenance, Oid, RepoLoc, Service,
    WireOutput,
};
use myelin_git::front_door::{FrontDoor, FrontDoorError, GitAction, GitRequest, PlacementResolver};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, DataRole, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, PrincipalStatus, Result as IdResult, RevokeTarget, RewriteTrace, RunId,
    RunToken, SubjectTree, TupleDelta, Zookie,
};
use myelin_storage::{
    FsBlobStore, GitPackTier, RepoGitPlacement, RepoId, RepoPlacementStatus, StorageGroup,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use std::cell::RefCell;
use std::collections::HashMap;

// ──────────────────────────────────────────────────────────────────────────────────────────────
//  CDC 4.1 / 4.2 — a real-shaped Identity that resolves machine identities to tenant principals
//  and gates per-action. The TENANT IS RESOLVED FROM THE CREDENTIAL (4.1, ID-3) — the front door
//  is what keys the cross-tenant decision on it.
// ──────────────────────────────────────────────────────────────────────────────────────────────

struct DrillId {
    /// "scheme:material" → (tenant, principal_kind, status) the verified credential resolves to.
    creds: HashMap<String, (String, PrincipalKind, PrincipalStatus)>,
    /// (principal_id, permission) the resolved subject holds (anything else → Deny). Keyed by the
    /// per-SUBJECT principal id (the real ReBAC tuple grain) so a same-id credential in another
    /// tenant grants nothing — and two principals in the SAME tenant can hold different permissions
    /// (a reader vs a writer vs a deploy key).
    grants: HashMap<(String, String), ()>,
    /// every (permission, object) check the door ran — so the drill PROVES 0 cross-tenant check.
    checks: RefCell<Vec<(String, String)>>,
}

impl DrillId {
    fn new() -> Self {
        Self {
            creds: HashMap::new(),
            grants: HashMap::new(),
            checks: RefCell::new(Vec::new()),
        }
    }
    fn cred(mut self, scheme: &str, material: &str, tenant: &str, kind: PrincipalKind) -> Self {
        self.creds.insert(
            format!("{scheme}:{material}"),
            (tenant.to_string(), kind, PrincipalStatus::Active),
        );
        self
    }
    /// Grant `permission` to the subject the credential `material` resolves to (its principal id is
    /// `pid-<material>`). Per-subject — the real ReBAC tuple grain.
    fn grant(mut self, material: &str, permission: &str) -> Self {
        self.grants
            .insert((format!("pid-{material}"), permission.to_string()), ());
        self
    }
}

impl IdentityService for DrillId {
    // 4.1 — resolve the machine identity → Principal; tenant FROM the verified credential (ID-3).
    fn authenticate(&self, c: &Credential) -> IdResult<Principal> {
        match self.creds.get(&format!("{}:{}", c.scheme, c.material)) {
            Some((tenant, kind, status)) => Ok(Principal::new(
                TenantId::from_token(tenant.clone()),
                Region("fr-par".into()),
                PrincipalId(format!("pid-{}", c.material)),
                kind.clone(),
                DataRole::Controller,
                *status,
            )),
            None => Err(AuthzError::FailClosed("unknown machine identity".into())),
        }
    }

    // 4.2 — the per-action fail-closed gate. The object's tenant scoping is `git:repo:<tenant>/...`,
    // so the grant lookup keys on the SUBJECT's tenant (which the door took from the token).
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        self.checks
            .borrow_mut()
            .push((permission.0.clone(), object.0.clone()));
        let pid = subject.principal_id.0.clone();
        if self.grants.contains_key(&(pid, permission.0.clone())) {
            Ok(Decision::Allow)
        } else {
            Ok(Decision::Deny)
        }
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
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _t: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────
//  CDC 12.2 — the REAL storage `GitPackTier::placement_of` wired as the front-door resolver. This
//  proves the residency reject against the ACTUAL contract-12.2 surface (region-pinned placement).
// ──────────────────────────────────────────────────────────────────────────────────────────────

struct TierPlacement {
    tier: GitPackTier<FsBlobStore>,
}

impl PlacementResolver for TierPlacement {
    fn placement_of(&self, repo: &RepoId) -> Option<RepoGitPlacement> {
        // The REAL 12.2 storage call — region-pinned, relocatable placement.
        self.tier.placement_of(repo)
    }
}

fn placed_tier(tenant: &str, placements: &[(&str, &str, RepoPlacementStatus)]) -> TierPlacement {
    let tier = GitPackTier::new(TenantId::from_token(tenant), FsBlobStore::new());
    for (key, region, status) in placements {
        tier.place_repo(
            RepoId::from_token(*key),
            RepoGitPlacement {
                group: StorageGroup::from_token("pack-0"),
                region: Region::new(*region),
                status: *status,
            },
        );
    }
    TierPlacement { tier }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────
//  A recording GitCore — the serving seam. Records every (repo, service) streamed so the drill
//  asserts 0 cross-tenant / 0 out-of-region reads (the door streamed NOTHING when it refused).
// ──────────────────────────────────────────────────────────────────────────────────────────────

struct RecCore {
    served: RefCell<Vec<(RepoLoc, Service)>>,
}
impl RecCore {
    fn new() -> Self {
        Self {
            served: RefCell::new(Vec::new()),
        }
    }
}
impl GitCore for RecCore {
    fn route(&self, op: GitOp) -> Backend {
        myelin_git::core::backend_for(op)
    }
    fn advertise_refs(&self, repo: &RepoLoc, svc: Service) -> Result<WireOutput, GitCoreError> {
        self.served.borrow_mut().push((repo.clone(), svc));
        Ok(WireOutput {
            stdout: b"refs-adv".to_vec(),
            status: 0,
        })
    }
    fn serve(
        &self,
        repo: &RepoLoc,
        svc: Service,
        _stdin: Vec<u8>,
    ) -> Result<WireOutput, GitCoreError> {
        self.served.borrow_mut().push((repo.clone(), svc));
        // a real pack-shaped stream (the byte plumbing the serving tier streams without buffering).
        Ok(WireOutput {
            stdout: b"PACK\x00streamed".to_vec(),
            status: 0,
        })
    }
    fn maintenance(&self, _r: &RepoLoc, _m: Maintenance) -> Result<WireOutput, GitCoreError> {
        unreachable!("the front door never runs maintenance")
    }
    fn read_blob_bounded(
        &self,
        _r: &RepoLoc,
        _o: &Oid,
        _maximum_bytes: usize,
    ) -> Result<Vec<u8>, GitCoreError> {
        unreachable!()
    }
    fn diff_blobs_bounded(
        &self,
        _r: &RepoLoc,
        _a: &Oid,
        _b: &Oid,
        _maximum_blob_bytes: usize,
        _maximum_lines: usize,
        _maximum_output_bytes: usize,
    ) -> Result<Vec<DiffLine>, GitCoreError> {
        unreachable!()
    }
    fn blame(&self, _r: &RepoLoc, _p: &str, _a: &Oid) -> Result<Vec<BlameHunk>, GitCoreError> {
        unreachable!()
    }
}

fn cred(scheme: &str, material: &str) -> Credential {
    Credential {
        scheme: scheme.to_string(),
        material: material.to_string(),
    }
}

fn req(scheme: &str, material: &str, tenant: &str, repo: &str, action: GitAction) -> GitRequest {
    GitRequest {
        credential: cred(scheme, material),
        url_tenant: tenant.to_string(),
        url_repo: repo.to_string(),
        action,
        body: b"0000want".to_vec(),
    }
}

// ══════════════════════════════════════ THE DRILL ══════════════════════════════════════════════

/// **GIT-D8 (the quantified gate): a token whose tenant ≠ the URL-path tenant → tenant from token;
/// 0 cross-tenant read; rejected at the front door.** The attacker holds a valid `acme` token
/// (e.g. a stolen/misused PAT) and addresses `globex/secret` — a real, hosted repo in the same
/// region. The door resolves the tenant from the TOKEN (`acme`), sees it ≠ the URL path (`globex`),
/// and REFUSES at the door — streaming nothing, checking nothing against globex's repo.
#[test]
fn git_d8_cross_tenant_front_door_isolation_zero_reads() {
    let id = DrillId::new()
        .cred("pat", "acme-token", "acme", PrincipalKind::Human)
        .grant("acme-token", "pull"); // the token's subject; the door never even reaches the check.
                                      // globex DOES host the secret repo here (so the only thing standing between the acme token and
                                      // globex's data is the front-door cross-tenant guard — exactly what GIT-D8 measures).
    let placement = placed_tier(
        "globex",
        &[("globex/secret", "fr-par", RepoPlacementStatus::Active)],
    );
    let door = FrontDoor::new(id, placement, RecCore::new(), Region::new("fr-par"));

    let r = req("pat", "acme-token", "globex", "secret", GitAction::Fetch);
    let err = door.authorize(&r).unwrap_err();

    // The decision keyed on the TOKEN tenant (`acme`), never the URL-path tenant (`globex`).
    assert_eq!(
        err,
        FrontDoorError::CrossTenant {
            token_tenant: "acme".into(),
            url_tenant: "globex".into(),
        }
    );
    // THE GREEN ARTIFACT (measured): cross-tenant-read-count == 0.
    let served = serve_count(&door);
    assert_eq!(
        served, 0,
        "GIT-D8: 0 cross-tenant read (door streamed nothing)"
    );
    // Defence in depth: the door never even ran a `check` against globex's repo object.
    assert_eq!(
        checks_count(&door),
        0,
        "GIT-D8: 0 cross-tenant check (never looked up the foreign repo)"
    );
}

/// **The residency reject (ADR-11 / 12.4): a route that would leave the region is REFUSED at the
/// front door — 0 out-of-region routes admitted.** The repo is pinned (in the REAL storage 12.2
/// placement) to `eu-central`; this front-door replica serves `fr-par`. The door reads the pinned
/// region from `placement_of(repo)` and refuses to route the repo out of its region.
#[test]
fn residency_reject_zero_out_of_region_routes() {
    let id = DrillId::new()
        .cred("ssh", "k", "acme", PrincipalKind::Human)
        .grant("k", "pull");
    let placement = placed_tier(
        "acme",
        &[("acme/widgets", "eu-central", RepoPlacementStatus::Active)],
    );
    let door = FrontDoor::new(id, placement, RecCore::new(), Region::new("fr-par"));

    let r = req("ssh", "k", "acme", "widgets", GitAction::Fetch);
    let err = door.authorize(&r).unwrap_err();
    assert_eq!(
        err,
        FrontDoorError::OutOfRegion {
            pinned: "eu-central".into(),
            target: "fr-par".into(),
        }
    );
    assert_eq!(serve_count(&door), 0, "0 out-of-region routes admitted");
}

/// **The chained e2e: SSH clone → push → check gate → residency reject path** — every leg of the
/// FIRST-RUNNABLE pipeline, in order, over the REAL 12.2 placement surface.
#[test]
fn chained_e2e_ssh_clone_push_check_gate_residency() {
    // A reader (pull only) and a writer (pull+push) in `acme`; a repo pinned to fr-par; one foreign
    // repo pinned to eu-central (the residency leg).
    let id = DrillId::new()
        .cred("ssh", "reader", "acme", PrincipalKind::Human)
        .cred("ssh", "writer", "acme", PrincipalKind::Human)
        .cred("deploy_key", "dk", "acme", PrincipalKind::Service)
        .grant("reader", "pull")
        .grant("writer", "pull")
        .grant("writer", "push");
    let placement = placed_tier(
        "acme",
        &[
            ("acme/widgets", "fr-par", RepoPlacementStatus::Active),
            ("acme/elsewhere", "eu-central", RepoPlacementStatus::Active),
        ],
    );
    let door = FrontDoor::new(id, placement, RecCore::new(), Region::new("fr-par"));

    // LEG 1 — SSH clone (fetch): authenticate(ssh pubkey) → check(pull) → placement(fr-par) → stream.
    let clone = door
        .route(&req("ssh", "reader", "acme", "widgets", GitAction::Fetch))
        .expect("clone granted");
    assert!(
        clone.stdout.starts_with(b"PACK"),
        "the clone streamed a pack"
    );

    // LEG 2 — SSH push (writer holds push): authenticate → check(push) → placement → stream.
    let push = door
        .route(&req("ssh", "writer", "acme", "widgets", GitAction::Push))
        .expect("push granted");
    assert!(push.stdout.starts_with(b"PACK"), "the push streamed");

    // LEG 3 — the CHECK GATE bites: a deploy key (a repo-scoped machine principal) with no `push`
    // grant is DENIED the push (fail-closed).
    let denied = door
        .authorize(&req("deploy_key", "dk", "acme", "widgets", GitAction::Push))
        .unwrap_err();
    // The deploy key resolves (4.1) but has no push grant in `acme` → check Deny (4.2).
    assert!(
        matches!(
            denied,
            FrontDoorError::AuthzDenied {
                decision: Decision::Deny,
                ..
            }
        ),
        "the check gate denied the un-granted push: {denied}"
    );

    // LEG 4 — the RESIDENCY REJECT: the same authorised writer fetching the eu-central repo is
    // refused at the door (the repo would leave its region).
    let region_reject = door
        .authorize(&req("ssh", "writer", "acme", "elsewhere", GitAction::Fetch))
        .unwrap_err();
    assert_eq!(
        region_reject,
        FrontDoorError::OutOfRegion {
            pinned: "eu-central".into(),
            target: "fr-par".into(),
        }
    );

    // Exactly the two GRANTED legs streamed (clone + push); the two refused legs streamed nothing.
    assert_eq!(serve_count(&door), 2, "only the 2 granted legs streamed");
}

/// **CDC 4.1 — every machine-identity kind resolves to a tenant `Principal`** (SSH pubkey /
/// deploy-key / PAT / per-job token), and the tenant the door routes under is the TOKEN's.
#[test]
fn cdc_4_1_every_machine_identity_resolves_to_a_tenant_principal() {
    for (scheme, kind) in [
        ("ssh", PrincipalKind::Human),
        ("deploy_key", PrincipalKind::Service),
        ("pat", PrincipalKind::Human),
        (
            "ci",
            PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef("run-1".into()),
                on_behalf_of: None,
            },
        ),
    ] {
        let id = DrillId::new()
            .cred(scheme, "k", "acme", kind.clone())
            .grant("k", "pull");
        let placement = placed_tier(
            "acme",
            &[("acme/widgets", "fr-par", RepoPlacementStatus::Active)],
        );
        let door = FrontDoor::new(id, placement, RecCore::new(), Region::new("fr-par"));
        let route = door
            .authorize(&req(scheme, "k", "acme", "widgets", GitAction::Fetch))
            .expect("granted");
        assert_eq!(route.repo, RepoLoc::new("acme", "fr-par", "widgets"));
        assert_eq!(route.principal.tenant.as_str(), "acme");
    }
}

/// **CDC 4.2 — the per-action gate distinguishes `pull` vs `push`** against the token-tenant repo
/// object, fail-closed when the grant is absent.
#[test]
fn cdc_4_2_per_action_gate_pull_vs_push() {
    // pull-only principal: clone OK, push DENIED.
    let id = DrillId::new()
        .cred("pat", "k", "acme", PrincipalKind::Human)
        .grant("k", "pull"); // no push
    let placement = placed_tier(
        "acme",
        &[("acme/widgets", "fr-par", RepoPlacementStatus::Active)],
    );
    let door = FrontDoor::new(id, placement, RecCore::new(), Region::new("fr-par"));

    door.authorize(&req("pat", "k", "acme", "widgets", GitAction::Fetch))
        .expect("pull allowed");
    let push_err = door
        .authorize(&req("pat", "k", "acme", "widgets", GitAction::Push))
        .unwrap_err();
    assert!(matches!(
        push_err,
        FrontDoorError::AuthzDenied {
            decision: Decision::Deny,
            ..
        }
    ));
}

/// **CDC 12.2 — `placement_of(repo)` over the REAL storage tier drives the route's region.** An
/// unplaced repo fails closed; a placed repo's pinned region is what the residency reject compares.
#[test]
fn cdc_12_2_placement_of_drives_region_pinning() {
    let id = DrillId::new()
        .cred("ssh", "k", "acme", PrincipalKind::Human)
        .grant("k", "pull");
    // only acme/widgets is placed; acme/ghost is not.
    let placement = placed_tier(
        "acme",
        &[("acme/widgets", "fr-par", RepoPlacementStatus::Active)],
    );
    let door = FrontDoor::new(id, placement, RecCore::new(), Region::new("fr-par"));

    // placed → route region == the placement's pinned region.
    let route = door
        .authorize(&req("ssh", "k", "acme", "widgets", GitAction::Fetch))
        .expect("granted");
    assert_eq!(route.repo.region, "fr-par");

    // unplaced → fail-closed (never fabricate a placement).
    let err = door
        .authorize(&req("ssh", "k", "acme", "ghost", GitAction::Fetch))
        .unwrap_err();
    assert_eq!(
        err,
        FrontDoorError::NoPlacement {
            repo: "ghost".into()
        }
    );
}

// ── small introspection helpers on the door's recording backends (the drill assertions read these).
//    Free functions (the orphan rule forbids an inherent impl on the foreign `FrontDoor` type here).
fn serve_count(door: &FrontDoor<DrillId, TierPlacement, RecCore>) -> usize {
    door.core_ref().served.borrow().len()
}
fn checks_count(door: &FrontDoor<DrillId, TierPlacement, RecCore>) -> usize {
    door.id_ref().checks.borrow().len()
}
