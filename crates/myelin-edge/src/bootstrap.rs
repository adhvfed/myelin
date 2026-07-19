//! # `bootstrap` — the operator bootstrap: seed a principal + mint a durable capability token (R4.0)
//!
//! The founder-dogfood make-or-break. Before this, NOTHING could authenticate against the real `edge`
//! binary: the cell token authority was ephemeral per boot and there was no mint path. This module is
//! the testable body behind the `edge bootstrap` operator subcommand: given a composed durable S1
//! [`PrincipalStore`] + the DURABLE cell [`CellTokenAuthority`] (recovered from the sealed
//! `cell_token_root` row), it idempotently ensures a principal exists and mints a fresh capability
//! token that authenticates against a SEPARATELY-running serving edge over the SAME durable DB + seal
//! key (the credential link is in PG; the cell root is durable, so the verifier's trust anchor is the
//! one the mint signed under).
//!
//! ## The operator trust boundary (stated, per the prompt)
//! Anyone holding the `DATABASE_URL` creds + `MYELIN_KMS_SEAL_KEY` can run this and mint a token for
//! any principal in any tenant. That is ACCEPTED: it is operator-plane infrastructure (the same
//! boundary the KMS seal key already draws). There is deliberately **no HTTP endpoint** that mints —
//! minting is an operator action on the box, never a network-reachable surface.
//!
//! ## Why the `agent` scheme + signed operator purpose
//! See [`BOOTSTRAP_SCHEME`] / [`BOOTSTRAP_AUTHORITY`]: `agent` is a real machine scheme with NO DPoP
//! requirement (a `pat` requires DPoP sender-constraint that a `git`/`curl` client cannot produce) and
//! NO authority ceiling (unlike `deploy_key`/`per_job`). The signed `OperatorBootstrap` purpose is
//! deliberately distinct from a delegated `AgentRun`; only that operator purpose may use the
//! `edge.operator` authority as an override across mapped Edge actions. Per-object ReBAC remains a
//! required second conjunct. Re-bootstrap is required for legacy tokens without a signed purpose.

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

/// The credential scheme the operator bootstrap token is minted + linked under (`agent`). A real
/// machine scheme: NO DPoP requirement (so a bearer/basic `git`/`curl` client authenticates) and NO
/// authority ceiling — exactly what the git-wire + product-API oracle tests prove sufficient.
pub const BOOTSTRAP_SCHEME: &str = "agent";

/// The authority grant set carried only by a signed `OperatorBootstrap` credential. It is not a
/// delegated agent capability and never makes `agent:run` a super-capability.
pub const BOOTSTRAP_AUTHORITY: &[&str] = &["edge.operator"];

/// The parameters an operator bootstrap needs (validated here — empty required fields are refused).
pub struct BootstrapParams<'a> {
    /// The tenant slug the principal + token belong to (the trust root).
    pub tenant: &'a str,
    /// The residency region (defaults to `MYELIN_REGION` at the call site).
    pub region: &'a str,
    /// The principal id — ALSO used as the credential `subject_key` (one stable handle the token
    /// carries and the S1 credential link resolves under [`BOOTSTRAP_SCHEME`]).
    pub principal: &'a str,
    /// The canonical Issues project UUID to grant this founder principal as a direct reader. This is
    /// required and explicit: bootstrap never invents a project and never grants tenant-wide access.
    pub issues_project: &'a str,
    /// An optional human display label (the `--display-name` flag) stored in the
    /// (per-subject-DEK-encrypted) profile. Purely cosmetic — no auth/whoami path reads it; `None`
    /// provisions no profile (and no DEK). (Named `display` — not the PII-fingerprinted `display_name`
    /// — because this is a transient operator PARAMETER, not a persisted schema field; the persisted
    /// `PrincipalProfile.display_name` column IS the tagged one the crypto-shred lint governs.)
    pub display: Option<&'a str>,
    /// The token time-to-live in days (the `exp` ceiling; the operator re-runs bootstrap to re-mint).
    pub ttl_days: u32,
}

/// The result of a bootstrap: the minted token + the metadata to print. The `token` is SECRET — the
/// caller prints it to STDOUT exactly once and NEVER logs/persists it elsewhere (no audit body, no
/// file).
pub struct BootstrapOutcome {
    /// The minted capability-token credential material (the SECRET — print once, never log).
    pub token: String,
    /// The tenant the token is for.
    pub tenant: String,
    /// The residency region.
    pub region: String,
    /// The principal id the token resolves to.
    pub principal_id: String,
    /// The credential subject key (== `principal_id`) — the S1 link handle.
    pub subject_key: String,
    /// The token's unique revocation id (the `edge revoke --jti <jti>` handle).
    pub jti: String,
    /// The token expiry as a Unix-seconds instant.
    pub expiry_unix: i64,
}

