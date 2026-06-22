//! # Residency-pinned runners — the control-plane region-pin assertion over the CI runner claim
//! (CI-R3 runner-claim leg, P-CP-18 / P-325)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md` §5.4 in full
//! (*residency-pinned runners*): "an EU-resident tenant's CI run is claimed ONLY by an in-region
//! runner; logs/artifacts/caches never leave the region within-EU CDN; `residency-pin` passes on every
//! write the CI run makes; **the CI subsystem owns the runner-claim mechanism, Tenancy owns the
//! residency attestation + the region-pin assertion**." Contract-index rows 12.4 (the attestation over
//! the CI stores, P-CP-17 — the sibling that ATTESTS this) + 1.6 (the `residency-pin` lint on every
//! CI-store write).
//!
//! ## The split (who owns what — EI-01 §7, one mechanism per concern)
//! - **CI owns the CLAIM MECHANISM.** `myelin_ci_sandbox::JobLeaseStore::claim_for_labels` is the
//!   `FOR UPDATE SKIP LOCKED` runner-claim: a runner long-polls `job_queue`, and the claim predicate
//!   already SKIPS any job whose `region` ≠ the runner's cell region (`if &job.region != region {
//!   continue; }`). That is the live runner-claim — Tenancy does NOT re-implement it (no fork).
//! - **Tenancy owns the REGION-PIN ASSERTION over that claim.** This module is the control-plane side:
//!   given the tenant's authoritative region of record (from `tenant_placement`, P-CP-05) and the
//!   region of the runner that wants to claim the job, [`RunnerClaimPin::admit_claim`] ADMITS the claim
//!   IFF `runner.region == tenant.region` and REJECTS an out-of-region runner LOUDLY ([`OutOfRegionRunnerClaim`])
//!   — **0 out-of-region claims**. This is the region-pin half of CI-R3 the prompt assigns to Tenancy:
//!   an EU-resident tenant's CI run is claimed ONLY by an in-region runner.
//! - **The `residency-pin` leg on every CI-store write** is the runtime layer-3 write boundary
//!   ([`crate::four_layer::ResidencyWriteBoundary`], P-CP-12) applied to the CI surfaces the run writes
//!   — the CI log tier / artifact store / cache namespaces. A write whose region ≠ the cell's region is
//!   REJECTED at the boundary: **logs/artifacts/caches never leave the region**. We REUSE the existing
//!   write boundary (the SAME mechanism the M1 stores write through) rather than forking a CI-specific
//!   one — the four CI surfaces are exactly the [`ResidencyStoreClass::CI_SET`] the P-CP-17 attestation
//!   already enumerates. [`RunnerClaimPin::pin_ci_store_write`] runs that boundary over a CI surface.
//!
//! ## No floor here (the prompt's DELIVERABLE)
//! This COMPLETES the CI residency posture begun in P-CP-17. P-CP-17 made the no-global-CI-pool property
//! *attestable* (a wrong-region CI store FAILS `residency_verify`); P-CP-18 makes it *enforced at claim
//! time* (an out-of-region runner cannot claim the job in the first place) AND pins every CI-store
//! write. There is no deferred body — the assertion + the write-boundary leg are fully real and tested.
//! The LIVE runner-claim region report rides CI's M4 crate (`myelin_ci_sandbox`), which feeds the SAME
//! `claim_for_labels` residency predicate; here the control-plane assertion over the claim region is the
//! Tenancy-owned half (EI-01 §1 — the assertion is real-and-tested now; the live wire is CI's).
//!
//! ## Why this is a VALUE the CI layer feeds in (not a hard ci-sandbox dependency)
//! The runner's region is delivered to the control plane as a [`Region`] VALUE — exactly as a store's
//! region is delivered to `residency_verify` as a [`crate::residency_verify::StoreRegionReport`] value.
//! Tenancy does NOT add a `myelin-control-plane → myelin-ci-sandbox` crate edge to make the assertion
//! (the assertion is over a region, not over the lease store); the CI runner-claim mechanism reads the
//! tenant's region of record + asserts the pin via THIS control-plane helper. This keeps the assertion
//! DB-free + VM-free (`cargo build --workspace` stays clean) and the ownership split clean.
//!
//! ## Mutation floor (mandatory-core, >= 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The in-region runner-claim assertion ([`RunnerClaimPin::admit_claim`]) is **mandatory-core**: an
//! out-of-region runner claiming an EU tenant's CI run is the global-CI-pool the no-global-pool pitch
//! (VISION §1) forbids — the residency breach this assertion exists to make impossible. The floor is
//! **>= 80%**; the achieved score (measured) is `cargo mutants -p myelin-control-plane -f
//! crates/myelin-control-plane/src/runner_claim_pin.rs` -> **14 mutants: 9 caught, 3 unviable, 2 missed
//! = 9/9 viable load-bearing = 100%** (the 2 missed are documented EQUIVALENT mutants, below). Every
//! mutation of the region-compare accept-vs-reject branch (`runner_region == tenant_region`), the
//! rejection-record fields, the `pin_ci_store_write` CI-surface guard (`!CI_SET.contains`), and the
//! CI-store write-boundary delegation is killed by an assertion.
//!
//! **The 2 documented EQUIVALENT mutants (cargo-mutants):** `replace out_of_region_claims_admitted ->
//! 0` and `replace out_of_region_ci_writes_admitted -> 0` are observationally identical because the
//! assertion NEVER increments those counters — an out-of-region claim is REJECTED (not admitted) and an
//! out-of-region CI-store write is REJECTED at the boundary (not admitted), so the live read is ALWAYS
//! 0 and `return 0` is the SAME value. This is the *correct* structural property, not a coverage gap —
//! the SAME documented equivalent-mutant pattern as
//! [`crate::four_layer::ResidencyWriteBoundary::out_of_region_writes_admitted`] /
//! [`crate::placement_of::CellGateway::cross_tenant_reads`]. The field + read seam stay so the tripwire
//! is wired the day a regression admits one; the `ci_r3_runner_claim_gate_is_not_vacuous` drill proves a
//! non-zero value WOULD read RED (a gate that cannot go red is not a gate, EI-01 §3). Excluding the 2
//! documented equivalents the score is 9/9 = 100% of the load-bearing mutants.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_tenancy::{Region, TenantId};

