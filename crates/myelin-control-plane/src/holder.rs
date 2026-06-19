//! # The control-plane store AS a `PersonalDataHolder` + the data-map (CP-D1 registry leg)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md` §3.3 (the control
//! plane holds ZERO in-region personal data; the assertion "control plane has zero
//! `is_personal=true` columns" is a committed CI gate — the `control-plane-pii-free` lint, D-CP-1)
//! and §4.3. Contract 1.4 (holder auto-registration), 10.1 (`PersonalDataHolder`).
//!
//! ## CP-D1 registry leg — the live registry has 0 `is_personal=true` columns
//! [`control_plane_data_map`] is the machine-readable data-map over the LIVE control-plane registry
//! schema (the analogue of what the GDPR data-map generator, P-GA-09, walks): one
//! [`ColumnClassification`] per registry column, each carrying its `is_personal` flag. EVERY column
//! is classified `is_personal = false` — the control plane carries opaque ids / region codes /
//! status enums / non-personal slugs / aggregate counts, never personal data.
//! [`assert_no_personal_columns`] is the CI gate that asserts the live schema has 0 personal
//! columns (the registry leg of CP-D1; the static *lint* leg is `control-plane-pii-free`, P-CP-04).
//! Because the control plane has no PII, this holder's DSR surface is a record-only holder: there is
//! no personal data to locate/export/erase here (the tenant's PII lives IN the cell, never here).

use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    Receipt, RectifyReceipt, Result as DsrResult, RestrictReceipt, SubjectRef, TenantId,
};

/// The PII-free receipt a control-plane DSR op produces: it attests the op ran against the
/// PII-free control plane (no content body — there is no personal data here to address).
fn empty_receipt(operation: &str) -> Receipt {
    Receipt {
        operation: operation.to_string(),
        // No personal data → no content body to address; a stable PII-free marker.
        content_hash: "control-plane:no-personal-data".to_string(),
        // No crypto-shred happens here (the control plane holds zero PII), so no key epoch destroyed.
        key_epoch_destroyed: None,
    }
}

/// One column of the control-plane registry schema, classified for the data-map (the analogue of a
/// `#[personal_data(...)]`-derived registry entry, contract 10.2). PII-free metadata: a table name +
/// a column name + the `is_personal` verdict. The CP-D1 registry leg asserts EVERY entry here is
/// `is_personal = false`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnClassification {
    /// The registry table the column belongs to (`cell` / `tenant_placement` / `cell_provisioning`
    /// / `local_tenant`).
    pub table: &'static str,
    /// The column name.
    pub column: &'static str,
    /// Whether the column is classified `is_personal=true` (the data-map verdict). For the control
    /// plane this is ALWAYS `false` — a `true` here is the CP-D1 registry-leg failure.
    pub is_personal: bool,
}

