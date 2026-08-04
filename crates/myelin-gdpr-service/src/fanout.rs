use std::collections::BTreeSet;
use std::sync::Mutex;

use myelin_gdpr::EraseScope;
use myelin_substrate::Clock;

use crate::dsr::{DsrError, DsrId, DsrKind, DsrOrchestrator, DsrState, Result};
use crate::orchestration::{EraseChecklist, HolderReceipt, UpstreamHolderOrchestrator};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HoldScope {
    Tenant(String),
    Subject {
        tenant: String,
        subject: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldVerdict {
    Proceed,
    Deferred,
}

#[derive(Default)]
pub struct LegalHoldRegistry {
    holds: Mutex<BTreeSet<HoldScope>>,
    unreadable: std::sync::atomic::AtomicBool,
}

impl LegalHoldRegistry {
    pub fn new() -> LegalHoldRegistry {
        LegalHoldRegistry::default()
    }

    pub fn set(&self, scope: HoldScope, on: bool) {
        let mut holds = self.holds.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            holds.insert(scope);
        } else {
            holds.remove(&scope);
        }
    }

    pub fn active_count(&self) -> usize {
        self.holds.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn set_unreadable(&self, unreadable: bool) {
        self.unreadable
            .store(unreadable, std::sync::atomic::Ordering::SeqCst);
    }

    fn poisoned(&self) -> bool {
        self.unreadable.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn scope_is_held(&self, scope: &EraseScope) -> bool {
        let holds = self.holds.lock().unwrap_or_else(|e| e.into_inner());
        match scope {
            EraseScope::Subject { subject, tenant } => {
                let tenant_token = tenant.0.clone();
                let subject_token = subject.principal.principal_id.0.clone();
                holds.contains(&HoldScope::Tenant(tenant_token.clone()))
                    || holds.contains(&HoldScope::Subject {
                        tenant: tenant_token,
                        subject: subject_token,
                    })
            }
            EraseScope::Tenant(tenant) => holds.contains(&HoldScope::Tenant(tenant.0.clone())),
        }
    }

    pub fn verdict(&self, kind: DsrKind, scope: &EraseScope) -> HoldVerdict {
        if !kind.is_erasure() {
            return HoldVerdict::Proceed;
        }
        if self.poisoned() {
            return HoldVerdict::Deferred;
        }
        if self.scope_is_held(scope) {
            HoldVerdict::Deferred
        } else {
            HoldVerdict::Proceed
        }
    }
}

pub const LEGAL_HOLD_ACTIVE_COUNT: (&str, &str) = ("gdpr.legal_hold_active_count", "count");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DsrCompletionReceipt {
    pub request_id: DsrId,
    pub scope_token: String,
    pub outcome: String,
    pub holder_receipts: Vec<HolderReceipt>,
    pub content_hash: String,
    pub completed_at_secs: u64,
}

impl DsrCompletionReceipt {
    fn build(
        request_id: &DsrId,
        scope_token: &str,
        outcome: &str,
        holder_receipts: &[HolderReceipt],
        completed_at_secs: u64,
    ) -> DsrCompletionReceipt {
        let mut body = format!(
            "request_id={}\u{1f}scope={scope_token}\u{1f}outcome={outcome}",
            request_id.0
        );
        for hr in holder_receipts {
            body.push('\u{1f}');
            body.push_str(&format!(
                "holder={}:{}:{}",
                hr.holder_id,
                hr.receipt.receipt.content_hash,
                match hr.receipt.receipt.key_epoch_destroyed {
                    Some(e) => e.to_string(),
                    None => "none".to_string(),
                }
            ));
        }
        body.push_str(&format!("\u{1f}timestamp={completed_at_secs}"));
        let digest = blake3::hash(body.as_bytes());
        DsrCompletionReceipt {
            request_id: request_id.clone(),
            scope_token: scope_token.to_string(),
            outcome: outcome.to_string(),
            holder_receipts: holder_receipts.to_vec(),
            content_hash: format!("blake3:{}", hex::encode(digest.as_bytes())),
            completed_at_secs,
        }
    }
}

fn scope_token(scope: &EraseScope) -> String {
    match scope {
        EraseScope::Subject { subject, tenant } => {
            format!("{}/{}", tenant.0, subject.principal.principal_id.0)
        }
        EraseScope::Tenant(tenant) => tenant.0.clone(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FanOutOutcome {
    Erased(DsrCompletionReceipt),
    DeferredUnderHold(DsrCompletionReceipt),
    ReadRightServed(DsrCompletionReceipt),
}

impl FanOutOutcome {
    pub fn receipt(&self) -> &DsrCompletionReceipt {
        match self {
            FanOutOutcome::Erased(r)
            | FanOutOutcome::DeferredUnderHold(r)
            | FanOutOutcome::ReadRightServed(r) => r,
        }
    }
}

pub struct FanOutDriver<'a, C: Clock> {
    dsr: &'a DsrOrchestrator<C>,
    holds: &'a LegalHoldRegistry,
    ledger: Option<&'a crate::erasure_ledger::ErasureLedger>,
}

impl<'a, C: Clock> FanOutDriver<'a, C> {
    pub fn new(dsr: &'a DsrOrchestrator<C>, holds: &'a LegalHoldRegistry) -> FanOutDriver<'a, C> {
        FanOutDriver {
            dsr,
            holds,
            ledger: None,
        }
    }

    pub fn with_ledger(
        dsr: &'a DsrOrchestrator<C>,
        holds: &'a LegalHoldRegistry,
        ledger: &'a crate::erasure_ledger::ErasureLedger,
    ) -> FanOutDriver<'a, C> {
        FanOutDriver {
            dsr,
            holds,
            ledger: Some(ledger),
        }
    }

    pub fn drive(
        &self,
        id: &DsrId,
        inventory: &crate::datamap::Inventory,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &EraseChecklist,
    ) -> Result<FanOutOutcome> {
        let req = self.dsr.request_view(id)?;
        let now = req.submitted_at_secs;

        if req.state == DsrState::Validated {
            self.dsr.fan_out(id, inventory)?;
        }

        match self.holds.verdict(req.kind, &req.scope) {
            HoldVerdict::Deferred => {
                let receipt = DsrCompletionReceipt::build(
                    id,
                    &scope_token(&req.scope),
                    "deferred:legal_hold",
                    &[],
                    now,
                );
                return Ok(FanOutOutcome::DeferredUnderHold(receipt));
            }
            HoldVerdict::Proceed => {}
        }

        if !req.kind.is_erasure() {
            self.dsr.verify(id, Vec::new())?;
            self.dsr.complete(id)?;
            let receipt = DsrCompletionReceipt::build(
                id,
                &scope_token(&req.scope),
                read_right_outcome(req.kind),
                &[],
                now,
            );
            return Ok(FanOutOutcome::ReadRightServed(receipt));
        }

        let holder_receipts = self.dsr_fan_out_erase(&req.scope, upstream, checklist)?;

        let receipt_strings: Vec<String> = holder_receipts
            .iter()
            .map(|hr| format!("{}:{}", hr.holder_id, hr.receipt.receipt.content_hash))
            .collect();
        if self.dsr.state_of(id)? == DsrState::AwaitingHolders {
            self.dsr.verify(id, receipt_strings)?;
        }
        if self.dsr.state_of(id)? == DsrState::Verified {
            self.dsr.complete(id)?;
        }

        let receipt = DsrCompletionReceipt::build(
            id,
            &scope_token(&req.scope),
            "erased",
            &holder_receipts,
            now,
        );

        if self.dsr.state_of(id)? == DsrState::Completed {
            self.write_erasure_ledger_entry(id, &req.scope, &holder_receipts, now);
        }

        Ok(FanOutOutcome::Erased(receipt))
    }

    fn write_erasure_ledger_entry(
        &self,
        id: &DsrId,
        scope: &EraseScope,
        holder_receipts: &[HolderReceipt],
        completed_at_secs: u64,
    ) {
        let Some(ledger) = self.ledger else { return };
        let (subject_token, tenant_token) = match scope {
            EraseScope::Subject { subject, tenant } => {
                (subject.principal.principal_id.0.clone(), tenant.0.clone())
            }
            EraseScope::Tenant(tenant) => ("*".to_string(), tenant.0.clone()),
        };
        let holders_erased: Vec<String> = holder_receipts
            .iter()
            .map(|hr| hr.holder_id.to_string())
            .collect();
        let key_epochs_destroyed: Vec<crate::erasure_ledger::DestroyedKeyEpoch> = holder_receipts
            .iter()
            .map(|hr| crate::erasure_ledger::DestroyedKeyEpoch {
                holder_id: hr.holder_id.to_string(),
                key_epoch_destroyed: hr.receipt.receipt.key_epoch_destroyed,
            })
            .collect();
        ledger.record_completion(
            id.clone(),
            subject_token,
            tenant_token,
            holders_erased,
            key_epochs_destroyed,
            completed_at_secs,
            completed_at_secs,
        );
    }

    fn dsr_fan_out_erase(
        &self,
        scope: &EraseScope,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &EraseChecklist,
    ) -> Result<Vec<HolderReceipt>> {
        upstream
            .fan_out_erase(scope, checklist)
            .map_err(|e| DsrError::HolderFanOut(e.0))
    }
}

fn read_right_outcome(kind: DsrKind) -> &'static str {
    match kind {
        DsrKind::Access => "access",
        DsrKind::Portability => "portability",
        DsrKind::Rectification => "rectification",
        DsrKind::Restriction => "restriction",
        DsrKind::Erasure => "erased",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holders::{InMemoryShredKms, ShredKeyClass, ShredKeyHandle};
    use crate::orchestration::{holder_ids, SeamHolder};
    use myelin_gdpr::{PersonalDataHolder, SubjectRef, TenantId};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::TestClock;

    use crate::datamap::{Inventory, InventoryEntry};
    use crate::dsr::{Initiator, Posture};

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            t("acme"),
        ))
    }

    fn subject_scope(s: &str) -> EraseScope {
        EraseScope::Subject {
            subject: subject(s),
            tenant: t("acme"),
        }
    }

    fn kms_with_all_holder_keys(tenant: &TenantId, base_epoch: u64) -> InMemoryShredKms {
        let kms = InMemoryShredKms::new();
        for (i, id) in [
            holder_ids::IDENTITY,
            holder_ids::BLOB,
            holder_ids::AUTHZ_TUPLES,
            holder_ids::BUS,
            holder_ids::CACHE,
            holder_ids::BACKUP,
        ]
        .iter()
        .enumerate()
        {
            kms.provision(
                ShredKeyHandle {
                    tenant: tenant.clone(),
                    class: ShredKeyClass::Subject((*id).to_string()),
                },
                base_epoch + i as u64,
            );
        }
        kms
    }

    fn seam_holders(kms: &InMemoryShredKms) -> Vec<(&'static str, SeamHolder<'_>)> {
        [
            holder_ids::IDENTITY,
            holder_ids::BLOB,
            holder_ids::AUTHZ_TUPLES,
            holder_ids::BUS,
            holder_ids::CACHE,
            holder_ids::BACKUP,
        ]
        .into_iter()
        .map(|id| {
            (
                id,
                SeamHolder::new(id, ShredKeyClass::Subject(id.to_string()), kms),
            )
        })
        .collect()
    }

    fn inventory() -> Inventory {
        let mut holders = BTreeSet::new();
        holders.insert("identity".to_string());
        holders.insert("search_index:search_index".to_string());
        Inventory {
            entries: vec![InventoryEntry {
                field_path: "PrincipalRow.email".into(),
                holder_id: "identity".into(),
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
        }
    }

    fn submit_validated_erase<C: Clock>(dsr: &DsrOrchestrator<C>, who: &str) -> DsrId {
        let id = dsr.dsr_submit(
            DsrKind::Erasure,
            t("acme"),
            subject(who),
            subject_scope(who),
            Posture::Controller,
            Initiator::Myelin,
        );
        assert!(dsr.validate(&id).unwrap(), "controller erase admitted");
        id
    }

    #[test]
    fn drive_fans_out_data_map_driven_and_seals_a_verifiable_receipt() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 100);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
        let holds = LegalHoldRegistry::new();
        let driver = FanOutDriver::new(&dsr, &holds);

        let id = submit_validated_erase(&dsr, "u-floor");
        let checklist = EraseChecklist::new();
        let outcome = driver
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();

        assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);
        let status_checklist = dsr.dsr_status(&id).unwrap().checklist;
        let ids: Vec<&str> = status_checklist
            .iter()
            .map(|c| c.holder_id.as_str())
            .collect();
        assert!(ids.contains(&"identity") && ids.contains(&"search_index:search_index"));

        assert_eq!(upstream.fanout_coverage(&checklist), 1.0);
        let receipt = match &outcome {
            FanOutOutcome::Erased(r) => r,
            other => panic!("expected Erased, got {other:?}"),
        };
        assert_eq!(
            receipt.holder_receipts.len(),
            6,
            "all six upstream holders receipted"
        );
        assert_eq!(
            receipt.holder_receipts[0].holder_id,
            holder_ids::IDENTITY,
            "Identity FIRST"
        );
        assert_eq!(receipt.outcome, "erased");
        assert!(
            receipt.content_hash.starts_with("blake3:"),
            "content-addressed (§4.2)"
        );
        for hr in &receipt.holder_receipts {
            assert!(hr.receipt.receipt.key_epoch_destroyed.is_some());
        }
        let cert = dsr.dsr_certificate(&id).unwrap();
        assert_eq!(cert.receipts.len(), 6);
        assert!(
            cert.merkle_inclusion.is_none(),
            "the Merkle seal is P-GA-20"
        );
    }

    #[test]
    fn checklist_is_resolved_from_the_map_a_new_map_holder_appears() {
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        let driver = FanOutDriver::new(&dsr, &holds);
        let kms = kms_with_all_holder_keys(&t("acme"), 10);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );

        let id_a = submit_validated_erase(&dsr, "u-a");
        driver
            .drive(&id_a, &inventory(), &upstream, &EraseChecklist::new())
            .unwrap();
        let a: BTreeSet<String> = dsr
            .dsr_status(&id_a)
            .unwrap()
            .checklist
            .iter()
            .map(|c| c.holder_id.clone())
            .collect();

        let mut inv_b = inventory();
        inv_b.holders.insert("refs_edge:refs_edge".to_string());
        let id_b = submit_validated_erase(&dsr, "u-b");
        driver
            .drive(&id_b, &inv_b, &upstream, &EraseChecklist::new())
            .unwrap();
        let b: BTreeSet<String> = dsr
            .dsr_status(&id_b)
            .unwrap()
            .checklist
            .iter()
            .map(|c| c.holder_id.clone())
            .collect();

        assert!(!a.contains("refs_edge:refs_edge"));
        assert!(
            b.contains("refs_edge:refs_edge"),
            "the new map holder appears in the checklist"
        );
    }

    #[test]
    fn legal_hold_defers_an_erase_and_does_not_fan_out() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 200);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        holds.set(
            HoldScope::Subject {
                tenant: "acme".into(),
                subject: "u-held".into(),
            },
            true,
        );
        assert_eq!(holds.active_count(), 1);
        let driver = FanOutDriver::new(&dsr, &holds);

        let id = submit_validated_erase(&dsr, "u-held");
        let checklist = EraseChecklist::new();
        let outcome = driver
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();

        assert!(
            matches!(outcome, FanOutOutcome::DeferredUnderHold(_)),
            "erase deferred under hold"
        );
        assert_eq!(outcome.receipt().outcome, "deferred:legal_hold");
        assert!(
            outcome.receipt().holder_receipts.is_empty(),
            "no holder was driven"
        );
        assert_eq!(
            dsr.state_of(&id).unwrap(),
            DsrState::AwaitingHolders,
            "parked, not completed"
        );
        assert_eq!(
            upstream.fanout_coverage(&checklist),
            0.0,
            "0 holders driven under hold"
        );

        holds.set(
            HoldScope::Subject {
                tenant: "acme".into(),
                subject: "u-held".into(),
            },
            false,
        );
        let outcome2 = driver
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();
        assert!(matches!(outcome2, FanOutOutcome::Erased(_)));
        assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);
        assert_eq!(upstream.fanout_coverage(&checklist), 1.0);
    }

    #[test]
    fn a_tenant_hold_defers_a_subject_erase_in_that_tenant() {
        let holds = LegalHoldRegistry::new();
        holds.set(HoldScope::Tenant("acme".into()), true);
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &subject_scope("anyone")),
            HoldVerdict::Deferred
        );
        let other = EraseScope::Subject {
            subject: subject("x"),
            tenant: t("other"),
        };
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &other),
            HoldVerdict::Proceed
        );
    }

    #[test]
    fn legal_hold_never_suspends_a_read_right() {
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        holds.set(HoldScope::Tenant("acme".into()), true);
        let driver = FanOutDriver::new(&dsr, &holds);
        let kms = kms_with_all_holder_keys(&t("acme"), 300);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );

        for (kind, want) in [
            (DsrKind::Access, "access"),
            (DsrKind::Portability, "portability"),
        ] {
            let id = dsr.dsr_submit(
                kind,
                t("acme"),
                subject("reader"),
                subject_scope("reader"),
                Posture::Controller,
                Initiator::Myelin,
            );
            dsr.validate(&id).unwrap();
            let outcome = driver
                .drive(&id, &inventory(), &upstream, &EraseChecklist::new())
                .unwrap();
            assert!(
                matches!(outcome, FanOutOutcome::ReadRightServed(_)),
                "{kind:?} proceeds under hold"
            );
            assert_eq!(outcome.receipt().outcome, want);
            assert_eq!(
                dsr.state_of(&id).unwrap(),
                DsrState::Completed,
                "{kind:?} completes"
            );
        }
    }

    #[test]
    fn an_unreadable_hold_registry_fails_safe_to_suspend_for_an_erase() {
        let holds = LegalHoldRegistry::new();
        holds.set_unreadable(true);
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &subject_scope("x")),
            HoldVerdict::Deferred
        );
        assert_eq!(
            holds.verdict(DsrKind::Access, &subject_scope("x")),
            HoldVerdict::Proceed
        );
    }

    #[test]
    fn drive_is_resumable_a_worker_kill_redrives_only_un_receipted_holders() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 400);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        let driver = FanOutDriver::new(&dsr, &holds);
        let id = submit_validated_erase(&dsr, "u-resume");
        let checklist = EraseChecklist::new();

        let first_three: Vec<(&'static str, &dyn PersonalDataHolder)> = holders
            .iter()
            .filter(|(id, _)| {
                *id == holder_ids::IDENTITY
                    || *id == holder_ids::BLOB
                    || *id == holder_ids::AUTHZ_TUPLES
            })
            .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
            .collect();
        let partial = UpstreamHolderOrchestrator::register_m1_upstream(first_three);
        partial
            .fan_out_erase(&subject_scope("u-resume"), &checklist)
            .unwrap();
        assert_eq!(
            checklist.done_count(),
            3,
            "the crash left three holders receipted"
        );
        let calls_after_partial: Vec<u32> =
            holders.iter().map(|(_, h)| h.erase_call_count()).collect();

        let outcome = driver
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();
        assert!(matches!(outcome, FanOutOutcome::Erased(_)));

        for (i, (id, _)) in holders.iter().enumerate() {
            if *id == holder_ids::IDENTITY
                || *id == holder_ids::BLOB
                || *id == holder_ids::AUTHZ_TUPLES
            {
                assert_eq!(
                    holders[i].1.erase_call_count(),
                    calls_after_partial[i],
                    "holder {id} was already receipted ⇒ NOT re-called (0 double-erase)"
                );
            } else {
                assert_eq!(
                    holders[i].1.erase_call_count(),
                    1,
                    "holder {id} driven on resume"
                );
            }
        }
        assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);
        assert_eq!(upstream.fanout_coverage(&checklist), 1.0);
    }

    #[test]
    fn re_driving_a_completed_dsr_is_idempotent() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 500);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(42));
        let holds = LegalHoldRegistry::new();
        let driver = FanOutDriver::new(&dsr, &holds);
        let id = submit_validated_erase(&dsr, "u-idem");
        let checklist = EraseChecklist::new();

        let first = driver
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();
        let calls_after_first: Vec<u32> =
            holders.iter().map(|(_, h)| h.erase_call_count()).collect();

        let second = driver
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();
        let calls_after_second: Vec<u32> =
            holders.iter().map(|(_, h)| h.erase_call_count()).collect();

        assert_eq!(
            first.receipt().content_hash,
            second.receipt().content_hash,
            "an idempotent re-drive seals the SAME content-addressed receipt"
        );
        assert_eq!(
            calls_after_first, calls_after_second,
            "no holder re-called on the idempotent re-drive"
        );
        assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);
    }

    #[test]
    fn completion_receipt_content_addresses_the_4_2_fields() {
        let id = DsrId("dsr:7".into());
        let hr = |holder: &'static str, epoch: Option<u64>| HolderReceipt {
            holder_id: holder,
            phase: crate::orchestration::CanonicalErasePhase::CryptoShredDek,
            receipt: myelin_gdpr::EraseReceipt {
                receipt: myelin_gdpr::Receipt::content_addressed(
                    "erase",
                    holder,
                    "u",
                    "acme",
                    "crypto_shred",
                    epoch,
                    0,
                ),
            },
        };
        let base = DsrCompletionReceipt::build(
            &id,
            "acme/u",
            "erased",
            &[hr("blob_store", Some(9))],
            1000,
        );
        let same = DsrCompletionReceipt::build(
            &id,
            "acme/u",
            "erased",
            &[hr("blob_store", Some(9))],
            1000,
        );
        assert_eq!(base.content_hash, same.content_hash);
        assert!(base.content_hash.starts_with("blake3:"));

        let diff_id = DsrCompletionReceipt::build(
            &DsrId("dsr:8".into()),
            "acme/u",
            "erased",
            &[hr("blob_store", Some(9))],
            1000,
        );
        assert_ne!(
            base.content_hash, diff_id.content_hash,
            "request_id is in the content address"
        );
        let diff_scope = DsrCompletionReceipt::build(
            &id,
            "acme/v",
            "erased",
            &[hr("blob_store", Some(9))],
            1000,
        );
        assert_ne!(
            base.content_hash, diff_scope.content_hash,
            "scope is in the content address"
        );
        let diff_outcome = DsrCompletionReceipt::build(
            &id,
            "acme/u",
            "deferred:legal_hold",
            &[hr("blob_store", Some(9))],
            1000,
        );
        assert_ne!(
            base.content_hash, diff_outcome.content_hash,
            "outcome is in the content address"
        );
        let diff_epoch = DsrCompletionReceipt::build(
            &id,
            "acme/u",
            "erased",
            &[hr("blob_store", Some(10))],
            1000,
        );
        assert_ne!(
            base.content_hash, diff_epoch.content_hash,
            "key_epoch is in the content address"
        );
        let diff_ts = DsrCompletionReceipt::build(
            &id,
            "acme/u",
            "erased",
            &[hr("blob_store", Some(9))],
            2000,
        );
        assert_ne!(
            base.content_hash, diff_ts.content_hash,
            "timestamp is in the content address"
        );
        let diff_holder = DsrCompletionReceipt::build(
            &id,
            "acme/u",
            "erased",
            &[hr("blob_store", Some(9)), hr("event_bus", Some(11))],
            1000,
        );
        assert_ne!(
            base.content_hash, diff_holder.content_hash,
            "the holder set is in the content address"
        );
        assert_eq!(base.request_id, id);
        assert_eq!(base.scope_token, "acme/u");
        assert_eq!(base.outcome, "erased");
        assert_eq!(base.completed_at_secs, 1000);
    }

    #[test]
    fn scope_token_is_pii_free_tenant_or_tenant_subject() {
        assert_eq!(scope_token(&subject_scope("u1")), "acme/u1");
        assert_eq!(scope_token(&EraseScope::Tenant(t("acme"))), "acme");
    }

    #[test]
    fn a_completed_erase_writes_the_erasure_ledger_idempotently() {
        use crate::erasure_ledger::ErasureLedger;

        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 600);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
        let holds = LegalHoldRegistry::new();
        let ledger = ErasureLedger::new();
        let driver = FanOutDriver::with_ledger(&dsr, &holds, &ledger);

        let id = submit_validated_erase(&dsr, "u-ledger");
        let checklist = EraseChecklist::new();
        let outcome = driver
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();
        assert!(matches!(outcome, FanOutOutcome::Erased(_)));
        assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);

        assert_eq!(ledger.len(), 1, "the completion wrote one ledger entry");
        let entry = ledger.entry(&id).unwrap();
        assert_eq!(
            entry.subject_token, "u-ledger",
            "the opaque subject token (principal_id), never PII"
        );
        assert_eq!(entry.tenant_token, "acme");
        assert_eq!(
            entry.holders_erased.len(),
            6,
            "all six driven holders recorded"
        );
        assert!(entry.erased_holder(holder_ids::IDENTITY));
        for ke in &entry.key_epochs_destroyed {
            assert!(
                ke.key_epoch_destroyed.is_some(),
                "each holder's destroyed key epoch is recorded"
            );
        }
        assert_eq!(entry.completed_at_offset, 1_700_000_000);
        let post_pit = ledger.post_pit_records_after(1_699_999_999);
        assert_eq!(
            post_pit.len(),
            1,
            "a restore before the completion re-erases this subject"
        );
        assert_eq!(post_pit[0].subject, "u-ledger");
        assert!(ledger.post_pit_records_after(1_700_000_000).is_empty());

        let driver2 = FanOutDriver::with_ledger(&dsr, &holds, &ledger);
        driver2
            .drive(&id, &inventory(), &upstream, &checklist)
            .unwrap();
        assert_eq!(
            ledger.len(),
            1,
            "a resume does NOT duplicate the ledger entry"
        );
    }

    #[test]
    fn a_deferred_erase_writes_no_ledger_entry() {
        use crate::erasure_ledger::ErasureLedger;

        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 700);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        holds.set(
            HoldScope::Subject {
                tenant: "acme".into(),
                subject: "u-held".into(),
            },
            true,
        );
        let ledger = ErasureLedger::new();
        let driver = FanOutDriver::with_ledger(&dsr, &holds, &ledger);

        let id = submit_validated_erase(&dsr, "u-held");
        let outcome = driver
            .drive(&id, &inventory(), &upstream, &EraseChecklist::new())
            .unwrap();
        assert!(matches!(outcome, FanOutOutcome::DeferredUnderHold(_)));
        assert!(
            ledger.is_empty(),
            "a deferred erase writes NO ledger entry (it did not complete)"
        );
    }

    #[test]
    fn legal_hold_telemetry_name_and_unit_are_pinned() {
        assert_eq!(LEGAL_HOLD_ACTIVE_COUNT.0, "gdpr.legal_hold_active_count");
        assert_eq!(LEGAL_HOLD_ACTIVE_COUNT.1, "count");
    }

    #[test]
    fn a_hold_is_reversible_set_then_clear() {
        let holds = LegalHoldRegistry::new();
        let s = HoldScope::Subject {
            tenant: "acme".into(),
            subject: "u".into(),
        };
        holds.set(s.clone(), true);
        assert_eq!(holds.active_count(), 1);
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &subject_scope("u")),
            HoldVerdict::Deferred
        );
        holds.set(s, false);
        assert_eq!(holds.active_count(), 0);
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &subject_scope("u")),
            HoldVerdict::Proceed
        );
    }
}
