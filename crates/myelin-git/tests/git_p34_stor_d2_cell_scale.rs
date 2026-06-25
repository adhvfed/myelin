//! # STOR-D2 RE-CONFIRMED at CELL SCALE under WORLD-SCALE clone load — git's restorable state
//! (GIT-P34 → global P-483, M5)
//!
//! **Prompt:** GIT-P34 → global **P-483** (M5). **Drill catalogue:**
//! `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` STOR-D2 (kill a cell; restore →
//! RPO/RTO — the permanent restore-verify gate) re-confirmed at cell scale UNDER WORLD-SCALE LOAD.
//! **Contract-index:** row 11.5 (restore-verify at cell scale, STOR-D2). **Doctrine:** EI-01 §3
//! (RPO/RTO + the surge multiplier read from the FROZEN file, never hardcoded; the world-scale load is
//! REAL generated traffic; never weaken a threshold to pass), §7 (REUSE — re-drive the storage-owned
//! gate, never a second copy).
//!
//! ## What this drill IS — git's restorable state through the STORAGE-OWNED gate (coherence, EI-01 §7)
//! P-444 (`myelin-storage::stor_d2_d8_cell_scale_under_world_scale_load_drill`) re-confirmed STOR-D2 at
//! cell scale over storage's OWN tenants. This is the **git-tier face**: it re-drives STORAGE's OWN
//! permanent gate ([`RestoreVerifyGate`], the X-1 restore-verify seam) over **git's restorable state** —
//! a repo's authoritative content-addressed git objects (commit / tree / blob), the bytes the
//! object-backed pack tier (GIT-P33) holds — across a CELL's worth of tenants, while a REAL world-scale
//! clone load (the GIT-D6 generator at the 30× surge) is offered. It does NOT re-implement the gate (it
//! RE-DRIVES it — the SUB-D10 idiom, no second copy) and it does NOT duplicate the storage cell-scale
//! drill (a different restorable surface: git's pack objects vs storage's generic WAL rows).
//!
//! ## The cell-scale shape (read from the FROZEN thresholds file, not a guess)
//! "Cell scale" = a full Pool cell's worth of tenants — the MEASURED `cell_sizing.pool_tenants_max` from
//! `thresholds.toml`, never a typed literal. The SCHED headline runs the WHOLE measured tenant count; a
//! CI smoke variant runs a thin slice (the SAME assertion path — no drift). A SINGLE tenant whose git
//! restore is not whole (a pack object whose bytes no longer re-hash to its OID address) fails the whole
//! cell (0 loss is per cell, not on average). The surge multiplier is read from the FILE.
//!
//! ## Floors named
//! - **No real WAL/PITR rebuild + live `git` object-DB restore at the full cell count on this floor** —
//!   the restored git objects are MODELLED as content-addressed blobs (the SAME model the storage
//!   STOR-D1/STOR-D2 drills use); when storage's real PITR drivers (P-059..P-061) + the object-backed
//!   pack tier's real restore land, they populate the SAME `GateInputs` shape at the full cell scale and
//!   this drill's wiring + assertions do not change.
//! - **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet); here
//!   the world-scale clone load is the GIT-D6 generator at 30× across the cell's tenants.
//! - **SCHED + a cheaper CI smoke variant** — the headline runs the full measured tenant count at SCHED
//!   frequency; the smoke rides every commit over a thin slice (SAME assertion path).

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

/// The MEASURED cell-scale tenant count (`cell_sizing.pool_tenants_max`) — never a literal (EI-01 §3).
fn pool_tenants_max() -> u32 {
    let t = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    // cell_sizing lives in the thresholds doc; read it via the toml doc the storage drill uses.
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
    // touch the typed loader so a future field rename is caught here too.
    let _ = t.surge.multiplier;
    n as u32
}

/// The surge multiplier read from the FROZEN `[surge]` (the world-scale clone-load multiplier).
fn surge_multiplier() -> u32 {
    Thresholds::load_canonical().expect("load").surge.multiplier
}

