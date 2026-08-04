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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MembershipKind {
    Cycle,
    Milestone,
}

impl MembershipKind {
    pub fn added_token(self) -> &'static str {
        match self {
            MembershipKind::Cycle => events::CYCLE_ISSUE_ADDED,
            MembershipKind::Milestone => events::MILESTONE_ISSUE_ADDED,
        }
    }

    pub fn removed_token(self) -> &'static str {
        match self {
            MembershipKind::Cycle => events::CYCLE_ISSUE_REMOVED,
            MembershipKind::Milestone => events::MILESTONE_ISSUE_REMOVED,
        }
    }

    pub fn url_segment(self) -> &'static str {
        match self {
            MembershipKind::Cycle => "cycle",
            MembershipKind::Milestone => "milestone",
        }
    }

    pub fn rel_token(self) -> &'static str {
        match self {
            MembershipKind::Cycle => "member_of_cycle",
            MembershipKind::Milestone => "member_of_milestone",
        }
    }
}

pub fn time_axis_ref(tenant: &str, kind: MembershipKind, axis_key: &str) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{tenant}/issue/{}/{axis_key}",
        kind.url_segment()
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembershipEdge {
    pub kind: MembershipKind,
    pub issue: ArtifactRef,
    pub axis: ArtifactRef,
    pub carried_over_from: Option<ArtifactRef>,
}

impl MembershipEdge {
    pub fn new(kind: MembershipKind, issue: ArtifactRef, axis: ArtifactRef) -> MembershipEdge {
        MembershipEdge {
            kind,
            issue,
            axis,
            carried_over_from: None,
        }
    }

    pub fn is_carried_over(&self) -> bool {
        self.carried_over_from.is_some()
    }
}

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
        subject: edge.axis.clone(),
        aggregate: edge_aggregate_key(&edge.axis, &edge.issue),
        payload,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

pub fn emit_membership_edge(
    tx: &mut dyn OutboxTx,
    edge: &MembershipEdge,
    added: bool,
    cause: Option<&EventEnvelope>,
) -> BusResult<EventId> {
    tx.emit(membership_draft(edge, added), cause)
}

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

fn is_closed_category(cat: StateCategory) -> bool {
    matches!(cat, StateCategory::Completed | StateCategory::Cancelled)
}

#[derive(Clone, Debug, PartialEq)]
pub struct BurndownPoint {
    pub cycle: ArtifactRef,
    pub at: String,
    pub remaining_estimate: f64,
    pub carried_over_estimate: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CfdBand {
    pub cycle: ArtifactRef,
    pub at: String,
    pub unstarted: u32,
    pub started: u32,
    pub completed: u32,
    pub cancelled: u32,
}

impl CfdBand {
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

    pub fn total(&self) -> u32 {
        self.unstarted + self.started + self.completed + self.cancelled
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentPointer {
    pub blob_ref: ContentHash,
    pub size_bytes: u64,
    pub region: String,
    pub pii_key_ref: String,
    pub content_type: String,
}

impl AttachmentPointer {
    pub fn row_byte_count(&self) -> usize {
        0
    }

    pub fn fetch_bytes(
        &self,
        store: &dyn BlobStore,
        tenant: &TenantId,
    ) -> Result<Vec<u8>, BlobError> {
        store.get(tenant, &self.blob_ref)
    }
}

pub fn attach(
    store: &dyn BlobStore,
    tenant: &TenantId,
    subject: &SubjectId,
    dek_epoch: u64,
    region: &str,
    content_type: &str,
    bytes: &[u8],
) -> Result<AttachmentPointer, BlobError> {
    let blob_ref = store.put(tenant, bytes)?;
    Ok(AttachmentPointer {
        blob_ref,
        size_bytes: bytes.len() as u64,
        region: region.to_string(),
        pii_key_ref: subject_dek_ref(&tenant.0, dek_epoch, subject),
        content_type: content_type.to_string(),
    })
}

pub fn subject_dek_ref(tenant: &str, dek_epoch: u64, subject: &SubjectId) -> String {
    format!("kms://{tenant}/{dek_epoch}/subject:{}", subject.0)
}

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
            "pii_key_ref": pointer.pii_key_ref,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: true,
        pii_key_ref: Some(myelin_events::PiiKeyRef(pointer.pii_key_ref.clone())),
    }
}

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
