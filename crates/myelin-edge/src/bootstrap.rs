use myelin_events::Timestamp;
use myelin_identity::{
    DataRole, ObjectId, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RelName,
    RelationTuple, TupleDelta,
};
use myelin_identity_service::{
    CapabilityMintSpec, CellTokenAuthority, PrincipalProfile, PrincipalStore, TupleStore,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

pub const BOOTSTRAP_SCHEME: &str = "agent";

pub const BOOTSTRAP_AUTHORITY: &[&str] = &["edge.operator"];

pub struct BootstrapParams<'a> {
    pub tenant: &'a str,
    pub region: &'a str,
    pub principal: &'a str,
    pub issues_project: &'a str,
    pub display: Option<&'a str>,
    pub ttl_days: u32,
}

pub struct BootstrapOutcome {
    pub token: String,
    pub tenant: String,
    pub region: String,
    pub principal_id: String,
    pub subject_key: String,
    pub jti: String,
    pub expiry_unix: i64,
}

#[derive(Debug)]
pub enum BootstrapError {
    BadParam(String),
    Store(String),
}

impl core::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BootstrapError::BadParam(m) => write!(f, "bootstrap parameter error: {m}"),
            BootstrapError::Store(m) => write!(f, "bootstrap durable store error: {m}"),
        }
    }
}

impl std::error::Error for BootstrapError {}

