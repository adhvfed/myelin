use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_gdpr::{DataRole, EraseScope, SubjectRef, TenantId};
use myelin_substrate::Clock;

use crate::datamap::Inventory;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DsrKind {
    Access,
    Portability,
    Rectification,
    Restriction,
    Erasure,
}

impl DsrKind {
    pub fn is_erasure(self) -> bool {
        matches!(self, DsrKind::Erasure)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Posture {
    Controller,
    Processor,
}

impl Posture {
    pub fn from_data_role(role: DataRole) -> Posture {
        match role {
            DataRole::TenantContent => Posture::Processor,
            DataRole::PlatformOperational => Posture::Controller,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Initiator {
    Myelin,
    TenantInstructed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DsrState {
    Received,
    Validated,
    FannedOut,
    AwaitingHolders,
    Verified,
    Completed,
    Refused,
    Failed,
}

impl DsrState {
    pub fn can_transition_to(self, next: DsrState) -> bool {
        use DsrState::*;
        if next == Failed {
            return !self.is_terminal();
        }
        matches!(
            (self, next),
            (Received, Validated)
                | (Validated, FannedOut)
                | (Validated, Refused)
                | (FannedOut, AwaitingHolders)
                | (AwaitingHolders, Verified)
                | (Verified, Completed)
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            DsrState::Completed | DsrState::Refused | DsrState::Failed
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DsrState::Received => "received",
            DsrState::Validated => "validated",
            DsrState::FannedOut => "fanned-out",
            DsrState::AwaitingHolders => "awaiting-holders",
            DsrState::Verified => "verified",
            DsrState::Completed => "completed",
            DsrState::Refused => "refused",
            DsrState::Failed => "failed",
        }
    }
}

pub const DSR_STATE: (&str, &str) = ("gdpr.dsr_state", "state");

pub const DSR_DEADLINE_SECS: u64 = 30 * 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DsrId(pub String);

impl DsrId {
    fn of(n: u64) -> DsrId {
        DsrId(format!("dsr:{n}"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChecklistItem {
    pub holder_id: String,
    pub field_mechanisms: Vec<String>,
}

pub fn resolve_checklist_from_map(inventory: &Inventory) -> Vec<ChecklistItem> {
    let mut by_holder: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for holder_id in &inventory.holders {
        by_holder.entry(holder_id.clone()).or_default();
    }
    for e in &inventory.entries {
        by_holder
            .entry(e.holder_id.clone())
            .or_default()
            .push(format!("{}::{}", e.field_path, e.erasure));
    }
    by_holder
        .into_iter()
        .map(|(holder_id, mut field_mechanisms)| {
            field_mechanisms.sort();
            ChecklistItem {
                holder_id,
                field_mechanisms,
            }
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct Dsr {
    pub id: DsrId,
    pub kind: DsrKind,
    pub tenant: TenantId,
    pub subject: SubjectRef,
    pub scope: EraseScope,
    pub posture: Posture,
    pub initiator: Initiator,
    pub state: DsrState,
    pub submitted_at_secs: u64,
    pub deadline_secs: u64,
    pub checklist: Vec<ChecklistItem>,
    pub receipts: Vec<String>,
}

impl Dsr {
    pub fn status(&self) -> DsrStatus {
        DsrStatus {
            state: self.state,
            deadline_secs: self.deadline_secs,
            checklist: self.checklist.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DsrStatus {
    pub state: DsrState,
    pub deadline_secs: u64,
    pub checklist: Vec<ChecklistItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MerkleProvenBundle {
    pub dsr_id: DsrId,
    pub receipts: Vec<String>,
    pub bundle_digest: String,
    pub merkle_inclusion: Option<String>,
}

impl MerkleProvenBundle {
    fn content_addressed(dsr_id: &DsrId, receipts: &[String]) -> MerkleProvenBundle {
        let mut preimage = dsr_id.0.clone();
        for r in receipts {
            preimage.push('\u{1f}');
            preimage.push_str(r);
        }
        let digest = blake3::hash(preimage.as_bytes());
        MerkleProvenBundle {
            dsr_id: dsr_id.clone(),
            receipts: receipts.to_vec(),
            bundle_digest: format!("blake3:{}", hex::encode(digest.as_bytes())),
            merkle_inclusion: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DsrError {
    UnknownDsr(DsrId),
    IllegalTransition { from: DsrState, to: DsrState },
    CertificateNotReady(DsrState),
    HolderFanOut(String),
}

impl std::fmt::Display for DsrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DsrError::UnknownDsr(id) => write!(f, "no DSR with id `{}`", id.0),
            DsrError::IllegalTransition { from, to } => write!(
                f,
                "illegal DSR transition {} → {} (§4.1 - the state machine is total + ordered; \
                 awaiting-holders cannot be skipped)",
                from.as_str(),
                to.as_str()
            ),
            DsrError::CertificateNotReady(state) => write!(
                f,
                "dsr_certificate not ready: DSR is `{}` (a certificate exists only once verified)",
                state.as_str()
            ),
            DsrError::HolderFanOut(msg) => write!(
                f,
                "holder fan-out errored: {msg} (§4.1 step 4 - the checklist is resumable; re-drive \
                 to resume from the failed holder)"
            ),
        }
    }
}

impl std::error::Error for DsrError {}

pub type Result<T> = std::result::Result<T, DsrError>;

pub struct DsrOrchestrator<C: Clock> {
    clock: C,
    register: Mutex<DsrRegister>,
}

#[derive(Default)]
struct DsrRegister {
    next: u64,
    dsrs: BTreeMap<DsrId, Dsr>,
}

impl<C: Clock> DsrOrchestrator<C> {
    pub fn new(clock: C) -> DsrOrchestrator<C> {
        DsrOrchestrator {
            clock,
            register: Mutex::new(DsrRegister::default()),
        }
    }

    pub fn dsr_submit(
        &self,
        kind: DsrKind,
        tenant: TenantId,
        subject: SubjectRef,
        scope: EraseScope,
        posture: Posture,
        initiator: Initiator,
    ) -> DsrId {
        let now = self.clock.now_secs();
        let mut reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let id = DsrId::of(reg.next);
        reg.next += 1;
        let dsr = Dsr {
            id: id.clone(),
            kind,
            tenant,
            subject,
            scope,
            posture,
            initiator,
            state: DsrState::Received,
            submitted_at_secs: now,
            deadline_secs: now + DSR_DEADLINE_SECS,
            checklist: Vec::new(),
            receipts: Vec::new(),
        };
        reg.dsrs.insert(id.clone(), dsr);
        id
    }

    pub fn dsr_status(&self, id: &DsrId) -> Result<DsrStatus> {
        let reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        reg.dsrs
            .get(id)
            .map(Dsr::status)
            .ok_or_else(|| DsrError::UnknownDsr(id.clone()))
    }

    pub fn dsr_certificate(&self, id: &DsrId) -> Result<MerkleProvenBundle> {
        let reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let dsr = reg
            .dsrs
            .get(id)
            .ok_or_else(|| DsrError::UnknownDsr(id.clone()))?;
        if !matches!(dsr.state, DsrState::Verified | DsrState::Completed) {
            return Err(DsrError::CertificateNotReady(dsr.state));
        }
        Ok(MerkleProvenBundle::content_addressed(id, &dsr.receipts))
    }

    pub fn validate(&self, id: &DsrId) -> Result<bool> {
        let mut reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let dsr = reg
            .dsrs
            .get_mut(id)
            .ok_or_else(|| DsrError::UnknownDsr(id.clone()))?;
        transition(dsr, DsrState::Validated)?;
        if Self::posture_gate_refuses(dsr) {
            transition(dsr, DsrState::Refused)?;
            return Ok(false);
        }
        Ok(true)
    }

    fn posture_gate_refuses(dsr: &Dsr) -> bool {
        dsr.kind.is_erasure()
            && dsr.posture == Posture::Processor
            && dsr.initiator == Initiator::Myelin
            && !matches!(dsr.scope, EraseScope::Tenant(_))
    }

    pub fn fan_out(&self, id: &DsrId, inventory: &Inventory) -> Result<Vec<ChecklistItem>> {
        let mut reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let dsr = reg
            .dsrs
            .get_mut(id)
            .ok_or_else(|| DsrError::UnknownDsr(id.clone()))?;
        transition(dsr, DsrState::FannedOut)?;
        dsr.checklist = resolve_checklist_from_map(inventory);
        transition(dsr, DsrState::AwaitingHolders)?;
        Ok(dsr.checklist.clone())
    }

    pub fn verify(&self, id: &DsrId, receipts: Vec<String>) -> Result<()> {
        let mut reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let dsr = reg
            .dsrs
            .get_mut(id)
            .ok_or_else(|| DsrError::UnknownDsr(id.clone()))?;
        transition(dsr, DsrState::Verified)?;
        dsr.receipts = receipts;
        Ok(())
    }

    pub fn complete(&self, id: &DsrId) -> Result<()> {
        let mut reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let dsr = reg
            .dsrs
            .get_mut(id)
            .ok_or_else(|| DsrError::UnknownDsr(id.clone()))?;
        transition(dsr, DsrState::Completed)
    }

    pub fn fail(&self, id: &DsrId) -> Result<()> {
        let mut reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let dsr = reg
            .dsrs
            .get_mut(id)
            .ok_or_else(|| DsrError::UnknownDsr(id.clone()))?;
        transition(dsr, DsrState::Failed)
    }

    pub fn state_of(&self, id: &DsrId) -> Result<DsrState> {
        let reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        reg.dsrs
            .get(id)
            .map(|d| d.state)
            .ok_or_else(|| DsrError::UnknownDsr(id.clone()))
    }

    pub fn request_view(&self, id: &DsrId) -> Result<DsrRequestView> {
        let reg = self.register.lock().unwrap_or_else(|e| e.into_inner());
        let dsr = reg
            .dsrs
            .get(id)
            .ok_or_else(|| DsrError::UnknownDsr(id.clone()))?;
        Ok(DsrRequestView {
            id: dsr.id.clone(),
            kind: dsr.kind,
            tenant: dsr.tenant.clone(),
            scope: dsr.scope.clone(),
            posture: dsr.posture,
            initiator: dsr.initiator,
            state: dsr.state,
            submitted_at_secs: dsr.submitted_at_secs,
        })
    }
}

#[derive(Clone, Debug)]
pub struct DsrRequestView {
    pub id: DsrId,
    pub kind: DsrKind,
    pub tenant: TenantId,
    pub scope: EraseScope,
    pub posture: Posture,
    pub initiator: Initiator,
    pub state: DsrState,
    pub submitted_at_secs: u64,
}

fn transition(dsr: &mut Dsr, to: DsrState) -> Result<()> {
    if !dsr.state.can_transition_to(to) {
        return Err(DsrError::IllegalTransition {
            from: dsr.state,
            to,
        });
    }
    dsr.state = to;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::TestClock;

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    fn subject_scope(s: &str) -> EraseScope {
        EraseScope::Subject {
            subject: subject(s),
            tenant: tenant(),
        }
    }

    fn orch_at(t0: u64) -> DsrOrchestrator<TestClock> {
        DsrOrchestrator::new(TestClock::at(t0))
    }

    #[test]
    fn happy_path_is_received_validated_fannedout_awaiting_verified_completed() {
        let o = orch_at(1000);
        let id = o.dsr_submit(
            DsrKind::Access,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller,
            Initiator::Myelin,
        );
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Received);
        assert!(o.validate(&id).unwrap(), "controller access is admitted");
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Validated);
        o.fan_out(&id, &Inventory::default()).unwrap();
        assert_eq!(o.state_of(&id).unwrap(), DsrState::AwaitingHolders);
        o.verify(&id, vec!["receipt-1".into()]).unwrap();
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Verified);
        o.complete(&id).unwrap();
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Completed);
    }

    #[test]
    fn awaiting_holders_cannot_be_skipped_verified_only_reachable_from_awaiting() {
        assert!(DsrState::AwaitingHolders.can_transition_to(DsrState::Verified));
        assert!(!DsrState::FannedOut.can_transition_to(DsrState::Verified));
        assert!(!DsrState::Validated.can_transition_to(DsrState::Verified));
        assert!(!DsrState::Received.can_transition_to(DsrState::Verified));
        assert!(!DsrState::Received.can_transition_to(DsrState::Completed));
    }

    #[test]
    fn transition_guard_is_total_terminal_states_have_no_outgoing_edges() {
        use DsrState::*;
        let all = [
            Received,
            Validated,
            FannedOut,
            AwaitingHolders,
            Verified,
            Completed,
            Refused,
            Failed,
        ];
        for &t in &[Completed, Refused, Failed] {
            assert!(t.is_terminal());
            for &n in &all {
                assert!(
                    !t.can_transition_to(n),
                    "{} → {} must be illegal",
                    t.as_str(),
                    n.as_str()
                );
            }
        }
        for &s in &[Received, Validated, FannedOut, AwaitingHolders, Verified] {
            assert!(!s.is_terminal(), "{} is non-terminal", s.as_str());
            assert!(s.can_transition_to(Failed), "{} can fail", s.as_str());
        }
        assert_eq!(Received.as_str(), "received");
        assert_eq!(AwaitingHolders.as_str(), "awaiting-holders");
        assert_eq!(Refused.as_str(), "refused");
        assert_eq!(Completed.as_str(), "completed");
        assert_eq!(Failed.as_str(), "failed");
    }

    #[test]
    fn fail_is_a_legal_terminal_off_ramp_from_any_non_terminal_state() {
        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Erasure,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller,
            Initiator::Myelin,
        );
        o.validate(&id).unwrap();
        o.fan_out(&id, &Inventory::default()).unwrap();
        o.fail(&id).unwrap();
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Failed);
        assert!(o.complete(&id).is_err());
    }

    #[test]
    fn submitted_dsr_ids_are_distinct_monotonic_ordinals() {
        let o = orch_at(0);
        let a = o.dsr_submit(
            DsrKind::Access,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller,
            Initiator::Myelin,
        );
        let b = o.dsr_submit(
            DsrKind::Access,
            tenant(),
            subject("p2"),
            subject_scope("p2"),
            Posture::Controller,
            Initiator::Myelin,
        );
        assert_ne!(
            a, b,
            "each submit mints a distinct id (the ordinal advances)"
        );
        assert_eq!(a, DsrId("dsr:0".into()));
        assert_eq!(b, DsrId("dsr:1".into()));
    }

    #[test]
    fn an_illegal_transition_is_a_loud_typed_error_never_a_silent_skip() {
        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Access,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller,
            Initiator::Myelin,
        );
        let err = o.verify(&id, vec![]).unwrap_err();
        assert_eq!(
            err,
            DsrError::IllegalTransition {
                from: DsrState::Received,
                to: DsrState::Verified
            }
        );
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Received);
    }

    #[test]
    fn posture_gate_refuses_a_myelin_initiated_erase_of_tenant_content() {
        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Erasure,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Processor,
            Initiator::Myelin,
        );
        assert!(!o.validate(&id).unwrap(), "the posture gate REFUSES it");
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Refused);
        assert!(o.fan_out(&id, &Inventory::default()).is_err());
    }

    #[test]
    fn posture_gate_admits_a_tenant_instructed_erase_of_tenant_content() {
        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Erasure,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Processor,
            Initiator::TenantInstructed,
        );
        assert!(
            o.validate(&id).unwrap(),
            "a tenant-instructed erase is ADMITTED"
        );
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Validated);
    }

    #[test]
    fn posture_gate_admits_a_tenant_offboarding_erase() {
        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Erasure,
            tenant(),
            subject("p1"),
            EraseScope::Tenant(tenant()),
            Posture::Processor,
            Initiator::Myelin,
        );
        assert!(
            o.validate(&id).unwrap(),
            "a tenant offboarding is an authorised erase"
        );
        assert_eq!(o.state_of(&id).unwrap(), DsrState::Validated);
    }

    #[test]
    fn posture_gate_admits_a_controller_posture_erase() {
        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Erasure,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller,
            Initiator::Myelin,
        );
        assert!(
            o.validate(&id).unwrap(),
            "a controller-posture erase is admitted"
        );
    }

    #[test]
    fn posture_gate_never_refuses_a_read_right_even_under_the_processor_posture() {
        for kind in [DsrKind::Access, DsrKind::Portability] {
            let o = orch_at(0);
            let id = o.dsr_submit(
                kind,
                tenant(),
                subject("p1"),
                subject_scope("p1"),
                Posture::Processor,
                Initiator::Myelin,
            );
            assert!(
                o.validate(&id).unwrap(),
                "{kind:?} proceeds under the processor posture"
            );
        }
    }

    #[test]
    fn posture_from_data_role_is_the_x5_anchor() {
        assert_eq!(
            Posture::from_data_role(DataRole::TenantContent),
            Posture::Processor
        );
        assert_eq!(
            Posture::from_data_role(DataRole::PlatformOperational),
            Posture::Controller
        );
    }

    #[test]
    fn dsr_submit_sets_the_deadline_to_now_plus_one_month() {
        let o = orch_at(1_700_000_000);
        let id = o.dsr_submit(
            DsrKind::Access,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller,
            Initiator::Myelin,
        );
        let status = o.dsr_status(&id).unwrap();
        assert_eq!(status.deadline_secs, 1_700_000_000 + DSR_DEADLINE_SECS);
        assert_eq!(status.deadline_secs - 1_700_000_000, 30 * 24 * 60 * 60);
    }

    #[test]
    fn fan_out_resolves_the_checklist_from_the_map_not_a_hardcoded_list() {
        use crate::datamap::{Inventory, InventoryEntry};
        use std::collections::BTreeSet;

        let mut holders = BTreeSet::new();
        holders.insert("oltp:identity_oltp".to_string());
        holders.insert("search_index:search_index".to_string());
        let inv = Inventory {
            entries: vec![InventoryEntry {
                field_path: "PrincipalRow.email".into(),
                holder_id: "oltp:identity_oltp".into(),
                holder: "H15".into(),
                region: "fr-par".into(),
                category: "ContactInfo".into(),
                role: "PlatformOperational".into(),
                basis: "Contract".into(),
                retention: "UntilContractEnd".into(),
                erasure: "CryptoShred(subject_dek)".into(),
                subject_locator: "principal_id".into(),
            }],
            holders,
            dpia_markers: BTreeSet::new(),
        };

        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Erasure,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller,
            Initiator::Myelin,
        );
        o.validate(&id).unwrap();
        let checklist = o.fan_out(&id, &inv).unwrap();

        assert_eq!(checklist.len(), 2);
        let ids: Vec<&str> = checklist.iter().map(|c| c.holder_id.as_str()).collect();
        assert!(ids.contains(&"oltp:identity_oltp"));
        assert!(ids.contains(&"search_index:search_index"));
        let identity = checklist
            .iter()
            .find(|c| c.holder_id == "oltp:identity_oltp")
            .unwrap();
        assert_eq!(
            identity.field_mechanisms,
            vec!["PrincipalRow.email::CryptoShred(subject_dek)"]
        );
        assert_eq!(o.dsr_status(&id).unwrap().checklist, checklist);
    }

    #[test]
    fn dsr_certificate_is_content_addressed_and_not_ready_before_verified() {
        let o = orch_at(0);
        let id = o.dsr_submit(
            DsrKind::Access,
            tenant(),
            subject("p1"),
            subject_scope("p1"),
            Posture::Controller,
            Initiator::Myelin,
        );
        assert_eq!(
            o.dsr_certificate(&id).unwrap_err(),
            DsrError::CertificateNotReady(DsrState::Received)
        );
        o.validate(&id).unwrap();
        o.fan_out(&id, &Inventory::default()).unwrap();
        o.verify(&id, vec!["receipt-a".into(), "receipt-b".into()])
            .unwrap();
        let cert = o.dsr_certificate(&id).unwrap();
        assert_eq!(cert.dsr_id, id);
        assert_eq!(
            cert.receipts,
            vec!["receipt-a".to_string(), "receipt-b".to_string()]
        );
        assert!(cert.bundle_digest.starts_with("blake3:"));
        assert!(
            cert.merkle_inclusion.is_none(),
            "the Merkle seal is P-GA-20"
        );
        let cert2 = o.dsr_certificate(&id).unwrap();
        assert_eq!(cert.bundle_digest, cert2.bundle_digest);
    }

    #[test]
    fn unknown_dsr_id_is_a_loud_error_never_a_silent_empty() {
        let o = orch_at(0);
        let ghost = DsrId("dsr:999".into());
        assert_eq!(
            o.dsr_status(&ghost).unwrap_err(),
            DsrError::UnknownDsr(ghost.clone())
        );
        assert_eq!(
            o.dsr_certificate(&ghost).unwrap_err(),
            DsrError::UnknownDsr(ghost)
        );
    }
}
