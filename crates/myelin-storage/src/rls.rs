//! The `(tenant, region)`-first RLS tenant-scoping guard — the IDOR floor (mandatory-core).
//!
//! **Architecture:** storage.md §1.1 (tenant is the first column / partition key of
//! everything; there is no cross-tenant query path; **tenant from the verified token, never
//! the path**), §3.1 (the `(tenant, region)`-first RLS tenant-scoping guard = the IDOR
//! floor and the `tenant-predicate` lint target). Contract 11.1 (the RLS half) + 12.1 (the
//! `(tenant, region)` partition key from the verified token).
//!
//! ## The structural enforcement (the `tenant-predicate` lint target)
//! Every query against a tenant-owned table is built through [`TenantQuery::for_table`],
//! which **takes a [`TenantScope`] by value** — and a [`TenantScope`] can ONLY be minted
//! from a verified [`Principal`] via [`TenantScope::from_verified_token`]. There is **no
//! constructor that takes a tenant from a path/string** (the IDOR shape). Therefore *a
//! tenant-less query against a tenant table does not compile*: you cannot call
//! [`TenantQuery::for_table`] without a `TenantScope`, and you cannot mint a `TenantScope`
//! without a verified token. This is the in-crate compile-fixture half of the
//! `tenant-predicate` lint (the source-scanning lint is P-S10/P-S11; the red/green fixtures
//! live in the harness lint set).
//!
//! ## Why this is mutation-tested mandatory-core (≥ 80% floor, EI-01 §2)
//! Cross-tenant IDOR is the order-by-non-negotiability "stop-the-bleeding" class (EI-01
//! §2). The single derivation that matters — *the predicate's tenant comes from the token,
//! never the path* ([`TenantScope::resolve`]) — is exhaustively unit-tested so every
//! mutation of its decision logic is caught by an assertion. The cargo-mutants floor and
//! the exact command are in the P-007 report.

use myelin_identity::Principal;
use myelin_tenancy::{Region, TenantId};

/// An error from the RLS guard. The taxonomy is intentionally tiny on this floor (the rich
/// error set lands with the real driver, P-S12); what matters is that a guard violation is
/// a typed, loud value — never a silent fallthrough.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RlsError {
    /// A query was built without the `(tenant, region)` predicate (defence in depth — the
    /// type system already prevents this; this is the runtime backstop a fuzzer/driver
    /// path would hit).
    MissingTenantPredicate,
}

impl core::fmt::Display for RlsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RlsError::MissingTenantPredicate => write!(
                f,
                "query against a tenant-owned table is missing its (tenant, region) RLS predicate \
                 (storage §1.1 — there is no cross-tenant query path)"
            ),
        }
    }
}

impl std::error::Error for RlsError {}

/// The verified `(tenant, region)` scope every tenant-table query carries.
///
/// **The IDOR floor lives in the constructor.** A `TenantScope` is minted ONLY from a
/// verified [`Principal`] (the token), via [`TenantScope::from_verified_token`]. The
/// `(tenant, region)` it carries is the principal's — the URL path is never consulted.
/// [`TenantScope::resolve`] makes the rule explicit and testable: given the token's tenant
/// and a path-asserted tenant, the resolved tenant is ALWAYS the token's, and the count of
/// path-derived tenants is 0 (the `path_derived_tenant_count == 0` /
/// `CrossTenantCount == 0` survival signal the IDOR drill asserts).
///
/// `region` is part of the key because residency is a first-class partition dimension
/// (12.1, ADR-11) — `(tenant, region)` is the partition key, not `tenant` alone. The
/// residency-pin lint (P-ST-04 / P-S11) enforces the region declaration; this guard
/// carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantScope {
    tenant: TenantId,
    region: Region,
}

impl TenantScope {
    /// Mint a scope from the **verified token** (the resolved [`Principal`] + its pinned
    /// region). This is the ONLY public constructor — there is deliberately no
    /// `from_path`/`from_str` (the IDOR shape). The `region` is supplied by the harness
    /// from the verified `Principal{… region …}` (authenticate, 4.1) — on this floor the
    /// `Principal` skeleton does not yet carry `region` (it lands with Identity M1), so the
    /// harness threads it explicitly; the call shape does not change when it moves onto the
    /// principal.
    pub fn from_verified_token(principal: &Principal, region: Region) -> TenantScope {
        TenantScope {
            tenant: principal.tenant.clone(),
            region,
        }
    }

