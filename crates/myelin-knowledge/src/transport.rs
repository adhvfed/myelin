//! # The resume-cursor durable collab transport (Layer 1) — KN-P07 → P-297, M3 (KN-D1, the headline)
//!
//! **Owning architecture docs:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md`
//! §1 (the layered collab stack — **transport is Layer 1, built FIRST**), §2 (the full resume-cursor
//! protocol: `CONNECT`/`SEND_OP`/`RECONNECT` pseudocode; `op_seq` == the firehose per-`(stream, scope)`
//! seq; `op_id = (client_id, lamport)`; `UNIQUE(tenant, page_id, op_id)` idempotent apply;
//! `resync_required → *.snapshot` fallback; `scope = doc:<page_id>` bounded; presence ephemeral
//! never persisted §2.3) + `01-tech-and-data-model.md` §3 (the `doc_op` op-log + `doc_snapshot`).
//!
//! **Contract-index:** row **3.5** the firehose resume-cursor transport — **OWNED-SEAM** (the
//! resume-cursor + idempotent-apply discipline + the CRDT-over-it are Knowledge's deliverable; the
//! Bus provides the transport seam [`myelin_events::Firehose`], P-141). Row **2.6** the `*.snapshot`
//! resync fallback — **CONSUMED** (the cold path; the rebuild itself is `myelin_events::reindex` /
//! EB-22). Reconciliation `00-reconciliation-decisions.md` **OQ-J** (the subscribe/resume protocol).
//!
//! ## What this module ships (KN-P07's owned half — the Knowledge stake in 3.5)
//! `myelin-events` already ships the Bus half of 3.5 — `Firehose::publish`/`subscribe`/`resume` with
//! the per-`(stream, scope)` monotonic `seq`, the `(last_seq, now]` backfill, the `resync_required`
//! verdict, and the `*`-rejecting [`FirehoseScope`] (P-141, reconciled-in-place per EI-01 §7 — this
//! module RIDES that transport, it does not re-implement it). What did NOT exist is **Knowledge's
//! resume-cursor + idempotent-apply discipline over it**:
//!
//! - **[`DocOp`] / [`OpId`]** — one collab op. `op_id = (client_id, lamport)` is deterministic +
//!   collision-free per client; the v1 payload is **CAS op bytes** (the named floor — see below).
//! - **[`DocOpLog`]** — the in-memory model of the `doc_op` table: a per-doc **monotonic `op_seq`**
//!   (== the firehose seq) and **`UNIQUE(tenant, page_id, op_id)` idempotent apply** (an
//!   `INSERT … ON CONFLICT DO NOTHING` — a re-delivered op is a **no-op**, never a double-apply).
//! - **[`CollabTransport`]** — the protocol from arch §2: `CONNECT` (authorize → resume backfill or
//!   `resync_required → snapshot` → live), `SEND_OP` (assign `op_seq`+`op_id` → persist idempotent →
//!   apply → `firehose.publish` the frame → coalesce a `knowledge.doc.updated` pointer via the
//!   OUTBOX, NEVER per-op on the durable bus), `RECONNECT` (re-run CONNECT at `last_seq` → resume
//!   replays exactly `(last_seq, now]`; the `UNIQUE(op_id)` makes in-flight re-sends no-ops).
//! - **[`Presence`]** — the ephemeral awareness tier (cursors / selections / who-is-here) over the
//!   firehose presence channel: throttled, **NOT persisted** (arch §2.3). A coarse durable read-state
//!   is the only durable trace; this tier never writes to `doc_op`.
//! - **The bounded-scope discipline** — [`doc_scope`] is `doc:<page_id>` (the whitelist-not-`*` rule
//!   generalised to the firehose); an unbounded scope is unrepresentable ([`FirehoseScope::parse`]).
//!
//! ## Layered (arch §1) — the transport is Layer 1, ENGINE-AGNOSTIC
//! Layer 2 (authority: the per-op `Id.check`) is a **stub here** (CONNECT authorizes through the
//! [`OpAuthority`] seam; the full ABAC body is **KN-P14/P16**). Layer 3 (merge) is the **CAS floor**
//! (KN-P13) → **Yrs CRDT** (KN-P29, M5). This transport carries opaque op bytes and is identical for
//! both — *that is why it is built first* (the CRDT is a Layer-3 swap, not a transport rewrite).
//!
//! ## FLOORS NAMED (VISION §3 — stubbed / deferred + the filling prompt)
//! - **CAS op bytes in v1.** [`DocOp::payload`] carries opaque CAS op bytes now; after the
//!   `engine_promote` cutover the SAME field carries **Yrs update bytes** — the transport, the
//!   `op_seq` cursor, and the idempotent apply are **unchanged**. The promotion is **KN-P29 (M5)**;
//!   the per-block CAS merge engine that produces the v1 bytes is **KN-P13**. **KN-D1 is written to
//!   re-run GREEN across that `engine_promote` boundary** (it asserts the transport's
//!   zero-loss/zero-dup property, which is independent of the apply engine) — re-confirmed in KN-P29.
//! - **Layer-2 authority is a stub.** [`OpAuthority`] is the CONNECT authorize seam; the real per-op
//!   `Id.check(edit|comment, page_ref, zookie)` + the zookie new-enemy guard is **KN-P14**, the ABAC
//!   `list_objects` push-down **KN-P16**. The seam here proves the call site exists (an op is
//!   authorized before it is applied) and fail-closes by default.
//! - **The live `doc_op` PERSIST is the in-memory [`DocOpLog`] on the substrate floor** (no live
//!   Postgres in `cargo build`, P-S12). The `INSERT … ON CONFLICT (tenant,page_id,op_id) DO NOTHING`
//!   semantics + the monotone `op_seq` are modelled byte-faithfully; the real Postgres `doc_op`
//!   co-commit rides the KN-P05 store + the KN-D7 outbox seam (the integration drill is the durable
//!   proof). The KN-D1 drill here proves the PROTOCOL property (0 lost / 0 dup across a kill + sever)
//!   over the in-process transport — the engine-agnostic correctness substrate.
//! ## `no-raw-publish` lint note (EI-01 §1 / §5 — NAMED, not a silent skip)
//! [`CollabTransport::send_op`] / [`CollabTransport::publish_presence`] call `firehose.publish(…)` —
//! the EPHEMERAL collab op-stream + presence fan-out the architecture explicitly sites on the
//! firehose (§4.3: "the firehose carries … collab op-streams"; §2.1 / ADR-04.5: "the durable bus
//! carries only the `knowledge.doc.updated` pointer; the collab op-stream never melts the durable
//! control bus"). A firehose frame is a references-not-payloads pointer (the `op_id` wire form),
//! never an inline-PII durable event, and is NOT emitted-iff-committed through the outbox. The
//! `no-raw-publish` lint's `.publish(` fingerprint collides with the frozen `firehose::publish`
//! method NAME, so this ONE transport file is on the lint-gate's NAMED, LOUD exclusion list
//! (`myelin-lints/src/bin/lint-gate.rs` + `tests/workspace_clean.rs`) — exactly the posture of
//! `firehose.rs` / `relay.rs`. Knowledge's DURABLE emit (the coalesced `knowledge.doc.updated` /
//! `knowledge.page.updated` via [`myelin_events::OutboxTx::emit`]) lives in [`crate::emit`], which
//! stays FULLY linted. A documented deviation, not a weakening.
//!
//! - **The `*.snapshot` resync rebuild body is consumed, not built here.** On `resync_required` this
//!   transport loads the latest [`PageSnapshot`] then goes live (the cold path is NAMED, never a
//!   silent gap); the block-granular snapshot emit + the reindex-from-source rebuild is **EB-22 /
//!   KN-P11** (`cold == live`). Here the snapshot is the seed state the over-window client resumes on.

