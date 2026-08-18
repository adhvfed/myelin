use std::sync::Arc;
use std::time::Duration;

use myelin_refs_service::{
    build_backup_scale_corpus, re_erase_at_backup_scale, EdgeProjection, R2ProjectionCache,
    RefsDekPin, RefsEdgeBuilder, RefsErasureLedger,
};
use myelin_storage::{InMemoryCache, KmsEngine};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

#[test]
fn ref_d5_restore_then_reerase_leaves_zero_recoverable_pii_at_backup_scale() {
    let subjects = 200usize;
    let edges_per_subject = 4usize;
    let corpus = build_backup_scale_corpus(&tenant(), &region(), subjects, edges_per_subject);
    assert_eq!(corpus.edge_count(), subjects * edges_per_subject);

    let dek = Arc::new(RefsDekPin::new(Arc::new(KmsEngine::new())));
    let cache = Arc::new(R2ProjectionCache::with_ttl(
        Arc::new(InMemoryCache::new()),
        dek.clone(),
        Duration::from_secs(300),
    ));
    let builder = RefsEdgeBuilder::new(EdgeProjection::new());
    let ledger = RefsErasureLedger::new();

    let targets: Vec<String> = corpus.subjects.iter().take(subjects / 2).cloned().collect();

    let now = "2026-06-24T00:00:00Z";
    let report =
        re_erase_at_backup_scale(&corpus, &builder, &cache, &dek, &ledger, &targets, now).unwrap();

    assert_eq!(
        report.cached_titles_resurrected_by_restore,
        targets.len() * edges_per_subject,
        "the restore re-warmed every erased subject's name-bearing cached titles"
    );
    assert_eq!(
        report.deks_resurrected_by_restore,
        targets.len(),
        "the restore resurrected every erased subject's per-subject DEK"
    );
    assert!(
        report.edges_re_tombstoned >= targets.len() * edges_per_subject,
        "the re-erase re-tombstoned every restored edge"
    );

    assert_eq!(
        report.recoverable_pii, 0,
        "0 decryptable cached titles post-restore (the cache purge re-applied)"
    );
    assert_eq!(
        report.live_deks_post_reerase, 0,
        "0 live per-subject DEKs post-restore (the crypto-shred re-applied; unrecoverable in backup too)"
    );
    assert_eq!(
        report.live_edges_post_reerase, 0,
        "0 live edges for a re-erased subject (every reference stays tombstoned; the person unresolvable)"
    );
    assert!(
        report.is_ref_d5_backup_scale_green(),
        "REF-D5 at backup scale MUST be GREEN: {}",
        report.summary()
    );
    assert_eq!(report.re_erased_subjects, targets.len());

    println!(
        "[P-456 REF-D5 BACKUP-SCALE GREEN 2026-06-24] {} (subjects={subjects}, edges_per_subject={edges_per_subject})",
        report.summary()
    );
}
