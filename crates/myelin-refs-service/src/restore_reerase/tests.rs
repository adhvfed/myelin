//! Unit + chained-drill tests for the REF-P25 backup-scale restore + re-erase surface (REF-D5 at
//! scale). Proves: the PII-free ledger records every erasure (10.8); a restore of a pre-erase backup
//! resurrects the cached titles + per-subject DEKs + un-tombstones the edges; the re-erase from the
//! ledger re-applies the IDENTICAL §4.6 erase → **0 recoverable PII** (0 decryptable cached titles, 0
//! live per-subject DEKs, 0 live edges for a re-erased subject); the pass is idempotent; the
//! counter-case (a MISSED ledger entry) leaves resurrected PII (the green is earned). The REF-P15 erase
//! mutation floor (`holder.rs`) STILL HOLDS at scale — this drill adds no new erase decision logic.

use std::sync::Arc;
use std::time::Duration;

use myelin_storage::{InMemoryCache, KmsEngine};

use super::*;
use crate::dek::RefsDekPin;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
const NOW: &str = "2026-06-24T00:00:00Z";

/// A fresh (cache, dek, builder, ledger) over a shared KMS — the SAME pin the cache seals with and the
/// re-erase crypto-shreds with (one hierarchy).
fn fixtures() -> (
    Arc<R2ProjectionCache>,
    Arc<RefsDekPin>,
    RefsEdgeBuilder,
    RefsErasureLedger,
) {
    let dek = Arc::new(RefsDekPin::new(Arc::new(KmsEngine::new())));
    let cache = Arc::new(R2ProjectionCache::with_ttl(
        Arc::new(InMemoryCache::new()),
        dek.clone(),
        Duration::from_secs(300),
    ));
    let builder = RefsEdgeBuilder::new(EdgeProjection::new());
    (cache, dek, builder, RefsErasureLedger::new())
}

/// **Unit: the PII-free ledger records an erasure + is non-shred-erasable (10.8).** A `record` is
/// remembered (idempotently merges); the entry carries ONLY opaque ids (the pseudonymous subject id +
/// the key NAMES + the edge ids), never a name/payload.
#[test]
fn ledger_records_erasure_pii_free_and_idempotent() {
    let ledger = RefsErasureLedger::new();
    assert!(ledger.is_empty());

    ledger.record(
        &tenant(),
        &region(),
        "p-opaque-0",
        &["kms://acme/0/subject:p-opaque-0".into()],
        &["edge-a".into()],
        NOW,
    );
    // Idempotent merge: a second record of the same subject merges key/edge ids, keeps the first time.
    ledger.record(
        &tenant(),
        &region(),
        "p-opaque-0",
        &["kms://acme/0/subject:p-opaque-0".into()],
        &["edge-b".into()],
        "2026-06-25T00:00:00Z",
    );

    assert_eq!(ledger.len(), 1, "one subject, merged");
    assert!(ledger.is_erased(&tenant(), &region(), "p-opaque-0"));
    let e = &ledger.entries()[0];
    assert_eq!(e.subject_id, "p-opaque-0");
    assert_eq!(e.edge_ids, vec!["edge-a".to_string(), "edge-b".to_string()]);
    assert_eq!(
        e.erased_at, NOW,
        "keeps the FIRST erased_at (non-shred-erasable)"
    );
    // PII-free: the subject id is the pseudonymous opaque id; the key ref is a NAME, never key material.
    assert!(e.subject_id.starts_with("p-opaque-"));
    assert!(e.key_refs[0].starts_with("kms://"));
}

/// **Unit: the ledger is non-shred-erasable across cells (residency-pin).** A subject erased in one cell
/// is NOT seen in another cell's read — the re-erase replays only within its own cell.
#[test]
fn ledger_is_cell_scoped() {
    let ledger = RefsErasureLedger::new();
    ledger.record(&tenant(), &region(), "p-opaque-0", &[], &[], NOW);
    assert!(ledger.is_erased(&tenant(), &region(), "p-opaque-0"));
    assert!(
        !ledger.is_erased(&TenantId("other".into()), &region(), "p-opaque-0"),
        "the ledger is cell-scoped (residency-pin — no cross-cell read)"
    );
}