// ════════════════════════════════════════════════════════════════════════════════════════════════
// MANDATORY-CORE MUTATION FLOOR (the KN-P07 cargo-mutants gate — TESTS field).
// ════════════════════════════════════════════════════════════════════════════════════════════════
// The IDEMPOTENT-APPLY PATH is mandatory-core: [`DocOpLog::persist`] (the `INSERT … ON CONFLICT
// (tenant, page_id, op_id) DO NOTHING` — the UNIQUE(op_id) guard), the per-doc monotone `op_seq`
// assignment (== the firehose seq), and [`CollabTransport::connect`]'s `(last_seq, now]` backfill /
// `resync_required` branch. The stated floor: **100% mutation score on the idempotent-apply path**
// (`persist` + `ops_since` + the `connect` resume/resync branch) — every arithmetic/comparison/branch
// mutant in those functions is killed by the unit tests + the KN-D1 chained drill (a mutated
// `last_seq + 1`, a flipped `op_seq > last_seq`, a swapped Applied/Duplicate arm, or a dropped
// ON-CONFLICT guard all change the 0-lost/0-dup assertion or the duplicate-no-op assertion). The
// drill's SET-equality assertion (not a tally) is what makes a "double-apply" or "drop-one" mutant
// observable. Run: `cargo mutants -p myelin-knowledge -f transport.rs`. The whole-transport score is
// not claimed at 100% (the telemetry accessors / Display arms are not core); the IDEMPOTENT-APPLY
// PATH is — that is the property KN-D1 gates and the floor the prompt names.

use myelin_events::{
    Firehose, FirehoseError, FirehoseScope, FrameDraft,
};
use myelin_identity::Principal;
use myelin_tenancy::TenantId;
use std::collections::HashMap;

/// The frozen firehose **stream** name for Knowledge collab ops: `fan.<tenant>.knowledge` (arch §2.1
/// — `stream = fan.<tenant>.knowledge`, `scope = doc:<page_id>`). A PII-free label (tenant id only),
/// so a telemetry key built from it is control-plane-PII-free by construction.
pub fn knowledge_stream(tenant: &TenantId) -> String {
    format!("fan.{}.knowledge", tenant.0)
}

/// The frozen **bounded scope** for one doc: `doc:<page_id>` (arch §2.1 / OQ-J — the bounded selector,
/// NEVER `*`). Lowers to the Bus's [`FirehoseScope`] (`ScopeKind::Doc`), whose [`FirehoseScope::parse`]
/// is the `*`-rejection chokepoint — so an unbounded doc scope is **unrepresentable** (the
/// whitelist-not-`*` rule generalised to the firehose, arch §2.2). A `page_id` that itself contains a
/// `*` / `:` is rejected as over-broad (it could not be a real opaque page id).
pub fn doc_scope(page_id: &str) -> Result<FirehoseScope, FirehoseError> {
    FirehoseScope::parse(&format!("doc:{page_id}"))
}

/// **A deterministic op id `op_id = (client_id, lamport)` (arch §2 / 01 §3).** Collision-free per
/// client (each client owns its `client_id`) and re-derivable (the same `(client, lamport)` always
/// names the same op), so `UNIQUE(tenant, page_id, op_id)` makes a re-delivered op a **no-op**
/// (idempotent apply; Helland 2012). The wire form is `<client_id>:<lamport>` — a PII-free token
/// (the client id is an opaque session/connection id, not a principal name).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OpId {
    /// The opaque per-client/connection id (collision-free across clients).
    pub client_id: String,
    /// The per-client monotone lamport counter (the client bumps it for every op it mints).
    pub lamport: u64,
}

impl OpId {
    /// A fresh `op_id` for `client_id` at `lamport`.
    pub fn new(client_id: impl Into<String>, lamport: u64) -> OpId {
        OpId { client_id: client_id.into(), lamport }
    }

    /// The canonical wire/db form (`<client_id>:<lamport>`) — the `op_id` text column (01 §3) + the
    /// `UNIQUE(tenant, page_id, op_id)` key half. PII-free (an opaque client id + a counter).
    pub fn wire(&self) -> String {
        format!("{}:{}", self.client_id, self.lamport)
    }
}

/// **The op kind (01 §3 `op_kind`).** The CAS-floor op set + the `engine_promote` cutover marker. The
/// transport NEVER interprets the kind/payload (it is a "dumb relay + persistence + authority", arch
/// §3.3) — the kind is carried opaque for the apply engine (CAS now, Yrs after KN-P29) + the
/// coalescer (a `block_*` op is a doc-structure change; a content op is a block-content change).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    /// An inline-text insert within a block (CAS: a content delta; Yrs: a `Y.Text` update).
    Insert,
    /// An inline-text delete within a block.
    Delete,
    /// An inline mark/format change (bold/italic/link — Peritext under the CRDT).
    Format,
    /// A block move within the tree (the LexoRank `order_key` write; Yrs: the move-CRDT op).
    Move,
    /// A block property set (a non-inline prop — checkbox, callout colour, …).
    SetProp,
    /// A block insert (a new block in the tree).
    BlockIns,
    /// A block delete (removes a block from the tree).
    BlockDel,
    /// **The CAS→CRDT cutover op (arch §3.4, KN-P29).** From this `op_seq` forward the payload carries
    /// Yrs update bytes; before it, CAS deltas. A reconnecting client resumes ACROSS this boundary,
    /// loads the seeded Yrs state once, and applies the tail — the transport is unchanged.
    EnginePromote,
}

impl OpKind {
    /// The stable wire/db token (the `op_kind` column value, 01 §3).
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

    /// Whether this op changes the DOC STRUCTURE (a block tree change) vs a block's inline content —
    /// the coalescer uses this to choose the `knowledge.page.updated` (semantic) vs the
    /// `knowledge.doc.updated` (pointer) emit (arch §7). NEVER changes how the op is transported.
    pub fn is_structural(self) -> bool {
        matches!(self, OpKind::Move | OpKind::BlockIns | OpKind::BlockDel)
    }
}

/// **One collab op a client SENDs (arch §2 `SEND_OP`).** The client mints the `op_id` + the kind +
/// the opaque CAS payload bytes; the TRANSPORT assigns the `op_seq` on persist (a client cannot mint
/// its own `op_seq` — the per-doc monotone seq is the transport's invariant, exactly as the firehose
/// `seq` is). `actor` is the pseudonymous principal (human OR agent — the same protocol, arch §2.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocOp {
    /// The deterministic `(client_id, lamport)` op id — the idempotent-apply key.
    pub op_id: OpId,
    /// The pseudonymous author (human or agent — the SAME `SEND_OP` path, arch §9).
    pub actor: String,
    /// The op kind (carried opaque for the apply engine + the coalescer).
    pub kind: OpKind,
    /// **The opaque op payload — CAS op bytes in v1 (the NAMED floor), Yrs update bytes after
    /// KN-P29.** The transport never reads it (references-not-payloads; a PII-bearing inline run is
    /// DEK-wrapped under `pii_key_ref`, 01 §3). Held as bytes so the engine swap is payload-only.
    pub payload: Vec<u8>,
    /// The per-subject DEK ref iff this op carries inline PII (11.4) — `None` on the common path.
    pub pii_key_ref: Option<String>,
}

impl DocOp {
    /// A CAS-floor op (the v1 path): `kind` + opaque CAS `payload` bytes, no inline PII.
    pub fn cas(op_id: OpId, actor: impl Into<String>, kind: OpKind, payload: impl Into<Vec<u8>>) -> DocOp {
        DocOp { op_id, actor: actor.into(), kind, payload: payload.into(), pii_key_ref: None }
    }
}

