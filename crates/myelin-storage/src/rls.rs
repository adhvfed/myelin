use myelin_identity::Principal;
use myelin_tenancy::{Region, TenantId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RlsError {
    MissingTenantPredicate,
}

impl core::fmt::Display for RlsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RlsError::MissingTenantPredicate => write!(
                f,
                "query against a tenant-owned table is missing its (tenant, region) RLS predicate \
                 (storage §1.1 - there is no cross-tenant query path)"
            ),
        }
    }
}

impl std::error::Error for RlsError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantScope {
    tenant: TenantId,
    region: Region,
}

impl TenantScope {
    pub fn from_verified_token(principal: &Principal, region: Region) -> TenantScope {
        TenantScope {
            tenant: principal.tenant.clone(),
            region,
        }
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn resolve(&self, path_tenant: Option<&TenantId>) -> ResolvedTenant {
        let attempted_mismatch = match path_tenant {
            Some(p) => p != &self.tenant,
            None => false,
        };
        ResolvedTenant {
            tenant: self.tenant.clone(),
            path_derived: false,
            attempted_path_mismatch: attempted_mismatch,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTenant {
    pub tenant: TenantId,
    pub path_derived: bool,
    pub attempted_path_mismatch: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantTable {
    name: &'static str,
}

impl TenantTable {
    pub const fn new(name: &'static str) -> TenantTable {
        TenantTable { name }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantQuery {
    scope: TenantScope,
    table: TenantTable,
}

impl TenantQuery {
    pub fn for_table(scope: TenantScope, table: TenantTable) -> TenantQuery {
        TenantQuery { scope, table }
    }

    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }

    pub fn table(&self) -> &TenantTable {
        &self.table
    }

    pub fn predicate_sql(&self) -> String {
        format!("{} WHERE tenant = $1 AND region = $2", self.table.name())
    }

    pub fn predicate_binds(&self) -> Vec<String> {
        vec![self.scope.tenant().0.clone(), self.scope.region().0.clone()]
    }

    pub fn validate(&self) -> Result<(), RlsError> {
        if self.scope.tenant().0.is_empty() {
            return Err(RlsError::MissingTenantPredicate);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn principal(tenant: &str) -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )
    }

    #[test]
    fn token_tenant_wins_over_path_tenant() {
        let scope = TenantScope::from_verified_token(&principal("acme"), Region("eu-west".into()));
        let resolved = scope.resolve(Some(&TenantId("evil-corp".into())));
        assert_eq!(resolved.tenant, TenantId("acme".into()));
        assert!(
            !resolved.path_derived,
            "the tenant must NEVER come from the path"
        );
        assert!(resolved.attempted_path_mismatch);
    }

    #[test]
    fn matching_path_tenant_resolves_to_token_with_no_mismatch() {
        let scope = TenantScope::from_verified_token(&principal("acme"), Region("eu-west".into()));
        let resolved = scope.resolve(Some(&TenantId("acme".into())));
        assert_eq!(resolved.tenant, TenantId("acme".into()));
        assert!(!resolved.path_derived);
        assert!(
            !resolved.attempted_path_mismatch,
            "matching tenants are not a mismatch"
        );
    }

    #[test]
    fn absent_path_tenant_resolves_to_token() {
        let scope = TenantScope::from_verified_token(&principal("acme"), Region("eu-west".into()));
        let resolved = scope.resolve(None);
        assert_eq!(resolved.tenant, TenantId("acme".into()));
        assert!(!resolved.path_derived);
        assert!(!resolved.attempted_path_mismatch);
    }

    #[test]
    fn scope_carries_tenant_and_region_from_token() {
        let scope =
            TenantScope::from_verified_token(&principal("acme"), Region("eu-central".into()));
        assert_eq!(scope.tenant(), &TenantId("acme".into()));
        assert_eq!(scope.region(), &Region("eu-central".into()));
    }

    #[test]
    fn query_carries_tenant_and_region_predicate() {
        let scope = TenantScope::from_verified_token(&principal("acme"), Region("eu-west".into()));
        let q = TenantQuery::for_table(scope, TenantTable::new("issue"));
        let sql = q.predicate_sql();
        assert!(
            sql.contains("tenant = $1 AND region = $2"),
            "predicate must render bind placeholders, not literals: {sql}"
        );
        assert!(
            !sql.contains('\''),
            "no value may be interpolated as a string literal (injection-safe): {sql}"
        );
        assert!(
            sql.starts_with("issue WHERE"),
            "predicate must target the declared table: {sql}"
        );
        assert_eq!(
            q.predicate_binds(),
            vec!["acme".to_string(), "eu-west".to_string()],
            "binds carry the verified (tenant, region) out-of-band"
        );
    }

    #[test]
    fn well_formed_query_validates() {
        let scope = TenantScope::from_verified_token(&principal("acme"), Region("eu-west".into()));
        let q = TenantQuery::for_table(scope, TenantTable::new("issue"));
        assert_eq!(q.validate(), Ok(()));
    }

    #[test]
    fn empty_tenant_scope_is_rejected_by_the_backstop() {
        let scope = TenantScope::from_verified_token(&principal(""), Region("eu-west".into()));
        let q = TenantQuery::for_table(scope, TenantTable::new("issue"));
        assert_eq!(q.validate(), Err(RlsError::MissingTenantPredicate));
    }

    #[test]
    fn rls_error_display_is_loud_and_specific() {
        let msg = RlsError::MissingTenantPredicate.to_string();
        assert!(
            !msg.is_empty(),
            "an RLS violation must not render as an empty string"
        );
        assert!(
            msg.contains("(tenant, region)"),
            "must name the missing predicate: {msg}"
        );
        assert!(
            msg.contains("cross-tenant"),
            "must cite the §1.1 no-cross-tenant rule: {msg}"
        );
    }

    #[test]
    fn query_exposes_scope_and_table() {
        let scope = TenantScope::from_verified_token(&principal("acme"), Region("eu-west".into()));
        let q = TenantQuery::for_table(scope.clone(), TenantTable::new("worklog"));
        assert_eq!(q.scope(), &scope);
        assert_eq!(q.table().name(), "worklog");
    }
}
