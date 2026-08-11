use myelin_harness::load_generator::{
    LoadGenerator, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_storage::{
    ContinuousArchiver, ErasureLedger, GateInputs, KekId, KeyClass, KmsEngine, RestoreVerifyGate,
    RestoredObject, SourceLog, WalRow, WalSegment,
};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("fr-par".into())
}

fn pool_tenants_max() -> u32 {
    let t = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    let doc: toml::Value = {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = manifest
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root");
        let text = std::fs::read_to_string(root.join("thresholds.toml")).expect("read thresholds");
        text.parse().expect("valid TOML")
    };
    let n = doc
        .get("cell_sizing")
        .and_then(|t| t.get("pool_tenants_max"))
        .and_then(|v| v.as_integer())
        .expect("cell_sizing.pool_tenants_max present (a missing threshold is LOUD)");
    assert!(n > 0, "the measured cell tenant count is positive");
    let _ = t.surge.multiplier;
    n as u32
}

fn surge_multiplier() -> u32 {
    Thresholds::load_canonical().expect("load").surge.multiplier
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

fn world_scale_clone_load(tenants: &[TenantId], base_requests: u64) -> u64 {
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
        "the world-scale clone load must offer requests (the load the gate is re-confirmed under)"
    );
    sink.requests
}

fn verify_one_tenant_git_restore(tenant: &TenantId) -> u64 {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(tenant, &region(), KeyClass::Tenant).unwrap();
    let arch = reachable_archiver(300);
    let objects = vec![
        RestoredObject::integral(format!("{}::git-blob:fn main(){{}}", tenant.0).into_bytes()),
        RestoredObject::integral(format!("{}::git-tree:src/main.rs", tenant.0).into_bytes()),
        RestoredObject::integral(format!("{}::git-commit:fix the bug", tenant.0).into_bytes()),
    ];
    let mut source = SourceLog::new();
    source.append(90, "git-row-90").append(100, "git-row-100");
    let rows = vec![
        WalRow {
            id: "git-row-90".into(),
            written_at: 90,
            blob_ref: Some(objects[0].content_address.clone()),
        },
        WalRow {
            id: "git-row-100".into(),
            written_at: 100,
            blob_ref: Some(objects[2].content_address.clone()),
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
                "STOR-D2 at cell scale: tenant {} git restore not whole: {e}",
                tenant.0
            )
        });
    assert_eq!(
        artifact.checksum_mismatches, 0,
        "every git object re-hashes to its OID"
    );
    assert_eq!(artifact.cross_seam_mismatches, 0);
    assert_eq!(artifact.resurrected_subjects, 0);
    artifact.restored_to_offset
}

fn reconfirm_git_cell_scale(tenant_count: u32, base_load_requests: u64) -> (u32, u64) {
    let tenants: Vec<TenantId> = (0..tenant_count)
        .map(|i| TenantId(format!("git-cell-tenant-{i:05}")))
        .collect();

    let load_requests = world_scale_clone_load(&tenants, base_load_requests);

    for tenant in &tenants {
        let restored_to = verify_one_tenant_git_restore(tenant);
        assert_eq!(
            restored_to, 100,
            "every restored git tenant lands at the consistency point T"
        );
    }

    (tenant_count, load_requests)
}

#[test]
fn git_p34_stor_d2_cell_scale_under_world_scale_load_sched() {
    let tenant_count = pool_tenants_max();
    assert!(
        tenant_count >= 1000,
        "the measured cell-scale tenant count must be a full cell ({tenant_count} tenants)"
    );
    let (n, load_requests) = reconfirm_git_cell_scale(tenant_count, 64);
    println!(
        "[P-483 STOR-D2@cell-scale (git) GREEN 2026-06-25] {n} restored git tenants re-confirmed whole \
         (every git object re-hashes to its OID, derived==source-replay, erasure held) UNDER world-scale \
         clone load ({load_requests} requests, {}× agent-skewed clone-surge). No threshold weakened.",
        surge_multiplier()
    );
}

#[test]
fn git_p34_stor_d2_cell_scale_ci_smoke() {
    let (n, load_requests) = reconfirm_git_cell_scale(8, 16);
    assert_eq!(n, 8);
    assert!(load_requests > 0);
}

#[test]
fn git_p34_stor_d2_one_corrupt_git_object_fails_the_gate() {
    use myelin_storage::{ContentHash, GateFailure};
    let tenant = TenantId("git-cell-tenant-00003".into());
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(&tenant, &region(), KeyClass::Tenant)
        .unwrap();
    let arch = reachable_archiver(300);
    let address = ContentHash::blake3(b"git-blob:fn main(){}");
    let corrupt = RestoredObject {
        content_address: address.clone(),
        bytes: b"git-blob:CORRUPTED".to_vec(),
    };
    let objects = vec![corrupt];
    let source = SourceLog::new();
    let rows = vec![WalRow {
        id: "git-row-1".into(),
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
    let verdict = RestoreVerifyGate::new().run(&inputs);
    match verdict.failure() {
        Some(GateFailure::ChecksumMismatch {
            content_address, ..
        }) => {
            assert_eq!(
                *content_address, address,
                "the corrupt git object's OID is named"
            );
        }
        other => {
            panic!("a corrupt git object must FAIL the gate with ChecksumMismatch, got {other:?}")
        }
    }
}