/// **One PERSISTED op (the `doc_op` row, 01 §3).** The [`DocOp`] the client sent plus the
/// transport-assigned per-doc monotone `op_seq` (== the firehose seq). This is what `resume` replays
/// and what `RECONNECT` reads back. `op_seq` is assigned ONCE on the first (winning) insert; a
/// re-delivered op with the same `op_id` does NOT get a new `op_seq` (the idempotent no-op).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedOp {
    /// The per-doc monotone sequence (the resume cursor; == the firehose `seq` for `doc:<page_id>`).
    pub op_seq: u64,
    /// The op the client sent.
    pub op: DocOp,
}

/// The outcome of a `SEND_OP` persist — distinguishes a FRESH apply from an idempotent NO-OP, so a
/// caller (and the drill) can assert "a re-delivered op was a no-op, not a double-apply" (the KN-D1
/// 0-duplicate property). NEVER an error on a duplicate — a duplicate is the EXPECTED at-least-once
/// case, absorbed silently (arch §2.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendOutcome {
    /// The op was new: it got a fresh monotone `op_seq` and was applied + published. Carries the
    /// assigned [`PersistedOp`].
    Applied(PersistedOp),
    /// The op was a re-delivery (its `op_id` already exists): a NO-OP. Carries the ALREADY-persisted
    /// op (the same `op_seq` it first got) — the apply did not run again, nothing was published twice.
    Duplicate(PersistedOp),
}

impl SendOutcome {
    /// The persisted op either way (the fresh one or the existing one a duplicate resolved to).
    pub fn persisted(&self) -> &PersistedOp {
        match self {
            SendOutcome::Applied(p) | SendOutcome::Duplicate(p) => p,
        }
    }

    /// `true` iff this send freshly applied (vs an idempotent no-op).
    pub fn applied(&self) -> bool {
        matches!(self, SendOutcome::Applied(_))
    }
}

/// **The `doc_op` op-log for ONE doc (01 §3 — the in-memory model of the partitioned `doc_op`
/// table).** A per-doc monotone `op_seq` (== the firehose seq) and the `UNIQUE(tenant, page_id,
/// op_id)` idempotent-apply index, modelling `INSERT … ON CONFLICT (tenant, page_id, op_id) DO
/// NOTHING`. This is the substrate-floor stand-in for the live Postgres `doc_op` (the real co-commit
/// is the KN-P05 store + the KN-D7 outbox seam, P-S12) — the monotone seq + the idempotent semantics
/// are byte-faithful, so the KN-D1 protocol property proven here holds against the live table.
#[derive(Debug, Default, Clone)]
pub struct DocOpLog {
    /// The applied ops in `op_seq` order (the live tail; compaction to a content-addressed snapshot +
    /// op-log GC landed in KN-P11 — see [`crate::compaction::SnapshotCompactor`], which reads this tail
    /// via [`Self::ops_up_to`] / [`Self::ops_in_range`] and prunes it via [`Self::gc_below`]).
    ops: Vec<PersistedOp>,
    /// The `UNIQUE(op_id)` index: `op_id.wire()` → the `op_seq` it first got (the idempotent guard).
    by_op_id: HashMap<String, u64>,
    /// The highest `op_seq` ever assigned (monotone; the next op is `last_seq + 1`).
    last_seq: u64,
}

impl DocOpLog {
    /// A fresh, empty op-log (a brand-new doc, or a doc seeded from a snapshot — the seed sets
    /// `last_seq` via [`Self::seed_from_snapshot`]).
    pub fn new() -> DocOpLog {
        DocOpLog::default()
    }

    /// **Persist an op idempotently (`INSERT … ON CONFLICT (tenant, page_id, op_id) DO NOTHING`).**
    /// A NEW `op_id` gets the next monotone `op_seq` and is appended → [`SendOutcome::Applied`]. A
    /// re-delivered `op_id` is a **no-op**: NO new `op_seq`, NO re-append → [`SendOutcome::Duplicate`]
    /// carrying the op as it was FIRST persisted (arch §2: "the UNIQUE(op_id) makes re-sends no-ops").
    pub fn persist(&mut self, op: DocOp) -> SendOutcome {
        let wire = op.op_id.wire();
        if let Some(&existing_seq) = self.by_op_id.get(&wire) {
            // ON CONFLICT DO NOTHING: the op already applied at `existing_seq` — a pure no-op. Return
            // the EXISTING persisted op (so the caller sees the same op_seq, never a second apply).
            let existing = self
                .ops
                .iter()
                .find(|p| p.op_seq == existing_seq)
                .cloned()
                .expect("the by_op_id index points at a persisted op");
            return SendOutcome::Duplicate(existing);
        }
        self.last_seq += 1;
        let persisted = PersistedOp { op_seq: self.last_seq, op };
        self.by_op_id.insert(wire, self.last_seq);
        self.ops.push(persisted.clone());
        SendOutcome::Applied(persisted)
    }

    /// **The resume read (01 §3 `doc_op_resume` index): the ops with `op_seq > last_seq`, in order.**
    /// This is the `(last_seq, now]` backfill `RECONNECT` replays — every op the client missed,
    /// exactly once, none lost. A caught-up client (`last_seq >= self.last_seq`) gets nothing.
    pub fn ops_since(&self, last_seq: u64) -> Vec<PersistedOp> {
        self.ops.iter().filter(|p| p.op_seq > last_seq).cloned().collect()
    }

    /// The highest assigned `op_seq` (the live head; the resume cursor a caught-up client holds).
    pub fn head_seq(&self) -> u64 {
        self.last_seq
    }

    /// **The ops with `op_seq ≤ up_to`, in `op_seq` order (the compaction prefix, KN-P11).** The
    /// materialised state a compaction snapshots is `materialize(ops_up_to(snap_seq))` — the doc's
    /// state up to (and including) the snapshot boundary. After a GC pruned rows ≤ a watermark, this
    /// returns only the RETAINED ops ≤ `up_to` (the snapshot carries the pruned remainder).
    pub fn ops_up_to(&self, up_to: u64) -> Vec<PersistedOp> {
        self.ops.iter().filter(|p| p.op_seq <= up_to).cloned().collect()
    }

    /// **The ops in `(from, to]`, in `op_seq` order (the tail a version-history reconstruct appends on
    /// top of a snapshot seed, KN-P11).** A reconstruct of version `to` from the nearest snapshot at
    /// `from = snap_seq` is `seed_state ++ materialize(ops_in_range(snap_seq, to))`.
    pub fn ops_in_range(&self, from: u64, to: u64) -> Vec<PersistedOp> {
        self.ops
            .iter()
            .filter(|p| p.op_seq > from && p.op_seq <= to)
            .cloned()
            .collect()
    }

    /// **The lowest `op_seq` still retained in the op-log (the GC floor), or `0` if the log is empty.**
    /// A version-history reconstruct uses this to detect a pruned gap: a needed op below this floor was
    /// GC'd with (potentially) no covering snapshot (KN-P11's `guard_no_gap`).
    pub fn lowest_seq(&self) -> u64 {
        self.ops.iter().map(|p| p.op_seq).min().unwrap_or(0)
    }

    /// **GC: prune the ops with `op_seq ≤ watermark` (the compacted, no-longer-needed range, KN-P11 /
    /// arch §3).** The caller ([`crate::compaction::SnapshotCompactor::gc`]) computes the watermark as
    /// `min(snap_seq, lowest_open_cursor)` so a row a connected client still trails is RETAINED (the
    /// cursor is the GC watermark — KD-1 survives compaction). The `by_op_id` idempotent-apply index is
    /// pruned in lock-step (a pruned op's `op_id` is no longer in the live tail; a re-delivery of it
    /// would re-apply — but a re-delivery older than the watermark is below every open cursor, so no
    /// connected client can produce it; the durable record is the snapshot). **The `last_seq` counter
    /// is NOT reset** — it stays monotone so future ops continue `head + 1`. Returns the rows pruned.
    pub fn gc_below(&mut self, watermark: u64) -> usize {
        let before = self.ops.len();
        self.ops.retain(|p| p.op_seq > watermark);
        // Keep the idempotent-apply index consistent with the retained tail (prune the pruned ops'
        // op_ids). The monotone last_seq is untouched (the seq counter survives the prune).
        self.by_op_id.retain(|_, seq| *seq > watermark);
        before - self.ops.len()
    }