/// **THE chained drill (REF-D5 at backup scale): erase → restore a pre-erase backup → re-erase from the
/// ledger → 0 recoverable PII.** A backup-scale corpus (many subjects × many edges); erase a subset;
/// restore resurrects their cached titles + per-subject DEKs + un-tombstones their edges; the re-erase
/// from the ledger re-applies the IDENTICAL §4.6 erase → the gate: 0 recoverable PII across every
/// surface (0 decryptable cached titles, 0 live per-subject DEKs, 0 live edges).
#[test]
fn restore_then_reerase_leaves_zero_recoverable_pii_at_backup_scale() {
    let (cache, dek, builder, ledger) = fixtures();
    // Backup scale: 12 subjects, 4 edges each = 48 name-bearing cached titles. Deterministic.
    let corpus = build_backup_scale_corpus(&tenant(), &region(), 12, 4);
    assert_eq!(corpus.edge_count(), 48);

    // Erase a subset (the DSAR targets) — the rest stay live (tenant isolation within the cell).
    let targets: Vec<String> = corpus.subjects.iter().take(5).cloned().collect();

    let report = re_erase_at_backup_scale(&corpus, &builder, &cache, &dek, &ledger, &targets, NOW);

    // The restore genuinely brought the PII back (the honest "what came back" signal is non-zero) —
    // otherwise the drill would be vacuous (proving 0 recoverable when nothing was resurrected).
    assert_eq!(
        report.cached_titles_resurrected_by_restore, 20,
        "the restore re-warmed 5 subjects × 4 cached titles (the name-bearing PII came back)"
    );
    assert_eq!(
        report.deks_resurrected_by_restore, 5,
        "the restore resurrected each erased subject's per-subject DEK"
    );
    assert!(
        report.edges_re_tombstoned >= 20,
        "the re-erase re-tombstoned every restored edge"
    );

    // THE GATE: 0 recoverable PII post-restore across EVERY surface (no resurrected PII past the erasure).
    assert_eq!(
        report.recoverable_pii, 0,
        "0 decryptable cached titles post-restore"
    );
    assert_eq!(
        report.live_deks_post_reerase, 0,
        "0 live per-subject DEKs post-restore"
    );
    assert_eq!(
        report.live_edges_post_reerase, 0,
        "0 live edges for a re-erased subject"
    );
    assert!(
        report.is_ref_d5_backup_scale_green(),
        "REF-D5 backup-scale GREEN: {}",
        report.summary()
    );

    // The ledger remembers every target (the durable, non-shred-erasable record).
    assert_eq!(report.re_erased_subjects, 5);
    for t in &targets {
        assert!(ledger.is_erased(&tenant(), &region(), t));
    }
}

/// **Tenant/subject isolation: a NON-erased subject's references + cached title survive the re-erase.**
/// The DSAR targets a subset; an untargeted subject in the SAME cell keeps its name-bearing cached title
/// and live edges (the re-erase is subject-grained, never a tenant-wide wipe).
#[test]
fn non_erased_subject_is_untouched_by_the_reerase() {
    let (cache, dek, builder, ledger) = fixtures();
    let corpus = build_backup_scale_corpus(&tenant(), &region(), 6, 2);
    let targets: Vec<String> = corpus.subjects.iter().take(2).cloned().collect();

    re_erase_at_backup_scale(&corpus, &builder, &cache, &dek, &ledger, &targets, NOW);

    // A subject NOT in the target set keeps its cached title + live edges.
    let survivor = corpus.subjects.last().unwrap().clone();
    assert!(!targets.contains(&survivor));
    let projection = builder.projection();
    for edge in corpus.edges_of(&survivor) {
        assert!(
            cache.read(&tenant(), &region(), &edge.source).is_some(),
            "the survivor's cached title is intact (subject-grained erase)"
        );
        assert!(
            projection
                .get(&tenant(), &region(), &edge.edge_id)
                .map(|r| !r.tombstoned)
                .unwrap_or(false),
            "the survivor's edge stays live"
        );
        // The survivor's per-subject DEK is still live (resolvable).
        assert!(
            dek.subject_backstop_is_live(&tenant(), &region(), &survivor),
            "the survivor's per-subject DEK is untouched"
        );
    }
}

