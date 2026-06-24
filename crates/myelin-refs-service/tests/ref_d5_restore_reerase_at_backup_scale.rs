//! # REF-D5 (backup scale) — restore + re-erase leaves 0 resurrected PII (REF-P25 / P-456, M5)
//!
//! **Drill catalogue:** REF-D5 (the erasure drill) at its **backup-scale form** — reference-graph.md
//! drill ~350 + §7 **D-5 the scale variant**. This is the **world-scale promotion** of the REF-P15
//! CI-variant erase drill (`integration_ref_p15_holder_erase.rs` — one subject, one cached title): erase
//! a subject + a referenced artifact across a SCALED Refs index, RESTORE the edge index from a PRE-erase
//! backup, then RE-ERASE from the **erasure ledger** (contract 10.8) — and prove **0 resurrected PII**
//! past the erasure (no resurrected PII even after a backup is restored, external-insights/04 §1).
//!
//! **Architecture:** reference-graph.md §4.6 tail (the small structural erasure surface — the per-subject
//! DEK crypto-shred of a cached title + the `*.erased`-driven edge tombstone, NO erasure backdoor), §7
//! D-5 (the scale variant). **Contract-index:** row **10.1** (the erase holder at backup scale, OWNED),
//! row **10.8** (the PII-free, non-shred-erasable erasure ledger that drives post-restore re-erasure,
//! CONSUMED), row **11.5** (backup / restore cross-seam). **Doctrine:** external-insights/04 §1 (the key
//! stays destroyed even after a backup is restored), EI-01 §3 (prove it at scale — the 0-recoverable
//! property is DRILLED green across a backup-scale corpus + the restore→re-erase cross-seam, not asserted
//! in prose; name the floor; never claim a green you did not earn).
//!
//! ## What this drill proves (the backup-scale REF-D5 green)
//! Erase a subset of subjects (cache-PII purge + per-subject DEK crypto-shred + edge tombstone, RECORDED
//! in the 10.8 ledger); RESTORE a pre-erase backup (the cached titles, per-subject DEKs, and live edges
//! come back); RE-ERASE from the ledger (re-run the IDENTICAL §4.6 erase) — and **0 recoverable PII**
//! survives: 0 decryptable cached titles, 0 live per-subject DEKs, 0 live edges for a re-erased subject
//! (the person unresolvable, no `500` on resolve — a crypto-shredded cache reads as a clean MISS).
//!
//! ## The CI→backup-scale promotion (the floor this prompt resolves)
//! This drill PROMOTES the REF-P15 CI-variant REF-D5 (`integration_ref_p15_holder_erase.rs` — one
//! subject's cached title purged from live Valkey, 0 recoverable) to its backup-scale form (a scaled
//! corpus + the restore→re-erase cross-seam the CI variant named as its REF-P25 floor). The REF-P15
//! erase mutation floor (`holder.rs`, 13/13 viable = 100%) is UNCHANGED and STILL HOLDS at scale — this
//! drill adds NO new erase decision logic; it scales the corpus the frozen §4.6 surface runs over (EI-01
//! §7, no parallel second eraser) and the drill's own counter-case (a missed ledger entry → resurrected
//! PII, in the lib unit tests) flips the verdict RED, proving the green is earned.
//!
//! ## Floor named (the ONE legitimate remaining floor)
//! The **30× world-scale FLEET-hardware backup/restore** over the PgStore-backed edge partition + the
//! KMS/Valkey backup ([`myelin_refs_service::WORLD_SCALE_BACKUP_FLEET_FLOOR`]) is the ONE legitimate
//! remaining floor. This drill proves the 0-recoverable-PII PROPERTY + the restore→re-erase cross-seam
//! over a deterministic backup-scale corpus with REAL crypto-shred (a per-subject DEK destroy makes the
//! sealed cached title genuinely undecryptable) — the property does not change shape when real fleet
//! hardware carries the full cardinality (a shredded key is unrecoverable by construction at any scale).
//!
//! Permanent-gate posture: re-run on every erase/restore-touching change; folds into E2E-4 (the DSAR
//! fan-out — REF-P27 carries the E2E run; this drills the Refs restore/re-erase mechanism it depends on).

use std::sync::Arc;
use std::time::Duration;

use myelin_refs_service::{
    build_backup_scale_corpus, re_erase_at_backup_scale, EdgeProjection, R2ProjectionCache,
    RefsDekPin, RefsEdgeBuilder, RefsErasureLedger, WORLD_SCALE_BACKUP_FLEET_FLOOR,
};
use myelin_storage::{InMemoryCache, KmsEngine};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

/// **THE backup-scale REF-D5 PROOF (the dated green artifact the DoD names).** A backup-scale corpus
/// (many subjects × many edges, each with a name-bearing cached title sealed under its per-subject DEK);
/// erase a subset (cache purge + crypto-shred + edge tombstone, recorded in the 10.8 ledger); restore a
/// pre-erase backup (resurrecting the PII); re-erase from the ledger — 0 recoverable PII survives.
#[test]
fn ref_d5_restore_then_reerase_leaves_zero_recoverable_pii_at_backup_scale() {
    // A scale large enough to span many subjects × many edges (the property is scale-invariant — a
    // crypto-shredded key is unrecoverable by construction; the fleet cardinality is the named floor).
    let subjects = 200usize;
    let edges_per_subject = 4usize;
    let corpus = build_backup_scale_corpus(&tenant(), &region(), subjects, edges_per_subject);
    assert_eq!(corpus.edge_count(), subjects * edges_per_subject);

    // The REAL crypto-shred surface: an InMemoryCache-backed R2ProjectionCache (the SAME crypto-shred
    // path the dev-stack Valkey backing rides — dev<->prod is a config swap) + the shared KMS hierarchy.
    let dek = Arc::new(RefsDekPin::new(Arc::new(KmsEngine::new())));
    let cache = Arc::new(R2ProjectionCache::with_ttl(
        Arc::new(InMemoryCache::new()),
        dek.clone(),
        Duration::from_secs(300),
    ));
    let builder = RefsEdgeBuilder::new(EdgeProjection::new());
    let ledger = RefsErasureLedger::new();

    // Erase HALF the subjects (the DSAR targets) — the other half stay live (subject-grained isolation).
    let targets: Vec<String> = corpus.subjects.iter().take(subjects / 2).cloned().collect();

    let now = "2026-06-24T00:00:00Z";
    let report = re_erase_at_backup_scale(&corpus, &builder, &cache, &dek, &ledger, &targets, now);

    // The restore genuinely resurrected the PII (non-vacuous: the drill proves 0-recoverable AFTER a
    // restore actually brought the name-bearing titles + per-subject DEKs back).
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

    // THE GATE: 0 resurrected PII past the erasure, across EVERY surface.
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

    // The floor is named (EI-01 §3) — the world-scale fleet backup/restore is the ONE remaining floor.
    assert!(WORLD_SCALE_BACKUP_FLEET_FLOOR.contains("30x"));

    println!(
        "[P-456 REF-D5 BACKUP-SCALE GREEN 2026-06-24] {} (subjects={subjects}, edges_per_subject={edges_per_subject})",
        report.summary()
    );
}