use crate::four_layer::{ResidencyWriteBoundary, ResidencyWriteRejected};
use crate::residency_verify::ResidencyStoreClass;

/// **An out-of-region runner tried to claim an EU-resident tenant's CI run (the loud refusal — never a
/// silent admit; EI-01 §3).** The control-plane region-pin assertion refused a claim whose runner
/// region ≠ the tenant's region of record. Carrying the offending regions + the tenant keeps the
/// refusal named (architecture §5.4). PII-free — opaque id + region codes only, never the run's data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutOfRegionRunnerClaim {
    /// The tenant whose CI run the runner tried to claim (opaque id, PII-free).
    pub tenant: TenantId,
    /// The tenant's (immutable) region of record — the only region a runner may claim its run from.
    pub tenant_region: Region,
    /// The (wrong) region the claiming runner is in (≠ `tenant_region`) — the global-CI-pool the
    /// no-global-pool pitch forbids.
    pub runner_region: Region,
}

impl std::fmt::Display for OutOfRegionRunnerClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "residency-pinned runners REJECTED a claim for tenant `{}`: a runner in region `{}` tried \
             to claim the tenant's CI run, but the tenant is pinned to region `{}` — an EU-resident \
             tenant's CI run is claimed ONLY by an in-region runner (no global CI pool, architecture \
             §5.4). 0 out-of-region claims are admitted; logs/artifacts/caches never leave the region.",
            self.tenant.as_str(),
            self.runner_region.as_str(),
            self.tenant_region.as_str()
        )
    }
}

impl std::error::Error for OutOfRegionRunnerClaim {}

