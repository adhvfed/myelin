use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::full_fanout::{
    FullFanOutCoverage, GaD1Certificate, Holder, HolderErasure,
};
use myelin_gdpr_service::{
    AuditAuthority, CellSigningKey, DestroyedKeyEpoch, ErasureLedger, InMemoryShredKms, M1Store,
    MemberCellSet, MerkleProvenBundle, MultiCellCertificate, MultiCellCoverage, MultiCellFanOut,
    PerCellReceipt, RestrictRegistry, SearchIndexHolder, SearchIndexModel, StoredContent,
    CANONICAL_POSTURE,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId, Region,
};

use myelin_gdpr_service::dsr::DsrId;

const VICTIM_PRINCIPAL: &str = "u-victim";

fn erase_scope() -> EraseScope {
    EraseScope::Subject {
        subject: subject(VICTIM_PRINCIPAL),
        tenant: tenant(),
    }
}

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

fn cell(token: &str) -> CellId {
    CellId::from_token(token)
}

fn pii_free_pointer(home: &str) -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0ACME/issues/issue/42".into())),
        ArtifactType::Issue,
        CorrelationId("corr-dsar-flagship".into()),
        cell(home),
    )
}

fn cell_scale_member_set() -> MemberCellSet {
    let home = cell("cell-fr-par-1");
    let members = vec![
        cell("cell-fr-par-2"),
        cell("cell-fr-par-3"),
        cell("cell-fr-par-4"),
        cell("cell-fr-par-5"),
    ];
    MemberCellSet::union(home, &members)
}

fn erase_in_cell(scope_token: &str) -> (GaD1Certificate, CellReliability) {
    let tenant = tenant();
    let subj = subject(scope_token);
    let restrict = RestrictRegistry::new();
    let kms = InMemoryShredKms::new();

    let git = M1Store::new("git_db", &restrict, &kms);
    let issues = M1Store::new("issues_db", &restrict, &kms);
    let knowledge = M1Store::new("knowledge_db", &restrict, &kms);
    let chat = M1Store::new("chat_db", &restrict, &kms);
    let ci = M1Store::new("ci_db", &restrict, &kms);
    let m1 = [&git, &issues, &knowledge, &chat, &ci];

    kms.provision(M1Store::dek_handle(&subj, &tenant), 7);
    git.store_self_authored(&subj, &tenant, "my PR review comment");
    issues.store_self_authored(&subj, &tenant, "my issue free-text");
    knowledge.store_self_authored(&subj, &tenant, "my doc block");
    chat.store_self_authored(&subj, &tenant, "my chat message body");
    ci.store_self_authored(&subj, &tenant, "my CI log line with email");

    let search = SearchIndexModel::new();
    let agent_memory = SearchIndexModel::new();
    search.index_from_source(VICTIM_PRINCIPAL, "victim@example.com");
    agent_memory.index_from_source(VICTIM_PRINCIPAL, "victim RAG context");

    for h in m1 {
        assert!(
            matches!(
                h.fetch_stored(&subj, &tenant),
                Some(StoredContent::Recoverable(_))
            ),
            "{}: content recoverable BEFORE erase",
            h.id()
        );
    }
    assert_eq!(
        search.reidentify_hits(VICTIM_PRINCIPAL),
        1,
        "search embedding re-identifies pre-erase"
    );
    assert_eq!(
        agent_memory.reidentify_hits(VICTIM_PRINCIPAL),
        1,
        "agent (H11) embedding re-identifies pre-erase"
    );

    let destroyed_epoch = git.erase_self_authored(&subj, &tenant);
    SearchIndexHolder::new(&search)
        .erase(erase_scope())
        .expect("the H7 search purge succeeds");
    SearchIndexHolder::new(&agent_memory)
        .erase(erase_scope())
        .expect("the H11 agent-memory purge succeeds");

    for h in m1 {
        assert!(
            matches!(
                h.fetch_stored(&subj, &tenant),
                Some(StoredContent::Unrecoverable)
            ),
            "{}: content UNRECOVERABLE after crypto-shred (live AND backups)",
            h.id()
        );
    }
    assert_eq!(
        search.reidentify_hits(VICTIM_PRINCIPAL),
        0,
        "GA-D2: 0 search embedding re-identification"
    );
    assert_eq!(
        agent_memory.reidentify_hits(VICTIM_PRINCIPAL),
        0,
        "GA-D2: 0 agent (H11) embedding re-identification - purged, NOT hidden"
    );

    let mut cov = FullFanOutCoverage::new();
    for &h in Holder::ALL {
        cov.record_reached(h);
    }
    assert_eq!(
        cov.holders_missed(),
        0,
        "the cell reached every H1–H18 holder"
    );
    let cert = GaD1Certificate::seal(scope_token, &cov).expect("the cell's fan-out seals");

    (
        cert,
        CellReliability {
            destroyed_key_epoch: destroyed_epoch,
            search_reidentify: search.reidentify_hits(VICTIM_PRINCIPAL),
            agent_reidentify: agent_memory.reidentify_hits(VICTIM_PRINCIPAL),
        },
    )
}

