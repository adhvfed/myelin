use crate::relay::{BusTransport, DrainReport};
use crate::{
    derive_envelope, AggregateKey, EmitContext, EventDraft, EventEnvelope, EventId, OutboxError,
    OutboxTx, Result,
};
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

fn bounded_retained_rows(
    rows: Vec<OutboxRow>,
    maximum_rows: usize,
    maximum_envelope_bytes: usize,
) -> Result<Vec<OutboxRow>> {
    if rows.len() > maximum_rows {
        return Err(OutboxError(
            "retained outbox snapshot exceeds its row limit".into(),
        ));
    }
    let mut total_bytes = 0usize;
    for row in &rows {
        let envelope_bytes = serde_json::to_vec(&row.envelope)
            .map_err(|_| OutboxError("retained outbox envelope could not be measured".into()))?
            .len();
        total_bytes = total_bytes
            .checked_add(envelope_bytes)
            .ok_or_else(|| OutboxError("retained outbox envelope byte count overflowed".into()))?;
        if total_bytes > maximum_envelope_bytes {
            return Err(OutboxError(
                "retained outbox snapshot exceeds its envelope byte limit".into(),
            ));
        }
    }
    Ok(rows)
}

pub const OUTBOX_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS outbox (
    event_id     TEXT        NOT NULL,
    aggregate    TEXT        NOT NULL,
    seq          BIGINT      NOT NULL,
    subject      TEXT        NOT NULL,
    envelope     JSONB       NOT NULL,
    published_at TIMESTAMPTZ,
    attempts     INT         NOT NULL DEFAULT 0,
    CONSTRAINT outbox_event_id_unique UNIQUE (event_id),
    CONSTRAINT outbox_aggregate_seq_unique UNIQUE (aggregate, seq)
);
-- the relay claims unsent rows ordered (aggregate, seq) with FOR UPDATE SKIP LOCKED:
CREATE INDEX IF NOT EXISTS outbox_unsent_idx ON outbox (aggregate, seq) WHERE published_at IS NULL;";

pub const OUTBOX_QUARANTINE_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS outbox_quarantine (
    event_id         TEXT        PRIMARY KEY REFERENCES outbox(event_id) ON DELETE RESTRICT,
    aggregate        TEXT        NOT NULL,
    seq              BIGINT      NOT NULL,
    reason_code      TEXT        NOT NULL,
    reason_detail    TEXT        NOT NULL,
    quarantined_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    acknowledged_at TIMESTAMPTZ,
    CONSTRAINT outbox_quarantine_reason_code_bounded CHECK (
        reason_code ~ '^[a-z0-9_]{1,64}$'
    ),
    CONSTRAINT outbox_quarantine_reason_detail_bounded CHECK (
        octet_length(reason_detail) BETWEEN 1 AND 256
    )
);
CREATE INDEX IF NOT EXISTS outbox_quarantine_aggregate_seq_idx
    ON outbox_quarantine (aggregate, seq);";

pub const OUTBOX_PUBLISHER_GRANTS_MIGRATION: &str = "\
DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'myelin_outbox_publisher') THEN
    REVOKE ALL ON TABLE outbox FROM myelin_outbox_publisher;
    REVOKE ALL ON TABLE outbox_quarantine FROM myelin_outbox_publisher;
    GRANT SELECT ON TABLE outbox TO myelin_outbox_publisher;
    GRANT UPDATE (published_at) ON TABLE outbox TO myelin_outbox_publisher;
    GRANT SELECT, INSERT ON TABLE outbox_quarantine TO myelin_outbox_publisher;
  END IF;
END
$$;";

pub const OUTBOX_PUBLISHER_GRANT_SCOPE_MIGRATION: &str = "\
DO $$
DECLARE
  target record;
BEGIN
  IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'myelin_outbox_publisher') THEN
    FOR target IN
      SELECT namespace.nspname,
             relation.relname,
             string_agg(format('%I', attribute.attname), ',' ORDER BY attribute.attnum) AS columns
        FROM pg_catalog.pg_class relation
        JOIN pg_catalog.pg_namespace namespace ON namespace.oid = relation.relnamespace
        JOIN pg_catalog.pg_attribute attribute ON attribute.attrelid = relation.oid
       WHERE namespace.nspname <> 'public'
         AND namespace.nspname NOT IN ('pg_catalog', 'information_schema')
         AND namespace.nspname NOT LIKE 'pg_toast%'
         AND relation.relkind IN ('r', 'p')
         AND relation.relname IN ('outbox', 'outbox_quarantine')
         AND attribute.attnum > 0
         AND NOT attribute.attisdropped
       GROUP BY namespace.nspname, relation.relname
    LOOP
      EXECUTE format(
        'REVOKE ALL PRIVILEGES ON TABLE %I.%I FROM myelin_outbox_publisher',
        target.nspname,
        target.relname
      );
      EXECUTE format(
        'REVOKE ALL PRIVILEGES (%s) ON TABLE %I.%I FROM myelin_outbox_publisher',
        target.columns,
        target.nspname,
        target.relname
      );
    END LOOP;

    IF pg_catalog.to_regclass('public.outbox') IS NOT NULL THEN
      REVOKE ALL PRIVILEGES ON TABLE public.outbox FROM myelin_outbox_publisher;
      REVOKE ALL PRIVILEGES (
        event_id, aggregate, seq, subject, envelope, published_at, attempts
      ) ON TABLE public.outbox FROM myelin_outbox_publisher;
      GRANT SELECT ON TABLE public.outbox TO myelin_outbox_publisher;
      GRANT UPDATE (published_at) ON TABLE public.outbox TO myelin_outbox_publisher;
    END IF;

    IF pg_catalog.to_regclass('public.outbox_quarantine') IS NOT NULL THEN
      REVOKE ALL PRIVILEGES ON TABLE public.outbox_quarantine FROM myelin_outbox_publisher;
      REVOKE ALL PRIVILEGES (
        event_id, aggregate, seq, reason_code, reason_detail, quarantined_at, acknowledged_at
      ) ON TABLE public.outbox_quarantine FROM myelin_outbox_publisher;
      GRANT SELECT, INSERT ON TABLE public.outbox_quarantine TO myelin_outbox_publisher;
    END IF;
  END IF;
