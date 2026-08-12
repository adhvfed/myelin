use crate::core::{GitCore, RepoLoc, Service, WireOutput};
use myelin_identity::{
    CaveatContext, Consistency, ConsistencyMode, Credential, Decision, IdentityService, Permission,
    Principal, PrincipalStatus, Zookie,
};
use myelin_storage::gitpack::{RepoGitPlacement, RepoId, RepoPlacementStatus};
use myelin_tenancy::{ArtifactRef, Region};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitAction {
    Fetch,
    Push,
}

impl GitAction {
    pub fn permission(self) -> Permission {
        match self {
            GitAction::Fetch => Permission("pull".to_string()),
            GitAction::Push => Permission("push".to_string()),
        }
    }

    pub fn service(self) -> Service {
        match self {
            GitAction::Fetch => Service::UploadPack,
            GitAction::Push => Service::ReceivePack,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GitRequest {
    pub credential: Credential,
    pub url_tenant: String,
    pub url_repo: String,
    pub action: GitAction,
    pub body: Vec<u8>,
}

pub trait PlacementResolver {
    type Error: std::fmt::Display;

    fn placement_of(&self, repo: &RepoId) -> Result<Option<RepoGitPlacement>, Self::Error>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrontDoorError {
    Unauthenticated {
        scheme: String,
    },
    PrincipalNotActive {
        status: PrincipalStatus,
    },
    CrossTenant {
        token_tenant: String,
        url_tenant: String,
    },
    AuthzDenied {
        permission: Permission,
        decision: Decision,
    },
    NoPlacement {
        repo: String,
    },
    PlacementUnavailable {
        detail: String,
    },
    RepoOffboarding {
        repo: String,
    },
    OutOfRegion {
        pinned: String,
        target: String,
    },
    IdentityUnavailable {
        detail: String,
    },
    Wire {
        detail: String,
    },
}

impl std::fmt::Display for FrontDoorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrontDoorError::Unauthenticated { scheme } => write!(
                f,
                "front door: authenticate REFUSED - credential scheme `{scheme}` did not resolve to \
                 a principal (fail-closed; no anonymous route)"
            ),
            FrontDoorError::PrincipalNotActive { status } => write!(
                f,
                "front door: principal is `{status:?}` (not Active) - fail-closed (ID-D1: a disabled \
                 identity gets zero access)"
            ),
            FrontDoorError::CrossTenant { token_tenant, url_tenant } => write!(
                f,
                "front door: CROSS-TENANT route REFUSED - token tenant `{token_tenant}` ≠ URL-path \
                 tenant `{url_tenant}`; the tenant comes from the TOKEN, never the URL (GIT-D8: 0 \
                 cross-tenant read)"
            ),
            FrontDoorError::AuthzDenied { permission, decision } => write!(
                f,
                "front door: check DENIED - `{}` returned `{decision:?}` (fail-closed; Conditional \
                 is never a silent allow)",
                permission.0
            ),
            FrontDoorError::NoPlacement { repo } => write!(
                f,
                "front door: placement_of(`{repo}`) found no placement - repo not hosted here \
                 (fail-closed; never fabricate a placement)"
            ),
            FrontDoorError::PlacementUnavailable { detail } => write!(
                f,
                "front door: placement dependency unavailable ({detail}) - fail-closed"
            ),
            FrontDoorError::RepoOffboarding { repo } => write!(
                f,
                "front door: repo `{repo}` is offboarding (packs pending crypto-shred) - refused"
            ),
            FrontDoorError::OutOfRegion { pinned, target } => write!(
                f,
                "front door: OUT-OF-REGION route REFUSED - repo pinned to `{pinned}`, route would \
                 serve from `{target}` (ADR-11 residency pin: 0 out-of-region routes admitted)"
            ),
            FrontDoorError::IdentityUnavailable { detail } => write!(
                f,
                "front door: Id dependency unavailable ({detail}) - fail-CLOSED (the bounded-stale \
                 fail-static degrade is GIT-P14)"
            ),
            FrontDoorError::Wire { detail } => {
                write!(f, "front door: serving-tier wire op failed: {detail}")
            }
        }
    }
}

impl std::error::Error for FrontDoorError {}

#[derive(Clone, Debug)]
pub struct GrantedRoute {
    pub principal: Principal,
    pub repo: RepoLoc,
    pub service: Service,
}

pub struct FrontDoor<I: IdentityService, P: PlacementResolver, C: GitCore> {
    id: I,
    placement: P,
    core: C,
    home_region: Region,
}

impl<I: IdentityService, P: PlacementResolver, C: GitCore> FrontDoor<I, P, C> {
    pub fn new(id: I, placement: P, core: C, home_region: Region) -> Self {
        Self {
            id,
            placement,
            core,
            home_region,
        }
    }

