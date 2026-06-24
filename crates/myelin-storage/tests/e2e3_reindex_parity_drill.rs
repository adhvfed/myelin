//! P-ST-36 (global **P-447**) GATE / DRILL — **E2E-3 (the storage half): cold-reindex == live for the
//! derived stores (OLAP/Search/Refs rebuilt from source); 0 drift; NO backup-restore path.** A dated
//! green artifact.
//!
//! **The GATE (testing-strategy E2E-3 §4.4 + storage.md §3.4/§6/§7.1/§7.3/§7.4):** wipe the derived
//! stores, reindex-from-source to the live offset through the live consumer path (`*.snapshot` replay,
//! 2.6), and assert the rebuilt read model **byte-matches** the live one (the F4 / REF-D4 / SRCH-D5
//! reindex-parity — **no bespoke recovery reader**). The structural truth (§7.1/§7.3): derived stores
//! are **NOT backed up** — reindex-from-source is the ONLY rebuild path (derived == source by
//! construction, no drift). Gate: **cold-reindex == live, 0 drift; 0 derived stores with a
//! backup-restore path.** **STOR-D1/STOR-D2 remain green** — this drill touches no backup/restore code
//! (the derived stores were never in the backup-able set, §7.1). **Never weaken a threshold to pass.**
//!
//! **The load-bearing zero (EI-01 §2):** a derived store that drifts from source after a rebuild serves
//! stale/wrong reads. The defence is STRUCTURAL: every derived store is fed by exactly ONE projection
//! path (the bus consumer's `ingest`), so a live event and a re-emitted `*.snapshot` drive the SAME
//! projection bytes — cold == live by construction. The drill PROVES it: build LIVE, wipe, reindex-from-
//! source through the REAL outbox→relay→bus→consumer path, assert byte-parity (0 drift).
//!
//! **This drill proves the gate can go RED** (a drifting derived store makes `stores_with_drift > 0`;
//! a derived store with a backup-restore path makes `derived_stores_with_backup_path > 0`) **AND green**
//! (the full derived-store set cold-reindex==live, 0 drift, 0 backup paths, certificate sealed), and
//! emits the E2E-3 drift count on the SAME [`SignalSource`] every drill uses (the load-bearing zero).
//!
//! **Relationship to the real Search/Refs reindexers (no duplication — EI-01 §7):** this is the STORAGE
//! half — the proof, in the data layer, that the derived-store CLASS rebuilds cold==live from source
//! with no backup path. The REAL Search (`SearchReindexer` / SRCH-D5) + Refs (`RefsReindexer` / REF-D4)
//! cold==live byte-parity drills are corroborated in the CDC `cdc_e2e3_reindex_parity.rs`. The two
//! proofs MEET; neither re-derives the other. **The E2E-4 DSAR fan-out is the sibling P-ST-35 (P-446).**
//!
//! **FLOORS (named, prompt DoD):** by M5 the reindex-from-source floors are promoted (the OLAP feed
//! P-ST-18, the Search reindexer SRCH-P16, the Refs reindexer REF-P16 are live). What remains
//! designed-not-built — **the generated projection-feeder index measured-trigger** (EI-04 §5: don't add
//! it before the volume is measured) — is named in the honesty register.

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

/// The three reference derived-store sources (OLAP analytics rows / Search index docs / Refs edges).
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

/// **GREEN: the full derived-store set cold-reindex == live (0 drift), NO backup-restore path; the
/// dated E2E-3 artifact is sealed.** The gate's headline. Emits the drift count on the harness signal.
#[test]
fn e2e3_storage_half_is_green_cold_equals_live_zero_drift() {
    let artifact = run_e2e3_storage_half(&region(), &all_sources(), &ctx_base())
        .expect("the E2E-3 storage half runs");

    assert!(
        artifact.is_green(),
        "the E2E-3 storage half is green: {artifact:?}"
    );
    assert_eq!(artifact.stores_with_drift, 0, "0 drift — cold == live");
    assert_eq!(
        artifact.derived_stores_with_backup_path, 0,
        "0 derived stores backed up — reindex-from-source is the only rebuild path (§7.1/§7.3)"
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

    // ── Emit the E2E-3 drift count on the SAME SignalSource every drill uses (the load-bearing zero).
    let mut sig = SignalSource::new();
    sig.set_scalar(
        SignalName::CrossTenantCount,
        artifact.stores_with_drift as i64,
    );
    // E2E-3 GREEN: 0 derived stores drifted (cold == live) — loud-not-swallowed on red.
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    // ── The dated green-artifact line (observability is part of the pass — EI-01 §3).
    println!(
        "[P-447 E2E-3 GREEN 2026-06-24] {} | cert={:016x}",
        artifact.summary(),
        artifact.certificate_hash
    );
}

/// **RED: a drifting derived store flips the gate (the drill proves the gate CAN fail).** A leg whose
/// cold hash diverges from live makes `stores_with_drift > 0` and the artifact RED — a real proof the
/// 0-drift gate is load-bearing, not vacuous.
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
    // Drift the OLAP leg: cold != live.
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

/// **RED: a derived store with a backup-restore path flips the gate (§7.1/§7.3).** Derived stores are
/// NOT backed up; a leg claiming a backup-restore path is a structural contradiction the gate catches.
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
        leg(DerivedStoreClass::Olap, true), // a backup-restore path on a derived store — forbidden.
        leg(DerivedStoreClass::Search, false),
        leg(DerivedStoreClass::Refs, false),
    ]);
    assert_eq!(red.derived_stores_with_backup_path, 1);
    assert!(
        !red.is_green(),
        "the gate is RED when a derived store has a backup-restore path"
    );
}

/// **The re-run is idempotent (the deterministic-`event_id` `ON CONFLICT DO NOTHING`).** Every per-store
/// leg's second reindex emits 0 new snapshots; cold == live stays byte-stable.
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