END
$$;";

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ulid(pub String);

impl From<Ulid> for EventId {
    fn from(u: Ulid) -> Self {
        EventId(u.0)
    }
}

pub trait IdMinter: Send + Sync {
    fn mint(&self) -> Ulid;
}

#[derive(Default)]
pub struct MonotonicMinter {
    next: AtomicU64,
}

impl MonotonicMinter {
    pub fn new() -> Self {
        Self::default()
    }
}

impl IdMinter for MonotonicMinter {
    fn mint(&self) -> Ulid {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        Ulid(format!("01J{n:020}"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxRow {
    pub event_id: EventId,
    pub aggregate: AggregateKey,
    pub seq: u64,
    pub subject: crate::ArtifactRef,
    pub envelope: EventEnvelope,
    pub published_at: Option<crate::Timestamp>,
    pub attempts: u32,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub(crate) struct Inner {
    pub(crate) rows: HashMap<EventId, OutboxRow>,
    pub(crate) order: Vec<EventId>,
    pub(crate) next_seq: HashMap<AggregateKey, u64>,
    pub(crate) dead_letters: Vec<OutboxRow>,
    pub(crate) claimed: std::collections::HashSet<EventId>,
}

#[derive(Clone)]
#[cfg_attr(any(test, feature = "test-support"), derive(Default))]
pub struct OutboxStore {
    backend: OutboxBackend,
}


pub trait DurableOutboxBacking: Send + Sync {
    fn commit_staged(&self, rows: Vec<OutboxRow>) -> Result<()>;

    fn commit_staged_absorb(&self, rows: Vec<OutboxRow>) -> Result<()> {
        self.commit_staged(rows)
    }

    fn outbox_depth(&self) -> usize;
    fn dead_letter_count(&self) -> usize;
    fn oldest_unsent_recorded_at(&self) -> Option<crate::Timestamp>;
    fn committed_count(&self) -> usize;
    fn row(&self, id: &EventId) -> Option<OutboxRow>;
    fn committed_rows(&self) -> Vec<OutboxRow>;
    fn try_committed_rows(&self) -> Result<Vec<OutboxRow>> {
        Ok(self.committed_rows())
    }
    fn try_retained_rows(&self) -> Result<Vec<OutboxRow>> {
        let mut rows = self.try_committed_rows()?;
        rows.extend(self.dead_letters());
        rows.sort_by(|left, right| {
            (&left.aggregate.0, left.seq).cmp(&(&right.aggregate.0, right.seq))
        });
        Ok(rows)
    }
    fn try_retained_rows_bounded(
        &self,
        maximum_rows: usize,
        maximum_envelope_bytes: usize,
    ) -> Result<Vec<OutboxRow>> {
        bounded_retained_rows(
            self.try_retained_rows()?,
            maximum_rows,
            maximum_envelope_bytes,
        )
    }
    fn dead_letters(&self) -> Vec<OutboxRow>;

    fn drain_once(&self, transport: &dyn BusTransport, batch: usize) -> Result<DrainReport>;
}

#[derive(Clone)]
enum OutboxBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<Inner>>),
    Durable(Arc<dyn DurableOutboxBacking>),
}

#[cfg(any(test, feature = "test-support"))]
impl Default for OutboxBackend {
    fn default() -> Self {
        OutboxBackend::Memory(Arc::new(Mutex::new(Inner::default())))
    }
}

impl OutboxStore {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn durable(backing: Arc<dyn DurableOutboxBacking>) -> Self {
        OutboxStore {
            backend: OutboxBackend::Durable(backing),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn mem(&self) -> Option<std::sync::MutexGuard<'_, Inner>> {
        match &self.backend {
            OutboxBackend::Memory(inner) => Some(inner.lock().unwrap_or_else(|e| e.into_inner())),
            OutboxBackend::Durable(_) => None,
        }
    }

    pub(crate) fn durable_backing(&self) -> Option<Arc<dyn DurableOutboxBacking>> {
        match &self.backend {
            OutboxBackend::Durable(b) => Some(Arc::clone(b)),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => None,
        }
    }

    pub fn begin(&self, minter: Arc<dyn IdMinter>, ctx_base: EmitContextBase) -> OutboxTransaction {
        OutboxTransaction {
            store: Some(self.clone()),
            minter,
            ctx_base,
            staged_rows: Vec::new(),
            state_committed: Arc::new(Mutex::new(None)),
        }
    }

    pub fn outbox_depth(&self) -> usize {
        match &self.backend {
            OutboxBackend::Durable(b) => b.outbox_depth(),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => {
                let inner = self.mem().expect("memory backend");
                inner
                    .order
                    .iter()
                    .filter(|id| {
                        inner
                            .rows
                            .get(*id)
                            .is_some_and(|r| r.published_at.is_none())
                    })
                    .count()
            }
        }
    }

    pub fn dead_letter_count(&self) -> usize {
        match &self.backend {
            OutboxBackend::Durable(b) => b.dead_letter_count(),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => self.mem().expect("memory backend").dead_letters.len(),
        }
    }

    pub fn oldest_unsent_recorded_at(&self) -> Option<crate::Timestamp> {
        match &self.backend {
            OutboxBackend::Durable(b) => b.oldest_unsent_recorded_at(),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => {
                let inner = self.mem().expect("memory backend");
                inner.order.iter().find_map(|id| {
                    inner
                        .rows
                        .get(id)
                        .filter(|r| r.published_at.is_none())
                        .map(|r| r.envelope.recorded_at.clone())
                })
            }
        }
    }

    pub fn committed_count(&self) -> usize {
        match &self.backend {
            OutboxBackend::Durable(b) => b.committed_count(),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => self.mem().expect("memory backend").order.len(),
        }
    }

    pub fn row(&self, id: &EventId) -> Option<OutboxRow> {
        match &self.backend {
            OutboxBackend::Durable(b) => b.row(id),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => self.mem().expect("memory backend").rows.get(id).cloned(),
        }
    }

    pub fn dead_letters(&self) -> Vec<OutboxRow> {
        match &self.backend {
            OutboxBackend::Durable(b) => b.dead_letters(),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => self.mem().expect("memory backend").dead_letters.clone(),
        }
    }

    pub fn committed_rows(&self) -> Vec<OutboxRow> {
        match &self.backend {
            OutboxBackend::Durable(b) => b.committed_rows(),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => {
                let inner = self.mem().expect("memory backend");
                inner
                    .order
                    .iter()
                    .filter_map(|id| inner.rows.get(id).cloned())
                    .collect()
            }
        }
    }

    pub fn try_committed_rows(&self) -> Result<Vec<OutboxRow>> {
        match &self.backend {
            OutboxBackend::Durable(b) => b.try_committed_rows(),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => Ok(self.committed_rows()),
        }
    }

    pub fn try_retained_rows(&self) -> Result<Vec<OutboxRow>> {
        match &self.backend {
            OutboxBackend::Durable(b) => b.try_retained_rows(),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => {
                let mut rows = self.committed_rows();
                rows.extend(self.dead_letters());
                rows.sort_by(|left, right| {
                    (&left.aggregate.0, left.seq).cmp(&(&right.aggregate.0, right.seq))
                });
                Ok(rows)
            }
        }
    }

    pub fn try_retained_rows_bounded(
        &self,
        maximum_rows: usize,
        maximum_envelope_bytes: usize,
    ) -> Result<Vec<OutboxRow>> {
        match &self.backend {
            OutboxBackend::Durable(b) => {
                b.try_retained_rows_bounded(maximum_rows, maximum_envelope_bytes)
            }
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => bounded_retained_rows(
                self.try_retained_rows()?,
                maximum_rows,
                maximum_envelope_bytes,
            ),
        }
    }

    #[doc(hidden)]
    #[cfg(any(test, feature = "test-support"))]
    pub fn restore_committed_row_for_test(&self, row: OutboxRow) {
        let Some(mut inner) = self.mem() else { return };
        if inner.rows.contains_key(&row.event_id) {
            return;
        }
        let next = inner.next_seq.entry(row.aggregate.clone()).or_insert(0);
        *next = (*next).max(row.seq + 1);
        inner.order.push(row.event_id.clone());
        inner.rows.insert(row.event_id.clone(), row);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmitContextBase {
    pub tenant: crate::TenantId,
    pub region: crate::Region,
    pub actor: crate::Actor,
    pub schema_ver: u32,
    pub occurred_at: crate::Timestamp,
    pub recorded_at: crate::Timestamp,
    pub caused_by: Option<crate::CausedBy>,
}

pub struct OutboxTransaction {
    store: Option<OutboxStore>,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    staged_rows: Vec<OutboxRow>,
    state_committed: Arc<Mutex<Option<String>>>,
}

impl OutboxTransaction {
    pub fn detached(minter: Arc<dyn IdMinter>, ctx_base: EmitContextBase) -> Self {
        Self {
            store: None,
            minter,
            ctx_base,
            staged_rows: Vec::new(),
            state_committed: Arc::new(Mutex::new(None)),
        }
    }

    pub fn into_staged_rows(mut self) -> Result<Vec<OutboxRow>> {
        if self.store.is_some() {
            return Err(OutboxError(
                "only a detached outbox transaction may export staged rows".into(),
            ));
        }
        let mut event_ids = HashSet::with_capacity(self.staged_rows.len());
        for row in &self.staged_rows {
            if !event_ids.insert(row.event_id.clone()) {
                return Err(OutboxError(
                    "detached outbox batch contains a duplicate event_id".into(),
                ));
            }
            if row.seq != 0 || row.published_at.is_some() || row.attempts != 0 {
                return Err(OutboxError(
                    "detached outbox rows must retain the unallocated, unpublished staging shape"
                        .into(),
                ));
            }
            if row.event_id != row.envelope.event_id
                || row.aggregate != row.envelope.aggregate
                || row.subject != row.envelope.subject
            {
                return Err(OutboxError(
                    "detached outbox row routing must exactly match its canonical envelope".into(),
                ));
            }
        }
        Ok(self.staged_rows.drain(..).collect())
    }

    pub fn stage_state_change(&mut self, change: impl Into<String>) {
        *self
            .state_committed
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(change.into());
    }

    pub fn commit_absorb(mut self) -> Result<()> {
        let store = self.store.take().ok_or_else(|| {
            OutboxError("detached outbox rows require a caller-owned atomic commit".into())
        })?;
        if let Some(backing) = store.durable_backing() {
            let rows: Vec<OutboxRow> = self.staged_rows.drain(..).collect();
            return backing.commit_staged_absorb(rows);
        }
        #[cfg(any(test, feature = "test-support"))]
        {
            let mut inner = store.mem().expect("memory backend");
            for row in &self.staged_rows {
                if let Some(existing) = inner.rows.get(&row.event_id) {
                    if existing.envelope != row.envelope {
                        return Err(OutboxError(format!(
                            "outbox event_id {:?} already present with a DIFFERENT payload - genuine collision",
                            row.event_id
                        )));
                    }
                }
            }
            for mut row in self.staged_rows.drain(..) {
                if inner.rows.contains_key(&row.event_id) {
                    continue;
                }
                let slot = inner.next_seq.entry(row.aggregate.clone()).or_insert(0);
                row.seq = *slot;
                *slot += 1;
                inner.order.push(row.event_id.clone());
                inner.rows.insert(row.event_id.clone(), row);
            }
            Ok(())
        }
        #[cfg(not(any(test, feature = "test-support")))]
        unreachable!(
            "a production OutboxStore is Durable-only (the Memory arm is test-support-gated)"
        )
    }

    pub fn commit(mut self) -> Result<()> {
        let store = self.store.take().ok_or_else(|| {
            OutboxError("detached outbox rows require a caller-owned atomic commit".into())
        })?;
        if let Some(backing) = store.durable_backing() {
            let rows: Vec<OutboxRow> = self.staged_rows.drain(..).collect();
            return backing.commit_staged(rows);
        }
        #[cfg(any(test, feature = "test-support"))]
        {
            let mut inner = store.mem().expect("memory backend");
            for row in &self.staged_rows {
                if inner.rows.contains_key(&row.event_id) {
                    return Err(OutboxError(format!(
                        "outbox UNIQUE(event_id) violation on {:?} - duplicate emit",
                        row.event_id
                    )));
                }
            }
            for mut row in self.staged_rows.drain(..) {
                let slot = inner.next_seq.entry(row.aggregate.clone()).or_insert(0);
                row.seq = *slot;
                *slot += 1;
                inner.order.push(row.event_id.clone());
                inner.rows.insert(row.event_id.clone(), row);
            }
            Ok(())
        }
        #[cfg(not(any(test, feature = "test-support")))]
        unreachable!(
            "a production OutboxStore is Durable-only (the Memory arm is test-support-gated)"
        )
    }

    pub fn staged_state(&self) -> Option<String> {
        self.state_committed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn staged_len(&self) -> usize {
        self.staged_rows.len()
    }

    pub fn emit_with_id(
        &mut self,
        id: EventId,
        draft: EventDraft,
        cause: Option<&EventEnvelope>,
    ) -> Result<EventId> {
        let aggregate = draft.aggregate.clone();
        let subject = draft.subject.clone();
        let ctx = EmitContext {
            event_id: id.clone(),
            tenant: self.ctx_base.tenant.clone(),
            region: self.ctx_base.region.clone(),
            actor: self.ctx_base.actor.clone(),
            schema_ver: self.ctx_base.schema_ver,
            occurred_at: self.ctx_base.occurred_at.clone(),
            recorded_at: self.ctx_base.recorded_at.clone(),
            caused_by: self.ctx_base.caused_by.clone(),
        };
        let envelope = derive_envelope(draft, ctx, cause);
        self.staged_rows.push(OutboxRow {
            event_id: id.clone(),
            aggregate,
            seq: 0,
            subject,
            envelope,
            published_at: None,
            attempts: 0,
        });
        Ok(id)
    }
}

impl OutboxTx for OutboxTransaction {
    fn emit(&mut self, draft: EventDraft, cause: Option<&EventEnvelope>) -> Result<EventId> {
        let id: EventId = self.minter.mint().into();
        let aggregate = draft.aggregate.clone();
        let subject = draft.subject.clone();
        let ctx = EmitContext {
            event_id: id.clone(),
            tenant: self.ctx_base.tenant.clone(),
            region: self.ctx_base.region.clone(),
            actor: self.ctx_base.actor.clone(),
            schema_ver: self.ctx_base.schema_ver,
            occurred_at: self.ctx_base.occurred_at.clone(),
            recorded_at: self.ctx_base.recorded_at.clone(),
            caused_by: self.ctx_base.caused_by.clone(),
        };
        let envelope = derive_envelope(draft, ctx, cause);
        self.staged_rows.push(OutboxRow {
            event_id: id.clone(),
            aggregate,
            seq: 0,
            subject,
            envelope,
            published_at: None,
            attempts: 0,
        });
        Ok(id)
    }
}

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub struct UlidMinter {
    last: Mutex<u128>,
    seed: u64,
    bump: AtomicU64,
}

impl Default for UlidMinter {
    fn default() -> Self {
        UlidMinter::new()
    }
}

impl UlidMinter {
    pub fn new() -> UlidMinter {
        use std::hash::{BuildHasher, Hasher};
        let os_entropy = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        UlidMinter {
            last: Mutex::new(0),
            seed: os_entropy ^ (u64::from(std::process::id()).rotate_left(32)) ^ nanos,
            bump: AtomicU64::new(0),
        }
    }

    fn now_ms() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    }

    fn rand80(&self) -> u128 {
        fn splitmix64(mut z: u64) -> u64 {
            z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        let n = self.bump.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let hi = splitmix64(self.seed ^ n ^ nanos);
        let lo = splitmix64(hi ^ self.seed.rotate_left(17));
        (u128::from(hi) << 64 | u128::from(lo)) & ((1u128 << 80) - 1)
    }

    fn render(value: u128) -> String {
        let mut buf = [0u8; 26];
        let mut v = value;
        for slot in buf.iter_mut().rev() {
            *slot = CROCKFORD[(v & 0x1f) as usize];
            v >>= 5;
        }
        String::from_utf8(buf.to_vec()).expect("crockford bytes are ASCII")
    }
}

impl IdMinter for UlidMinter {
    fn mint(&self) -> Ulid {
        let candidate = (Self::now_ms() << 80) | self.rand80();
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let value = if candidate > *last {
            candidate
        } else {
            last.wrapping_add(1)
        };
        *last = value;
        Ulid(Self::render(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Actor, ArtifactRef, CausedBy, DataRole, EventType, Region, TenantId, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(principal()),
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

    fn store_and_minter() -> (OutboxStore, Arc<dyn IdMinter>) {
        (
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
        )
    }

    #[test]
    fn migration_is_the_frozen_2_3_shape() {
        assert!(OUTBOX_MIGRATION.contains("CREATE TABLE IF NOT EXISTS outbox"));
        assert!(OUTBOX_MIGRATION.contains("UNIQUE (event_id)"));
        assert!(OUTBOX_MIGRATION.contains("UNIQUE (aggregate, seq)"));
        for col in [
            "event_id",
            "aggregate",
            "seq",
            "subject",
            "envelope",
            "published_at",
        ] {
            assert!(
                OUTBOX_MIGRATION.contains(col),
                "migration is missing column {col}"
            );
        }
        assert!(!OUTBOX_MIGRATION.contains("DROP TABLE"));
    }

    #[test]
    fn quarantine_migration_keeps_payload_in_the_original_outbox_only() {
        assert!(OUTBOX_QUARANTINE_MIGRATION.contains("PRIMARY KEY REFERENCES outbox(event_id)"));
        for col in [
            "aggregate",
            "seq",
            "reason_code",
            "reason_detail",
            "acknowledged_at",
        ] {
            assert!(OUTBOX_QUARANTINE_MIGRATION.contains(col));
        }
        assert!(!OUTBOX_QUARANTINE_MIGRATION.contains("envelope"));
        assert!(!OUTBOX_QUARANTINE_MIGRATION.contains("payload"));
        assert!(!OUTBOX_QUARANTINE_MIGRATION.contains("subject"));
        assert!(!OUTBOX_QUARANTINE_MIGRATION.contains("DROP TABLE"));
    }

    #[test]
    fn publisher_grants_are_column_scoped_and_forbid_outbox_mutation() {
        assert!(OUTBOX_PUBLISHER_GRANTS_MIGRATION.contains("GRANT SELECT ON TABLE outbox"));
        assert!(OUTBOX_PUBLISHER_GRANTS_MIGRATION.contains("UPDATE (published_at)"));
        assert!(
            OUTBOX_PUBLISHER_GRANTS_MIGRATION.contains("SELECT, INSERT ON TABLE outbox_quarantine")
        );
        assert!(!OUTBOX_PUBLISHER_GRANTS_MIGRATION.contains("GRANT INSERT ON TABLE outbox "));
        assert!(!OUTBOX_PUBLISHER_GRANTS_MIGRATION.contains("GRANT DELETE"));
        assert!(!OUTBOX_PUBLISHER_GRANTS_MIGRATION.contains("UPDATE (attempts)"));
    }

    #[test]
    fn commit_makes_event_and_state_durable_together() {
        let (store, minter) = store_and_minter();
        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("issue PROJ-1 created");
        let id = tx
            .emit(draft("issues.issue.created", "issue:PROJ-1"), None)
            .unwrap();
        assert_eq!(tx.staged_len(), 1, "one event buffered");
        let id2 = tx
            .emit(draft("issues.issue.updated", "issue:PROJ-1"), None)
            .unwrap();
        assert_eq!(
            store.outbox_depth(),
            0,
            "an open transaction has written nothing"
        );
        assert_eq!(tx.staged_len(), 2, "two events buffered (not a constant)");
        assert_eq!(tx.staged_state().as_deref(), Some("issue PROJ-1 created"));

        tx.commit().unwrap();
        assert_eq!(store.outbox_depth(), 2);
        assert_eq!(store.committed_count(), 2);
        let row = store.row(&id).expect("committed row is present");
        assert_eq!(row.seq, 0, "first event for the aggregate is seq 0");
        assert!(
            row.published_at.is_none(),
            "a freshly committed row is unsent"
        );
        assert_eq!(store.row(&id2).unwrap().seq, 1, "second event is seq 1");
    }

    #[test]
    fn dropped_transaction_emits_nothing_emit_iff_committed() {
        let (store, minter) = store_and_minter();
        {
            let mut tx = store.begin(minter, ctx_base());
            tx.stage_state_change("issue PROJ-9 created");
            tx.emit(draft("issues.issue.created", "issue:PROJ-9"), None)
                .unwrap();
            assert_eq!(tx.staged_len(), 1, "buffered, not committed");
        }
        assert_eq!(
            store.outbox_depth(),
            0,
            "an aborted transaction writes no event"
        );
        assert_eq!(store.committed_count(), 0, "no ghost row from an abort");
        assert_eq!(store.dead_letter_count(), 0);
    }

    #[test]
    fn detached_transaction_exports_only_canonical_unallocated_rows() {
        let (_, minter) = store_and_minter();
        let mut tx = OutboxTransaction::detached(minter, ctx_base());
        let id = tx
            .emit(draft("issues.issue.created", "issue:DETACHED"), None)
            .unwrap();

        let rows = tx.into_staged_rows().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event_id, id);
        assert_eq!(rows[0].event_id, rows[0].envelope.event_id);
        assert_eq!(
            rows[0].seq, 0,
            "the durable co-commit owns sequence allocation"
        );
        assert!(rows[0].published_at.is_none());
        assert_eq!(rows[0].attempts, 0);
    }

    #[test]
    fn detached_transaction_rejects_commit_and_preallocated_sequence() {
        let (_, minter) = store_and_minter();
        let mut commit_tx = OutboxTransaction::detached(Arc::clone(&minter), ctx_base());
        commit_tx
            .emit(draft("issues.issue.created", "issue:DETACHED"), None)
            .unwrap();
        assert!(
            commit_tx.commit().is_err(),
            "there is no second publish path"
        );

        let mut staged_tx = OutboxTransaction::detached(minter, ctx_base());
        staged_tx
            .emit(draft("issues.issue.created", "issue:DETACHED"), None)
            .unwrap();
        staged_tx.staged_rows[0].seq = 7;
        assert!(staged_tx.into_staged_rows().is_err());
    }

    #[test]
    fn detached_transaction_rejects_duplicate_event_ids_within_one_drive() {
        struct ConstantMinter;
        impl IdMinter for ConstantMinter {
            fn mint(&self) -> Ulid {
                Ulid("01DETACHEDDUPLICATE0000000".into())
            }
        }

        let mut tx = OutboxTransaction::detached(Arc::new(ConstantMinter), ctx_base());
        tx.emit(draft("issues.issue.created", "issue:ONE"), None)
            .unwrap();
        tx.emit(draft("issues.issue.created", "issue:TWO"), None)
            .unwrap();
        assert!(tx.into_staged_rows().is_err());
    }

    #[test]
    fn detached_transaction_rejects_each_routing_mismatch_independently() {
        for mismatch in ["event_id", "aggregate", "subject"] {
            let (_, minter) = store_and_minter();
            let mut tx = OutboxTransaction::detached(minter, ctx_base());
            tx.emit(draft("issues.issue.created", "issue:DETACHED"), None)
                .unwrap();
            match mismatch {
                "event_id" => tx.staged_rows[0].event_id = EventId("different-id".into()),
                "aggregate" => tx.staged_rows[0].aggregate = AggregateKey("issue:DIFFERENT".into()),
                "subject" => {
                    tx.staged_rows[0].subject =
                        ArtifactRef("myelin://acme/issues/issue/DIFFERENT".into())
                }
                _ => unreachable!(),
            }
            assert!(
                tx.into_staged_rows().is_err(),
                "an isolated {mismatch} mismatch must be rejected"
            );
        }
    }

    #[test]
    fn detached_transaction_rejects_each_allocated_shape_independently() {
        for invalid in ["sequence", "published", "attempts"] {
            let (_, minter) = store_and_minter();
            let mut tx = OutboxTransaction::detached(minter, ctx_base());
            tx.emit(draft("issues.issue.created", "issue:DETACHED"), None)
                .unwrap();
            match invalid {
                "sequence" => tx.staged_rows[0].seq = 7,
                "published" => {
                    tx.staged_rows[0].published_at = Some(Timestamp("2026-06-19T00:00:02Z".into()))
                }
                "attempts" => tx.staged_rows[0].attempts = 1,
                _ => unreachable!(),
            }
            assert!(
                tx.into_staged_rows().is_err(),
                "an isolated {invalid} staging violation must be rejected"
            );
        }
    }

    #[test]
    fn absorb_commit_is_idempotent_but_rejects_divergent_payloads() {
        struct ConstantMinter;
        impl IdMinter for ConstantMinter {
            fn mint(&self) -> Ulid {
                Ulid("01ABSORBID0000000000000000".into())
            }
        }

        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(ConstantMinter);
        for _ in 0..2 {
            let mut tx = store.begin(Arc::clone(&minter), ctx_base());
            tx.emit(draft("issues.issue.created", "issue:ABSORB"), None)
                .unwrap();
            tx.commit_absorb().unwrap();
        }
        assert_eq!(store.committed_count(), 1, "identical replay is absorbed");
        assert_eq!(store.try_committed_rows().unwrap().len(), 1);

        let mut divergent = store.begin(minter, ctx_base());
        divergent
            .emit(draft("issues.issue.deleted", "issue:ABSORB"), None)
            .unwrap();
        assert!(
            divergent.commit_absorb().is_err(),
            "the same deterministic id with a different envelope is a collision"
        );
        assert_eq!(store.committed_count(), 1);
    }

    #[test]
    fn absorb_commit_allocates_contiguous_sequence_numbers() {
        let (store, minter) = store_and_minter();
        let mut tx = store.begin(minter, ctx_base());
        let first = tx
            .emit(draft("issues.issue.created", "issue:ABSORB"), None)
            .unwrap();
        let second = tx
            .emit(draft("issues.issue.updated", "issue:ABSORB"), None)
            .unwrap();
        tx.commit_absorb().unwrap();

        assert_eq!(store.row(&first).unwrap().seq, 0);
        assert_eq!(store.row(&second).unwrap().seq, 1);
        assert_eq!(store.committed_count(), 2);
    }

    #[test]
    fn store_backed_transaction_cannot_export_around_its_commit_boundary() {
        let (store, minter) = store_and_minter();
        let mut tx = store.begin(minter, ctx_base());
        tx.emit(draft("issues.issue.created", "issue:BACKED"), None)
            .unwrap();
        assert!(tx.into_staged_rows().is_err());
        assert_eq!(store.outbox_depth(), 0);
    }

    #[test]
    fn emit_derives_causality_and_assigns_monotonic_seq_per_aggregate() {
        let (store, minter) = store_and_minter();
        let mut tx = store.begin(minter, ctx_base());

        let root_id = tx
            .emit(draft("issues.issue.created", "issue:PROJ-1"), None)
            .unwrap();
        let root_env = store_envelope(&tx, 0);
        assert_eq!(root_env.depth, 0);
        assert_eq!(
            root_env.correlation_id.0, root_id.0,
            "root carries its own correlation"
        );

        let child_id = tx
            .emit(draft("refs.edge.created", "issue:PROJ-1"), Some(&root_env))
            .unwrap();
        let child_env = store_envelope(&tx, 1);
        assert_eq!(child_env.depth, 1, "caused event is depth parent+1");
        assert_eq!(child_env.causation_id, Some(root_id.clone()));
        assert_ne!(root_id, child_id);

        tx.commit().unwrap();
        assert_eq!(store.row(&root_id).unwrap().seq, 0);
        assert_eq!(store.row(&child_id).unwrap().seq, 1);
        assert_eq!(
            store.row(&root_id).unwrap().aggregate,
            store.row(&child_id).unwrap().aggregate
        );
    }

    fn store_envelope(tx: &OutboxTransaction, i: usize) -> EventEnvelope {
        tx.staged_rows[i].envelope.clone()
    }

    #[test]
    fn seq_is_independent_per_aggregate() {
        let (store, minter) = store_and_minter();
        let mut tx = store.begin(minter, ctx_base());
        let a0 = tx
            .emit(draft("issues.issue.created", "issue:A"), None)
            .unwrap();
        let b0 = tx
            .emit(draft("issues.issue.created", "issue:B"), None)
            .unwrap();
        let a1 = tx
            .emit(draft("issues.issue.updated", "issue:A"), None)
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(store.row(&a0).unwrap().seq, 0);
        assert_eq!(store.row(&b0).unwrap().seq, 0);
        assert_eq!(store.row(&a1).unwrap().seq, 1);
    }

    #[test]
    fn eb03_per_aggregate_seq_is_monotonic_and_gap_free_under_concurrent_emitters() {
        use std::sync::Arc as StdArc;
        let store = OutboxStore::new();
        let minter: StdArc<dyn IdMinter> = StdArc::new(MonotonicMinter::new());
        const N: u64 = 64;
        let hot = "issue:HOT";

        let mut handles = Vec::new();
        for _ in 0..N {
            let store = store.clone();
            let minter = StdArc::clone(&minter);
            handles.push(std::thread::spawn(move || {
                let mut tx = store.begin(minter, ctx_base());
                let id = tx.emit(draft("issues.issue.updated", hot), None).unwrap();
                tx.commit().unwrap();
                id
            }));
        }
        let ids: Vec<EventId> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        let mut seqs: Vec<u64> = ids.iter().map(|id| store.row(id).unwrap().seq).collect();
        seqs.sort_unstable();
        let expected: Vec<u64> = (0..N).collect();
        assert_eq!(
            seqs, expected,
            "concurrent emitters must yield contiguous, unique seqs"
        );
        assert_eq!(
            store.committed_count(),
            N as usize,
            "every committed event is present once"
        );
    }

    #[test]
    fn eb03_aborted_transaction_leaves_no_seq_gap() {
        let (store, minter) = store_and_minter();
        let agg = "issue:GAPCHECK";

        let mut ta = store.begin(Arc::clone(&minter), ctx_base());
        let a = ta.emit(draft("issues.issue.created", agg), None).unwrap();
        ta.commit().unwrap();
        assert_eq!(store.row(&a).unwrap().seq, 0);

        {
            let mut tb = store.begin(Arc::clone(&minter), ctx_base());
            tb.emit(draft("issues.issue.updated", agg), None).unwrap();
        }

        let mut tc = store.begin(Arc::clone(&minter), ctx_base());
        let c = tc.emit(draft("issues.issue.updated", agg), None).unwrap();
        tc.commit().unwrap();
        assert_eq!(
            store.row(&c).unwrap().seq,
            1,
            "abort must not burn a seq → gap-free"
        );
        assert_eq!(
            store.committed_count(),
            2,
            "only the two committed events exist"
        );
    }

    #[test]
    fn ulid_minter_two_instances_mint_disjoint_ids() {
        assert_eq!(
            MonotonicMinter::new().mint(),
            MonotonicMinter::new().mint(),
            "the deterministic test minter resets per instance (the named hazard)"
        );
        let a = UlidMinter::new();
        let b = UlidMinter::new();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1_000 {
            assert!(seen.insert(a.mint()), "minter A repeated an id");
            assert!(
                seen.insert(b.mint()),
                "minter B collided with A or repeated"
            );
        }
    }

    #[test]
    fn ulid_minter_is_monotonic_and_canonical_within_process() {
        let m = UlidMinter::new();
        let mut prev = m.mint();
        assert_eq!(prev.0.len(), 26, "canonical 26-char ULID rendering");
        for _ in 0..1_000 {
            let next = m.mint();
            assert_eq!(next.0.len(), 26);
            assert!(
                next.0.bytes().all(|b| CROCKFORD.contains(&b)),
                "canonical Crockford alphabet only"
            );
            assert!(
                prev < next,
                "same-ms burst must stay monotonic: {prev:?} < {next:?}"
            );
            prev = next;
        }
    }

    #[test]
    fn minted_ids_are_stable_and_monotonic() {
        let minter = MonotonicMinter::new();
        let a = minter.mint();
        let b = minter.mint();
        assert_ne!(a, b);
        assert!(a < b, "ULIDs are monotonic (time-ordered): {a:?} < {b:?}");
        let id: EventId = a.clone().into();
        assert_eq!(id.0, a.0);
    }

    #[derive(Default)]
    struct MockBacking {
        committed: Mutex<Vec<OutboxRow>>,
        drain_calls: Mutex<Vec<usize>>,
        snapshot_fails: bool,
    }

    impl DurableOutboxBacking for MockBacking {
        fn commit_staged(&self, rows: Vec<OutboxRow>) -> Result<()> {
            self.committed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend(rows);
            Ok(())
        }
        fn outbox_depth(&self) -> usize {
            self.committed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter(|r| r.published_at.is_none())
                .count()
        }
        fn dead_letter_count(&self) -> usize {
            0
        }
        fn oldest_unsent_recorded_at(&self) -> Option<Timestamp> {
            self.committed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .find(|r| r.published_at.is_none())
                .map(|r| r.envelope.recorded_at.clone())
        }
        fn committed_count(&self) -> usize {
            self.committed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len()
        }
        fn row(&self, id: &EventId) -> Option<OutboxRow> {
            self.committed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .find(|r| &r.event_id == id)
                .cloned()
        }
        fn committed_rows(&self) -> Vec<OutboxRow> {
            self.committed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
        fn try_committed_rows(&self) -> Result<Vec<OutboxRow>> {
            if self.snapshot_fails {
                Err(OutboxError(
                    "injected committed-row snapshot failure".into(),
                ))
            } else {
                Ok(self.committed_rows())
            }
        }
        fn dead_letters(&self) -> Vec<OutboxRow> {
            Vec::new()
        }
        fn drain_once(&self, _transport: &dyn BusTransport, batch: usize) -> Result<DrainReport> {
            self.drain_calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(batch);
            let mut rows = self.committed.lock().unwrap_or_else(|e| e.into_inner());
            let mut published = 0;
            for r in rows.iter_mut().filter(|r| r.published_at.is_none()) {
                r.published_at = Some(Timestamp("2026-06-19T00:00:09Z".into()));
                published += 1;
            }
            Ok(DrainReport {
                published,
                ..Default::default()
            })
        }
    }

    #[test]
    fn fallible_snapshot_propagates_backing_failure_instead_of_returning_empty() {
        let store = OutboxStore::durable(Arc::new(MockBacking {
            snapshot_fails: true,
            ..MockBacking::default()
        }));
        let error = store
            .try_retained_rows()
            .expect_err("load-bearing recovery snapshot must fail loud");
        assert!(error.0.contains("injected committed-row snapshot failure"));
    }

    #[test]
    fn retained_snapshot_accepts_exact_and_slack_ceilings_without_dropping_rows() {
        let (store, minter) = store_and_minter();
        let mut tx = store.begin(minter, ctx_base());
        tx.emit(draft("issues.issue.created", "issue:BOUNDED"), None)
            .unwrap();
        tx.commit().unwrap();

        let expected = store.try_retained_rows().unwrap();
        assert_eq!(expected.len(), 1, "fixture has one retained witness");
        let envelope_bytes = serde_json::to_vec(&expected[0].envelope).unwrap().len();

        assert_eq!(
            store
                .try_retained_rows_bounded(expected.len(), envelope_bytes)
                .unwrap(),
            expected,
            "both ceilings are inclusive"
        );
        assert_eq!(
            store
                .try_retained_rows_bounded(expected.len() + 1, envelope_bytes + 1)
                .unwrap(),
            expected,
            "a below-ceiling snapshot preserves every retained witness"
        );
    }

    #[test]
    fn retained_snapshot_refuses_one_row_or_envelope_byte_over_a_ceiling() {
        let (store, minter) = store_and_minter();
        let mut tx = store.begin(minter, ctx_base());
        tx.emit(draft("issues.issue.created", "issue:OVERSIZED"), None)
            .unwrap();
        tx.commit().unwrap();

        let retained = store.try_retained_rows().unwrap();
        let envelope_bytes = serde_json::to_vec(&retained[0].envelope).unwrap().len();
        assert!(envelope_bytes > 0);

        let rows_error = store
            .try_retained_rows_bounded(0, envelope_bytes)
            .expect_err("one row over the ceiling must fail closed");
        assert_eq!(
            rows_error.0,
            "retained outbox snapshot exceeds its row limit"
        );

        let bytes_error = store
            .try_retained_rows_bounded(retained.len(), envelope_bytes - 1)
            .expect_err("one envelope byte over the ceiling must fail closed");
        assert_eq!(
            bytes_error.0,
            "retained outbox snapshot exceeds its envelope byte limit"
        );
    }

    #[test]
    fn commit_dispatches_staged_rows_to_the_durable_backing() {
        let backing = Arc::new(MockBacking::default());
        let store = OutboxStore::durable(backing.clone());
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("state");
        let a = tx
            .emit(draft("issues.issue.created", "issue:A"), None)
            .unwrap();
        let b = tx
            .emit(draft("issues.issue.updated", "issue:A"), None)
            .unwrap();
        assert_eq!(tx.staged_len(), 2, "two rows buffered");
        assert_eq!(
            backing.committed_count(),
            0,
            "an open tx wrote nothing durable"
        );

        tx.commit().unwrap();
        let committed = backing.committed_rows();
        assert_eq!(committed.len(), 2, "both staged rows handed to the backing");
        assert_eq!(committed[0].event_id, a);
        assert_eq!(committed[1].event_id, b);
        assert_eq!(store.committed_count(), 2);
        assert_eq!(store.outbox_depth(), 2);
        assert_eq!(store.row(&a).unwrap().event_id, a);
    }

    #[test]
    fn durable_store_reads_route_to_the_backing() {
        let (mem, minter) = store_and_minter();
        let mut tx = mem.begin(minter, ctx_base());
        let a = tx
            .emit(draft("issues.issue.created", "issue:A"), None)
            .unwrap();
        tx.emit(draft("issues.issue.updated", "issue:B"), None)
            .unwrap();
        tx.commit().unwrap();
        let rows = mem.committed_rows();

        let backing = Arc::new(MockBacking::default());
        backing.commit_staged(rows.clone()).unwrap();
        let store = OutboxStore::durable(backing.clone());

        assert_eq!(store.committed_count(), 2);
        assert_eq!(store.outbox_depth(), 2, "both rows unsent in the backing");
        assert_eq!(store.committed_rows().len(), 2);
        assert_eq!(store.row(&a).unwrap().event_id, a);
        assert!(store.dead_letters().is_empty());
        assert_eq!(store.dead_letter_count(), 0);
        assert_eq!(
            store.oldest_unsent_recorded_at(),
            Some(rows[0].envelope.recorded_at.clone()),
            "oldest-unsent age anchor read off the backing"
        );
    }

    #[test]
    fn relay_drain_routes_to_the_durable_backing_composite_verb() {
        use crate::relay::{InProcessBus, Relay, DEFAULT_DRAIN_BATCH};
        let backing = Arc::new(MockBacking::default());
        let store = OutboxStore::durable(backing.clone());
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let mut tx = store.begin(minter, ctx_base());
        tx.emit(draft("issues.issue.created", "issue:A"), None)
            .unwrap();
        tx.emit(draft("issues.issue.updated", "issue:A"), None)
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(store.outbox_depth(), 2);

        let relay = Relay::new(store.clone(), InProcessBus::new(), || {
            Timestamp("2026-06-19T00:00:09Z".into())
        });
        let report = relay.drain_once();
        assert_eq!(report.published, 2, "drain routed to backing.drain_once");
        assert_eq!(
            store.outbox_depth(),
            0,
            "the backing marked the rows published"
        );
        assert_eq!(
            backing
                .drain_calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_slice(),
            &[DEFAULT_DRAIN_BATCH],
            "the composite verb was called once with the default batch bound"
        );
    }
}
