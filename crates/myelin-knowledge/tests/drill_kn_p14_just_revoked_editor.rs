use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, DataRole, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, PrincipalStatus, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree,
    TupleDelta, Zookie,
};
use myelin_knowledge::{
    AuthZookie, CollectionSchema, IncomingOp, OpAuthorizer, OpDecision, OpPermission, RejectReason,
};
use myelin_query::{FieldType, FieldValue};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn actor(id: &str) -> Principal {
    Principal::new(
        TenantId("acme".into()),
        Region("eu-west".into()),
        PrincipalId(id.into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn page_ref(page: &str) -> ArtifactRef {
    ArtifactRef(format!("kn:page:{page}"))
}

struct ZookieAwareCheck {
    revoked_at: std::collections::HashMap<String, String>,
    granted: std::collections::HashSet<String>,
}

impl ZookieAwareCheck {
    fn new() -> ZookieAwareCheck {
        ZookieAwareCheck {
            revoked_at: std::collections::HashMap::new(),
            granted: std::collections::HashSet::new(),
        }
    }
    fn grant(&mut self, s: &str) {
        self.granted.insert(s.to_string());
    }
    fn revoke_at(&mut self, s: &str, rev: &str) {
        self.revoked_at.insert(s.to_string(), rev.to_string());
    }
}

impl IdentityService for ZookieAwareCheck {
    fn check(
        &self,
        subject: &Principal,
        _permission: &Permission,
        _object: &ArtifactRef,
        at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        let id = &subject.principal_id.0;
        if !self.granted.contains(id) {
            return Ok(Decision::Deny);
        }
        if let Some(rev) = self.revoked_at.get(id) {
            if !rev.is_empty() && at.at_least.0.as_str() >= rev.as_str() {
                return Ok(Decision::Deny);
            }
        }
        Ok(Decision::Allow)
    }
    fn authenticate(&self, _c: &Credential) -> myelin_identity::Result<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _at: &Consistency,
    ) -> myelin_identity::Result<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("list_objects → KN-P16"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> myelin_identity::Result<SubjectTree> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> myelin_identity::Result<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn delegation(
        &self,
        _a: &Principal,
        _t: &Principal,
    ) -> myelin_identity::Result<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn write_tuples(
        &self,
        _d: &[TupleDelta],
        _p: Option<&Precondition>,
    ) -> myelin_identity::Result<Zookie> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _t: &FailStaticBound,
    ) -> myelin_identity::Result<RunToken> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn resolve_pseudonym(
        &self,
        _s: &PrincipalId,
        _t: &TenantId,
    ) -> myelin_identity::Result<String> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn erase(&self, _s: &PrincipalId) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> myelin_identity::Result<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
}

fn content_op(actor: Principal, page: &str, zookie: AuthZookie) -> IncomingOp {
    IncomingOp {
        actor,
        page_id: page.to_string(),
        object: page_ref(page),
        permission: OpPermission::Edit,
        zookie,
        block_id: None,
        db_row: vec![],
    }
}

#[test]
fn kn_p14_just_revoked_editor_op_straddling_zookie_rejected_zero_stale_grant_writes() {
    let mut id = ZookieAwareCheck::new();
    id.grant("alice");
    id.revoke_at("alice", "z9");
    let mut auth = OpAuthorizer::new(id);

    let pre = content_op(actor("alice"), "design-doc", AuthZookie::empty());
    let pre_decision = auth.authorize_op(&pre, &CollectionSchema::new());
    assert_eq!(
        pre_decision,
        OpDecision::Apply,
        "before the revoke Alice's op is authorized"
    );
    let applied_pre = auth.apply_if_authorized(&pre_decision);
    assert!(applied_pre, "the authorized op reaches the merge layer");

    assert!(
        auth.acl_zookies_mut()
            .stamp("design-doc", Zookie("z9".into())),
        "the ACL change stamps page.acl_zookie forward (monotone advance)"
    );

    let post = content_op(
        actor("alice"),
        "design-doc",
        AuthZookie::of(Zookie("z9".into())),
    );
    let post_decision = auth.authorize_op(&post, &CollectionSchema::new());
    assert!(
        matches!(
            post_decision,
            OpDecision::Rejected(RejectReason::PermissionDenied { .. })
        ),
        "the just-revoked editor's op is REJECTED on permission (new-enemy guard fired)"
    );
    let applied_post = auth.apply_if_authorized(&post_decision);
    assert!(
        !applied_post,
        "the rejected op NEVER reaches the merge layer"
    );
    auth.audit_stale_grant_write_if_misapplied(&post_decision, applied_post);

    assert_eq!(
        auth.counter().stale_grant_writes(),
        0,
        "0 STALE-GRANT WRITES - the new-enemy guard let nothing through"
    );
    assert!(
        auth.counter().rejected_by_zookie() >= 1,
        "the zookie guard fired at least once"
    );
    let (metric, n) = auth.counter().telemetry_sample();
    assert_eq!(
        metric, "knowledge.stale_grant_writes",
        "the canonical 0-stale-grant metric name"
    );
    assert_eq!(n, 0, "the dated-green gate value");
}

#[test]
fn kn_p14_stale_client_zookie_cannot_serve_a_revoked_grant() {
    let mut id = ZookieAwareCheck::new();
    id.grant("alice");
    id.revoke_at("alice", "z9");
    let mut auth = OpAuthorizer::new(id);
    auth.acl_zookies_mut()
        .stamp("design-doc", Zookie("z9".into()));

    let op = content_op(
        actor("alice"),
        "design-doc",
        AuthZookie::of(Zookie("z2".into())),
    );
    let decision = auth.authorize_op(&op, &CollectionSchema::new());
    assert!(
        decision.is_rejected(),
        "a stale client zookie cannot serve a since-revoked grant"
    );
    assert!(!auth.apply_if_authorized(&decision));
    assert_eq!(
        auth.counter().stale_grant_writes(),
        0,
        "0 stale-grant writes"
    );
}

#[test]
fn kn_p14_schema_validation_rejects_invalid_db_row_zero_invalid_rows() {
    let mut id = ZookieAwareCheck::new();
    id.grant("alice");
    let mut auth = OpAuthorizer::new(id);
    let schema = CollectionSchema::new()
        .declare("title", FieldType::Text)
        .declare("priority", FieldType::Int)
        .declare("done", FieldType::Bool);

    let mut good = content_op(actor("alice"), "tasks-db", AuthZookie::empty());
    good.db_row = vec![
        ("title".into(), FieldValue::Text("ship KN-P14".into())),
        ("priority".into(), FieldValue::Int(1)),
        ("done".into(), FieldValue::Bool(false)),
    ];
    assert_eq!(
        auth.authorize_op(&good, &schema),
        OpDecision::Apply,
        "a well-typed row is authorized"
    );

    let mut invalid_rows = 0u32;
    let mut bad = content_op(actor("alice"), "tasks-db", AuthZookie::empty());
    bad.db_row = vec![("priority".into(), FieldValue::Text("high".into()))];
    let decision = auth.authorize_op(&bad, &schema);
    let applied = auth.apply_if_authorized(&decision);
    if applied {
        invalid_rows += 1;
    }
    assert!(
        matches!(
            decision,
            OpDecision::Rejected(RejectReason::SchemaViolation { .. })
        ),
        "the malformed db-row op is rejected before merge"
    );
    assert_eq!(
        invalid_rows, 0,
        "0 INVALID ROWS PERSISTED - the schema gate held"
    );
}

#[test]
fn kn_p14_three_check_ladder_order() {
    let mut id = ZookieAwareCheck::new();
    id.grant("alice");
    let mut auth = OpAuthorizer::new(id);
    let schema = CollectionSchema::new().declare("title", FieldType::Text);

    auth.erasures_mut().erase(&page_ref("erased-doc"));
    let mut on_erased = content_op(actor("alice"), "erased-doc", AuthZookie::empty());
    on_erased.db_row = vec![("title".into(), FieldValue::Int(7))];
    assert_eq!(
        auth.authorize_op(&on_erased, &schema),
        OpDecision::Degraded,
        "erased content degrades regardless of permission/schema"
    );
}
