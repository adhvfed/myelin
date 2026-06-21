//! # Contract 10.5 (the outbound-mirror half, C-4) CDC pair — the **Git mirror feature calling
//! `mirror_allowed`**, with GDPR's REAL `transfer_allowed` as the lawful-transfer half
//!
//! **DATED GREEN ARTIFACT (2026-06-21).** P-CP-16 / P-251. This file is the consumer-driven contract
//! pair for the **mirror half** of contract 10.5 (the outbound push-mirror residency gate,
//! `mirror_allowed(tenant_id, mirror_target) → Allow | Deny{reason}`, architecture §7.4 / §5.3).
//!
//! ## The CDC pair (contract 10.5, mirror half — C-4)
//! - **PROVIDER** = `myelin-control-plane` — [`MirrorGate::mirror_allowed`] (the control plane's
//!   residency-boundary decision: same-region ⇒ no crossing; cross-region ⇒ gated, consulting the
//!   GDPR transfer policy), wired here over the **REAL** GDPR `transfer_allowed` half.
//! - **PROVIDER (the lawful-transfer half)** = `myelin-gdpr-service` — the REAL
//!   [`myelin_gdpr_service::TransferGate`] (`transfer_allowed`: deny extra-EU by default; admit
//!   within-EU/EEA; admit an extra-EU target ONLY with a recorded transfer mechanism). The control
//!   plane consumes it through the [`myelin_control_plane::TransferPolicy`] port — the production DAG
//!   takes NO runtime edge to the service crate (this DEV-only CDC test is where the two halves meet).
//! - **CONSUMER** = the **Git mirror feature** — modelled here by [`GitMirrorFeature`], the producer
//!   subsystem (a Git mirror config that pushes a repo to a foreign host). It asks `mirror_allowed`
//!   BEFORE pushing and **HONOURS the deny** (it does not push PII-bearing content on a `Deny`).
//!
//! **Why both consumer + the GDPR half are modelled/wired in-test (documented deviation, EI-01 §1):**
//! `myelin-git` is a leaf SERVICE crate ABOVE the control plane in the §2.9 DAG, so the control plane
//! cannot depend back on it (an upward edge) — the CONSUMER is exercised here by a faithful
//! [`GitMirrorFeature`] using ONLY the public `mirror_allowed` surface the real Git mirror feature
//! calls. The GDPR `TransferGate` IS the real provider (a DEV-only dep, the same edge the existing
//! `cdc_10_8` test uses) wrapped in a thin [`GdprTransferPolicy`] adapter implementing the control
//! plane's `TransferPolicy` port — so the **real** deny-extra-EU-by-default policy drives the gate. If
//! either half drifts (the gate stops denying a crossing without a `transfer_allowed` entry, or the
//! GDPR policy stops denying extra-EU by default), this stops passing — that is the contract.
//!
//! ## What the pair proves
//! 1. A Git mirror to an **extra-EU host WITHOUT a `transfer_allowed` entry** is **denied** by the
//!    real policy → the Git mirror feature does NOT push (0 unauthorised cross-residency pushes).
//! 2. Recording a transfer mechanism (the `[OPEN — LEGAL]` counsel-ratified entry) flips that SAME
//!    target to **allowed** — the gate consults the registry, it is not a blanket block.
//! 3. A **same-region** mirror is allowed without crossing the boundary; a **within-EU cross-region**
//!    mirror is allowed via the policy (within-EU acceleration, §5.3).

use myelin_control_plane::{
    Capacity, Cell, CellStatus, IsolationKind, MirrorDecision, MirrorGate, MirrorTarget,
    PlacementStatus, Registry, TenantPlacement, TransferPolicy,
};
use myelin_gdpr_service::TransferGate;
use myelin_tenancy::{CellId, Region, TenantId};

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// THE LAWFUL-TRANSFER HALF — the REAL GDPR `TransferGate`, adapted to the control plane's
// `TransferPolicy` port. The control plane owns the residency-boundary decision; this is GDPR's
// "is this transfer lawful?" half (§7.4 ownership split). No re-implementation — the real gate.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// Adapts the REAL [`myelin_gdpr_service::TransferGate`] to the control plane's [`TransferPolicy`]
/// port: `transfer_allowed(target)` is `true` iff GDPR's gate verdicts the transfer Allowed (within-EU,
/// or extra-EU with a recorded mechanism). The control plane consumes this — it never re-derives it.
struct GdprTransferPolicy<'a> {
    gate: &'a TransferGate,
}

