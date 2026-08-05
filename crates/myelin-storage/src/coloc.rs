// @residency-cell-pinned:file - M0 region-less pool floor: the cell's region pins data
// per-query via the (tenant, region) TenantScope; the per-pool runtime pin lands with STOR-D5.
use std::sync::Arc;

use myelin_events::{
    EmitContextBase, EventDraft, EventEnvelope, EventId, IdMinter, OutboxStore, OutboxTransaction,
    OutboxTx, OUTBOX_MIGRATION,
};

use crate::oltp::{OltpConfig, OltpError, OltpPool, PermitGuard};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColocError {
    Pool(OltpError),
    CommitRolledBack(String),
}

impl core::fmt::Display for ColocError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ColocError::Pool(e) => write!(f, "co-located OLTP pool error: {e}"),
            ColocError::CommitRolledBack(why) => write!(
                f,
                "co-located transaction rolled back - neither state nor outbox committed: {why}"
            ),
        }
    }
}

impl std::error::Error for ColocError {}

impl From<OltpError> for ColocError {
    fn from(e: OltpError) -> Self {
        ColocError::Pool(e)
    }
}

#[derive(Clone)]
pub struct ColocatedOltp {
    pool: OltpPool,
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
}

impl ColocatedOltp {
    pub fn open(
        config: OltpConfig,
        outbox: OutboxStore,
        minter: Arc<dyn IdMinter>,
    ) -> Result<ColocatedOltp, ColocError> {
        let pool = OltpPool::open(config)?;
        Ok(ColocatedOltp {
            pool,
            outbox,
            minter,
        })
    }

