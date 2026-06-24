//! # Drill — SRCH-D10 (HYOK cross-store at scale) + SRCH-D4 at backup scale (SRCH-P29 → P-422)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D10 (~365:
//! a HYOK content class → Search skips it; 0 HYOK plaintext in ANY derived store — index segments,
//! vectors, caches, backups — the cross-store assertion jointly with Storage + Agent) + SRCH-D4
//! (~359, at BACKUP scale: erase a subject → 0 recoverable personal data INCL. vectors INCL.
//! backups). **Architecture:** `search-and-indexing.md` §4.8 (the crypto-shred layering: per-tenant
//! index DEK + per-subject source DEK backstop; the HYOK structural skip
//! `can_derive_plaintext_index() = false`; embeddings purged with source incl. backups).
//!
//! ## What this drill proves (the dated green artifacts, 2026-06-24)
//! 1. **SRCH-D10 — HYOK cross-store at scale.** A HYOK content class is `IndexAdmission::SkipHyok`
//!    (the frozen verdict): Search builds NO plaintext index over it. The gate walks the FULL
//!    cross-store set (index segments / vectors / caches / backups) and asserts the HYOK class is in
//!    **0 of them**, while a platform-managed control class (indexed through the SAME live path) IS
//!    present — so the green is a real present-vs-absent contrast, not a "nothing was indexed"
//!    artefact. **0 HYOK plaintext in any derived store.**
//! 2. **SRCH-D4 at backup scale.** A moderate live corpus references a data subject; the subject's
//!    index segments are sealed as BACKUPS under the per-tenant index DEK (real AES-256-GCM). The DSR
//!    fan-out erases the subject (purge + compact → 0 recoverable live, 0 orphan embedding), and the
//!    tenant-decommission crypto-shred destroys the per-tenant index DEK. After the shred, EVERY
//!    sealed backup segment is plaintext-UNRECOVERABLE (the DEK no longer resolves — the §7.5
//!    backstop). **0 recoverable incl. vectors incl. backups.**
//!
//! ## Floors named
//! These ARE the named floor follow-ons of the SRCH-P15 CI-variant SRCH-D4 (per-tenant index DEK →
//! backup-scale erasure; HYOK structural skip → cross-store assertion at scale). The JOINT
//! cross-store assertion with Storage + Agent + the holder-coverage receipt is the M5 DSAR fan-out
//! **E2E-4 (SRCH-P32 / P-465)** — this drill is Search's half. Run at a scaled-down (CI) variant of
//! "backup scale"; the world-scale 30x fleet corpus is the only remaining floor. The SRCH-P15 erase
//! mutation floor holds (this drill re-drives that exact path).

use std::sync::Arc;

use myelin_gdpr::{PersonalDataHolder, SubjectRef};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{Dek, Hyok, HyokKeyService, HyokServiceDenied, KmsEngine, PlatformManaged};
use myelin_storage::{DekHandle, WrappedDek};
use myelin_tenancy::{Region, TenantId};

use myelin_search::{
    build_live_corpus, subject_matcher, AclFilter, BackupScaleEraseGate, BackupScaleEraseInputs,
    HyokCrossStoreGate, HyokCrossStoreInputs, SealedBackupSegment, SearchDekPin, SearchEraseHolder,
};

const NOW: &str = "2026-06-24T00:00:00Z";

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

/// A HYOK key service that denies every wrap/unwrap (the customer holds the key outside Myelin).
struct DenyAllHyok;
impl HyokKeyService for DenyAllHyok {
    fn wrap(&self, _dek: &Dek) -> Result<WrappedDek, HyokServiceDenied> {
        Err(HyokServiceDenied)
    }
    fn unwrap(&self, _w: &WrappedDek) -> Result<DekHandle, HyokServiceDenied> {
        Err(HyokServiceDenied)
    }
    fn destroy(&self) {}
}

/// **SRCH-D10 — a HYOK class is structurally absent from EVERY derived store (0 HYOK plaintext
/// cross-store), while a platform control class IS present.** The dated green artifact.
#[test]
fn srch_d10_hyok_cross_store_zero_plaintext_anywhere() {
    // A platform-managed control class IS indexed through the live path (index + vectors).
    let (ix, ids) = build_live_corpus(&tenant(), &region(), "u-ctrl", &["c1", "c2"], &["o1", "o2"]);
    let engine = KmsEngine::new();
    let platform = PlatformManaged::new(&engine, region());
    let hyok = Hyok::new(DenyAllHyok);

    let inputs = HyokCrossStoreInputs {
        indexer: &ix,
        tenant: tenant(),
        region: region(),
        platform_cache_present: true, // the control query's RankedResults is cached (§4.10)
        platform_backup_present: true, // a sealed backup segment exists for the control class (§7.5)
        platform_doc_id: ids[0].clone(),
        platform_probe_text: "raft leadership".into(),
        now: NOW.into(),
    };

    let artifact = HyokCrossStoreGate::new()
        .run_or_fail_ci(&inputs, &hyok, &platform)
        .expect("SRCH-D10 green: 0 HYOK plaintext in any derived store");

    assert_eq!(
        artifact.stores_with_hyok_plaintext, 0,
        "0 HYOK plaintext in any derived store (index/vectors/caches/backups) — the SRCH-D10 gate"
    );
    assert_eq!(
        artifact.stores_walked.len(),
        4,
        "all four derived stores walked"
    );
    assert_eq!(
        artifact.stores_with_platform_class, 4,
        "the platform-managed control class IS present in all four stores (the walk is real)"
    );
    // The dated green-artifact line (observability is part of the pass).
    println!("[P-422 GATE GREEN 2026-06-24] {}", artifact.summary());
    assert!(artifact.is_green());
}

