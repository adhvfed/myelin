use crate::partition::PartitionKey;
use myelin_tenancy::{Region, TenantId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResidencyError {
    WrongCellRegion {
        tenant: TenantId,
        cell_region: Region,
        partition_region: Region,
    },
    CrossRegionRead {
        tenant: TenantId,
        stream_region: Region,
        read_from_region: Region,
    },
}

impl std::fmt::Display for ResidencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResidencyError::WrongCellRegion {
                tenant,
                cell_region,
                partition_region,
            } => write!(
                f,
                "Bus stream residency REFUSED for tenant `{}`: the stream's partition pins region \
                 `{}` but the cell it lives in is region `{}` - a stream is provisioned inside \
                 exactly one cell and cannot exist outside its cell's region (the pin is the \
                 cell's, NOT the caller's; architecture §7.1). REFUSED (not a silent pass, EI-01 \
                 §3).",
                tenant.as_str(),
                partition_region.as_str(),
                cell_region.as_str(),
            ),
            ResidencyError::CrossRegionRead {
                tenant,
                stream_region,
                read_from_region,
            } => write!(
                f,
                "Bus stream residency REFUSED for tenant `{}`: a read routed from region `{}` may \
                 NOT read a stream pinned to region `{}` - there is no cross-region stream read \
                 path (residency: no cross-region query path, external-insights/04 §1). REFUSED \
                 (0 cross-region reads is the CP-D3 green artifact).",
                tenant.as_str(),
                read_from_region.as_str(),
                stream_region.as_str(),
            ),
        }
    }
}

impl std::error::Error for ResidencyError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusStreamResidency {
    tenant: TenantId,
    subsystem: String,
    region: Region,
}

