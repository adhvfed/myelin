use myelin_events::{Firehose, FirehoseError, FirehoseScope, FrameDraft};
use myelin_identity::Principal;
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

#[derive(Debug, Default, Clone)]
pub struct DocOpLog {
    ops: Vec<PersistedOp>,
    by_op_id: HashMap<String, u64>,
    last_seq: u64,
}

impl DocOpLog {
    pub fn new() -> DocOpLog {
        DocOpLog::default()
    }

    pub fn persist(&mut self, op: DocOp) -> SendOutcome {
        let wire = op.op_id.wire();
        if let Some(&existing_seq) = self.by_op_id.get(&wire) {
            let existing = self
                .ops
                .iter()
                .find(|p| p.op_seq == existing_seq)
                .cloned()
                .expect("the by_op_id index points at a persisted op");
            return SendOutcome::Duplicate(existing);
        }
        self.last_seq += 1;
        let persisted = PersistedOp {
            op_seq: self.last_seq,
            op,
        };
        self.by_op_id.insert(wire, self.last_seq);
        self.ops.push(persisted.clone());
        SendOutcome::Applied(persisted)
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
        self.by_op_id.retain(|_, seq| *seq > watermark);
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
    pub blob_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Connected {
    Resumed {
        backfill: Vec<PersistedOp>,
    },
    ResyncFromSnapshot {
        snapshot: PageSnapshot,
        tail: Vec<PersistedOp>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    OverBroadScope(String),
    Unauthorized { page_id: String },
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
        }
    }
}

impl std::error::Error for TransportError {}

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

pub struct CollabTransport<A: OpAuthority = FailClosedAuthority> {
    tenant: TenantId,
    page_id: String,
    stream: String,
    scope: FirehoseScope,
    log: DocOpLog,
    firehose: Firehose,
    authority: A,
    snapshot: Option<PageSnapshot>,
}

impl CollabTransport<FailClosedAuthority> {
    pub fn open(
        tenant: TenantId,
        page_id: &str,
    ) -> Result<CollabTransport<FailClosedAuthority>, TransportError> {
        CollabTransport::open_with_authority(tenant, page_id, FailClosedAuthority)
    }
}

impl<A: OpAuthority> CollabTransport<A> {
    pub fn open_with_authority(
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
        let mut t = CollabTransport::open_with_authority(tenant, page_id, authority)?;
        t.firehose = Firehose::with_limits(window_frames, myelin_events::DEFAULT_INFLIGHT_CAP);
        Ok(t)
    }

    pub fn install_snapshot(&mut self, snapshot: PageSnapshot) {
        self.log.seed_from_snapshot(&snapshot);
        self.snapshot = Some(snapshot);
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

    pub fn connect(
        &mut self,
        principal: &Principal,
        action: AuthAction,
        cursor: Option<u64>,
    ) -> Result<Connected, TransportError> {
        if !self.authority.authorize(principal, &self.page_id, action) {
            return Err(TransportError::Unauthorized {
                page_id: self.page_id.clone(),
            });
        }

        let last_seq = cursor.unwrap_or(0);
        match self.firehose.resume(&self.stream, &self.scope, last_seq) {
            Ok(_sub) => {
                let backfill = self.log.ops_since(last_seq);
                Ok(Connected::Resumed { backfill })
            }
            Err(e) if e.is_resync_required() => {
                let snapshot = self.snapshot.clone().unwrap_or(PageSnapshot {
                    snap_seq: 0,
                    blob_hash: String::new(),
                });
                let tail = self.log.ops_since(snapshot.snap_seq);
                Ok(Connected::ResyncFromSnapshot { snapshot, tail })
            }
            Err(_) => Ok(Connected::Resumed {
                backfill: self.log.ops_since(last_seq),
            }),
        }
    }

    pub fn send_op(&mut self, op: DocOp) -> SendOutcome {
        let outcome = self.log.persist(op);
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
        outcome
    }

    pub fn reconnect(
        &mut self,
        principal: &Principal,
        action: AuthAction,
        last_seq: u64,
    ) -> Result<Connected, TransportError> {
        self.connect(principal, action, Some(last_seq))
    }

    pub fn subscribe(
        &mut self,
        cursor: Option<u64>,
    ) -> Result<myelin_events::FirehoseSubscription, FirehoseError> {
        self.firehose.subscribe(&self.stream, &self.scope, cursor)
    }

    pub fn publish_presence(&mut self, presence: &Presence) {
        let presence_stream = format!("{}.presence", self.stream);
        self.firehose.publish(
            &presence_stream,
            &self.scope,
            FrameDraft::new(format!("{}|{}", presence.client_id, presence.awareness)),
        );
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
            PrincipalId("p-opaque".into()),
            PrincipalKind::Human,
            tenant(),
        )
    }