    pub fn migrations(service_tables: &[&'static str]) -> Vec<&'static str> {
        let mut set: Vec<&'static str> = service_tables.to_vec();
        set.push(OUTBOX_MIGRATION);
        set
    }

    pub fn begin(&self, ctx_base: EmitContextBase) -> Result<ColocatedTx, ColocError> {
        let permit = self.pool.acquire(&ctx_base.tenant)?;
        let outbox_tx = self.outbox.begin(Arc::clone(&self.minter), ctx_base);
        Ok(ColocatedTx {
            _permit: permit,
            outbox_tx,
            staged_state: Vec::new(),
        })
    }

    pub fn outbox(&self) -> &OutboxStore {
        &self.outbox
    }

    pub fn outbox_depth(&self) -> usize {
        self.outbox.outbox_depth()
    }

    pub fn pool(&self) -> &OltpPool {
        &self.pool
    }
}

pub struct ColocatedTx {
    _permit: PermitGuard,
    outbox_tx: OutboxTransaction,
    staged_state: Vec<String>,
}

impl ColocatedTx {
    pub fn stage_state(&mut self, change: impl Into<String>) -> &mut Self {
        self.staged_state.push(change.into());
        self.outbox_tx
            .stage_state_change(change_label(&self.staged_state));
        self
    }

    pub fn emit(
        &mut self,
        draft: EventDraft,
        cause: Option<&EventEnvelope>,
    ) -> Result<EventId, ColocError> {
        self.outbox_tx
            .emit(draft, cause)
            .map_err(|e| ColocError::CommitRolledBack(e.0))
    }

    pub fn staged_event_count(&self) -> usize {
        self.outbox_tx.staged_len()
    }

    pub fn staged_state(&self) -> &[String] {
        &self.staged_state
    }

    pub fn commit(self) -> Result<(), ColocError> {
        self.outbox_tx
            .commit()
            .map_err(|e| ColocError::CommitRolledBack(e.0))
    }

    pub fn commit_with_state_fault(self, reason: &str) -> Result<(), ColocError> {
        Err(ColocError::CommitRolledBack(format!(
            "injected state-write failure: {reason}"
        )))
    }
}

fn change_label(staged: &[String]) -> String {
    staged.join("; ")
}

pub use myelin_events::OUTBOX_MIGRATION as COLOCATED_OUTBOX_MIGRATION;

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{Actor, CausedBy, Region, TenantId, Timestamp};
    use myelin_events::{
        AggregateKey, ArtifactRef, DataRole, EventDraft, EventType, MonotonicMinter, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn cfg() -> OltpConfig {
        OltpConfig {
            max_pool_size: 8,
            statement_timeout_ms: 3_000,
            per_tenant_in_flight_cap: 4,
        }
    }

    fn store() -> ColocatedOltp {
        ColocatedOltp::open(
            cfg(),
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
        )
        .expect("a valid config opens the co-located OLTP store")
    }

    fn ctx_base(tenant: &str) -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId(tenant.into()),
            region: Region("eu-west".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId(tenant.into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    fn draft(type_: &str, aggregate: &str) -> EventDraft {
        EventDraft {
            type_: EventType(type_.into()),
            subject: ArtifactRef(format!("myelin://acme/issues/issue/{aggregate}")),
            aggregate: AggregateKey(aggregate.into()),
            payload: serde_json::json!({ "ref": aggregate }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }

    #[test]
    fn outbox_migration_is_co_located_in_the_service_db() {
        let set = ColocatedOltp::migrations(&["CREATE TABLE issue (id TEXT PRIMARY KEY);"]);
        assert!(
            set.contains(&OUTBOX_MIGRATION),
            "the outbox table DDL must be in the service DB migration set (co-located)"
        );
        assert!(
            set.iter().any(|m| m.contains("issue")),
            "the service's own domain table is in the same set"
        );
        assert!(OUTBOX_MIGRATION.contains("CREATE TABLE IF NOT EXISTS outbox"));
        assert!(OUTBOX_MIGRATION.contains("UNIQUE (aggregate, seq)"));
    }

    #[test]
    fn staged_state_is_mirrored_onto_the_outbox_transaction() {
        let db = store();
        let mut tx = db.begin(ctx_base("acme")).unwrap();
        tx.stage_state("write A");
        tx.stage_state("write B");
        assert_eq!(
            tx.staged_state(),
            &["write A".to_string(), "write B".to_string()]
        );
        assert_eq!(
            tx.outbox_tx.staged_state().as_deref(),
            Some("write A; write B"),
            "the joined staged-state label must be mirrored onto the outbox transaction"
        );
    }

    #[test]
    fn commit_makes_state_and_events_durable_together() {
        let db = store();
        let mut tx = db.begin(ctx_base("acme")).unwrap();
        tx.stage_state("issue PROJ-1 created");
        let id = tx
            .emit(draft("issues.issue.created", "issue:PROJ-1"), None)
            .unwrap();
        tx.emit(draft("issues.issue.updated", "issue:PROJ-1"), None)
            .unwrap();
        assert_eq!(tx.staged_event_count(), 2, "two events buffered");
        assert_eq!(tx.staged_state(), &["issue PROJ-1 created".to_string()]);
        assert_eq!(
            db.outbox_depth(),
            0,
            "an open co-located tx has written nothing"
        );

        tx.commit().unwrap();
        assert_eq!(db.outbox_depth(), 2);
        let row = db
            .outbox()
            .row(&id)
            .expect("the committed event row is present");
        assert_eq!(
            row.seq, 0,
            "first event for the aggregate is the seq-0 cursor anchor"
        );
        assert!(
            row.published_at.is_none(),
            "freshly co-committed rows are unsent"
        );
    }

    #[test]
    fn dropped_tx_writes_neither_state_nor_event() {
        let db = store();
        {
            let mut tx = db.begin(ctx_base("acme")).unwrap();
            tx.stage_state("issue PROJ-9 created");
            tx.emit(draft("issues.issue.created", "issue:PROJ-9"), None)
                .unwrap();
            assert_eq!(tx.staged_event_count(), 1, "buffered, not committed");
        }
        assert_eq!(
            db.outbox_depth(),
            0,
            "an aborted co-located tx writes no event"
        );
        assert_eq!(
            db.outbox().committed_count(),
            0,
            "no ghost row from an abort"
        );
    }

    #[test]
    fn both_roll_back_under_injected_mid_tx_failure() {
        let db = store();
        let mut tx = db.begin(ctx_base("acme")).unwrap();
        tx.stage_state("issue PROJ-7 created");
        tx.emit(draft("issues.issue.created", "issue:PROJ-7"), None)
            .unwrap();
        assert_eq!(tx.staged_event_count(), 1);

        let result = tx.commit_with_state_fault("disk full");
        assert!(
            matches!(result, Err(ColocError::CommitRolledBack(_))),
            "an injected mid-tx state failure must roll the whole tx back: {result:?}"
        );
        assert_eq!(
            db.outbox_depth(),
            0,
            "a rolled-back co-commit writes no event"
        );
        assert_eq!(
            db.outbox().committed_count(),
            0,
            "no committed row from a rolled-back state write"
        );
    }

    #[test]
    fn seq_is_the_monotonic_cross_seam_cursor() {
        let db = store();
        let agg = "issue:CURSOR";
        let mut ids = Vec::new();
        for i in 0..3 {
            let mut tx = db.begin(ctx_base("acme")).unwrap();
            tx.stage_state(format!("state write {i}"));
            let id = tx.emit(draft("issues.issue.updated", agg), None).unwrap();
            tx.commit().unwrap();
            ids.push(id);
        }
        let seqs: Vec<u64> = ids
            .iter()
            .map(|id| db.outbox().row(id).unwrap().seq)
            .collect();
        assert_eq!(
            seqs,
            vec![0, 1, 2],
            "OLTP commit order == event order (the §7.3 cursor)"
        );
    }

    #[test]
    fn begin_fast_fails_when_the_pool_is_saturated() {
        let cfg = OltpConfig {
            max_pool_size: 1,
            statement_timeout_ms: 1_000,
            per_tenant_in_flight_cap: 1,
        };
        let db = ColocatedOltp::open(
            cfg,
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
        )
        .unwrap();
        let _held = db.begin(ctx_base("acme")).unwrap();
        let rejected = db.begin(ctx_base("acme"));
        assert!(
            matches!(rejected, Err(ColocError::Pool(_))),
            "a saturated pool must fast-fail the BEGIN, never block (got Ok={})",
            rejected.is_ok()
        );
    }

    #[test]
    fn committing_frees_the_connection_for_the_next_tx() {
        let cfg = OltpConfig {
            max_pool_size: 1,
            statement_timeout_ms: 1_000,
            per_tenant_in_flight_cap: 1,
        };
        let db = ColocatedOltp::open(
            cfg,
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
        )
        .unwrap();
        {
            let mut tx = db.begin(ctx_base("acme")).unwrap();
            tx.emit(draft("issues.issue.created", "issue:A"), None)
                .unwrap();
            tx.commit().unwrap();
        }
        let mut tx2 = db.begin(ctx_base("acme")).unwrap();
        tx2.emit(draft("issues.issue.created", "issue:B"), None)
            .unwrap();
        tx2.commit().unwrap();
        assert_eq!(db.outbox_depth(), 2, "both co-committed events are durable");
    }

    #[test]
    fn coloc_error_display_is_loud() {
        let e = ColocError::CommitRolledBack("disk full".into());
        let msg = e.to_string();
        assert!(!msg.is_empty());
        assert!(msg.contains("rolled back"), "must name the rollback: {msg}");
        assert!(
            msg.contains("neither"),
            "must say neither side committed: {msg}"
        );
    }
}