/// **SRCH-D4 at backup scale — erase a subject ⇒ 0 recoverable incl. vectors incl. backups.** The
/// subject's index segments are sealed as backups under the per-tenant index DEK; after the erase +
/// the tenant-decommission crypto-shred, every backup segment is plaintext-unrecoverable. The dated
/// green artifact.
#[test]
fn srch_d4_backup_scale_zero_recoverable_including_backups() {
    let target = "u-target";
    // 3 docs reference the subject; 10 do not (a moderate corpus, the CI variant of "backup scale").
    let subject_docs = ["t1", "t2", "t3"];
    let other_docs = ["o0", "o1", "o2", "o3", "o4", "o5", "o6", "o7", "o8", "o9"];
    let (ix, ids) = build_live_corpus(&tenant(), &region(), target, &subject_docs, &other_docs);

    // Pre-erase: the subject is reachable by full-text + by k-NN (the index + vector stores hold it).
    let matcher = subject_matcher(target, &tenant());
    assert_eq!(
        ix.locate_subject(&tenant(), &region(), &matcher).len(),
        3,
        "the subject references three live docs before the erase"
    );
    let pre_ft = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "raft leadership", 30)
        .expect("ft");
    assert!(
        !pre_ft.is_empty(),
        "the subject's docs are full-text reachable before the erase"
    );

    // Reserve the per-tenant index DEK; SEAL the subject's index segments as backups under it.
    let kms = Arc::new(KmsEngine::new());
    let pin = SearchDekPin::new(kms);
    let key_ref = pin
        .reserve(&tenant(), &region())
        .expect("reserve the per-tenant index DEK");
    let dek = pin
        .resolve(&key_ref, &region())
        .expect("resolve the live DEK");
    let subject_doc_ids: Vec<&String> = ids
        .iter()
        .filter(|id| subject_docs.iter().any(|d| id.ends_with(d)))
        .collect();
    let backups: Vec<SealedBackupSegment> = subject_doc_ids
        .iter()
        .map(|id| {
            SealedBackupSegment::seal(
                &dek,
                id,
                format!("{target}'s index segment plaintext for {id}").as_bytes(),
            )
        })
        .collect();
    assert_eq!(backups.len(), 3, "three subject backup segments sealed");

    let holder = SearchEraseHolder::new(ix.clone(), pin.clone(), region());

    let mut inputs = BackupScaleEraseInputs {
        erase_holder: &holder,
        dek: &pin,
        index_key_ref: key_ref,
        subject: subject(target),
        tenant: tenant(),
        backup_segments: &backups,
        subject_backstop_id: None, // tenant-decommission shred reaches them all
        now: NOW.into(),
    };

    let artifact = BackupScaleEraseGate::new()
        .run_or_fail_ci(&mut inputs)
        .expect("SRCH-D4 at backup scale green: 0 recoverable incl. vectors incl. backups");

    assert_eq!(
        artifact.live_docs_purged, 3,
        "the three subject docs were purged"
    );
    assert_eq!(
        artifact.live_docs_remaining, 0,
        "0 live docs remain (purged, not hidden)"
    );
    assert!(
        artifact.zero_orphan_embedding,
        "0 orphan embedding after compaction (§3.3)"
    );
    assert_eq!(
        artifact.backup_segments_recoverable_before_shred, 3,
        "the backups DID hold the plaintext before the shred (the proof is real, not vacuous)"
    );
    assert_eq!(
        artifact.backup_segments_recoverable_after_shred, 0,
        "0 backup segments recoverable after the crypto-shred (0 recoverable incl. backups, §7.5)"
    );

    // Cross-check the live store too: the subject is GONE from full-text + k-NN (0 recoverable live).
    let post = ix.locate_subject(&tenant(), &region(), &matcher);
    assert!(
        post.is_empty(),
        "0 docs reference the subject after the erase (live)"
    );
    // The 10 unrelated docs survive (the erase is surgical).
    let unrelated = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "paxos", 30)
        .expect("ft unrelated");
    assert_eq!(
        unrelated.len(),
        10,
        "all 10 unrelated docs survive (surgical erase)"
    );

    println!("[P-422 GATE GREEN 2026-06-24] {}", artifact.summary());
    assert!(artifact.is_green());
}

/// **The holder's `erase` contract surface (10.1) still leaves 0 recoverable live (the SRCH-P15
/// floor holds across the SRCH-P29 backup-scale work).** This drill re-drives the SAME live path; it
/// does not re-implement the erase.
#[test]
fn srch_p15_erase_floor_holds_under_srch_p29() {
    let (ix, _ids) = build_live_corpus(
        &tenant(),
        &region(),
        "u-target",
        &["t1", "t2"],
        &["o1", "o2"],
    );
    let kms = Arc::new(KmsEngine::new());
    let pin = SearchDekPin::new(kms);
    pin.reserve(&tenant(), &region()).expect("reserve");
    let holder = SearchEraseHolder::new(ix.clone(), pin, region());

    holder
        .erase(myelin_gdpr::EraseScope::Subject {
            subject: subject("u-target"),
            tenant: tenant(),
        })
        .expect("erase the subject");

    let matcher = subject_matcher("u-target", &tenant());
    assert!(
        ix.locate_subject(&tenant(), &region(), &matcher).is_empty(),
        "0 recoverable live after the erase (the SRCH-P15 mutation floor holds)"
    );
}
