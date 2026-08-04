use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use myelin_ci_sandbox::{CheckoutAuthorizationScope, RunTokenCredential};
use myelin_storage::{with_tenant_tx_error, PgError};
use myelin_tenancy::{Region, TenantId};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::ci_claim_token_issuer::{
    authority_from_durable_claim, verify_claim_window, verify_locked_claim,
};
use crate::ci_drive_manifest::CiDriveManifestStore;
use crate::ci_manifest_job_runner::{
    CiJobTokenIssueError, CiJobTokenRequest, MAX_CI_JOB_TOKEN_TTL_SECS,
};
use crate::ci_run_store::CiRunStore;
use crate::job_queue_store::CiJobQueueStore;
use crate::job_spec_store::CiJobSpecStore;

pub const CI_PHASE_CREDENTIAL_V1_DOMAIN: &[u8] = b"myelin.ci.phase-credential.v1\0";

pub const CI_PHASE_CREDENTIAL_GENERATION_PREFIX: &str = "ci-credential:v1:";

pub const CI_PHASE_CREDENTIAL_BINDING_V1: i16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CiCredentialPurpose {
    CheckoutAdvertise,
    CheckoutFetch,
    CheckoutMaterialization,
    Workload,
}

impl CiCredentialPurpose {
    pub fn token(self) -> &'static str {
        match self {
            Self::CheckoutAdvertise => "checkout_advertise",
            Self::CheckoutFetch => "checkout_fetch",
            Self::CheckoutMaterialization => "checkout_materialization",
            Self::Workload => "workload",
        }
    }

    pub fn ordinal(self) -> i16 {
        match self {
            Self::CheckoutAdvertise => 1,
            Self::CheckoutFetch => 2,
            Self::CheckoutMaterialization => 3,
            Self::Workload => 4,
        }
    }

    fn required_predecessor(self) -> Option<CiCredentialPurpose> {
        match self {
            Self::CheckoutAdvertise => None,
            Self::CheckoutFetch => Some(Self::CheckoutAdvertise),
            Self::CheckoutMaterialization => Some(Self::CheckoutFetch),
            Self::Workload => Some(Self::CheckoutMaterialization),
        }
    }

    pub fn from_token(token: &str) -> Option<CiCredentialPurpose> {
        match token {
            "checkout_advertise" => Some(Self::CheckoutAdvertise),
            "checkout_fetch" => Some(Self::CheckoutFetch),
            "checkout_materialization" => Some(Self::CheckoutMaterialization),
            "workload" => Some(Self::Workload),
            _ => None,
        }
    }

    pub fn is_preparation(self) -> bool {
        !matches!(self, Self::Workload)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CiJobCredentialWriteVersion {
    #[default]
    V1ClaimBound,
    V2PhaseBound,
}

#[derive(Clone, Copy, Debug)]
pub struct CiPhaseGenerationInputs<'a> {
    pub tenant_id: &'a str,
    pub region: &'a str,
    pub wf_run_id: &'a str,
    pub ci_run_id: &'a str,
    pub job_id: &'a str,
    pub token_authority_handle: &'a str,
    pub idem_token: &'a str,
    pub lease_owner: &'a str,
    pub lease_epoch: i64,
    pub claim_nonce: &'a str,
    pub claim_started_at_epoch_secs: i64,
    pub claim_expires_at_epoch_secs: i64,
    pub purpose: CiCredentialPurpose,
    pub issued_at_epoch_secs: i64,
    pub expires_at_epoch_secs: i64,
    pub binding_version: i16,
}

pub fn phase_generation_id(inputs: CiPhaseGenerationInputs<'_>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CI_PHASE_CREDENTIAL_V1_DOMAIN);
    for value in [
        inputs.tenant_id,
        inputs.region,
        inputs.wf_run_id,
        inputs.ci_run_id,
        inputs.job_id,
        inputs.token_authority_handle,
        inputs.idem_token,
        inputs.lease_owner,
        inputs.claim_nonce,
        inputs.purpose.token(),
    ] {
        hash_length_prefixed(&mut hasher, value.as_bytes());
    }
    hasher.update(&inputs.lease_epoch.to_be_bytes());
    hasher.update(&inputs.claim_started_at_epoch_secs.to_be_bytes());
    hasher.update(&inputs.claim_expires_at_epoch_secs.to_be_bytes());
    hasher.update(&inputs.purpose.ordinal().to_be_bytes());
    hasher.update(&inputs.issued_at_epoch_secs.to_be_bytes());
    hasher.update(&inputs.expires_at_epoch_secs.to_be_bytes());
    hasher.update(&inputs.binding_version.to_be_bytes());
    format!(
        "{CI_PHASE_CREDENTIAL_GENERATION_PREFIX}{}",
        hasher.finalize()
    )
}

