pub mod iam_events;

pub use iam_events::{
    signals, IamEventProjection, IamSubjectRef, IDENTITY_BREAK_GLASS, IDENTITY_EVENT_TOKENS,
    IDENTITY_ROLE_GRANTED, IDENTITY_TUPLE_WRITTEN,
};

use myelin_tenancy::{ArtifactRef, Region, TenantId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type PrincipalRegion = Region;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrincipalId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuntimeRef(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataRole {
    Controller,
    Processor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrincipalStatus {
    Active,
    Suspended,
    Disabled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrincipalKind {
    Human,
    Agent {
        runtime_ref: RuntimeRef,
        on_behalf_of: Option<PrincipalId>,
    },
    Service,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub tenant: TenantId,
    pub region: Region,
    pub principal_id: PrincipalId,
    pub kind: PrincipalKind,
    pub data_role: DataRole,
    pub status: PrincipalStatus,
}

impl Principal {
    pub fn new(
        tenant: TenantId,
        region: Region,
        principal_id: PrincipalId,
        kind: PrincipalKind,
        data_role: DataRole,
        status: PrincipalStatus,
    ) -> Self {
        Principal {
            tenant,
            region,
            principal_id,
            kind,
            data_role,
            status,
        }
    }

    pub fn stub(principal_id: PrincipalId, kind: PrincipalKind, tenant: TenantId) -> Self {
        Principal {
            region: Region(format!("{}-home", tenant.0)),
            tenant,
            principal_id,
            kind,
            data_role: DataRole::Controller,
            status: PrincipalStatus::Active,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    pub scheme: String,
    pub material: String,
}

impl core::fmt::Debug for Credential {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Credential")
            .field("scheme", &self.scheme)
            .field("material", &"<redacted>")
            .finish()
    }
}

impl Drop for Credential {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.material.zeroize();
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectType(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelName(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Permission(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ColRef {
    pub table: String,
    pub column: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AuthzIndexRef(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SetExpr {
    All,
    None,
    Ids(Vec<ObjectId>),
    NotIds(Vec<ObjectId>),
    InRelation {
        relation: RelName,
        via_column: ColRef,
    },
    Union(Vec<SetExpr>),
    Intersect(Vec<SetExpr>),
    Difference(Box<SetExpr>, Box<SetExpr>),
    TupleSet {
        index: AuthzIndexRef,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListObjectsResult {
    Ids { ids: Vec<ObjectId>, zookie: Zookie },
    Filter { set_expr: SetExpr, zookie: Zookie },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectTree {
    pub object: ObjectId,
    pub relation: RelName,
    pub members: Vec<PrincipalId>,
    pub zookie: Zookie,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewriteTrace {
    pub steps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FieldId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransitionId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Literal {
    Bool(bool),
    Int(i64),
    Str(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaveatContext {
    pub object: ArtifactRef,
    pub field: Option<FieldId>,
    pub transition: Option<TransitionId>,
    pub attrs: BTreeMap<String, Literal>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Allow,
    Deny,
    Conditional,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Zookie(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsistencyMode {
    Strong,
    BoundedStale,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Consistency {
    pub at_least: Zookie,
    pub mode: ConsistencyMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailStaticBound {
    pub static_max_secs: u64,
}

impl FailStaticBound {
    pub const DEFAULT_W: FailStaticBound = FailStaticBound {
        static_max_secs: 300,
    };
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationTuple {
    pub object: ObjectId,
    pub relation: RelName,
    pub subject: PrincipalId,
    pub caveat: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TupleDelta {
    Add(RelationTuple),
    Remove(RelationTuple),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Precondition {
    pub expected_zookie: Option<Zookie>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectivePolicy {
    pub caveats: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationCaveats(pub Vec<String>);

#[derive(Clone, PartialEq, Eq)]
pub struct RunToken {
    pub token: String,
    pub jti: String,
}

impl core::fmt::Debug for RunToken {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RunToken")
            .field("token", &"<redacted>")
            .field("jti", &"<redacted>")
            .finish()
    }
}

impl RunToken {
    pub fn into_parts(mut self) -> (String, String) {
        (
            core::mem::take(&mut self.token),
            core::mem::take(&mut self.jti),
        )
    }
}

impl Drop for RunToken {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.token.zeroize();
        self.jti.zeroize();
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevokeTarget {
    Jti(String),
    Principal(PrincipalId),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamespaceFragment {
    pub object_type: ObjectType,
    pub relations: Vec<RelName>,
    pub permissions: Vec<Permission>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FragmentAdmit {
    Admitted { fragment_id: String },
    Rejected { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthzError {
    BadRequest(String),
    Unavailable(String),
    FailClosed(String),
    NotYetImplemented(&'static str),
}

pub type Result<T> = core::result::Result<T, AuthzError>;

pub trait IdentityService {
    fn authenticate(&self, credential: &Credential) -> Result<Principal>;

    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
        caveat: Option<&CaveatContext>,
    ) -> Result<Decision>;

    fn list_objects(
        &self,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> Result<ListObjectsResult>;

    fn list_subjects(
        &self,
        object: &ObjectId,
        permission: &Permission,
        at: &Consistency,
    ) -> Result<SubjectTree>;

    fn explain(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ObjectId,
        at: &Consistency,
    ) -> Result<RewriteTrace>;

    fn delegation(&self, agent: &Principal, trigger_actor: &Principal) -> Result<EffectivePolicy>;

    fn write_tuples(
        &self,
        deltas: &[TupleDelta],
        precondition: Option<&Precondition>,
    ) -> Result<Zookie>;

    fn mint_run_token(
        &self,
        agent_id: &PrincipalId,
        run_id: &RunId,
        delegation_caveats: &DelegationCaveats,
        ttl: &FailStaticBound,
    ) -> Result<RunToken>;

    fn revoke(&self, target: &RevokeTarget) -> Result<()>;

    fn resolve_pseudonym(&self, subject: &PrincipalId, tenant: &TenantId) -> Result<String>;

    fn erase(&self, subject: &PrincipalId) -> Result<()>;

    fn admit_fragment(&self, fragment: &NamespaceFragment) -> Result<FragmentAdmit>;
}

#[deprecated(
    since = "0.0.0",
    note = "renamed to IdentityService (the full eleven-method §11.1 ABI, P-ID-01)"
)]
pub trait AuthzClient: IdentityService {}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PseudonymHandle {
    pseudonym: String,
    tenant: String,
}

pub const PSEUDONYM_DOMAIN_SUFFIX: &str = ".noreply";

impl PseudonymHandle {
    pub fn new(pseudonym: impl Into<String>, tenant: impl Into<String>) -> Option<PseudonymHandle> {
        let pseudonym = pseudonym.into();
        let tenant = tenant.into();
        if pseudonym.is_empty() || tenant.is_empty() {
            return None;
        }
        if pseudonym.contains('@') {
            return None;
        }
        if tenant.contains('@') || tenant.contains('.') {
            return None;
        }
        Some(PseudonymHandle { pseudonym, tenant })
    }

    pub fn pseudonym(&self) -> &str {
        &self.pseudonym
    }

    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    pub fn render(&self) -> String {
        format!(
            "{}@{}{}",
            self.pseudonym, self.tenant, PSEUDONYM_DOMAIN_SUFFIX
        )
    }

    pub fn parse(s: &str) -> Option<PseudonymHandle> {
        let local_and_domain = s.strip_suffix(PSEUDONYM_DOMAIN_SUFFIX)?;
        let (pseudonym, tenant) = local_and_domain.split_once('@')?;
        PseudonymHandle::new(pseudonym.to_string(), tenant.to_string())
    }
}

impl core::fmt::Display for PseudonymHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.render())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_debug_redacts_material_and_owns_zeroizing_drop() {
        let credential = Credential {
            scheme: "pat".into(),
            material: "secret-material".into(),
        };
        let rendered = format!("{credential:?}");
        assert!(rendered.contains("pat"));
        assert!(!rendered.contains("secret-material"));
        assert!(rendered.contains("<redacted>"));
        assert!(core::mem::needs_drop::<Credential>());
    }

    #[test]
    fn run_token_debug_redacts_bearer_and_jti() {
        let token = RunToken {
            token: "secret-bearer".into(),
            jti: "secret-jti".into(),
        };
        let rendered = format!("{token:?}");
        assert!(!rendered.contains("secret-bearer"));
        assert!(!rendered.contains("secret-jti"));
        assert!(rendered.contains("<redacted>"));
        assert!(core::mem::needs_drop::<RunToken>());
    }

    #[test]
    fn principal_carries_the_frozen_six_fields() {
        let p = Principal::new(
            TenantId("acme".into()),
            Region("eu-west".into()),
            PrincipalId("p1".into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt".into()),
                on_behalf_of: Some(PrincipalId("human".into())),
            },
            DataRole::Processor,
            PrincipalStatus::Active,
        );
        assert!(matches!(p.kind, PrincipalKind::Agent { .. }));
        assert_eq!(p.data_role, DataRole::Processor);
        assert_eq!(p.status, PrincipalStatus::Active);
        assert_eq!(p.region, Region("eu-west".into()));
        assert_eq!(p.principal_id, PrincipalId("p1".into()));
    }

    #[test]
    fn data_role_variant_tokens_match_the_events_reconciliation() {
        assert_eq!(
            serde_json::to_string(&DataRole::Controller).unwrap(),
            "\"Controller\""
        );
        assert_eq!(
            serde_json::to_string(&DataRole::Processor).unwrap(),
            "\"Processor\""
        );
    }

    #[test]
    fn set_expr_and_caveat_round_trip_stably() {
        let expr = SetExpr::Union(vec![
            SetExpr::All,
            SetExpr::None,
            SetExpr::Ids(vec![ObjectId("a".into()), ObjectId("b".into())]),
            SetExpr::NotIds(vec![ObjectId("c".into())]),
            SetExpr::InRelation {
                relation: RelName("reader".into()),
                via_column: ColRef {
                    table: "issue".into(),
                    column: "id".into(),
                },
            },
            SetExpr::Intersect(vec![SetExpr::All]),
            SetExpr::Difference(Box::new(SetExpr::All), Box::new(SetExpr::None)),
            SetExpr::TupleSet {
                index: AuthzIndexRef("authz_visible".into()),
            },
        ]);
        let json = serde_json::to_string(&expr).unwrap();
        let back: SetExpr = serde_json::from_str(&json).unwrap();
        assert_eq!(expr, back);

        assert!(json.contains("InRelation"));
        assert!(json.contains("via_column"));
        assert!(json.contains("TupleSet"));

        let mut attrs = BTreeMap::new();
        attrs.insert("severity".to_string(), Literal::Int(3));
        attrs.insert("confidential".to_string(), Literal::Bool(true));
        attrs.insert("owner".to_string(), Literal::Str("alice".into()));
        let caveat = CaveatContext {
            object: ArtifactRef("myelin://acme/issue/issue/PROJ-1".into()),
            field: Some(FieldId("salary".into())),
            transition: Some(TransitionId("approve".into())),
            attrs,
        };
        let cjson = serde_json::to_string(&caveat).unwrap();
        let cback: CaveatContext = serde_json::from_str(&cjson).unwrap();
        assert_eq!(caveat, cback);
        assert!(cjson.contains("\"object\""));
        assert!(cjson.contains("\"field\""));
        assert!(cjson.contains("\"transition\""));
        assert!(cjson.contains("\"attrs\""));
    }

    #[test]
    fn list_objects_result_two_variants_each_with_zookie() {
        let ids = ListObjectsResult::Ids {
            ids: vec![ObjectId("x".into())],
            zookie: Zookie("z1".into()),
        };
        let filter = ListObjectsResult::Filter {
            set_expr: SetExpr::All,
            zookie: Zookie("z2".into()),
        };
        for r in [&ids, &filter] {
            let s = serde_json::to_string(r).unwrap();
            let b: ListObjectsResult = serde_json::from_str(&s).unwrap();
            assert_eq!(r, &b);
        }
    }

    #[test]
    fn fail_static_default_w_is_five_minutes() {
        assert_eq!(FailStaticBound::DEFAULT_W.static_max_secs, 300);
    }

    #[test]
    fn identity_service_eleven_signatures_are_frozen_and_implementable() {
        struct StubId;
        impl IdentityService for StubId {
            fn authenticate(&self, _c: &Credential) -> Result<Principal> {
                Err(AuthzError::NotYetImplemented(
                    "authenticate → P-ID-06/07 (M1)",
                ))
            }
            fn check(
                &self,
                _s: &Principal,
                _p: &Permission,
                _o: &ArtifactRef,
                _at: &Consistency,
                _cav: Option<&CaveatContext>,
            ) -> Result<Decision> {
                Ok(Decision::Deny)
            }
            fn list_objects(
                &self,
                _s: &Principal,
                _p: &Permission,
                _ty: &ObjectType,
                _at: &Consistency,
            ) -> Result<ListObjectsResult> {
                Err(AuthzError::NotYetImplemented(
                    "list_objects → P-ID-11/12 (M1)",
                ))
            }
            fn list_subjects(
                &self,
                _o: &ObjectId,
                _p: &Permission,
                _at: &Consistency,
            ) -> Result<SubjectTree> {
                Err(AuthzError::NotYetImplemented(
                    "list_subjects → P-ID-13 (M1)",
                ))
            }
            fn explain(
                &self,
                _s: &Principal,
                _p: &Permission,
                _o: &ObjectId,
                _at: &Consistency,
            ) -> Result<RewriteTrace> {
                Err(AuthzError::NotYetImplemented("explain → P-ID-13 (M1)"))
            }
            fn delegation(&self, _a: &Principal, _t: &Principal) -> Result<EffectivePolicy> {
                Err(AuthzError::NotYetImplemented("delegation → P-ID-17 (M1)"))
            }
            fn write_tuples(
                &self,
                _d: &[TupleDelta],
                _pre: Option<&Precondition>,
            ) -> Result<Zookie> {
                Err(AuthzError::NotYetImplemented("write_tuples → P-ID-08 (M1)"))
            }
            fn mint_run_token(
                &self,
                _a: &PrincipalId,
                _r: &RunId,
                _d: &DelegationCaveats,
                _ttl: &FailStaticBound,
            ) -> Result<RunToken> {
                Err(AuthzError::NotYetImplemented(
                    "mint_run_token → P-ID-18 (M1)",
                ))
            }
            fn revoke(&self, _t: &RevokeTarget) -> Result<()> {
                Err(AuthzError::NotYetImplemented("revoke → P-ID-14 (M1)"))
            }
            fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> Result<String> {
                Err(AuthzError::NotYetImplemented(
                    "resolve_pseudonym → P-ID-19 (M1)",
                ))
            }
            fn erase(&self, _s: &PrincipalId) -> Result<()> {
                Err(AuthzError::NotYetImplemented("erase → P-ID-20 (M1)"))
            }
            fn admit_fragment(&self, _f: &NamespaceFragment) -> Result<FragmentAdmit> {
                Err(AuthzError::NotYetImplemented(
                    "admit_fragment → P-ID-10 (M1)",
                ))
            }
        }

        let id = StubId;
        let subject = Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Service,
            TenantId("t".into()),
        );
        let at = Consistency {
            at_least: Zookie("z".into()),
            mode: ConsistencyMode::Strong,
        };

        let d = id.check(
            &subject,
            &Permission("read".into()),
            &ArtifactRef("myelin://t/issue/issue/PROJ-1".into()),
            &at,
            None,
        );
        assert_eq!(d, Ok(Decision::Deny));

        assert!(matches!(
            id.authenticate(&Credential {
                scheme: "oidc".into(),
                material: "tok".into()
            }),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.list_objects(
                &subject,
                &Permission("read".into()),
                &ObjectType("issue".into()),
                &at
            ),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.list_subjects(&ObjectId("o".into()), &Permission("watcher".into()), &at),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.delegation(&subject, &subject),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.write_tuples(&[], None),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.mint_run_token(
                &PrincipalId("agent".into()),
                &RunId("run-1".into()),
                &DelegationCaveats(vec![]),
                &FailStaticBound::DEFAULT_W
            ),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.revoke(&RevokeTarget::Jti("j".into())),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.resolve_pseudonym(&PrincipalId("p".into()), &TenantId("t".into())),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.erase(&PrincipalId("p".into())),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.admit_fragment(&NamespaceFragment {
                object_type: ObjectType("issue".into()),
                relations: vec![RelName("reader".into())],
                permissions: vec![Permission("view".into())],
            }),
            Err(AuthzError::NotYetImplemented(_))
        ));
    }

    #[test]
    fn pseudonym_grammar_renders_the_frozen_shape() {
        let h = PseudonymHandle::new("anon-7f3a", "acme").expect("a well-formed handle");
        assert_eq!(
            h.render(),
            "anon-7f3a@acme.noreply",
            "the frozen grammar is `<pseudonym>@<tenant>.noreply`"
        );
        assert_eq!(h.to_string(), "anon-7f3a@acme.noreply");
    }

    #[test]
    fn pseudonym_grammar_round_trips() {
        let h = PseudonymHandle::new("anon-7f3a", "globex").unwrap();
        let parsed = PseudonymHandle::parse(&h.render()).expect("the rendering parses back");
        assert_eq!(parsed, h, "parse(render(h)) == h");
        assert_eq!(parsed.pseudonym(), "anon-7f3a");
        assert_eq!(parsed.tenant(), "globex");
    }

    #[test]
    fn pseudonym_grammar_refuses_non_conforming() {
        assert!(PseudonymHandle::parse("alice@acme.com").is_none());
        assert!(PseudonymHandle::parse("anon-acme.noreply").is_none());
        assert!(PseudonymHandle::parse("a@b@acme.noreply").is_none());
        assert!(PseudonymHandle::parse("@acme.noreply").is_none());
        assert!(PseudonymHandle::parse("anon@.noreply").is_none());
        assert!(PseudonymHandle::new("anon", "ac.me").is_none());
        assert!(PseudonymHandle::new("a@b", "acme").is_none());
        assert!(PseudonymHandle::new("", "acme").is_none());
        assert!(PseudonymHandle::new("anon", "").is_none());
    }
}