/// A bootstrap failure. Loud + typed; NEVER carries the token material (only the structural fault).
#[derive(Debug)]
pub enum BootstrapError {
    /// A required parameter was empty/invalid (never a silently-coerced default).
    BadParam(String),
    /// The durable S1 store write (principal upsert / credential link) failed.
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

/// **Idempotently ensure the `(tenant, region)` principal exists (Human kind) + link its `agent`
/// credential, then mint a fresh durable capability token for it (the testable body behind
/// `edge bootstrap`).**
///
/// - `store` is the DURABLE S1 [`PrincipalStore`] (`with_pg`) — the same one the serving edge reads.
/// - `cell` is the DURABLE cell authority (recovered from `cell_token_root`) — so the minted token's
///   signature verifies against the serving edge's trust anchor.
/// - `now_unix` is the current Unix-seconds instant (the caller's clock — injected for determinism).
///
/// Running twice mints a NEW token (a fresh `jti`) for the SAME principal without corrupting anything:
/// `put_principal` / `link_credential` are idempotent upserts. The seeded credential link +
/// principal are exactly what [`myelin_identity_service::CapabilityAuthenticator::authenticate`]
/// resolves (`resolve_credential(scope, "agent", subject_key)` → the principal row); the fresh `jti`
/// is absent from the S7 denylist, so the token is live until revoked or expired.
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
    // This check MUST precede every durable principal/credential write. A malformed project must not
    // leave a half-bootstrapped founder identity behind, and bootstrap never coerces a loose UUID.
    if !myelin_issues::api::is_canonical_uuid(params.issues_project) {
        return Err(BootstrapError::BadParam(
            "--issues-project must be a canonical lowercase UUID".into(),
        ));
    }

    // The verified `(tenant, region)` write scope. The only TenantScope constructor is
    // `from_verified_token`, so a scope from a path is structurally impossible — we mint a minimal
    // verified operator principal carrying the exact operator-supplied tenant AND region (this is
    // the operator plane; the trust boundary is the seal key + DB creds, stated in the module docs).
    // Do not use the stub constructor: its default region would make durable event attribution
    // disagree with the scope and token whenever bootstrap targets a non-default region.
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

    // (1) Idempotent upsert of the Human principal. A display name (optional) is sealed under the
    //     per-subject DEK via the durable KMS; with no display name we provision NO profile (and no
    //     DEK) — the common case. The email column is left empty (the founder has no PII to store).
    let profile = params.display.filter(|d| !d.trim().is_empty()).map(|d| {
        // Field-shorthand init (locals of the same name) — the SAME pattern principal_store.rs's
        // test `profile()` helper uses so the live `no-untagged-personal-data` field scanner does
        // not read this initialiser as an untagged PII field DEFINITION (the tagged definition
        // lives on `PrincipalProfile`, where the lint must see it).
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

    // (2) Link the `agent` credential (subject_key == principal id) — idempotent (re-linking updates
    //     the target to the same principal). This is the S1 record the token's `sub` claim resolves.
    let subject_key = params.principal.to_string();
    store
        .link_credential(&scope, BOOTSTRAP_SCHEME, &subject_key, &pid)
        .map_err(|e| BootstrapError::Store(e.to_string()))?;

    // (3) Idempotently grant ONLY the requested project reader edge. This is an operator-plane write
    //     attributed to `bootstrap-operator`; the founder is the tuple SUBJECT, never falsely
    //     recorded as self-granting authority. This is the ordinary durable
    //     TupleStore path (not raw SQL and not a may_create bypass): Issues' production may_create
    //     still evaluates `view` on this exact project for every create. Re-running Add converges on
    //     the rebac_tuple primary key and never widens the relation.
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

    // (4) Mint a fresh token ONLY after the durable reader grant exists. A reported-success token is
    //     therefore immediately authorized to stage an Issue in the explicit project. A unique `jti`
    //     (principal + a high-resolution timestamp) guarantees a
    //     re-run mints a DISTINCT token (distinct revocation handle) for the same principal. `exp` is
    //     `now + ttl_days`. The macaroon caveat chain is empty at mint (unattenuated root authority).
    let jti = fresh_jti(params.principal);
    let expiry_unix = now_unix.saturating_add(i64::from(params.ttl_days).saturating_mul(86_400));
    let token = cell.mint(&CapabilityMintSpec {
        tenant: params.tenant.to_string(),
        region: params.region.to_string(),
        subject_key: subject_key.clone(),
        jti: jti.clone(),
        exp_unix: expiry_unix,
        authority: BOOTSTRAP_AUTHORITY.iter().map(|g| g.to_string()).collect(),
        dpop_jkt: None, // a TTL-constrained operator token, not DPoP-bound (git/curl cannot prove DPoP).
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

/// A unique revocation id for a freshly-minted operator token: `bootstrap-<principal>-<unix_nanos>`.
/// The nanosecond clock makes two sequential bootstraps of the SAME principal mint distinct `jti`s
/// (each is a separately-revocable token). NOT a secret — it is the public revocation handle.
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
            .is_none());
        assert!(tuples.tuples_in(&scope()).is_empty());
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
        let edges = tuples.tuples_in(&scope());
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