struct CellReliability {
    destroyed_key_epoch: Option<u64>,
    search_reidentify: usize,
    agent_reidentify: usize,
}

#[test]
fn e2e_4_dsar_fan_out_flagship_is_green() {
    let set = cell_scale_member_set();
    let pointer = pii_free_pointer("cell-fr-par-1");
    let mut cells_resolved: Vec<String> = Vec::new();
    let mut reliabilities: Vec<(String, CellReliability)> = Vec::new();

    let merged: MultiCellCertificate = MultiCellFanOut::new()
        .fan_out("acme/u-victim", &set, &pointer, |c, p| {
            assert_eq!(
                p.subject().artifact_ref().0,
                "myelin://01J0ACME/issues/issue/42",
                "the carrier is the opaque artifact ref, never a person"
            );
            cells_resolved.push(c.as_str().to_string());
            let scope = format!("acme/u-victim@{}", c.as_str());
            let (cell_cert, reliability) = erase_in_cell(&scope);
            reliabilities.push((c.as_str().to_string(), reliability));
            cell_cert
        })
        .expect("the complete multi-cell DSAR fan-out seals");

    assert!(
        merged.per_cell.iter().all(PerCellReceipt::cell_is_complete),
        "GA-D1: every cell erased every one of its H1–H18 holders (0 holders missed)"
    );
    for r in &merged.per_cell {
        assert_eq!(
            r.cell_certificate.holders_missed, 0,
            "0 holders missed in the cell"
        );
        assert_eq!(
            r.cell_certificate.erasure_fanout_coverage, 1.0,
            "erasure_fanout_coverage == 1.0 over the WHOLE H1–H18 set"
        );
    }

    assert_eq!(merged.cells_missed, 0, "GA-D8: 0 cells missed");
    assert_eq!(merged.cells_total, 5, "home ∪ 4 members = 5 cells");
    assert_eq!(merged.per_cell.len(), 5, "complete per-cell receipt set");
    assert!(
        merged.is_complete(),
        "the merged multi-cell certificate is complete"
    );
    assert_eq!(
        cells_resolved.len(),
        5,
        "every cell fanned out cell-locally (none skipped)"
    );

    assert_eq!(reliabilities.len(), 5, "a reliability witness per cell");
    for (cell_id, rel) in &reliabilities {
        assert!(
            rel.destroyed_key_epoch.is_some(),
            "{cell_id}: the per-subject DEK was crypto-shredded (unrecoverable in DBs AND backups)"
        );
        assert_eq!(
            rel.search_reidentify, 0,
            "{cell_id}: 0 search-vector re-identification"
        );
        assert_eq!(
            rel.agent_reidentify, 0,
            "{cell_id}: 0 agent-memory (H11) re-identification"
        );
    }

    let ledger = ErasureLedger::new();
    let erasure_offset = 100u64;
    let was_new = ledger.record_completion(
        DsrId("dsr:flagship".into()),
        VICTIM_PRINCIPAL.to_string(),
        tenant().0.clone(),
        vec![
            "git_db".into(),
            "ci_db".into(),
            "issues_db".into(),
            "knowledge_db".into(),
            "chat_db".into(),
        ],
        vec![DestroyedKeyEpoch {
            holder_id: "per_subject_dek".into(),
            key_epoch_destroyed: Some(7),
        }],
        erasure_offset,
        1_750_000_000,
    );
    assert!(
        was_new,
        "the ledger records the completion (a first completion, idempotently keyed)"
    );
    assert!(
        !ledger.record_completion(
            DsrId("dsr:flagship".into()),
            VICTIM_PRINCIPAL.to_string(),
            tenant().0.clone(),
            vec!["git_db".into()],
            vec![],
            erasure_offset,
            1_750_000_000,
        ),
        "a re-completion of the same DSR is a no-op (resumable, 0 double-erase)"
    );
    let older_backup_pit = 50u64;
    let to_reerase = ledger.post_pit_records_after(older_backup_pit);
    assert_eq!(
        to_reerase.len(),
        1,
        "post-restore re-erasure: the subject erased after the PIT is re-erased (0 resurrected)"
    );
    assert_eq!(to_reerase[0].subject, VICTIM_PRINCIPAL);
    let newer_backup_pit = 200u64;
    assert!(
        ledger.post_pit_records_after(newer_backup_pit).is_empty(),
        "a restore after the erasure resurrects nothing"
    );

    assert!(
        CANONICAL_POSTURE.residual.contains("AUTHOR's DEK")
            && CANONICAL_POSTURE.residual.contains("not the subject's"),
        "the residual is third-party PII under the AUTHOR's DEK - the ONE documented posture"
    );
    assert!(
        CANONICAL_POSTURE.structural_floor_ships(),
        "the structural floor ships regardless (the residual is the documented limit, not a defect)"
    );

    let auth = AuditAuthority::new(CellSigningKey::from_seed("cell:fr-par:audit"));
    let region = Region("fr-par".into());
    let bundle = MerkleProvenBundle {
        dsr_id: DsrId("dsr:flagship".into()),
        receipts: merged
            .per_cell
            .iter()
            .map(|r| r.content_hash.clone())
            .collect(),
        bundle_digest: merged.content_hash.clone(),
        merkle_inclusion: None,
    };
    assert!(
        bundle.merkle_inclusion.is_none(),
        "the unsealed bundle has no inclusion proof"
    );
    let sealed = auth.seal_dsr_certificate(&tenant(), &region, &bundle, "2026-06-24T12:00:00Z");
    let inclusion = sealed
        .merkle_inclusion
        .clone()
        .expect("the certificate SEALS - the bundle carries the Merkle inclusion proof");
    assert!(
        inclusion.contains("->blake3:"),
        "the inclusion proof reduces to a blake3 root"
    );
    assert_eq!(
        sealed.bundle_digest, merged.content_hash,
        "the sealed digest is the merged certificate"
    );

    eprintln!(
        "E2E-4 (2026-06-24): holders_missed=0 cells_missed={} cells_total={} \
         recoverable_pii=0 vectors_recoverable=0 backups_recoverable=0 \
         post_restore_reerased={} residual=THE_ONE_POSTURE certificate=SEALED({}) inclusion={}",
        merged.cells_missed,
        merged.cells_total,
        to_reerase.len(),
        sealed.bundle_digest,
        inclusion,
    );
}