impl BusStreamResidency {
    pub fn provision(
        partition: &PartitionKey,
        subsystem: &str,
        cell: &Region,
    ) -> Result<BusStreamResidency, ResidencyError> {
        if partition.region != *cell {
            return Err(ResidencyError::WrongCellRegion {
                tenant: partition.tenant.clone(),
                cell_region: cell.clone(),
                partition_region: partition.region.clone(),
            });
        }
        Ok(BusStreamResidency {
            tenant: partition.tenant.clone(),
            subsystem: subsystem.to_string(),
            region: cell.clone(),
        })
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn subsystem(&self) -> &str {
        &self.subsystem
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn authorize_read(&self, read_from: &Region) -> Result<(), ResidencyError> {
        if *read_from != self.region {
            return Err(ResidencyError::CrossRegionRead {
                tenant: self.tenant.clone(),
                stream_region: self.region.clone(),
                read_from_region: read_from.clone(),
            });
        }
        Ok(())
    }

    pub fn region_report(&self) -> BusRegionReport {
        BusRegionReport {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusRegionReport {
    pub tenant: TenantId,
    pub region: Region,
}

impl BusRegionReport {
    pub fn matches_region_of_record(&self, region_of_record: &Region) -> bool {
        self.region == *region_of_record
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusResidencySignal {
    pub tenant: TenantId,
    pub region: Region,
    pub cross_region_reads_admitted: u32,
    pub cross_region_reads_rejected: u32,
}

impl BusResidencySignal {
    pub fn green(tenant: TenantId, region: Region, rejected: u32) -> BusResidencySignal {
        BusResidencySignal {
            tenant,
            region,
            cross_region_reads_admitted: 0,
            cross_region_reads_rejected: rejected,
        }
    }

    pub fn red(tenant: TenantId, region: Region, admitted: u32) -> BusResidencySignal {
        BusResidencySignal {
            tenant,
            region,
            cross_region_reads_admitted: admitted,
            cross_region_reads_rejected: 0,
        }
    }

    pub fn is_green(&self) -> bool {
        self.cross_region_reads_admitted == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fr_par() -> Region {
        Region::new("fr-par")
    }
    fn eu_north() -> Region {
        Region::new("eu-north")
    }
    fn acme() -> TenantId {
        TenantId::from_token("01J0ACME")
    }
    fn partition(region: &Region) -> PartitionKey {
        PartitionKey::new(acme(), region.clone())
    }

    #[test]
    fn a_stream_pinned_to_one_region_rejects_a_read_from_another() {
        let cell = fr_par();
        let stream = BusStreamResidency::provision(&partition(&cell), "issue", &cell)
            .expect("a partition matching the cell region provisions");
        assert_eq!(stream.region(), &fr_par());

        stream
            .authorize_read(&fr_par())
            .expect("an in-region read is authorized");

        let err = stream.authorize_read(&eu_north()).expect_err(
            "a read from a different region MUST be rejected (no cross-region read path)",
        );
        assert_eq!(
            err,
            ResidencyError::CrossRegionRead {
                tenant: acme(),
                stream_region: fr_par(),
                read_from_region: eu_north(),
            }
        );
        assert!(
            err.to_string().contains("no cross-region stream read path"),
            "loud reason: {err}"
        );
    }

    #[test]
    fn a_provision_with_a_region_disagreeing_with_the_cell_is_rejected() {
        let cell = fr_par();
        let err = BusStreamResidency::provision(&partition(&eu_north()), "issue", &cell)
            .expect_err(
            "a partition region ≠ the cell region MUST be rejected (the stream lives in its cell)",
        );
        assert_eq!(
            err,
            ResidencyError::WrongCellRegion {
                tenant: acme(),
                cell_region: fr_par(),
                partition_region: eu_north(),
            }
        );
        assert!(
            err.to_string().contains("the pin is the cell's"),
            "loud reason: {err}"
        );
    }

    #[test]
    fn the_bus_reports_its_region_into_residency_verify() {
        let cell = fr_par();
        let stream =
            BusStreamResidency::provision(&partition(&cell), "issue", &cell).expect("provision");
        let report = stream.region_report();
        assert_eq!(report.tenant, acme());
        assert_eq!(report.region, fr_par());
        assert!(report.matches_region_of_record(&fr_par()));
        assert!(!report.matches_region_of_record(&eu_north()));
    }

    #[test]
    fn bus_residency_signal_green_and_red() {
        let green = BusResidencySignal::green(acme(), fr_par(), 3);
        assert_eq!(
            green.cross_region_reads_admitted, 0,
            "the green artifact is 0 admitted"
        );
        assert_eq!(
            green.cross_region_reads_rejected, 3,
            "3 cross-region reads were blocked"
        );
        assert!(green.is_green());

        let red = BusResidencySignal::red(acme(), fr_par(), 1);
        assert_eq!(
            red.cross_region_reads_admitted, 1,
            "a leaked cross-region read reads RED"
        );
        assert!(!red.is_green());
    }

    #[test]
    fn cp_d3_bus_slice_zero_cross_region_reads() {
        let cell = fr_par();
        let stream =
            BusStreamResidency::provision(&partition(&cell), "issue", &cell).expect("provision");

        let reads = [
            fr_par(),
            eu_north(),
            fr_par(),
            Region::new("us-east"),
            eu_north(),
        ];

        let mut admitted = 0u32;
        let mut rejected = 0u32;
        for read_from in &reads {
            match stream.authorize_read(read_from) {
                Ok(()) => {
                    assert_eq!(
                        read_from,
                        stream.region(),
                        "an admitted read must be in-region"
                    );
                }
                Err(ResidencyError::CrossRegionRead {
                    read_from_region, ..
                }) => {
                    assert_ne!(
                        &read_from_region,
                        stream.region(),
                        "a rejected read was cross-region"
                    );
                    rejected += 1;
                }
                Err(other) => panic!("unexpected error: {other}"),
            }
            if stream.authorize_read(read_from).is_ok() && read_from != stream.region() {
                admitted += 1;
            }
        }

        assert_eq!(
            admitted, 0,
            "THE GATE: 0 cross-region stream reads admitted (CP-D3 Bus slice)"
        );
        assert_eq!(rejected, 3, "every cross-region read attempt was rejected");

        let signal = BusResidencySignal::green(acme(), fr_par(), rejected);
        assert!(
            signal.is_green(),
            "the bus-residency artifact is green (0 admitted)"
        );
    }
}
