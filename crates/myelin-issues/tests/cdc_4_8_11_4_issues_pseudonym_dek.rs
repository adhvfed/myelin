//! # CDC pair — contract 4.8 (pseudonym grammar) + 11.4 (per-subject-DEK columns) for Issues
//! (ISS-P07 / P-373, M4-I1)
//!
//! **The two halves this artifact proves (the prompt's GATE):**
//! - **4.8 — pseudonymous-by-default identity columns.** PROVIDER: the Issues write path
//!   ([`myelin_issues::apply_mutation_sealed`]) pseudonymises the reporter through the ONE Identity
//!   person↔pseudonym map ([`IdentityService::resolve_pseudonym`]). CONSUMER: a read-side that parses
//!   the stored `reporter` column back through the FROZEN `<pseudonym>@<tenant>.noreply` grammar
//!   ([`myelin_identity::PseudonymHandle`]) — proving the column carries an opaque pseudonym, NOT a
//!   raw id (recon §X-7, the 0-raw-id assertion).
//! - **11.4 — per-subject-DEK free-text columns.** PROVIDER: the write path seals `title` / `props`
//!   under the SUBJECT's per-subject DEK ([`myelin_storage::encryption::ColumnCryptor`]) and threads
//!   the REAL `kms://<tenant>/<epoch>/subject:<id>` `pii_key_ref` onto the emitted `issue.created`
//!   event. CONSUMER: a read-side that decrypts the sealed column with the named DEK while the key
//!   lives — and asserts 0 plaintext free-text at rest (the GATE artifact).
//!
//! The provider + consumer are the SAME frozen shapes (one grammar, one cryptor — EI-01 §7), proven
//! against the in-memory `KmsEngine` (DB-free). The live-Postgres at-rest round-trip is the
//! `integration`-feature artifact (tests/integration_iss_p07_subject_dek.rs).

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

/// The CDC stub Identity: resolves a fixed person↔pseudonym map (the S2 map's behaviour, 4.8) and
/// allows `manage` on the issue object. The REAL map + engine are the Identity service (EI-01 §7).
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
        // the raw reporter pseudonym the draft carries is replaced by the resolved one at the column.
        reporter_pseudonym: "u-42".into(),
    }
}

/// **CDC 4.8 — the reporter identity column is a PSEUDONYM (provider) that parses back through the
/// frozen grammar (consumer) — 0 raw id.** The write path resolves `u-42` → `8a2f@acme.noreply` via
/// the ONE Identity map; the stored `reporter` column is parsed back through
/// [`PseudonymHandle::parse`] (the consumer) to the same `(token, tenant)` — proving the column holds
/// an opaque pseudonym, never a raw id (recon §X-7).
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

    // PROVIDER: the stored reporter column is the resolved pseudonym.
    let stored = sealed.reporter.render();
    assert_eq!(stored, "8a2f@acme.noreply");
    // CONSUMER: the column parses back through the FROZEN grammar to the same (token, tenant).
    let parsed = PseudonymHandle::parse(&stored).expect("the column is a well-formed pseudonym");
    assert_eq!(parsed.pseudonym(), "8a2f");
    assert_eq!(parsed.tenant(), "acme");
    // 0-raw-id (recon §X-7): the stored identity column is NOT a raw id.
    assert!(
        !is_raw_principal_id(&stored),
        "the reporter column holds a pseudonym, never a raw id"
    );
    // the raw principal id MUST NOT leak into the stored column.
    assert_ne!(stored, "u-42", "the raw principal id is never stored");
}

/// **CDC 11.4 — the free-text columns are sealed under the per-subject DEK (provider) and decrypt
/// back while the key lives (consumer) — 0 plaintext at rest.** The write path seals `title`/`props`
/// under the subject's per-subject DEK and emits the REAL `kms://…/subject:<id>` key ref; the consumer
/// decrypts with the named DEK and asserts the ciphertext never held the plaintext.
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

    // PROVIDER: the title/props columns are sealed under the per-subject DEK (the GD-4 lever).
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

    // 0 plaintext free-text at rest: neither column holds the plaintext byte-run.
    assert!(
        !sealed.title.contains_plaintext(d.title.as_bytes()),
        "0 plaintext title at rest"
    );
    assert!(
        !sealed.props.contains_plaintext(&d.props),
        "0 plaintext props at rest"
    );

    // CONSUMER: decrypt back with the named DEK while the key lives → exact plaintext.
    let region = Region::new("fr-par");
    let title_pt = decrypt_free_text(&engine, &region, &sealed.title).expect("title decrypts");
    assert_eq!(title_pt, d.title.as_bytes());
    let props_pt = decrypt_free_text(&engine, &region, &sealed.props).expect("props decrypts");
    assert_eq!(props_pt, d.props);

    // the emitted issue.created event carries the REAL per-subject-DEK pii_key_ref (not a placeholder),
    // and NEVER the inline body (references-not-payloads).
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

/// **The write FAILS CLOSED when the reporter cannot be pseudonymised — never a stored raw id.** An
/// unresolvable subject is a loud error; nothing is written (0 ghost), no raw id is persisted.
#[test]
fn an_unresolvable_reporter_fails_the_write_closed_no_raw_id() {
    let store = OutboxStore::new();
    let engine = KmsEngine::new();
    let object = IdArtifactRef("myelin://acme/issue/issue/ENG-3".into());
    // NO pseudonym map entry for u-42 → fail closed.
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
    // the message names the fail-closed pseudonymise failure.
    assert!(
        format!("{err}").contains("pseudonymise"),
        "the failure is the pseudonymise fail-closed, got: {err}"
    );
    // 0 ghost: nothing committed.
    assert_eq!(
        store.committed_count(),
        0,
        "a failed-closed write writes nothing"
    );
}