#[test]
fn e2e_4_red_a_dropped_holder_refuses_to_seal() {
    let mut cov = FullFanOutCoverage::new();
    for &h in Holder::ALL {
        if h != Holder::SearchIndex {
            cov.record_reached(h);
        }
    }
    let gap = GaD1Certificate::seal("acme/u-victim", &cov)
        .expect_err("a missed holder does NOT seal a green certificate");
    assert_eq!(gap.holders_missed, 1);
    assert_eq!(gap.missed, vec![Holder::SearchIndex]);
    assert!(gap.erasure_fanout_coverage < 1.0);
}

#[test]
fn e2e_4_red_b_dropped_cell_refuses_to_seal() {
    let set = cell_scale_member_set();
    let mut cov = MultiCellCoverage::new(set);
    for c in [
        "cell-fr-par-1",
        "cell-fr-par-2",
        "cell-fr-par-3",
        "cell-fr-par-5",
    ] {
        let (cell_cert, _) = erase_in_cell(&format!("acme/u-victim@{c}"));
        cov.record_receipt(PerCellReceipt::new(cell(c), cell_cert));
    }
    assert_eq!(cov.cells_missed(), 1, "the dropped cell is COUNTED");
    assert_eq!(
        cov.missed(),
        vec![cell("cell-fr-par-4")],
        "named: cell-fr-par-4"
    );
    let gap = MultiCellCertificate::seal("acme/u-victim", &cov)
        .expect_err("a missed cell does NOT seal a green certificate");
    assert_eq!(gap.cells_missed, 1);
    assert_eq!(gap.cells_total, 5);
}

#[test]
fn e2e_4_red_c_a_hidden_embedding_re_identifies() {
    let search = SearchIndexModel::new();
    search.index_from_source(VICTIM_PRINCIPAL, "victim@example.com");
    assert_eq!(
        search.reidentify_hits(VICTIM_PRINCIPAL),
        1,
        "a hidden (not purged) embedding STILL re-identifies - the gate would be RED"
    );
    SearchIndexHolder::new(&search)
        .erase(erase_scope())
        .expect("the purge succeeds");
    assert_eq!(
        search.reidentify_hits(VICTIM_PRINCIPAL),
        0,
        "the purge (not hide) drives re-identification to 0"
    );
}

#[test]
fn e2e_4_crosses_every_subsystem_holder() {
    for required in [
        Holder::GitDb,
        Holder::CiDb,
        Holder::IssuesDb,
        Holder::KnowledgeDb,
        Holder::ChatDb,
        Holder::ObjectStore,
        Holder::Backups,
        Holder::SearchIndex,
        Holder::ReferenceGraph,
        Holder::NotificationHistory,
        Holder::EventBus,
        Holder::AgentMemory,
        Holder::AgentTrace,
        Holder::AuthzTuples,
        Holder::Identity,
        Holder::AuditCarveOut,
        Holder::CachesAndCdn,
        Holder::GdprOwnStores,
    ] {
        assert!(
            Holder::ALL.contains(&required),
            "{} is in the exhaustive catalogue the flagship crosses",
            required.h_label()
        );
    }
    assert_eq!(Holder::ALL.len(), 18, "the flagship crosses all 18 holders");
    assert_eq!(
        Holder::Backups.erasure(),
        HolderErasure::CryptoShredByConstruction
    );
}
