use crate::block_tree::BlockId;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockState {
    pub inline: String,
    pub props: String,
    pub version: u64,
}

impl BlockState {
    pub fn new(inline: impl Into<String>, props: impl Into<String>) -> BlockState {
        BlockState {
            inline: inline.into(),
            props: props.into(),
            version: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CasOutcome {
    Committed(BlockState),
    Conflict {
        current: BlockState,
    },
}

impl CasOutcome {
    pub fn committed(&self) -> bool {
        matches!(self, CasOutcome::Committed(_))
    }

    pub fn is_conflict(&self) -> bool {
        matches!(self, CasOutcome::Conflict { .. })
    }

    pub fn state(&self) -> &BlockState {
        match self {
            CasOutcome::Committed(s) => s,
            CasOutcome::Conflict { current } => current,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CasError {
    NoSuchBlock(BlockId),
    DuplicateBlock(BlockId),
}

impl std::fmt::Display for CasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CasError::NoSuchBlock(b) => write!(f, "no content row for block {}", b.as_str()),
            CasError::DuplicateBlock(b) => {
                write!(
                    f,
                    "content row for block {} already exists (insert once)",
                    b.as_str()
                )
            }
        }
    }
}

impl std::error::Error for CasError {}

#[derive(Debug, Default, Clone)]
pub struct CasStore {
    blocks: BTreeMap<BlockId, BlockState>,
    meter: ConflictMeter,
}

impl CasStore {
    pub fn new() -> CasStore {
        CasStore::default()
    }

    pub fn insert_block(
        &mut self,
        block_id: BlockId,
        inline: impl Into<String>,
        props: impl Into<String>,
    ) -> Result<&BlockState, CasError> {
        if self.blocks.contains_key(&block_id) {
            return Err(CasError::DuplicateBlock(block_id));
        }
        self.blocks
            .insert(block_id.clone(), BlockState::new(inline, props));
        Ok(self.blocks.get(&block_id).expect("just inserted"))
    }

    pub fn get(&self, block_id: &BlockId) -> Option<&BlockState> {
        self.blocks.get(block_id)
    }

    pub fn edit_block(
        &mut self,
        block_id: &BlockId,
        expected_version: u64,
        new_inline: impl Into<String>,
        new_props: impl Into<String>,
    ) -> Result<CasOutcome, CasError> {
        let current = self
            .blocks
            .get_mut(block_id)
            .ok_or_else(|| CasError::NoSuchBlock(block_id.clone()))?;
        if current.version == expected_version {
            current.inline = new_inline.into();
            current.props = new_props.into();
            current.version += 1;
            let committed = current.clone();
            self.meter.record_commit();
            Ok(CasOutcome::Committed(committed))
        } else {
            let conflict = current.clone();
            self.meter.record_conflict();
            Ok(CasOutcome::Conflict { current: conflict })
        }
    }

    pub fn snapshot_block(&self, block_id: &BlockId) -> Result<BlockState, CasError> {
        self.blocks
            .get(block_id)
            .cloned()
            .ok_or_else(|| CasError::NoSuchBlock(block_id.clone()))
    }

    pub fn restore_block(
        &mut self,
        block_id: &BlockId,
        expected_version: u64,
        snapshot: &BlockState,
    ) -> Result<CasOutcome, CasError> {
        self.edit_block(
            block_id,
            expected_version,
            snapshot.inline.clone(),
            snapshot.props.clone(),
        )
    }

    pub fn meter(&self) -> &ConflictMeter {
        &self.meter
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

pub fn cas_update_sql() -> &'static str {
    "UPDATE block \
        SET inline = $4, props = $5, version = version + 1, edited_by = $6, edited_at = now() \
      WHERE tenant = $1 AND block_id = $2 AND version = $3"
}

#[derive(Debug, Default, Clone)]
pub struct SoftLockTable {
    locks: HashMap<BlockId, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoftLock {
    Acquired,
    Held {
        by: String,
    },
}

impl SoftLockTable {
    pub fn new() -> SoftLockTable {
        SoftLockTable::default()
    }

    pub fn acquire(&mut self, block_id: &BlockId, client_id: &str) -> SoftLock {
        match self.locks.get(block_id) {
            Some(holder) if holder != client_id => SoftLock::Held { by: holder.clone() },
            _ => {
                self.locks.insert(block_id.clone(), client_id.to_string());
                SoftLock::Acquired
            }
        }
    }

    pub fn release(&mut self, block_id: &BlockId, client_id: &str) {
        if self
            .locks
            .get(block_id)
            .map(|h| h == client_id)
            .unwrap_or(false)
        {
            self.locks.remove(block_id);
        }
    }

    pub fn holder(&self, block_id: &BlockId) -> Option<&String> {
        self.locks.get(block_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedEdit {
    pub block_id: BlockId,
    pub expected_version: u64,
    pub new_inline: String,
    pub new_props: String,
}

#[derive(Debug, Default, Clone)]
pub struct OfflineQueue {
    edits: Vec<QueuedEdit>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconcileResult {
    pub edit: QueuedEdit,
    pub outcome: CasOutcome,
}

impl OfflineQueue {
    pub fn new() -> OfflineQueue {
        OfflineQueue::default()
    }

    pub fn queue(
        &mut self,
        block_id: BlockId,
        expected_version: u64,
        new_inline: impl Into<String>,
        new_props: impl Into<String>,
    ) {
        self.edits.push(QueuedEdit {
            block_id,
            expected_version,
            new_inline: new_inline.into(),
            new_props: new_props.into(),
        });
    }

    pub fn len(&self) -> usize {
        self.edits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    pub fn reconcile(&mut self, store: &mut CasStore) -> Vec<Result<ReconcileResult, CasError>> {
        let drained = std::mem::take(&mut self.edits);
        drained
            .into_iter()
            .map(|edit| {
                store
                    .edit_block(
                        &edit.block_id,
                        edit.expected_version,
                        edit.new_inline.clone(),
                        edit.new_props.clone(),
                    )
                    .map(|outcome| ReconcileResult { edit, outcome })
            })
            .collect()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConflictMeter {
    committed: u64,
    conflicted: u64,
}

pub const CAS_CONFLICT_RATE_METRIC: &str = "knowledge.cas_conflict_rate";

impl ConflictMeter {
    pub fn new() -> ConflictMeter {
        ConflictMeter::default()
    }

    pub fn record_commit(&mut self) {
        self.committed += 1;
    }

    pub fn record_conflict(&mut self) {
        self.conflicted += 1;
    }

    pub fn committed(&self) -> u64 {
        self.committed
    }

    pub fn conflicted(&self) -> u64 {
        self.conflicted
    }

    pub fn attempts(&self) -> u64 {
        self.committed + self.conflicted
    }

    pub fn conflict_rate(&self) -> f64 {
        let attempts = self.attempts();
        if attempts == 0 {
            0.0
        } else {
            self.conflicted as f64 / attempts as f64
        }
    }

    pub fn telemetry_sample(&self) -> (&'static str, f64) {
        (CAS_CONFLICT_RATE_METRIC, self.conflict_rate())
    }
}

#[derive(Debug, Default, Clone)]
pub struct SimultaneousPresence {
    present: HashMap<BlockId, HashSet<String>>,
}

impl SimultaneousPresence {
    pub fn new() -> SimultaneousPresence {
        SimultaneousPresence::default()
    }

    pub fn enter(&mut self, block_id: &BlockId, client_id: &str) {
        self.present
            .entry(block_id.clone())
            .or_default()
            .insert(client_id.to_string());
    }

    pub fn leave(&mut self, block_id: &BlockId, client_id: &str) {
        if let Some(set) = self.present.get_mut(block_id) {
            set.remove(client_id);
            if set.is_empty() {
                self.present.remove(block_id);
            }
        }
    }

    pub fn is_contended(&self, block_id: &BlockId) -> bool {
        self.present
            .get(block_id)
            .map(|s| s.len() >= 2)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bid(s: &str) -> BlockId {
        BlockId(s.to_string())
    }

    fn store_with_block() -> (CasStore, BlockId) {
        let mut s = CasStore::new();
        let b = bid("b1");
        s.insert_block(b.clone(), "hello", "{}").unwrap();
        (s, b)
    }

    #[test]
    fn cas_winner_commits_and_bumps_version() {
        let (mut s, b) = store_with_block();
        assert_eq!(
            s.get(&b).unwrap().version,
            1,
            "a fresh block is at version 1"
        );
        let out = s.edit_block(&b, 1, "hello world", "{}").unwrap();
        assert!(out.committed(), "the CAS at the current version commits");
        assert_eq!(out.state().version, 2, "the version bumped by exactly 1");
        assert_eq!(
            out.state().inline,
            "hello world",
            "the new content was written"
        );
        assert_eq!(
            s.get(&b).unwrap().version,
            2,
            "the store reflects the committed write"
        );
    }

    #[test]
    fn cas_loser_gets_conflict_current_zero_silent_overwrite() {
        let (mut s, b) = store_with_block();
        let a = s.edit_block(&b, 1, "A's edit", "{}").unwrap();
        assert!(a.committed());
        let bout = s.edit_block(&b, 1, "B's edit", "{}").unwrap();
        assert!(
            bout.is_conflict(),
            "the stale writer LOSES the CAS (rows_affected == 0)"
        );
        match &bout {
            CasOutcome::Conflict { current } => {
                assert_eq!(
                    current.version, 2,
                    "the loser is handed the CURRENT server version"
                );
                assert_eq!(
                    current.inline, "A's edit",
                    "the loser sees the WINNER's content to reconcile"
                );
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        assert_eq!(
            s.get(&b).unwrap().inline,
            "A's edit",
            "B's edit was REJECTED, not silently applied - 0 silent overwrites"
        );
        assert_eq!(
            s.get(&b).unwrap().version,
            2,
            "the version did NOT advance on the conflict"
        );
    }

    #[test]
    fn loser_reconciles_at_current_version_and_commits() {
        let (mut s, b) = store_with_block();
        s.edit_block(&b, 1, "A's edit", "{}").unwrap();
        let conflict = s.edit_block(&b, 1, "B's edit", "{}").unwrap();
        let current = match conflict {
            CasOutcome::Conflict { current } => current,
            other => panic!("expected conflict, got {other:?}"),
        };
        let reconciled = s
            .edit_block(&b, current.version, "A's edit + B's reconciled edit", "{}")
            .unwrap();
        assert!(
            reconciled.committed(),
            "the reconciled edit at the current version now commits"
        );
        assert_eq!(
            reconciled.state().version,
            3,
            "the reconciled commit bumps to v3"
        );
    }

    #[test]
    fn different_blocks_edit_in_parallel_no_false_conflict() {
        let mut s = CasStore::new();
        let (b1, b2) = (bid("b1"), bid("b2"));
        s.insert_block(b1.clone(), "one", "{}").unwrap();
        s.insert_block(b2.clone(), "two", "{}").unwrap();
        let x = s.edit_block(&b1, 1, "one edited", "{}").unwrap();
        let y = s.edit_block(&b2, 1, "two edited", "{}").unwrap();
        assert!(x.committed(), "b1's edit commits");
        assert!(
            y.committed(),
            "b2's edit commits - NO false conflict with b1"
        );
        assert_eq!(s.get(&b1).unwrap().inline, "one edited");
        assert_eq!(s.get(&b2).unwrap().inline, "two edited");
        assert_eq!(
            s.meter().conflicted(),
            0,
            "different-block edits produce 0 false conflicts"
        );
        assert_eq!(s.meter().committed(), 2, "both committed");
    }

    #[test]
    fn edit_before_insert_errors_loudly() {
        let mut s = CasStore::new();
        assert_eq!(
            s.edit_block(&bid("ghost"), 1, "x", "{}").unwrap_err(),
            CasError::NoSuchBlock(bid("ghost"))
        );
    }

    #[test]
    fn duplicate_content_insert_refused() {
        let (mut s, b) = store_with_block();
        assert_eq!(
            s.insert_block(b.clone(), "x", "{}").unwrap_err(),
            CasError::DuplicateBlock(b)
        );
    }

    #[test]
    fn soft_lock_is_advisory_not_mandatory() {
        let mut locks = SoftLockTable::new();
        let b = bid("b1");
        assert_eq!(
            locks.acquire(&b, "client-A"),
            SoftLock::Acquired,
            "A takes the advisory lock"
        );
        assert_eq!(
            locks.holder(&b),
            Some(&"client-A".to_string()),
            "A is the advisory holder"
        );
        assert_eq!(
            locks.acquire(&b, "client-B"),
            SoftLock::Held {
                by: "client-A".into()
            },
            "B sees A editing (the UX courtesy) but is not blocked"
        );
        assert_eq!(locks.acquire(&b, "client-A"), SoftLock::Acquired);
        locks.release(&b, "client-A");
        assert_eq!(
            locks.acquire(&b, "client-B"),
            SoftLock::Acquired,
            "after release B acquires"
        );
        locks.release(&b, "client-A");
        assert_eq!(
            locks.holder(&b),
            Some(&"client-B".to_string()),
            "A cannot release B's lock"
        );
    }

    #[test]
    fn soft_lock_does_not_gate_the_cas_write() {
        let (mut s, b) = store_with_block();
        s.edit_block(&b, 1, "A", "{}").unwrap();
        let conflict = s.edit_block(&b, 1, "B", "{}").unwrap();
        assert!(
            conflict.is_conflict(),
            "the CAS guard protects regardless of any soft-lock"
        );
    }

    #[test]
    fn snapshot_restore_through_the_cas_guard() {
        let (mut s, b) = store_with_block();
        let snap = s.snapshot_block(&b).unwrap();
        assert_eq!(snap.inline, "hello");
        s.edit_block(&b, 1, "edited once", "{}").unwrap();
        s.edit_block(&b, 2, "edited twice", "{}").unwrap();
        let restored = s.restore_block(&b, 3, &snap).unwrap();
        assert!(
            restored.committed(),
            "a restore at the current version commits"
        );
        assert_eq!(
            restored.state().inline,
            "hello",
            "the content reverted to the snapshot"
        );
        assert_eq!(
            restored.state().version,
            4,
            "the restore is a new revision (v4), not a counter rewind"
        );

        s.edit_block(&b, 4, "live edit after restore", "{}")
            .unwrap();
        let stale_restore = s.restore_block(&b, 4, &snap).unwrap();
        assert!(
            stale_restore.is_conflict(),
            "a restore racing a live edit conflicts, never clobbers"
        );
        assert_eq!(
            s.get(&b).unwrap().inline,
            "live edit after restore",
            "the live edit survived the stale restore (0 silent overwrite)"
        );
    }

    #[test]
    fn offline_queued_edit_commits_when_base_holds() {
        let (mut s, b) = store_with_block();
        let mut q = OfflineQueue::new();
        q.queue(b.clone(), 1, "offline edit", "{}");
        assert_eq!(q.len(), 1);
        let results = q.reconcile(&mut s);
        assert_eq!(results.len(), 1);
        let r = results[0].as_ref().unwrap();
        assert!(
            r.outcome.committed(),
            "the offline edit committed (base version still held)"
        );
        assert_eq!(s.get(&b).unwrap().inline, "offline edit");
        assert!(q.is_empty(), "the queue drained on reconcile");
    }

    #[test]
    fn stale_offline_edit_conflicts_on_reconnect() {
        let (mut s, b) = store_with_block();
        let mut q = OfflineQueue::new();
        q.queue(b.clone(), 1, "offline edit", "{}");
        s.edit_block(&b, 1, "online edit while peer offline", "{}")
            .unwrap();
        let results = q.reconcile(&mut s);
        let r = results[0].as_ref().unwrap();
        assert!(
            r.outcome.is_conflict(),
            "the stale offline edit conflicts (reconcile, not overwrite)"
        );
        assert_eq!(
            s.get(&b).unwrap().inline,
            "online edit while peer offline",
            "the online edit survived - the offline edit did NOT silently overwrite it"
        );
    }

    #[test]
    fn offline_edit_to_missing_block_errors_loudly() {
        let mut s = CasStore::new();
        let mut q = OfflineQueue::new();
        q.queue(bid("gone"), 1, "x", "{}");
        let results = q.reconcile(&mut s);
        assert_eq!(
            results[0].as_ref().unwrap_err(),
            &CasError::NoSuchBlock(bid("gone"))
        );
    }

    #[test]
    fn conflict_rate_metric_is_emitted() {
        let (mut s, b) = store_with_block();
        assert_eq!(
            s.meter().conflict_rate(),
            0.0,
            "a fresh doc has 0 conflict rate (no divide-by-zero)"
        );
        s.edit_block(&b, 1, "a", "{}").unwrap();
        s.edit_block(&b, 1, "stale", "{}").unwrap();
        s.edit_block(&b, 2, "b", "{}").unwrap();
        assert_eq!(s.meter().committed(), 2);
        assert_eq!(s.meter().conflicted(), 1);
        assert_eq!(s.meter().attempts(), 3);
        assert!(
            (s.meter().conflict_rate() - (1.0 / 3.0)).abs() < 1e-9,
            "the conflict rate is 1/3"
        );
        let (name, rate) = s.meter().telemetry_sample();
        assert_eq!(
            name, "knowledge.cas_conflict_rate",
            "the canonical CRDT-promotion-trigger metric name"
        );
        assert!((rate - (1.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn simultaneous_presence_marks_a_block_contended() {
        let mut p = SimultaneousPresence::new();
        let b = bid("b1");
        p.enter(&b, "A");
        assert!(!p.is_contended(&b), "one author present is not contended");
        p.enter(&b, "B");
        assert!(
            p.is_contended(&b),
            "two simultaneous authors → contended (the CRDT-promotion signal)"
        );
        p.leave(&b, "A");
        assert!(
            !p.is_contended(&b),
            "back to one author → no longer contended"
        );
    }

    #[test]
    fn cas_sql_carries_the_optimistic_guard() {
        let sql = cas_update_sql();
        assert!(
            sql.contains("version = version + 1"),
            "the version is bumped: {sql}"
        );
        assert!(
            sql.contains("WHERE tenant = $1 AND block_id = $2 AND version = $3"),
            "the CAS guard: {sql}"
        );
        assert!(
            !sql.contains("WHERE TRUE"),
            "the write is a bounded single-row CAS, never unguarded"
        );
    }
}
