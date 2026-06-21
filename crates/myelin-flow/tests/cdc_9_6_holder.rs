//! # The CDC pair for the workflow-history `PersonalDataHolder` — contract 9.6 (the STRUCTURAL half)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 9.6
//! (`PersonalDataHolder` (workflow history) + `replay` — references-not-payloads; inline-PII rows
//! per-subject crypto-shred). Owning architecture: `durable-workflow.md` §5.5 (the holder over
//! `workflow_run`/`wf_history`/`wf_signal`) + §4.8 (the references-not-payloads + crypto-shred +
//! tombstone erasure triad).
//!
//! ## What this pair pins (the PROVIDER ↔ CONSUMER agreement of 9.6's structural half)
//!
//! **9.6 PROVIDER (the flow [`WfHistoryHolder`]) — the agreement the workflow engine guarantees:**
//! - the flow OLTP store auto-registers as a holder (1.4 / 10.1) and classifies to H8 (the §5.5
//!   references-not-payloads reconcile — 0 orphan);
//! - `locate` returns the subject's `wf_history` appearances scoped to (subject, tenant), counting
//!   referenced-actor result refs AND the inline-PII `result_key_ref` — references-not-payloads, no
//!   stored name;
//! - `export` emits a PII-free reference bundle (the appearance count + a content-address, no
//!   free-text body);
//! - `erase` is the STRUCTURAL references-not-payloads erase: the refs-stored rows tombstone for free
//!   (0 PII columns mutated; the per-subject-DEK crypto-shred reach is the NAMED P-FLOW-23 floor).
//!
//! **9.6 CONSUMER (a DSR orchestrator, contract 10.4) — what it relies on:**
//! - it fans out a rights request to the flow holder through the ONE `PersonalDataHolder` trait
//!   (10.1) — `locate`/`export`/`erase` — and receives content-addressed receipts it hash-links into
//!   the audit log; the bundle is references-not-payloads (no inline PII leak).
//!
//! This pins the provider's promise NOW (the holder structural half); the `replay` LEG of 9.6 lands
//! P-FLOW-05. The pair proves a DSR orchestrator can locate + export a subject's workflow history
//! through the frozen holder trait, not a private channel.

use myelin_flow::{
    flow_history_holder, register_flow_holder, WfHistoryHolder, WfJournal, FLOW_OLTP_STORE,
};
use myelin_flow::schema::WfHistoryRow;
use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId as GdprTenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_substrate::{assert_holder_completeness, Holder, StoreKind};
use myelin_tenancy::{Region, TenantId};

fn tenant_gdpr() -> GdprTenantId {
    GdprTenantId::from_token("acme")
}

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        GdprTenantId::from_token("acme"),
    ))
}

/// A `wf_history` row naming `actor` by ref in a `result` ArtifactRef.
fn row(run_id: &str, seq: i64, actor: &str) -> WfHistoryRow {
    WfHistoryRow {
        tenant: tenant(),
        region: Region::new("fr-par"),
        run_id: run_id.into(),
        seq,
        kind: "activity_completed".into(),
        command_id: format!("agent.run:{seq}"),
        result: Some(vec![ArtifactRef(format!(
            "myelin://acme/identity/principal/{actor}"
        ))]),
        result_key_ref: None,
    }
}

/// **9.6 PROVIDER: the flow store auto-registers as a holder + classifies to H8 (1.4 / §5.5).** The
/// store is opened through the one door (registered by construction) and is in the exhaustive H1–H18
/// list — the holder-completeness assertion is green (0 orphan; the DSAR fan-out cannot miss workflow
/// history).
#[test]
fn provider_flow_store_registers_and_classifies_h8() {
    let registry = register_flow_holder();
    assert!(registry.is_registered(StoreKind::Oltp, FLOW_OLTP_STORE));
    assert_eq!(flow_history_holder(), Some(Holder::H8EventBus));
    assert_eq!(
        assert_holder_completeness(
            registry.registrations(),
            &myelin_flow::flow_store_classifier()
        ),
        Ok(()),
        "the flow store is in the exhaustive H1–H18 list — 0 orphan"
    );
}