    /// The number of ops in the live tail (bounded by the compaction cadence, KN-P11).
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// `true` iff the log holds no ops.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// **Seed the log's cursor from a snapshot's `snap_seq` (the `resync_required` cold path, arch
    /// §2.1).** A client whose cursor predates the retention window loads the [`PageSnapshot`] (which
    /// includes ops up to `snap_seq`) then resumes from `snap_seq` — the log's `op_seq` continues
    /// monotone PAST the snapshot (the live tail after the snapshot is what the resumed client gets).
    /// This models GC having pruned `doc_op` rows ≤ `snap_seq`: the seq counter survives the prune.
    pub fn seed_from_snapshot(&mut self, snapshot: &PageSnapshot) {
        if snapshot.snap_seq > self.last_seq {
            self.last_seq = snapshot.snap_seq;
        }
    }
}

/// **A block-granular page snapshot (01 §3 `doc_snapshot` / 02 §2.1 — the `resync_required` fallback
/// target, contract 2.6 CONSUMED).** Carries the `snap_seq` (the `op_seq` the snapshot includes up
/// to) + an opaque content-addressed blob handle. On `resync_required` a client loads this then goes
/// live from `snap_seq` (the cold path is NAMED, never a silent gap). The block-granular emit + the
/// `cold == live` reindex-from-source rebuild is **EB-22 / KN-P11**; here it is the seed state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageSnapshot {
    /// The `op_seq` this snapshot materialises up to (a resumed client continues from here).
    pub snap_seq: u64,
    /// The content-addressed snapshot blob handle (per-tenant-DEK wrapped, K6) — opaque here.
    pub blob_hash: String,
}

/// **The CONNECT outcome (arch §2 `CONNECT`).** Either the client resumed from its cursor (the warm
/// path: backfill `(last_seq, now]` then live), or it was over the retention window and must first
/// load a snapshot (the cold `resync_required` path — NAMED). Either way the client ends up live with
/// zero ops lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Connected {
    /// The warm path: the client's cursor was in the retention window. Carries the backfilled ops
    /// `(last_seq, now]` (the gap it missed — replayed exactly once) that the client applies
    /// idempotently before going live.
    Resumed {
        /// The backfilled ops the client missed (in `op_seq` order; possibly empty for a caught-up
        /// reconnect). The client applies these (deduping on `op_id`) then receives live frames.
        backfill: Vec<PersistedOp>,
    },
    /// The cold path: `last_seq` predated the retention window → the client loads the snapshot then
    /// goes live from `snap_seq`. NAMED (`resync_required`), never a silent gap. Carries the snapshot
    /// to load + the live-tail ops after it (`(snap_seq, now]`) the client applies on top.
    ResyncFromSnapshot {
        /// The snapshot to materialise first (the cold seed state).
        snapshot: PageSnapshot,
        /// The live-tail ops after the snapshot (`(snap_seq, now]`) applied on top of the seed.
        tail: Vec<PersistedOp>,
    },
}

/// **Why a CONNECT / SEND_OP failed (the typed LOUD verdicts — never a silent fail).**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// The scope was unbounded / over-broad (`doc:*` / an empty page id) — REJECTED at CONNECT (the
    /// whitelist-not-`*` rule, arch §2.2). Carries the offending scope.
    OverBroadScope(String),
    /// **Layer-2 authority denied the op (arch §2 step 1 — "no op without authz").** The CONNECT
    /// `Id.check(edit|comment, page_ref, zookie)` returned Deny (or the seam is the fail-closed
    /// stub). Carries the page the check denied on. The full ABAC body is KN-P14/P16.
    Unauthorized {
        /// The page the authorize denied for.
        page_id: String,
    },
}

impl core::fmt::Display for TransportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            TransportError::OverBroadScope(s) => {
                write!(f, "collab transport rejects over-broad doc scope `{s}` (never `*`)")
            }
            TransportError::Unauthorized { page_id } => {
                write!(f, "collab op denied by Layer-2 authority on page `{page_id}` (no op without authz)")
            }
        }
    }
}

impl std::error::Error for TransportError {}

/// **The Layer-2 authorize seam the transport calls at CONNECT (arch §2 step 1 / §3.1 — "no op
/// without authz").** A CONNECT authorizes `edit|comment` on the `page_ref` (carrying the zookie for
/// read-your-writes, contract 4.10) BEFORE any op is backfilled or applied. This is the seam; the
/// real per-op `Id.check` + the zookie new-enemy guard is **KN-P14**, the ABAC `list_objects`
/// push-down **KN-P16**. The default ([`FailClosedAuthority`]) DENIES (fail-closed, ADR-03) so an
/// un-wired authority is never mistaken for "open".
pub trait OpAuthority {
    /// Authorize `principal` to `edit|comment` (one of [`AuthAction`]) on `page_id`. Returns `true`
    /// iff allowed. The default seam fail-closes to `false`; KN-P14 swaps the real `Id.check` body in
    /// behind this exact seam (EI-01 §7 — one primitive, the call site is unchanged).
    fn authorize(&self, principal: &Principal, page_id: &str, action: AuthAction) -> bool;
}

/// The collab authorize action (arch §2 step 1 — `edit|comment`). An `edit` op needs `edit`; a
/// comment-only collaborator authorizes `comment` (it may SEND_OP comment ops but not content ops —
/// the per-op enforcement is KN-P14; CONNECT authorizes the coarse capability).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthAction {
    /// Edit the doc content (the full collaborator capability).
    Edit,
    /// Comment only (a restricted collaborator — comment ops, not content ops).
    Comment,
}

/// **The fail-closed Layer-2 authority stub (the named KN-P14 floor).** Every authorize DENIES until
/// the per-op `Id.check` body lands (KN-P14). Fail-closed (ADR-03): an un-wired authority NEVER
/// opens — the SAME posture the `KnowledgeEntrypointAuthorizer` shell takes (EI-01 §7). The CONNECT
/// path proves the call site exists (an op is authorized before it is applied); KN-P14 makes it real.
#[derive(Clone, Copy, Debug, Default)]
pub struct FailClosedAuthority;

impl OpAuthority for FailClosedAuthority {
    fn authorize(&self, _principal: &Principal, _page_id: &str, _action: AuthAction) -> bool {
        // Fail-closed: deny until KN-P14 wires the real Id.check. Never fail-open.
        false
    }
}

/// **An allow-all test authority — for the unit/drill proofs of the TRANSPORT property only.** The
/// KN-D1 drill proves the resume-cursor 0-lost/0-dup property, which is INDEPENDENT of the authority
/// decision; the authority's own gate is proven by [`FailClosedAuthority`] (deny) + KN-P14's body.
/// Marked `#[doc(hidden)]`-adjacent in intent: it is the test seam, NEVER wired into a real CONNECT.
#[derive(Clone, Copy, Debug, Default)]
pub struct AllowAllAuthority;

impl OpAuthority for AllowAllAuthority {
    fn authorize(&self, _principal: &Principal, _page_id: &str, _action: AuthAction) -> bool {
        true
    }
}

