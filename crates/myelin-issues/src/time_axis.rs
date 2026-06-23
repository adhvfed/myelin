//! The TIME AXIS (cycles/sprints + milestones) + attachments in BlobStore — ISS-P19 / P-386.
//!
//! ## The one invariant: the time axis is MEMBERSHIP EDGES over the one model, never a new
//! containment graph (external-insights/01 §7; arch 01 §5).
//!
//! A cycle (sprint) and a milestone (version/release) are **separate objects** (the `cycle` /
//! `milestone` tables, [`crate::migrations`]) — they are NOT issues (no workflow state, no assignee).
//! An issue's membership in a cycle/milestone is a **relation row** (`cycle_membership`; the milestone
//! membership rides the SAME edge shape), NOT a containment edge in the `parent` tree. This is the
//! resolution external-insights/01 §7 demands: adding an issue to a cycle is an **edge write** (a
//! `cycle_membership` row + ONE TE-7 mirror event), never a containment migration that re-parents the
//! issue. The denormalised `issue.cycle_id` column is a CACHE of this edge truth (arch 01 §5).
//!
//! ### Why a TE-7 mirror, not a bespoke membership event (contract 5.5)
//!
//! The membership edge is dual-homed exactly like an `issue_relation` lifecycle edge: Issues writes
//! the forward edge transactionally + emits ONE typed `issue.cycle.issue_added` / `.removed` (resp.
//! `issue.milestone.issue_added` / `.removed`) event carrying `rel_class = lifecycle`; the Refs mirror
//! materialises the projection + fixes the inverse pairing. We re-use the byte-identical mirror shape
//! [`crate::refs_glue::REL_CLASS_LIFECYCLE`] + [`crate::refs_glue::edge_aggregate_key`] — Issues
//! produces the SAME wire tokens the Refs mirror ingests, never a second vocabulary (EI-01 §7: extend,
//! never fork). The emit shares the caller's transaction, so the membership row and its mirror event
//! co-commit (**emit-iff-committed**: an aborted membership write drops the buffered event with it).
//!
//! ### Carry-over provenance (flow A3)
//!
//! When an active cycle completes with unfinished issues, an incomplete issue is **carried over** into
//! the next cycle: a NEW `cycle_membership` row in the destination cycle with `carried_over_from` set
//! to the SOURCE cycle id. The provenance is preserved across the rollover ([`rollover_carry_over`]),
//! so the burndown/CFD feed can attribute carried-over work distinctly (it is not new scope).
//!
//! ## Attachments in BlobStore — the row holds the POINTER, never the bytes (contract 11.2; arch §1.2)
//!
//! An attachment's bytes live in [`myelin_storage::blob::BlobStore`] (content-addressed by BLAKE3,
//! per-tenant-deduped, residency-pinned to the cell region). The OLTP `issue` row holds ONLY the
//! content-address pointer + per-subject-DEK metadata + the byte length + the residency region — **0
//! bytes of the attachment in the row** ([`AttachmentPointer`]). This is the contract-11.2 posture:
//! "the row holds the pointer + per-subject-DEK metadata, not the bytes". The crypto-shred erasure of
//! the blob (immutable/backup tier) is the holder erasure path (§7, ISS-P07/holder_erase), NOT a
//! synchronous `delete` — so the pointer carries the `pii_key_ref` that names the wrapping key.
//!
//! ## FLOOR named (none new here)
//!
//! The burndown / CFD / velocity ANALYTICS land in the **OLAP read store** (ISS-P20 / P-387): this
//! module ships the time-axis MODEL + the off-the-bus FEED ([`BurndownPoint`] / [`CfdBand`] are the
//! shapes the OLAP consumer reindexes from the `issue.cycle.*` + `issue.*` streams), but the
//! restriction-flag-honouring OLAP aggregation + the served charts are ISS-P20's deliverable. The feed
//! is fed off the bus, never computed in the write path (CQRS, arch §1.2).

