use myelin_storage::placement_durable::{DurableCellRow, DurablePlacementRow};
use myelin_tenancy::{CellId, Region, TenantId};

use crate::schema::{
    Capacity, Cell, CellStatus, IsolationKind, PlacementStatus, ProvisioningOutcome,
    TenantPlacement,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RegistryRowError {
    InvalidText {
        field: &'static str,
        value: String,
    },
    OutOfRange {
        field: &'static str,
        value: String,
        expected: &'static str,
    },
}

impl RegistryRowError {
    fn invalid_text(field: &'static str, value: &str) -> Self {
        Self::InvalidText {
            field,
            value: value.to_string(),
        }
    }

    fn out_of_range(field: &'static str, value: impl ToString, expected: &'static str) -> Self {
        Self::OutOfRange {
            field,
            value: value.to_string(),
            expected,
        }
    }
}

impl core::fmt::Display for RegistryRowError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidText { field, value } => {
                write!(f, "field `{field}` has unknown value `{value}`")
            }
            Self::OutOfRange {
                field,
                value,
                expected,
            } => write!(
                f,
                "field `{field}` has value `{value}`, outside the supported range {expected}"
            ),
        }
    }
}

impl std::error::Error for RegistryRowError {}

pub(crate) fn cell_status_text(status: CellStatus) -> &'static str {
    match status {
        CellStatus::Provisioning => "Provisioning",
        CellStatus::Active => "Active",
        CellStatus::Draining => "Draining",
    }
}

fn cell_status_from(value: &str) -> Result<CellStatus, RegistryRowError> {
    match value {
        "Provisioning" => Ok(CellStatus::Provisioning),
        "Active" => Ok(CellStatus::Active),
        "Draining" => Ok(CellStatus::Draining),
        _ => Err(RegistryRowError::invalid_text("status", value)),
    }
}

pub(crate) fn isolation_text(kind: IsolationKind) -> &'static str {
    match kind {
        IsolationKind::Pool => "Pool",
        IsolationKind::Bridge => "Bridge",
        IsolationKind::Dedicated => "Dedicated",
    }
}

pub(crate) fn isolation_from(value: &str) -> Result<IsolationKind, RegistryRowError> {
    match value {
        "Pool" => Ok(IsolationKind::Pool),
        "Bridge" => Ok(IsolationKind::Bridge),
        "Dedicated" => Ok(IsolationKind::Dedicated),
        _ => Err(RegistryRowError::invalid_text("isolation_kind", value)),
    }
}

pub(crate) fn placement_status_text(status: PlacementStatus) -> &'static str {
    match status {
        PlacementStatus::Pending => "Pending",
        PlacementStatus::Active => "Active",
        PlacementStatus::Offboarding => "Offboarding",
    }
}

fn placement_status_from(value: &str) -> Result<PlacementStatus, RegistryRowError> {
    match value {
        "Pending" => Ok(PlacementStatus::Pending),
        "Active" => Ok(PlacementStatus::Active),
        "Offboarding" => Ok(PlacementStatus::Offboarding),
        _ => Err(RegistryRowError::invalid_text("status", value)),
    }
}

pub(crate) fn provisioning_outcome_text(outcome: ProvisioningOutcome) -> &'static str {
    match outcome {
        ProvisioningOutcome::Running => "Running",
        ProvisioningOutcome::Passed => "Passed",
        ProvisioningOutcome::Failed => "Failed",
    }
}

pub(crate) fn provisioning_outcome_from(
    value: &str,
) -> Result<ProvisioningOutcome, RegistryRowError> {
    match value {
        "Running" => Ok(ProvisioningOutcome::Running),
        "Passed" => Ok(ProvisioningOutcome::Passed),
        "Failed" => Ok(ProvisioningOutcome::Failed),
        _ => Err(RegistryRowError::invalid_text("outcome", value)),
    }
}

pub(crate) fn validate_cell(cell: &Cell) -> Result<(), RegistryRowError> {
    i64::try_from(cell.capacity.storage_bytes_max).map_err(|_| {
        RegistryRowError::out_of_range(
            "storage_bytes_max",
            cell.capacity.storage_bytes_max,
            "0..=9223372036854775807",
        )
    })?;
    if cell.utilisation > 100 {
        return Err(RegistryRowError::out_of_range(
            "utilisation",
            cell.utilisation,
            "0..=100",
        ));
    }
    Ok(())
}

pub(crate) fn encode_cell(cell: &Cell) -> Result<DurableCellRow, RegistryRowError> {
    validate_cell(cell)?;
    Ok(DurableCellRow {
        cell_id: cell.cell_id.as_str().to_string(),
        region: cell.region.as_str().to_string(),
        status: cell_status_text(cell.status).to_string(),
        isolation_kind: isolation_text(cell.isolation_kind).to_string(),
        tenants_max: i64::from(cell.capacity.tenants_max),
        write_qps_max: i64::from(cell.capacity.write_qps_max),
        storage_bytes_max: i64::try_from(cell.capacity.storage_bytes_max)
            .expect("validate_cell established the signed database range"),
        utilisation: i16::from(cell.utilisation),
        version: i64::from(cell.version),
        endpoint: cell.endpoint.clone(),
    })
}