fn hash_length_prefixed(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiPhaseCredentialBinding {
    pub binding_version: i16,
    pub purpose: CiCredentialPurpose,
    pub generation_id: String,
    pub jti: String,
    pub issued_at_epoch_secs: i64,
    pub expires_at_epoch_secs: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiCredentialGenerationOutcome {
    Applied,
    Replayed,
}

#[derive(Clone, Debug)]
pub struct MintedPhaseCredential {
    pub credential: RunTokenCredential,
    pub binding: CiPhaseCredentialBinding,
    pub checkout: Option<CheckoutAuthorizationScope>,
    pub outcome: CiCredentialGenerationOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiCredentialGenerationError {
    WriteVersionPinned,
    InvalidClaim,
    WrongRegion,
    ClaimUnavailable,
    DurableAuthorityUnavailable,
    PurposeUnavailableForJobShape,
    OutOfOrderGeneration,
    JournalPredicateUnmet,
    MissingParentAttempt,
    GenerationExpired,
    GenerationDivergence,
    ExpiryOutOfRange,
    IdentityRefused,
    Database,
}

impl std::fmt::Display for CiCredentialGenerationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = match self {
            Self::WriteVersionPinned => {
                "the credential store is pinned to the V1 claim-bound write version"
            }
            Self::InvalidClaim => "the presented claim is invalid",
            Self::WrongRegion => "the presented claim is outside this cell region",
            Self::ClaimUnavailable => "the exact scheduler claim generation is not live",
            Self::DurableAuthorityUnavailable => {
                "durable run, manifest, or launch authority is unavailable"
            }
            Self::PurposeUnavailableForJobShape => {
                "the requested credential purpose does not exist for this job shape"
            }
            Self::OutOfOrderGeneration => {
                "the requested purpose is not the next generation for this claim"
            }
            Self::JournalPredicateUnmet => {
                "the prelaunch journal is not in the state this purpose requires"
            }
            Self::MissingParentAttempt => "the exact durable parent attempt is absent",
            Self::GenerationExpired => {
                "this purpose's generation has expired; a claim never remints a phase"
            }
            Self::GenerationDivergence => {
                "the durable generation differs from the recomputed binding"
            }
            Self::ExpiryOutOfRange => {
                "the credential anchor or expiry is outside the supported range"
            }
            Self::IdentityRefused => "Identity refused or returned a divergent credential",
            Self::Database => "the durable credential-generation transaction failed",
        };
        write!(f, "CI phase credential refused: {detail}")
    }
}

impl std::error::Error for CiCredentialGenerationError {}

impl From<PgError> for CiCredentialGenerationError {
    fn from(_: PgError) -> Self {
        Self::Database
    }
}

fn map_sql_error(_error: sqlx::Error) -> CiCredentialGenerationError {
    CiCredentialGenerationError::Database
}

#[derive(Clone, Debug)]
pub struct CiPhaseCredentialMintRequest {
    pub claim: CiJobTokenRequest,
    pub reserve_id: String,
    pub checkout: Option<CheckoutAuthorizationScope>,
    pub purpose: CiCredentialPurpose,
    pub generation_id: String,
    pub issued_at_epoch_secs: i64,
    pub expires_at_epoch_secs: i64,
}

pub trait CiPhaseCredentialMinter: Send + Sync {
    fn mint_phase<'a>(
        &'a self,
        request: CiPhaseCredentialMintRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RunTokenCredential, CiJobTokenIssueError>> + Send + 'a>>;
}

#[derive(Clone)]
pub struct CiJobCredentialGenerationStore {
    pool: PgPool,
    region: String,
    write_version: CiJobCredentialWriteVersion,
    minter: Arc<dyn CiPhaseCredentialMinter>,
}

impl CiJobCredentialGenerationStore {
    pub fn with_pg(
        pool: PgPool,
        region: impl Into<String>,
        minter: Arc<dyn CiPhaseCredentialMinter>,
    ) -> Self {
        Self::with_pg_and_write_version(
            pool,
            region,
            minter,
            CiJobCredentialWriteVersion::V1ClaimBound,
        )
    }

    pub fn with_pg_and_write_version(
        pool: PgPool,
        region: impl Into<String>,
        minter: Arc<dyn CiPhaseCredentialMinter>,
        write_version: CiJobCredentialWriteVersion,
    ) -> Self {
        Self {
            pool,
            region: region.into(),
            write_version,
            minter,
        }
    }

    pub fn write_version(&self) -> CiJobCredentialWriteVersion {
        self.write_version
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub async fn mint_phase_credential(
        &self,
        claim: &CiJobTokenRequest,
        purpose: CiCredentialPurpose,
    ) -> Result<MintedPhaseCredential, CiCredentialGenerationError> {
        self.mint_phase_credential_inner(claim, purpose, None).await
    }

    pub async fn mint_phase_credential_for_checkout_scope(
        &self,
        claim: &CiJobTokenRequest,
        purpose: CiCredentialPurpose,
        expected_checkout: Option<&CheckoutAuthorizationScope>,
    ) -> Result<MintedPhaseCredential, CiCredentialGenerationError> {
        self.mint_phase_credential_inner(claim, purpose, Some(expected_checkout.cloned()))
            .await
    }

    async fn mint_phase_credential_inner(
        &self,
        claim: &CiJobTokenRequest,
        purpose: CiCredentialPurpose,
        expected_checkout: Option<Option<CheckoutAuthorizationScope>>,
    ) -> Result<MintedPhaseCredential, CiCredentialGenerationError> {
        if self.write_version != CiJobCredentialWriteVersion::V2PhaseBound {
            return Err(CiCredentialGenerationError::WriteVersionPinned);
        }
        claim
            .validate()
            .map_err(|_| CiCredentialGenerationError::InvalidClaim)?;
        if claim.region != self.region {
            return Err(CiCredentialGenerationError::WrongRegion);
        }

        let claim = claim.clone();
        let tenant_id = claim.tenant_id.clone();
        let region = claim.region.clone();
        let manifest_store = CiDriveManifestStore::new(
            self.pool.clone(),
            TenantId(tenant_id.clone()),
            Region(region.clone()),
        )
        .map_err(|_| CiCredentialGenerationError::InvalidClaim)?;
        let spec_store = CiJobSpecStore::with_pg(self.pool.clone());
        let minter = self.minter.clone();

        with_tenant_tx_error(&self.pool, &tenant_id, &region, move |connection| {
            Box::pin(async move {
                let locked = CiJobQueueStore::lock_for_token_mint_on_conn(
                    connection,
                    &claim.tenant_id,
                    &claim.region,
                    &claim.job_id,
                    &claim.wf_run_id,
                )
                .await
                .map_err(|_| CiCredentialGenerationError::ClaimUnavailable)?
                .ok_or(CiCredentialGenerationError::ClaimUnavailable)?;
                verify_locked_claim(&claim, &locked)
                    .map_err(|_| CiCredentialGenerationError::ClaimUnavailable)?;
                if locked.state != "leased" {
                    return Err(CiCredentialGenerationError::ClaimUnavailable);
                }
                let liveness = live_generation_facts(connection, &claim).await?;

                let run = CiRunStore::lock_for_token_mint_on_conn(
                    connection,
                    &claim.tenant_id,
                    &claim.region,
                    &claim.ci_run_id,
                    &claim.wf_run_id,
                )
                .await
                .map_err(|_| CiCredentialGenerationError::DurableAuthorityUnavailable)?
                .ok_or(CiCredentialGenerationError::DurableAuthorityUnavailable)?;
                let (manifest, _) = manifest_store
                    .load_by_identity_on_conn(connection, &claim.wf_run_id, &claim.ci_run_id)
                    .await
                    .map_err(|_| CiCredentialGenerationError::DurableAuthorityUnavailable)?
                    .ok_or(CiCredentialGenerationError::DurableAuthorityUnavailable)?;
                let launch = spec_store
                    .get_launch_template_on_conn(connection, &claim.tenant_id, &claim.job_id)
                    .await
                    .map_err(|_| CiCredentialGenerationError::DurableAuthorityUnavailable)?;
                let authority = authority_from_durable_claim(&claim, &run, &manifest, &launch)
                    .map_err(|_| CiCredentialGenerationError::DurableAuthorityUnavailable)?;
                verify_supplied_checkout_matches_durable(
                    expected_checkout.as_ref(),
                    &authority.checkout,
                )?;
                if locked.stage.as_deref() != Some(authority.stage.as_str())
                    || locked.trust_tier != authority.trust_tier
                {
                    return Err(CiCredentialGenerationError::DurableAuthorityUnavailable);
                }
                verify_claim_window(&claim, &locked, &launch)
                    .map_err(|_| CiCredentialGenerationError::DurableAuthorityUnavailable)?;
                let checkout_bearing = authority.checkout.is_some();
                match (purpose, checkout_bearing) {
                    (CiCredentialPurpose::Workload, _) => {}
                    (_, true) => {}
                    (_, false) => {
                        return Err(CiCredentialGenerationError::PurposeUnavailableForJobShape)
                    }
                }
                if purpose.is_preparation() && locked.claim_window_secs.is_none() {
                    return Err(CiCredentialGenerationError::DurableAuthorityUnavailable);
                }

                lock_credential_generation_job(
                    connection,
                    &claim.tenant_id,
                    &claim.region,
                    &claim.job_id,
                )
                .await?;
                let current = current_generation(connection, &claim).await?;
                let existing = existing_generation(connection, &claim, purpose).await?;
                verify_purpose_precondition(
                    connection,
                    &claim,
                    purpose,
                    checkout_bearing,
                    current.as_ref(),
                    existing.is_some(),
                )
                .await?;

                let inputs_of = |issued: i64, expires: i64| CiPhaseGenerationInputs {
                    tenant_id: &claim.tenant_id,
                    region: &claim.region,
                    wf_run_id: &claim.wf_run_id,
                    ci_run_id: &claim.ci_run_id,
                    job_id: &claim.job_id,
                    token_authority_handle: &claim.token_authority_handle,
                    idem_token: &claim.idem_token,
                    lease_owner: &claim.lease_owner,
                    lease_epoch: claim.lease_epoch,
                    claim_nonce: &claim.claim_nonce,
                    claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
                    claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
                    purpose,
                    issued_at_epoch_secs: issued,
                    expires_at_epoch_secs: expires,
                    binding_version: CI_PHASE_CREDENTIAL_BINDING_V1,
                };

                let (binding, outcome) = match existing {
                    Some(row) => {
                        if row.binding_version != CI_PHASE_CREDENTIAL_BINDING_V1 {
                            return Err(CiCredentialGenerationError::GenerationDivergence);
                        }
                        let recomputed =
                            phase_generation_id(inputs_of(row.issued_at, row.expires_at));
                        let expected_jti = crate::ci_identity_adapter::expected_phase_jti(
                            &recomputed,
                            row.issued_at,
                        )
                        .map_err(|_| CiCredentialGenerationError::ExpiryOutOfRange)?;
                        if recomputed != row.generation_id || expected_jti != row.jti {
                            return Err(CiCredentialGenerationError::GenerationDivergence);
                        }
                        if row.expires_at <= liveness.now_epoch_secs {
                            return Err(CiCredentialGenerationError::GenerationExpired);
                        }
                        (
                            CiPhaseCredentialBinding {
                                binding_version: row.binding_version,
                                purpose,
                                generation_id: row.generation_id,
                                jti: row.jti,
                                issued_at_epoch_secs: row.issued_at,
                                expires_at_epoch_secs: row.expires_at,
                            },
                            CiCredentialGenerationOutcome::Replayed,
                        )
                    }
                    None => {
                        let issued = liveness.now_epoch_secs;
                        let expires = deterministic_phase_expiry(issued, &claim)?;
                        if issued < claim.claim_started_at_epoch_secs {
                            return Err(CiCredentialGenerationError::ExpiryOutOfRange);
                        }
                        let generation_id = phase_generation_id(inputs_of(issued, expires));
                        let jti =
                            crate::ci_identity_adapter::expected_phase_jti(&generation_id, issued)
                                .map_err(|_| CiCredentialGenerationError::ExpiryOutOfRange)?;
                        insert_generation(
                            connection,
                            &claim,
                            purpose,
                            issued,
                            expires,
                            &generation_id,
                            &jti,
                        )
                        .await?;
                        (
                            CiPhaseCredentialBinding {
                                binding_version: CI_PHASE_CREDENTIAL_BINDING_V1,
                                purpose,
                                generation_id,
                                jti,
                                issued_at_epoch_secs: issued,
                                expires_at_epoch_secs: expires,
                            },
                            CiCredentialGenerationOutcome::Applied,
                        )
                    }
                };

                let credential = minter
                    .mint_phase(CiPhaseCredentialMintRequest {
                        claim: claim.clone(),
                        reserve_id: authority
                            .reserve_id
                            .clone()
                            .ok_or(CiCredentialGenerationError::DurableAuthorityUnavailable)?,
                        checkout: authority.checkout.clone(),
                        purpose,
                        generation_id: binding.generation_id.clone(),
                        issued_at_epoch_secs: binding.issued_at_epoch_secs,
                        expires_at_epoch_secs: binding.expires_at_epoch_secs,
                    })
                    .await
                    .map_err(|_| CiCredentialGenerationError::IdentityRefused)?;
                validate_phase_credential(&claim, &binding, &credential)?;

                Ok(MintedPhaseCredential {
                    credential,
                    binding,
                    checkout: authority.checkout,
                    outcome,
                })
            })
        })
        .await
    }
}

fn verify_supplied_checkout_matches_durable(
    expected_checkout: Option<&Option<CheckoutAuthorizationScope>>,
    durable_checkout: &Option<CheckoutAuthorizationScope>,
) -> Result<(), CiCredentialGenerationError> {
    if expected_checkout.is_some_and(|expected| expected != durable_checkout) {
        Err(CiCredentialGenerationError::DurableAuthorityUnavailable)
    } else {
        Ok(())
    }
}

pub(crate) fn deterministic_phase_expiry(
    anchor_epoch_secs: i64,
    claim: &CiJobTokenRequest,
) -> Result<i64, CiCredentialGenerationError> {
    let ceiling = i64::try_from(MAX_CI_JOB_TOKEN_TTL_SECS)
        .map_err(|_| CiCredentialGenerationError::ExpiryOutOfRange)?;
    anchor_epoch_secs
        .checked_add(ceiling)
        .map(|token_ceiling| token_ceiling.min(claim.claim_expires_at_epoch_secs))
        .filter(|expiry| *expiry > anchor_epoch_secs)
        .ok_or(CiCredentialGenerationError::ExpiryOutOfRange)
}

pub(crate) fn validate_phase_credential(
    claim: &CiJobTokenRequest,
    binding: &CiPhaseCredentialBinding,
    credential: &RunTokenCredential,
) -> Result<(), CiCredentialGenerationError> {
    if credential.jti != binding.jti || credential.jti == claim.token_authority_handle {
        return Err(CiCredentialGenerationError::IdentityRefused);
    }
    let expected_ttl = u64::try_from(
        binding
            .expires_at_epoch_secs
            .checked_sub(binding.issued_at_epoch_secs)
            .ok_or(CiCredentialGenerationError::ExpiryOutOfRange)?,
    )
    .map_err(|_| CiCredentialGenerationError::ExpiryOutOfRange)?;
    if credential.ttl_secs() != expected_ttl || credential.ttl_secs() > MAX_CI_JOB_TOKEN_TTL_SECS {
        return Err(CiCredentialGenerationError::IdentityRefused);
    }
    if binding.expires_at_epoch_secs > claim.claim_expires_at_epoch_secs {
        return Err(CiCredentialGenerationError::ExpiryOutOfRange);
    }
    Ok(())
}

struct LiveGenerationFacts {
    now_epoch_secs: i64,
}

async fn live_generation_facts(
    connection: &mut sqlx::PgConnection,
    claim: &CiJobTokenRequest,
) -> Result<LiveGenerationFacts, CiCredentialGenerationError> {
    let row = sqlx::query(
        "SELECT FLOOR(EXTRACT(EPOCH FROM statement_timestamp()))::bigint AS now_epoch_secs,
                COALESCE(q.lease_expires > statement_timestamp(), false) AS lease_live,
                (q.completion_receipt IS NULL) AS uncompleted,
                EXISTS (
                  SELECT 1 FROM ci_job AS surface
                  WHERE surface.tenant_id = q.tenant_id
                    AND surface.region = q.region
                    AND surface.job_id = q.job_id
                    AND surface.state IN ('queued', 'leased')
                ) AS surface_live
         FROM job_queue AS q
         WHERE q.tenant_id = $1 AND q.region = $2 AND q.job_id = $3::uuid AND q.run_id = $4::uuid",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(&claim.wf_run_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sql_error)?
    .ok_or(CiCredentialGenerationError::ClaimUnavailable)?;
    let lease_live: bool = row
        .try_get("lease_live")
        .map_err(|_| CiCredentialGenerationError::Database)?;
    let uncompleted: bool = row
        .try_get("uncompleted")
        .map_err(|_| CiCredentialGenerationError::Database)?;
    let surface_live: bool = row
        .try_get("surface_live")
        .map_err(|_| CiCredentialGenerationError::Database)?;
    if !lease_live || !uncompleted || !surface_live {
        return Err(CiCredentialGenerationError::ClaimUnavailable);
    }
    Ok(LiveGenerationFacts {
        now_epoch_secs: row
            .try_get("now_epoch_secs")
            .map_err(|_| CiCredentialGenerationError::Database)?,
    })
}

async fn lock_credential_generation_job(
    connection: &mut sqlx::PgConnection,
    tenant_id: &str,
    region: &str,
    job_id: &str,
) -> Result<(), CiCredentialGenerationError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "myelin.ci.parent-attempt.v1:{tenant_id}:{region}:{job_id}"
        ))
        .execute(&mut *connection)
        .await
        .map_err(map_sql_error)?;
    Ok(())
}