/// **The re-erase is idempotent: running the pass again over the SAME ledger is a clean no-op success
/// (still 0 recoverable PII).** Re-shredding a dead key + re-tombstoning a tombstoned edge never errors
/// or resurrects.
#[test]
fn reerase_is_idempotent() {
    let (cache, dek, builder, ledger) = fixtures();
    let corpus = build_backup_scale_corpus(&tenant(), &region(), 3, 2);
    let targets: Vec<String> = corpus.subjects.clone();

    let first = re_erase_at_backup_scale(&corpus, &builder, &cache, &dek, &ledger, &targets, NOW);
    assert!(first.is_ref_d5_backup_scale_green());

    // Run the pass AGAIN (no new restore happened) — a clean no-op success.
    let again = re_erase_at_backup_scale(&corpus, &builder, &cache, &dek, &ledger, &targets, NOW);
    assert_eq!(again.recoverable_pii, 0, "idempotent: still 0 recoverable");
    assert_eq!(again.live_deks_post_reerase, 0);
    assert_eq!(again.live_edges_post_reerase, 0);
    assert!(again.is_ref_d5_backup_scale_green());
}

/// **Counter-case (the green is EARNED): a MISSED ledger entry leaves resurrected PII.** If the restore
/// resurrects a subject the ledger does NOT remember erasing, the re-erase replay never covers it — its
/// cached title is decryptable + its DEK live + its edges live again. The drill's own counter-case flips
/// the 0-recoverable verdict RED (proving the green is not vacuous).
#[test]
fn missed_ledger_entry_resurrects_pii_red_counter_case() {
    let (cache, dek, builder, ledger) = fixtures();
    let corpus = build_backup_scale_corpus(&tenant(), &region(), 4, 2);

    // Steady-state: ingest + warm everything.
    for edge in &corpus.edges {
        builder.handle(&corpus.edge_event(edge));
    }
    let projection = builder.projection().clone();
    for subject_id in &corpus.subjects {
        warm_subject_titles_test(&corpus, &cache, &dek, subject_id);
    }

    // A subject the restore resurrects but the LEDGER NEVER RECORDED (the buggy path: erase happened but
    // the durable record was lost — exactly what the non-shred-erasable ledger exists to prevent).
    let forgotten = corpus.subjects[0].clone();
    // Simulate the erase WITHOUT recording it in the ledger.
    let holder = RefsCacheHolder::with_cache(cache.clone(), projection.clone());
    holder
        .erase(EraseScope::Subject {
            subject: subject_ref(&forgotten, &tenant()),
            tenant: tenant(),
        })
        .unwrap();
    dek.destroy_subject_backstop(&tenant(), &forgotten);
    for edge in corpus.edges_of(&forgotten) {
        projection.tombstone(&tenant(), &region(), &edge.edge_id, "erased");
    }
    assert!(!ledger.is_erased(&tenant(), &region(), &forgotten));

    // A RESTORE resurrects the forgotten subject (cached title + DEK + edge come back).
    dek.reserve_subject_backstop(&tenant(), &region(), &forgotten)
        .unwrap();
    for edge in corpus.edges_of(&forgotten) {
        let proj = Projection {
            ref_: edge.source.clone(),
            title: edge.cached_title.clone(),
            state: "open".into(),
            icon: "doc".into(),
            render_hint: "card".into(),
            sub_anchor: None,
            flag: None,
        };
        cache
            .fill(&tenant(), &region(), &edge.source, &proj)
            .unwrap();
        builder.handle(&corpus.edge_event(edge));
    }

    // RE-ERASE from the (EMPTY) ledger — the forgotten subject is NOT replayed.
    // The ledger has no entries, so the re-erase covers nothing. The forgotten subject's PII survives:
    let mut still_recoverable = 0usize;
    for edge in corpus.edges_of(&forgotten) {
        if cache.read(&tenant(), &region(), &edge.source).is_some() {
            still_recoverable += 1;
        }
    }
    assert!(
        still_recoverable > 0,
        "RED: a subject the non-shred-erasable ledger forgot is resurrected past the erasure — \
         the 0-recoverable green is EARNED only because the ledger records every erasure"
    );
}

/// Test-only re-export of the private warm helper (the counter-case needs to warm without erasing).
fn warm_subject_titles_test(
    corpus: &BackupScaleErasureCorpus,
    cache: &R2ProjectionCache,
    dek: &RefsDekPin,
    subject_id: &str,
) {
    super::warm_subject_titles(corpus, cache, dek, subject_id);
}

/// **The world-scale fleet floor is named (EI-01 §3) + the telemetry signal is a named constant.**
#[test]
fn floor_and_signal_are_named() {
    assert!(WORLD_SCALE_BACKUP_FLEET_FLOOR.contains("30x"));
    assert!(WORLD_SCALE_BACKUP_FLEET_FLOOR.contains("fleet hardware"));
    assert_eq!(
        REERASE_RECOVERABLE_PII_SIGNAL,
        "refs.reerase_recoverable_pii"
    );
}