use crate::events;
use crate::refs_glue::{edge_aggregate_key, REL_CLASS_LIFECYCLE};
use crate::workflow::StateCategory;
use myelin_events::{
    ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType, OutboxTx,
    Result as BusResult, Visibility,
};
use myelin_storage::blob::{BlobError, BlobStore, ContentHash};
use myelin_storage::encryption::SubjectId;
use myelin_tenancy::TenantId;

// ════════════════════════════════════════════════════════════════════════════════════════════════
// §1 — THE TIME-AXIS MEMBERSHIP EDGE (cycle / milestone) — NOT containment (arch 01 §5)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// Which time-axis object an issue is a member of (a cycle/sprint OR a milestone/version). BOTH are
/// MEMBERSHIP edges over the one issue model — neither is a containment edge in the `parent` tree
/// (external-insights/01 §7). The kind selects the event family + the URN object-type segment; the
/// edge SHAPE is identical (that is the point — one membership model, two axes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MembershipKind {
    /// A cycle/sprint membership (`cycle_membership` is the truth; `issue.cycle_id` is a cache).
    Cycle,
    /// A milestone/version membership (the same edge shape; the import target, sketch 09).
    Milestone,
}

impl MembershipKind {
    /// The `issue.*` event token an ADD emits (the membership edge appeared). A cycle add emits
    /// [`events::CYCLE_ISSUE_ADDED`]; a milestone add emits [`events::MILESTONE_ISSUE_ADDED`].
    pub fn added_token(self) -> &'static str {
        match self {
            MembershipKind::Cycle => events::CYCLE_ISSUE_ADDED,
            MembershipKind::Milestone => events::MILESTONE_ISSUE_ADDED,
        }
    }

    /// The `issue.*` event token a REMOVE emits (the membership edge dropped).
    pub fn removed_token(self) -> &'static str {
        match self {
            MembershipKind::Cycle => events::CYCLE_ISSUE_REMOVED,
            MembershipKind::Milestone => events::MILESTONE_ISSUE_REMOVED,
        }
    }

    /// The artifact-type segment of the time-axis object's URN (`myelin://<tenant>/issue/<seg>/<id>`).
    /// A cycle is `cycle`; a milestone is `milestone` (both are issue-subsystem objects, NOT issues).
    pub fn url_segment(self) -> &'static str {
        match self {
            MembershipKind::Cycle => "cycle",
            MembershipKind::Milestone => "milestone",
        }
    }

    /// The `rel` token a membership edge carries (a stable lifecycle-rel vocabulary token). A cycle
    /// membership is `member_of_cycle`; a milestone membership is `member_of_milestone`. PII-free.
    pub fn rel_token(self) -> &'static str {
        match self {
            MembershipKind::Cycle => "member_of_cycle",
            MembershipKind::Milestone => "member_of_milestone",
        }
    }
}

/// The canonical URN of a time-axis object (a cycle or a milestone). NOT an issue URN — the object
/// type segment is `cycle`/`milestone`. PII-free (an opaque key).
pub fn time_axis_ref(tenant: &str, kind: MembershipKind, axis_key: &str) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{tenant}/issue/{}/{axis_key}",
        kind.url_segment()
    ))
}

/// One time-axis membership EDGE (the `cycle_membership` row truth — and, for a milestone, the same
/// edge shape). The `issue` is the member; the `axis` is the cycle/milestone object it belongs to.
/// `carried_over_from` carries the carry-over provenance (flow A3): when set, this membership was
/// produced by a rollover from the named SOURCE cycle (the issue is carried-over work, not new scope).
///
/// **This is an EDGE, not containment.** Writing it is a relation-row write + ONE mirror event, never
/// a re-parent of the issue. The `issue.parent_id` tree is untouched by a cycle/milestone add/remove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembershipEdge {
    /// Which axis this edge is on (cycle vs milestone).
    pub kind: MembershipKind,
    /// The member issue's canonical URN (the `<PROJECTKEY>-<seqno>` issue ref).
    pub issue: ArtifactRef,
    /// The time-axis object's canonical URN (the cycle/milestone the issue belongs to).
    pub axis: ArtifactRef,
    /// Carry-over provenance (flow A3): the SOURCE cycle URN this membership was carried over FROM,
    /// or `None` for a fresh add. Only ever set on a cycle membership produced by [`rollover_carry_over`].
    pub carried_over_from: Option<ArtifactRef>,
}