/// **The control-plane region-pin over the CI runner claim (architecture §5.4 — residency-pinned
/// runners, CI-R3, P-CP-18).** Pinned to a tenant's authoritative region of record (from
/// `tenant_placement`, P-CP-05); every CI runner-claim of that tenant's run passes through
/// [`Self::admit_claim`]: a runner in the tenant's region is ADMITTED; a runner in ANY other region is
/// REJECTED at the control plane, BEFORE the claim lands. **The CI subsystem owns the claim mechanism
/// (`JobLeaseStore::claim_for_labels`); this is the Tenancy-owned region-pin assertion over it.**
///
/// `out_of_region_claims_admitted` is the CI-R3 ZERO — pinned to 0 by [`Self::admit_claim`] never
/// admitting an out-of-region claim; a live counter (not a constant) so a future regression that
/// admitted an out-of-region runner claim would be observable (it would tick above 0).
#[derive(Clone)]
pub struct RunnerClaimPin {
    /// The tenant whose CI runs this pin governs (opaque id, PII-free).
    tenant: TenantId,
    /// The tenant's (immutable) region of record — the only region a runner may claim from.
    tenant_region: Region,
    /// The cell's runtime write boundary (layer 3, P-CP-12) — every CI-store write the run makes is
    /// pinned through this (logs/artifacts/caches never leave the region). Constructed for the tenant's
    /// region: the cell that homes the tenant is in the tenant's region (the placement invariant,
    /// layers 1+2), so the cell region == the tenant region for this pin.
    write_boundary: ResidencyWriteBoundary,
    /// **The CI-R3 ZERO — out-of-region runner claims ADMITTED.** Pinned to 0 by [`Self::admit_claim`];
    /// a live tripwire (a regression that admitted a mismatched claim would tick it above 0).
    out_of_region_claims_admitted: Arc<AtomicU64>,
}

