use sqlx::postgres::PgPool;

use myelin_events::relay::{BusTransport, DrainReport, MAX_PUBLISH_ATTEMPTS};
use myelin_events::{EventId, OutboxError, OutboxRow, Result, Timestamp};

use crate::pgrelay::{PgRelay, RetainedRowsError};

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

fn unavailable_outbox_read(operation: &str) -> OutboxError {
    OutboxError(format!(
        "durable outbox {operation} read is unavailable; state is unknown"
    ))
}

fn bounded_retained_rows_error(error: RetainedRowsError) -> OutboxError {
    match error {
        RetainedRowsError::TooManyRows => {
            OutboxError("retained outbox snapshot exceeds its row limit".into())
        }
        RetainedRowsError::TooManyEnvelopeBytes => {
            OutboxError("retained outbox snapshot exceeds its envelope byte limit".into())
        }
        RetainedRowsError::Storage => unavailable_outbox_read("retained rows"),
    }
}

fn require_outbox_read<T>(operation: &str, result: Result<T>) -> T {
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
        require_outbox_read("depth", self.try_outbox_depth())
    }

    fn try_outbox_depth(&self) -> Result<usize> {
        self.block(async { self.relay().unsent_depth().await })
            .map(|depth| depth.max(0) as usize)
            .map_err(|_| unavailable_outbox_read("depth"))
    }

    fn dead_letter_count(&self) -> usize {
        require_outbox_read("dead-letter count", self.try_dead_letter_count())
    }

    fn try_dead_letter_count(&self) -> Result<usize> {
        self.block(async { self.relay().dead_count().await })
            .map(|count| count.max(0) as usize)
            .map_err(|_| unavailable_outbox_read("dead-letter count"))
    }

    fn oldest_unsent_recorded_at(&self) -> Option<Timestamp> {
        require_outbox_read(
            "oldest-unsent timestamp",
            self.try_oldest_unsent_recorded_at(),
        )
    }

    fn try_oldest_unsent_recorded_at(&self) -> Result<Option<Timestamp>> {
        self.block(async { self.relay().oldest_unsent_recorded_at().await })
            .map(|timestamp| timestamp.map(Timestamp))
            .map_err(|_| unavailable_outbox_read("oldest-unsent timestamp"))
    }

    fn committed_count(&self) -> usize {
        require_outbox_read("committed count", self.try_committed_count())
    }

    fn try_committed_count(&self) -> Result<usize> {
        self.block(async { self.relay().committed_live_count().await })
            .map(|count| count.max(0) as usize)
            .map_err(|_| unavailable_outbox_read("committed count"))
    }

    fn row(&self, id: &EventId) -> Option<OutboxRow> {
        require_outbox_read("row", self.try_row(id))
    }

    fn try_row(&self, id: &EventId) -> Result<Option<OutboxRow>> {
        self.block(async { self.relay().committed_row(id).await })
            .map_err(|_| unavailable_outbox_read("row"))
    }

    fn committed_rows(&self) -> Vec<OutboxRow> {
        require_outbox_read("committed rows", self.try_committed_rows())
    }

    fn try_committed_rows(&self) -> Result<Vec<OutboxRow>> {
        self.block(async {
            self.relay()
                .committed_live_rows()
                .await
                .map_err(|_| unavailable_outbox_read("committed rows"))
        })
    }

    fn try_retained_rows(&self) -> Result<Vec<OutboxRow>> {
        self.block(async {
            self.relay()
                .retained_rows()
                .await
                .map_err(|_| unavailable_outbox_read("retained rows"))
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
                .map_err(bounded_retained_rows_error)
        })
    }

    fn dead_letters(&self) -> Vec<OutboxRow> {
        require_outbox_read("dead-letter rows", self.try_dead_letters())
    }

    fn try_dead_letters(&self) -> Result<Vec<OutboxRow>> {
        self.block(async { self.relay().dead_rows().await })
            .map_err(|_| unavailable_outbox_read("dead-letter rows"))
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
    use myelin_events::OutboxError;

    #[test]
    fn infallible_read_boundary_fails_loud_without_logging_database_detail() {
        let panic = std::panic::catch_unwind(|| {
            require_outbox_read::<usize>(
                "depth",
                Err(OutboxError("sentinel database detail".to_string())),
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
