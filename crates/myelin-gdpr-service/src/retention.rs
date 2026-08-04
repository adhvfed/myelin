use core::time::Duration;

use myelin_gdpr::{EraseScope, RetentionClass};

use crate::fanout::{HoldVerdict, LegalHoldRegistry};
use crate::orchestration::{HolderReceipt, UpstreamHolderOrchestrator};

pub const RETENTION_HELD_SCOPE_DELETIONS: (&str, &str) =
    ("gdpr.retention_held_scope_deletions", "count");

pub const RETENTION_EXPIRY_RUNS: (&str, &str) = ("gdpr.retention_expiry_runs", "count");

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetentionSource {
    TenantPolicy,
    PlatformDefault,
    LegalFloor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionInput {
    pub policy: RetentionClass,
    pub source: RetentionSource,
}

impl RetentionInput {
    pub fn new(policy: RetentionClass, source: RetentionSource) -> RetentionInput {
        RetentionInput { policy, source }
    }

    pub fn window_secs(&self) -> u64 {
        match &self.policy {
            RetentionClass::Fixed(d) | RetentionClass::AuditCarveOut(d) => d.as_secs(),
            RetentionClass::TenantPolicy => 0,
            RetentionClass::UntilContractEnd => u64::MAX,
        }
    }

    pub fn is_legal_floor(&self) -> bool {
        self.source == RetentionSource::LegalFloor
            || matches!(self.policy, RetentionClass::AuditCarveOut(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveRetention {
    pub window_secs: u64,
    pub winning_source: RetentionSource,
    pub floor_clamped: bool,
}

impl EffectiveRetention {
    pub fn window_secs(&self) -> u64 {
        self.window_secs
    }

    pub fn has_elapsed(&self, stored_at_secs: u64, now_secs: u64) -> bool {
        if self.window_secs == u64::MAX {
            return false;
        }
        now_secs.saturating_sub(stored_at_secs) >= self.window_secs
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpiryOutcome {
    Expired(Vec<HolderReceipt>),
    DeferredUnderHold,
}

impl ExpiryOutcome {
    pub fn ran_deletion(&self) -> bool {
        matches!(self, ExpiryOutcome::Expired(_))
    }
}

pub struct RetentionEngine<'a> {
    holds: &'a LegalHoldRegistry,
}

impl<'a> RetentionEngine<'a> {
    pub fn new(holds: &'a LegalHoldRegistry) -> RetentionEngine<'a> {
        RetentionEngine { holds }
    }

    pub fn effective_retention(&self, inputs: &[RetentionInput]) -> EffectiveRetention {
        let longest_floor: Option<u64> = inputs
            .iter()
            .filter(|i| i.is_legal_floor())
            .map(RetentionInput::window_secs)
            .max();

        let most_restrictive: Option<&RetentionInput> = inputs
            .iter()
            .filter(|i| !i.is_legal_floor())
            .min_by(|a, b| {
                a.window_secs()
                    .cmp(&b.window_secs())
                    .then(a.source.cmp(&b.source))
            });

        match (most_restrictive, longest_floor) {
            (None, None) => EffectiveRetention {
                window_secs: u64::MAX,
                winning_source: RetentionSource::PlatformDefault,
                floor_clamped: false,
            },
            (None, Some(floor)) => EffectiveRetention {
                window_secs: floor,
                winning_source: RetentionSource::LegalFloor,
                floor_clamped: true,
            },
            (Some(pick), None) => EffectiveRetention {
                window_secs: pick.window_secs(),
                winning_source: pick.source,
                floor_clamped: false,
            },
            (Some(pick), Some(floor)) => {
                let pick_window = pick.window_secs();
                if floor > pick_window {
                    EffectiveRetention {
                        window_secs: floor,
                        winning_source: RetentionSource::LegalFloor,
                        floor_clamped: true,
                    }
                } else {
                    EffectiveRetention {
                        window_secs: pick_window,
                        winning_source: pick.source,
                        floor_clamped: false,
                    }
                }
            }
        }
    }

    pub fn expire(
        &self,
        scope: &EraseScope,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &crate::orchestration::EraseChecklist,
    ) -> Result<ExpiryOutcome, ExpiryError> {
        match self.holds.verdict(crate::dsr::DsrKind::Erasure, scope) {
            HoldVerdict::Deferred => Ok(ExpiryOutcome::DeferredUnderHold),
            HoldVerdict::Proceed => {
                let receipts = upstream
                    .fan_out_erase(scope, checklist)
                    .map_err(|e| ExpiryError::HolderFanOut(e.0))?;
                Ok(ExpiryOutcome::Expired(receipts))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpiryError {
    HolderFanOut(String),
}

impl core::fmt::Display for ExpiryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ExpiryError::HolderFanOut(e) => {
                write!(f, "retention-expiry holder fan-out failed: {e}")
            }
        }
    }
}

impl std::error::Error for ExpiryError {}

pub fn platform_default(d: Duration) -> RetentionInput {
    RetentionInput::new(RetentionClass::Fixed(d), RetentionSource::PlatformDefault)
}

pub fn tenant_window(d: Duration) -> RetentionInput {
    RetentionInput::new(RetentionClass::Fixed(d), RetentionSource::TenantPolicy)
}

pub fn tenant_delete_immediately() -> RetentionInput {
    RetentionInput::new(RetentionClass::TenantPolicy, RetentionSource::TenantPolicy)
}

pub fn legal_floor(d: Duration) -> RetentionInput {
    RetentionInput::new(
        RetentionClass::AuditCarveOut(d),
        RetentionSource::LegalFloor,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fanout::HoldScope;
    use crate::holders::{InMemoryShredKms, ShredKeyClass, ShredKeyHandle};
    use crate::orchestration::{
        holder_ids, EraseChecklist, SeamHolder, UpstreamHolderOrchestrator,
    };
    use myelin_gdpr::{PersonalDataHolder, SubjectRef, TenantId};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    const DAY: u64 = 24 * 60 * 60;

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

    fn upstream_over<'a>(
        holders: &'a [(&'static str, SeamHolder<'a>)],
    ) -> UpstreamHolderOrchestrator<'a> {
        UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        )
    }

    #[test]
    fn tenant_30_days_beats_platform_default_90_days() {
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let eff = engine.effective_retention(&[
            platform_default(Duration::from_secs(90 * DAY)),
            tenant_window(Duration::from_secs(30 * DAY)),
        ]);
        assert_eq!(
            eff.window_secs(),
            30 * DAY,
            "the tenant's 30 days (most restrictive) wins"
        );
        assert_eq!(
            eff.winning_source,
            RetentionSource::TenantPolicy,
            "recorded: the tenant won"
        );
        assert!(!eff.floor_clamped, "no floor involved");
    }

    #[test]
    fn legal_6_month_floor_overrides_tenant_delete_immediately() {
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let six_months = 180 * DAY;
        let eff = engine.effective_retention(&[
            tenant_delete_immediately(),
            legal_floor(Duration::from_secs(six_months)),
        ]);
        assert_eq!(
            eff.window_secs(),
            six_months,
            "the legal floor clamps the effective window UP"
        );
        assert_eq!(
            eff.winning_source,
            RetentionSource::LegalFloor,
            "recorded: the floor won"
        );
        assert!(
            eff.floor_clamped,
            "the floor clamped the tenant delete-immediately UP"
        );
    }

    #[test]
    fn a_floor_shorter_than_the_tenant_window_does_not_clamp() {
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let eff = engine.effective_retention(&[
            tenant_window(Duration::from_secs(365 * DAY)),
            legal_floor(Duration::from_secs(30 * DAY)),
        ]);
        assert_eq!(
            eff.window_secs(),
            365 * DAY,
            "the tenant year exceeds the floor - tenant wins"
        );
        assert_eq!(eff.winning_source, RetentionSource::TenantPolicy);
        assert!(
            !eff.floor_clamped,
            "the floor did not clamp (tenant > floor)"
        );
    }

    #[test]
    fn no_inputs_retain_never_auto_delete() {
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let eff = engine.effective_retention(&[]);
        assert_eq!(
            eff.window_secs(),
            u64::MAX,
            "open-ended - retain, never auto-delete"
        );
        assert!(
            !eff.has_elapsed(0, u64::MAX),
            "an open-ended window never elapses"
        );
    }

    #[test]
    fn equal_length_tie_breaks_toward_the_tenant() {
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let eff = engine.effective_retention(&[
            platform_default(Duration::from_secs(30 * DAY)),
            tenant_window(Duration::from_secs(30 * DAY)),
        ]);
        assert_eq!(eff.window_secs(), 30 * DAY);
        assert_eq!(
            eff.winning_source,
            RetentionSource::TenantPolicy,
            "tie → the tenant won"
        );
    }

    #[test]
    fn has_elapsed_is_a_deterministic_window_check() {
        let eff = EffectiveRetention {
            window_secs: 30 * DAY,
            winning_source: RetentionSource::TenantPolicy,
            floor_clamped: false,
        };
        assert!(
            !eff.has_elapsed(1000, 1000 + 29 * DAY),
            "29 days < 30-day window - not elapsed"
        );
        assert!(
            eff.has_elapsed(1000, 1000 + 30 * DAY),
            "30 days reaches the window - elapsed"
        );
    }

    #[test]
    fn a_legal_hold_suspends_expiry_and_resumes_on_lift() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 100);
        let holders = seam_holders(&kms);
        let upstream = upstream_over(&holders);
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);

        holds.set(
            HoldScope::Subject {
                tenant: "acme".into(),
                subject: "u-held".into(),
            },
            true,
        );
        let scope = subject_scope("u-held");
        let checklist = EraseChecklist::new();

        let outcome = engine.expire(&scope, &upstream, &checklist).unwrap();
        assert_eq!(
            outcome,
            ExpiryOutcome::DeferredUnderHold,
            "suspend-don't-delete under the hold"
        );
        assert!(!outcome.ran_deletion(), "0 held-scope deletions");
        assert_eq!(checklist.done_count(), 0, "no holder driven under the hold");
        for (_, h) in &holders {
            assert_eq!(
                h.erase_call_count(),
                0,
                "0 held-scope deletions - no holder called"
            );
        }

        holds.set(
            HoldScope::Subject {
                tenant: "acme".into(),
                subject: "u-held".into(),
            },
            false,
        );
        let outcome2 = engine.expire(&scope, &upstream, &checklist).unwrap();
        assert!(
            outcome2.ran_deletion(),
            "the deferred deletion resumes on hold-lift"
        );
        let receipts = match outcome2 {
            ExpiryOutcome::Expired(r) => r,
            other => panic!("expected Expired on resume, got {other:?}"),
        };
        assert_eq!(
            receipts.len(),
            6,
            "every holder fanned on resume (the §3 mechanisms)"
        );
        assert_eq!(
            receipts[0].holder_id,
            holder_ids::IDENTITY,
            "Identity FIRST (canonical order)"
        );
        for hr in &receipts {
            assert!(hr.receipt.receipt.key_epoch_destroyed.is_some());
        }
    }

    #[test]
    fn an_unheld_expiry_runs_the_section_3_erasure_mechanisms() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 200);
        let holders = seam_holders(&kms);
        let upstream = upstream_over(&holders);
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);

        let outcome = engine
            .expire(
                &subject_scope("u-expire"),
                &upstream,
                &EraseChecklist::new(),
            )
            .unwrap();
        assert!(outcome.ran_deletion());
        let receipts = match outcome {
            ExpiryOutcome::Expired(r) => r,
            other => panic!("expected Expired, got {other:?}"),
        };
        assert_eq!(receipts.len(), 6, "every holder expired in canonical order");
    }

    #[test]
    fn a_tenant_hold_suspends_a_subject_expiry() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 300);
        let holders = seam_holders(&kms);
        let upstream = upstream_over(&holders);
        let holds = LegalHoldRegistry::new();
        holds.set(HoldScope::Tenant("acme".into()), true);
        let engine = RetentionEngine::new(&holds);

        let outcome = engine
            .expire(&subject_scope("anyone"), &upstream, &EraseChecklist::new())
            .unwrap();
        assert_eq!(
            outcome,
            ExpiryOutcome::DeferredUnderHold,
            "the tenant hold suspends the expiry"
        );
        for (_, h) in &holders {
            assert_eq!(h.erase_call_count(), 0, "0 held-scope deletions");
        }
    }

    #[test]
    fn an_unreadable_hold_registry_fails_safe_to_suspend_expiry() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 400);
        let holders = seam_holders(&kms);
        let upstream = upstream_over(&holders);
        let holds = LegalHoldRegistry::new();
        holds.set_unreadable(true);
        let engine = RetentionEngine::new(&holds);

        let outcome = engine
            .expire(&subject_scope("x"), &upstream, &EraseChecklist::new())
            .unwrap();
        assert_eq!(
            outcome,
            ExpiryOutcome::DeferredUnderHold,
            "an un-readable registry fails safe to suspend (never auto-deletes)"
        );
        for (_, h) in &holders {
            assert_eq!(
                h.erase_call_count(),
                0,
                "fail-safe-to-suspend - no holder called"
            );
        }
    }

    #[test]
    fn is_legal_floor_is_an_or_either_condition_alone_is_a_floor() {
        let by_source = RetentionInput::new(
            RetentionClass::Fixed(Duration::from_secs(180 * DAY)),
            RetentionSource::LegalFloor,
        );
        assert!(
            by_source.is_legal_floor(),
            "LegalFloor source alone makes it a floor"
        );
        let by_policy = RetentionInput::new(
            RetentionClass::AuditCarveOut(Duration::from_secs(180 * DAY)),
            RetentionSource::PlatformDefault,
        );
        assert!(
            by_policy.is_legal_floor(),
            "AuditCarveOut policy alone makes it a floor"
        );

        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let eff = engine.effective_retention(&[tenant_delete_immediately(), by_source]);
        assert_eq!(
            eff.window_secs(),
            180 * DAY,
            "the source-named floor clamps the tenant UP"
        );
        assert_eq!(eff.winning_source, RetentionSource::LegalFloor);
    }

    #[test]
    fn a_floor_equal_to_the_pick_does_not_clamp_the_pick_wins() {
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let thirty = 30 * DAY;
        let eff = engine.effective_retention(&[
            tenant_window(Duration::from_secs(thirty)),
            legal_floor(Duration::from_secs(thirty)),
        ]);
        assert_eq!(
            eff.window_secs(),
            thirty,
            "equal windows - the window is the same either way"
        );
        assert_eq!(
            eff.winning_source,
            RetentionSource::TenantPolicy,
            "at floor == pick the TENANT wins (the clamp is strict `>`, not `>=`)"
        );
        assert!(
            !eff.floor_clamped,
            "the floor did not clamp (it equals, not exceeds, the pick)"
        );
    }

    #[test]
    fn only_floors_yield_the_longest_floor() {
        let holds = LegalHoldRegistry::new();
        let engine = RetentionEngine::new(&holds);
        let eff = engine.effective_retention(&[
            legal_floor(Duration::from_secs(30 * DAY)),
            legal_floor(Duration::from_secs(180 * DAY)),
        ]);
        assert_eq!(
            eff.window_secs(),
            180 * DAY,
            "the longest floor is the lawful minimum"
        );
        assert_eq!(eff.winning_source, RetentionSource::LegalFloor);
        assert!(eff.floor_clamped);
    }
}
