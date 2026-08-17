use myelin_events::{Firehose, FirehoseError, FirehoseScope, FrameDraft};
use myelin_identity::Principal;
use myelin_storage::blob::ContentHash;
use myelin_tenancy::TenantId;
use std::collections::HashMap;

pub fn knowledge_stream(tenant: &TenantId) -> String {
    format!("fan.{}.knowledge", tenant.0)
}

pub fn doc_scope(page_id: &str) -> Result<FirehoseScope, FirehoseError> {
    FirehoseScope::parse(&format!("doc:{page_id}"))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OpId {
    pub client_id: String,
    pub lamport: u64,
}

impl OpId {
    pub fn new(client_id: impl Into<String>, lamport: u64) -> OpId {
        OpId {
            client_id: client_id.into(),
            lamport,
        }
    }

    pub fn wire(&self) -> String {
        format!("{}:{}", self.client_id, self.lamport)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Insert,
    Delete,
    Format,
    Move,
    SetProp,
    BlockIns,
    BlockDel,
    EnginePromote,
}

impl OpKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OpKind::Insert => "insert",
            OpKind::Delete => "delete",
            OpKind::Format => "format",
            OpKind::Move => "move",
            OpKind::SetProp => "set_prop",
            OpKind::BlockIns => "block_ins",
            OpKind::BlockDel => "block_del",
            OpKind::EnginePromote => "engine_promote",
        }
    }

    pub fn is_structural(self) -> bool {
        matches!(self, OpKind::Move | OpKind::BlockIns | OpKind::BlockDel)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocOp {
    pub op_id: OpId,
    pub actor: String,
    pub kind: OpKind,
    pub payload: Vec<u8>,
    pub pii_key_ref: Option<String>,
}

impl DocOp {
    pub fn cas(
        op_id: OpId,
        actor: impl Into<String>,
        kind: OpKind,
        payload: impl Into<Vec<u8>>,
    ) -> DocOp {
        DocOp {
            op_id,
            actor: actor.into(),
            kind,
            payload: payload.into(),
            pii_key_ref: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedOp {
    pub op_seq: u64,
    pub op: DocOp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendOutcome {
    Applied(PersistedOp),
    Duplicate(PersistedOp),
}

impl SendOutcome {
    pub fn persisted(&self) -> &PersistedOp {
        match self {
            SendOutcome::Applied(p) | SendOutcome::Duplicate(p) => p,
        }
    }

    pub fn applied(&self) -> bool {
        matches!(self, SendOutcome::Applied(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpLogError {
    ConflictingOpId(OpId),
    SequenceExhausted,
}

impl core::fmt::Display for OpLogError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            OpLogError::ConflictingOpId(op_id) => write!(
                f,
                "op id `{}` was reused with different content",
                op_id.wire()
            ),
            OpLogError::SequenceExhausted => f.write_str("document operation cursor is exhausted"),
        }
    }
}

impl std::error::Error for OpLogError {}

#[derive(Debug, Default, Clone)]
pub struct DocOpLog {
    ops: Vec<PersistedOp>,
    by_op_id: HashMap<[u8; 32], OpReceipt>,
    last_seq: u64,
}

#[derive(Clone, Debug)]
struct OpReceipt {
    op_seq: u64,
    fingerprint: [u8; 32],
}

fn op_fingerprint(op: &DocOp) -> [u8; 32] {
    fn field(hasher: &mut blake3::Hasher, bytes: &[u8]) {
        hasher.update(&(bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }

    let mut hasher = blake3::Hasher::new();
    field(&mut hasher, op.op_id.client_id.as_bytes());
    hasher.update(&op.op_id.lamport.to_be_bytes());
    field(&mut hasher, op.actor.as_bytes());
    field(&mut hasher, op.kind.as_str().as_bytes());
    field(&mut hasher, &op.payload);
    match &op.pii_key_ref {
        Some(key_ref) => {
            hasher.update(&[1]);
            field(&mut hasher, key_ref.as_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
    *hasher.finalize().as_bytes()
}

fn op_id_fingerprint(op_id: &OpId) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&(op_id.client_id.len() as u64).to_be_bytes());
    hasher.update(op_id.client_id.as_bytes());
    hasher.update(&op_id.lamport.to_be_bytes());
    *hasher.finalize().as_bytes()
}

impl DocOpLog {
    pub fn new() -> DocOpLog {
        DocOpLog::default()
    }

    pub fn persist(&mut self, op: DocOp) -> Result<SendOutcome, OpLogError> {
        let op_id = op_id_fingerprint(&op.op_id);
        let fingerprint = op_fingerprint(&op);
        if let Some(receipt) = self.by_op_id.get(&op_id) {
            if receipt.fingerprint != fingerprint {
                return Err(OpLogError::ConflictingOpId(op.op_id));
            }
            let persisted = self
                .ops
                .iter()
                .find(|persisted| persisted.op_seq == receipt.op_seq)
                .cloned()
                .unwrap_or(PersistedOp {
                    op_seq: receipt.op_seq,
                    op,
                });
            return Ok(SendOutcome::Duplicate(persisted));
        }
        let op_seq = self
            .last_seq
            .checked_add(1)
            .ok_or(OpLogError::SequenceExhausted)?;
        let persisted = PersistedOp { op_seq, op };
        self.last_seq = op_seq;
        self.by_op_id.insert(
            op_id,
            OpReceipt {
                op_seq,
                fingerprint,
            },
        );
        self.ops.push(persisted.clone());
        Ok(SendOutcome::Applied(persisted))
    }

    pub fn ops_since(&self, last_seq: u64) -> Vec<PersistedOp> {
        self.ops
            .iter()
            .filter(|p| p.op_seq > last_seq)
            .cloned()
            .collect()
    }

    pub fn head_seq(&self) -> u64 {
        self.last_seq
    }

    pub fn ops_up_to(&self, up_to: u64) -> Vec<PersistedOp> {
        self.ops
            .iter()
            .filter(|p| p.op_seq <= up_to)
            .cloned()
            .collect()
    }

    pub fn ops_in_range(&self, from: u64, to: u64) -> Vec<PersistedOp> {
        self.ops
            .iter()
            .filter(|p| p.op_seq > from && p.op_seq <= to)
            .cloned()
            .collect()
    }

    pub fn lowest_seq(&self) -> u64 {
        self.ops.iter().map(|p| p.op_seq).min().unwrap_or(0)
    }

    pub fn gc_below(&mut self, watermark: u64) -> usize {
        let before = self.ops.len();
        self.ops.retain(|p| p.op_seq > watermark);
        before - self.ops.len()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn seed_from_snapshot(&mut self, snapshot: &PageSnapshot) {
        if snapshot.snap_seq > self.last_seq {
            self.last_seq = snapshot.snap_seq;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageSnapshot {
    pub snap_seq: u64,
    pub blob_hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recovery {
    Resumed {
        cursor: u64,
        backfill: Vec<PersistedOp>,
    },
    RebuiltFromLog {
        cursor: u64,
        backfill: Vec<PersistedOp>,
    },
    ResyncFromSnapshot {
        cursor: u64,
        snapshot: PageSnapshot,
        tail: Vec<PersistedOp>,
    },
}

impl Recovery {
    pub fn cursor(&self) -> u64 {
        match self {
            Recovery::Resumed { cursor, .. }
            | Recovery::RebuiltFromLog { cursor, .. }
            | Recovery::ResyncFromSnapshot { cursor, .. } => *cursor,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    OverBroadScope(String),
    Unauthorized { page_id: String },
    ActorMismatch { page_id: String },
    CursorMismatch { page_id: String },
    InvalidSnapshot { page_id: String },
    OpLog(OpLogError),
    Firehose(FirehoseError),
}

impl core::fmt::Display for TransportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            TransportError::OverBroadScope(s) => {
                write!(
                    f,
                    "collab transport rejects over-broad doc scope `{s}` (never `*`)"
                )
            }
            TransportError::Unauthorized { page_id } => {
                write!(f, "collab op denied by Layer-2 authority on page `{page_id}` (no op without authz)")
            }
            TransportError::ActorMismatch { page_id } => write!(
                f,
                "collab op actor does not match its authenticated principal on page `{page_id}`"
            ),
            TransportError::CursorMismatch { page_id } => write!(
                f,
                "snapshot cursor does not continue the live stream on page `{page_id}`"
            ),
            TransportError::InvalidSnapshot { page_id } => write!(
                f,
                "snapshot is empty, regressive, or conflicts on page `{page_id}`"
            ),
            TransportError::OpLog(error) => write!(f, "collab op rejected: {error}"),
            TransportError::Firehose(error) => write!(f, "collab transport failed: {error}"),
        }
    }
}

impl std::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TransportError::OpLog(error) => Some(error),
            TransportError::Firehose(error) => Some(error),
            _ => None,
        }
    }
}

pub trait OpAuthority {
    fn authorize(&self, principal: &Principal, page_id: &str, action: AuthAction) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthAction {
    Edit,
    Comment,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FailClosedAuthority;

impl OpAuthority for FailClosedAuthority {
    fn authorize(&self, _principal: &Principal, _page_id: &str, _action: AuthAction) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AllowAllAuthority;

impl OpAuthority for AllowAllAuthority {
    fn authorize(&self, _principal: &Principal, _page_id: &str, _action: AuthAction) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presence {
    pub client_id: String,
    pub awareness: String,
}

impl Presence {
    pub fn new(client_id: impl Into<String>, awareness: impl Into<String>) -> Presence {
        Presence {
            client_id: client_id.into(),
            awareness: awareness.into(),
        }
    }
}

pub struct CollabTransport<A: OpAuthority> {
    tenant: TenantId,
    page_id: String,
    stream: String,
    scope: FirehoseScope,
    log: DocOpLog,
    firehose: Firehose,
    authority: A,
    snapshot: Option<PageSnapshot>,
}

impl<A: OpAuthority> CollabTransport<A> {
    pub fn open(
        tenant: TenantId,
        page_id: &str,
        authority: A,
    ) -> Result<CollabTransport<A>, TransportError> {
        let scope = doc_scope(page_id)
            .map_err(|_| TransportError::OverBroadScope(format!("doc:{page_id}")))?;
        let stream = knowledge_stream(&tenant);
        Ok(CollabTransport {
            tenant,
            page_id: page_id.to_string(),
            stream,
            scope,
            log: DocOpLog::new(),
            firehose: Firehose::new(),
            authority,
            snapshot: None,
        })
    }

    pub fn open_with_window(
        tenant: TenantId,
        page_id: &str,
        authority: A,
        window_frames: usize,
    ) -> Result<CollabTransport<A>, TransportError> {
        let mut t = CollabTransport::open(tenant, page_id, authority)?;
        t.firehose = Firehose::with_limits(window_frames, myelin_events::DEFAULT_INFLIGHT_CAP);
        Ok(t)
    }

    pub fn install_snapshot(&mut self, snapshot: PageSnapshot) -> Result<(), TransportError> {
        let conflicts = self.snapshot.as_ref().is_some_and(|installed| {
            snapshot.snap_seq < installed.snap_seq
                || (snapshot.snap_seq == installed.snap_seq
                    && snapshot.blob_hash != installed.blob_hash)
        });
        if snapshot.blob_hash.digest_hex.is_empty() || conflicts {
            return Err(TransportError::InvalidSnapshot {
                page_id: self.page_id.clone(),
            });
        }
        let head = self.log.head_seq().max(snapshot.snap_seq);
        if !self.firehose.seed_head(&self.stream, &self.scope, head) {
            return Err(TransportError::CursorMismatch {
                page_id: self.page_id.clone(),
            });
        }
        self.log.seed_from_snapshot(&snapshot);
        self.snapshot = Some(snapshot);
        Ok(())
    }

    pub fn page_id(&self) -> &str {
        &self.page_id
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }

    pub fn scope(&self) -> &FirehoseScope {
        &self.scope
    }

    pub fn head_seq(&self) -> u64 {
        self.log.head_seq()
    }

    fn authorize(&self, principal: &Principal, action: AuthAction) -> Result<(), TransportError> {
        if principal.tenant != self.tenant
            || !self.authority.authorize(principal, &self.page_id, action)
        {
            return Err(TransportError::Unauthorized {
                page_id: self.page_id.clone(),
            });
        }
        Ok(())
    }

    pub fn recover(
        &self,
        principal: &Principal,
        action: AuthAction,
        cursor: Option<u64>,
    ) -> Result<Recovery, TransportError> {
        self.authorize(principal, action)?;

        let last_seq = cursor.unwrap_or(0);
        let cursor = self.log.head_seq();
        match self.firehose.backfill(&self.stream, &self.scope, last_seq) {
            Ok(_) => {
                let backfill = self.log.ops_since(last_seq);
                Ok(Recovery::Resumed { cursor, backfill })
            }
            Err(e) if e.is_resync_required() => {
                if let Some(snapshot) = self.snapshot.clone() {
                    let tail = self.log.ops_since(snapshot.snap_seq);
                    Ok(Recovery::ResyncFromSnapshot {
                        cursor,
                        snapshot,
                        tail,
                    })
                } else {
                    Ok(Recovery::RebuiltFromLog {
                        cursor,
                        backfill: self.log.ops_since(0),
                    })
                }
            }
            Err(error) => Err(TransportError::Firehose(error)),
        }
    }

    pub fn send_op(
        &mut self,
        principal: &Principal,
        op: DocOp,
    ) -> Result<SendOutcome, TransportError> {
        self.authorize(principal, AuthAction::Edit)?;
        if op.actor != principal.principal_id.0 {
            return Err(TransportError::ActorMismatch {
                page_id: self.page_id.clone(),
            });
        }
        let outcome = self.log.persist(op).map_err(TransportError::OpLog)?;
        if let SendOutcome::Applied(persisted) = &outcome {
            let _frame = self.firehose.publish(
                &self.stream,
                &self.scope,
                FrameDraft::new(format!(
                    "{}@{}",
                    persisted.op.op_id.wire(),
                    persisted.op_seq
                )),
            );
        }
        Ok(outcome)
    }

    pub fn subscribe(
        &mut self,
        principal: &Principal,
        action: AuthAction,
        cursor: Option<u64>,
    ) -> Result<myelin_events::FirehoseSubscription, TransportError> {
        self.authorize(principal, action)?;
        self.firehose
            .subscribe(&self.stream, &self.scope, cursor)
            .map_err(TransportError::Firehose)
    }

    pub fn publish_presence(
        &mut self,
        principal: &Principal,
        presence: &Presence,
    ) -> Result<(), TransportError> {
        self.authorize(principal, AuthAction::Edit)?;
        let presence_stream = format!("{}.presence", self.stream);
        self.firehose.publish(
            &presence_stream,
            &self.scope,
            FrameDraft::new(format!("{}|{}", presence.client_id, presence.awareness)),
        );
        Ok(())
    }

    pub fn op_count(&self) -> usize {
        self.log.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("actor-1".into()),
            PrincipalKind::Human,
            tenant(),
        )
    }

    fn open() -> CollabTransport<AllowAllAuthority> {
        CollabTransport::open(tenant(), "page-1", AllowAllAuthority).expect("opens")
    }

    fn op(client: &str, lamport: u64, kind: OpKind) -> DocOp {
        DocOp::cas(
            OpId::new(client, lamport),
            "actor-1",
            kind,
            format!("cas:{client}:{lamport}").into_bytes(),
        )
    }

    fn send(t: &mut CollabTransport<AllowAllAuthority>, op: DocOp) -> SendOutcome {
        t.send_op(&principal(), op)
            .expect("the actor is authorized to edit")
    }

    #[test]
    fn op_seq_is_per_doc_monotonic() {
        let mut t = open();
        let a = send(&mut t, op("c1", 1, OpKind::Insert));
        let b = send(&mut t, op("c1", 2, OpKind::Insert));
        let c = send(&mut t, op("c2", 1, OpKind::Insert));
        assert_eq!(a.persisted().op_seq, 1);
        assert_eq!(b.persisted().op_seq, 2);
        assert_eq!(
            c.persisted().op_seq,
            3,
            "monotone across clients (one per-doc cursor)"
        );
        assert_eq!(t.head_seq(), 3);
        assert!(
            a.applied() && b.applied() && c.applied(),
            "all three are fresh applies"
        );
    }

    #[test]
    fn an_exhausted_operation_cursor_fails_without_mutation() {
        let mut log = DocOpLog::new();
        log.seed_from_snapshot(&PageSnapshot {
            snap_seq: u64::MAX,
            blob_hash: ContentHash::blake3(b"last"),
        });

        assert_eq!(
            log.persist(op("c1", 1, OpKind::Insert)),
            Err(OpLogError::SequenceExhausted)
        );
        assert_eq!(log.head_seq(), u64::MAX);
        assert!(log.is_empty(), "the rejected operation is not retained");
    }

    #[test]
    fn a_redelivered_op_is_an_idempotent_no_op() {
        let mut t = open();
        let first = send(&mut t, op("c1", 7, OpKind::Insert));
        assert!(first.applied());
        assert_eq!(t.head_seq(), 1);

        let redelivered = send(&mut t, op("c1", 7, OpKind::Insert));
        assert!(
            !redelivered.applied(),
            "a re-delivered op did NOT freshly apply"
        );
        assert!(
            matches!(redelivered, SendOutcome::Duplicate(_)),
            "it is reported a Duplicate no-op"
        );
        assert_eq!(
            redelivered.persisted().op_seq,
            1,
            "the duplicate resolves to the FIRST op_seq (no new seq assigned)"
        );
        assert_eq!(
            t.head_seq(),
            1,
            "the head did NOT advance (0 duplicate effect)"
        );
        assert_eq!(
            t.op_count(),
            1,
            "exactly one op in the log (the duplicate was a no-op)"
        );
    }

    #[test]
    fn out_of_window_cursor_resyncs_from_snapshot() {
        let mut t = CollabTransport::open_with_window(tenant(), "page-1", AllowAllAuthority, 3)
            .expect("opens");
        t.install_snapshot(PageSnapshot {
            snap_seq: 2,
            blob_hash: ContentHash::blake3(b"snapshot at 2"),
        })
        .expect("the snapshot seeds an empty live stream");
        for i in 1..=6 {
            send(&mut t, op("c1", i, OpKind::Insert));
        }

        let connected = t
            .recover(&principal(), AuthAction::Edit, Some(1))
            .expect("connect succeeds via the cold path");
        match connected {
            Recovery::ResyncFromSnapshot { snapshot, tail, .. } => {
                assert_eq!(
                    snapshot.snap_seq, 2,
                    "the cold path loads the installed snapshot"
                );
                let seqs: Vec<u64> = tail.iter().map(|p| p.op_seq).collect();
                assert_eq!(
                    seqs,
                    vec![3, 4, 5, 6, 7, 8],
                    "the live tail after the snapshot, 0 ops lost"
                );
            }
            other => panic!("expected ResyncFromSnapshot, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_and_live_operations_share_one_cursor() {
        let mut t = open();
        t.install_snapshot(PageSnapshot {
            snap_seq: 7,
            blob_hash: ContentHash::blake3(b"snapshot at 7"),
        })
        .expect("the snapshot seeds an empty live stream");
        let sub = t
            .subscribe(&principal(), AuthAction::Edit, None)
            .expect("the viewer starts live after the snapshot");

        let sent = send(&mut t, op("c1", 1, OpKind::Insert));
        let frame = sub.pull().expect("the edit is delivered live");

        assert_eq!(sent.persisted().op_seq, 8);
        assert_eq!(frame.seq, 8, "the live cursor is the durable op cursor");
    }

    #[test]
    fn snapshot_installation_is_monotonic_and_content_addressed() {
        let mut t = open();
        let installed = PageSnapshot {
            snap_seq: 7,
            blob_hash: ContentHash::blake3(b"seven"),
        };
        t.install_snapshot(installed.clone())
            .expect("the first snapshot installs");
        t.install_snapshot(installed)
            .expect("the same snapshot is idempotent");

        for invalid in [
            PageSnapshot {
                snap_seq: 6,
                blob_hash: ContentHash::blake3(b"six"),
            },
            PageSnapshot {
                snap_seq: 7,
                blob_hash: ContentHash::blake3(b"different"),
            },
            PageSnapshot {
                snap_seq: 8,
                blob_hash: ContentHash {
                    algo: myelin_storage::blob::HashAlgo::Blake3,
                    digest_hex: String::new(),
                },
            },
        ] {
            assert!(matches!(
                t.install_snapshot(invalid),
                Err(TransportError::InvalidSnapshot { .. })
            ));
        }
        assert_eq!(t.head_seq(), 7, "rejected snapshots cannot move the cursor");
    }

    #[test]
    fn in_window_cursor_resumes_without_a_snapshot() {
        let mut t = CollabTransport::open_with_window(tenant(), "page-1", AllowAllAuthority, 3)
            .expect("opens");
        for i in 1..=6 {
            send(&mut t, op("c1", i, OpKind::Insert));
        }
        let connected = t
            .recover(&principal(), AuthAction::Edit, Some(4))
            .expect("warm resume");
        match connected {
            Recovery::Resumed { backfill, .. } => {
                assert_eq!(
                    backfill.iter().map(|p| p.op_seq).collect::<Vec<_>>(),
                    vec![5, 6],
                    "the warm path backfills (last_seq, now] from the op-log"
                );
            }
            other => panic!("expected Resumed, got {other:?}"),
        }
    }

    #[test]
    fn an_expired_cursor_without_a_snapshot_rebuilds_from_the_durable_log() {
        let mut t = CollabTransport::open_with_window(tenant(), "page-1", AllowAllAuthority, 2)
            .expect("opens");
        for i in 1..=4 {
            send(&mut t, op("c1", i, OpKind::Insert));
        }

        let connected = t
            .recover(&principal(), AuthAction::Edit, Some(0))
            .expect("the durable log remains recoverable");

        match connected {
            Recovery::RebuiltFromLog { backfill, .. } => assert_eq!(
                backfill.iter().map(|op| op.op_seq).collect::<Vec<_>>(),
                vec![1, 2, 3, 4],
                "the cold rebuild is explicit and loses no operations"
            ),
            other => panic!("expected RebuiltFromLog, got {other:?}"),
        }
    }

    #[test]
    fn an_over_broad_scope_is_rejected_at_open() {
        for bad in ["*", "page*", "", "  "] {
            let r = CollabTransport::open(tenant(), bad, FailClosedAuthority);
            assert!(
                r.is_err(),
                "an over-broad page scope `{bad}` must be rejected at open"
            );
            assert!(
                matches!(r, Err(TransportError::OverBroadScope(_))),
                "`{bad}` is an over-broad-scope rejection"
            );
        }
        assert!(
            CollabTransport::open(tenant(), "page-abc-123", FailClosedAuthority).is_ok(),
            "a bounded page scope opens"
        );
    }

    #[test]
    fn recovery_fail_closes_without_authority() {
        let t = CollabTransport::open(tenant(), "page-1", FailClosedAuthority).expect("opens");
        let r = t.recover(&principal(), AuthAction::Edit, None);
        assert!(
            matches!(r, Err(TransportError::Unauthorized { .. })),
            "the fail-closed authority denies the connect (no op without authz)"
        );
    }

    #[test]
    fn every_collaboration_entry_point_fail_closes() {
        let mut t = CollabTransport::open(tenant(), "page-1", FailClosedAuthority).expect("opens");
        let actor = principal();

        assert!(matches!(
            t.send_op(&actor, op("c1", 1, OpKind::Insert)),
            Err(TransportError::Unauthorized { .. })
        ));
        assert!(matches!(
            t.subscribe(&actor, AuthAction::Edit, None),
            Err(TransportError::Unauthorized { .. })
        ));
        assert!(matches!(
            t.publish_presence(&actor, &Presence::new("c1", "caret:0")),
            Err(TransportError::Unauthorized { .. })
        ));
        assert_eq!(t.head_seq(), 0, "a denial advances no cursor");
        assert_eq!(t.op_count(), 0, "a denial persists no operation");
    }

    #[test]
    fn even_an_allow_all_authority_cannot_cross_a_tenant_boundary() {
        let mut t = open();
        let foreign = Principal::stub(
            PrincipalId("actor-1".into()),
            PrincipalKind::Human,
            TenantId("other".into()),
        );

        assert!(matches!(
            t.recover(&foreign, AuthAction::Edit, None),
            Err(TransportError::Unauthorized { .. })
        ));
        assert!(matches!(
            t.send_op(&foreign, op("c1", 1, OpKind::Insert)),
            Err(TransportError::Unauthorized { .. })
        ));
        assert!(matches!(
            t.subscribe(&foreign, AuthAction::Edit, None),
            Err(TransportError::Unauthorized { .. })
        ));
        assert_eq!(t.op_count(), 0, "cross-tenant input leaves no trace");
    }

    #[test]
    fn an_operation_cannot_spoof_its_authenticated_actor() {
        let mut t = open();
        let mut forged = op("c1", 1, OpKind::Insert);
        forged.actor = "someone-else".into();

        assert!(matches!(
            t.send_op(&principal(), forged),
            Err(TransportError::ActorMismatch { .. })
        ));
        assert_eq!(t.op_count(), 0, "the forged operation was not persisted");
    }

    #[test]
    fn an_operation_id_cannot_be_reused_for_different_content() {
        let mut t = open();
        send(&mut t, op("c1", 1, OpKind::Insert));
        let mut conflicting = op("c1", 1, OpKind::Insert);
        conflicting.payload = b"different edit".to_vec();

        assert!(matches!(
            t.send_op(&principal(), conflicting),
            Err(TransportError::OpLog(OpLogError::ConflictingOpId(_)))
        ));
        assert_eq!(t.head_seq(), 1, "the conflict advances no cursor");
        assert_eq!(t.op_count(), 1, "the original operation remains canonical");
    }

    #[test]
    fn authorized_recovery_returns_the_durable_backfill() {
        let mut t = open();
        send(&mut t, op("c1", 1, OpKind::Insert));
        send(&mut t, op("c1", 2, OpKind::Insert));
        let connected = t
            .recover(&principal(), AuthAction::Edit, None)
            .expect("authorized connect");
        match connected {
            Recovery::Resumed { backfill, .. } => {
                assert_eq!(
                    backfill.len(),
                    2,
                    "a fresh authorized connect backfills the whole tail"
                );
            }
            other => panic!("expected Resumed, got {other:?}"),
        }
    }

    #[test]
    fn the_recovery_cursor_hands_off_to_live_without_a_gap() {
        let mut t = open();
        send(&mut t, op("c1", 1, OpKind::Insert));
        send(&mut t, op("c1", 2, OpKind::Insert));
        let recovery = t
            .recover(&principal(), AuthAction::Edit, None)
            .expect("the actor recovers through the current head");
        assert_eq!(recovery.cursor(), 2);

        send(&mut t, op("c1", 3, OpKind::Insert));
        let live = t
            .subscribe(&principal(), AuthAction::Edit, Some(recovery.cursor()))
            .expect("the recovery cursor opens the live stream");
        assert_eq!(
            live.drain_ready()
                .into_iter()
                .map(|frame| frame.seq)
                .collect::<Vec<_>>(),
            vec![3],
            "the edit between recovery and subscribe is replayed"
        );

        send(&mut t, op("c1", 4, OpKind::Insert));
        assert_eq!(
            live.pull().map(|frame| frame.seq),
            Some(4),
            "the same subscription continues live"
        );
    }

    #[test]
    fn presence_is_ephemeral_and_never_persisted() {
        let mut t = open();
        send(&mut t, op("c1", 1, OpKind::Insert));
        let head_before = t.head_seq();
        let ops_before = t.op_count();

        for i in 0..50 {
            t.publish_presence(&principal(), &Presence::new("c1", format!("caret:{i}")))
                .expect("the actor is authorized to publish presence");
        }
        assert_eq!(
            t.head_seq(),
            head_before,
            "presence did NOT advance the op-log cursor"
        );
        assert_eq!(
            t.op_count(),
            ops_before,
            "presence is NEVER persisted to the op-log (arch §2.3)"
        );
    }

    #[test]
    fn a_second_connection_sees_an_edit_live() {
        let mut t = open();
        let sub = t
            .subscribe(&principal(), AuthAction::Edit, None)
            .expect("a live subscription opens");
        let sent = send(&mut t, op("c2", 1, OpKind::Insert));
        let frames = sub.drain_ready();
        assert_eq!(
            frames.len(),
            1,
            "the live subscriber received the published frame"
        );
        assert_eq!(
            frames[0].seq,
            sent.persisted().op_seq,
            "the live frame seq == the op_seq"
        );
    }

    #[test]
    fn op_kind_structural_classification() {
        assert!(OpKind::Move.is_structural());
        assert!(OpKind::BlockIns.is_structural());
        assert!(OpKind::BlockDel.is_structural());
        assert!(!OpKind::Insert.is_structural());
        assert!(!OpKind::Format.is_structural());
        for k in [
            OpKind::Insert,
            OpKind::Delete,
            OpKind::Format,
            OpKind::Move,
            OpKind::SetProp,
            OpKind::BlockIns,
            OpKind::BlockDel,
            OpKind::EnginePromote,
        ] {
            assert!(!k.as_str().is_empty());
        }
    }

    #[test]
    fn op_id_wire_form() {
        assert_eq!(OpId::new("c1", 42).wire(), "c1:42");
        assert_ne!(OpId::new("c1", 1).wire(), OpId::new("c1", 2).wire());
        assert_eq!(OpId::new("c1", 1).wire(), OpId::new("c1", 1).wire());
    }

    fn log_seq(n: u64) -> DocOpLog {
        let mut log = DocOpLog::new();
        for i in 1..=n {
            log.persist(op("c1", i, OpKind::Insert))
                .expect("the op id is fresh");
        }
        log
    }

    #[test]
    fn ops_up_to_is_inclusive() {
        let log = log_seq(5);
        let seqs: Vec<u64> = log.ops_up_to(3).iter().map(|p| p.op_seq).collect();
        assert_eq!(
            seqs,
            vec![1, 2, 3],
            "ops_up_to(3) includes op_seq 3 (inclusive prefix)"
        );
        assert!(log.ops_up_to(0).is_empty(), "ops_up_to(0) is empty");
        assert_eq!(
            log.ops_up_to(5).len(),
            5,
            "ops_up_to(head) is the whole log"
        );
    }

    #[test]
    fn ops_in_range_is_from_exclusive_to_inclusive() {
        let log = log_seq(6);
        let seqs: Vec<u64> = log.ops_in_range(2, 5).iter().map(|p| p.op_seq).collect();
        assert_eq!(
            seqs,
            vec![3, 4, 5],
            "(2, 5] excludes the seed boundary 2 and includes the target 5"
        );
        assert!(
            !log.ops_in_range(3, 6).iter().any(|p| p.op_seq == 3),
            "op_seq == from is excluded (the seed already covers it)"
        );
    }

    #[test]
    fn lowest_seq_is_the_gc_floor() {
        let mut log = log_seq(5);
        assert_eq!(log.lowest_seq(), 1, "the lowest retained op is op_seq 1");
        log.gc_below(2);
        assert_eq!(
            log.lowest_seq(),
            3,
            "after GC ≤ 2 the floor rises to op_seq 3"
        );
        log.gc_below(99);
        assert_eq!(log.lowest_seq(), 0, "an empty log's floor is 0");
    }

    #[test]
    fn gc_below_prunes_at_and_below_the_watermark_keeps_above() {
        let mut log = log_seq(6);
        let pruned = log.gc_below(3);
        assert_eq!(pruned, 3, "exactly op_seq 1,2,3 (≤ watermark) pruned");
        let kept: Vec<u64> = log.ops_up_to(6).iter().map(|p| p.op_seq).collect();
        assert_eq!(
            kept,
            vec![4, 5, 6],
            "op_seq AT the watermark (3) is pruned; ABOVE it is kept"
        );
        assert_eq!(
            log.head_seq(),
            6,
            "the monotone op_seq counter survives the prune"
        );
        let redelivered_kept = log
            .persist(op("c1", 4, OpKind::Insert))
            .expect("an identical redelivery is valid");
        assert!(
            matches!(redelivered_kept, SendOutcome::Duplicate(_)),
            "a retained op's op_id stays in the index → its re-delivery is an idempotent Duplicate"
        );
        assert_eq!(
            redelivered_kept.persisted().op_seq,
            4,
            "resolves to the kept op_seq (4)"
        );
        let redelivered_pruned = log
            .persist(op("c1", 2, OpKind::Insert))
            .expect("an identical compacted redelivery is valid");
        assert!(
            matches!(redelivered_pruned, SendOutcome::Duplicate(_)),
            "a compacted operation remains an idempotent duplicate"
        );
        assert_eq!(redelivered_pruned.persisted().op_seq, 2);
        let redelivered_at_watermark = log
            .persist(op("c1", 3, OpKind::Insert))
            .expect("an identical compacted redelivery is valid");
        assert!(
            matches!(redelivered_at_watermark, SendOutcome::Duplicate(_)),
            "the operation at the watermark remains idempotent too"
        );
        let mut conflicting = op("c1", 2, OpKind::Insert);
        conflicting.payload = b"different after compaction".to_vec();
        assert!(matches!(
            log.persist(conflicting),
            Err(OpLogError::ConflictingOpId(_))
        ));
        let next = log
            .persist(op("c1", 7, OpKind::Insert))
            .expect("the op id is fresh");
        assert_eq!(
            next.persisted().op_seq,
            7,
            "duplicates after GC do not advance the monotonic cursor"
        );
        let mut log2 = log_seq(3);
        assert_eq!(log2.gc_below(0), 0, "gc_below(0) prunes nothing");
        assert_eq!(log2.len(), 3);
    }
}
