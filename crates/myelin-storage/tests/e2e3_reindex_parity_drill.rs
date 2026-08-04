use std::collections::BTreeMap;

use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_storage::{
    run_e2e3_storage_half, DerivedReindexSource, DerivedStoreClass, DerivedStoreParity,
    E2e3StorageArtifact,
};

use myelin_events::{Actor, EmitContextBase, Region, TenantId, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn region() -> Region {
    Region("fr-par".into())
}
fn tenant() -> TenantId {
    TenantId("01J0ACME".into())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
        caused_by: None,
    }
}

fn all_sources() -> BTreeMap<DerivedStoreClass, DerivedReindexSource> {
    let mut olap = DerivedReindexSource::new("olap_src");
    olap.upsert("issue:PROJ-1", 1, serde_json::json!({ "cfd": 3 }))
        .upsert("issue:PROJ-2", 2, serde_json::json!({ "cfd": 5 }));

    let mut search = DerivedReindexSource::new("search_src");
    search
        .upsert("page:home", 1, serde_json::json!({ "text": "raft" }))
        .upsert("page:guide", 2, serde_json::json!({ "text": "paxos" }))
        .upsert("page:faq", 1, serde_json::json!({ "text": "faq" }));

    let mut refs = DerivedReindexSource::new("refs_src");
    refs.upsert(
        "edge:PR-1->ISSUE-1",
        1,
        serde_json::json!({ "kind": "closes" }),
    )
    .upsert(
        "edge:COMMIT-1->PR-1",
        1,
        serde_json::json!({ "kind": "part_of" }),
    );

    BTreeMap::from([
        (DerivedStoreClass::Olap, olap),
        (DerivedStoreClass::Search, search),
        (DerivedStoreClass::Refs, refs),
    ])
}

#[test]
fn e2e3_storage_half_is_green_cold_equals_live_zero_drift() {
    let artifact = run_e2e3_storage_half(&region(), &all_sources(), &ctx_base())
        .expect("the E2E-3 storage half runs");

    assert!(
        artifact.is_green(),
        "the E2E-3 storage half is green: {artifact:?}"
    );
    assert_eq!(artifact.stores_with_drift, 0, "0 drift - cold == live");
    assert_eq!(
        artifact.derived_stores_with_backup_path, 0,
        "0 derived stores backed up - reindex-from-source is the only rebuild path (§7.1/§7.3)"
    );
    assert!(
        artifact.covers_all_derived_stores(),
        "the artifact covers the WHOLE derived-store set (OLAP + Search + Refs)"
    );
    for leg in &artifact.legs {
        assert!(
            leg.cold_matches_live(),
            "{}: cold reindex byte-matches live (0 drift)",
            leg.store.name()
        );
        assert_eq!(
            leg.snapshots_emitted_second,
            0,
            "{}: the re-run is idempotent (0 new snapshots)",
            leg.store.name()
        );
    }

    let mut sig = SignalSource::new();
    sig.set_scalar(
        SignalName::CrossTenantCount,
        artifact.stores_with_drift as i64,
    );
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-447 E2E-3 GREEN 2026-06-24] {} | cert={:016x}",
        artifact.summary(),
        artifact.certificate_hash
    );
}

#[test]
fn e2e3_gate_goes_red_when_a_derived_store_drifts() {
    let green = || DerivedStoreParity {
        store: DerivedStoreClass::Olap,
        live_hash: 11,
        cold_hash: 11,
        snapshots_emitted_first: 2,
        snapshots_emitted_second: 0,
        has_backup_restore_path: false,
    };
    let search = DerivedStoreParity {
        store: DerivedStoreClass::Search,
        ..green()
    };
    let refs = DerivedStoreParity {
        store: DerivedStoreClass::Refs,
        ..green()
    };
    let drifted = DerivedStoreParity {
        cold_hash: 99,
        ..green()
    };
    let red = E2e3StorageArtifact::seal(vec![drifted, search, refs]);
    assert_eq!(red.stores_with_drift, 1, "one store drifted");
    assert!(
        !red.is_green(),
        "the gate is RED when a derived store drifts"
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, red.stores_with_drift as i64);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "the harness signal goes RED on drift > 0"
    );
}

#[test]
fn e2e3_gate_goes_red_when_a_derived_store_has_a_backup_restore_path() {
    let leg = |store, backed| DerivedStoreParity {
        store,
        live_hash: 7,
        cold_hash: 7,
        snapshots_emitted_first: 1,
        snapshots_emitted_second: 0,
        has_backup_restore_path: backed,
    };
    let red = E2e3StorageArtifact::seal(vec![
        leg(DerivedStoreClass::Olap, true),
        leg(DerivedStoreClass::Search, false),
        leg(DerivedStoreClass::Refs, false),
    ]);
    assert_eq!(red.derived_stores_with_backup_path, 1);
    assert!(
        !red.is_green(),
        "the gate is RED when a derived store has a backup-restore path"
    );
}

#[test]
fn e2e3_reindex_re_run_is_idempotent() {
    let artifact = run_e2e3_storage_half(&region(), &all_sources(), &ctx_base()).unwrap();
    for leg in &artifact.legs {
        assert!(
            leg.snapshots_emitted_first > 0,
            "{}: the first rebuild emitted snapshots",
            leg.store.name()
        );
        assert_eq!(
            leg.snapshots_emitted_second,
            0,
            "{}: the re-run emitted 0 NEW snapshots",
            leg.store.name()
        );
    }
}