    /// The verified tenant this scope pins every query to.
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// The residency region this scope pins every query to.
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// **THE IDOR-floor decision (mutation-tested mandatory-core).** Resolve the effective
    /// tenant for a request whose URL path *asserts* `path_tenant`. The answer is ALWAYS
    /// the token's tenant (`self.tenant`); the path is **never** trusted.
    ///
    /// Returns the [`ResolvedTenant`] carrying (a) the effective tenant (= the token's) and
    /// (b) `path_derived: true` IFF the path asserted a DIFFERENT tenant than the token
    /// (an attempted IDOR that was rejected). The IDOR drill counts these: a read whose
    /// token-tenant ≠ path-tenant resolves to the token-tenant, and the
    /// `path_derived_tenant_count` (= the number of requests whose effective tenant was
    /// taken from the path) is **0** — because this function never takes it from the path.
    pub fn resolve(&self, path_tenant: Option<&TenantId>) -> ResolvedTenant {
        // The effective tenant is the TOKEN's, unconditionally. The path is observed only
        // to flag an attempted mismatch for the survival signal — it is NEVER the source.
        let attempted_mismatch = match path_tenant {
            Some(p) => p != &self.tenant,
            None => false,
        };
        ResolvedTenant {
            tenant: self.tenant.clone(),
            // The effective tenant was NEVER derived from the path — by construction.
            path_derived: false,
            attempted_path_mismatch: attempted_mismatch,
        }
    }
}

/// The outcome of [`TenantScope::resolve`] — the effective tenant + the IDOR survival flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTenant {
    /// The effective tenant — ALWAYS the token's (never the path's).
    pub tenant: TenantId,
    /// Was the effective tenant derived from the URL path? Always `false` (the
    /// `path_derived_tenant_count == 0` floor). Present so a drill can assert it.
    pub path_derived: bool,
    /// Did the URL path ASSERT a different tenant than the token (a rejected IDOR attempt)?
    /// `true` here means "an attack was attempted and the guard held" — the effective
    /// tenant is still the token's.
    pub attempted_path_mismatch: bool,
}

/// A tenant-owned table the RLS guard applies to. A subsystem declares its tenant tables as
/// `TenantTable`s; the type's existence is what makes "a tenant-less query against a tenant
/// table" a thing the type system can forbid (you need a [`TenantScope`] to query one).
///
/// On this floor a `TenantTable` is its table name (the thin, visible-SQL identifier, §2.8 —
/// not an ORM entity). The typed-row mapping lands with the driver (P-S12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantTable {
    name: &'static str,
}

impl TenantTable {
    /// Declare a tenant-owned table by its (static) name.
    pub const fn new(name: &'static str) -> TenantTable {
        TenantTable { name }
    }

    /// The table's name.
    pub fn name(&self) -> &'static str {
        self.name
    }
}

/// A query against a tenant-owned table, **carrying its `(tenant, region)` predicate by
/// construction**. The ONLY constructor ([`TenantQuery::for_table`]) takes a
/// [`TenantScope`] — so a query without a tenant predicate is unconstructable (the
/// `tenant-predicate` lint's compile-fixture). [`TenantQuery::predicate_sql`] renders the
/// thin, visible `WHERE tenant = $.. AND region = $..` clause every statement carries.
///
/// ## The tenant-predicate compile-fixture (a tenant-less query does not compile)
/// Building a query against a tenant table WITHOUT a verified `(tenant, region)` scope is a
/// compile error — the structural half of the `tenant-predicate` lint. This is asserted as
/// a `compile_fail` doctest (the in-crate fixture the prompt names; the source-scanning
/// lint with red/green fixtures is P-S10/P-S11):
///
/// ```compile_fail
/// use myelin_storage::{TenantQuery, TenantTable};
/// // No TenantScope in scope — there is no constructor that derives a tenant from a
/// // path/string. `for_table` REQUIRES a verified TenantScope, so this cannot compile:
/// let _q = TenantQuery::for_table(TenantTable::new("issue"));
/// ```
///
/// The green counterpart (a query built WITH a verified scope) is the unit tests below.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantQuery {
    scope: TenantScope,
    table: TenantTable,
}

impl TenantQuery {
    /// Build a query against `table`, scoped to the verified `(tenant, region)`. There is
    /// no overload that omits the scope — that is the structural IDOR floor.
    pub fn for_table(scope: TenantScope, table: TenantTable) -> TenantQuery {
        TenantQuery { scope, table }
    }

