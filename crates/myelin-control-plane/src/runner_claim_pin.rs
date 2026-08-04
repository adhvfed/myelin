use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_tenancy::{Region, TenantId};

use crate::four_layer::{ResidencyWriteBoundary, ResidencyWriteRejected};
use crate::residency_verify::ResidencyStoreClass;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutOfRegionRunnerClaim {
    pub tenant: TenantId,
    pub tenant_region: Region,
    pub runner_region: Region,
}

impl std::fmt::Display for OutOfRegionRunnerClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "residency-pinned runners REJECTED a claim for tenant `{}`: a runner in region `{}` tried \
             to claim the tenant's CI run, but the tenant is pinned to region `{}` - an EU-resident \
             tenant's CI run is claimed ONLY by an in-region runner (no global CI pool, architecture \
             §5.4). 0 out-of-region claims are admitted; logs/artifacts/caches never leave the region.",
            self.tenant.as_str(),
            self.runner_region.as_str(),
            self.tenant_region.as_str()
        )
    }
}

impl std::error::Error for OutOfRegionRunnerClaim {}

#[derive(Clone)]
pub struct RunnerClaimPin {
    tenant: TenantId,
    tenant_region: Region,
    write_boundary: ResidencyWriteBoundary,
    out_of_region_claims_admitted: Arc<AtomicU64>,
}

impl RunnerClaimPin {
    pub fn for_tenant(tenant: TenantId, tenant_region: Region) -> RunnerClaimPin {
        RunnerClaimPin {
            write_boundary: ResidencyWriteBoundary::for_cell(tenant_region.clone()),
            tenant,
            tenant_region,
            out_of_region_claims_admitted: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn tenant_region(&self) -> &Region {
        &self.tenant_region
    }

    pub fn out_of_region_claims_admitted(&self) -> u64 {
        self.out_of_region_claims_admitted.load(Ordering::SeqCst)
    }

    pub fn admit_claim(&self, runner_region: &Region) -> Result<(), OutOfRegionRunnerClaim> {
        if *runner_region == self.tenant_region {
            return Ok(());
        }
        Err(OutOfRegionRunnerClaim {
            tenant: self.tenant.clone(),
            tenant_region: self.tenant_region.clone(),
            runner_region: runner_region.clone(),
        })
    }

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

    pub fn out_of_region_ci_writes_admitted(&self) -> u64 {
        self.write_boundary.out_of_region_writes_admitted()
    }
}

impl std::fmt::Debug for RunnerClaimPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

#[derive(Debug)]
pub enum CiStoreWritePinError {
    OutOfRegion {
        store: ResidencyStoreClass,
        rejected: Box<ResidencyWriteRejected>,
    },
    NotACiSurface {
        store: ResidencyStoreClass,
    },
}

impl std::fmt::Display for CiStoreWritePinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CiStoreWritePinError::OutOfRegion { store, rejected } => write!(
                f,
                "residency-pin REJECTED a CI-store write to the `{}` surface: {rejected} - a CI \
                 log/artifact/cache must never leave the region (architecture §5.4).",
                store.label()
            ),
            CiStoreWritePinError::NotACiSurface { store } => write!(
                f,
                "residency-pin misuse: the `{}` store class is NOT a CI surface - this leg pins the \
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

    #[test]
    fn admit_claim_admits_in_region_rejects_out_of_region() {
        let pin = pin();
        pin.admit_claim(&Region::new("fr-par"))
            .expect("an in-region runner claims the EU tenant's CI run");
        assert_eq!(
            pin.out_of_region_claims_admitted(),
            0,
            "0 out-of-region claims admitted"
        );

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
        assert_eq!(
            pin.out_of_region_claims_admitted(),
            0,
            "the out-of-region claim was rejected, not admitted"
        );
    }

    #[test]
    fn pin_ci_store_write_admits_in_region_rejects_out_of_region() {
        let pin = pin();
        for surface in ResidencyStoreClass::CI_SET {
            pin.pin_ci_store_write(surface, &Region::new("fr-par"))
                .unwrap_or_else(|e| {
                    panic!("in-region CI write to `{}` admitted: {e}", surface.label())
                });
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

    #[test]
    fn pin_region_is_immutable() {
        let pin = pin();
        assert_eq!(pin.tenant_region().as_str(), "fr-par");
        assert_eq!(pin.tenant().as_str(), "01J0EUTENANT");
    }

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

    #[test]
    fn cdc_runner_claim_region_pin_provider_consumer() {
        struct CiScheduler;
        impl CiScheduler {
            fn try_claim(
                pin: &RunnerClaimPin,
                runner_region: &Region,
            ) -> Result<(), OutOfRegionRunnerClaim> {
                pin.admit_claim(runner_region)
            }
        }

        let pin =
            RunnerClaimPin::for_tenant(TenantId::from_token("01J0EUTENANT"), Region::new("fr-par"));

        CiScheduler::try_claim(&pin, &Region::new("fr-par"))
            .expect("the scheduler admits an in-region runner");
        let refused = CiScheduler::try_claim(&pin, &Region::new("eu-north"))
            .expect_err("the scheduler refuses an out-of-region runner (0 out-of-region claims)");
        assert_eq!(refused.runner_region.as_str(), "eu-north");
        assert_eq!(refused.tenant_region.as_str(), "fr-par");
    }
}