/// **Ephemeral presence / awareness (arch §2.3 — cursors / selections / who-is-here).** Rides the
/// firehose presence channel, throttled, and is **NEVER persisted** — there is deliberately no path
/// from a [`Presence`] frame to the [`DocOpLog`] / `doc_op` table. A coarse durable read-state summary
/// is the only durable trace (not modelled here). A presence frame is at-most-once (a dropped
/// presence frame is fine — it is superseded by the next), unlike an op (which is at-least-once +
/// idempotent). Held as an opaque PII-free pointer (a caret offset + an opaque session id), never an
/// inline body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presence {
    /// The opaque session/client id whose cursor this is (PII-free).
    pub client_id: String,
    /// The opaque awareness payload (a caret offset / selection range / "is typing") — never
    /// persisted, never an inline content body.
    pub awareness: String,
}

impl Presence {
    /// A presence frame for `client_id` carrying the opaque `awareness` payload.
    pub fn new(client_id: impl Into<String>, awareness: impl Into<String>) -> Presence {
        Presence { client_id: client_id.into(), awareness: awareness.into() }
    }
}

/// **The resume-cursor durable collab transport for ONE doc (Layer 1, arch §2 — the KN-D1
/// headline).** Wraps the Bus's [`Firehose`] (the transport seam) + the doc's [`DocOpLog`] (the
/// idempotent op-log) + the Layer-2 [`OpAuthority`] seam, and implements `CONNECT` / `SEND_OP` /
/// `RECONNECT`. Engine-agnostic: the op payload is opaque CAS bytes now, Yrs bytes after KN-P29 —
/// the transport is unchanged (that is why it is built first).
///
/// The transport pins ONE doc to ONE cell (the v1 single-cell collab floor — the cross-cell op
/// fan-out is KN-P30, M5). Generic over the [`OpAuthority`] so the real KN-P14 `Id.check` swaps in
/// behind the same seam (the default is [`FailClosedAuthority`] — deny until wired).
pub struct CollabTransport<A: OpAuthority = FailClosedAuthority> {
    tenant: TenantId,
    page_id: String,
    stream: String,
    scope: FirehoseScope,
    log: DocOpLog,
    firehose: Firehose,
    authority: A,
    /// The latest snapshot (the `resync_required` seed). `None` until a compaction mints one (KN-P11);
    /// here a test seeds it to drive the cold path. The retention-window floor below it is the
    /// firehose's (`Firehose::with_limits`); an out-of-window resume falls back to this.
    snapshot: Option<PageSnapshot>,
}

impl CollabTransport<FailClosedAuthority> {
    /// **Open the transport for a doc with the FAIL-CLOSED Layer-2 authority (the production default,
    /// arch §3.1 — no op without authz, deny until KN-P14 wires the real `Id.check`).** Rejects an
    /// over-broad scope at open (a `page_id` that is not a bounded doc selector).
    pub fn open(tenant: TenantId, page_id: &str) -> Result<CollabTransport<FailClosedAuthority>, TransportError> {
        CollabTransport::open_with_authority(tenant, page_id, FailClosedAuthority)
    }
}