pub fn bootstrap_principal_and_mint(
    store: &PrincipalStore,
    tuples: &TupleStore,
    cell: &CellTokenAuthority,
    params: &BootstrapParams<'_>,
    now_unix: i64,
) -> Result<BootstrapOutcome, BootstrapError> {
    if params.tenant.trim().is_empty() {
        return Err(BootstrapError::BadParam(
            "--tenant must be non-empty".into(),
        ));
    }
    if params.region.trim().is_empty() {
        return Err(BootstrapError::BadParam(
            "--region must be non-empty".into(),
        ));
    }
    if params.principal.trim().is_empty() {
        return Err(BootstrapError::BadParam(
            "--principal must be non-empty".into(),
        ));
    }
    if !myelin_issues::api::is_canonical_uuid(params.issues_project) {
        return Err(BootstrapError::BadParam(
            "--issues-project must be a canonical lowercase UUID".into(),
        ));
    }

    let operator = Principal::new(
        TenantId(params.tenant.to_string()),
        Region(params.region.to_string()),
        PrincipalId("bootstrap-operator".into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let scope = TenantScope::from_verified_token(&operator, operator.region.clone());
    let pid = PrincipalId(params.principal.to_string());

    let profile = params.display.filter(|d| !d.trim().is_empty()).map(|d| {
        let email = String::new();
        let display_name = d.to_string();
        PrincipalProfile {
            email,
            display_name,
        }
    });
    store
        .put_principal(
            &scope,
            pid.clone(),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
            profile.as_ref(),
        )
        .map_err(|e| BootstrapError::Store(e.to_string()))?;

    let subject_key = params.principal.to_string();
    store
        .link_credential(&scope, BOOTSTRAP_SCHEME, &subject_key, &pid)
        .map_err(|e| BootstrapError::Store(e.to_string()))?;

    let grant = TupleDelta::Add(RelationTuple {
        object: ObjectId(format!("project:{}", params.issues_project)),
        relation: RelName("reader".into()),
        subject: pid,
        caveat: None,
    });
    tuples
        .write_tuples(
            &scope,
            &operator,
            &[grant],
            None,
            None,
            timestamp_from_unix(now_unix),
        )
        .map_err(|e| BootstrapError::Store(format!("Issues project reader grant failed: {e}")))?;

    let jti = fresh_jti(params.principal);
    let expiry_unix = now_unix.saturating_add(i64::from(params.ttl_days).saturating_mul(86_400));
    let token = cell.mint(&CapabilityMintSpec {
        tenant: params.tenant.to_string(),
        region: params.region.to_string(),
        subject_key: subject_key.clone(),
        jti: jti.clone(),
        exp_unix: expiry_unix,
        authority: BOOTSTRAP_AUTHORITY.iter().map(|g| g.to_string()).collect(),
        dpop_jkt: None,
        purpose: myelin_identity_service::CredentialPurpose::OperatorBootstrap,
        audience: myelin_identity_service::CredentialAudience::Edge,
    });

    Ok(BootstrapOutcome {
        token,
        tenant: params.tenant.to_string(),
        region: params.region.to_string(),
        principal_id: params.principal.to_string(),
        subject_key,
        jti,
        expiry_unix,
    })
}

fn timestamp_from_unix(unix: i64) -> Timestamp {
    let value = chrono::DateTime::from_timestamp(unix, 0)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    Timestamp(value)
}

fn fresh_jti(principal: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("bootstrap-{principal}-{nanos}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::OutboxStore;
    use myelin_storage::KmsEngine;
    use std::sync::Arc;

    const PROJECT: &str = "11111111-1111-1111-1111-111111111111";

    fn params<'a>(project: &'a str) -> BootstrapParams<'a> {
        BootstrapParams {
            tenant: "acme",
            region: "fr-par",
            principal: "founder",
            issues_project: project,
            display: None,
            ttl_days: 30,
        }
    }

    fn scope() -> TenantScope {
        TenantScope::from_verified_token(
            &Principal::new(
                TenantId("acme".into()),
                Region("fr-par".into()),
                PrincipalId("bootstrap-test".into()),
                PrincipalKind::Human,
                DataRole::Controller,
                PrincipalStatus::Active,
            ),
            Region("fr-par".into()),
        )
    }

    #[test]
    fn malformed_project_is_rejected_before_any_durable_shape_changes() {
        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let tuples = TupleStore::new(OutboxStore::new());
        let cell = CellTokenAuthority::from_seed(&[7; 32], &[9; 32]).unwrap();

        let result = bootstrap_principal_and_mint(
            &store,
            &tuples,
            &cell,
            &params("NOT-A-UUID"),
            1_700_000_000,
        );
        assert!(matches!(result, Err(BootstrapError::BadParam(_))));
        assert!(store
            .get_principal(&scope(), &PrincipalId("founder".into()))
            .expect("the unchanged in-memory directory remains readable")
            .is_none());
        assert!(tuples
            .tuples_in(&scope())
            .expect("read tuples after rejected bootstrap")
            .is_empty());
    }

    #[test]
    fn rebootstrap_converges_on_one_narrow_project_reader_edge() {
        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let outbox = OutboxStore::new();
        let tuples = TupleStore::new(outbox.clone());
        let cell = CellTokenAuthority::from_seed(&[7; 32], &[9; 32]).unwrap();

        let first =
            bootstrap_principal_and_mint(&store, &tuples, &cell, &params(PROJECT), 1_700_000_000)
                .unwrap();
        let second =
            bootstrap_principal_and_mint(&store, &tuples, &cell, &params(PROJECT), 1_700_000_001)
                .unwrap();
        assert_ne!(first.jti, second.jti);
        let edges = tuples
            .tuples_in(&scope())
            .expect("read the bootstrap relationship");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].tuple.object.0, format!("project:{PROJECT}"));
        assert_eq!(edges[0].tuple.relation.0, "reader");
        assert_eq!(edges[0].tuple.subject.0, "founder");
        assert!(edges[0].tuple.caveat.is_none());
        let events = outbox.committed_rows();
        assert_eq!(
            events.len(),
            2,
            "each explicit operator retry remains auditable"
        );
        assert!(events.iter().all(|row| {
            row.envelope.actor.0.principal_id.0 == "bootstrap-operator"
                && row.envelope.actor.0.tenant.as_str() == "acme"
                && row.envelope.actor.0.region.as_str() == "fr-par"
                && row.envelope.tenant.as_str() == "acme"
                && row.envelope.region.as_str() == "fr-par"
                && row.envelope.actor.0.principal_id.0 != edges[0].tuple.subject.0
        }));
    }
}
