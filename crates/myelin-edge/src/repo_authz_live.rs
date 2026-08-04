use crate::repo_authz::{RepoAuthorizer, RepoPermission};
use myelin_events::Timestamp;
use myelin_git::core::RepoLoc;
use myelin_git::live_check::{bounded_stale_at, is_allow, perm, strong_at, GitCheckGate};
use myelin_identity::{
    IdentityService, ListObjectsResult, ObjectId, ObjectType, Permission, Principal, RelName,
    RelationTuple, RevokeTarget, TupleDelta, Zookie,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_substrate::{FailStaticError, FailStaticThreshold, Seconds, SystemClock};
use myelin_tenancy::ArtifactRef;
use std::collections::BTreeSet;

pub const REPO_ADMIN_RELATION: &str = "admin";

pub fn repo_object_id(slug: &str) -> String {
    format!("repo:{slug}")
}

pub fn repo_object_ref(slug: &str) -> ArtifactRef {
    ArtifactRef(repo_object_id(slug))
}

pub struct CheckBackedRepoAuthorizer {
    gate: GitCheckGate<StoreBackedCheck, SystemClock>,
}

impl CheckBackedRepoAuthorizer {
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

impl CheckBackedRepoAuthorizer {
    fn subject_revoked(&self, principal: &Principal) -> bool {
        let scope = TenantScope::from_verified_token(principal, principal.region.clone());
        self.gate.id_ref().revocations().is_revoked(
            &scope,
            &RevokeTarget::Principal(principal.principal_id.clone()),
            &Timestamp(String::new()),
        )
    }
}

impl RepoAuthorizer for CheckBackedRepoAuthorizer {
    fn authorize_repo_permission(
        &self,
        principal: &Principal,
        repo: &RepoLoc,
        permission: RepoPermission,
    ) -> bool {
        if principal.tenant.0 != repo.tenant {
            return false;
        }
        let object = repo_object_ref(&repo.repo);
        let revoked = self.subject_revoked(principal);
        let decision = match permission {
            RepoPermission::Pull | RepoPermission::Push => {
                let name = match permission {
                    RepoPermission::Pull => perm::PULL,
                    _ => perm::PUSH,
                };
                self.gate.front_door_check(
                    principal,
                    &Permission(name.to_string()),
                    &object,
                    Zookie(String::new()),
                    revoked,
                )
            }
            RepoPermission::ProtectedPush => self.gate.check_failstatic(
                principal,
                &Permission(perm::PROTECTED_PUSH.to_string()),
                &object,
                &strong_at(Zookie(String::new())),
                revoked,
            ),
            RepoPermission::ApproveUntrustedCi => self.gate.fork_endorsement_check(
                principal,
                &object,
                Zookie(String::new()),
                revoked,
            ),
        };
        is_allow(&decision)
    }

    fn visible_repos(
        &self,
        principal: &Principal,
        tenant: &str,
        region: &str,
        candidates: &[String],
    ) -> Vec<String> {
        if principal.tenant.0 != tenant {
            return Vec::new();
        }
        let ids: BTreeSet<String> = match self.gate.id_ref().list_objects(
            principal,
            &Permission(perm::PULL.to_string()),
            &ObjectType("repo".to_string()),
            &bounded_stale_at(Zookie(String::new())),
        ) {
            Ok(ListObjectsResult::Ids { ids, .. }) => ids.into_iter().map(|o| o.0).collect(),
            _ => BTreeSet::new(),
        };
        candidates
            .iter()
            .filter(|slug| {
                ids.contains(&repo_object_id(slug))
                    || self.authorize_repo_permission(
                        principal,
                        &RepoLoc::new(tenant, region, slug.as_str()),
                        RepoPermission::Pull,
                    )
            })
            .cloned()
            .collect()
    }
}

pub trait RepoBootstrapGrants: Send + Sync {
    fn grant_creator(&self, creator: &Principal, repo: &RepoLoc) -> Result<(), String>;

    fn revoke_creator(&self, creator: &Principal, repo: &RepoLoc) -> Result<(), String>;
}

pub struct NoRepoBootstrap;

impl RepoBootstrapGrants for NoRepoBootstrap {
    fn grant_creator(&self, _creator: &Principal, _repo: &RepoLoc) -> Result<(), String> {
        Ok(())
    }
    fn revoke_creator(&self, _creator: &Principal, _repo: &RepoLoc) -> Result<(), String> {
        Ok(())
    }
}

pub struct TupleRepoBootstrap {
    tuples: TupleStore,
}

impl TupleRepoBootstrap {
    pub fn new(tuples: TupleStore) -> TupleRepoBootstrap {
        TupleRepoBootstrap { tuples }
    }

    fn admin_tuple(creator: &Principal, repo: &RepoLoc) -> RelationTuple {
        RelationTuple {
            object: ObjectId(repo_object_id(&repo.repo)),
            relation: RelName(REPO_ADMIN_RELATION.to_string()),
            subject: creator.principal_id.clone(),
            caveat: None,
        }
    }

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
        Self::tenant_pin(creator, repo)?;
        let delta = TupleDelta::Remove(Self::admin_tuple(creator, repo));
        let scope = TenantScope::from_verified_token(creator, creator.region.clone());
        self.tuples
            .write_tuples(&scope, creator, &[delta], None, None, now_rfc3339())
            .map(|_zookie| ())
            .map_err(|e| e.to_string())
    }
}

fn now_rfc3339() -> Timestamp {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
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
    use crate::repo_authz::RepoAccess;
    use myelin_events::OutboxStore;
    use myelin_identity::{DataRole, FragmentAdmit, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_substrate::FailStaticThreshold;
    use myelin_tenancy::{Region, TenantId};

    fn threshold() -> FailStaticThreshold {
        FailStaticThreshold {
            status: "OPEN - LEGAL".into(),
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

    #[test]
    fn ungranted_principal_is_denied_read_and_write() {
        let authz = authorizer(check_with_git_fragment());
        let mallory = principal("svc:mallory", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        assert!(!authz.authorize_repo(&mallory, &repo, RepoAccess::Read));
        assert!(!authz.authorize_repo(&mallory, &repo, RepoAccess::Write));
    }

    #[test]
    fn bootstrap_grant_then_authorizer_admits_creator() {
        let sbc = check_with_git_fragment();
        let bootstrap = TupleRepoBootstrap::new(sbc.tuples().clone());
        let authz = authorizer(sbc);
        let creator = principal("svc:creator", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");

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

    #[test]
    fn revoke_creator_on_never_granted_is_a_noop_ok() {
        let sbc = check_with_git_fragment();
        let bootstrap = TupleRepoBootstrap::new(sbc.tuples().clone());
        let creator = principal("svc:creator", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        bootstrap
            .revoke_creator(&creator, &repo)
            .expect("remove of an absent tuple is a no-op Ok");
    }

    #[test]
    fn revoke_creator_refuses_a_foreign_tenant() {
        let sbc = check_with_git_fragment();
        let bootstrap = TupleRepoBootstrap::new(sbc.tuples().clone());
        let creator = principal("svc:creator", "acme");
        let foreign = RepoLoc::new("globex", "eu-west", "widgets");
        assert!(bootstrap.revoke_creator(&creator, &foreign).is_err());
    }

    #[test]
    fn namespaced_slug_bootstrap_grant_admits_creator() {
        let sbc = check_with_git_fragment();
        let bootstrap = TupleRepoBootstrap::new(sbc.tuples().clone());
        let authz = authorizer(sbc);
        let creator = principal("svc:creator", "acme");
        let repo = RepoLoc::new("acme", "eu-west", "team/app");

        assert!(
            !authz.authorize_repo(&creator, &repo, RepoAccess::Read),
            "pre-grant: denied (the admit below is the grant's doing)"
        );
        bootstrap.grant_creator(&creator, &repo).expect("bootstrap grant on team/app");
        assert!(
            authz.authorize_repo(&creator, &repo, RepoAccess::Read),
            "the namespaced-slug bootstrap grant admits its creator (pull)"
        );
        assert!(
            authz.authorize_repo(&creator, &repo, RepoAccess::Write),
            "the namespaced-slug bootstrap grant admits its creator (push)"
        );
        let alias = RepoLoc::new("acme", "eu-west", "app");
        assert!(!authz.authorize_repo(&creator, &alias, RepoAccess::Read));
    }

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

    fn write_relation(sbc: &StoreBackedCheck, p: &Principal, relation: &str, slug: &str) {
        let scope = TenantScope::from_verified_token(p, p.region.clone());
        sbc.tuples()
            .write_tuples(
                &scope,
                p,
                &[TupleDelta::Add(RelationTuple {
                    object: ObjectId(repo_object_id(slug)),
                    relation: RelName(relation.into()),
                    subject: p.principal_id.clone(),
                    caveat: None,
                })],
                None,
                None,
                now_rfc3339(),
            )
            .expect("write relation tuple");
    }

    #[test]
    fn writer_admits_push_but_not_protected_push_or_endorse() {
        let sbc = check_with_git_fragment();
        let dev = principal("svc:dev", "acme");
        write_relation(&sbc, &dev, "writer", "widgets");
        let authz = authorizer(sbc);
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        assert!(authz.authorize_repo_permission(&dev, &repo, RepoPermission::Pull));
        assert!(authz.authorize_repo_permission(&dev, &repo, RepoPermission::Push));
        assert!(
            !authz.authorize_repo_permission(&dev, &repo, RepoPermission::ProtectedPush),
            "a push-only writer must NOT clear the merge / branch-protection gate"
        );
        assert!(
            !authz.authorize_repo_permission(&dev, &repo, RepoPermission::ApproveUntrustedCi),
            "a push-only writer must NOT endorse untrusted fork CI"
        );
    }

    #[test]
    fn admin_admits_protected_push_endorser_admits_only_endorse() {
        let sbc = check_with_git_fragment();
        let boss = principal("svc:boss", "acme");
        let bot = principal("svc:bot", "acme");
        write_relation(&sbc, &boss, REPO_ADMIN_RELATION, "widgets");
        write_relation(&sbc, &bot, "approve_untrusted_ci", "widgets");
        let authz = authorizer(sbc);
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        assert!(authz.authorize_repo_permission(&boss, &repo, RepoPermission::ProtectedPush));
        assert!(
            !authz.authorize_repo_permission(&boss, &repo, RepoPermission::ApproveUntrustedCi),
            "admin does not imply the endorsement relation (frozen fragment)"
        );
        assert!(authz.authorize_repo_permission(&bot, &repo, RepoPermission::ApproveUntrustedCi));
        assert!(
            !authz.authorize_repo_permission(&bot, &repo, RepoPermission::Pull),
            "the endorsement relation alone confers no read"
        );
        assert!(!authz.authorize_repo_permission(&bot, &repo, RepoPermission::ProtectedPush));
    }

    #[test]
    fn visible_repos_filters_to_the_granted_set() {
        let sbc = check_with_git_fragment();
        let bootstrap = TupleRepoBootstrap::new(sbc.tuples().clone());
        let creator = principal("svc:creator", "acme");
        bootstrap
            .grant_creator(&creator, &RepoLoc::new("acme", "eu-west", "alpha"))
            .expect("grant on alpha");
        let authz = authorizer(sbc);
        let candidates = vec!["alpha".to_string(), "beta".to_string()];
        assert_eq!(
            authz.visible_repos(&creator, "acme", "eu-west", &candidates),
            vec!["alpha".to_string()],
            "only the granted repo is visible; `beta`'s existence is not leaked"
        );
        let mallory = principal("svc:mallory", "acme");
        assert!(
            authz
                .visible_repos(&mallory, "acme", "eu-west", &candidates)
                .is_empty(),
            "an un-granted principal lists NOTHING"
        );
        assert!(authz
            .visible_repos(&creator, "globex", "eu-west", &candidates)
            .is_empty());
    }

    #[test]
    fn visible_repos_ids_fast_path_over_a_fed_index() {
        use myelin_events::{BusTransport, EventHandler as _, InProcessBus, Relay};
        let outbox = OutboxStore::new();
        let tuples = TupleStore::new(outbox.clone());
        let index = myelin_identity_service::ReverseIndex::new();
        let consumer = myelin_identity_service::ReverseIndexConsumer::new(index.clone());
        let sbc = StoreBackedCheck::with_index(tuples, index);
        for admit in sbc.admit_git_fragment() {
            assert!(matches!(admit, FragmentAdmit::Admitted { .. }));
        }
        let creator = principal("svc:creator", "acme");
        TupleRepoBootstrap::new(sbc.tuples().clone())
            .grant_creator(&creator, &RepoLoc::new("acme", "eu-west", "alpha"))
            .expect("grant on alpha");
        let bus = InProcessBus::new();
        let relay = Relay::new(outbox, bus.clone(), || Timestamp("t".into()));
        relay.drain_to_empty();
        for env in bus.consume("") {
            let _ = consumer.handle(&env, &mut myelin_events::HandlerTx::none());
        }
        let authz = authorizer(sbc);
        let candidates = vec!["alpha".to_string(), "beta".to_string()];
        assert_eq!(
            authz.visible_repos(&creator, "acme", "eu-west", &candidates),
            vec!["alpha".to_string()],
            "the fed-index Ids path admits the granted repo and still hides `beta`"
        );
    }

    #[test]
    fn repo_object_grammar_is_one_spelling() {
        assert_eq!(repo_object_id("widgets"), "repo:widgets");
        assert_eq!(repo_object_ref("widgets").0, "repo:widgets");
    }

    #[test]
    fn now_rfc3339_is_well_formed() {
        let Timestamp(s) = now_rfc3339();
        assert_eq!(s.len(), 20, "YYYY-MM-DDThh:mm:ssZ: {s}");
        assert!(s.ends_with('Z') && s.as_bytes()[10] == b'T', "{s}");
        assert!(s.starts_with("20"), "{s}");
    }
}