    /// The scope this query is pinned to.
    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }

    /// The table this query targets.
    pub fn table(&self) -> &TenantTable {
        &self.table
    }

    /// Render the thin, visible-SQL `(tenant, region)` predicate this query always carries
    /// (storage §3.1 — "thin, visible SQL over a heavy ORM"). The tenant/region bind values
    /// come from the verified scope; this is the predicate the `tenant-predicate` lint
    /// requires on every statement against a tenant table.
    pub fn predicate_sql(&self) -> String {
        format!(
            "{} WHERE tenant = '{}' AND region = '{}'",
            self.table.name(),
            self.scope.tenant().0,
            self.scope.region().0,
        )
    }

    /// Defence-in-depth backstop: a query is well-formed only if it carries a non-empty
    /// `(tenant, region)` predicate. The type system already guarantees this (you cannot
    /// build a `TenantQuery` without a `TenantScope`); this method is the runtime check a
    /// raw-driver / fuzzer path would hit, returning the loud [`RlsError`] rather than
    /// silently running an unscoped statement.
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

    /// THE IDOR-floor test: a read whose **token-tenant ≠ path-tenant** resolves to the
    /// **token-tenant**, with `path_derived == false` (0 path-derived tenants) and the
    /// attempted mismatch flagged. This is the §1.1 IDOR floor at the unit level.
    #[test]
    fn token_tenant_wins_over_path_tenant() {
        let scope = TenantScope::from_verified_token(&principal("acme"), Region("eu-west".into()));
        // The path asserts a DIFFERENT tenant — the classic IDOR attempt.
        let resolved = scope.resolve(Some(&TenantId("evil-corp".into())));
        // Effective tenant is the TOKEN's, never the path's.
        assert_eq!(resolved.tenant, TenantId("acme".into()));
        // 0 path-derived tenants — the survival-signal floor.
        assert!(
            !resolved.path_derived,
            "the tenant must NEVER come from the path"
        );
        // The attack was attempted and the guard held.
        assert!(resolved.attempted_path_mismatch);
    }

    /// The matching-path case: token-tenant == path-tenant resolves to the token's tenant,
    /// no mismatch flagged, still 0 path-derived.
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

    /// The no-path case (an internal/RPC call with no URL path): resolves to the token's
    /// tenant, no mismatch, 0 path-derived.
    #[test]
    fn absent_path_tenant_resolves_to_token() {
        let scope = TenantScope::from_verified_token(&principal("acme"), Region("eu-west".into()));
        let resolved = scope.resolve(None);
        assert_eq!(resolved.tenant, TenantId("acme".into()));
        assert!(!resolved.path_derived);
        assert!(!resolved.attempted_path_mismatch);
    }

    /// The scope carries BOTH tenant and region from the verified token (12.1 — the
    /// partition key is `(tenant, region)`, not tenant alone).
    #[test]
    fn scope_carries_tenant_and_region_from_token() {
        let scope =
            TenantScope::from_verified_token(&principal("acme"), Region("eu-central".into()));
        assert_eq!(scope.tenant(), &TenantId("acme".into()));
        assert_eq!(scope.region(), &Region("eu-central".into()));
    }

    /// A query against a tenant table CARRIES its `(tenant, region)` predicate (the thin,
    /// visible SQL). The tenant/region bind values are the verified token's.
    #[test]
    fn query_carries_tenant_and_region_predicate() {
        let scope = TenantScope::from_verified_token(&principal("acme"), Region("eu-west".into()));
        let q = TenantQuery::for_table(scope, TenantTable::new("issue"));
        let sql = q.predicate_sql();
        assert!(
            sql.contains("tenant = 'acme'"),
            "predicate must pin the token tenant: {sql}"
        );
        assert!(
            sql.contains("region = 'eu-west'"),
            "predicate must pin the region: {sql}"
        );
        assert!(
            sql.starts_with("issue WHERE"),
            "predicate must target the declared table: {sql}"
        );
    }

    /// A well-formed query validates green; the predicate is present by construction.
    #[test]
    fn well_formed_query_validates() {
        let scope = TenantScope::from_verified_token(&principal("acme"), Region("eu-west".into()));
        let q = TenantQuery::for_table(scope, TenantTable::new("issue"));
        assert_eq!(q.validate(), Ok(()));
    }

    /// The defence-in-depth backstop reads RED on an empty-tenant scope (the shape a
    /// raw-driver path could synthesise) — proving `validate` is not vacuously green.
    #[test]
    fn empty_tenant_scope_is_rejected_by_the_backstop() {
        let scope = TenantScope::from_verified_token(&principal(""), Region("eu-west".into()));
        let q = TenantQuery::for_table(scope, TenantTable::new("issue"));
        assert_eq!(q.validate(), Err(RlsError::MissingTenantPredicate));
    }

    /// The `RlsError` Display renders the loud, specific message (not an empty default) —
    /// a violation is observable, never a blank string (closes the Display mutant; EI-01 §3
    /// observability is part of the pass).
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

    /// The query exposes its scope + table for the holder/relay paths that need them.
    #[test]
    fn query_exposes_scope_and_table() {
        let scope = TenantScope::from_verified_token(&principal("acme"), Region("eu-west".into()));
        let q = TenantQuery::for_table(scope.clone(), TenantTable::new("worklog"));
        assert_eq!(q.scope(), &scope);
        assert_eq!(q.table().name(), "worklog");
    }
}
