use myelin_identity::{IdentityService, Principal, PrincipalId, PrincipalKind, PseudonymHandle};
use myelin_tenancy::TenantId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueActorKind {
    Human,
    Agent,
    Service,
    Unknown,
}

impl IssueActorKind {
    pub fn from_principal(principal: &Principal) -> Self {
        match principal.kind {
            PrincipalKind::Human => Self::Human,
            PrincipalKind::Agent { .. } => Self::Agent,
            PrincipalKind::Service => Self::Service,
        }
    }

    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "human" => Some(Self::Human),
            "agent" => Some(Self::Agent),
            "service" => Some(Self::Service),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
            Self::Service => "service",
            Self::Unknown => "unknown",
        }
    }
}

pub fn public_issue_actor(tenant: &str, principal_id: &str) -> String {
    let digest =
        blake3::hash(format!("myelin.issue.public-actor.v1\0{tenant}\0{principal_id}").as_bytes());
    format!("issue-author-{}@{tenant}.noreply", &digest.to_hex()[..32])
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IssuePseudonym(PseudonymHandle);

impl IssuePseudonym {
    pub fn from_handle(handle: PseudonymHandle) -> IssuePseudonym {
        IssuePseudonym(handle)
    }

    pub fn parse(rendering: &str) -> Result<IssuePseudonym, PseudonymError> {
        match PseudonymHandle::parse(rendering) {
            Some(handle) => Ok(IssuePseudonym(handle)),
            None => Err(PseudonymError::NotPseudonymous(rendering.to_string())),
        }
    }

    pub fn render(&self) -> String {
        self.0.render()
    }

    pub fn token(&self) -> &str {
        self.0.pseudonym()
    }

    pub fn tenant(&self) -> &str {
        self.0.tenant()
    }

    pub fn handle(&self) -> &PseudonymHandle {
        &self.0
    }
}

impl core::fmt::Display for IssuePseudonym {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.render())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PseudonymError {
    NotPseudonymous(String),
    ResolveFailed { subject: String, why: String },
    ResolvedValueMalformed(String),
}

impl core::fmt::Display for PseudonymError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PseudonymError::NotPseudonymous(v) => write!(
                f,
                "REFUSED: `{v}` is not a `<pseudonym>@<tenant>.noreply` pseudonym - an Issues identity \
                 column never holds a raw id / name / email (recon §X-7)"
            ),
            PseudonymError::ResolveFailed { subject, why } => write!(
                f,
                "resolve_pseudonym failed for subject `{subject}` ({why}) - the write fails closed, \
                 never a stored raw id"
            ),
            PseudonymError::ResolvedValueMalformed(v) => write!(
                f,
                "Identity resolved a non-grammar pseudonym `{v}` - refused (4.8 contract break)"
            ),
        }
    }
}

impl std::error::Error for PseudonymError {}

pub fn pseudonymise<Id: IdentityService>(
    id: &Id,
    subject: &PrincipalId,
    tenant: &TenantId,
) -> Result<IssuePseudonym, PseudonymError> {
    let rendering =
        id.resolve_pseudonym(subject, tenant)
            .map_err(|e| PseudonymError::ResolveFailed {
                subject: subject.0.clone(),
                why: format!("{e:?}"),
            })?;
    IssuePseudonym::parse(&rendering).map_err(|_| PseudonymError::ResolvedValueMalformed(rendering))
}

pub fn is_resolvable_pseudonym(stored: &str) -> bool {
    IssuePseudonym::parse(stored).is_ok()
}