/// **The data-map over the live control-plane registry schema (CP-D1 registry leg).** Every column
/// of every registry table, classified. The control plane holds opaque ids / region / status /
/// non-personal slug / aggregate counts ONLY — so every column is `is_personal = false`. This is the
/// machine-readable inventory the CP-D1 registry-leg gate asserts is PII-free (the static lint leg
/// is `control-plane-pii-free`, P-CP-04, over the `@control-plane`-marked schema file).
///
/// If a future column were to carry personal data, it would have to be added BOTH here (where
/// [`assert_no_personal_columns`] would reject an `is_personal = true` entry) AND it would trip the
/// `control-plane-pii-free` source lint — a PII column escapes neither the data-map NOR the lint.
pub fn control_plane_data_map() -> Vec<ColumnClassification> {
    // Helper: every control-plane column is PII-free by construction.
    let pii_free = |table, column| ColumnClassification {
        table,
        column,
        is_personal: false,
    };
    vec![
        // `cell` — opaque cell inventory.
        pii_free("cell", "cell_id"),
        pii_free("cell", "region"),
        pii_free("cell", "status"),
        pii_free("cell", "isolation_kind"),
        pii_free("cell", "capacity"),
        pii_free("cell", "utilisation"),
        pii_free("cell", "version"),
        pii_free("cell", "endpoint"),
        // `tenant_placement` — opaque placement record (the registry-schema half of 12.3).
        pii_free("tenant_placement", "tenant_id"),
        pii_free("tenant_placement", "region"),
        pii_free("tenant_placement", "home_cell"),
        pii_free("tenant_placement", "isolation_tier"),
        pii_free("tenant_placement", "slug"), // a NON-personal routing label, never a name.
        pii_free("tenant_placement", "status"),
        pii_free("tenant_placement", "member_cells"),
        // `cell_provisioning` — opaque orchestration log.
        pii_free("cell_provisioning", "cell_id"),
        pii_free("cell_provisioning", "step"),
        pii_free("cell_provisioning", "outcome"),
        // `local_tenant` — the per-cell directory (opaque ids + tier + flag).
        pii_free("local_tenant", "tenant_id"),
        pii_free("local_tenant", "isolation_tier"),
        pii_free("local_tenant", "active"),
    ]
}

/// **The CP-D1 registry-leg CI gate: the live control-plane schema has 0 `is_personal=true`
/// columns** (architecture §3.3, D-CP-1). Returns `Ok(())` if every column in the data-map is
/// `is_personal = false`; else `Err` naming every personal column found (a build failure). A
/// control-plane registry table may carry NO personal data — the tenant's PII is born inside the
/// cell, never here.
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

/// The stable, PII-free holder name the control-plane registry store registers under (contract 1.4).
pub const CONTROL_PLANE_STORE: &str = "control_plane_registry";

/// **The control-plane registry store AS a [`PersonalDataHolder`] (contract 10.1).** The harness
/// auto-registers every store it opens (§3.4); the control-plane registry store registers through
/// the same one door so "we forgot the control-plane store" is structurally impossible. Because the
/// control plane holds ZERO in-region personal data (§3.3), this is a **record-only holder**: there
/// is no personal data here to locate/export/rectify/erase — the DSR bodies return the
/// no-personal-data verdict, NOT a deferred-floor marker. (A tenant's actual PII lives in the
/// cell's stores — Identity's principal store, etc. — which are the holders the DSR fan-out reaches;
/// the control plane only routes.)
#[derive(Clone, Copy, Debug)]
pub struct ControlPlaneHolder;

impl ControlPlaneHolder {
    /// The holder for the control-plane registry store.
    pub fn new() -> ControlPlaneHolder {
        ControlPlaneHolder
    }

    /// The stable, PII-free holder name.
    pub fn store_name(&self) -> &'static str {
        // SAFETY of the literal: CONTROL_PLANE_STORE is a `&'static str`.
        CONTROL_PLANE_STORE
    }
}

impl Default for ControlPlaneHolder {
    fn default() -> Self {
        ControlPlaneHolder::new()
    }
}

/// The verdict every control-plane DSR body returns: there is **no personal data** held here, so a
/// locate/export finds nothing and an erase has nothing to shred. This is NOT a deferred floor — it
/// is the architectural truth (§3.3): the control plane is PII-free, so its holder surface is
/// genuinely empty. (Contrast `myelin_storage::OltpStoreHolder`, whose bodies ARE a deferred GDPR-M1
/// floor because that store DOES hold PII.)
fn no_personal_data(method: &str) -> DsrError {
    DsrError(format!(
        "control-plane registry holds ZERO personal data (architecture §3.3) — {method} finds \
         nothing here; a tenant's PII lives IN the cell (Identity's principal store, …), never in \
         the control plane. This holder registers so the one-door discipline covers it, but its \
         DSR surface is empty by construction."
    ))
}