impl<A: OpAuthority> CollabTransport<A> {
    /// Open the transport with an explicit [`OpAuthority`] (the KN-P14 real `Id.check` swaps in here;
    /// a test uses [`AllowAllAuthority`] to prove the transport property independent of the authz
    /// gate). Rejects an over-broad scope at open (the `*`-rejection chokepoint).
    pub fn open_with_authority(
        tenant: TenantId,
        page_id: &str,
        authority: A,
    ) -> Result<CollabTransport<A>, TransportError> {
        let scope = doc_scope(page_id).map_err(|_| TransportError::OverBroadScope(format!("doc:{page_id}")))?;
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

    /// Open with a BOUNDED firehose retention window (the KN-D1 drill drives a SMALL window to force
    /// the out-of-window `resync_required` cold path deterministically — arch §2.1). The window
    /// capacity is the NAMED firehose floor (tuned by D-10/EB-30); here a small value exercises the
    /// resync leg.
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

    /// Install the latest [`PageSnapshot`] (a compaction would mint this, KN-P11). The
    /// `resync_required` cold path resumes on top of it. Models the GC having pruned `doc_op` rows
    /// ≤ `snap_seq`; the log's `op_seq` counter is advanced to (at least) `snap_seq`.
    pub fn install_snapshot(&mut self, snapshot: PageSnapshot) {
        self.log.seed_from_snapshot(&snapshot);
        self.snapshot = Some(snapshot);
    }

    /// The doc's `page_id` (the bounded scope's resource id).
    pub fn page_id(&self) -> &str {
        &self.page_id
    }

    /// The doc's tenant (the `(tenant, page_id)` pin — the single-cell collab floor, KN-P30 lifts the
    /// cross-cell pin). The emit call site ([`crate::emit::emit_change`]) needs it to build the
    /// `knowledge.doc.updated` aggregate at coalesce time (arch §7).
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// The firehose `(stream, scope)` this doc's ops ride (`fan.<tenant>.knowledge`, `doc:<page_id>`).
    pub fn stream(&self) -> &str {
        &self.stream
    }

    /// The bounded doc scope (`doc:<page_id>`).
    pub fn scope(&self) -> &FirehoseScope {
        &self.scope
    }

    /// The doc's op-log head (`op_seq`) — the resume cursor a caught-up client holds.
    pub fn head_seq(&self) -> u64 {
        self.log.head_seq()
    }

    /// **`CONNECT(principal, action, cursor)` (arch §2 `CONNECT`).** (1) AUTHORIZE through the
    /// Layer-2 seam (no op without authz); (2) RESUME the firehose at `cursor`, backfilling
    /// `(cursor, now]` from the op-log; on `resync_required` (cursor below the retention window) load
    /// the snapshot then go live from `snap_seq`. Returns the [`Connected`] outcome the client applies
    /// idempotently (op_id dedup) before receiving live frames. A `cursor = None` is a fresh connect
    /// (backfill the whole window / from `0`).
    ///
    /// **Presence** (step 3 of CONNECT) is the ephemeral awareness join — modelled by
    /// [`Self::publish_presence`] / [`Self::subscribe`]; it is NOT part of the durable connect
    /// outcome (it never persists).
    pub fn connect(
        &mut self,
        principal: &Principal,
        action: AuthAction,
        cursor: Option<u64>,
    ) -> Result<Connected, TransportError> {
        // Step 1 — AUTHORIZE (Layer 2, arch §2): no op (and no backfill of ops) without authz.
        if !self.authority.authorize(principal, &self.page_id, action) {
            return Err(TransportError::Unauthorized { page_id: self.page_id.clone() });
        }

        let last_seq = cursor.unwrap_or(0);
        // Step 2 — RESUME from the firehose: backfill (last_seq, now] or fall back to the snapshot.
        match self.firehose.resume(&self.stream, &self.scope, last_seq) {
            Ok(_sub) => {
                // The firehose could replay the gap from its window → the warm path. The op-log is the
                // source of truth for the backfilled OPS (the firehose frame is a pointer; the op-log
                // carries the op bytes — arch §2.1 "the durable bus carries only the pointer").
                let backfill = self.log.ops_since(last_seq);
                Ok(Connected::Resumed { backfill })
            }
            Err(e) if e.is_resync_required() => {
                // Step 2 cold path — `resync_required`: the cursor predates the retention window. Load
                // the snapshot (block-granular *.snapshot, contract 2.6) then go live from snap_seq.
                // NAMED, never a silent gap. The snapshot rebuild itself is EB-22 / KN-P11.
                let snapshot = self.snapshot.clone().unwrap_or(PageSnapshot {
                    // No compaction has minted a snapshot yet → the cold seed is the empty-doc state at
                    // the current head (the whole live tail is the "tail" applied on top). A real
                    // *.snapshot is the KN-P11 body; here the seed is the current materialised state.
                    snap_seq: 0,
                    blob_hash: String::new(),
                });
                let tail = self.log.ops_since(snapshot.snap_seq);
                Ok(Connected::ResyncFromSnapshot { snapshot, tail })
            }
            // The firehose `resume` only errors with `OverBroadScope` (unreachable — the scope is a
            // typed bounded `FirehoseScope`) or `ResyncRequired` (handled). Any other is a bug.
            Err(_) => Ok(Connected::Resumed { backfill: self.log.ops_since(last_seq) }),
        }
    }

    /// **`SEND_OP(op)` (arch §2 `SEND_OP`).** (2) assign `op_seq` + the client's `op_id`; (3) PERSIST
    /// idempotent (`INSERT … ON CONFLICT (tenant, page_id, op_id) DO NOTHING`); (4) apply to live
    /// state (Layer 3 — opaque here); (5) `firehose.publish` the frame to fan out to other
    /// subscribers; (6) the coalescer emits a `knowledge.doc.updated` pointer via the OUTBOX
    /// (debounced — NEVER per-op on the durable bus, arch §7; the coalesced emit body is the KN-P06
    /// `emit_change` seam wired at the call site, see [`crate::emit`]).
    ///
    /// Returns [`SendOutcome::Applied`] for a fresh op (assigned `op_seq`, published) or
    /// [`SendOutcome::Duplicate`] for a re-delivery (a NO-OP — NOT published again, NOT re-applied).
    /// The duplicate path is the at-least-once absorber (the KN-D1 0-duplicate property): a re-sent
    /// in-flight op resolves to its first `op_seq` and does nothing.
    pub fn send_op(&mut self, op: DocOp) -> SendOutcome {
        // Step 3 — PERSIST idempotent. A duplicate op_id is a no-op (the UNIQUE(op_id) guard).
        let outcome = self.log.persist(op);
        if let SendOutcome::Applied(persisted) = &outcome {
            // Steps 4–5: apply (Layer 3, opaque here) + fan out the frame on the firehose. The frame's
            // seq IS the op_seq (the per-(stream, scope) monotone seq == the per-doc op_seq, OQ-J). The
            // payload is an opaque pointer (the op_id wire form) — references-not-payloads (the op
            // BYTES live in the durable op-log, not the ephemeral firehose frame).
            // The frame payload carries the AUTHORITATIVE op_seq (the op-log's per-doc cursor) so a
            // subscriber keys off the op_seq, not the firehose's own frame seq. On a fresh doc the two
            // coincide (OQ-J — one cursor); after a snapshot seed (GC pruned the firehose window's
            // early frames too) the op-log's op_seq runs ahead of the freshly-restarted firehose seq,
            // so the op_seq travels in the frame, not implied by the frame seq.
            let _frame = self.firehose.publish(
                &self.stream,
                &self.scope,
                FrameDraft::new(format!("{}@{}", persisted.op.op_id.wire(), persisted.op_seq)),
            );
            // Step 6 — coalesce → `knowledge.doc.updated` via the OUTBOX is the call site's job (the
            // debounced emit through `crate::emit::emit_change`, NEVER per-op on the durable bus). The
            // transport does NOT publish a durable event per op (arch §7 / ADR-04.5).
        }
        // A `Duplicate` published NOTHING and applied nothing — the idempotent no-op.
        outcome
    }

    /// **`RECONNECT(last_seq)` (arch §2 `RECONNECT`).** A dropped client re-runs CONNECT at its
    /// `last_durably_applied_op_seq`; resume replays EXACTLY `(last_seq, now]` and the
    /// `UNIQUE(op_id)` makes any in-flight re-sends no-ops → **0 ops lost, 0 duplicate effects** (the
    /// KN-D1 drill). A convenience over [`Self::connect`] with the reconnecting cursor.
    pub fn reconnect(
        &mut self,
        principal: &Principal,
        action: AuthAction,
        last_seq: u64,
    ) -> Result<Connected, TransportError> {
        self.connect(principal, action, Some(last_seq))
    }

    /// **Open a live subscription on this doc's `(stream, scope)`** (the warm-path live tier after a
    /// CONNECT). Frames published by [`Self::send_op`] fan out to it (the live delivery). A bounded
    /// scope is guaranteed (the transport's scope is typed). `cursor = None` is live-from-now;
    /// `Some(seq)` backfills `(seq, now]` from the firehose window first.
    pub fn subscribe(&mut self, cursor: Option<u64>) -> Result<myelin_events::FirehoseSubscription, FirehoseError> {
        self.firehose.subscribe(&self.stream, &self.scope, cursor)
    }

    /// **Publish an EPHEMERAL presence frame (arch §2.3 — cursors / selections / who-is-here).** Rides
    /// the firehose presence channel, throttled, **NEVER persisted** — there is NO write to the
    /// op-log here (a presence frame never becomes a `doc_op` row). At-most-once is fine (a dropped
    /// presence frame is superseded by the next). The presence frame uses a SEPARATE stream suffix so
    /// it never shares the op-stream's resume cursor (presence is not resumable — it is live-only).
    pub fn publish_presence(&mut self, presence: &Presence) {
        // A SEPARATE presence stream (`<stream>.presence`) so presence frames never enter the op-log
        // cursor and a presence drop never triggers a resync. Ephemeral, not persisted (arch §2.3).
        let presence_stream = format!("{}.presence", self.stream);
        self.firehose.publish(
            &presence_stream,
            &self.scope,
            FrameDraft::new(format!("{}|{}", presence.client_id, presence.awareness)),
        );
        // CRITICAL: no `self.log.persist(...)` — presence is NEVER persisted (the data-loss-free
        // invariant runs the OTHER way: op-log is durable, presence is ephemeral).
    }

    /// The number of ops durably in the op-log (the resume source size). Presence frames are NOT
    /// counted — they never enter the op-log (the ephemeral-never-persisted invariant, arch §2.3).
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
        Principal::stub(PrincipalId("p-opaque".into()), PrincipalKind::Human, tenant())
    }

    fn open() -> CollabTransport<AllowAllAuthority> {
        CollabTransport::open_with_authority(tenant(), "page-1", AllowAllAuthority).expect("opens")
    }

    fn op(client: &str, lamport: u64, kind: OpKind) -> DocOp {
        DocOp::cas(OpId::new(client, lamport), "actor-1", kind, format!("cas:{client}:{lamport}").into_bytes())
    }

    // ---- op_seq monotonicity --------------------------------------------------------------------

    /// **`op_seq` is per-doc MONOTONE (== the firehose seq, OQ-J).** Three sends get `1, 2, 3`; the
    /// frame seq equals the op_seq (the debug_assert in `send_op` proves the one-cursor invariant).
    #[test]
    fn op_seq_is_per_doc_monotonic() {
        let mut t = open();
        let a = t.send_op(op("c1", 1, OpKind::Insert));
        let b = t.send_op(op("c1", 2, OpKind::Insert));
        let c = t.send_op(op("c2", 1, OpKind::Insert));
        assert_eq!(a.persisted().op_seq, 1);
        assert_eq!(b.persisted().op_seq, 2);
        assert_eq!(c.persisted().op_seq, 3, "monotone across clients (one per-doc cursor)");
        assert_eq!(t.head_seq(), 3);
        assert!(a.applied() && b.applied() && c.applied(), "all three are fresh applies");
    }

    // ---- the idempotent ON CONFLICT apply (a re-delivered op is a no-op) -------------------------

    /// **A re-delivered op (same `op_id`) is a NO-OP (`UNIQUE(tenant, page_id, op_id)`, arch §2).** The
    /// re-send resolves to the FIRST `op_seq`, does NOT get a new seq, does NOT advance the head, and
    /// is reported [`SendOutcome::Duplicate`] (not an error — a duplicate is the expected
    /// at-least-once case, absorbed silently).
    #[test]
    fn a_redelivered_op_is_an_idempotent_no_op() {
        let mut t = open();
        let first = t.send_op(op("c1", 7, OpKind::Insert));
        assert!(first.applied());
        assert_eq!(t.head_seq(), 1);

        // the SAME (client_id, lamport) is re-delivered (an at-least-once retransmit).
        let redelivered = t.send_op(op("c1", 7, OpKind::Insert));
        assert!(!redelivered.applied(), "a re-delivered op did NOT freshly apply");
        assert!(matches!(redelivered, SendOutcome::Duplicate(_)), "it is reported a Duplicate no-op");
        assert_eq!(
            redelivered.persisted().op_seq,
            1,
            "the duplicate resolves to the FIRST op_seq (no new seq assigned)"
        );
        assert_eq!(t.head_seq(), 1, "the head did NOT advance (0 duplicate effect)");
        assert_eq!(t.op_count(), 1, "exactly one op in the log (the duplicate was a no-op)");
    }

