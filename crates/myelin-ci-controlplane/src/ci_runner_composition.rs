//! Production Identity composition for the dormant CI runner lane.
//!
//! This module owns the one construction path from the sealed durable cell token root and durable
//! S7 store to the claim-time issuer and final pre-spawn authorizer. It deliberately does not spawn
//! a runner: activation remains refused until the scoped reserve/release hooks, exact-tenant worker,
//! and crash matrix are composed around these authorities.

use std::sync::Arc;

use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_identity_service::{
    CellTokenAuthority, PasetoCapabilitySigner, PasetoCapabilityVerifier, RevocationStore,
    RunTokenMinter,
};
use myelin_storage::{
    DurableCellRootBacking, DurableRevocationBacking, SealKey, SubstrateProvider,
};

use crate::{
    CiJobQueueStore, IdentityCiJobCredentialMinter, IdentityCiJobLaunchAuthorizer,
    LockedManifestCiJobTokenIssuer,
};

/// Credential-free refusal from the production CI Identity composition root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiRunnerIdentityCompositionError {
    InvalidCellId,
    DurableCellRootUnavailable,
    InvalidCellRoot,
}

impl std::fmt::Display for CiRunnerIdentityCompositionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCellId => f.write_str("CI runner cell identity is invalid"),
            Self::DurableCellRootUnavailable => {
                f.write_str("CI runner durable cell token authority is unavailable")
            }
            Self::InvalidCellRoot => {
                f.write_str("CI runner durable cell token authority is invalid")
            }
        }
    }
}

impl std::error::Error for CiRunnerIdentityCompositionError {}

/// The two production Identity authorities consumed by the durable runner path.
#[derive(Clone)]
pub struct CiRunnerIdentityAuthorities {
    token_issuer: LockedManifestCiJobTokenIssuer,
    launch_authorizer: Arc<IdentityCiJobLaunchAuthorizer>,
}

impl CiRunnerIdentityAuthorities {
    pub fn token_issuer(&self) -> &LockedManifestCiJobTokenIssuer {
        &self.token_issuer
    }

    pub fn launch_authorizer(&self) -> Arc<IdentityCiJobLaunchAuthorizer> {
        self.launch_authorizer.clone()
    }
}

/// Recover the cell's sealed signing root and compose one shared durable S7 lifecycle into the real
/// PASETO claim minter and verifier.
///
/// The returned issuer first locks and reconstructs the exact scheduler/run/manifest authority. The
/// returned authorizer verifies the resulting signed credential and performs the one-shot durable
/// scheduler-generation launch CAS immediately before sandbox spawn.
pub async fn ci_runner_identity_authorities(
    provider: SubstrateProvider,
    cell_id: impl Into<String>,
    seal_key: &SealKey,
    rt: tokio::runtime::Handle,
) -> Result<CiRunnerIdentityAuthorities, CiRunnerIdentityCompositionError> {
    let cell_id = cell_id.into();
    if !valid_cell_id(&cell_id) {
        return Err(CiRunnerIdentityCompositionError::InvalidCellId);
    }

    let material = DurableCellRootBacking::new(provider.db_pool().clone(), cell_id)
        .load_or_generate(seal_key)
        .await
        .map_err(|_| CiRunnerIdentityCompositionError::DurableCellRootUnavailable)?;
    let cell = Arc::new(
        CellTokenAuthority::from_material(&material)
            .map_err(|_| CiRunnerIdentityCompositionError::InvalidCellRoot)?,
    );
    let revocations =
        RevocationStore::with_pg(DurableRevocationBacking::new(provider.clone()), rt.clone());
    let signer = Arc::new(PasetoCapabilitySigner::new(cell.clone()));
    let minter = RunTokenMinter::with_signer_and_tuples(revocations.clone(), None, signer);
    let verifier = Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor()));
    let region = provider.config().region.clone();
    let token_issuer = LockedManifestCiJobTokenIssuer::new(
        provider.db_pool().clone(),
        region.clone(),
        Arc::new(IdentityCiJobCredentialMinter::new(minter)),
    );
    let launch_authorizer = Arc::new(IdentityCiJobLaunchAuthorizer::new(
        RunTokenAuthorizer::new(verifier, revocations),
        CiJobQueueStore::with_pg(provider.db_pool().clone()),
        myelin_tenancy::Region(region),
        rt,
    ));

    Ok(CiRunnerIdentityAuthorities {
        token_issuer,
        launch_authorizer,
    })
}

fn valid_cell_id(cell_id: &str) -> bool {
    !cell_id.is_empty()
        && cell_id.trim() == cell_id
        && cell_id.len() <= 128
        && !cell_id.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::valid_cell_id;

    #[test]
    fn cell_id_is_canonical_before_durable_root_lookup() {
        assert!(valid_cell_id("cell-eu-1"));
        assert!(!valid_cell_id(""));
        assert!(!valid_cell_id(" cell-eu-1"));
        assert!(!valid_cell_id("cell-eu-1 "));
        assert!(!valid_cell_id("cell-\neu-1"));
        assert!(!valid_cell_id(&"x".repeat(129)));
    }
}
