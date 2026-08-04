use myelin_control_plane::{
    Capacity, Cell, CellStatus, IsolationKind, MirrorDecision, MirrorGate, MirrorTarget,
    PlacementStatus, Registry, TenantPlacement, TransferPolicy,
};
use myelin_gdpr_service::TransferGate;
use myelin_tenancy::{CellId, Region, TenantId};

struct GdprTransferPolicy<'a> {
    gate: &'a TransferGate,
}

impl TransferPolicy for GdprTransferPolicy<'_> {
    fn transfer_allowed(&self, target: &Region) -> bool {
        self.gate.transfer_allowed(target).is_allowed()
    }
}

struct GitMirrorFeature {
    pushes_made: u64,
}

impl GitMirrorFeature {
    fn new() -> GitMirrorFeature {
        GitMirrorFeature { pushes_made: 0 }
    }

    fn try_mirror_push(
        &mut self,
        gate: &mut MirrorGate,
        registry: &Registry,
        tenant: &TenantId,
        target: &MirrorTarget,
        policy: &dyn TransferPolicy,
    ) -> MirrorDecision {
        let decision = gate.mirror_allowed(registry, tenant, target, policy);
        if decision.is_allowed() {
            self.pushes_made += 1;
        }
        decision
    }
}

fn registry_with(tenant: &str, region: &str, home: &str) -> Registry {
    let mut reg = Registry::new();
    reg.insert_cell(Cell {
        cell_id: CellId::from_token(home),
        region: Region::new(region),
        status: CellStatus::Active,
        isolation_kind: IsolationKind::Pool,
        capacity: Capacity {
            tenants_max: 1000,
            write_qps_max: 5000,
            storage_bytes_max: 1 << 40,
        },
        utilisation: 10,
        version: 1,
        endpoint: format!("cell.{region}.{home}.myelin.eu"),
    });
    reg.place_tenant(TenantPlacement {
        tenant_id: TenantId::from_token(tenant),
        region: Region::new(region),
        home_cell: CellId::from_token(home),
        isolation_tier: IsolationKind::Pool,
        slug: tenant.into(),
        status: PlacementStatus::Active,
        member_cells: vec![CellId::from_token(home)],
    })
    .expect("a single-region placement is admitted");
    reg
}

#[test]
fn cdc_10_5_mirror_gate_git_feature_honours_deny_by_default() {
    let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
    let real_gate = TransferGate::new();
    let policy = GdprTransferPolicy { gate: &real_gate };
    let mut mirror_gate = MirrorGate::new();
    let mut feature = GitMirrorFeature::new();
    let acme = TenantId::from_token("01J0ACME");

    let extra_eu = MirrorTarget::new("github.com", Region::new("us-east"));
    let denied = feature.try_mirror_push(&mut mirror_gate, &reg, &acme, &extra_eu, &policy);
    assert!(
        !denied.is_allowed(),
        "the REAL transfer_allowed denies extra-EU by default → the gate denies"
    );
    assert!(
        matches!(denied, MirrorDecision::Deny { .. }),
        "a crossing push without a transfer_allowed entry is REFUSED (loud), not logged-and-allowed"
    );
    assert_eq!(
        feature.pushes_made, 0,
        "the Git mirror feature did NOT push (honours the deny)"
    );
    assert_eq!(
        mirror_gate.unauthorised_pushes_prevented(),
        1,
        "the prevented unauthorised cross-residency push is counted (the C-4 zero)"
    );

    real_gate.record_transfer_mechanism(Region::new("us-east"));
    let allowed = feature.try_mirror_push(&mut mirror_gate, &reg, &acme, &extra_eu, &policy);
    assert!(
        allowed.is_allowed(),
        "an extra-EU target WITH a recorded transfer mechanism is permitted"
    );
    assert_eq!(
        feature.pushes_made, 1,
        "the feature pushes on the now-lawful transfer"
    );
    assert_eq!(
        mirror_gate.unauthorised_pushes_prevented(),
        1,
        "the allow did not bump the prevented-push counter - still the one deny"
    );

    println!(
        "[P-251 CDC 10.5 mirror-half GREEN 2026-06-21] the Git mirror feature ⇄ mirror_allowed over \
         the REAL GDPR transfer_allowed: an extra-EU host (us-east) WITHOUT a transfer_allowed entry \
         was DENIED (deny-by-default) and the feature did NOT push (pushes_made=0); recording a lawful \
         basis flipped the SAME target to ALLOWED (pushes_made=1). The control plane decided the \
         residency-boundary crossing; GDPR's REAL policy decided lawfulness. FLOOR: the counsel-ratified \
         transfer_allowed entries are [OPEN - LEGAL] (Schrems II / Art. 44-49), a parallel legal track."
    );
}

#[test]
fn cdc_10_5_mirror_gate_same_region_and_within_eu_allowed() {
    let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
    let real_gate = TransferGate::new();
    let policy = GdprTransferPolicy { gate: &real_gate };
    let mut mirror_gate = MirrorGate::new();
    let mut feature = GitMirrorFeature::new();
    let acme = TenantId::from_token("01J0ACME");

    let same = MirrorTarget::new("git.acme.internal.fr", Region::new("fr-par"));
    assert!(feature
        .try_mirror_push(&mut mirror_gate, &reg, &acme, &same, &policy)
        .is_allowed());

    let within_eu = MirrorTarget::new("mirror.nl.example", Region::new("nl-ams"));
    assert!(feature
        .try_mirror_push(&mut mirror_gate, &reg, &acme, &within_eu, &policy)
        .is_allowed());

    assert_eq!(
        feature.pushes_made, 2,
        "both the same-region and within-EU mirrors push"
    );
    assert_eq!(
        mirror_gate.unauthorised_pushes_prevented(),
        0,
        "no unauthorised cross-residency push - the within-EU set is lawful (§5.3)"
    );
}