/// **9.6 CONSUMER: a DSR orchestrator locates + exports a subject's workflow history through the ONE
/// trait (10.4 → 10.1).** The orchestrator holds the flow holder behind `dyn PersonalDataHolder`,
/// calls `locate`/`export`, and gets content-addressed, references-not-payloads receipts it would
/// hash-link into the audit log. Over a POPULATED journal it locates the real appearances; over an
/// EMPTY one it is empty-but-correct.
#[test]
fn consumer_dsr_orchestrator_locates_and_exports_via_the_trait() {
    // The provider: the flow holder over a populated journal (two rows name the subject).
    let journal = WfJournal::new();
    journal.append_history_for_test(row("run-1", 0, "u-subject"));
    journal.append_history_for_test(row("run-2", 0, "u-subject"));
    journal.append_history_for_test(row("run-3", 0, "u-other")); // does not name the subject.
    let holder = WfHistoryHolder::with_journal(journal);

    // The consumer: a DSR orchestrator fans out through the trait object (a heterogeneous holder set).
    let holders: Vec<Box<dyn PersonalDataHolder>> = vec![Box::new(holder)];
    let subj = subject("u-subject");
    for h in &holders {
        // locate: a content-addressed, references-not-payloads receipt (no key shredded).
        let loc = h.locate(&subj, tenant_gdpr()).expect("locate succeeds");
        assert_eq!(loc.receipt.operation, "locate");
        assert!(loc.receipt.content_hash.starts_with("blake3:"));
        assert!(loc.receipt.key_epoch_destroyed.is_none(), "locate shreds no key");
        // export: a PII-free reference bundle.
        let exp = h.export(&subj, tenant_gdpr()).expect("export succeeds");
        assert_eq!(exp.receipt.operation, "export");
        assert!(exp.receipt.content_hash.starts_with("blake3:"));
    }

    // Empty-but-correct over an unbacked holder (the registration-only `serve`-before-journal form).
    let empty: Box<dyn PersonalDataHolder> = Box::new(WfHistoryHolder::default());
    assert!(empty.locate(&subj, tenant_gdpr()).is_ok(), "unbacked locate is empty-but-correct");
    assert!(empty.export(&subj, tenant_gdpr()).is_ok(), "unbacked export is empty-but-correct");
}

/// **9.6 CONSUMER: a DSR orchestrator erases a subject (the STRUCTURAL references-not-payloads
/// erase).** The orchestrator calls `erase`; the refs-stored journal rows tombstone for free (0 PII
/// columns mutated), and the receipt records NO key destroyed at the flow surface (the per-subject-DEK
/// crypto-shred reach is the NAMED P-FLOW-23 floor). Idempotent: re-erase returns the identical
/// receipt.
#[test]
fn consumer_dsr_orchestrator_erases_structurally() {
    let journal = WfJournal::new();
    journal.append_history_for_test(row("run-1", 0, "u-erase"));
    journal.append_history_for_test(row("run-2", 0, "u-keep"));
    let holder = WfHistoryHolder::with_journal(journal.clone());

    let before = journal.history_in_tenant(&tenant());
    let scope = EraseScope::Subject { subject: subject("u-erase"), tenant: tenant_gdpr() };
    let er = holder.erase(scope.clone()).expect("structural erase succeeds");
    assert!(
        er.receipt.key_epoch_destroyed.is_none(),
        "0 keys shredded at the flow surface (refs-stored; inline-PII DEK shred is P-FLOW-23)"
    );

    // 0 PII columns mutated — the refs-stored rows are byte-identical (tombstone for free).
    let after = journal.history_in_tenant(&tenant());
    assert_eq!(after, before, "references-not-payloads: 0 PII columns mutated on erase");

    // Idempotent.
    let er2 = holder.erase(scope).expect("re-erase is idempotent");
    assert_eq!(er, er2, "the same erase scope yields the identical content-addressed receipt");
}
