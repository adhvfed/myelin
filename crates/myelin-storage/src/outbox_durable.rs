use sqlx::postgres::PgPool;

use myelin_events::relay::{BusTransport, DrainReport, MAX_PUBLISH_ATTEMPTS};
use myelin_events::{EventId, OutboxError, OutboxRow, Result, Timestamp};

use crate::pg::PgError;
use crate::pgrelay::PgRelay;

#[derive(Clone)]
pub struct PgOutboxBacking {
    pool: PgPool,
    rt: tokio::runtime::Handle,
}

impl PgOutboxBacking {
    pub fn new(pool: PgPool, rt: tokio::runtime::Handle) -> PgOutboxBacking {
        PgOutboxBacking { pool, rt }
    }

    fn relay(&self) -> PgRelay {
        PgRelay::new(self.pool.clone())
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

fn require_outbox_read<T>(operation: &str, result: std::result::Result<T, PgError>) -> T {
    result.unwrap_or_else(|_| {
        panic!("FAIL-STATIC: durable outbox {operation} read failed; state is unknown")
    })
}

impl myelin_events::DurableOutboxBacking for PgOutboxBacking {
    fn commit_staged(&self, rows: Vec<OutboxRow>) -> Result<()> {
        self.block(async {
            self.relay()
                .commit_staged_atomic(&rows)
                .await
                .map_err(|e| OutboxError(e.to_string()))
        })
    }

    fn commit_staged_absorb(&self, rows: Vec<OutboxRow>) -> Result<()> {
        self.block(async {
            self.relay()
                .commit_staged_absorb(&rows)
                .await
                .map_err(|e| OutboxError(e.to_string()))
        })
    }

    fn outbox_depth(&self) -> usize {
        let depth = self.block(async { self.relay().unsent_depth().await });
        require_outbox_read("depth", depth).max(0) as usize
    }

    fn dead_letter_count(&self) -> usize {
        let count = self.block(async { self.relay().dead_count().await });
        require_outbox_read("dead-letter count", count).max(0) as usize
    }

    fn oldest_unsent_recorded_at(&self) -> Option<Timestamp> {
        let timestamp = self.block(async { self.relay().oldest_unsent_recorded_at().await });
        require_outbox_read("oldest-unsent timestamp", timestamp).map(Timestamp)
    }

    fn committed_count(&self) -> usize {
        let count = self.block(async { self.relay().committed_live_count().await });
        require_outbox_read("committed count", count).max(0) as usize
    }

    fn row(&self, id: &EventId) -> Option<OutboxRow> {
        let row = self.block(async { self.relay().committed_row(id).await });
        require_outbox_read("row", row)
    }

    fn committed_rows(&self) -> Vec<OutboxRow> {
        let rows = self.block(async { self.relay().committed_live_rows().await });
        require_outbox_read("committed rows", rows)
    }

    fn try_committed_rows(&self) -> Result<Vec<OutboxRow>> {
        self.block(async {
            self.relay()
                .committed_live_rows()
                .await
                .map_err(|e| OutboxError(e.to_string()))
        })
    }

    fn try_retained_rows(&self) -> Result<Vec<OutboxRow>> {
        self.block(async {
            self.relay()
                .retained_rows()
                .await
                .map_err(|e| OutboxError(e.to_string()))
        })
    }

    fn try_retained_rows_bounded(
        &self,
        maximum_rows: usize,
        maximum_envelope_bytes: usize,
    ) -> Result<Vec<OutboxRow>> {
        self.block(async {
            self.relay()
                .retained_rows_bounded(maximum_rows, maximum_envelope_bytes)
                .await
                .map_err(|e| OutboxError(e.to_string()))
        })
    }

    fn dead_letters(&self) -> Vec<OutboxRow> {
        let rows = self.block(async { self.relay().dead_rows().await });
        require_outbox_read("dead-letter rows", rows)
    }

    fn drain_once(&self, transport: &dyn BusTransport, batch: usize) -> Result<DrainReport> {
        self.block(async {
            self.relay()
                .drain_once_dead_letter(transport, batch as i64, MAX_PUBLISH_ATTEMPTS)
                .await
                .map_err(|e| OutboxError(e.to_string()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::require_outbox_read;
    use crate::pg::PgError;

    #[test]
    fn infallible_read_boundary_fails_loud_without_logging_database_detail() {
        let panic = std::panic::catch_unwind(|| {
            require_outbox_read::<usize>(
                "depth",
                Err(PgError::Query("sentinel database detail".to_string())),
            )
        })
        .expect_err("a durable read failure must not become a zero value");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("panic payload is a string");
        assert!(message.contains("durable outbox depth read failed"));
        assert!(!message.contains("sentinel database detail"));
    }
}