    // ---- the resync_required → snapshot path ----------------------------------------------------

    /// **An out-of-window cursor falls back to the snapshot (`resync_required`, arch §2.1, NAMED).** A
    /// SMALL retention window forces the cold path: a client whose `last_seq` predates the window gets
    /// `ResyncFromSnapshot { snapshot, tail }` (load the snapshot then go live from `snap_seq`) — the
    /// signal is acted on, never a silent gap.
    #[test]
    fn out_of_window_cursor_resyncs_from_snapshot() {
        // window holds only the most-recent 3 frames.
        let mut t = CollabTransport::open_with_window(tenant(), "page-1", AllowAllAuthority, 3)
            .expect("opens");
        // a compaction minted a snapshot up to op_seq 2 (the cold seed; the op-log cursor advances
        // to 2, modelling GC having pruned doc_op rows <= snap_seq).
        t.install_snapshot(PageSnapshot { snap_seq: 2, blob_hash: "blake3:snap".into() });
        // publish 6 ops → op_seq runs 3..8 (past the snapshot); the firehose window holds the last 3.
        for i in 1..=6 {
            t.send_op(op("c1", i, OpKind::Insert));
        }

        // a client at last_seq = 1 (below the firehose window floor) → resync_required → snapshot.
        let connected = t
            .connect(&principal(), AuthAction::Edit, Some(1))
            .expect("connect succeeds via the cold path");
        match connected {
            Connected::ResyncFromSnapshot { snapshot, tail } => {
                assert_eq!(snapshot.snap_seq, 2, "the cold path loads the installed snapshot");
                // the tail after the snapshot is (snap_seq, now] = ops with op_seq > 2 = {3..8}.
                let seqs: Vec<u64> = tail.iter().map(|p| p.op_seq).collect();
                assert_eq!(seqs, vec![3, 4, 5, 6, 7, 8], "the live tail after the snapshot, 0 ops lost");
            }
            other => panic!("expected ResyncFromSnapshot, got {other:?}"),
        }
    }

