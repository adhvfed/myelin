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

struct DrillId {
    creds: HashMap<String, (String, PrincipalKind, PrincipalStatus)>,
    grants: HashMap<(String, String), ()>,
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
    fn grant(mut self, material: &str, permission: &str) -> Self {
        self.grants
            .insert((format!("pid-{material}"), permission.to_string()), ());
        self
    }
}

impl IdentityService for DrillId {
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

struct TierPlacement {
    tier: GitPackTier<FsBlobStore>,
}

impl PlacementResolver for TierPlacement {
    fn placement_of(&self, repo: &RepoId) -> Option<RepoGitPlacement> {
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
    fn blame_bounded(
        &self,
        _r: &RepoLoc,
        _p: &str,
        _a: &Oid,
        _maximum_path_bytes: usize,
        _maximum_blob_bytes: usize,
        _maximum_hunks: usize,
    ) -> Result<Vec<BlameHunk>, GitCoreError> {
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

#[test]
fn git_d8_cross_tenant_front_door_isolation_zero_reads() {
    let id = DrillId::new()
        .cred("pat", "acme-token", "acme", PrincipalKind::Human)
        .grant("acme-token", "pull");
    let placement = placed_tier(
        "globex",
        &[("globex/secret", "fr-par", RepoPlacementStatus::Active)],
    );
    let door = FrontDoor::new(id, placement, RecCore::new(), Region::new("fr-par"));

    let r = req("pat", "acme-token", "globex", "secret", GitAction::Fetch);
    let err = door.authorize(&r).unwrap_err();

    assert_eq!(
        err,
        FrontDoorError::CrossTenant {
            token_tenant: "acme".into(),
            url_tenant: "globex".into(),
        }
    );
    let served = serve_count(&door);
    assert_eq!(
        served, 0,
        "GIT-D8: 0 cross-tenant read (door streamed nothing)"
    );
    assert_eq!(
        checks_count(&door),
        0,
        "GIT-D8: 0 cross-tenant check (never looked up the foreign repo)"
    );
}

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

#[test]
fn chained_e2e_ssh_clone_push_check_gate_residency() {
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

    let clone = door
        .route(&req("ssh", "reader", "acme", "widgets", GitAction::Fetch))
        .expect("clone granted");
    assert!(
        clone.stdout.starts_with(b"PACK"),
        "the clone streamed a pack"
    );

    let push = door
        .route(&req("ssh", "writer", "acme", "widgets", GitAction::Push))
        .expect("push granted");
    assert!(push.stdout.starts_with(b"PACK"), "the push streamed");

    let denied = door
        .authorize(&req("deploy_key", "dk", "acme", "widgets", GitAction::Push))
        .unwrap_err();
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

    assert_eq!(serve_count(&door), 2, "only the 2 granted legs streamed");
}

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

#[test]
fn cdc_4_2_per_action_gate_pull_vs_push() {
    let id = DrillId::new()
        .cred("pat", "k", "acme", PrincipalKind::Human)
        .grant("k", "pull");
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

#[test]
fn cdc_12_2_placement_of_drives_region_pinning() {
    let id = DrillId::new()
        .cred("ssh", "k", "acme", PrincipalKind::Human)
        .grant("k", "pull");
    let placement = placed_tier(
        "acme",
        &[("acme/widgets", "fr-par", RepoPlacementStatus::Active)],
    );
    let door = FrontDoor::new(id, placement, RecCore::new(), Region::new("fr-par"));

    let route = door
        .authorize(&req("ssh", "k", "acme", "widgets", GitAction::Fetch))
        .expect("granted");
    assert_eq!(route.repo.region, "fr-par");

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

fn serve_count(door: &FrontDoor<DrillId, TierPlacement, RecCore>) -> usize {
    door.core_ref().served.borrow().len()
}
fn checks_count(door: &FrontDoor<DrillId, TierPlacement, RecCore>) -> usize {
    door.id_ref().checks.borrow().len()
}