    fn open() -> CollabTransport<AllowAllAuthority> {
        CollabTransport::open_with_authority(tenant(), "page-1", AllowAllAuthority).expect("opens")
    }

    fn op(client: &str, lamport: u64, kind: OpKind) -> DocOp {
        DocOp::cas(
            OpId::new(client, lamport),
            "actor-1",
            kind,
            format!("cas:{client}:{lamport}").into_bytes(),
        )
    }

    #[test]
    fn op_seq_is_per_doc_monotonic() {
        let mut t = open();
        let a = t.send_op(op("c1", 1, OpKind::Insert));
        let b = t.send_op(op("c1", 2, OpKind::Insert));
        let c = t.send_op(op("c2", 1, OpKind::Insert));
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
    fn a_redelivered_op_is_an_idempotent_no_op() {
        let mut t = open();
        let first = t.send_op(op("c1", 7, OpKind::Insert));
        assert!(first.applied());
        assert_eq!(t.head_seq(), 1);

        let redelivered = t.send_op(op("c1", 7, OpKind::Insert));
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
            blob_hash: "blake3:snap".into(),
        });
        for i in 1..=6 {
            t.send_op(op("c1", i, OpKind::Insert));
        }

        let connected = t
            .connect(&principal(), AuthAction::Edit, Some(1))
            .expect("connect succeeds via the cold path");
        match connected {
            Connected::ResyncFromSnapshot { snapshot, tail } => {
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
    fn in_window_cursor_resumes_without_a_snapshot() {
        let mut t = CollabTransport::open_with_window(tenant(), "page-1", AllowAllAuthority, 3)
            .expect("opens");
        for i in 1..=6 {
            t.send_op(op("c1", i, OpKind::Insert));
        }
        let connected = t
            .connect(&principal(), AuthAction::Edit, Some(4))
            .expect("warm resume");
        match connected {
            Connected::Resumed { backfill } => {
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
    fn an_over_broad_scope_is_rejected_at_open() {
        for bad in ["*", "page*", "", "  "] {
            let r = CollabTransport::open(tenant(), bad);
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
            CollabTransport::open(tenant(), "page-abc-123").is_ok(),
            "a bounded page scope opens"
        );
    }

    #[test]
    fn connect_fail_closes_to_unauthorized_by_default() {
        let mut t = CollabTransport::open(tenant(), "page-1").expect("opens");
        let r = t.connect(&principal(), AuthAction::Edit, None);
        assert!(
            matches!(r, Err(TransportError::Unauthorized { .. })),
            "the fail-closed authority denies the connect (no op without authz)"
        );
    }

    #[test]
    fn an_authorized_connect_reaches_the_resume_path() {
        let mut t = open();
        t.send_op(op("c1", 1, OpKind::Insert));
        t.send_op(op("c1", 2, OpKind::Insert));
        let connected = t
            .connect(&principal(), AuthAction::Edit, None)
            .expect("authorized connect");
        match connected {
            Connected::Resumed { backfill } => {
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
    fn presence_is_ephemeral_and_never_persisted() {
        let mut t = open();
        t.send_op(op("c1", 1, OpKind::Insert));
        let head_before = t.head_seq();
        let ops_before = t.op_count();

        for i in 0..50 {
            t.publish_presence(&Presence::new("c1", format!("caret:{i}")));
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
        let sub = t.subscribe(None).expect("a live subscription opens");
        let sent = t.send_op(op("c2", 1, OpKind::Insert));
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
            log.persist(op("c1", i, OpKind::Insert));
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
        let redelivered_kept = log.persist(op("c1", 4, OpKind::Insert));
        assert!(
            matches!(redelivered_kept, SendOutcome::Duplicate(_)),
            "a retained op's op_id stays in the index → its re-delivery is an idempotent Duplicate"
        );
        assert_eq!(
            redelivered_kept.persisted().op_seq,
            4,
            "resolves to the kept op_seq (4)"
        );
        let redelivered_pruned = log.persist(op("c1", 2, OpKind::Insert));
        assert!(
            redelivered_pruned.applied(),
            "a pruned op's op_id left the index → its re-delivery is a fresh Apply (not a stale dup)"
        );
        let redelivered_at_watermark = log.persist(op("c1", 3, OpKind::Insert));
        assert!(
            redelivered_at_watermark.applied(),
            "the op AT the watermark left the index too (consistent prune) → a fresh Apply"
        );
        let next = log.persist(op("c1", 7, OpKind::Insert));
        assert_eq!(
            next.persisted().op_seq,
            9,
            "a fresh op continues head+1 after GC + the re-applies"
        );
        let mut log2 = log_seq(3);
        assert_eq!(log2.gc_below(0), 0, "gc_below(0) prunes nothing");
        assert_eq!(log2.len(), 3);
    }
}