    pub fn id_ref(&self) -> &I {
        &self.id
    }

    pub fn placement_ref(&self) -> &P {
        &self.placement
    }

    pub fn core_ref(&self) -> &C {
        &self.core
    }

    pub fn liveness(&self) -> bool {
        true
    }

    pub fn readiness(&self, probe: &Credential, probe_repo: &RepoId) -> bool {
        let id_reachable = self.id.authenticate(probe).is_ok();
        let placement_reachable = self.placement.placement_of(probe_repo).is_ok();
        id_reachable && placement_reachable
    }

    pub fn route(&self, req: &GitRequest) -> Result<WireOutput, FrontDoorError> {
        let route = self.authorize(req)?;
        self.core
            .serve(&route.repo, route.service, req.body.clone())
            .map_err(|e| FrontDoorError::Wire {
                detail: e.to_string(),
            })
    }

    pub fn authorize(&self, req: &GitRequest) -> Result<GrantedRoute, FrontDoorError> {
        let principal = self.id.authenticate(&req.credential).map_err(|e| {
            if is_transport_error(&e) {
                FrontDoorError::IdentityUnavailable {
                    detail: format!("{e:?}"),
                }
            } else {
                FrontDoorError::Unauthenticated {
                    scheme: req.credential.scheme.clone(),
                }
            }
        })?;

        if principal.status != PrincipalStatus::Active {
            return Err(FrontDoorError::PrincipalNotActive {
                status: principal.status,
            });
        }

        let token_tenant = principal.tenant.as_str().to_string();
        if token_tenant != req.url_tenant {
            return Err(FrontDoorError::CrossTenant {
                token_tenant,
                url_tenant: req.url_tenant.clone(),
            });
        }

        let permission = req.action.permission();
        let object = repo_artifact_ref(&token_tenant, &req.url_repo);
        let consistency = Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        };
        let caveat = self.caveat_for(req, &object);
        let decision = self
            .id
            .check(
                &principal,
                &permission,
                &object,
                &consistency,
                caveat.as_ref(),
            )
            .map_err(|e| {
                if is_transport_error(&e) {
                    FrontDoorError::IdentityUnavailable {
                        detail: format!("{e:?}"),
                    }
                } else {
                    FrontDoorError::AuthzDenied {
                        permission: permission.clone(),
                        decision: Decision::Deny,
                    }
                }
            })?;
        if decision != Decision::Allow {
            return Err(FrontDoorError::AuthzDenied {
                permission,
                decision,
            });
        }

        let repo_id = RepoId::from_token(repo_placement_key(&token_tenant, &req.url_repo));
        let placement = self
            .placement
            .placement_of(&repo_id)
            .map_err(|error| FrontDoorError::PlacementUnavailable {
                detail: error.to_string(),
            })?
            .ok_or_else(|| FrontDoorError::NoPlacement {
                repo: req.url_repo.clone(),
            })?;
        if placement.status == RepoPlacementStatus::Offboarding {
            return Err(FrontDoorError::RepoOffboarding {
                repo: req.url_repo.clone(),
            });
        }

        let pinned = placement.region.clone();
        if pinned != self.home_region {
            return Err(FrontDoorError::OutOfRegion {
                pinned: pinned.as_str().to_string(),
                target: self.home_region.as_str().to_string(),
            });
        }

        let repo = RepoLoc::new(
            token_tenant,
            pinned.as_str().to_string(),
            req.url_repo.clone(),
        );
        Ok(GrantedRoute {
            principal,
            repo,
            service: req.action.service(),
        })
    }

    pub fn advertise_refs(&self, req: &GitRequest) -> Result<WireOutput, FrontDoorError> {
        let route = self.authorize(req)?;
        self.core
            .advertise_refs(&route.repo, route.service)
            .map_err(|e| FrontDoorError::Wire {
                detail: e.to_string(),
            })
    }