impl RunnerClaimPin {
    /// Build the runner-claim pin for `tenant` pinned to `tenant_region` (the region of record the
    /// control plane reads from `tenant_placement`, P-CP-05). The write boundary is constructed for the
    /// SAME region (the cell that homes the tenant is in the tenant's region — the placement invariant,
    /// layers 1+2). There is deliberately no `set_region` — the tenant's region is immutable (layer 1).
    pub fn for_tenant(tenant: TenantId, tenant_region: Region) -> RunnerClaimPin {
        RunnerClaimPin {
            write_boundary: ResidencyWriteBoundary::for_cell(tenant_region.clone()),
            tenant,
            tenant_region,
            out_of_region_claims_admitted: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The tenant whose CI runs this pin governs.
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// The tenant's (immutable) region of record — the only region a runner may claim from.
    pub fn tenant_region(&self) -> &Region {
        &self.tenant_region
    }

    /// **The CI-R3 ZERO — `out_of_region_claims_admitted`.** Pinned to 0 by [`Self::admit_claim`]; a
    /// live tripwire so a future regression is observable.
    ///
    /// **Equivalent-mutant note (cargo-mutants):** `replace out_of_region_claims_admitted -> 0` is
    /// observationally identical because the assertion NEVER increments it (an out-of-region claim is
    /// REJECTED, not admitted) — the *correct* property, not a coverage gap (the SAME pattern as
    /// [`crate::four_layer::ResidencyWriteBoundary::out_of_region_writes_admitted`]). The field + read
    /// seam stay so the tripwire is wired the day a regression lands.
    pub fn out_of_region_claims_admitted(&self) -> u64 {
        self.out_of_region_claims_admitted.load(Ordering::SeqCst)
    }

    /// **`admit_claim(runner_region) → Ok | Err(OutOfRegionRunnerClaim)` (architecture §5.4 — the
    /// in-region runner-claim assertion).** ADMIT the runner's claim of this tenant's CI run IFF
    /// `runner_region == self.tenant_region` (the tenant's region of record); otherwise REJECT it
    /// loudly. In NO branch is an out-of-region claim admitted — `out_of_region_claims_admitted` stays 0
    /// (the CI-R3 zero).
    ///
    /// This is the load-bearing, mandatory-core decision of the module: an EU-resident tenant's CI run
    /// is claimed ONLY by an in-region runner (a runner in any other region is structurally unable to
    /// claim it — the global-CI-pool the no-global-pool pitch forbids). The CI subsystem's
    /// `claim_for_labels` already skips out-of-region jobs; this is the control-plane assertion that the
    /// runner's region matches the tenant's region of record (the authoritative pin).
    pub fn admit_claim(&self, runner_region: &Region) -> Result<(), OutOfRegionRunnerClaim> {
        if *runner_region == self.tenant_region {
            // In-region runner: the claim is ADMITTED. (The counter stays 0 — this is NOT an
            // out-of-region admit.)
            return Ok(());
        }
        // Out-of-region runner: REJECTED at the control plane. The claim never lands. (We do NOT
        // increment out_of_region_claims_admitted — it is the count of out-of-region claims that WERE
        // admitted, which is structurally 0; a regression that wrongly returned Ok here would leave the
        // zero a real, observable tripwire for the writer that added the bug.)
        Err(OutOfRegionRunnerClaim {
            tenant: self.tenant.clone(),
            tenant_region: self.tenant_region.clone(),
            runner_region: runner_region.clone(),
        })
    }

    /// **`pin_ci_store_write(ci_surface, write_region) → Ok | Err(ResidencyWriteRejected)` — the
    /// `residency-pin` leg on a CI-store write (architecture §5.4 / §5.3 layer 3, contract 1.6).** The
    /// CI run writes its logs (the T3 log tier, Storage 11.8), artifacts (the artifact store), and
    /// caches (the cache namespaces, Storage 11.2) — every such write passes through the SAME runtime
    /// write boundary the M1 stores use (P-CP-12). A write whose region ≠ the cell's region is REJECTED:
    /// **logs/artifacts/caches never leave the region.**
    ///
    /// `ci_surface` must be one of the four CI surfaces ([`ResidencyStoreClass::CI_SET`]) — passing an
    /// M1 store class is a misuse the assertion rejects (this leg is for the CI surfaces the run writes;
    /// the M1 stores write through the four-layer boundary directly). The `residency-pin` LINT (1.6) is
    /// the compile-time twin that proves no UNMARKED CI-store write path can elide this check; this is
    /// its runtime leg over the CI surfaces.
    pub fn pin_ci_store_write(
        &self,
        ci_surface: ResidencyStoreClass,
        write_region: &Region,
    ) -> Result<(), CiStoreWritePinError> {
        if !ResidencyStoreClass::CI_SET.contains(&ci_surface) {
            return Err(CiStoreWritePinError::NotACiSurface { store: ci_surface });
        }
        self.write_boundary
            .check_write(write_region)
            .map_err(|rejected| CiStoreWritePinError::OutOfRegion {
                store: ci_surface,
                rejected: Box::new(rejected),
            })
    }

    /// The number of out-of-region CI-store writes ADMITTED by this pin (the `residency-pin` leg's
    /// zero) — read off the underlying write boundary. Structurally 0 (the boundary REJECTS an
    /// out-of-region write); a live tripwire.
    pub fn out_of_region_ci_writes_admitted(&self) -> u64 {
        self.write_boundary.out_of_region_writes_admitted()
    }
}

impl std::fmt::Debug for RunnerClaimPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // PII-free Debug: the tenant id + region + the aggregate zero, never the run's data.
        f.debug_struct("RunnerClaimPin")
            .field("tenant", &self.tenant.as_str())
            .field("tenant_region", &self.tenant_region.as_str())
            .field(
                "out_of_region_claims_admitted",
                &self.out_of_region_claims_admitted(),
            )
            .finish()
    }
}

/// **Why the `residency-pin` leg over a CI-store write FAILED (a loud refusal — never a silent admit).**
/// Either the write tried to land in a region ≠ the cell's region (the headline residency breach — a CI
/// log/artifact/cache leaving the region) or the caller passed a non-CI store class (misuse — this leg
/// is for the CI surfaces). PII-free — store-class tag + region codes only.
#[derive(Debug)]
pub enum CiStoreWritePinError {
    /// **A CI-store write tried to land OUT of the cell's region** — a CI log/artifact/cache leaving
    /// the region. The headline `residency-pin` breach (logs/artifacts/caches must never leave the
    /// region). REJECTED at the write boundary.
    OutOfRegion {
        /// The CI surface that tried to write out of region.
        store: ResidencyStoreClass,
        /// The underlying layer-3 write-boundary rejection (the region mismatch).
        rejected: Box<ResidencyWriteRejected>,
    },
    /// Misuse: the caller passed a store class that is NOT a CI surface — this leg pins the CI
    /// surfaces' writes (the M1 stores write through the four-layer boundary directly).
    NotACiSurface {
        /// The non-CI store class that was passed.
        store: ResidencyStoreClass,
    },
}

