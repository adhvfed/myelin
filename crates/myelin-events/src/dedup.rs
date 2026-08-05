use crate::ConsumerName;
use std::collections::HashSet;
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

pub const CONSUMER_DEDUP_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS consumer_dedup (
    consumer    TEXT        NOT NULL,
    event_id    TEXT        NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT consumer_dedup_pk PRIMARY KEY (consumer, event_id)
);";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoCommitError(pub String);

impl core::fmt::Display for CoCommitError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "consumer co-commit failed: {}", self.0)
    }
}

impl std::error::Error for CoCommitError {}

pub trait CoCommitTx: Send {
    fn connection(&mut self) -> Option<&mut dyn core::any::Any>;
    fn commit(self: Box<Self>) -> Result<(), CoCommitError>;
    fn rollback(self: Box<Self>);
}

pub trait DurableDedup: Send + Sync {
    fn mark_handled(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool;
    fn is_handled(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool;
    fn revert(&self, consumer: &ConsumerName, event_id: &crate::EventId);
    fn forget(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool;
    fn begin_co_commit(
        &self,
        consumer: &ConsumerName,
        event_id: &crate::EventId,
        tenant: &crate::TenantId,
        region: &crate::Region,
    ) -> (Box<dyn CoCommitTx>, bool);
}

#[derive(Clone)]
enum DedupBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<HashSet<(ConsumerName, crate::EventId)>>>),
    Durable(Arc<dyn DurableDedup>),
}

#[cfg(any(test, feature = "test-support"))]
impl Default for DedupBackend {
    fn default() -> Self {
        DedupBackend::Memory(Arc::new(Mutex::new(HashSet::new())))
    }
}

#[cfg(any(test, feature = "test-support"))]
struct MemoryCoCommit {
    set: Arc<Mutex<HashSet<(ConsumerName, crate::EventId)>>>,
    key: (ConsumerName, crate::EventId),
    inserted: bool,
}

#[cfg(any(test, feature = "test-support"))]
impl CoCommitTx for MemoryCoCommit {
    fn connection(&mut self) -> Option<&mut dyn core::any::Any> {
        None
    }
    fn commit(self: Box<Self>) -> Result<(), CoCommitError> {
        Ok(())
    }
    fn rollback(self: Box<Self>) {
        if self.inserted {
            self.set
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.key);
        }
    }
}

#[derive(Clone)]
#[cfg_attr(any(test, feature = "test-support"), derive(Default))]
pub struct DedupLedger {
    backend: DedupBackend,
}


impl DedupLedger {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn durable(backing: Arc<dyn DurableDedup>) -> Self {
        DedupLedger {
            backend: DedupBackend::Durable(backing),
        }
    }

