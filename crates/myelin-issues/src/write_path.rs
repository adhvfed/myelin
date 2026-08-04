use crate::dek::{self, IssueFreeText};
use crate::events;
use crate::pseudonym::{self, IssuePseudonym, PseudonymError};
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventType, IdMinter,
    OutboxStore, OutboxTransaction, OutboxTx, PiiKeyRef, Visibility,
};
use myelin_identity::{
    CaveatContext, Consistency, ConsistencyMode, Decision, IdentityService, Permission,
    Precondition, Principal, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_storage::encryption::{EncryptedColumn, KeyChoiceError, SubjectId};
use myelin_storage::kms::KmsEngine;
use std::sync::Arc;

pub const PERM_MANAGE: &str = "manage";
pub const PERM_TRANSITION: &str = "transition";
pub const PERM_PERFORM_TRANSITION: &str = "perform_transition";
pub const PERM_COMMENT: &str = "comment";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueDraft {
    pub project_id: u128,
    pub title: String,
    pub props: Vec<u8>,
    pub reporter_pseudonym: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MutationKind {
    Create(IssueDraft),
    Update {
        delta: Vec<u8>,
    },
    Transition {
        from: String,
        to: String,
    },
    Assign {
        assignee_pseudonym: String,
    },
    Watch {
        watcher_pseudonym: String,
    },
    ConfidentialGrant {
        grantee_pseudonym: String,
    },
}

impl MutationKind {
    pub fn permission(&self) -> Permission {
        match self {
            MutationKind::Create(_)
            | MutationKind::Update { .. }
            | MutationKind::Assign { .. }
            | MutationKind::ConfidentialGrant { .. } => Permission(PERM_MANAGE.into()),
            MutationKind::Transition { .. } => Permission(PERM_PERFORM_TRANSITION.into()),
            MutationKind::Watch { .. } => Permission(PERM_COMMENT.into()),
        }
    }

    pub fn event_token(&self) -> Option<&'static str> {
        match self {
            MutationKind::Create(_) => Some(events::ISSUE_CREATED),
            MutationKind::Update { .. } => Some(events::ISSUE_UPDATED),
            MutationKind::Transition { .. } => Some(events::ISSUE_TRANSITIONED),
            MutationKind::Assign { .. } => Some(events::ISSUE_ASSIGNED),
            MutationKind::Watch { .. } | MutationKind::ConfidentialGrant { .. } => None,
        }
    }

    fn tuple_delta(&self, object: &myelin_identity::ObjectId) -> Option<TupleDelta> {
        let (rel, subject) = match self {
            MutationKind::Assign { assignee_pseudonym } => ("assignee", assignee_pseudonym),
            MutationKind::Watch { watcher_pseudonym } => ("watcher", watcher_pseudonym),
            MutationKind::ConfidentialGrant { grantee_pseudonym } => {
                ("confidential_grant", grantee_pseudonym)
            }
            _ => return None,
        };
        Some(TupleDelta::Add(RelationTuple {
            object: object.clone(),
            relation: RelName(rel.into()),
            subject: myelin_identity::PrincipalId(subject.clone()),
            caveat: None,
        }))
    }

    fn carries_personal_data(&self) -> bool {
        matches!(self, MutationKind::Create(_) | MutationKind::Update { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteError {
    Invalid(String),
    Denied { permission: String },
    Authz(String),
    Outbox(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Invalid(why) => write!(f, "invalid write-path input: {why}"),
            WriteError::Denied { permission } => write!(
                f,
                "write DENIED by Id.check on `{permission}` (fail-closed, ADR-03) - nothing written"
            ),
            WriteError::Authz(why) => write!(f, "authz surface error (write fail-closed): {why}"),
            WriteError::Outbox(why) => write!(f, "outbox co-commit failed: {why}"),
        }
    }
}

impl std::error::Error for WriteError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteOutcome {
    pub event_id: Option<myelin_events::EventId>,
    pub zookie: Option<Zookie>,
}

pub fn issue_aggregate_key(project_id: u128, issue_local_id: &str) -> AggregateKey {
    AggregateKey(format!("issue:{project_id}:{issue_local_id}"))
}

pub fn issue_ref(tenant: &str, issue_local_id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{tenant}/issue/issue/{issue_local_id}"))
}

#[allow(clippy::too_many_arguments)]
pub fn apply_mutation<Id: IdentityService>(
    store: &OutboxStore,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    id: &Id,
    actor: &Principal,
    issue_local_id: &str,
    mutation: &MutationKind,
    cause: Option<&myelin_events::EventEnvelope>,
) -> Result<WriteOutcome, WriteError> {
    apply_mutation_inner(
        store,
        minter,
        ctx_base,
        id,
        actor,
        issue_local_id,
        mutation,
        cause,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_mutation_inner<Id: IdentityService>(
    store: &OutboxStore,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    id: &Id,
    actor: &Principal,
    issue_local_id: &str,
    mutation: &MutationKind,
    cause: Option<&myelin_events::EventEnvelope>,
    real_pii_key_ref: Option<PiiKeyRef>,
) -> Result<WriteOutcome, WriteError> {
    validate(mutation)?;

    let tenant = ctx_base.tenant.0.clone();
    let object_ref = issue_ref(&tenant, issue_local_id);
    let object_id = myelin_identity::ObjectId(object_ref.0.clone());
    let permission = mutation.permission();

    let caveat = caveat_for(mutation, &object_ref);
    let at = strong_consistency(&ctx_base);
    match id.check(actor, &permission, &object_ref, &at, Some(&caveat)) {
        Ok(Decision::Allow) => {}
        Ok(Decision::Deny) | Ok(Decision::Conditional) => {
            return Err(WriteError::Denied {
                permission: permission.0,
            });
        }
        Err(e) => return Err(WriteError::Authz(format!("{e:?}"))),
    }

    let zookie = match mutation.tuple_delta(&object_id) {
        Some(delta) => {
            let precondition: Option<&Precondition> = None;
            match id.write_tuples(&[delta], precondition) {
                Ok(zk) => Some(zk),
                Err(e) => return Err(WriteError::Authz(format!("{e:?}"))),
            }
        }
        None => None,
    };

    let mut tx = store.begin(minter, ctx_base);
    tx.stage_state_change(state_change_description(mutation, issue_local_id));

    let event_id = match mutation.event_token() {
        Some(token) => {
            let draft = event_draft(
                token,
                &object_ref,
                project_of(mutation),
                issue_local_id,
                mutation,
                real_pii_key_ref.clone(),
            );
            match tx.emit(draft, cause) {
                Ok(eid) => Some(eid),
                Err(e) => return Err(WriteError::Outbox(format!("{e:?}"))),
            }
        }
        None => None,
    };

    commit_tx(tx)?;

    Ok(WriteOutcome { event_id, zookie })
}

fn validate(mutation: &MutationKind) -> Result<(), WriteError> {
    let nonempty = |label: &str, v: &str| -> Result<(), WriteError> {
        if v.trim().is_empty() {
            Err(WriteError::Invalid(format!("{label} must not be empty")))
        } else {
            Ok(())
        }
    };
    match mutation {
        MutationKind::Create(draft) => nonempty("reporter_pseudonym", &draft.reporter_pseudonym),
        MutationKind::Update { delta } => {
            if delta.is_empty() {
                Err(WriteError::Invalid("update delta must not be empty".into()))
            } else {
                Ok(())
            }
        }
        MutationKind::Transition { from, to } => {
            nonempty("transition.from", from)?;
            nonempty("transition.to", to)?;
            if from == to {
                return Err(WriteError::Invalid(
                    "transition.from must differ from transition.to".into(),
                ));
            }
            Ok(())
        }
        MutationKind::Assign { assignee_pseudonym } => nonempty("assignee", assignee_pseudonym),
        MutationKind::Watch { watcher_pseudonym } => nonempty("watcher", watcher_pseudonym),
        MutationKind::ConfidentialGrant { grantee_pseudonym } => {
            nonempty("confidential_grant", grantee_pseudonym)
        }
    }
}

fn caveat_for(mutation: &MutationKind, object: &ArtifactRef) -> CaveatContext {
    let mut attrs = std::collections::BTreeMap::new();
    let transition = match mutation {
        MutationKind::Transition { from, to } => {
            attrs.insert("from".into(), myelin_identity::Literal::Str(from.clone()));
            attrs.insert("to".into(), myelin_identity::Literal::Str(to.clone()));
            Some(myelin_identity::TransitionId(format!("{from}->{to}")))
        }
        _ => None,
    };
    CaveatContext {
        object: object.clone(),
        field: None,
        transition,
        attrs,
    }
}

fn strong_consistency(_ctx: &EmitContextBase) -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn project_of(mutation: &MutationKind) -> u128 {
    match mutation {
        MutationKind::Create(draft) => draft.project_id,
        _ => 0,
    }
}

fn event_draft(
    token: &str,
    object: &ArtifactRef,
    project_id: u128,
    issue_local_id: &str,
    mutation: &MutationKind,
    real_pii_key_ref: Option<PiiKeyRef>,
) -> EventDraft {
    let contains_pii = mutation.carries_personal_data();
    let mut payload = serde_json::json!({
        "issue": object.0,
        "issue_local_id": issue_local_id,
    });
    match mutation {
        MutationKind::Transition { from, to } => {
            payload["from"] = serde_json::Value::String(from.clone());
            payload["to"] = serde_json::Value::String(to.clone());
        }
        MutationKind::Assign { assignee_pseudonym } => {
            payload["assignee"] = serde_json::Value::String(assignee_pseudonym.clone());
        }
        _ => {}
    }
    EventDraft {
        type_: EventType(token.into()),
        subject: object.clone(),
        aggregate: issue_aggregate_key(project_id, issue_local_id),
        payload,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: contains_pii,
        pii_key_ref: if contains_pii {
            Some(
                real_pii_key_ref
                    .unwrap_or_else(|| PiiKeyRef(format!("issue-dek:{issue_local_id}"))),
            )
        } else {
            None
        },
    }
}

fn state_change_description(mutation: &MutationKind, issue_local_id: &str) -> String {
    match mutation {
        MutationKind::Create(_) => format!("issue {issue_local_id} created"),
        MutationKind::Update { .. } => format!("issue {issue_local_id} updated"),
        MutationKind::Transition { from, to } => {
            format!("issue {issue_local_id} transitioned {from} -> {to}")
        }
        MutationKind::Assign { assignee_pseudonym } => {
            format!("issue {issue_local_id} assigned to {assignee_pseudonym}")
        }
        MutationKind::Watch { watcher_pseudonym } => {
            format!("issue {issue_local_id} watched by {watcher_pseudonym}")
        }
        MutationKind::ConfidentialGrant { grantee_pseudonym } => {
            format!("issue {issue_local_id} confidential-grant to {grantee_pseudonym}")
        }
    }
}

fn commit_tx(tx: OutboxTransaction) -> Result<(), WriteError> {
    tx.commit()
        .map_err(|e| WriteError::Outbox(format!("{e:?}")))
}

#[derive(Clone, Debug)]
pub struct SealedCreate {
    pub reporter: IssuePseudonym,
    pub title: EncryptedColumn,
    pub props: EncryptedColumn,
}

impl SealedCreate {
    pub fn pii_key_ref(&self) -> PiiKeyRef {
        PiiKeyRef(self.title.key_ref.to_uri())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SealError {
    Pseudonym(PseudonymError),
    Dek(String),
}

impl core::fmt::Display for SealError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SealError::Pseudonym(e) => write!(f, "pseudonymise failed (write fails closed): {e}"),
            SealError::Dek(e) => write!(f, "per-subject-DEK seal failed (write fails closed): {e}"),
        }
    }
}

impl std::error::Error for SealError {}

impl From<KeyChoiceError> for SealError {
    fn from(e: KeyChoiceError) -> Self {
        SealError::Dek(format!("{e}"))
    }
}

#[allow(clippy::too_many_arguments)]
pub fn apply_mutation_sealed<Id: IdentityService>(
    store: &OutboxStore,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    id: &Id,
    engine: &KmsEngine,
    actor: &Principal,
    issue_local_id: &str,
    draft: &IssueDraft,
    cause: Option<&myelin_events::EventEnvelope>,
) -> Result<(WriteOutcome, SealedCreate), WriteError> {
    let tenant = ctx_base.tenant.clone();
    let region = ctx_base.region.clone();

    let reporter = pseudonym::pseudonymise(id, &actor.principal_id, &tenant)
        .map_err(|e| WriteError::Invalid(format!("{}", SealError::Pseudonym(e))))?;
    let subject = SubjectId::new(reporter.render());

    let title = dek::encrypt_free_text(
        engine,
        &region,
        &tenant,
        &subject,
        IssueFreeText::Title,
        draft.title.as_bytes(),
    )
    .map_err(|e| WriteError::Invalid(format!("{}", SealError::from(e))))?;
    let props = dek::encrypt_free_text(
        engine,
        &region,
        &tenant,
        &subject,
        IssueFreeText::Props,
        &draft.props,
    )
    .map_err(|e| WriteError::Invalid(format!("{}", SealError::from(e))))?;

    let sealed = SealedCreate {
        reporter,
        title,
        props,
    };

    let mutation = MutationKind::Create(draft.clone());
    let outcome = apply_mutation_inner(
        store,
        minter,
        ctx_base,
        id,
        actor,
        issue_local_id,
        &mutation,
        cause,
        Some(sealed.pii_key_ref()),
    )?;
    Ok((outcome, sealed))
}

#[allow(clippy::too_many_arguments)]
pub fn create_issue<Id: IdentityService, R: crate::keys::PrefixReserve>(
    store: &OutboxStore,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    id: &Id,
    engine: &KmsEngine,
    allocator: &crate::keys::HiLoKeyAllocator<R>,
    actor: &Principal,
    prefix: &str,
    draft: &IssueDraft,
    cause: Option<&myelin_events::EventEnvelope>,
) -> Result<(crate::keys::CanonicalKey, WriteOutcome, SealedCreate), WriteError> {
    let key = allocator
        .allocate(&ctx_base.tenant, prefix)
        .map_err(|e| WriteError::Outbox(format!("key allocation failed: {e}")))?;
    let issue_local_id = key.render();

    let (outcome, sealed) = apply_mutation_sealed(
        store,
        minter,
        ctx_base,
        id,
        engine,
        actor,
        &issue_local_id,
        draft,
        cause,
    )?;
    Ok((key, outcome, sealed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{HiLoKeyAllocator, InMemoryPrefixCounter};
    use myelin_events::{
        Actor, CausedBy, EventEnvelope, MonotonicMinter, Region, TenantId, Timestamp,
    };
    use myelin_identity::{
        AuthzError, Credential, EffectivePolicy, FragmentAdmit, ListObjectsResult,
        NamespaceFragment, ObjectId, ObjectType, PrincipalId, PrincipalKind, RewriteTrace, RunId,
        RunToken, SubjectTree,
    };
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type IdResult<T> = myelin_identity::Result<T>;

    struct StubId {
        allow: HashMap<String, Decision>,
        write_tuples_calls: AtomicUsize,
        check_calls: AtomicUsize,
    }
    impl StubId {
        fn new() -> Self {
            Self {
                allow: HashMap::new(),
                write_tuples_calls: AtomicUsize::new(0),
                check_calls: AtomicUsize::new(0),
            }
        }
        fn allowing(mut self, permission: &str, object: &ArtifactRef) -> Self {
            self.allow
                .insert(format!("{permission}@{}", object.0), Decision::Allow);
            self
        }
    }
    impl IdentityService for StubId {
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn check(
            &self,
            _s: &Principal,
            permission: &Permission,
            object: &ArtifactRef,
            at: &Consistency,
            _cav: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            self.check_calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(at.mode, ConsistencyMode::Strong, "write gate reads Strong");
            Ok(self
                .allow
                .get(&format!("{}@{}", permission.0, object.0))
                .copied()
                .unwrap_or(Decision::Deny))
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
        fn write_tuples(
            &self,
            deltas: &[TupleDelta],
            _p: Option<&Precondition>,
        ) -> IdResult<Zookie> {
            self.write_tuples_calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(deltas.len(), 1, "v1 mutations write exactly one tuple");
            Ok(Zookie("zk-issue-1".into()))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &RunId,
            _d: &myelin_identity::DelegationCaveats,
            _t: &myelin_identity::FailStaticBound,
        ) -> IdResult<RunToken> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn resolve_pseudonym(&self, s: &PrincipalId, t: &TenantId) -> IdResult<String> {
            Ok(
                myelin_identity::PseudonymHandle::new(format!("psn-{}", s.0), t.0.clone())
                    .expect("a valid pseudonym handle")
                    .render(),
            )
        }
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T10:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T10:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    fn actor() -> Principal {
        Principal::stub(
            PrincipalId("u-1".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }

    fn draft() -> IssueDraft {
        IssueDraft {
            project_id: 7,
            title: "fix the charge bug".into(),
            props: b"{\"severity\":3}".to_vec(),
            reporter_pseudonym: "psn:abc".into(),
        }
    }

    #[test]
    fn create_co_commits_event_and_state_in_one_tx() {
        let store = OutboxStore::new();
        let object = issue_ref("acme", "ENG-1");
        let id = StubId::new().allowing(PERM_MANAGE, &object);

        let out = apply_mutation(
            &store,
            minter(),
            ctx_base(),
            &id,
            &actor(),
            "ENG-1",
            &MutationKind::Create(draft()),
            None,
        )
        .expect("an allowed create commits");

        assert_eq!(store.outbox_depth(), 1, "one issue.* event co-committed");
        assert_eq!(store.committed_count(), 1);
        let eid = out.event_id.expect("create emits a lifecycle event");
        let row = store.row(&eid).expect("the committed row is present");
        assert_eq!(row.seq, 0, "first event for the issue aggregate is seq 0");
        assert_eq!(row.envelope.type_.0, events::ISSUE_CREATED);
        assert_eq!(row.aggregate, issue_aggregate_key(7, "ENG-1"));
        assert!(out.zookie.is_none(), "a create writes no relation tuple");
        assert_eq!(id.check_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn denied_write_emits_nothing_zero_ghost() {
        let store = OutboxStore::new();
        let id = StubId::new();

        let err = apply_mutation(
            &store,
            minter(),
            ctx_base(),
            &id,
            &actor(),
            "ENG-9",
            &MutationKind::Create(draft()),
            None,
        )
        .expect_err("a denied write fails");

        assert_eq!(
            err,
            WriteError::Denied {
                permission: PERM_MANAGE.into()
            }
        );
        assert_eq!(store.outbox_depth(), 0, "a denied write emits no event");
        assert_eq!(store.committed_count(), 0, "no ghost row from a denial");
        assert_eq!(id.write_tuples_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn invalid_input_writes_nothing_and_skips_the_gate() {
        let store = OutboxStore::new();
        let object = issue_ref("acme", "ENG-2");
        let id = StubId::new().allowing(PERM_MANAGE, &object);
        let mut bad = draft();
        bad.reporter_pseudonym = "  ".into();

        let err = apply_mutation(
            &store,
            minter(),
            ctx_base(),
            &id,
            &actor(),
            "ENG-2",
            &MutationKind::Create(bad),
            None,
        )
        .expect_err("a blank pseudonym is invalid");
        assert!(matches!(err, WriteError::Invalid(_)));
        assert_eq!(
            store.committed_count(),
            0,
            "invalid write committed nothing"
        );
        assert_eq!(
            id.check_calls.load(Ordering::SeqCst),
            0,
            "validation is BEFORE the gate"
        );
    }

    #[test]
    fn chained_create_update_transition_is_monotonic_and_dedup_safe() {
        let store = OutboxStore::new();
        let object = issue_ref("acme", "ENG-1");
        let id = StubId::new()
            .allowing(PERM_MANAGE, &object)
            .allowing(PERM_PERFORM_TRANSITION, &object);
        let m = minter();

        let create = apply_mutation(
            &store,
            Arc::clone(&m),
            ctx_base(),
            &id,
            &actor(),
            "ENG-1",
            &MutationKind::Create(draft()),
            None,
        )
        .expect("create commits");
        let update = apply_mutation(
            &store,
            Arc::clone(&m),
            ctx_base(),
            &id,
            &actor(),
            "ENG-1",
            &MutationKind::Update {
                delta: b"priority: 2 -> 1".to_vec(),
            },
            None,
        )
        .expect("update commits");
        let transition = apply_mutation(
            &store,
            Arc::clone(&m),
            ctx_base(),
            &id,
            &actor(),
            "ENG-1",
            &MutationKind::Transition {
                from: "todo".into(),
                to: "in_progress".into(),
            },
            None,
        )
        .expect("transition commits");

        let agg = issue_aggregate_key(7, "ENG-1");
        let agg_for_noncreate = issue_aggregate_key(0, "ENG-1");
        let create_row = store.row(&create.event_id.unwrap()).unwrap();
        let update_row = store.row(&update.event_id.unwrap()).unwrap();
        let transition_row = store.row(&transition.event_id.unwrap()).unwrap();
        assert_eq!(create_row.aggregate, agg);
        assert_eq!(create_row.seq, 0, "create is seq 0 on its aggregate");
        assert_eq!(update_row.aggregate, agg_for_noncreate);
        assert_eq!(transition_row.aggregate, agg_for_noncreate);
        assert_eq!(update_row.seq, 0);
        assert_eq!(
            transition_row.seq, 1,
            "transition follows update in commit order"
        );

        assert_eq!(create_row.envelope.type_.0, events::ISSUE_CREATED);
        assert_eq!(update_row.envelope.type_.0, events::ISSUE_UPDATED);
        assert_eq!(transition_row.envelope.type_.0, events::ISSUE_TRANSITIONED);
        assert_eq!(store.committed_count(), 3);

        let ids = [
            create_row.event_id.clone(),
            update_row.event_id.clone(),
            transition_row.event_id.clone(),
        ];
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        assert_eq!(
            unique.len(),
            3,
            "every emitted event carries a distinct stable id"
        );
    }

    #[test]
    fn assign_writes_the_tuple_returns_zookie_and_emits() {
        let store = OutboxStore::new();
        let object = issue_ref("acme", "ENG-3");
        let id = StubId::new().allowing(PERM_MANAGE, &object);

        let out = apply_mutation(
            &store,
            minter(),
            ctx_base(),
            &id,
            &actor(),
            "ENG-3",
            &MutationKind::Assign {
                assignee_pseudonym: "psn:dev".into(),
            },
            None,
        )
        .expect("assign commits");

        assert_eq!(
            id.write_tuples_calls.load(Ordering::SeqCst),
            1,
            "assign drives exactly one write_tuples (4.6)"
        );
        assert_eq!(
            out.zookie,
            Some(Zookie("zk-issue-1".into())),
            "the write_tuples zookie is returned for read-your-writes (4.10)"
        );
        let row = store.row(&out.event_id.unwrap()).unwrap();
        assert_eq!(row.envelope.type_.0, events::ISSUE_ASSIGNED);
        assert_eq!(store.outbox_depth(), 1, "issue.assigned co-committed");
    }

    #[test]
    fn watch_and_grant_write_tuples_but_emit_no_lifecycle_event() {
        for mutation in [
            MutationKind::Watch {
                watcher_pseudonym: "psn:w".into(),
            },
            MutationKind::ConfidentialGrant {
                grantee_pseudonym: "psn:g".into(),
            },
        ] {
            let store = OutboxStore::new();
            let object = issue_ref("acme", "ENG-4");
            let perm = mutation.permission();
            let id = StubId::new().allowing(&perm.0, &object);

            let out = apply_mutation(
                &store,
                minter(),
                ctx_base(),
                &id,
                &actor(),
                "ENG-4",
                &mutation,
                None,
            )
            .expect("relation change commits");

            assert_eq!(
                id.write_tuples_calls.load(Ordering::SeqCst),
                1,
                "a relation change drives write_tuples"
            );
            assert!(out.zookie.is_some(), "the zookie is returned (4.10)");
            assert!(
                out.event_id.is_none(),
                "a pure relation change emits no lifecycle event"
            );
            assert_eq!(
                store.outbox_depth(),
                0,
                "no lifecycle event co-committed for a pure relation change"
            );
        }
    }

    #[test]
    fn pii_bearing_event_flags_and_key_refs_but_carries_no_inline_body() {
        let store = OutboxStore::new();
        let object = issue_ref("acme", "ENG-5");
        let id = StubId::new().allowing(PERM_MANAGE, &object);
        let d = draft();

        let out = apply_mutation(
            &store,
            minter(),
            ctx_base(),
            &id,
            &actor(),
            "ENG-5",
            &MutationKind::Create(d.clone()),
            None,
        )
        .expect("create commits");
        let row = store.row(&out.event_id.unwrap()).unwrap();
        assert!(
            row.envelope.contains_personal_data,
            "a create with free-text carries the PII flag"
        );
        assert!(
            row.envelope.pii_key_ref.is_some(),
            "a PII-bearing event carries a key ref (per-subject DEK - ISS-P07 floor)"
        );
        let payload_str = serde_json::to_string(&row.envelope.payload).unwrap();
        assert!(
            !payload_str.contains(&d.title),
            "the inline title body must NOT be on the wire (references-not-payloads)"
        );
    }

    #[test]
    fn caused_mutation_inherits_correlation_and_depth() {
        let store = OutboxStore::new();
        let object = issue_ref("acme", "ENG-6");
        let id = StubId::new().allowing(PERM_MANAGE, &object);

        let parent = parent_envelope();
        let out = apply_mutation(
            &store,
            minter(),
            ctx_base(),
            &id,
            &actor(),
            "ENG-6",
            &MutationKind::Create(draft()),
            Some(&parent),
        )
        .expect("a caused create commits");
        let row = store.row(&out.event_id.unwrap()).unwrap();
        assert_eq!(
            row.envelope.depth,
            parent.depth + 1,
            "child is depth parent+1"
        );
        assert_eq!(
            row.envelope.correlation_id, parent.correlation_id,
            "the correlation root carries"
        );
        assert_eq!(
            row.envelope.causation_id.as_ref(),
            Some(&parent.event_id),
            "causation = the parent event"
        );
    }

    fn parent_envelope() -> EventEnvelope {
        let store = OutboxStore::new();
        let mut tx = store.begin(minter(), ctx_base());
        tx.emit(
            EventDraft {
                type_: EventType("chat.message.created".into()),
                subject: ArtifactRef("myelin://acme/chat/message/m-1".into()),
                aggregate: AggregateKey("chat:m-1".into()),
                payload: serde_json::json!({}),
                data_role: DataRole::Controller,
                visibility: Visibility::Internal,
                contains_personal_data: false,
                pii_key_ref: None,
            },
            None,
        )
        .unwrap();
        tx.commit().unwrap();
        store.committed_rows()[0].envelope.clone()
    }

    #[test]
    fn create_issue_mints_canonical_key_and_co_commits() {
        let store = OutboxStore::new();
        let engine = KmsEngine::new();
        let allocator = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
        let m = minter();

        let id = StubId::new()
            .allowing(PERM_MANAGE, &issue_ref("acme", "ENG-1"))
            .allowing(PERM_MANAGE, &issue_ref("acme", "ENG-2"));

        let (key, out, sealed) = create_issue(
            &store,
            Arc::clone(&m),
            ctx_base(),
            &id,
            &engine,
            &allocator,
            &actor(),
            "ENG",
            &draft(),
            None,
        )
        .expect("create_issue commits");

        assert_eq!(key.render(), "ENG-1");
        assert_eq!(key.render_display_key(), "#1", "the #form is display-only");
        let row = store.row(&out.event_id.unwrap()).unwrap();
        assert_eq!(row.envelope.type_.0, events::ISSUE_CREATED);
        assert_eq!(row.aggregate, issue_aggregate_key(7, "ENG-1"));
        assert_eq!(row.subject.0, "myelin://acme/issue/issue/ENG-1");
        assert!(row.envelope.contains_personal_data);
        assert_eq!(
            row.envelope.pii_key_ref.as_ref().map(|r| r.0.clone()),
            Some(sealed.pii_key_ref().0),
            "the create carries the per-subject-DEK key ref"
        );

        let (key2, _, _) = create_issue(
            &store,
            Arc::clone(&m),
            ctx_base(),
            &id,
            &engine,
            &allocator,
            &actor(),
            "ENG",
            &draft(),
            None,
        )
        .expect("second create commits");
        assert_eq!(key2.render(), "ENG-2", "monotonic per prefix");
    }

    #[test]
    fn create_issue_fails_closed_on_reserve_error() {
        struct FailReserve;
        impl crate::keys::PrefixReserve for FailReserve {
            fn reserve(
                &self,
                _t: &TenantId,
                _p: &str,
                _b: u32,
            ) -> Result<crate::keys::ReservedBlock, crate::keys::ReserveError> {
                Err(crate::keys::ReserveError::Backend(
                    "counter unavailable".into(),
                ))
            }
        }
        let store = OutboxStore::new();
        let engine = KmsEngine::new();
        let allocator = HiLoKeyAllocator::new(FailReserve);
        let id = StubId::new().allowing(PERM_MANAGE, &issue_ref("acme", "ENG-1"));

        let err = create_issue(
            &store,
            minter(),
            ctx_base(),
            &id,
            &engine,
            &allocator,
            &actor(),
            "ENG",
            &draft(),
            None,
        )
        .expect_err("a reserve failure fails the create closed");
        assert!(matches!(err, WriteError::Outbox(_)));
        assert_eq!(
            store.committed_count(),
            0,
            "nothing written when the key cannot be minted"
        );
    }

}
