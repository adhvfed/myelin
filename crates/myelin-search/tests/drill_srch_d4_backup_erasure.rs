use std::sync::Arc;

use myelin_gdpr::{PersonalDataHolder, SubjectRef};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::KmsEngine;
use myelin_tenancy::{Region, TenantId};

use myelin_search::{
    build_live_corpus, subject_matcher, AclFilter, BackupScaleEraseGate, BackupScaleEraseInputs,
    SealedBackupSegment, SearchDekPin, SearchEraseHolder,
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

#[test]
fn srch_d4_backup_scale_zero_recoverable_including_backups() {
    let target = "u-target";
    let subject_docs = ["t1", "t2", "t3"];
    let other_docs = ["o0", "o1", "o2", "o3", "o4", "o5", "o6", "o7", "o8", "o9"];
    let (ix, ids) = build_live_corpus(&tenant(), &region(), target, &subject_docs, &other_docs);

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
        subject_backstop_id: None,
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

    let post = ix.locate_subject(&tenant(), &region(), &matcher);
    assert!(
        post.is_empty(),
        "0 docs reference the subject after the erase (live)"
    );
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