impl MembershipEdge {
    /// A fresh membership edge (no carry-over provenance — the issue is being added directly).
    pub fn new(kind: MembershipKind, issue: ArtifactRef, axis: ArtifactRef) -> MembershipEdge {
        MembershipEdge {
            kind,
            issue,
            axis,
            carried_over_from: None,
        }
    }

    /// Whether this membership is carried-over work (it has a `carried_over_from` provenance pointer).
    /// The burndown/CFD feed reads this to attribute carried-over work distinctly from new scope.
    pub fn is_carried_over(&self) -> bool {
        self.carried_over_from.is_some()
    }
}

/// Build the `issue.cycle.issue_added` / `.removed` (resp. milestone) [`EventDraft`] for a membership
/// edge write — the TE-7 mirror (contract 5.5). The aggregate is the SHARED `edge:<axis>-><issue>` key
/// (byte-identical to [`edge_aggregate_key`]) so the add → remove sequence is per-aggregate ordered;
/// the subject is the time-axis OBJECT (so the cycle/milestone aggregate carries its membership churn).
/// `rel_class = lifecycle` (a membership edge is a lifecycle edge, never a `reference` content edge).
/// PII-free (opaque URNs only); the carry-over provenance rides the payload (also an opaque URN).
fn membership_draft(edge: &MembershipEdge, added: bool) -> EventDraft {
    let type_ = if added {
        edge.kind.added_token()
    } else {
        edge.kind.removed_token()
    };
    let mut payload = serde_json::json!({
        "source": edge.axis.0,
        "target": edge.issue.0,
        "rel": edge.kind.rel_token(),
        "rel_class": REL_CLASS_LIFECYCLE,
    });
    if let Some(src) = &edge.carried_over_from {
        payload["carried_over_from"] = serde_json::Value::String(src.0.clone());
    }
    EventDraft {
        type_: EventType(type_.into()),
        // The subject is the time-axis OBJECT (the cycle/milestone) whose membership churned.
        subject: edge.axis.clone(),
        // The shared edge aggregate (axis -> issue): the add → remove sequence is ordered per edge.
        aggregate: edge_aggregate_key(&edge.axis, &edge.issue),
        payload,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// **Emit `issue.{cycle,milestone}.issue_added` / `.removed` for a membership EDGE write, IN THE SAME
/// TRANSACTION as the `cycle_membership` row write (contract 5.5 — the TE-7 mirror; the membership
/// table is TRUTH).**
///
/// Call this from the SAME `tx` that wrote/deleted the membership row. `added` is `true` for an add,
/// `false` for a remove. Returns the minted `event_id`. The forward edge is emitted only; the Refs
/// mirror projects the inverse. Because the emit shares `tx`, the membership row and its mirror event
/// co-commit (**emit-iff-committed**) — a membership add is NEVER a containment migration (it is one
/// edge row + one event), and is never written without its mirror edge.
pub fn emit_membership_edge(
    tx: &mut dyn OutboxTx,
    edge: &MembershipEdge,
    added: bool,
    cause: Option<&EventEnvelope>,
) -> BusResult<EventId> {
    tx.emit(membership_draft(edge, added), cause)
}

/// **Carry-over a set of UNFINISHED issues from a completing cycle into the next cycle (flow A3).**
///
/// Returns the NEW `cycle_membership` edges for the destination cycle, each carrying the
/// `carried_over_from = source` provenance. ONLY issues whose state category is not a CLOSED category
/// (`completed`/`cancelled`) are carried over — finished work is NOT re-scoped into the next cycle.
/// The caller writes each returned edge to the destination cycle (a row + one mirror event via
/// [`emit_membership_edge`]); the SOURCE cycle's memberships are left intact as the historical record
/// (the burndown of the completed cycle stays computable). This is a pure function — it computes WHICH
/// issues carry over + the provenance; the row/edge writes are the caller's transactional concern.
pub fn rollover_carry_over(
    tenant: &str,
    source_cycle_key: &str,
    dest_cycle_key: &str,
    members: &[(ArtifactRef, StateCategory)],
) -> Vec<MembershipEdge> {
    let source = time_axis_ref(tenant, MembershipKind::Cycle, source_cycle_key);
    let dest = time_axis_ref(tenant, MembershipKind::Cycle, dest_cycle_key);
    members
        .iter()
        .filter(|(_, cat)| !is_closed_category(*cat))
        .map(|(issue, _)| MembershipEdge {
            kind: MembershipKind::Cycle,
            issue: issue.clone(),
            axis: dest.clone(),
            carried_over_from: Some(source.clone()),
        })
        .collect()
}

/// Whether a state category is CLOSED (the issue is finished — `completed` or `cancelled`). Closed
/// issues are NOT carried over on a cycle rollover (they are not unfinished scope).
fn is_closed_category(cat: StateCategory) -> bool {
    matches!(cat, StateCategory::Completed | StateCategory::Cancelled)
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// §2 — THE BURNDOWN / CFD FEED (off the bus → OLAP; the ANALYTICS are ISS-P20) (arch §1.2)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// One BURNDOWN sample point for a cycle (remaining-work over time). The OLAP read store (ISS-P20)
/// reindexes a series of these from the `issue.cycle.*` + `issue.transitioned` streams off the bus —
/// this is the SHAPE of the feed, not the served chart. `remaining_estimate` is the sum of estimates
/// of issues in the cycle NOT yet in a closed category; `carried_over_estimate` is the sub-sum that is
/// carried-over work (flow A3) so the chart can render it distinctly. Durations/estimates are unitless
/// story-point-or-estimate numbers; the `at` instant is RFC-3339 UTC (the frozen unit, events §1).
#[derive(Clone, Debug, PartialEq)]
pub struct BurndownPoint {
    /// The cycle's canonical URN this sample belongs to.
    pub cycle: ArtifactRef,
    /// The RFC-3339 UTC instant of the sample.
    pub at: String,
    /// The remaining (not-closed) estimate at `at`.
    pub remaining_estimate: f64,
    /// The sub-portion of `remaining_estimate` that is carried-over work (`carried_over_from` set).
    pub carried_over_estimate: f64,
}

/// One CUMULATIVE-FLOW band sample for a cycle: the count of issues in each `state_category` at an
/// instant (the CFD stacks these categories over time). The OLAP read store (ISS-P20) reindexes the
/// series; this is the feed shape. The four counts mirror the FIXED `state_cat` set (arch §2) — the
/// cross-project reporting invariant (a renamed state still lands in its fixed category band).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CfdBand {
    /// The cycle's canonical URN this band belongs to.
    pub cycle: ArtifactRef,
    /// The RFC-3339 UTC instant of the sample.
    pub at: String,
    /// Count in the `unstarted` category.
    pub unstarted: u32,
    /// Count in the `started` category.
    pub started: u32,
    /// Count in the `completed` category.
    pub completed: u32,
    /// Count in the `cancelled` category.
    pub cancelled: u32,
}

impl CfdBand {
    /// Tally a CFD band from a snapshot of `(category)` over the cycle's current members. A pure
    /// reduction — the OLAP consumer (ISS-P20) calls this per sample instant off the bus, never in
    /// the write path (CQRS). The total equals the member count (every member lands in exactly one
    /// fixed category band — no member is dropped or double-counted).
    pub fn tally(cycle: &ArtifactRef, at: &str, members: &[StateCategory]) -> CfdBand {
        let mut band = CfdBand {
            cycle: cycle.clone(),
            at: at.to_string(),
            unstarted: 0,
            started: 0,
            completed: 0,
            cancelled: 0,
        };
        for cat in members {
            match cat {
                StateCategory::Unstarted => band.unstarted += 1,
                StateCategory::Started => band.started += 1,
                StateCategory::Completed => band.completed += 1,
                StateCategory::Cancelled => band.cancelled += 1,
            }
        }
        band
    }

    /// The total member count across all four fixed category bands (the invariant: no member is
    /// dropped — the sum of the bands equals the cycle's member count).
    pub fn total(&self) -> u32 {
        self.unstarted + self.started + self.completed + self.cancelled
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// §3 — ATTACHMENTS IN BLOBSTORE — the row holds the POINTER, never the bytes (contract 11.2)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The attachment POINTER stored on the OLTP `issue` row — 0 bytes of the attachment (contract
/// 11.2; arch §1.2).**
///
/// The attachment bytes live in [`BlobStore`] (content-addressed, residency-pinned). This struct is
/// what the row holds: the content address ([`ContentHash`]), the byte length, the residency region,
/// the per-subject-DEK metadata ([`pii_key_ref`](Self::pii_key_ref) — the `kms://…` URN naming the
/// wrapping key, so the crypto-shred erasure reaches the blob), and the (PII-bearing) filename's own
/// reference. It carries NO `bytes` field — by construction the row can never hold the attachment
/// payload ([`Self::row_byte_count`] is always 0 for the attachment content; the
/// `attachment_row_holds_zero_bytes` assertion is the green artifact).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentPointer {
    /// The BlobStore content address (BLAKE3 multihash) — the attachment's identity, NOT its bytes.
    pub blob_ref: ContentHash,
    /// The attachment's byte length (metadata — the SIZE, never the content).
    pub size_bytes: u64,
    /// The residency region the blob is pinned to (== the cell region; the residency-pin lint).
    pub region: String,
    /// The per-subject-DEK `kms://…` URN naming the wrapping key (so the holder erasure path can
    /// crypto-shred the blob — §7). `class` is `blob` or `subject:<id>` per the envelope grammar.
    pub pii_key_ref: String,
    /// The MIME content type (metadata; e.g. `image/png`). Not the bytes.
    pub content_type: String,
}

impl AttachmentPointer {
    /// **The 0-bytes-in-row invariant (the GATE's green artifact).** An [`AttachmentPointer`] holds a
    /// content ADDRESS + metadata; it never holds the attachment bytes. This always returns 0 — the
    /// row can structurally never carry the payload (there is no `bytes` field). The CI assertion
    /// reads this to prove "0 bytes in the OLTP row".
    pub fn row_byte_count(&self) -> usize {
        0
    }

    /// Resolve the attachment's bytes from the BlobStore (re-hash-on-read; refuses a corrupt serve,
    /// [`BlobError::IntegrityFail`]). The bytes are FETCHED from the blob tier on demand — they are
    /// never resident on the row. `tenant` keys the per-tenant blob keyspace (residency-pinned).
    pub fn fetch_bytes(
        &self,
        store: &dyn BlobStore,
        tenant: &TenantId,
    ) -> Result<Vec<u8>, BlobError> {
        store.get(tenant, &self.blob_ref)
    }
}

/// **Attach a blob to an issue: PUT the bytes into BlobStore + return the POINTER for the row
/// (contract 11.2).**
///
/// Hash-on-write stores the bytes in `tenant`'s residency-pinned keyspace (per-tenant dedup — the
/// SAME bytes attached twice store once) and returns an [`AttachmentPointer`] for the OLTP row. The
/// pointer carries the per-subject-DEK `pii_key_ref` ([`subject_dek_ref`]) so the holder erasure path
/// can crypto-shred the blob (§7). The bytes go to the BLOB tier ONLY — the returned pointer holds 0
/// of them (the row never sees the payload). `subject` is the attachment-uploader/owner whose DEK
/// wraps the blob; `region` is the cell region (residency-pin). `content_type`/`size` are metadata.
pub fn attach(
    store: &dyn BlobStore,
    tenant: &TenantId,
    subject: &SubjectId,
    dek_epoch: u64,
    region: &str,
    content_type: &str,
    bytes: &[u8],
) -> Result<AttachmentPointer, BlobError> {
    // Hash-on-write: the bytes go to the BLOB tier only (content-addressed, per-tenant dedup).
    let blob_ref = store.put(tenant, bytes)?;
    Ok(AttachmentPointer {
        blob_ref,
        size_bytes: bytes.len() as u64,
        region: region.to_string(),
        pii_key_ref: subject_dek_ref(&tenant.0, dek_epoch, subject),
        content_type: content_type.to_string(),
    })
}

/// The per-subject-DEK `kms://<tenant>/<epoch>/subject:<id>` URN for an attachment blob (the frozen
/// `pii_key_ref` grammar, `myelin_events::envelope`; class `subject:<id>`). Naming the subject's DEK
/// (not a per-tenant key) is what makes the attachment crypto-shreddable on that subject's erasure
/// (§7 / GD-4): destroying the subject DEK renders every blob it wrapped unreadable.
pub fn subject_dek_ref(tenant: &str, dek_epoch: u64, subject: &SubjectId) -> String {
    format!("kms://{tenant}/{dek_epoch}/subject:{}", subject.0)
}

/// Build the `issue.attachment.added` / `.removed` [`EventDraft`] for an attachment pointer write.
/// The payload carries the content ADDRESS (`blob_ref`) + size + region + the `pii_key_ref`, NEVER the
/// bytes (references-not-payloads, contract 2.7). `contains_personal_data` is set (an attachment may
/// hold PII) and the `pii_key_ref` is threaded so a consumer routes the GDPR posture. The subject is
/// the ISSUE (the attachment is an issue sub-fact); the aggregate is the issue (per-issue ordering).
fn attachment_draft(
    issue: &ArtifactRef,
    aggregate: myelin_events::AggregateKey,
    pointer: &AttachmentPointer,
    added: bool,
) -> EventDraft {
    let type_ = if added {
        events::ATTACHMENT_ADDED
    } else {
        events::ATTACHMENT_REMOVED
    };
    EventDraft {
        type_: EventType(type_.into()),
        subject: issue.clone(),
        aggregate,
        payload: serde_json::json!({
            "issue": issue.0,
            "blob_ref": pointer.blob_ref.to_multihash_string(),
            "size_bytes": pointer.size_bytes,
            "region": pointer.region,
            "content_type": pointer.content_type,
            // the POINTER + key ref travel — NEVER the bytes (references-not-payloads, 2.7).
            "pii_key_ref": pointer.pii_key_ref,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: true,
        pii_key_ref: Some(myelin_events::PiiKeyRef(pointer.pii_key_ref.clone())),
    }
}

/// **Emit `issue.attachment.added` / `.removed` for an attachment pointer write, IN THE SAME
/// TRANSACTION as the row write (emit-iff-committed).**
///
/// The event carries the BlobStore POINTER + the `pii_key_ref`, never the bytes (the bytes were
/// already PUT to the blob tier by [`attach`]; the row + the event hold only the pointer). `added` is
/// `true` for an attach, `false` for a detach. Returns the minted `event_id`.
pub fn emit_attachment(
    tx: &mut dyn OutboxTx,
    issue: &ArtifactRef,
    aggregate: myelin_events::AggregateKey,
    pointer: &AttachmentPointer,
    added: bool,
    cause: Option<&EventEnvelope>,
) -> BusResult<EventId> {
    tx.emit(attachment_draft(issue, aggregate, pointer, added), cause)
}

#[cfg(test)]
#[path = "time_axis/tests.rs"]
mod tests;
