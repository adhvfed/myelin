use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    Receipt, RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};

fn empty_receipt(operation: &str) -> Receipt {
    Receipt {
        operation: operation.to_string(),
        content_hash: "control-plane:no-personal-data".to_string(),
        key_epoch_destroyed: None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnClassification {
    pub table: &'static str,
    pub column: &'static str,
    pub is_personal: bool,
}

pub fn control_plane_data_map() -> Vec<ColumnClassification> {
    let pii_free = |table, column| ColumnClassification {
        table,
        column,
        is_personal: false,
    };
    vec![
        pii_free("cell", "cell_id"),
        pii_free("cell", "region"),
        pii_free("cell", "status"),
        pii_free("cell", "isolation_kind"),
        pii_free("cell", "capacity"),
        pii_free("cell", "utilisation"),
        pii_free("cell", "version"),
        pii_free("cell", "endpoint"),
        pii_free("tenant_placement", "tenant_id"),
        pii_free("tenant_placement", "region"),
        pii_free("tenant_placement", "home_cell"),
        pii_free("tenant_placement", "isolation_tier"),
        pii_free("tenant_placement", "slug"),
        pii_free("tenant_placement", "status"),
        pii_free("tenant_placement", "member_cells"),
        pii_free("cell_provisioning", "cell_id"),
        pii_free("cell_provisioning", "step"),
        pii_free("cell_provisioning", "outcome"),
        pii_free("local_tenant", "tenant_id"),
        pii_free("local_tenant", "isolation_tier"),
        pii_free("local_tenant", "active"),
    ]
}

pub fn assert_no_personal_columns() -> Result<(), Vec<ColumnClassification>> {
    let personal: Vec<ColumnClassification> = control_plane_data_map()
        .into_iter()
        .filter(|c| c.is_personal)
        .collect();
    if personal.is_empty() {
        Ok(())
    } else {
        Err(personal)
    }
}

pub const CONTROL_PLANE_STORE: &str = "control_plane_registry";

#[derive(Clone, Copy, Debug)]
pub struct ControlPlaneHolder;

impl ControlPlaneHolder {
    pub fn new() -> ControlPlaneHolder {
        ControlPlaneHolder
    }

    pub fn store_name(&self) -> &'static str {
        CONTROL_PLANE_STORE
    }
}

impl Default for ControlPlaneHolder {
    fn default() -> Self {
        ControlPlaneHolder::new()
    }
}

fn no_personal_data(method: &str) -> DsrError {
    DsrError(format!(
        "control-plane registry holds ZERO personal data (architecture §3.3) - {method} finds \
         nothing here; a tenant's PII lives IN the cell (Identity's principal store, …), never in \
         the control plane. This holder registers so the one-door discipline covers it, but its \
         DSR surface is empty by construction."
    ))
}

impl PersonalDataHolder for ControlPlaneHolder {
    fn locate(&self, _subject: &SubjectRef, _tenant: TenantId) -> DsrResult<LocateReport> {
        Ok(LocateReport {
            receipt: empty_receipt("locate"),
        })
    }
    fn export(&self, _subject: &SubjectRef, _tenant: TenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: empty_receipt("export"),
        })
    }
    fn rectify(&self, _subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Err(no_personal_data("rectify"))
    }
    fn restrict(&self, _subject: &SubjectRef, _on: bool) -> DsrResult<RestrictReceipt> {
        Err(no_personal_data("restrict"))
    }
    fn erase(&self, _scope: EraseScope) -> DsrResult<EraseReceipt> {
        Err(no_personal_data("erase"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp_d1_registry_leg_zero_personal_columns() {
        let map = control_plane_data_map();
        assert!(
            map.len() >= 20,
            "the data-map covers every registry column ({} found)",
            map.len()
        );
        assert!(
            map.iter().all(|c| !c.is_personal),
            "control-plane registry must be PII-free"
        );
        assert_no_personal_columns().expect("0 is_personal=true columns on the control plane");
    }

    #[test]
    fn data_map_covers_all_four_tables() {
        let map = control_plane_data_map();
        for table in [
            "cell",
            "tenant_placement",
            "cell_provisioning",
            "local_tenant",
        ] {
            assert!(
                map.iter().any(|c| c.table == table),
                "the data-map must cover the `{table}` table"
            );
        }
    }

    #[test]
    fn the_gate_catches_a_personal_column() {
        let mut map = control_plane_data_map();
        map.push(ColumnClassification {
            table: "tenant_placement",
            column: "admin_email",
            is_personal: true,
        });
        let personal: Vec<_> = map.into_iter().filter(|c| c.is_personal).collect();
        assert_eq!(
            personal.len(),
            1,
            "a smuggled-in personal column is caught by the data-map"
        );
        assert_eq!(personal[0].column, "admin_email");
    }

    #[test]
    fn holder_has_a_stable_pii_free_name() {
        let h = ControlPlaneHolder::new();
        assert_eq!(h.store_name(), "control_plane_registry");
        assert_eq!(CONTROL_PLANE_STORE, "control_plane_registry");
    }

    #[test]
    fn dsr_surface_is_empty_by_construction() {
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        let h = ControlPlaneHolder::new();
        let subject = SubjectRef::new(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        ));
        assert!(h.locate(&subject, TenantId::from_token("acme")).is_ok());
        assert!(h.export(&subject, TenantId::from_token("acme")).is_ok());
        match h.erase(EraseScope::Tenant(TenantId::from_token("acme"))) {
            Err(DsrError(msg)) => assert!(
                msg.contains("ZERO personal data"),
                "the empty-by-construction reason must be named: {msg}"
            ),
            Ok(_) => panic!("control-plane erase reports the PII-free truth, not a silent ok"),
        }
    }
}