impl TransferPolicy for GdprTransferPolicy<'_> {
    fn transfer_allowed(&self, target: &Region) -> bool {
        // The REAL deny-extra-EU-by-default policy drives the verdict.
        self.gate.transfer_allowed(target).is_allowed()
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// THE CONSUMER — the Git mirror feature (the producer subsystem, §7.4). It asks `mirror_allowed`
// BEFORE pushing and HONOURS the deny. It uses ONLY the public provider surface the real feature calls.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// The Git outbound push-mirror feature (the producer subsystem). It is configured with a mirror
/// target and, before each push, asks the control plane `mirror_allowed`; it pushes ONLY on an Allow.
/// `pushes_made` counts the pushes it actually sent to the wire (the C-4 zero: 0 unauthorised
/// cross-residency pushes).
struct GitMirrorFeature {
    /// The number of pushes this feature actually sent to a foreign host (the wire).
    pushes_made: u64,
}

impl GitMirrorFeature {
    fn new() -> GitMirrorFeature {
        GitMirrorFeature { pushes_made: 0 }
    }

    /// Attempt an outbound push-mirror to `target` for `tenant`. Asks the PROVIDER `mirror_allowed`
    /// first and HONOURS the verdict: pushes (bumps `pushes_made`) ONLY on an Allow; on a Deny it does
    /// NOT push (the byte never reaches the foreign host). Returns the gate's decision (for assertions).
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
            // The mirror is permitted — the feature pushes the repo to the (in-region / lawful) host.
            self.pushes_made += 1;
        }
        // else: the feature HONOURS the deny — it does NOT push PII-bearing content.
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
        capacity: Capacity { tenants_max: 1000, write_qps_max: 5000, storage_bytes_max: 1 << 40 },
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

/// **THE CDC PAIR (dated green artifact): the Git mirror feature ⇄ `mirror_allowed` over the REAL
/// `transfer_allowed`.** Proves the producer honours the gate's deny-by-default for an extra-EU host,
/// and that recording a lawful basis flips it — both driven by the REAL GDPR policy.
#[test]
fn cdc_10_5_mirror_gate_git_feature_honours_deny_by_default() {
    let reg = registry_with("01J0ACME", "fr-par", "cell-w-1"); // ACME is EU-resident (fr-par).
    let real_gate = TransferGate::new(); // the REAL GDPR transfer policy — deny extra-EU by default.
    let policy = GdprTransferPolicy { gate: &real_gate };
    let mut mirror_gate = MirrorGate::new();
    let mut feature = GitMirrorFeature::new();
    let acme = TenantId::from_token("01J0ACME");

    // ── (1) An extra-EU host WITHOUT a transfer_allowed entry → DENIED by the REAL policy. The Git
    //    mirror feature HONOURS the deny — 0 pushes to the foreign host (the C-4 zero). ──
    let extra_eu = MirrorTarget::new("github.com", Region::new("us-east"));
    let denied = feature.try_mirror_push(&mut mirror_gate, &reg, &acme, &extra_eu, &policy);
    assert!(!denied.is_allowed(), "the REAL transfer_allowed denies extra-EU by default → the gate denies");
    assert!(
        matches!(denied, MirrorDecision::Deny { .. }),
        "a crossing push without a transfer_allowed entry is REFUSED (loud), not logged-and-allowed"
    );
    assert_eq!(feature.pushes_made, 0, "the Git mirror feature did NOT push (honours the deny)");
    assert_eq!(
        mirror_gate.unauthorised_pushes_prevented(),
        1,
        "the prevented unauthorised cross-residency push is counted (the C-4 zero)"
    );

    // ── (2) Record a lawful basis (the `[OPEN — LEGAL]` counsel-ratified entry) on the REAL gate →
    //    the SAME extra-EU target flips to ALLOWED; the feature now pushes (a lawful transfer). ──
    real_gate.record_transfer_mechanism(Region::new("us-east"));
    let allowed = feature.try_mirror_push(&mut mirror_gate, &reg, &acme, &extra_eu, &policy);
    assert!(allowed.is_allowed(), "an extra-EU target WITH a recorded transfer mechanism is permitted");
    assert_eq!(feature.pushes_made, 1, "the feature pushes on the now-lawful transfer");
    assert_eq!(
        mirror_gate.unauthorised_pushes_prevented(),
        1,
        "the allow did not bump the prevented-push counter — still the one deny"
    );

    println!(
        "[P-251 CDC 10.5 mirror-half GREEN 2026-06-21] the Git mirror feature ⇄ mirror_allowed over \
         the REAL GDPR transfer_allowed: an extra-EU host (us-east) WITHOUT a transfer_allowed entry \
         was DENIED (deny-by-default) and the feature did NOT push (pushes_made=0); recording a lawful \
         basis flipped the SAME target to ALLOWED (pushes_made=1). The control plane decided the \
         residency-boundary crossing; GDPR's REAL policy decided lawfulness. FLOOR: the counsel-ratified \
         transfer_allowed entries are [OPEN — LEGAL] (Schrems II / Art. 44-49), a parallel legal track."
    );
}

/// **The same-region + within-EU legs over the REAL policy.** A same-region mirror crosses no boundary
/// (allowed, no policy consult); a within-EU cross-region mirror is allowed via the REAL policy
/// (within-EU acceleration, §5.3) — both push, neither is an unauthorised cross-residency push.
#[test]
fn cdc_10_5_mirror_gate_same_region_and_within_eu_allowed() {
    let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
    let real_gate = TransferGate::new();
    let policy = GdprTransferPolicy { gate: &real_gate };
    let mut mirror_gate = MirrorGate::new();
    let mut feature = GitMirrorFeature::new();
    let acme = TenantId::from_token("01J0ACME");

    // Same region (fr-par → fr-par): no crossing, allowed.
    let same = MirrorTarget::new("git.acme.internal.fr", Region::new("fr-par"));
    assert!(feature.try_mirror_push(&mut mirror_gate, &reg, &acme, &same, &policy).is_allowed());

    // Within-EU cross-region (fr-par → nl-ams): crosses the tenant's region but the REAL policy admits
    // within-EU structurally (within-EU acceleration, §5.3).
    let within_eu = MirrorTarget::new("mirror.nl.example", Region::new("nl-ams"));
    assert!(feature.try_mirror_push(&mut mirror_gate, &reg, &acme, &within_eu, &policy).is_allowed());

    assert_eq!(feature.pushes_made, 2, "both the same-region and within-EU mirrors push");
    assert_eq!(
        mirror_gate.unauthorised_pushes_prevented(),
        0,
        "no unauthorised cross-residency push — the within-EU set is lawful (§5.3)"
    );
}