    fn caveat_for(&self, req: &GitRequest, object: &ArtifactRef) -> Option<CaveatContext> {
        use myelin_identity::Literal;
        let mut attrs = std::collections::BTreeMap::new();
        attrs.insert(
            "action".to_string(),
            Literal::Str(match req.action {
                GitAction::Fetch => "fetch".to_string(),
                GitAction::Push => "push".to_string(),
            }),
        );
        Some(CaveatContext {
            object: object.clone(),
            field: None,
            transition: None,
            attrs,
        })
    }
}

fn repo_artifact_ref(tenant: &str, repo: &str) -> ArtifactRef {
    ArtifactRef(format!("git:repo:{tenant}/{repo}"))
}

fn repo_placement_key(tenant: &str, repo: &str) -> String {
    format!("{tenant}/{repo}")
}

fn is_transport_error(e: &myelin_identity::AuthzError) -> bool {
    matches!(
        e,
        myelin_identity::AuthzError::NotYetImplemented(_)
            | myelin_identity::AuthzError::Unavailable(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Backend, GitCoreError, GitOp};
    use myelin_identity::{
        AuthzError, DataRole, ListObjectsResult, Literal, ObjectId, ObjectType, PrincipalId,
        PrincipalKind, Result as IdResult, RewriteTrace, SubjectTree, TupleDelta,
    };
    use myelin_tenancy::TenantId;
    use std::cell::RefCell;

    struct StubId {
        principals: std::collections::HashMap<String, (String, PrincipalStatus)>,
        allow: Vec<String>,
        authn_unavailable: bool,
        checks_seen: RefCell<Vec<(String, String)>>,
        last_caveat_action: RefCell<Option<String>>,
    }

    impl StubId {
        fn new() -> Self {
            Self {
                principals: std::collections::HashMap::new(),
                allow: vec!["pull".into(), "push".into()],
                authn_unavailable: false,
                checks_seen: RefCell::new(Vec::new()),
                last_caveat_action: RefCell::new(None),
            }
        }
        fn with_principal(mut self, key: &str, tenant: &str, status: PrincipalStatus) -> Self {
            self.principals
                .insert(key.to_string(), (tenant.to_string(), status));
            self
        }
        fn allowing(mut self, perms: &[&str]) -> Self {
            self.allow = perms.iter().map(|s| s.to_string()).collect();
            self
        }
    }

    impl IdentityService for StubId {
        fn authenticate(&self, c: &Credential) -> IdResult<Principal> {
            if self.authn_unavailable {
                return Err(AuthzError::NotYetImplemented("Id floor not wired"));
            }
            let key = format!("{}:{}", c.scheme, c.material);
            match self.principals.get(&key) {
                Some((tenant, status)) => Ok(Principal::new(
                    TenantId::from_token(tenant.clone()),
                    Region("fr-par".into()),
                    PrincipalId(format!("pid-{}", c.material)),
                    PrincipalKind::Human,
                    DataRole::Controller,
                    *status,
                )),
                None => Err(AuthzError::FailClosed("unknown credential".into())),
            }
        }
        fn check(
            &self,
            _s: &Principal,
            permission: &Permission,
            object: &ArtifactRef,
            _at: &Consistency,
            cav: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            self.checks_seen
                .borrow_mut()
                .push((permission.0.clone(), object.0.clone()));
            *self.last_caveat_action.borrow_mut() = cav.and_then(|c| {
                c.attrs.get("action").and_then(|l| match l {
                    Literal::Str(s) => Some(s.clone()),
                    _ => None,
                })
            });
            if self.allow.contains(&permission.0) {
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
        fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicyT> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn write_tuples(
            &self,
            _d: &[TupleDelta],
            _p: Option<&myelin_identity::Precondition>,
        ) -> IdResult<Zookie> {
            Err(AuthzError::NotYetImplemented("n/a"))
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
    use myelin_identity::EffectivePolicy as EffectivePolicyT;

    struct StubPlacement {
        placements: std::collections::HashMap<String, (String, RepoPlacementStatus)>,
    }
    impl StubPlacement {
        fn new() -> Self {
            Self {
                placements: std::collections::HashMap::new(),
            }
        }
        fn with(mut self, key: &str, region: &str, status: RepoPlacementStatus) -> Self {
            self.placements
                .insert(key.to_string(), (region.to_string(), status));
            self
        }
    }
    impl PlacementResolver for StubPlacement {
        type Error = std::convert::Infallible;

        fn placement_of(&self, repo: &RepoId) -> Result<Option<RepoGitPlacement>, Self::Error> {
            Ok(self
                .placements
                .get(repo.as_str())
                .map(|(region, status)| RepoGitPlacement {
                    group: myelin_storage::gitpack::StorageGroup::from_token("g1"),
                    region: Region(region.clone()),
                    status: *status,
                }))
        }
    }

    struct UnavailablePlacement;

    impl PlacementResolver for UnavailablePlacement {
        type Error = &'static str;

        fn placement_of(&self, _repo: &RepoId) -> Result<Option<RepoGitPlacement>, Self::Error> {
            Err("placement state is unavailable")
        }
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
            crate::core::backend_for(op)
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
                stdout: b"PACK".to_vec(),
                status: 0,
            })
        }
        fn maintenance(
            &self,
            _r: &RepoLoc,
            _m: crate::core::Maintenance,
        ) -> Result<WireOutput, GitCoreError> {
            unreachable!("front door never runs maintenance")
        }
        fn read_blob_bounded(
            &self,
            _r: &RepoLoc,
            _o: &crate::core::Oid,
            _maximum_bytes: usize,
        ) -> Result<Vec<u8>, GitCoreError> {
            unreachable!()
        }
        fn diff_blobs_bounded(
            &self,
            _r: &RepoLoc,
            _a: &crate::core::Oid,
            _b: &crate::core::Oid,
            _maximum_blob_bytes: usize,
            _maximum_lines: usize,
            _maximum_output_bytes: usize,
        ) -> Result<Vec<crate::core::DiffLine>, GitCoreError> {
            unreachable!()
        }
        fn blame_bounded(
            &self,
            _r: &RepoLoc,
            _p: &str,
            _a: &crate::core::Oid,
            _maximum_path_bytes: usize,
            _maximum_blob_bytes: usize,
            _maximum_hunks: usize,
        ) -> Result<Vec<crate::core::BlameHunk>, GitCoreError> {
            unreachable!()
        }
    }

    fn cred(scheme: &str, material: &str) -> Credential {
        Credential {
            scheme: scheme.to_string(),
            material: material.to_string(),
        }
    }

    #[test]
    fn git_request_debug_cannot_bypass_credential_redaction() {
        let request = GitRequest {
            credential: cred("pat", "secret-bearer"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: Vec::new(),
        };
        let rendered = format!("{request:?}");
        assert!(!rendered.contains("secret-bearer"));
        assert!(rendered.contains("<redacted>"));
    }

    fn door(
        id: StubId,
        placement: StubPlacement,
        core: RecCore,
        home: &str,
    ) -> FrontDoor<StubId, StubPlacement, RecCore> {
        FrontDoor::new(id, placement, core, Region(home.into()))
    }

    #[test]
    fn fetch_happy_path_authenticates_checks_places_and_streams() {
        for scheme in ["ssh", "deploy_key", "pat", "ci"] {
            let id = StubId::new().with_principal(
                &format!("{scheme}:k1"),
                "acme",
                PrincipalStatus::Active,
            );
            let placement =
                StubPlacement::new().with("acme/widgets", "fr-par", RepoPlacementStatus::Active);
            let core = RecCore::new();
            let d = door(id, placement, core, "fr-par");
            let req = GitRequest {
                credential: cred(scheme, "k1"),
                url_tenant: "acme".into(),
                url_repo: "widgets".into(),
                action: GitAction::Fetch,
                body: b"0000".to_vec(),
            };
            let out = d.route(&req).expect("granted");
            assert_eq!(out.stdout, b"PACK");
            let served = d.core.served.borrow();
            assert_eq!(served.len(), 1);
            assert_eq!(served[0].0, RepoLoc::new("acme", "fr-par", "widgets"));
            assert_eq!(served[0].1, Service::UploadPack);
        }
    }

    #[test]
    fn git_d8_cross_tenant_token_is_refused_at_the_door_zero_reads() {
        let id = StubId::new().with_principal("pat:stolen", "acme", PrincipalStatus::Active);
        let placement =
            StubPlacement::new().with("globex/secret", "fr-par", RepoPlacementStatus::Active);
        let core = RecCore::new();
        let d = door(id, placement, core, "fr-par");
        let req = GitRequest {
            credential: cred("pat", "stolen"),
            url_tenant: "globex".into(),
            url_repo: "secret".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        let err = d.authorize(&req).unwrap_err();
        assert_eq!(
            err,
            FrontDoorError::CrossTenant {
                token_tenant: "acme".into(),
                url_tenant: "globex".into(),
            }
        );
        assert_eq!(d.core.served.borrow().len(), 0, "0 cross-tenant read");
        assert!(d.id.checks_seen.borrow().is_empty());
    }

    #[test]
    fn out_of_region_route_is_refused_at_the_door() {
        let id = StubId::new().with_principal("ssh:k", "acme", PrincipalStatus::Active);
        let placement =
            StubPlacement::new().with("acme/widgets", "eu-central", RepoPlacementStatus::Active);
        let core = RecCore::new();
        let d = door(id, placement, core, "fr-par");
        let req = GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        let err = d.authorize(&req).unwrap_err();
        assert_eq!(
            err,
            FrontDoorError::OutOfRegion {
                pinned: "eu-central".into(),
                target: "fr-par".into(),
            }
        );
        assert_eq!(
            d.core.served.borrow().len(),
            0,
            "0 out-of-region routes admitted"
        );
    }

    #[test]
    fn push_without_push_permission_is_denied() {
        let id = StubId::new()
            .with_principal("ssh:k", "acme", PrincipalStatus::Active)
            .allowing(&["pull"]);
        let placement =
            StubPlacement::new().with("acme/widgets", "fr-par", RepoPlacementStatus::Active);
        let d = door(id, placement, RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Push,
            body: vec![],
        };
        let err = d.authorize(&req).unwrap_err();
        assert!(matches!(
            err,
            FrontDoorError::AuthzDenied {
                decision: Decision::Deny,
                ..
            }
        ));
        assert_eq!(d.core.served.borrow().len(), 0);
    }

    #[test]
    fn unknown_credential_is_unauthenticated() {
        let id = StubId::new();
        let placement = StubPlacement::new();
        let d = door(id, placement, RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("ssh", "nope"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        assert_eq!(
            d.authorize(&req).unwrap_err(),
            FrontDoorError::Unauthenticated {
                scheme: "ssh".into()
            }
        );
    }

    #[test]
    fn disabled_principal_is_refused() {
        let id = StubId::new().with_principal("pat:old", "acme", PrincipalStatus::Disabled);
        let placement =
            StubPlacement::new().with("acme/widgets", "fr-par", RepoPlacementStatus::Active);
        let d = door(id, placement, RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("pat", "old"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        assert_eq!(
            d.authorize(&req).unwrap_err(),
            FrontDoorError::PrincipalNotActive {
                status: PrincipalStatus::Disabled
            }
        );
    }

    #[test]
    fn unplaced_repo_is_refused() {
        let id = StubId::new().with_principal("ssh:k", "acme", PrincipalStatus::Active);
        let placement = StubPlacement::new();
        let d = door(id, placement, RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        assert_eq!(
            d.authorize(&req).unwrap_err(),
            FrontDoorError::NoPlacement {
                repo: "widgets".into()
            }
        );
    }

    #[test]
    fn offboarding_repo_is_refused() {
        let id = StubId::new().with_principal("ssh:k", "acme", PrincipalStatus::Active);
        let placement =
            StubPlacement::new().with("acme/widgets", "fr-par", RepoPlacementStatus::Offboarding);
        let d = door(id, placement, RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        assert_eq!(
            d.authorize(&req).unwrap_err(),
            FrontDoorError::RepoOffboarding {
                repo: "widgets".into()
            }
        );
    }

    #[test]
    fn check_object_is_scoped_to_the_token_tenant() {
        let id = StubId::new().with_principal("ssh:k", "acme", PrincipalStatus::Active);
        let placement =
            StubPlacement::new().with("acme/widgets", "fr-par", RepoPlacementStatus::Active);
        let d = door(id, placement, RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Push,
            body: vec![],
        };
        d.authorize(&req).expect("granted");
        let seen = d.id.checks_seen.borrow();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "push");
        assert_eq!(seen[0].1, "git:repo:acme/widgets");
    }

    #[test]
    fn check_carries_the_caveat_action_rider() {
        let id = StubId::new().with_principal("ssh:k", "acme", PrincipalStatus::Active);
        let placement =
            StubPlacement::new().with("acme/widgets", "fr-par", RepoPlacementStatus::Active);
        let d = door(id, placement, RecCore::new(), "fr-par");
        d.authorize(&GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Push,
            body: vec![],
        })
        .expect("granted");
        assert_eq!(
            d.id.last_caveat_action.borrow().as_deref(),
            Some("push"),
            "the door supplies the §8.6 action rider to check"
        );
    }

    #[test]
    fn liveness_is_always_up_and_readiness_gates_on_dependencies() {
        let id = StubId::new().with_principal("probe:p", "sys", PrincipalStatus::Active);
        let placement = StubPlacement::new();
        let d = door(id, placement, RecCore::new(), "fr-par");
        assert!(d.liveness(), "liveness never checks a backend");
        assert!(
            d.readiness(&cred("probe", "p"), &RepoId::from_token("sys/_probe")),
            "Id reachable → ready"
        );

        let mut unavailable_id = StubId::new();
        unavailable_id.authn_unavailable = true;
        let d2 = door(
            unavailable_id,
            StubPlacement::new(),
            RecCore::new(),
            "fr-par",
        );
        assert!(d2.liveness(), "liveness stays up even when Id is down");
        assert!(
            !d2.readiness(&cred("probe", "p"), &RepoId::from_token("sys/_probe")),
            "Id unreachable → not ready"
        );

        let id = StubId::new().with_principal("probe:p", "sys", PrincipalStatus::Active);
        let d3 = FrontDoor::new(
            id,
            UnavailablePlacement,
            RecCore::new(),
            Region("fr-par".into()),
        );
        assert!(
            !d3.readiness(&cred("probe", "p"), &RepoId::from_token("sys/_probe")),
            "placement state unavailable → not ready"
        );
    }

    #[test]
    fn placement_failure_is_not_reported_as_an_absent_repository() {
        let id = StubId::new().with_principal("ssh:k", "acme", PrincipalStatus::Active);
        let door = FrontDoor::new(
            id,
            UnavailablePlacement,
            RecCore::new(),
            Region("fr-par".into()),
        );
        let request = GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: Vec::new(),
        };

        assert_eq!(
            door.authorize(&request).unwrap_err(),
            FrontDoorError::PlacementUnavailable {
                detail: "placement state is unavailable".into(),
            }
        );
    }

    #[test]
    fn id_transport_failure_fails_closed_as_unavailable() {
        let mut id = StubId::new();
        id.authn_unavailable = true;
        let d = door(id, StubPlacement::new(), RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        assert!(matches!(
            d.authorize(&req).unwrap_err(),
            FrontDoorError::IdentityUnavailable { .. }
        ));
        assert_eq!(d.core.served.borrow().len(), 0);
    }

    #[test]
    fn advertise_refs_runs_the_same_cross_tenant_gate() {
        let id = StubId::new().with_principal("pat:s", "acme", PrincipalStatus::Active);
        let placement =
            StubPlacement::new().with("globex/secret", "fr-par", RepoPlacementStatus::Active);
        let d = door(id, placement, RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("pat", "s"),
            url_tenant: "globex".into(),
            url_repo: "secret".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        assert!(matches!(
            d.advertise_refs(&req).unwrap_err(),
            FrontDoorError::CrossTenant { .. }
        ));
        assert_eq!(
            d.core.served.borrow().len(),
            0,
            "no ref adv to a foreign tenant"
        );
    }

    #[test]
    fn action_maps_to_permission_and_service() {
        assert_eq!(GitAction::Fetch.permission(), Permission("pull".into()));
        assert_eq!(GitAction::Push.permission(), Permission("push".into()));
        assert_eq!(GitAction::Fetch.service(), Service::UploadPack);
        assert_eq!(GitAction::Push.service(), Service::ReceivePack);
    }

    #[test]
    fn error_display_is_distinct_and_nonempty() {
        let xtenant = FrontDoorError::CrossTenant {
            token_tenant: "a".into(),
            url_tenant: "b".into(),
        };
        let region = FrontDoorError::OutOfRegion {
            pinned: "fr-par".into(),
            target: "eu-central".into(),
        };
        let s1 = xtenant.to_string();
        let s2 = region.to_string();
        assert!(s1.contains("CROSS-TENANT") && s1.contains("GIT-D8"));
        assert!(s2.contains("OUT-OF-REGION") && s2.contains("ADR-11"));
        assert_ne!(s1, s2);
    }
}