pub fn is_raw_principal_id(stored: &str) -> bool {
    !is_resolvable_pseudonym(stored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{
        AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
        EffectivePolicy, FailStaticBound, FragmentAdmit, ListObjectsResult, NamespaceFragment,
        ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalKind, RevokeTarget,
        RewriteTrace, RunId, RunToken, SubjectTree, TupleDelta, Zookie,
    };
    use std::collections::HashMap;

    type IdResult<T> = myelin_identity::Result<T>;

    struct StubId {
        map: HashMap<String, String>,
    }
    impl StubId {
        fn with(subject: &str, pseudonym: &str) -> Self {
            let mut map = HashMap::new();
            map.insert(subject.to_string(), pseudonym.to_string());
            Self { map }
        }
        fn empty() -> Self {
            Self {
                map: HashMap::new(),
            }
        }
    }
    impl IdentityService for StubId {
        fn resolve_pseudonym(&self, subject: &PrincipalId, _t: &TenantId) -> IdResult<String> {
            self.map
                .get(&subject.0)
                .cloned()
                .ok_or(AuthzError::NotYetImplemented("no map entry"))
        }
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn check(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &myelin_tenancy::ArtifactRef,
            _a: &Consistency,
            _c: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            Err(AuthzError::NotYetImplemented("n/a"))
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
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
    }

    fn pid(s: &str) -> PrincipalId {
        PrincipalId(s.into())
    }
    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    #[test]
    fn assignee_resolves_to_a_pseudonym_not_a_raw_id() {
        let id = StubId::with("u-42", "8a2f@acme.noreply");
        let column = pseudonymise(&id, &pid("u-42"), &tenant()).expect("resolves to a pseudonym");
        assert_eq!(column.render(), "8a2f@acme.noreply");
        assert_eq!(column.token(), "8a2f");
        assert_eq!(column.tenant(), "acme");
        assert!(!is_raw_principal_id(&column.render()));
    }

    #[test]
    fn a_raw_id_or_name_or_email_is_refused_at_the_column() {
        for raw in ["u-42", "Ada Lovelace", "ada@example.com", "", "8a2f@acme"] {
            assert!(
                IssuePseudonym::parse(raw).is_err(),
                "`{raw}` must be refused - it is not a `<pseudonym>@<tenant>.noreply` pseudonym"
            );
            assert!(
                is_raw_principal_id(raw),
                "`{raw}` reads as a raw identifier (a leak) - the 0-raw-id gate flags it"
            );
        }
    }

    #[test]
    fn an_unresolvable_subject_fails_closed_never_stores_a_raw_id() {
        let id = StubId::empty();
        let err =
            pseudonymise(&id, &pid("u-99"), &tenant()).expect_err("no map entry → fail closed");
        assert!(matches!(err, PseudonymError::ResolveFailed { .. }));
    }

    #[test]
    fn stored_pseudonym_round_trips_and_is_resolvable_shaped() {
        let stored = "8a2f@acme.noreply";
        assert!(is_resolvable_pseudonym(stored));
        let parsed = IssuePseudonym::parse(stored).unwrap();
        assert_eq!(parsed.render(), stored);
        assert_eq!(IssuePseudonym::parse(&parsed.render()).unwrap(), parsed);
    }

    #[test]
    fn a_malformed_resolved_pseudonym_is_refused() {
        let id = StubId::with("u-7", "not-a-pseudonym");
        let err = pseudonymise(&id, &pid("u-7"), &tenant()).expect_err("malformed → refused");
        assert!(matches!(err, PseudonymError::ResolvedValueMalformed(_)));
    }

    #[test]
    fn agent_and_human_subjects_pseudonymise_identically() {
        let id = StubId::with("agent-1", "ag9c@acme.noreply");
        let _ = PrincipalKind::Human;
        let column = pseudonymise(&id, &pid("agent-1"), &tenant()).expect("agent resolves");
        assert!(!is_raw_principal_id(&column.render()));
    }

    #[test]
    fn public_issue_attribution_is_stable_scoped_and_opaque() {
        let alice = public_issue_actor("acme", "human:alice@example.com");
        assert_eq!(alice, public_issue_actor("acme", "human:alice@example.com"));
        assert_ne!(
            alice,
            public_issue_actor("globex", "human:alice@example.com")
        );
        assert!(!alice.contains("alice"));
        assert!(is_resolvable_pseudonym(&alice));
    }

    #[test]
    fn attribution_kind_comes_from_identity_not_an_identifier_prefix() {
        let agent = Principal::stub(
            PrincipalId("an-opaque-id-without-a-kind-prefix".into()),
            PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef("hosted:luna".into()),
                on_behalf_of: None,
            },
            tenant(),
        );
        assert_eq!(
            IssueActorKind::from_principal(&agent),
            IssueActorKind::Agent
        );
        assert_eq!(
            IssueActorKind::from_stored("agent"),
            Some(IssueActorKind::Agent)
        );
        assert_eq!(IssueActorKind::from_stored("invented"), None);
    }
}
