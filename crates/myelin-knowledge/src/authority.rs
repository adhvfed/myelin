use crate::block_tree::BlockId;
use myelin_identity::{
    CaveatContext, Consistency, ConsistencyMode, Decision, IdentityService, Permission, Principal,
    Zookie,
};
use myelin_query::{FieldType, FieldValue};
use myelin_tenancy::ArtifactRef;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpPermission {
    Edit,
    Comment,
}

impl OpPermission {
    pub fn permission(self) -> Permission {
        match self {
            OpPermission::Edit => Permission("edit".into()),
            OpPermission::Comment => Permission("comment".into()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OpPermission::Edit => "edit",
            OpPermission::Comment => "comment",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthZookie(Zookie);

impl AuthZookie {
    pub fn of(zookie: Zookie) -> AuthZookie {
        AuthZookie(zookie)
    }

    pub fn empty() -> AuthZookie {
        AuthZookie(Zookie(String::new()))
    }

    pub fn consistency(&self) -> Consistency {
        Consistency {
            at_least: self.0.clone(),
            mode: ConsistencyMode::Strong,
        }
    }

    pub fn zookie(&self) -> &Zookie {
        &self.0
    }
}

#[derive(Debug, Default, Clone)]
pub struct AclZookieTable {
    stamps: BTreeMap<String, Zookie>,
}

impl AclZookieTable {
    pub fn new() -> AclZookieTable {
        AclZookieTable::default()
    }

    pub fn stamp(&mut self, page_id: &str, new_zookie: Zookie) -> bool {
        match self.stamps.get(page_id) {
            Some(current) if new_zookie.0 <= current.0 => false,
            _ => {
                self.stamps.insert(page_id.to_string(), new_zookie);
                true
            }
        }
    }

    pub fn current(&self, page_id: &str) -> AuthZookie {
        self.stamps
            .get(page_id)
            .cloned()
            .map(AuthZookie::of)
            .unwrap_or_else(AuthZookie::empty)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejectReason {
    PermissionDenied { page_id: String },
    SchemaViolation { detail: String },
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RejectReason::PermissionDenied { page_id } => {
                write!(
                    f,
                    "Layer-2 permission denied on page `{page_id}` (no op without authz)"
                )
            }
            RejectReason::SchemaViolation { detail } => {
                write!(
                    f,
                    "Layer-2 schema validation rejected the db-row op: {detail}"
                )
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpDecision {
    Apply,
    Rejected(RejectReason),
    Degraded,
}

impl OpDecision {
    pub fn applied(&self) -> bool {
        matches!(self, OpDecision::Apply)
    }

    pub fn is_rejected(&self) -> bool {
        matches!(self, OpDecision::Rejected(_))
    }

    pub fn is_degraded(&self) -> bool {
        matches!(self, OpDecision::Degraded)
    }
}

#[derive(Debug, Default, Clone)]
pub struct ErasureLedger {
    erased: std::collections::BTreeSet<String>,
}

impl ErasureLedger {
    pub fn new() -> ErasureLedger {
        ErasureLedger::default()
    }

    pub fn erase(&mut self, artifact: &ArtifactRef) {
        self.erased.insert(artifact.0.clone());
    }

    pub fn is_erased(&self, artifact: &ArtifactRef) -> bool {
        self.erased.contains(&artifact.0)
    }
}

#[derive(Debug, Default, Clone)]
pub struct CollectionSchema {
    fields: BTreeMap<String, FieldType>,
}

impl CollectionSchema {
    pub fn new() -> CollectionSchema {
        CollectionSchema::default()
    }

    pub fn declare(mut self, name: impl Into<String>, ty: FieldType) -> CollectionSchema {
        self.fields.insert(name.into(), ty);
        self
    }

    pub fn field_type(&self, name: &str) -> Option<FieldType> {
        self.fields.get(name).copied()
    }
}

#[derive(Debug, Default, Clone)]
pub struct SchemaValidator;

impl SchemaValidator {
    pub fn validate(
        &self,
        schema: &CollectionSchema,
        row: &[(String, FieldValue)],
    ) -> Result<(), RejectReason> {
        for (name, value) in row {
            match schema.field_type(name) {
                None => {
                    return Err(RejectReason::SchemaViolation {
                        detail: format!("undeclared field `{name}`"),
                    });
                }
                Some(declared) => {
                    let actual = value.field_type();
                    if actual != declared {
                        return Err(RejectReason::SchemaViolation {
                            detail: format!(
                                "field `{name}` is `{}`, got `{}`",
                                declared.wire_id(),
                                actual.wire_id()
                            ),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StaleGrantCounter {
    stale_grant_writes: u64,
    rejected_by_zookie: u64,
}

pub const STALE_GRANT_WRITES_METRIC: &str = "knowledge.stale_grant_writes";

impl StaleGrantCounter {
    pub fn new() -> StaleGrantCounter {
        StaleGrantCounter::default()
    }

    fn record_zookie_rejection(&mut self) {
        self.rejected_by_zookie += 1;
    }

    fn record_stale_grant_write(&mut self) {
        self.stale_grant_writes += 1;
    }

    pub fn stale_grant_writes(&self) -> u64 {
        self.stale_grant_writes
    }

    pub fn rejected_by_zookie(&self) -> u64 {
        self.rejected_by_zookie
    }

    pub fn telemetry_sample(&self) -> (&'static str, u64) {
        (STALE_GRANT_WRITES_METRIC, self.stale_grant_writes)
    }
}

#[derive(Clone, Debug)]
pub struct IncomingOp {
    pub actor: Principal,
    pub page_id: String,
    pub object: ArtifactRef,
    pub permission: OpPermission,
    pub zookie: AuthZookie,
    pub block_id: Option<BlockId>,
    pub db_row: Vec<(String, FieldValue)>,
}

pub struct OpAuthorizer<S: IdentityService> {
    identity: S,
    acl_zookies: AclZookieTable,
    erasures: ErasureLedger,
    validator: SchemaValidator,
    counter: StaleGrantCounter,
}

impl<S: IdentityService> OpAuthorizer<S> {
    pub fn new(identity: S) -> OpAuthorizer<S> {
        OpAuthorizer {
            identity,
            acl_zookies: AclZookieTable::new(),
            erasures: ErasureLedger::new(),
            validator: SchemaValidator,
            counter: StaleGrantCounter::new(),
        }
    }

    pub fn acl_zookies_mut(&mut self) -> &mut AclZookieTable {
        &mut self.acl_zookies
    }

    pub fn acl_zookies(&self) -> &AclZookieTable {
        &self.acl_zookies
    }

    pub fn erasures_mut(&mut self) -> &mut ErasureLedger {
        &mut self.erasures
    }

    pub fn counter(&self) -> &StaleGrantCounter {
        &self.counter
    }

    pub fn authorize_op(&mut self, op: &IncomingOp, schema: &CollectionSchema) -> OpDecision {
        if self.erasures.is_erased(&op.object) {
            return OpDecision::Degraded;
        }

        let page_zookie = self.effective_zookie(op);
        let at = page_zookie.consistency();
        let permission = op.permission.permission();
        let decision = self
            .identity
            .check(&op.actor, &permission, &op.object, &at, None);
        match decision {
            Ok(Decision::Allow) => {}
            _ => {
                self.counter.record_zookie_rejection();
                return OpDecision::Rejected(RejectReason::PermissionDenied {
                    page_id: op.page_id.clone(),
                });
            }
        }

        if !op.db_row.is_empty() {
            if let Err(reason) = self.validator.validate(schema, &op.db_row) {
                return OpDecision::Rejected(reason);
            }
        }

        OpDecision::Apply
    }

    fn effective_zookie(&self, op: &IncomingOp) -> AuthZookie {
        let page = self.acl_zookies.current(&op.page_id);
        if op.zookie.zookie().0 > page.zookie().0 {
            op.zookie.clone()
        } else {
            page
        }
    }

    pub fn apply_if_authorized(&mut self, decision: &OpDecision) -> bool {
        decision.applied()
    }

    #[doc(hidden)]
    pub fn audit_stale_grant_write_if_misapplied(
        &mut self,
        decision: &OpDecision,
        was_applied: bool,
    ) {
        if was_applied && !decision.applied() {
            self.counter.record_stale_grant_write();
        }
    }
}

pub fn field_caveat(
    object: ArtifactRef,
    attrs: BTreeMap<String, myelin_identity::Literal>,
) -> CaveatContext {
    CaveatContext {
        object,
        field: None,
        transition: None,
        attrs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{
        AuthzError, Credential, DataRole, DelegationCaveats, EffectivePolicy, FragmentAdmit,
        ListObjectsResult, NamespaceFragment, ObjectId, ObjectType, Precondition, PrincipalId,
        PrincipalKind, PrincipalStatus, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree,
        TupleDelta,
    };
    use myelin_tenancy::{Region, TenantId};

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
        fn grant(&mut self, subject: &str) {
            self.granted.insert(subject.to_string());
        }
        fn revoke_at(&mut self, subject: &str, revision: &str) {
            self.revoked_at
                .insert(subject.to_string(), revision.to_string());
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
            if let Some(revoked) = self.revoked_at.get(id) {
                if at.at_least.0.as_str() >= revoked.as_str() && !revoked.is_empty() {
                    return Ok(Decision::Deny);
                }
            }
            Ok(Decision::Allow)
        }

        fn authenticate(&self, _c: &Credential) -> myelin_identity::Result<Principal> {
            Err(AuthzError::NotYetImplemented(
                "not used by the Layer-2 op gate",
            ))
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
            _t: &myelin_identity::FailStaticBound,
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

    fn op_for(actor: Principal, page: &str, perm: OpPermission, zookie: AuthZookie) -> IncomingOp {
        IncomingOp {
            actor,
            page_id: page.to_string(),
            object: page_ref(page),
            permission: perm,
            zookie,
            block_id: None,
            db_row: vec![],
        }
    }

    #[test]
    fn granted_editor_op_is_applied() {
        let mut id = ZookieAwareCheck::new();
        id.grant("alice");
        let mut auth = OpAuthorizer::new(id);
        let op = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        let decision = auth.authorize_op(&op, &CollectionSchema::new());
        assert_eq!(
            decision,
            OpDecision::Apply,
            "a granted editor's op is authorized"
        );
        assert_eq!(
            auth.counter().stale_grant_writes(),
            0,
            "0 stale-grant writes"
        );
    }

    #[test]
    fn ungranted_actor_op_is_rejected() {
        let id = ZookieAwareCheck::new();
        let mut auth = OpAuthorizer::new(id);
        let op = op_for(
            actor("mallory"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        let decision = auth.authorize_op(&op, &CollectionSchema::new());
        assert!(
            decision.is_rejected(),
            "an ungranted op is rejected (fail-closed, ADR-03)"
        );
    }

    #[test]
    fn just_revoked_editor_op_straddling_zookie_is_rejected_zero_stale_grant_writes() {
        let mut id = ZookieAwareCheck::new();
        id.grant("alice");
        id.revoke_at("alice", "z5");
        let mut auth = OpAuthorizer::new(id);

        let before = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        assert_eq!(
            auth.authorize_op(&before, &CollectionSchema::new()),
            OpDecision::Apply,
            "before the revoke Alice (granted) can edit"
        );

        assert!(
            auth.acl_zookies_mut().stamp("p1", Zookie("z5".into())),
            "the ACL change stamps page.acl_zookie forward (monotone advance)"
        );

        let after = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::of(Zookie("z5".into())),
        );
        let decision = auth.authorize_op(&after, &CollectionSchema::new());
        assert!(
            decision.is_rejected(),
            "the just-revoked editor's op (straddling the zookie) is REJECTED (new-enemy guard)"
        );
        let was_applied = auth.apply_if_authorized(&decision);
        assert!(
            !was_applied,
            "the rejected op is NOT applied to the merge layer"
        );
        auth.audit_stale_grant_write_if_misapplied(&decision, was_applied);
        assert_eq!(
            auth.counter().stale_grant_writes(),
            0,
            "0 STALE-GRANT WRITES - the new-enemy guard let nothing through"
        );
        assert!(
            auth.counter().rejected_by_zookie() >= 1,
            "the zookie guard fired"
        );
        let (name, n) = auth.counter().telemetry_sample();
        assert_eq!(name, "knowledge.stale_grant_writes");
        assert_eq!(n, 0, "the dated-green gate value");
    }

    #[test]
    fn audit_lever_detects_a_misapplied_rejected_op() {
        let id = ZookieAwareCheck::new();
        let mut auth = OpAuthorizer::new(id);
        let op = op_for(
            actor("mallory"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        let decision = auth.authorize_op(&op, &CollectionSchema::new());
        assert!(decision.is_rejected());
        assert!(!auth.apply_if_authorized(&decision));
        auth.audit_stale_grant_write_if_misapplied(&decision, false);
        assert_eq!(
            auth.counter().stale_grant_writes(),
            0,
            "the faithful path keeps the count 0"
        );
        auth.audit_stale_grant_write_if_misapplied(&decision, true);
        assert_eq!(
            auth.counter().stale_grant_writes(),
            1,
            "the audit lever DETECTS a misapplied rejected op - the gate is falsifiable"
        );
    }

    #[test]
    fn stale_client_zookie_cannot_downgrade_the_watermark() {
        let mut id = ZookieAwareCheck::new();
        id.grant("alice");
        id.revoke_at("alice", "z5");
        let mut auth = OpAuthorizer::new(id);
        auth.acl_zookies_mut().stamp("p1", Zookie("z5".into()));

        let op = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::of(Zookie("z1".into())),
        );
        let decision = auth.authorize_op(&op, &CollectionSchema::new());
        assert!(
            decision.is_rejected(),
            "a stale client zookie cannot downgrade below the page's current acl_zookie (no stale grant)"
        );
    }

    #[test]
    fn acl_zookie_stamp_monotonically_advances() {
        let mut table = AclZookieTable::new();
        assert_eq!(
            table.current("p1"),
            AuthZookie::empty(),
            "an un-stamped page is at the empty zookie"
        );
        assert!(
            table.stamp("p1", Zookie("z2".into())),
            "first stamp advances"
        );
        assert!(
            table.stamp("p1", Zookie("z5".into())),
            "a later revision advances"
        );
        assert!(
            !table.stamp("p1", Zookie("z3".into())),
            "an OLDER revision is REFUSED (monotone)"
        );
        assert!(
            !table.stamp("p1", Zookie("z5".into())),
            "the SAME revision is refused (strictly later)"
        );
        assert_eq!(
            table.current("p1").zookie().0,
            "z5",
            "the watermark stays at the latest"
        );
    }

    #[test]
    fn schema_validation_rejects_a_type_mismatch_before_merge() {
        let mut id = ZookieAwareCheck::new();
        id.grant("alice");
        let mut auth = OpAuthorizer::new(id);
        let schema = CollectionSchema::new()
            .declare("title", FieldType::Text)
            .declare("count", FieldType::Int);

        let mut good = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        good.db_row = vec![
            ("title".into(), FieldValue::Text("hi".into())),
            ("count".into(), FieldValue::Int(3)),
        ];
        assert_eq!(
            auth.authorize_op(&good, &schema),
            OpDecision::Apply,
            "a well-typed db-row op applies"
        );

        let mut bad = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        bad.db_row = vec![("count".into(), FieldValue::Text("not-an-int".into()))];
        let decision = auth.authorize_op(&bad, &schema);
        match decision {
            OpDecision::Rejected(RejectReason::SchemaViolation { detail }) => {
                assert!(
                    detail.contains("count"),
                    "the violation names the field: {detail}"
                );
                assert!(
                    detail.contains("int"),
                    "it names the declared type: {detail}"
                );
            }
            other => panic!("expected a SchemaViolation, got {other:?}"),
        }
    }

    #[test]
    fn schema_validation_rejects_an_undeclared_field() {
        let mut id = ZookieAwareCheck::new();
        id.grant("alice");
        let mut auth = OpAuthorizer::new(id);
        let schema = CollectionSchema::new().declare("title", FieldType::Text);
        let mut op = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        op.db_row = vec![("ghost".into(), FieldValue::Text("x".into()))];
        let decision = auth.authorize_op(&op, &schema);
        assert!(
            matches!(
                decision,
                OpDecision::Rejected(RejectReason::SchemaViolation { .. })
            ),
            "an undeclared field is rejected (the schema is closed)"
        );
    }

    #[test]
    fn permission_is_checked_before_schema() {
        let id = ZookieAwareCheck::new();
        let mut auth = OpAuthorizer::new(id);
        let schema = CollectionSchema::new().declare("title", FieldType::Text);
        let mut op = op_for(
            actor("mallory"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        op.db_row = vec![("count".into(), FieldValue::Int(1))];
        let decision = auth.authorize_op(&op, &schema);
        assert!(
            matches!(
                decision,
                OpDecision::Rejected(RejectReason::PermissionDenied { .. })
            ),
            "an ungranted op is rejected on permission first"
        );
    }

    #[test]
    fn op_against_erased_content_degrades() {
        let mut id = ZookieAwareCheck::new();
        id.grant("alice");
        let mut auth = OpAuthorizer::new(id);
        let obj = page_ref("p1");
        auth.erasures_mut().erase(&obj);

        let op = op_for(
            actor("alice"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        let decision = auth.authorize_op(&op, &CollectionSchema::new());
        assert_eq!(
            decision,
            OpDecision::Degraded,
            "an op against erased content degrades (never applies, never resurrects)"
        );
        assert!(decision.is_degraded());
        assert!(
            !decision.applied(),
            "a degraded op never reaches the merge layer"
        );
    }

    #[test]
    fn erasure_is_idempotent_and_independent_of_permission() {
        let id = ZookieAwareCheck::new();
        let mut auth = OpAuthorizer::new(id);
        let obj = page_ref("p1");
        auth.erasures_mut().erase(&obj);
        auth.erasures_mut().erase(&obj);
        let op = op_for(
            actor("mallory"),
            "p1",
            OpPermission::Edit,
            AuthZookie::empty(),
        );
        assert_eq!(
            auth.authorize_op(&op, &CollectionSchema::new()),
            OpDecision::Degraded,
            "erased content degrades regardless of permission; re-erase never resurrects"
        );
    }

    #[test]
    fn comment_op_authorizes_the_comment_permission() {
        assert_eq!(
            OpPermission::Comment.permission(),
            Permission("comment".into())
        );
        assert_eq!(OpPermission::Edit.permission(), Permission("edit".into()));
        assert_eq!(OpPermission::Comment.as_str(), "comment");
    }

    #[test]
    fn per_op_check_reads_at_strong_consistency() {
        let z = AuthZookie::of(Zookie("z5".into()));
        let at = z.consistency();
        assert_eq!(
            at.mode,
            ConsistencyMode::Strong,
            "the per-op check is read-your-writes (4.10)"
        );
        assert_eq!(
            at.at_least.0, "z5",
            "at-or-after the page's stamped acl_zookie"
        );
    }
}