pub(crate) fn decode_cell(row: &DurableCellRow) -> Result<Cell, RegistryRowError> {
    let tenants_max = u32::try_from(row.tenants_max).map_err(|_| {
        RegistryRowError::out_of_range("tenants_max", row.tenants_max, "0..=4294967295")
    })?;
    let write_qps_max = u32::try_from(row.write_qps_max).map_err(|_| {
        RegistryRowError::out_of_range("write_qps_max", row.write_qps_max, "0..=4294967295")
    })?;
    let storage_bytes_max = u64::try_from(row.storage_bytes_max).map_err(|_| {
        RegistryRowError::out_of_range(
            "storage_bytes_max",
            row.storage_bytes_max,
            "0..=9223372036854775807",
        )
    })?;
    let utilisation = u8::try_from(row.utilisation)
        .ok()
        .filter(|value| *value <= 100)
        .ok_or_else(|| RegistryRowError::out_of_range("utilisation", row.utilisation, "0..=100"))?;
    let version = u32::try_from(row.version)
        .map_err(|_| RegistryRowError::out_of_range("version", row.version, "0..=4294967295"))?;

    Ok(Cell {
        cell_id: CellId::from_token(&row.cell_id),
        region: Region::new(&row.region),
        status: cell_status_from(&row.status)?,
        isolation_kind: isolation_from(&row.isolation_kind)?,
        capacity: Capacity {
            tenants_max,
            write_qps_max,
            storage_bytes_max,
        },
        utilisation,
        version,
        endpoint: row.endpoint.clone(),
    })
}

pub(crate) fn encode_placement(placement: &TenantPlacement) -> DurablePlacementRow {
    DurablePlacementRow {
        tenant_id: placement.tenant_id.as_str().to_string(),
        region: placement.region.as_str().to_string(),
        home_cell: placement.home_cell.as_str().to_string(),
        isolation_tier: isolation_text(placement.isolation_tier).to_string(),
        slug: placement.slug.clone(),
        status: placement_status_text(placement.status).to_string(),
        member_cells: placement
            .member_cells
            .iter()
            .map(|cell| cell.as_str().to_string())
            .collect(),
    }
}

pub(crate) fn decode_placement(
    row: &DurablePlacementRow,
) -> Result<TenantPlacement, RegistryRowError> {
    Ok(TenantPlacement {
        tenant_id: TenantId::from_token(&row.tenant_id),
        region: Region::new(&row.region),
        home_cell: CellId::from_token(&row.home_cell),
        isolation_tier: isolation_from(&row.isolation_tier)?,
        slug: row.slug.clone(),
        status: placement_status_from(&row.status)?,
        member_cells: row.member_cells.iter().map(CellId::from_token).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Cell {
        Cell {
            cell_id: CellId::from_token("cell-w-1"),
            region: Region::new("eu-west"),
            status: CellStatus::Active,
            isolation_kind: IsolationKind::Pool,
            capacity: Capacity {
                tenants_max: 1_000,
                write_qps_max: 5_000,
                storage_bytes_max: 1 << 40,
            },
            utilisation: 10,
            version: 1,
            endpoint: "cell.eu-west.myelin.eu".into(),
        }
    }

    #[test]
    fn a_valid_cell_keeps_its_meaning_across_the_database_boundary() {
        let cell = cell();
        assert_eq!(decode_cell(&encode_cell(&cell).unwrap()).unwrap(), cell);
    }

    #[test]
    fn a_capacity_the_database_cannot_represent_is_rejected_before_the_write() {
        let mut cell = cell();
        cell.capacity.storage_bytes_max = i64::MAX as u64 + 1;

        let error = encode_cell(&cell).expect_err("u64 must not wrap through PostgreSQL bigint");
        assert_eq!(
            error,
            RegistryRowError::OutOfRange {
                field: "storage_bytes_max",
                value: "9223372036854775808".into(),
                expected: "0..=9223372036854775807",
            }
        );
    }

    #[test]
    fn corrupt_signed_values_never_become_large_unsigned_capacities() {
        for (field, corrupt) in [
            (
                "tenants_max",
                DurableCellRow {
                    tenants_max: -1,
                    ..encode_cell(&cell()).unwrap()
                },
            ),
            (
                "write_qps_max",
                DurableCellRow {
                    write_qps_max: -1,
                    ..encode_cell(&cell()).unwrap()
                },
            ),
            (
                "storage_bytes_max",
                DurableCellRow {
                    storage_bytes_max: -1,
                    ..encode_cell(&cell()).unwrap()
                },
            ),
            (
                "version",
                DurableCellRow {
                    version: -1,
                    ..encode_cell(&cell()).unwrap()
                },
            ),
        ] {
            let error = decode_cell(&corrupt).expect_err("negative durable value must fail closed");
            assert!(error.to_string().contains(field), "{field}: {error}");
        }
    }

    #[test]
    fn utilisation_is_a_percentage_on_both_sides_of_the_boundary() {
        let mut domain = cell();
        domain.utilisation = 101;
        assert!(encode_cell(&domain).is_err());

        let durable = DurableCellRow {
            utilisation: 101,
            ..encode_cell(&cell()).unwrap()
        };
        assert!(decode_cell(&durable).is_err());
    }
}
