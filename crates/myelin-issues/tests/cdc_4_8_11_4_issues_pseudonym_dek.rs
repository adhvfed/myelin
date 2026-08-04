use myelin_events::{Actor, CausedBy, EmitContextBase, MonotonicMinter, OutboxStore, Timestamp};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, PseudonymHandle, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree,
    TupleDelta, Zookie,
};
use myelin_issues::{
    apply_mutation_sealed, decrypt_free_text, is_raw_principal_id, IssueDraft, PERM_MANAGE,
};
use myelin_storage::kms::KmsEngine;
use myelin_tenancy::ArtifactRef as IdArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::collections::HashMap;
use std::sync::Arc;

type IdResult<T> = myelin_identity::Result<T>;

struct CdcId {
    pseudonyms: HashMap<String, String>,
    allow: HashMap<String, Decision>,
}
impl CdcId {
    fn new() -> Self {
        Self {
            pseudonyms: HashMap::new(),
            allow: HashMap::new(),
        }
    }
    fn with_pseudonym(mut self, subject: &str, pseudonym: &str) -> Self {
        self.pseudonyms.insert(subject.into(), pseudonym.into());
        self
    }
    fn allowing(mut self, permission: &str, object: &IdArtifactRef) -> Self {
        self.allow
            .insert(format!("{permission}@{}", object.0), Decision::Allow);
        self
    }
}
impl IdentityService for CdcId {
    fn resolve_pseudonym(&self, subject: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        self.pseudonyms
            .get(&subject.0)
            .cloned()
            .ok_or(AuthzError::NotYetImplemented("no map entry"))
    }
    fn check(
        &self,
        _s: &Principal,
        permission: &Permission,
        object: &IdArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        Ok(self
            .allow
            .get(&format!("{}@{}", permission.0, object.0))
            .copied()
            .unwrap_or(Decision::Deny))
    }
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
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

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("u-42".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-23T10:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T10:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn actor() -> Principal {
    Principal::stub(
        PrincipalId("u-42".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn draft() -> IssueDraft {
    IssueDraft {
        project_id: 7,
        title: "fix the login bug for Ada Lovelace".into(),
        props: b"{\"customer\":\"ada@example.com\"}".to_vec(),
        reporter_pseudonym: "u-42".into(),
    }
}

#[test]
fn cdc_4_8_reporter_column_is_pseudonymous_and_parses_back_zero_raw_id() {
    let store = OutboxStore::new();
    let engine = KmsEngine::new();
    let object = IdArtifactRef("myelin://acme/issue/issue/ENG-1".into());
    let id = CdcId::new()
        .with_pseudonym("u-42", "8a2f@acme.noreply")
        .allowing(PERM_MANAGE, &object);

    let (_, sealed) = apply_mutation_sealed(
        &store,
        Arc::new(MonotonicMinter::new()),
        ctx_base(),
        &id,
        &engine,
        &actor(),
        "ENG-1",
        &draft(),
        None,
    )
    .expect("a sealed create commits");

    let stored = sealed.reporter.render();
    assert_eq!(stored, "8a2f@acme.noreply");
    let parsed = PseudonymHandle::parse(&stored).expect("the column is a well-formed pseudonym");
    assert_eq!(parsed.pseudonym(), "8a2f");
    assert_eq!(parsed.tenant(), "acme");
    assert!(
        !is_raw_principal_id(&stored),
        "the reporter column holds a pseudonym, never a raw id"
    );
    assert_ne!(stored, "u-42", "the raw principal id is never stored");
}

#[test]
fn cdc_11_4_free_text_is_subject_dek_sealed_and_decrypts_zero_plaintext_at_rest() {
    let store = OutboxStore::new();
    let engine = KmsEngine::new();
    let object = IdArtifactRef("myelin://acme/issue/issue/ENG-2".into());
    let id = CdcId::new()
        .with_pseudonym("u-42", "8a2f@acme.noreply")
        .allowing(PERM_MANAGE, &object);
    let d = draft();

    let (outcome, sealed) = apply_mutation_sealed(
        &store,
        Arc::new(MonotonicMinter::new()),
        ctx_base(),
        &id,
        &engine,
        &actor(),
        "ENG-2",
        &d,
        None,
    )
    .expect("a sealed create commits");

    assert!(
        sealed
            .title
            .key_ref
            .class
            .as_token()
            .starts_with("subject:"),
        "title is keyed under the per-subject DEK"
    );
    assert!(
        sealed
            .props
            .key_ref
            .class
            .as_token()
            .starts_with("subject:"),
        "props is keyed under the per-subject DEK"
    );

    assert!(
        !sealed.title.contains_plaintext(d.title.as_bytes()),
        "0 plaintext title at rest"
    );
    assert!(
        !sealed.props.contains_plaintext(&d.props),
        "0 plaintext props at rest"
    );

    let region = Region::new("fr-par");
    let title_pt = decrypt_free_text(&engine, &region, &sealed.title).expect("title decrypts");
    assert_eq!(title_pt, d.title.as_bytes());
    let props_pt = decrypt_free_text(&engine, &region, &sealed.props).expect("props decrypts");
    assert_eq!(props_pt, d.props);

    let eid = outcome.event_id.expect("create emits a lifecycle event");
    let row = store.row(&eid).expect("the committed row");
    let key_ref = row
        .envelope
        .pii_key_ref
        .as_ref()
        .expect("a PII-bearing event carries a key ref");
    assert!(
        key_ref.0.starts_with("kms://acme/") && key_ref.0.contains("/subject:"),
        "the event carries the REAL per-subject-DEK key ref, got {}",
        key_ref.0
    );
    let payload = serde_json::to_string(&row.envelope.payload).unwrap();
    assert!(
        !payload.contains("Ada Lovelace") && !payload.contains("ada@example.com"),
        "the inline free-text PII body is NEVER on the wire (references-not-payloads)"
    );
}

#[test]
fn an_unresolvable_reporter_fails_the_write_closed_no_raw_id() {
    let store = OutboxStore::new();
    let engine = KmsEngine::new();
    let object = IdArtifactRef("myelin://acme/issue/issue/ENG-3".into());
    let id = CdcId::new().allowing(PERM_MANAGE, &object);

    let err = apply_mutation_sealed(
        &store,
        Arc::new(MonotonicMinter::new()),
        ctx_base(),
        &id,
        &engine,
        &actor(),
        "ENG-3",
        &draft(),
        None,
    )
    .expect_err("an unresolvable reporter fails the write closed");
    assert!(
        format!("{err}").contains("pseudonymise"),
        "the failure is the pseudonymise fail-closed, got: {err}"
    );
    assert_eq!(
        store.committed_count(),
        0,
        "a failed-closed write writes nothing"
    );
}
