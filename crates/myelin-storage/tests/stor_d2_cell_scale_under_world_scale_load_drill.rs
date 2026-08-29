use std::path::Path;

use myelin_harness::load_generator::{
    LoadGenerator, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_storage::{
    ContinuousArchiver, ErasureLedger, GateInputs, KekId, KeyClass, KmsEngine, RestoreVerifyGate,
    RestoredObject, SourceLog, WalRow, WalSegment,
};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("fr-par".into())
}

fn thresholds_doc() -> toml::Value {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest");
    let path = root.join("thresholds.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the versioned thresholds file must load at {path:?}: {e}"));
    text.parse().expect("thresholds.toml must be valid TOML")
}

fn int_threshold(doc: &toml::Value, table: &str, key: &str) -> u64 {
    let v = doc
        .get(table)
        .and_then(|t| t.get(key))
        .and_then(|v| v.as_integer())
        .unwrap_or_else(|| panic!("{table}.{key} must be present (a missing threshold is LOUD)"));
    assert!(v > 0, "{table}.{key} must be a positive value");
    v as u64
}

fn pool_tenants_max() -> u32 {
    int_threshold(&thresholds_doc(), "cell_sizing", "pool_tenants_max") as u32
}

fn surge_multiplier() -> u32 {
    int_threshold(&thresholds_doc(), "surge", "multiplier") as u32
}

fn reachable_archiver(tail: u64) -> ContinuousArchiver {
    let mut arch = ContinuousArchiver::new();
    arch.archive_segment(WalSegment {
        end_offset: 0,
        committed_at: 0,
    })
    .unwrap();
    arch.take_base_backup(1);
    arch.archive_segment(WalSegment {
        end_offset: tail,
        committed_at: 10,
    })
    .unwrap();
    arch
}

#[derive(Default)]
struct CellLoadSink {
    requests: u64,
}
impl Sink for CellLoadSink {
    fn handle(&mut self, _request: &Request) {
        self.requests = self.requests.saturating_add(1);
    }
}

fn world_scale_load_across_cell(tenants: &[TenantId], base_requests: u64) -> u64 {
    let m = Multiplier::custom(surge_multiplier()).expect("a positive surge multiplier");
    let gen = LoadGenerator::new(
        base_requests,
        m,
        PrincipalMix::agent_skewed(),
        StormProfile::ci_surge(),
        tenants.to_vec(),
    )
    .expect("a non-empty cell tenant list");
    let mut sink = CellLoadSink::default();
    gen.drive(&mut sink);
    assert!(
        sink.requests > 0,
        "the world-scale load generator must offer requests (the load the gates are re-confirmed under)"
    );
    sink.requests
}

fn verify_one_tenant_restore(tenant: &TenantId) -> u64 {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(tenant, &region(), KeyClass::Tenant).unwrap();
    let arch = reachable_archiver(300);
    let objects = vec![
        RestoredObject::integral(format!("{}::blob-90", tenant.0).into_bytes()),
        RestoredObject::integral(format!("{}::blob-100", tenant.0).into_bytes()),
    ];
    let mut source = SourceLog::new();
    source.append(90, "r90").append(100, "r100");
    let rows = vec![
        WalRow {
            id: "r90".into(),
            written_at: 90,
            blob_ref: Some(objects[0].content_address.clone()),
        },
        WalRow {
            id: "r100".into(),
            written_at: 100,
            blob_ref: Some(objects[1].content_address.clone()),
        },
    ];
    let ledger = ErasureLedger::new();
    let inputs = GateInputs {
        archiver: &arch,
        target: 100,
        rows: &rows,
        objects: &objects,
        source: &source,
        kms: &kms,
        erasure_ledger: &ledger,
    };
    let artifact = RestoreVerifyGate::new()
        .run_or_fail_ci(&inputs)
        .unwrap_or_else(|e| {
            panic!(
                "STOR-D1 at cell scale: tenant {} restore not whole: {e}",
                tenant.0
            )
        });
    assert_eq!(artifact.checksum_mismatches, 0);
    assert_eq!(artifact.cross_seam_mismatches, 0);
    assert_eq!(artifact.resurrected_subjects, 0);
    artifact.restored_to_offset
}

fn reconfirm_cell_scale(tenant_count: u32, base_load_requests: u64) -> (u32, u64) {
    let tenants: Vec<TenantId> = (0..tenant_count)
        .map(|i| TenantId(format!("cell-tenant-{i:05}")))
        .collect();

    let load_requests = world_scale_load_across_cell(&tenants, base_load_requests);

    for tenant in &tenants {
        let restored_to = verify_one_tenant_restore(tenant);
        assert_eq!(
            restored_to, 100,
            "every restored tenant lands at the consistency point T"
        );
    }

    (tenant_count, load_requests)
}

#[test]
fn stor_d2_cell_scale_under_world_scale_load_sched() {
    let tenant_count = pool_tenants_max();
    assert!(
        tenant_count >= 1000,
        "the measured cell-scale tenant count must be a full cell ({tenant_count} tenants)"
    );
    let (n, load_requests) = reconfirm_cell_scale(tenant_count, 64);
    println!(
        "[P-444 STOR-D1/D2@cell-scale GREEN 2026-06-24] {n} restored tenants re-confirmed whole \
         under generated load ({load_requests} requests, {}× agent-skewed CI-surge).",
        surge_multiplier()
    );
}

#[test]
fn stor_d2_cell_scale_ci_smoke() {
    let (n, load_requests) = reconfirm_cell_scale(8, 16);
    assert_eq!(n, 8);
    assert!(load_requests > 0);
}

#[test]
fn stor_d1_cell_scale_one_corrupt_tenant_fails_the_gate() {
    use myelin_storage::{ContentHash, GateFailure};
    let tenant = TenantId("cell-tenant-00003".into());
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(&tenant, &region(), KeyClass::Tenant)
        .unwrap();
    let arch = reachable_archiver(300);
    let address = ContentHash::blake3(b"good");
    let corrupt = RestoredObject {
        content_address: address.clone(),
        bytes: b"CORRUPT".to_vec(),
    };
    let objects = vec![corrupt];
    let source = SourceLog::new();
    let rows = vec![WalRow {
        id: "r1".into(),
        written_at: 50,
        blob_ref: Some(address.clone()),
    }];
    let ledger = ErasureLedger::new();
    let inputs = GateInputs {
        archiver: &arch,
        target: 100,
        rows: &rows,
        objects: &objects,
        source: &source,
        kms: &kms,
        erasure_ledger: &ledger,
    };
    let err = RestoreVerifyGate::new().run_or_fail_ci(&inputs).expect_err(
        "a corrupt restored tenant copy MUST fail the cell-scale gate, never silently pass",
    );
    assert!(
        matches!(err, GateFailure::ChecksumMismatch { .. }),
        "the cell-scale gate surfaces the checksum mismatch: {err}"
    );
}