    fn mem(&self) -> Option<std::sync::MutexGuard<'_, HashSet<(ConsumerName, crate::EventId)>>> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            DedupBackend::Memory(inner) => Some(inner.lock().unwrap_or_else(|e| e.into_inner())),
            DedupBackend::Durable(_) => None,
        }
    }

    pub fn mark_handled(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            DedupBackend::Memory(inner) => inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert((consumer.clone(), event_id.clone())),
            DedupBackend::Durable(d) => d.mark_handled(consumer, event_id),
        }
    }

    pub fn begin_co_commit(
        &self,
        consumer: &ConsumerName,
        event_id: &crate::EventId,
        tenant: &crate::TenantId,
        region: &crate::Region,
    ) -> (Box<dyn CoCommitTx>, bool) {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            DedupBackend::Memory(inner) => {
                let key = (consumer.clone(), event_id.clone());
                let fresh = inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key.clone());
                (
                    Box::new(MemoryCoCommit {
                        set: Arc::clone(inner),
                        key,
                        inserted: fresh,
                    }),
                    fresh,
                )
            }
            DedupBackend::Durable(d) => d.begin_co_commit(consumer, event_id, tenant, region),
        }
    }

    pub fn is_handled(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            DedupBackend::Memory(inner) => inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&(consumer.clone(), event_id.clone())),
            DedupBackend::Durable(d) => d.is_handled(consumer, event_id),
        }
    }

    pub fn revert(&self, consumer: &ConsumerName, event_id: &crate::EventId) {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            DedupBackend::Memory(inner) => {
                inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&(consumer.clone(), event_id.clone()));
            }
            DedupBackend::Durable(d) => d.revert(consumer, event_id),
        }
    }

    pub fn forget(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            DedupBackend::Memory(inner) => inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&(consumer.clone(), event_id.clone())),
            DedupBackend::Durable(d) => d.forget(consumer, event_id),
        }
    }

    pub fn len(&self) -> usize {
        self.mem().map(|s| s.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.mem().map(|s| s.is_empty()).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventId;

    fn consumer(name: &str) -> ConsumerName {
        ConsumerName(name.into())
    }

    #[test]
    fn same_consumer_event_inserted_twice_is_one_effect() {
        let ledger = DedupLedger::new();
        let c = consumer("indexer");
        let id = EventId("01J-1".into());

        assert!(
            ledger.mark_handled(&c, &id),
            "first delivery is FRESH → the handler runs"
        );
        assert!(
            !ledger.mark_handled(&c, &id),
            "redelivery is a DUPLICATE → the handler is skipped"
        );
        assert!(
            !ledger.mark_handled(&c, &id),
            "and again - still a duplicate"
        );
        assert_eq!(
            ledger.len(),
            1,
            "exactly ONE (consumer, event_id) pair recorded"
        );
        assert!(ledger.is_handled(&c, &id), "the pair is durably handled");
    }

    #[test]
    fn two_consumers_record_the_same_event_independently() {
        let ledger = DedupLedger::new();
        let a = consumer("indexer");
        let b = consumer("notifier");
        let id = EventId("01J-1".into());

        assert!(ledger.mark_handled(&a, &id), "fresh for consumer A");
        assert!(
            ledger.mark_handled(&b, &id),
            "ALSO fresh for consumer B (different PK)"
        );
        assert!(
            !ledger.mark_handled(&a, &id),
            "redelivery to A is a duplicate"
        );
        assert!(
            !ledger.mark_handled(&b, &id),
            "redelivery to B is a duplicate"
        );
        assert_eq!(ledger.len(), 2, "two distinct (consumer, event_id) pairs");
        assert!(ledger.is_handled(&a, &id) && ledger.is_handled(&b, &id));
        assert!(
            !ledger.is_handled(&consumer("other"), &id),
            "a third consumer has not handled it"
        );
    }

    #[test]
    fn is_empty_is_handled_and_revert_track_state() {
        let ledger = DedupLedger::new();
        let c = consumer("indexer");
        let id = EventId("01J-1".into());
        assert!(ledger.is_empty(), "a fresh ledger is empty");
        assert!(!ledger.is_handled(&c, &id), "nothing handled yet");

        assert!(ledger.mark_handled(&c, &id), "first mark is fresh");
        assert!(!ledger.is_empty(), "no longer empty after a mark");
        assert!(ledger.is_handled(&c, &id), "the exact pair is handled");

        ledger.revert(&c, &id);
        assert!(
            !ledger.is_handled(&c, &id),
            "after revert the pair is unhandled (a retry re-runs)"
        );
        assert!(ledger.is_empty(), "revert removed the only pair");
    }

    #[test]
    fn forget_returns_whether_a_mark_was_removed_and_re_runs_the_handler() {
        let ledger = DedupLedger::new();
        let c = consumer("indexer");
        let id = EventId("01J-snapshot".into());

        assert!(
            !ledger.forget(&c, &id),
            "forgetting a mark that was never present returns false"
        );

        assert!(ledger.mark_handled(&c, &id), "first delivery is fresh");
        assert!(ledger.is_handled(&c, &id), "the pair is handled");
        assert!(
            ledger.forget(&c, &id),
            "forgetting a present mark returns true"
        );
        assert!(
            !ledger.is_handled(&c, &id),
            "after forget the pair is unhandled - the cold rebuild re-applies the snapshot"
        );
        assert!(
            !ledger.forget(&c, &id),
            "a second forget of the now-absent mark returns false"
        );
        assert!(
            ledger.mark_handled(&c, &id),
            "redelivery after forget is fresh → the handler re-runs (reindex-after-wipe)"
        );
    }

    #[test]
    fn migration_is_the_frozen_2_5_shape() {
        assert!(CONSUMER_DEDUP_MIGRATION.contains("CREATE TABLE IF NOT EXISTS consumer_dedup"));
        assert!(CONSUMER_DEDUP_MIGRATION.contains("PRIMARY KEY (consumer, event_id)"));
        for col in ["consumer", "event_id", "recorded_at"] {
            assert!(
                CONSUMER_DEDUP_MIGRATION.contains(col),
                "missing column {col}"
            );
        }
        assert!(
            !CONSUMER_DEDUP_MIGRATION.contains("DROP TABLE"),
            "forward-only: no destructive down"
        );
    }
}
