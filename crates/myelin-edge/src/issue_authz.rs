//! Production composition adapter for the Issues authorization-bootstrap saga.
//!
//! Issues cannot depend on the Identity service crate without creating a subsystem/service cycle.
//! The edge already composes both leaves, so this module binds the exact staged tuple intent to the
//! durable Identity `TupleStore`. The synchronous Identity ABI is isolated on Tokio's blocking pool;
//! no database/RPC bridge blocks an async request or recovery-worker executor thread.

use myelin_events::Timestamp;
use myelin_identity::{
    ObjectId, Principal, PrincipalId, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_issues::{
    IssueAuthorizationBinding, IssueAuthorizationOutcome, IssueAuthorizer, IssueStoreError,
    IssueTupleWriter, PgIssueStore,
};
use myelin_storage::TenantScope;
use std::future::Future;
use std::pin::Pin;

/// Concrete production adapter over the same durable tuple store used by Identity checks.
#[derive(Clone)]
pub struct IdentityIssueTupleWriter {
    tuples: TupleStore,
}

impl IdentityIssueTupleWriter {
    pub fn new(tuples: TupleStore) -> Self {
        Self { tuples }
    }

    /// Build from the live Identity engine so writes and authorization reads cannot accidentally
    /// use parallel tuple stores.
    pub fn from_identity(identity: &StoreBackedCheck) -> Self {
        Self::new(identity.tuples().clone())
    }
}

/// Drive one bounded restart-recovery batch through the concrete Identity adapter. Individual
/// failures remain pending and are returned to the caller for metrics/backoff; one unavailable
/// object cannot prevent the rest of the tenant/region batch from converging.
pub async fn reconcile_pending_issue_authorizations<A: IssueAuthorizer>(
    store: &PgIssueStore<A>,
    identity: &StoreBackedCheck,
    worker: &Principal,
    limit: u32,
) -> Result<Vec<(String, Result<IssueAuthorizationOutcome, IssueStoreError>)>, IssueStoreError> {
    let writer = IdentityIssueTupleWriter::from_identity(identity);
    let pending = store.pending_authorization_ids(worker, limit).await?;
    let mut outcomes = Vec::with_capacity(pending.len());
    for issue_id in pending {
        let outcome = store
            .reconcile_authorization(worker, &issue_id, &writer)
            .await;
        outcomes.push((issue_id, outcome));
    }
    Ok(outcomes)
}

impl IssueTupleWriter for IdentityIssueTupleWriter {
    fn ensure_parent_project<'a>(
        &'a self,
        scope: &'a TenantScope,
        actor: &'a Principal,
        binding: &'a IssueAuthorizationBinding,
    ) -> Pin<Box<dyn Future<Output = Result<Zookie, String>> + Send + 'a>> {
        let tuples = self.tuples.clone();
        let scope = scope.clone();
        let actor = actor.clone();
        let delta = parent_project_delta(binding);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                tuples.write_tuples(&scope, &actor, &[delta], None, None, now_timestamp())
            })
            .await
            .map_err(|_| "identity_tuple_worker_join_failed".to_string())?
            .map_err(|_| "identity_tuple_write_failed".to_string())
        })
    }
}

fn parent_project_delta(binding: &IssueAuthorizationBinding) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(binding.issue_object.clone()),
        relation: RelName(binding.relation.clone()),
        subject: PrincipalId(binding.project_userset.clone()),
        caveat: None,
    })
}

fn now_timestamp() -> Timestamp {
    let now = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now());
    Timestamp(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_issues::IssueAuthorizationState;

    #[test]
    fn adapter_writes_the_exact_staged_parent_project_userset() {
        let binding = IssueAuthorizationBinding {
            issue_id: "33333333-3333-3333-3333-333333333333".into(),
            project_id: "11111111-1111-1111-1111-111111111111".into(),
            issue_object: "issue:33333333-3333-3333-3333-333333333333".into(),
            project_userset: "project:11111111-1111-1111-1111-111111111111#view".into(),
            relation: "parent_project".into(),
            request_event_id: "01J00000000000000000000000".into(),
            created_event_id: "01J00000000000000000000001".into(),
            state: IssueAuthorizationState::Pending,
            zookie: None,
            attempts: 0,
        };
        assert_eq!(
            parent_project_delta(&binding),
            TupleDelta::Add(RelationTuple {
                object: ObjectId(binding.issue_object),
                relation: RelName("parent_project".into()),
                subject: PrincipalId(binding.project_userset),
                caveat: None,
            })
        );
    }
}