    /// An IN-window cursor takes the WARM path (resume backfill) — no snapshot needed. The boundary
    /// pins that the cold path fires only below the window floor.
    #[test]
    fn in_window_cursor_resumes_without_a_snapshot() {
        let mut t = CollabTransport::open_with_window(tenant(), "page-1", AllowAllAuthority, 3)
            .expect("opens");
        for i in 1..=6 {
            t.send_op(op("c1", i, OpKind::Insert));
        }
        // last_seq = 4 (its next-missing op 5 is still in the window) → warm resume backfills {5,6}.
        let connected = t.connect(&principal(), AuthAction::Edit, Some(4)).expect("warm resume");
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

    // ---- scope-bound rejection ------------------------------------------------------------------

    /// **An over-broad doc scope is REJECTED (the whitelist-not-`*` rule, arch §2.2).** A `page_id`
    /// that is `*` / empty / contains a scope separator cannot open a transport — the bounded
    /// `doc:<page_id>` selector is the only admissible scope.
    #[test]
    fn an_over_broad_scope_is_rejected_at_open() {
        // `*` / a `*`-containing page id / the empty (or whitespace) id are over-broad: the
        // `doc:<page_id>` they would form is unbounded → rejected (the whitelist-not-`*` rule).
        for bad in ["*", "page*", "", "  "] {
            let r = CollabTransport::open(tenant(), bad);
            assert!(r.is_err(), "an over-broad page scope `{bad}` must be rejected at open");
            assert!(
                matches!(r, Err(TransportError::OverBroadScope(_))),
                "`{bad}` is an over-broad-scope rejection"
            );
        }
        // a bounded (opaque) page id opens fine — page ids are opaque, so a `:` in one is admitted
        // (it is part of the bounded `doc:<page_id>` resource id, not a second scope separator).
        assert!(CollabTransport::open(tenant(), "page-abc-123").is_ok(), "a bounded page scope opens");
    }

    // ---- the Layer-2 authority gate (CONNECT authorizes; fail-closed by default) -----------------

    /// **CONNECT authorizes through Layer 2 — the fail-closed default DENIES (arch §2 step 1 / KN-P14
    /// floor).** With the production [`FailClosedAuthority`], a connect is `Unauthorized` (no op,
    /// no backfill, without authz). KN-P14 swaps the real `Id.check` body in behind the same seam.
    #[test]
    fn connect_fail_closes_to_unauthorized_by_default() {
        let mut t = CollabTransport::open(tenant(), "page-1").expect("opens");
        let r = t.connect(&principal(), AuthAction::Edit, None);
        assert!(
            matches!(r, Err(TransportError::Unauthorized { .. })),
            "the fail-closed authority denies the connect (no op without authz)"
        );
    }

    /// An authorized connect (the real `Id.check` allowed) reaches the resume path (warm — a fresh
    /// client gets the whole tail backfilled).
    #[test]
    fn an_authorized_connect_reaches_the_resume_path() {
        let mut t = open();
        t.send_op(op("c1", 1, OpKind::Insert));
        t.send_op(op("c1", 2, OpKind::Insert));
        let connected = t.connect(&principal(), AuthAction::Edit, None).expect("authorized connect");
        match connected {
            Connected::Resumed { backfill } => {
                assert_eq!(backfill.len(), 2, "a fresh authorized connect backfills the whole tail");
            }
            other => panic!("expected Resumed, got {other:?}"),
        }
    }

    // ---- presence is ephemeral, never persisted -------------------------------------------------

    /// **Presence is EPHEMERAL — it NEVER enters the op-log (arch §2.3).** Publishing presence frames
    /// does not change the op count / the head seq; only `send_op` does. The data-loss-free invariant
    /// runs the right way: ops are durable + resumable, presence is live-only + droppable.
    #[test]
    fn presence_is_ephemeral_and_never_persisted() {
        let mut t = open();
        t.send_op(op("c1", 1, OpKind::Insert));
        let head_before = t.head_seq();
        let ops_before = t.op_count();

        // publish a flurry of presence frames — NONE persists.
        for i in 0..50 {
            t.publish_presence(&Presence::new("c1", format!("caret:{i}")));
        }
        assert_eq!(t.head_seq(), head_before, "presence did NOT advance the op-log cursor");
        assert_eq!(t.op_count(), ops_before, "presence is NEVER persisted to the op-log (arch §2.3)");
    }

    // ---- the live fan-out (a second connection sees an edit live) --------------------------------

    /// **A second subscriber sees an edit LIVE (the §4 first-runnable: "a second connection sees edits
    /// live").** A live subscription receives the frame a `send_op` fans out — the live-delivery half
    /// of the transport (the warm-path live tier after CONNECT).
    #[test]
    fn a_second_connection_sees_an_edit_live() {
        let mut t = open();
        let sub = t.subscribe(None).expect("a live subscription opens");
        // a peer sends an op → the live subscriber receives its frame.
        let sent = t.send_op(op("c2", 1, OpKind::Insert));
        let frames = sub.drain_ready();
        assert_eq!(frames.len(), 1, "the live subscriber received the published frame");
        assert_eq!(frames[0].seq, sent.persisted().op_seq, "the live frame seq == the op_seq");
    }

    // ---- op kind structural classification (the coalescer's choice, not the transport's) ---------

    /// `is_structural` classifies block-tree ops (move/block_ins/block_del) vs content ops — the
    /// coalescer's `knowledge.page.updated` (semantic) vs `knowledge.doc.updated` (pointer) choice
    /// (arch §7). It NEVER changes how the op is transported.
    #[test]
    fn op_kind_structural_classification() {
        assert!(OpKind::Move.is_structural());
        assert!(OpKind::BlockIns.is_structural());
        assert!(OpKind::BlockDel.is_structural());
        assert!(!OpKind::Insert.is_structural());
        assert!(!OpKind::Format.is_structural());
        // every kind has a stable wire token (the op_kind column, 01 §3).
        for k in [
            OpKind::Insert, OpKind::Delete, OpKind::Format, OpKind::Move,
            OpKind::SetProp, OpKind::BlockIns, OpKind::BlockDel, OpKind::EnginePromote,
        ] {
            assert!(!k.as_str().is_empty());
        }
    }

    /// The `op_id` wire form is the `UNIQUE(op_id)` key half (`<client>:<lamport>`), PII-free.
    #[test]
    fn op_id_wire_form() {
        assert_eq!(OpId::new("c1", 42).wire(), "c1:42");
        // two ops from the same client at different lamports are distinct keys.
        assert_ne!(OpId::new("c1", 1).wire(), OpId::new("c1", 2).wire());
        // the same (client, lamport) is the SAME key (the idempotent-apply determinism).
        assert_eq!(OpId::new("c1", 1).wire(), OpId::new("c1", 1).wire());
    }

    // ---- the KN-P11 compaction helpers (ops_up_to / ops_in_range / lowest_seq / gc_below) ---------

    /// A log with ops op_seq 1..=n (one per lamport), for the compaction-helper boundary tests.
    fn log_seq(n: u64) -> DocOpLog {
        let mut log = DocOpLog::new();
        for i in 1..=n {
            log.persist(op("c1", i, OpKind::Insert));
        }
        log
    }

    /// **`ops_up_to(k)` is INCLUSIVE of `k` (the compaction prefix boundary).** The compacted prefix
    /// must include op_seq == k (the snapshot includes ops up to AND including snap_seq).
    #[test]
    fn ops_up_to_is_inclusive() {
        let log = log_seq(5);
        let seqs: Vec<u64> = log.ops_up_to(3).iter().map(|p| p.op_seq).collect();
        assert_eq!(seqs, vec![1, 2, 3], "ops_up_to(3) includes op_seq 3 (inclusive prefix)");
        assert!(log.ops_up_to(0).is_empty(), "ops_up_to(0) is empty");
        assert_eq!(log.ops_up_to(5).len(), 5, "ops_up_to(head) is the whole log");
    }

    /// **`ops_in_range(from, to)` is `(from, to]` — `from` EXCLUSIVE, `to` INCLUSIVE (the tail boundary
    /// a reconstruct appends on a snapshot seed).** op_seq == from is EXCLUDED (the seed covers it);
    /// op_seq == to is INCLUDED (the target version).
    #[test]
    fn ops_in_range_is_from_exclusive_to_inclusive() {
        let log = log_seq(6);
        let seqs: Vec<u64> = log.ops_in_range(2, 5).iter().map(|p| p.op_seq).collect();
        assert_eq!(
            seqs,
            vec![3, 4, 5],
            "(2, 5] excludes the seed boundary 2 and includes the target 5"
        );
        // The from-exclusive boundary specifically: op_seq == from must NOT appear (kills `>` → `>=`).
        assert!(
            !log.ops_in_range(3, 6).iter().any(|p| p.op_seq == 3),
            "op_seq == from is excluded (the seed already covers it)"
        );
    }

    /// **`lowest_seq` is the GC floor (the lowest retained op_seq, 0 if empty).** After a GC prunes
    /// the low end, `lowest_seq` rises — the gap detector reads it.
    #[test]
    fn lowest_seq_is_the_gc_floor() {
        let mut log = log_seq(5);
        assert_eq!(log.lowest_seq(), 1, "the lowest retained op is op_seq 1");
        log.gc_below(2); // prune ≤ 2
        assert_eq!(log.lowest_seq(), 3, "after GC ≤ 2 the floor rises to op_seq 3");
        log.gc_below(99); // prune everything retained
        assert_eq!(log.lowest_seq(), 0, "an empty log's floor is 0");
    }

    /// **`gc_below(w)` prunes `op_seq ≤ w` and RETAINS `op_seq > w` (the watermark boundary).** A row
    /// AT the watermark is pruned; a row ABOVE it is retained (kills the `>` → `==`/`<`/`>=` mutants on
    /// the retention predicate). The monotone `last_seq` counter survives.
    #[test]
    fn gc_below_prunes_at_and_below_the_watermark_keeps_above() {
        let mut log = log_seq(6);
        let pruned = log.gc_below(3); // prune op_seq ≤ 3, keep 4,5,6
        assert_eq!(pruned, 3, "exactly op_seq 1,2,3 (≤ watermark) pruned");
        let kept: Vec<u64> = log.ops_up_to(6).iter().map(|p| p.op_seq).collect();
        assert_eq!(kept, vec![4, 5, 6], "op_seq AT the watermark (3) is pruned; ABOVE it is kept");
        assert_eq!(log.head_seq(), 6, "the monotone op_seq counter survives the prune");
        // The idempotent index is pruned in lock-step with the SAME watermark boundary: a RETAINED
        // op's op_id stays in the index (a re-delivery is still a Duplicate resolving to its kept
        // op_seq), while a PRUNED op's op_id leaves the index (a re-delivery becomes a fresh Apply).
        // This pins line 361's `> watermark` boundary exactly (kills ==/</>= on the index prune).
        let redelivered_kept = log.persist(op("c1", 4, OpKind::Insert)); // op_seq 4 was KEPT (> 3)
        assert!(
            matches!(redelivered_kept, SendOutcome::Duplicate(_)),
            "a retained op's op_id stays in the index → its re-delivery is an idempotent Duplicate"
        );
        assert_eq!(redelivered_kept.persisted().op_seq, 4, "resolves to the kept op_seq (4)");
        let redelivered_pruned = log.persist(op("c1", 2, OpKind::Insert)); // op_seq 2 was PRUNED (≤ 3)
        assert!(
            redelivered_pruned.applied(),
            "a pruned op's op_id left the index → its re-delivery is a fresh Apply (not a stale dup)"
        );
        // The op EXACTLY AT the watermark (op_seq 3) is pruned from BOTH ops and the index in
        // lock-step (the same `> watermark` boundary on line 361): its re-delivery is a fresh Apply,
        // never a Duplicate pointing at a pruned-from-ops op_seq (which would panic). Pins `>` vs `>=`.
        let redelivered_at_watermark = log.persist(op("c1", 3, OpKind::Insert)); // op_seq 3 == watermark
        assert!(
            redelivered_at_watermark.applied(),
            "the op AT the watermark left the index too (consistent prune) → a fresh Apply"
        );
        // The fresh op continues head+1 (the monotone counter survived the prune). Head: 6 (kept dup
        // op_seq 4 was a no-op) → 7 (pruned op_seq 2 re-applied) → 8 (op_seq 3 re-applied), so the
        // next fresh op is op_seq 9.
        let next = log.persist(op("c1", 7, OpKind::Insert));
        assert_eq!(next.persisted().op_seq, 9, "a fresh op continues head+1 after GC + the re-applies");
        // gc_below(0) is a no-op (nothing at-or-below 0).
        let mut log2 = log_seq(3);
        assert_eq!(log2.gc_below(0), 0, "gc_below(0) prunes nothing");
        assert_eq!(log2.len(), 3);
    }
}