pub(crate) struct DurableGeneration {
    pub purpose: CiCredentialPurpose,
    pub binding_version: i16,
    pub generation_id: String,
    pub jti: String,
    pub issued_at: i64,
    pub expires_at: i64,
}

fn generation_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<DurableGeneration, CiCredentialGenerationError> {
    let token: String = row
        .try_get("purpose")
        .map_err(|_| CiCredentialGenerationError::Database)?;
    Ok(DurableGeneration {
        purpose: CiCredentialPurpose::from_token(&token)
            .ok_or(CiCredentialGenerationError::Database)?,
        binding_version: row
            .try_get("binding_version")
            .map_err(|_| CiCredentialGenerationError::Database)?,
        generation_id: row
            .try_get("generation_id")
            .map_err(|_| CiCredentialGenerationError::Database)?,
        jti: row
            .try_get("jti")
            .map_err(|_| CiCredentialGenerationError::Database)?,
        issued_at: row
            .try_get("issued_at_epoch_secs")
            .map_err(|_| CiCredentialGenerationError::Database)?,
        expires_at: row
            .try_get("expires_at_epoch_secs")
            .map_err(|_| CiCredentialGenerationError::Database)?,
    })
}

async fn current_generation(
    connection: &mut sqlx::PgConnection,
    claim: &CiJobTokenRequest,
) -> Result<Option<DurableGeneration>, CiCredentialGenerationError> {
    let row = sqlx::query(
        "SELECT purpose, binding_version, generation_id, jti,
                issued_at_epoch_secs, expires_at_epoch_secs
         FROM ci_job_credential_generation
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND lease_epoch = $4 AND claim_nonce = $5::uuid
         ORDER BY phase_ordinal DESC
         LIMIT 1",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    row.as_ref().map(generation_from_row).transpose()
}

async fn existing_generation(
    connection: &mut sqlx::PgConnection,
    claim: &CiJobTokenRequest,
    purpose: CiCredentialPurpose,
) -> Result<Option<DurableGeneration>, CiCredentialGenerationError> {
    let row = sqlx::query(
        "SELECT purpose, binding_version, generation_id, jti,
                issued_at_epoch_secs, expires_at_epoch_secs
         FROM ci_job_credential_generation
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND lease_epoch = $4 AND claim_nonce = $5::uuid AND purpose = $6",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .bind(purpose.token())
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    row.as_ref().map(generation_from_row).transpose()
}

async fn verify_purpose_precondition(
    connection: &mut sqlx::PgConnection,
    claim: &CiJobTokenRequest,
    purpose: CiCredentialPurpose,
    checkout_bearing: bool,
    current: Option<&DurableGeneration>,
    replaying: bool,
) -> Result<(), CiCredentialGenerationError> {
    if let Some(current) = current {
        if current.purpose.ordinal() > purpose.ordinal() {
            return Err(CiCredentialGenerationError::OutOfOrderGeneration);
        }
    }
    if !replaying {
        match purpose.required_predecessor() {
            None => {
                if current.is_some() {
                    return Err(CiCredentialGenerationError::OutOfOrderGeneration);
                }
            }
            Some(required) => {
                if !checkout_bearing {
                    if current.is_some() {
                        return Err(CiCredentialGenerationError::OutOfOrderGeneration);
                    }
                } else if current.map(|row| row.purpose) != Some(required) {
                    return Err(CiCredentialGenerationError::OutOfOrderGeneration);
                }
            }
        }
    }
    if purpose == CiCredentialPurpose::CheckoutAdvertise {
        return Ok(());
    }
    if purpose == CiCredentialPurpose::Workload && !checkout_bearing {
        return Ok(());
    }
    require_parent_attempt(connection, claim).await?;
    let transport = phase_status(connection, claim, "checkout_transport").await?;
    let materialization = phase_status(connection, claim, "checkout_materialization").await?;
    let ok = match purpose {
        CiCredentialPurpose::CheckoutFetch => transport.as_deref() == Some("started"),
        CiCredentialPurpose::CheckoutMaterialization => {
            transport.as_deref() == Some("measured")
                && materialization.as_deref() == Some("started")
        }
        CiCredentialPurpose::Workload => {
            transport.as_deref() == Some("measured")
                && materialization.as_deref() == Some("measured")
        }
        CiCredentialPurpose::CheckoutAdvertise => true,
    };
    if ok {
        Ok(())
    } else {
        Err(CiCredentialGenerationError::JournalPredicateUnmet)
    }
}

async fn require_parent_attempt(
    connection: &mut sqlx::PgConnection,
    claim: &CiJobTokenRequest,
) -> Result<(), CiCredentialGenerationError> {
    let found = sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM ci_job_parent_attempt
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND wf_run_id = $4::uuid AND ci_run_id = $5::uuid
           AND lease_owner = $6 AND lease_epoch = $7 AND claim_nonce = $8::uuid
           AND claim_started_at_epoch_secs = $9 AND claim_expires_at_epoch_secs = $10",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(&claim.wf_run_id)
    .bind(&claim.ci_run_id)
    .bind(&claim.lease_owner)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .bind(claim.claim_started_at_epoch_secs)
    .bind(claim.claim_expires_at_epoch_secs)
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    if found.is_some() {
        Ok(())
    } else {
        Err(CiCredentialGenerationError::MissingParentAttempt)
    }
}

async fn phase_status(
    connection: &mut sqlx::PgConnection,
    claim: &CiJobTokenRequest,
    phase: &str,
) -> Result<Option<String>, CiCredentialGenerationError> {
    sqlx::query_scalar::<_, String>(
        "SELECT status FROM ci_job_prelaunch_usage
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND lease_epoch = $4 AND claim_nonce = $5::uuid AND phase = $6",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .bind(phase)
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sql_error)
}

#[allow(clippy::too_many_arguments)]
async fn insert_generation(
    connection: &mut sqlx::PgConnection,
    claim: &CiJobTokenRequest,
    purpose: CiCredentialPurpose,
    issued_at: i64,
    expires_at: i64,
    generation_id: &str,
    jti: &str,
) -> Result<(), CiCredentialGenerationError> {
    let inserted = sqlx::query(
        "INSERT INTO ci_job_credential_generation (
           tenant_id, region, job_id, wf_run_id, ci_run_id, token_authority_handle, idem_token,
           lease_owner, lease_epoch, claim_nonce, claim_started_at_epoch_secs,
           claim_expires_at_epoch_secs, binding_version, purpose, phase_ordinal,
           issued_at_epoch_secs, expires_at_epoch_secs, generation_id, jti
         ) VALUES ($1, $2, $3::uuid, $4::uuid, $5::uuid, $6, $7, $8, $9, $10::uuid, $11, $12,
                   $13, $14, $15, $16, $17, $18, $19)",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(&claim.wf_run_id)
    .bind(&claim.ci_run_id)
    .bind(&claim.token_authority_handle)
    .bind(&claim.idem_token)
    .bind(&claim.lease_owner)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .bind(claim.claim_started_at_epoch_secs)
    .bind(claim.claim_expires_at_epoch_secs)
    .bind(CI_PHASE_CREDENTIAL_BINDING_V1)
    .bind(purpose.token())
    .bind(purpose.ordinal())
    .bind(issued_at)
    .bind(expires_at)
    .bind(generation_id)
    .bind(jti)
    .execute(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    if inserted.rows_affected() == 1 {
        Ok(())
    } else {
        Err(CiCredentialGenerationError::Database)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiPhaseGenerationGate {
    pub tenant_id: String,
    pub region: String,
    pub wf_run_id: String,
    pub ci_run_id: String,
    pub job_id: String,
    pub token_authority_handle: String,
    pub idem_token: String,
    pub checkout_commit: Option<String>,
    pub lease_owner: String,
    pub lease_epoch: i64,
    pub claim_nonce: String,
    pub claim_started_at_epoch_secs: i64,
    pub claim_expires_at_epoch_secs: i64,
    pub purpose: CiCredentialPurpose,
    pub binding_version: i16,
    pub generation_id: String,
    pub jti: String,
    pub issued_at_epoch_secs: i64,
    pub expires_at_epoch_secs: i64,
}

fn required_journal_statuses(
    purpose: CiCredentialPurpose,
) -> (Option<&'static str>, Option<&'static str>) {
    match purpose {
        CiCredentialPurpose::CheckoutAdvertise | CiCredentialPurpose::CheckoutFetch => {
            (Some("started"), None)
        }
        CiCredentialPurpose::CheckoutMaterialization => (Some("measured"), Some("started")),
        CiCredentialPurpose::Workload => (Some("measured"), Some("measured")),
    }
}

pub const VERIFY_PHASE_GENERATION_QUERY: &str = "\
SELECT 1
FROM job_queue AS q
JOIN ci_job_credential_generation AS g
  ON g.tenant_id = q.tenant_id AND g.region = q.region AND g.job_id = q.job_id
 AND g.lease_epoch = q.lease_epoch AND g.claim_nonce = q.claim_nonce
JOIN ci_job_spec AS launch
  ON launch.tenant_id = q.tenant_id AND launch.region = q.region
 AND launch.job_id = q.job_id AND launch.run_id = q.run_id
WHERE q.tenant_id = $1
  AND q.region = $2
  AND q.job_id = $3::uuid
  AND q.run_id = $4::uuid
  AND q.state = 'leased'
  AND q.lease_owner = $5
  AND q.lease_epoch = $6
  AND q.claim_nonce = $7::uuid
  AND FLOOR(EXTRACT(EPOCH FROM q.claim_started_at))::bigint = $8
  AND FLOOR(EXTRACT(EPOCH FROM q.claim_expires_at))::bigint = $9
  AND q.claim_expires_at > statement_timestamp()
  AND q.lease_expires > statement_timestamp()
  AND q.completion_receipt IS NULL
  AND g.purpose = $10
  AND g.binding_version = $11
  AND g.generation_id = $12
  AND g.jti = $13
  AND g.issued_at_epoch_secs = $14
  AND g.expires_at_epoch_secs = $15
  AND g.wf_run_id = q.run_id
  AND g.ci_run_id = $16::uuid
  AND g.token_authority_handle = $17
  AND g.idem_token = $18
  AND (launch.spec #>> '{spec,workspace,commit}') IS NOT DISTINCT FROM $21::text
  AND g.lease_owner = q.lease_owner
  AND g.claim_started_at_epoch_secs = $8
  AND g.claim_expires_at_epoch_secs = $9
  AND g.expires_at_epoch_secs > FLOOR(EXTRACT(EPOCH FROM statement_timestamp()))::bigint
  AND NOT EXISTS (
    SELECT 1
    FROM ci_job_credential_generation AS successor
    WHERE successor.tenant_id = g.tenant_id
      AND successor.region = g.region
      AND successor.job_id = g.job_id
      AND successor.lease_epoch = g.lease_epoch
      AND successor.claim_nonce = g.claim_nonce
      AND successor.phase_ordinal > g.phase_ordinal
  )
  AND EXISTS (
    SELECT 1
    FROM ci_job_parent_attempt AS parent
    WHERE parent.tenant_id = q.tenant_id
      AND parent.region = q.region
      AND parent.job_id = q.job_id
      AND parent.wf_run_id = q.run_id
      AND parent.ci_run_id = $16::uuid
      AND parent.lease_owner = q.lease_owner
      AND parent.lease_epoch = q.lease_epoch
      AND parent.claim_nonce = q.claim_nonce
      AND parent.claim_started_at_epoch_secs = $8
      AND parent.claim_expires_at_epoch_secs = $9
  )
  AND EXISTS (
    SELECT 1
    FROM ci_job AS surface
    WHERE surface.tenant_id = q.tenant_id
      AND surface.region = q.region
      AND surface.job_id = q.job_id
      AND surface.state IN ('queued', 'leased')
  )
  AND ($19::text IS NULL OR EXISTS (
    SELECT 1
    FROM ci_job_prelaunch_usage AS transport
    WHERE transport.tenant_id = q.tenant_id
      AND transport.region = q.region
      AND transport.job_id = q.job_id
      AND transport.lease_epoch = q.lease_epoch
      AND transport.claim_nonce = q.claim_nonce
      AND transport.phase = 'checkout_transport'
      AND transport.status = $19
  ))
  AND ($20::text IS NULL OR EXISTS (
    SELECT 1
    FROM ci_job_prelaunch_usage AS materialization
    WHERE materialization.tenant_id = q.tenant_id
      AND materialization.region = q.region
      AND materialization.job_id = q.job_id
      AND materialization.lease_epoch = q.lease_epoch
      AND materialization.claim_nonce = q.claim_nonce
      AND materialization.phase = 'checkout_materialization'
      AND materialization.status = $20
  ))";

pub async fn verify_phase_generation_live(
    pool: &PgPool,
    gate: &CiPhaseGenerationGate,
) -> Result<bool, CiCredentialGenerationError> {
    if gate.purpose == CiCredentialPurpose::Workload {
        return Err(CiCredentialGenerationError::PurposeUnavailableForJobShape);
    }
    let (transport, materialization) = required_journal_statuses(gate.purpose);
    let gate = gate.clone();
    let live = myelin_storage::with_tenant_tx(
        pool,
        &gate.tenant_id.clone(),
        &gate.region.clone(),
        move |connection| {
            Box::pin(async move {
                let row: Option<i32> = sqlx::query_scalar(VERIFY_PHASE_GENERATION_QUERY)
                    .bind(&gate.tenant_id)
                    .bind(&gate.region)
                    .bind(&gate.job_id)
                    .bind(&gate.wf_run_id)
                    .bind(&gate.lease_owner)
                    .bind(gate.lease_epoch)
                    .bind(&gate.claim_nonce)
                    .bind(gate.claim_started_at_epoch_secs)
                    .bind(gate.claim_expires_at_epoch_secs)
                    .bind(gate.purpose.token())
                    .bind(gate.binding_version)
                    .bind(&gate.generation_id)
                    .bind(&gate.jti)
                    .bind(gate.issued_at_epoch_secs)
                    .bind(gate.expires_at_epoch_secs)
                    .bind(&gate.ci_run_id)
                    .bind(&gate.token_authority_handle)
                    .bind(&gate.idem_token)
                    .bind(transport)
                    .bind(materialization)
                    .bind(&gate.checkout_commit)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(|error| PgError::Query(error.to_string()))?;
                Ok(row.is_some())
            })
        },
    )
    .await
    .map_err(CiCredentialGenerationError::from)?;
    Ok(live)
}

pub fn lock_phase_generation_query() -> String {
    format!("{VERIFY_PHASE_GENERATION_QUERY}\nFOR SHARE OF q")
}

pub struct RetainedCiPhaseGeneration {
    transaction: Option<Transaction<'static, Postgres>>,
    gate: CiPhaseGenerationGate,
}

impl RetainedCiPhaseGeneration {
    pub async fn validate(&mut self) -> Result<(), CiCredentialGenerationError> {
        let gate = self.gate.clone();
        let transaction = self
            .transaction
            .as_mut()
            .ok_or(CiCredentialGenerationError::Database)?;
        if phase_generation_predicate_holds(transaction, &gate, false).await? {
            Ok(())
        } else {
            Err(CiCredentialGenerationError::ClaimUnavailable)
        }
    }

    pub async fn release(mut self) -> Result<(), CiCredentialGenerationError> {
        match self.transaction.take() {
            Some(transaction) => transaction
                .rollback()
                .await
                .map_err(|_| CiCredentialGenerationError::Database),
            None => Ok(()),
        }
    }
}

async fn phase_generation_predicate_holds(
    connection: &mut sqlx::PgConnection,
    gate: &CiPhaseGenerationGate,
    lock: bool,
) -> Result<bool, CiCredentialGenerationError> {
    let (transport, materialization) = required_journal_statuses(gate.purpose);
    let owned_query;
    let query_text = if lock {
        owned_query = lock_phase_generation_query();
        owned_query.as_str()
    } else {
        VERIFY_PHASE_GENERATION_QUERY
    };
    let row: Option<i32> = sqlx::query_scalar(query_text)
        .bind(&gate.tenant_id)
        .bind(&gate.region)
        .bind(&gate.job_id)
        .bind(&gate.wf_run_id)
        .bind(&gate.lease_owner)
        .bind(gate.lease_epoch)
        .bind(&gate.claim_nonce)
        .bind(gate.claim_started_at_epoch_secs)
        .bind(gate.claim_expires_at_epoch_secs)
        .bind(gate.purpose.token())
        .bind(gate.binding_version)
        .bind(&gate.generation_id)
        .bind(&gate.jti)
        .bind(gate.issued_at_epoch_secs)
        .bind(gate.expires_at_epoch_secs)
        .bind(&gate.ci_run_id)
        .bind(&gate.token_authority_handle)
        .bind(&gate.idem_token)
        .bind(transport)
        .bind(materialization)
        .bind(&gate.checkout_commit)
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_sql_error)?;
    Ok(row.is_some())
}

async fn lock_purpose_journal_rows(
    connection: &mut sqlx::PgConnection,
    gate: &CiPhaseGenerationGate,
) -> Result<(), CiCredentialGenerationError> {
    let (transport, materialization) = required_journal_statuses(gate.purpose);
    let mut phases: Vec<&str> = Vec::new();
    if transport.is_some() {
        phases.push("checkout_transport");
    }
    if materialization.is_some() {
        phases.push("checkout_materialization");
    }
    if phases.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "SELECT 1
         FROM ci_job_prelaunch_usage
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND lease_epoch = $4 AND claim_nonce = $5::uuid
           AND phase = ANY($6)
         ORDER BY phase
         FOR SHARE",
    )
    .bind(&gate.tenant_id)
    .bind(&gate.region)
    .bind(&gate.job_id)
    .bind(gate.lease_epoch)
    .bind(&gate.claim_nonce)
    .bind(&phases)
    .fetch_all(&mut *connection)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}

pub async fn acquire_phase_generation_ownership(
    pool: &PgPool,
    gate: &CiPhaseGenerationGate,
) -> Result<Option<RetainedCiPhaseGeneration>, CiCredentialGenerationError> {
    if gate.purpose == CiCredentialPurpose::Workload {
        return Err(CiCredentialGenerationError::PurposeUnavailableForJobShape);
    }
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| CiCredentialGenerationError::Database)?;
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true),
                set_config('myelin.region', $2, true)",
    )
    .bind(&gate.tenant_id)
    .bind(&gate.region)
    .execute(&mut *transaction)
    .await
    .map_err(map_sql_error)?;

    if !phase_generation_predicate_holds(&mut transaction, gate, true).await? {
        transaction
            .rollback()
            .await
            .map_err(|_| CiCredentialGenerationError::Database)?;
        return Ok(None);
    }
    lock_purpose_journal_rows(&mut transaction, gate).await?;
    if !phase_generation_predicate_holds(&mut transaction, gate, false).await? {
        transaction
            .rollback()
            .await
            .map_err(|_| CiCredentialGenerationError::Database)?;
        return Ok(None);
    }
    Ok(Some(RetainedCiPhaseGeneration {
        transaction: Some(transaction),
        gate: gate.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim() -> CiJobTokenRequest {
        CiJobTokenRequest {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            project_id: "55555555-5555-4555-8555-555555555555".into(),
            wf_run_id: "11111111-1111-4111-8111-111111111111".into(),
            ci_run_id: "22222222-2222-4222-8222-222222222222".into(),
            job_id: "33333333-3333-4333-8333-333333333333".into(),
            token_authority_handle: format!("ci-token-authority:v2:{}", "a".repeat(64)),
            idem_token: "idem:test".into(),
            lease_owner: "runner:test".into(),
            lease_epoch: 7,
            claim_nonce: "44444444-4444-4444-8444-444444444444".into(),
            claim_started_at_epoch_secs: 1_785_000_000,
            claim_expires_at_epoch_secs: 1_785_004_800,
        }
    }

    fn inputs<'a>(
        claim: &'a CiJobTokenRequest,
        purpose: CiCredentialPurpose,
    ) -> CiPhaseGenerationInputs<'a> {
        CiPhaseGenerationInputs {
            tenant_id: &claim.tenant_id,
            region: &claim.region,
            wf_run_id: &claim.wf_run_id,
            ci_run_id: &claim.ci_run_id,
            job_id: &claim.job_id,
            token_authority_handle: &claim.token_authority_handle,
            idem_token: &claim.idem_token,
            lease_owner: &claim.lease_owner,
            lease_epoch: claim.lease_epoch,
            claim_nonce: &claim.claim_nonce,
            claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
            claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
            purpose,
            issued_at_epoch_secs: 1_785_000_100,
            expires_at_epoch_secs: 1_785_000_400,
            binding_version: CI_PHASE_CREDENTIAL_BINDING_V1,
        }
    }

    const ALL_PURPOSES: [CiCredentialPurpose; 4] = [
        CiCredentialPurpose::CheckoutAdvertise,
        CiCredentialPurpose::CheckoutFetch,
        CiCredentialPurpose::CheckoutMaterialization,
        CiCredentialPurpose::Workload,
    ];

    #[test]
    fn mint_time_checkout_commit_divergence_is_refused_against_durable_authority() {
        let durable = myelin_ci_sandbox::derive_checkout_authorization_scope(
            myelin_ci_sandbox::JobKind::Ci,
            &myelin_ci_sandbox::WorkspaceSpec {
                repo_ref: Some("myelin://acme/git/repo/core".into()),
                commit: Some("a".repeat(40)),
            },
        )
        .unwrap();
        let supplied = myelin_ci_sandbox::derive_checkout_authorization_scope(
            myelin_ci_sandbox::JobKind::Ci,
            &myelin_ci_sandbox::WorkspaceSpec {
                repo_ref: Some("myelin://acme/git/repo/core".into()),
                commit: Some("b".repeat(40)),
            },
        )
        .unwrap();
        assert_eq!(
            verify_supplied_checkout_matches_durable(Some(&supplied), &durable),
            Err(CiCredentialGenerationError::DurableAuthorityUnavailable)
        );
        assert!(verify_supplied_checkout_matches_durable(Some(&durable), &durable).is_ok());
    }

    #[test]
    fn retained_phase_predicate_exact_compares_checkout_commit_to_durable_spec() {
        for predicate in [
            "JOIN ci_job_spec AS launch",
            "launch.job_id = q.job_id AND launch.run_id = q.run_id",
            "(launch.spec #>> '{spec,workspace,commit}') IS NOT DISTINCT FROM $21::text",
        ] {
            assert!(
                VERIFY_PHASE_GENERATION_QUERY.contains(predicate),
                "retained phase predicate must bind `{predicate}`"
            );
            assert!(
                lock_phase_generation_query().contains(predicate),
                "locking retained phase predicate must bind `{predicate}`"
            );
        }
    }

    #[test]
    fn purpose_tokens_and_ordinals_are_the_schema_vocabulary() {
        assert_eq!(
            ALL_PURPOSES.map(|p| (p.token(), p.ordinal())),
            [
                ("checkout_advertise", 1),
                ("checkout_fetch", 2),
                ("checkout_materialization", 3),
                ("workload", 4),
            ]
        );
        for purpose in ALL_PURPOSES {
            assert_eq!(
                CiCredentialPurpose::from_token(purpose.token()),
                Some(purpose)
            );
        }
        assert_eq!(CiCredentialPurpose::from_token("workload_v2"), None);
    }

    #[test]
    fn the_predecessor_chain_is_the_exact_phase_order() {
        assert_eq!(
            CiCredentialPurpose::CheckoutAdvertise.required_predecessor(),
            None
        );
        assert_eq!(
            CiCredentialPurpose::CheckoutFetch.required_predecessor(),
            Some(CiCredentialPurpose::CheckoutAdvertise)
        );
        assert_eq!(
            CiCredentialPurpose::CheckoutMaterialization.required_predecessor(),
            Some(CiCredentialPurpose::CheckoutFetch)
        );
        assert_eq!(
            CiCredentialPurpose::Workload.required_predecessor(),
            Some(CiCredentialPurpose::CheckoutMaterialization)
        );
    }

    #[test]
    fn the_generation_digest_separates_every_purpose_and_every_bound_field() {
        let base = claim();
        let ids: Vec<String> = ALL_PURPOSES
            .iter()
            .map(|purpose| phase_generation_id(inputs(&base, *purpose)))
            .collect();
        let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), 4, "each purpose has its own generation id");
        for id in &ids {
            assert!(id.starts_with(CI_PHASE_CREDENTIAL_GENERATION_PREFIX));
            assert_eq!(
                id.len(),
                CI_PHASE_CREDENTIAL_GENERATION_PREFIX.len() + 64,
                "the digest is a full blake3 hex"
            );
        }

        let reference = phase_generation_id(inputs(&base, CiCredentialPurpose::Workload));
        type Mutation = (&'static str, fn(&mut CiJobTokenRequest));
        let claim_mutations: [Mutation; 10] = [
            ("tenant", |c| c.tenant_id = "globex".into()),
            ("region", |c| c.region = "eu-west".into()),
            ("workflow run", |c| {
                c.wf_run_id = "55555555-5555-4555-8555-555555555555".into()
            }),
            ("CI run", |c| {
                c.ci_run_id = "66666666-6666-4666-8666-666666666666".into()
            }),
            ("job", |c| {
                c.job_id = "77777777-7777-4777-8777-777777777777".into()
            }),
            ("authority handle", |c| {
                c.token_authority_handle = format!("ci-token-authority:v2:{}", "b".repeat(64))
            }),
            ("idem token", |c| c.idem_token = "idem:other".into()),
            ("owner", |c| c.lease_owner = "runner:other".into()),
            ("epoch", |c| c.lease_epoch += 1),
            ("nonce", |c| {
                c.claim_nonce = "88888888-8888-4888-8888-888888888888".into()
            }),
        ];
        for (label, mutate) in claim_mutations {
            let mut mutated = claim();
            mutate(&mut mutated);
            assert_ne!(
                reference,
                phase_generation_id(inputs(&mutated, CiCredentialPurpose::Workload)),
                "the digest must bind {label}"
            );
        }

        let mut started = inputs(&base, CiCredentialPurpose::Workload);
        started.claim_started_at_epoch_secs += 1;
        assert_ne!(reference, phase_generation_id(started));
        let mut expires = inputs(&base, CiCredentialPurpose::Workload);
        expires.claim_expires_at_epoch_secs += 1;
        assert_ne!(reference, phase_generation_id(expires));
        let mut anchor = inputs(&base, CiCredentialPurpose::Workload);
        anchor.issued_at_epoch_secs += 1;
        assert_ne!(reference, phase_generation_id(anchor));
        let mut expiry = inputs(&base, CiCredentialPurpose::Workload);
        expiry.expires_at_epoch_secs += 1;
        assert_ne!(reference, phase_generation_id(expiry));
        let mut version = inputs(&base, CiCredentialPurpose::Workload);
        version.binding_version += 1;
        assert_ne!(reference, phase_generation_id(version));
    }

    #[test]
    fn the_generation_digest_is_externally_pinned() {
        assert_eq!(
            phase_generation_id(inputs(&claim(), CiCredentialPurpose::CheckoutAdvertise)),
            "ci-credential:v1:474e7dd42d69ab828675d88de32b95a1cadc2959a5cd89387700e0addc71635c"
        );
    }

    #[test]
    fn the_phase_expiry_is_capped_by_both_the_ttl_and_the_claim() {
        let claim = claim();
        assert_eq!(
            deterministic_phase_expiry(1_785_000_100, &claim).unwrap(),
            1_785_000_400
        );
        assert_eq!(
            deterministic_phase_expiry(1_785_004_700, &claim).unwrap(),
            1_785_004_800
        );
        assert_eq!(
            deterministic_phase_expiry(1_785_004_800, &claim),
            Err(CiCredentialGenerationError::ExpiryOutOfRange)
        );
        assert_eq!(
            deterministic_phase_expiry(i64::MAX, &claim),
            Err(CiCredentialGenerationError::ExpiryOutOfRange)
        );
    }

    #[test]
    fn the_write_version_defaults_to_the_claim_bound_production_pin() {
        assert_eq!(
            CiJobCredentialWriteVersion::default(),
            CiJobCredentialWriteVersion::V1ClaimBound
        );
    }

    #[test]
    fn credential_validation_requires_exact_jti_and_exact_ttl() {
        let claim = claim();
        let binding = CiPhaseCredentialBinding {
            binding_version: CI_PHASE_CREDENTIAL_BINDING_V1,
            purpose: CiCredentialPurpose::Workload,
            generation_id: "ci-credential:v1:deadbeef".into(),
            jti: "runtok:svc:ci:ci-credential:v1:deadbeef:2026-07-30T00:00:00Z".into(),
            issued_at_epoch_secs: 1_785_000_100,
            expires_at_epoch_secs: 1_785_000_400,
        };
        let good = RunTokenCredential::new("bearer", &binding.jti, 300).unwrap();
        validate_phase_credential(&claim, &binding, &good).unwrap();

        let wrong_jti = RunTokenCredential::new("bearer", "runtok:other", 300).unwrap();
        assert_eq!(
            validate_phase_credential(&claim, &binding, &wrong_jti),
            Err(CiCredentialGenerationError::IdentityRefused)
        );
        let wrong_ttl = RunTokenCredential::new("bearer", &binding.jti, 299).unwrap();
        assert_eq!(
            validate_phase_credential(&claim, &binding, &wrong_ttl),
            Err(CiCredentialGenerationError::IdentityRefused)
        );
        let copied_handle =
            RunTokenCredential::new("bearer", &claim.token_authority_handle, 300).unwrap();
        let handle_binding = CiPhaseCredentialBinding {
            jti: claim.token_authority_handle.clone(),
            ..binding.clone()
        };
        assert_eq!(
            validate_phase_credential(&claim, &handle_binding, &copied_handle),
            Err(CiCredentialGenerationError::IdentityRefused)
        );
        let claim_overlong = CiPhaseCredentialBinding {
            expires_at_epoch_secs: claim.claim_expires_at_epoch_secs + 1,
            issued_at_epoch_secs: claim.claim_expires_at_epoch_secs + 1 - 300,
            ..binding
        };
        let overlong = RunTokenCredential::new("bearer", &claim_overlong.jti, 300).unwrap();
        assert_eq!(
            validate_phase_credential(&claim, &claim_overlong, &overlong),
            Err(CiCredentialGenerationError::ExpiryOutOfRange)
        );
    }
}