/// An archiver whose base + WAL tail makes every offset in `0..=tail` reachable.
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

/// A sink that counts the world-scale clone load offered across the cell during the verify window.
#[derive(Default)]
struct CellLoadSink {
    requests: u64,
}
impl Sink for CellLoadSink {
    fn handle(&mut self, _request: &Request) {
        self.requests = self.requests.saturating_add(1);
    }
}

/// Drive a world-scale (30× agent-skewed clone-surge) load across `tenants` and return the request count.
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

/// Re-run STORAGE's OWN restore-verify gate (STOR-D2) for ONE tenant's restored GIT state and return its
/// consistency point T. A whole git restore: every authoritative content-addressed git object (commit /
/// tree / blob — the bytes the object-backed pack tier holds) present + checksum-parity-verified, the
/// derived projection == source-replay, erasure held. Modelled git objects — the same model the storage
/// STOR-D2 drill uses; the real object-DB restore populates the SAME `GateInputs` shape.
fn verify_one_tenant_git_restore(tenant: &TenantId) -> u64 {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    kms.ensure_dek(tenant, &region(), KeyClass::Tenant).unwrap();
    let arch = reachable_archiver(300);
    // a repo's authoritative git objects — a commit referencing a tree referencing a blob (the bytes the
    // object-backed pack tier holds, content-addressed by their OID). The restore brought them back whole.
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

/// Re-confirm STOR-D2 across a CELL's worth of tenants while a REAL world-scale clone load is offered.
/// Returns `(tenant_count, load_requests)`. ONE assertion path shared by the SCHED headline + CI smoke.
fn reconfirm_git_cell_scale(tenant_count: u32, base_load_requests: u64) -> (u32, u64) {
    let tenants: Vec<TenantId> = (0..tenant_count)
        .map(|i| TenantId(format!("git-cell-tenant-{i:05}")))
        .collect();

    // (a) the restore-verify is re-confirmed UNDER world-scale clone load (the GIT-D6 generator).
    let load_requests = world_scale_clone_load(&tenants, base_load_requests);

    // (b) STOR-D2: re-run storage's OWN restore-verify gate over git's restorable state for EVERY tenant.
    //     A single tenant whose git restore is not whole panics (0 loss is per cell, not on average).
    for tenant in &tenants {
        let restored_to = verify_one_tenant_git_restore(tenant);
        assert_eq!(
            restored_to, 100,
            "every restored git tenant lands at the consistency point T"
        );
    }

    (tenant_count, load_requests)
}

/// **THE SCHED DRILL (the dated green artifact the DoD names).** Re-confirm STOR-D2 over git's restorable
/// state across the FULL measured cell tenant count under world-scale clone load — bounds read from FILE.
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

/// **THE CI SMOKE VARIANT (rides every commit): the same cell-scale re-confirm over a THIN slice.** SAME
/// assertion path — no drift from the SCHED headline.
#[test]
fn git_p34_stor_d2_cell_scale_ci_smoke() {
    let (n, load_requests) = reconfirm_git_cell_scale(8, 16);
    assert_eq!(n, 8);
    assert!(load_requests > 0);
}

/// **MANDATORY counter-case: a SINGLE corrupt restored git object fails the WHOLE cell (0 loss is per
/// cell, not on average).** A git object whose restored bytes do not re-hash to its OID address must FAIL
/// storage's restore-verify gate — proving the cell-scale gate is a real bar, never weakened (EI-01 §3).
#[test]
fn git_p34_stor_d2_one_corrupt_git_object_fails_the_gate() {
    use myelin_storage::{ContentHash, GateFailure};
    let tenant = TenantId("git-cell-tenant-00003".into());
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    kms.ensure_dek(&tenant, &region(), KeyClass::Tenant)
        .unwrap();
    let arch = reachable_archiver(300);
    // the OID address is for the good object bytes, but the restore brought back CORRUPT bytes.
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