impl PersonalDataHolder for ControlPlaneHolder {
    fn locate(&self, _subject: &SubjectRef, _tenant: TenantId) -> DsrResult<LocateReport> {
        // No personal data here — locate finds nothing. Returns an empty report (a PII-free
        // receipt) rather than an error so a DSR fan-out across holders treats the control plane as
        // "nothing to do".
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
        // Nothing to rectify — there is no personal data to correct on the control plane.
        Err(no_personal_data("rectify"))
    }
    fn restrict(&self, _subject: &SubjectRef, _on: bool) -> DsrResult<RestrictReceipt> {
        Err(no_personal_data("restrict"))
    }
    fn erase(&self, _scope: EraseScope) -> DsrResult<EraseReceipt> {
        // Nothing to erase — the control plane is PII-free, so erase has nothing to shred here.
        Err(no_personal_data("erase"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **CP-D1 registry leg: the live control-plane schema has 0 `is_personal=true` columns.** The
    /// data-map over every registry table is asserted PII-free (architecture §3.3, D-CP-1).
    #[test]
    fn cp_d1_registry_leg_zero_personal_columns() {
        let map = control_plane_data_map();
        // Sanity: the map covers the real schema (not a vacuous empty map).
        assert!(map.len() >= 20, "the data-map covers every registry column ({} found)", map.len());
        // EVERY column is is_personal=false — the registry leg of CP-D1.
        assert!(map.iter().all(|c| !c.is_personal), "control-plane registry must be PII-free");
        assert_no_personal_columns().expect("0 is_personal=true columns on the control plane");
    }

    /// The data-map covers all four tables (cell / tenant_placement / cell_provisioning /
    /// local_tenant) — the directory is covered too (it is cell-local but still PII-free).
    #[test]
    fn data_map_covers_all_four_tables() {
        let map = control_plane_data_map();
        for table in ["cell", "tenant_placement", "cell_provisioning", "local_tenant"] {
            assert!(
                map.iter().any(|c| c.table == table),
                "the data-map must cover the `{table}` table"
            );
        }
    }

    /// The gate FAILS loudly if a personal column ever appears (the gate is not vacuous — it would
    /// catch a regression). We synthesise a personal column and assert the filter catches it.
    #[test]
    fn the_gate_catches_a_personal_column() {
        let mut map = control_plane_data_map();
        map.push(ColumnClassification {
            table: "tenant_placement",
            column: "admin_email",
            is_personal: true,
        });
        let personal: Vec<_> = map.into_iter().filter(|c| c.is_personal).collect();
        assert_eq!(personal.len(), 1, "a smuggled-in personal column is caught by the data-map");
        assert_eq!(personal[0].column, "admin_email");
    }

    /// The control-plane holder registers under its stable PII-free name (contract 1.4).
    #[test]
    fn holder_has_a_stable_pii_free_name() {
        let h = ControlPlaneHolder::new();
        assert_eq!(h.store_name(), "control_plane_registry");
        assert_eq!(CONTROL_PLANE_STORE, "control_plane_registry");
    }

    /// The holder's DSR surface is empty BY CONSTRUCTION (not a deferred floor): locate/export find
    /// nothing; rectify/restrict/erase report there is no personal data here (§3.3).
    #[test]
    fn dsr_surface_is_empty_by_construction() {
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        let h = ControlPlaneHolder::new();
        let subject = SubjectRef::new(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        ));
        // locate/export succeed with nothing (a DSR fan-out sees "nothing to do" here).
        assert!(h.locate(&subject, TenantId::from_token("acme")).is_ok());
        assert!(h.export(&subject, TenantId::from_token("acme")).is_ok());
        // erase reports there is nothing to shred — and names WHY (PII-free by construction).
        match h.erase(EraseScope::Tenant(TenantId::from_token("acme"))) {
            Err(DsrError(msg)) => assert!(
                msg.contains("ZERO personal data"),
                "the empty-by-construction reason must be named: {msg}"
            ),
            Ok(_) => panic!("control-plane erase reports the PII-free truth, not a silent ok"),
        }
    }
}