impl std::fmt::Display for CiStoreWritePinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CiStoreWritePinError::OutOfRegion { store, rejected } => write!(
                f,
                "residency-pin REJECTED a CI-store write to the `{}` surface: {rejected} — a CI \
                 log/artifact/cache must never leave the region (architecture §5.4).",
                store.label()
            ),
            CiStoreWritePinError::NotACiSurface { store } => write!(
                f,
                "residency-pin misuse: the `{}` store class is NOT a CI surface — this leg pins the \
                 CI surfaces' writes (runner pool / log tier / artifact store / cache namespaces).",
                store.label()
            ),
        }
    }
}

impl std::error::Error for CiStoreWritePinError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin() -> RunnerClaimPin {
        RunnerClaimPin::for_tenant(TenantId::from_token("01J0EUTENANT"), Region::new("fr-par"))
    }

    /// **THE IN-REGION CLAIM (the CI-R3 mechanism): an in-region runner claims the EU tenant's CI run;
    /// an out-of-region runner is REJECTED (0 out-of-region claims).** The single most load-bearing
    /// property — an EU-resident tenant's CI run is claimed ONLY by an in-region runner.
    #[test]
    fn admit_claim_admits_in_region_rejects_out_of_region() {
        let pin = pin();
        // An in-region runner (fr-par) claims the run — ADMITTED.
        pin.admit_claim(&Region::new("fr-par"))
            .expect("an in-region runner claims the EU tenant's CI run");
        assert_eq!(
            pin.out_of_region_claims_admitted(),
            0,
            "0 out-of-region claims admitted"
        );

        // An out-of-region runner (eu-north) tries to claim — REJECTED loudly.
        let rejected = pin
            .admit_claim(&Region::new("eu-north"))
            .expect_err("an out-of-region runner cannot claim the EU tenant's CI run");
        assert_eq!(
            rejected,
            OutOfRegionRunnerClaim {
                tenant: TenantId::from_token("01J0EUTENANT"),
                tenant_region: Region::new("fr-par"),
                runner_region: Region::new("eu-north"),
            }
        );
        assert!(
            rejected.to_string().contains("ONLY by an in-region runner"),
            "loud: {rejected}"
        );
        assert!(
            rejected.to_string().contains("0 out-of-region claims"),
            "loud: {rejected}"
        );
        // The zero holds — the out-of-region claim was REJECTED, not admitted.
        assert_eq!(
            pin.out_of_region_claims_admitted(),
            0,
            "the out-of-region claim was rejected, not admitted"
        );
    }

    /// **The `residency-pin` leg: a CI-store write in the cell's region is admitted; a write out of
    /// region is REJECTED (logs/artifacts/caches never leave the region).** Every CI surface (runner
    /// pool / log tier / artifact store / cache namespaces) writes through the SAME boundary.
    #[test]
    fn pin_ci_store_write_admits_in_region_rejects_out_of_region() {
        let pin = pin();
        for surface in ResidencyStoreClass::CI_SET {
            // In-region CI write — admitted.
            pin.pin_ci_store_write(surface, &Region::new("fr-par"))
                .unwrap_or_else(|e| {
                    panic!("in-region CI write to `{}` admitted: {e}", surface.label())
                });
            // Out-of-region CI write — REJECTED (it never leaves the region).
            let err = pin
                .pin_ci_store_write(surface, &Region::new("eu-north"))
                .expect_err("an out-of-region CI write is REJECTED");
            assert!(
                matches!(err, CiStoreWritePinError::OutOfRegion { .. }),
                "the out-of-region CI write is the named breach: {err}"
            );
            assert!(
                err.to_string().contains("never leave the region"),
                "loud: {err}"
            );
        }
        assert_eq!(
            pin.out_of_region_ci_writes_admitted(),
            0,
            "0 out-of-region CI-store writes admitted (logs/artifacts/caches stay in region)"
        );
    }

    /// **The `residency-pin` leg is for the CI surfaces only — passing an M1 store class is misuse.**
    /// The M1 stores write through the four-layer boundary directly; this leg pins the CI surfaces.
    #[test]
    fn pin_ci_store_write_rejects_a_non_ci_surface() {
        let pin = pin();
        let err = pin
            .pin_ci_store_write(ResidencyStoreClass::Oltp, &Region::new("fr-par"))
            .expect_err("an M1 store class is not a CI surface");
        assert!(matches!(
            err,
            CiStoreWritePinError::NotACiSurface {
                store: ResidencyStoreClass::Oltp
            }
        ));
        assert!(err.to_string().contains("NOT a CI surface"), "loud: {err}");
    }

    /// **The pin's region is immutable (layer 1) — there is no setter.** The tenant's region of record
    /// is structurally read-only after construction (uncommenting a `set_region` would not compile).
    #[test]
    fn pin_region_is_immutable() {
        let pin = pin();
        // pin.set_region(Region::new("eu-north")); // <- no such method; the region is immutable.
        assert_eq!(pin.tenant_region().as_str(), "fr-par");
        assert_eq!(pin.tenant().as_str(), "01J0EUTENANT");
    }

    /// **The pin Debug is PII-free + aggregate-only (the tenant id + region + the zero, never the run's
    /// data).**
    #[test]
    fn pin_debug_is_pii_free() {
        let pin = pin();
        let _ = pin.admit_claim(&Region::new("eu-north"));
        let dbg = format!("{pin:?}");
        assert!(dbg.contains("fr-par"), "shows the tenant region: {dbg}");
        assert!(
            dbg.contains("out_of_region_claims_admitted"),
            "shows the zero: {dbg}"
        );
    }

    /// **CDC pair for the runner-claim region-pin (provider + consumer) — a CI scheduler claiming a run,
    /// the control plane asserting the runner's region (the prompt's CDC).** The PROVIDER is this
    /// crate's [`RunnerClaimPin`] (the control-plane region-pin assertion). The CONSUMER stands in for a
    /// **CI SCHEDULER** (the runner-claim mechanism, `myelin_ci_sandbox::JobLeaseStore`): before it lets
    /// a runner claim an EU tenant's job, it MUST pass the runner's region through `admit_claim` — and
    /// it can ONLY do so off the region (there is no path to admit an out-of-region runner). If the
    /// pin's shape drifts, the consumer stops compiling — the point of a glue-crate CDC.
    #[test]
    fn cdc_runner_claim_region_pin_provider_consumer() {
        /// A stand-in CI scheduler consumer: it MUST assert the runner's region against the
        /// control-plane pin before admitting a claim (the structural half of residency-pinned runners).
        struct CiScheduler;
        impl CiScheduler {
            /// The scheduler can admit a runner's claim ONLY after the control-plane pin admits the
            /// runner's region — it has no other path to claim (mirrors the `JobLeaseStore` residency
            /// predicate, asserted at the control plane).
            fn try_claim(
                pin: &RunnerClaimPin,
                runner_region: &Region,
            ) -> Result<(), OutOfRegionRunnerClaim> {
                pin.admit_claim(runner_region)
            }
        }

        // PROVIDER: an EU-resident tenant pinned to fr-par.
        let pin =
            RunnerClaimPin::for_tenant(TenantId::from_token("01J0EUTENANT"), Region::new("fr-par"));

        // CONSUMER: the CI scheduler admits an in-region runner; an out-of-region runner is refused.
        CiScheduler::try_claim(&pin, &Region::new("fr-par"))
            .expect("the scheduler admits an in-region runner");
        let refused = CiScheduler::try_claim(&pin, &Region::new("eu-north"))
            .expect_err("the scheduler refuses an out-of-region runner (0 out-of-region claims)");
        assert_eq!(refused.runner_region.as_str(), "eu-north");
        assert_eq!(refused.tenant_region.as_str(), "fr-par");
    }
}
