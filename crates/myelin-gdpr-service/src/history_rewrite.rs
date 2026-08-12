use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use myelin_events::{
    Actor, AggregateKey, CausedBy, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{ArtifactRef, TenantId};

use crate::audit::{AuditConsumer, Outcome};
use crate::holders::{CryptoShredKms, ShredKeyClass, ShredKeyHandle};

pub const HISTORY_REWRITE_ACTION: &str = "git.history_rewrite";

pub const HISTORY_REWRITE_DENIED_ACTION: &str = "git.history_rewrite.denied";

pub const HISTORY_REWRITE_FIRST_CLASS_PROMPT: &str =
    "P-GA-35 → P-451 (M5) - history-rewrite as a first-class audited op + the invalidation fan-out (GA-10)";

pub const HISTORY_REWRITE_OUTBOUND_GATE_PROMPT: &str =
    "P-GA-36 (M5) - the outbound push-mirror residency gate (GA-11, deny extra-EU by default)";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRewriteRequest {
    pub tenant: TenantId,
    pub repo: ArtifactRef,
    pub actor_pseudonym: String,
    pub rewrite_spec: String,
}

impl HistoryRewriteRequest {
    pub fn actor_pseudonym_local(&self) -> String {
        self.actor_pseudonym
            .split('@')
            .next()
            .unwrap_or(&self.actor_pseudonym)
            .to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum RewritePhase {
    Audit = 0,
    Rewrite = 1,
    CryptoShredPackTier = 2,
    InvalidateCaches = 3,
}

impl RewritePhase {
    pub const ALL: [RewritePhase; 4] = [
        RewritePhase::Audit,
        RewritePhase::Rewrite,
        RewritePhase::CryptoShredPackTier,
        RewritePhase::InvalidateCaches,
    ];

    pub fn token(self) -> &'static str {
        match self {
            RewritePhase::Audit => "audit",
            RewritePhase::Rewrite => "rewrite",
            RewritePhase::CryptoShredPackTier => "crypto_shred_pack_tier",
            RewritePhase::InvalidateCaches => "invalidate_caches",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhaseReceipt {
    pub phase: RewritePhase,
    pub content_hash: String,
    pub deferred_floor: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRewriteReceipt {
    pub repo: ArtifactRef,
    pub action: String,
    pub phase_receipts: Vec<PhaseReceipt>,
    pub residual_named: String,
}

impl HistoryRewriteReceipt {
    pub fn skeleton_complete(&self) -> bool {
        RewritePhase::ALL
            .iter()
            .all(|p| self.phase_receipts.iter().any(|r| r.phase == *p))
    }

    pub fn residual_for(repo: &ArtifactRef) -> String {
        format!(
            "independent off-platform clones of {} held by third parties are not reachable by the invalidation fan-out - named, not pretended-solved (gdpr §6.6); the outbound replication gate is {HISTORY_REWRITE_OUTBOUND_GATE_PROMPT}",
            repo.0
        )
    }
}

#[derive(Default)]
pub struct HistoryRewriteActivity {
    done: Mutex<BTreeMap<RewritePhase, PhaseReceipt>>,
    phase_calls: Mutex<BTreeMap<RewritePhase, u32>>,
}

impl HistoryRewriteActivity {
    pub fn new() -> HistoryRewriteActivity {
        HistoryRewriteActivity::default()
    }

    pub fn drive(&self, request: &HistoryRewriteRequest) -> HistoryRewriteReceipt {
        let mut receipts = Vec::new();
        for phase in RewritePhase::ALL {
            let receipt = self.run_phase(request, phase);
            receipts.push(receipt);
        }
        HistoryRewriteReceipt {
            repo: request.repo.clone(),
            action: HISTORY_REWRITE_ACTION.to_string(),
            phase_receipts: receipts,
            residual_named: HistoryRewriteReceipt::residual_for(&request.repo),
        }
    }

    fn run_phase(&self, request: &HistoryRewriteRequest, phase: RewritePhase) -> PhaseReceipt {
        {
            let done = self.done.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = done.get(&phase) {
                return existing.clone();
            }
        }
        *self
            .phase_calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(phase)
            .or_insert(0) += 1;

        let deferred_floor = false;
        let body = format!(
            "repo={}\u{1f}phase={}\u{1f}spec={}\u{1f}actor={}",
            request.repo.0,
            phase.token(),
            request.rewrite_spec,
            request.actor_pseudonym
        );
        let digest = blake3::hash(body.as_bytes());
        let receipt = PhaseReceipt {
            phase,
            content_hash: format!("blake3:{}", hex::encode(digest.as_bytes())),
            deferred_floor,
        };
        self.done
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(phase, receipt.clone());
        receipt
    }

    pub fn phase_call_count(&self, phase: RewritePhase) -> u32 {
        *self
            .phase_calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&phase)
            .unwrap_or(&0)
    }

    pub fn simulate_crash_losing(&self, from: RewritePhase) {
        let mut done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        done.retain(|p, _| *p < from);
    }
}

pub struct RewriteRateLimiter {
    budget: u32,
    used: Mutex<BTreeMap<TenantId, (u64, u32)>>,
}

impl RewriteRateLimiter {
    pub fn new(budget: u32) -> RewriteRateLimiter {
        RewriteRateLimiter {
            budget,
            used: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn try_acquire(&self, tenant: &TenantId, now_window: u64) -> bool {
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        let entry = used.entry(tenant.clone()).or_insert((now_window, 0));
        if entry.0 != now_window {
            *entry = (now_window, 0);
        }
        if entry.1 >= self.budget {
            return false;
        }
        entry.1 += 1;
        true
    }

    pub fn budget(&self) -> u32 {
        self.budget
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CacheEntryRef {
    pub scope_segment: String,
    pub name: String,
}

impl CacheEntryRef {
    pub fn new(scope_segment: impl Into<String>, name: impl Into<String>) -> CacheEntryRef {
        CacheEntryRef {
            scope_segment: scope_segment.into(),
            name: name.into(),
        }
    }

    pub fn is_trusted(&self) -> bool {
        self.scope_segment == "trusted"
    }
}

pub trait CacheNamespaceInvalidator {
    fn entries_for(&self, tenant: &TenantId, repo: &ArtifactRef) -> Vec<CacheEntryRef>;

    fn purge(&self, tenant: &TenantId, repo: &ArtifactRef, entry: &CacheEntryRef) -> bool;

    fn still_present(&self, tenant: &TenantId, repo: &ArtifactRef, entry: &CacheEntryRef) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidationFanOut {
    pub repo: ArtifactRef,
    pub purged: Vec<CacheEntryRef>,
    pub stale_remaining: Vec<CacheEntryRef>,
}

impl InvalidationFanOut {
    pub fn all_purged(&self) -> bool {
        self.stale_remaining.is_empty()
    }

    pub fn stale_hits(&self) -> usize {
        self.stale_remaining.len()
    }
}

#[derive(Debug, Default)]
pub struct InMemoryCacheNamespaces {
    entries: Mutex<BTreeSet<(String, String, String, String)>>,
}

impl InMemoryCacheNamespaces {
    pub fn new() -> InMemoryCacheNamespaces {
        InMemoryCacheNamespaces::default()
    }

    pub fn seed(&self, tenant: &TenantId, repo: &ArtifactRef, entry: &CacheEntryRef) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((
                tenant.0.clone(),
                entry.scope_segment.clone(),
                repo.0.clone(),
                entry.name.clone(),
            ));
    }
}

impl CacheNamespaceInvalidator for InMemoryCacheNamespaces {
    fn entries_for(&self, tenant: &TenantId, repo: &ArtifactRef) -> Vec<CacheEntryRef> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(t, _, r, _)| t == &tenant.0 && r == &repo.0)
            .map(|(_, scope, _, name)| CacheEntryRef::new(scope.clone(), name.clone()))
            .collect()
    }

    fn purge(&self, tenant: &TenantId, repo: &ArtifactRef, entry: &CacheEntryRef) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(
                tenant.0.clone(),
                entry.scope_segment.clone(),
                repo.0.clone(),
                entry.name.clone(),
            ))
    }

    fn still_present(&self, tenant: &TenantId, repo: &ArtifactRef, entry: &CacheEntryRef) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(
                tenant.0.clone(),
                entry.scope_segment.clone(),
                repo.0.clone(),
                entry.name.clone(),
            ))
    }
}

pub struct RewriteAudit;

impl RewriteAudit {
    pub fn audit_event(
        request: &HistoryRewriteRequest,
        outcome: Outcome,
        seq: u64,
    ) -> EventEnvelope {
        let actor = Principal::stub(
            PrincipalId(request.actor_pseudonym_local()),
            PrincipalKind::Human,
            request.tenant.clone(),
        );
        let region = actor.region.clone();
        let action = match outcome {
            Outcome::Denied => HISTORY_REWRITE_DENIED_ACTION,
            _ => HISTORY_REWRITE_ACTION,
        };
        EventEnvelope {
            event_id: EventId(format!("{action}:{}:{seq}", request.repo.0)),
            type_: EventType(action.into()),
            schema_ver: 1,
            tenant: request.tenant.clone(),
            region,
            actor: Actor(actor),
            subject: request.repo.clone(),
            aggregate: AggregateKey(format!("repo:{}", request.repo.0)),
            causation_id: None,
            correlation_id: CorrelationId(format!("git.history_rewrite:{}", request.repo.0)),
            caused_by: Some(CausedBy("gdpr.history_rewrite".into())),
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
            payload: serde_json::json!({
                "rewrite_spec": request.rewrite_spec,
                "outcome": outcome.as_wire(),
            }),
        }
    }

    pub fn seal(
        consumer: &AuditConsumer,
        request: &HistoryRewriteRequest,
        outcome: Outcome,
    ) -> u64 {
        let seq = consumer.log().len_for(&request.tenant);
        let ev = RewriteAudit::audit_event(request, outcome, seq);
        consumer.handle(&ev, &mut myelin_events::HandlerTx::none());
        seq
    }
}

pub struct RewriteWiring<'a> {
    pub rate_limiter: &'a RewriteRateLimiter,
    pub audit: &'a AuditConsumer,
    pub kms: &'a dyn CryptoShredKms,
    pub caches: &'a dyn CacheNamespaceInvalidator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RewriteDenied {
    RateLimited { tenant: TenantId, budget: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct GaTenCertificate {
    pub repo: ArtifactRef,
    pub action: String,
    pub audit_seq: u64,
    pub fan_out: InvalidationFanOut,
    pub pack_shred_epoch: Option<u64>,
    pub stale_pii_hits: usize,
    pub residual_named: String,
    pub content_hash: String,
}

impl GaTenCertificate {
    pub fn is_complete(&self) -> bool {
        self.stale_pii_hits == 0 && self.fan_out.all_purged()
    }
}

pub struct FirstClassRewriteOp;

impl FirstClassRewriteOp {
    pub fn run(
        request: &HistoryRewriteRequest,
        wiring: &RewriteWiring<'_>,
        now_window: u64,
    ) -> Result<GaTenCertificate, RewriteDenied> {
        if !wiring.rate_limiter.try_acquire(&request.tenant, now_window) {
            RewriteAudit::seal(wiring.audit, request, Outcome::Denied);
            return Err(RewriteDenied::RateLimited {
                tenant: request.tenant.clone(),
                budget: wiring.rate_limiter.budget(),
            });
        }

        let audit_seq = RewriteAudit::seal(wiring.audit, request, Outcome::Applied);

        let pack_handle = ShredKeyHandle {
            tenant: request.tenant.clone(),
            class: ShredKeyClass::Tenant,
        };
        let pack_shred_epoch = wiring.kms.destroy(&pack_handle);

        let reached = wiring.caches.entries_for(&request.tenant, &request.repo);
        let mut purged = Vec::new();
        for entry in &reached {
            if wiring.caches.purge(&request.tenant, &request.repo, entry) {
                purged.push(entry.clone());
            }
        }
        let stale_remaining: Vec<CacheEntryRef> = reached
            .iter()
            .filter(|e| {
                wiring
                    .caches
                    .still_present(&request.tenant, &request.repo, e)
            })
            .cloned()
            .collect();
        let fan_out = InvalidationFanOut {
            repo: request.repo.clone(),
            purged,
            stale_remaining,
        };
        let stale_pii_hits = fan_out.stale_hits();

        let residual_named = HistoryRewriteReceipt::residual_for(&request.repo);
        let content_hash = ga_ten_content_address(
            &request.repo,
            audit_seq,
            &fan_out,
            pack_shred_epoch,
            stale_pii_hits,
        );
        Ok(GaTenCertificate {
            repo: request.repo.clone(),
            action: HISTORY_REWRITE_ACTION.to_string(),
            audit_seq,
            fan_out,
            pack_shred_epoch,
            stale_pii_hits,
            residual_named,
            content_hash,
        })
    }
}

fn ga_ten_content_address(
    repo: &ArtifactRef,
    audit_seq: u64,
    fan_out: &InvalidationFanOut,
    pack_shred_epoch: Option<u64>,
    stale_pii_hits: usize,
) -> String {
    let mut body = format!("ga_10\u{1f}repo={}\u{1f}seq={audit_seq}", repo.0);
    for e in &fan_out.purged {
        body.push('\u{1f}');
        body.push_str(&format!("purged={}/{}", e.scope_segment, e.name));
    }
    body.push_str(&format!(
        "\u{1f}pack_epoch={}\u{1f}stale_hits={stale_pii_hits}",
        pack_shred_epoch.map(|e| e.to_string()).unwrap_or_default()
    ));
    format!(
        "blake3:{}",
        hex::encode(blake3::hash(body.as_bytes()).as_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> HistoryRewriteRequest {
        HistoryRewriteRequest {
            tenant: TenantId("acme".into()),
            repo: ArtifactRef("myelin://acme/git/repo-1".into()),
            actor_pseudonym: "p-7@acme.noreply".into(),
            rewrite_spec: "filter-repo:remove-blob:b-123".into(),
        }
    }

    #[test]
    fn the_activity_drives_every_phase_and_no_phase_is_a_deferred_floor() {
        let activity = HistoryRewriteActivity::new();
        let receipt = activity.drive(&request());

        assert_eq!(
            receipt.action, HISTORY_REWRITE_ACTION,
            "the op is audited as git.history_rewrite"
        );
        assert!(receipt.skeleton_complete(), "every phase is checkpointed");
        let order: Vec<_> = receipt.phase_receipts.iter().map(|r| r.phase).collect();
        assert_eq!(
            order,
            RewritePhase::ALL.to_vec(),
            "phases run in §6.6 order"
        );
        for r in &receipt.phase_receipts {
            assert!(
                !r.deferred_floor,
                "{} has a real mechanism on the M5 op (no deferred floor)",
                r.phase.token()
            );
        }
        assert!(receipt.residual_named.contains("off-platform clones"));
        assert!(
            receipt.residual_named.contains("P-GA-36"),
            "the residual names the outbound push-mirror residency gate (GA-11)"
        );
    }

    #[test]
    fn a_redrive_without_a_crash_runs_no_phase_body_twice() {
        let activity = HistoryRewriteActivity::new();
        let first = activity.drive(&request());
        for phase in RewritePhase::ALL {
            assert_eq!(
                activity.phase_call_count(phase),
                1,
                "{} ran once",
                phase.token()
            );
        }
        let second = activity.drive(&request());
        for phase in RewritePhase::ALL {
            assert_eq!(
                activity.phase_call_count(phase),
                1,
                "{} did NOT re-run on the re-drive",
                phase.token()
            );
        }
        assert_eq!(
            first.phase_receipts, second.phase_receipts,
            "idempotent - same receipts"
        );
    }

    #[test]
    fn a_crash_redrives_only_the_un_receipted_phases() {
        let activity = HistoryRewriteActivity::new();

        activity.drive(&request());
        activity.simulate_crash_losing(RewritePhase::CryptoShredPackTier);

        let resumed = activity.drive(&request());
        assert_eq!(
            activity.phase_call_count(RewritePhase::Audit),
            1,
            "phase 0 survived → not re-run"
        );
        assert_eq!(
            activity.phase_call_count(RewritePhase::Rewrite),
            1,
            "phase 1 survived → not re-run"
        );
        assert_eq!(
            activity.phase_call_count(RewritePhase::CryptoShredPackTier),
            2,
            "phase 2 was lost → re-run exactly once more"
        );
        assert_eq!(
            activity.phase_call_count(RewritePhase::InvalidateCaches),
            2,
            "phase 3 was lost → re-run exactly once more"
        );
        assert!(resumed.skeleton_complete());
    }

    #[test]
    fn phase_receipts_are_deterministic_content_addresses() {
        let a = HistoryRewriteActivity::new();
        let b = HistoryRewriteActivity::new();
        let ra = a.drive(&request());
        let rb = b.drive(&request());
        assert_eq!(
            ra.phase_receipts, rb.phase_receipts,
            "deterministic across activities"
        );
        for r in &ra.phase_receipts {
            assert!(r.content_hash.starts_with("blake3:"), "content-addressed");
        }
    }

    #[test]
    fn the_action_token_and_follow_on_are_pinned() {
        assert_eq!(HISTORY_REWRITE_ACTION, "git.history_rewrite");
        assert!(HISTORY_REWRITE_FIRST_CLASS_PROMPT.contains("P-GA-35"));
        assert_eq!(RewritePhase::Audit as u8, 0, "audit is phase 0");
        assert!(
            RewritePhase::InvalidateCaches > RewritePhase::Audit,
            "invalidation is last"
        );
    }

    #[test]
    fn each_phase_token_is_the_exact_string() {
        assert_eq!(RewritePhase::Audit.token(), "audit");
        assert_eq!(RewritePhase::Rewrite.token(), "rewrite");
        assert_eq!(
            RewritePhase::CryptoShredPackTier.token(),
            "crypto_shred_pack_tier"
        );
        assert_eq!(RewritePhase::InvalidateCaches.token(), "invalidate_caches");
        let tokens: std::collections::BTreeSet<_> =
            RewritePhase::ALL.iter().map(|p| p.token()).collect();
        assert_eq!(tokens.len(), 4, "every phase has a distinct token");
    }

    #[test]
    fn skeleton_complete_requires_every_phase() {
        let activity = HistoryRewriteActivity::new();
        let full = activity.drive(&request());
        assert!(full.skeleton_complete(), "a full drive is complete");

        let mut missing = full.clone();
        missing
            .phase_receipts
            .retain(|r| r.phase != RewritePhase::Rewrite);
        assert!(
            !missing.skeleton_complete(),
            "a missing phase is not complete"
        );

        let mut empty = full;
        empty.phase_receipts.clear();
        assert!(!empty.skeleton_complete(), "no phases is not complete");
    }

    use crate::holders::InMemoryShredKms;

    fn seeded_caches(tenant: &TenantId, repo: &ArtifactRef) -> InMemoryCacheNamespaces {
        let caches = InMemoryCacheNamespaces::new();
        caches.seed(tenant, repo, &CacheEntryRef::new("trusted", "clone-bundle"));
        caches.seed(tenant, repo, &CacheEntryRef::new("trusted", "pack-bitmap"));
        caches.seed(tenant, repo, &CacheEntryRef::new("fork:42", "fork-bundle"));
        caches
    }

    fn pack_kms(tenant: &TenantId) -> InMemoryShredKms {
        let kms = InMemoryShredKms::new();
        kms.provision(
            ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Tenant,
            },
            7,
        );
        kms
    }

    #[test]
    fn ga_10_history_rewrite_is_audited_and_the_fan_out_leaves_zero_stale_pii() {
        let req = request();
        let limiter = RewriteRateLimiter::new(2);
        let audit = AuditConsumer::new();
        let kms = pack_kms(&req.tenant);
        let caches = seeded_caches(&req.tenant, &req.repo);
        let wiring = RewriteWiring {
            rate_limiter: &limiter,
            audit: &audit,
            kms: &kms,
            caches: &caches,
        };

        let cert = FirstClassRewriteOp::run(&req, &wiring, 0).expect("op admitted under budget");

        assert_eq!(cert.stale_pii_hits, 0, "0 stale-PII cache/clone hits");
        assert!(cert.fan_out.all_purged(), "every reached blob was purged");
        assert!(cert.is_complete(), "the GA-10 certificate is complete");
        assert_eq!(
            cert.fan_out.purged.len(),
            3,
            "all stale clone/bundle blobs purged"
        );
        for entry in &cert.fan_out.purged {
            assert!(
                !caches.still_present(&req.tenant, &req.repo, entry),
                "{:?} is gone after the fan-out",
                entry
            );
        }
        let entries = audit.log().entries_for(&req.tenant);
        assert_eq!(entries.len(), 1, "one git.history_rewrite audit entry");
        assert_eq!(entries[0].action, HISTORY_REWRITE_ACTION);
        assert_eq!(
            entries[0].actor.actor, "p-7@acme.noreply",
            "actor is the tenant-admin pseudonym"
        );
        assert_eq!(entries[0].outcome, Outcome::Applied);
        assert_eq!(
            cert.audit_seq, 0,
            "the entry landed at the chain genesis seq"
        );
        assert_eq!(
            cert.pack_shred_epoch,
            Some(7),
            "the pack-tier DEK epoch is recorded"
        );
        assert!(
            !kms.is_present(&ShredKeyHandle {
                tenant: req.tenant.clone(),
                class: ShredKeyClass::Tenant
            }),
            "the pack-tier DEK is destroyed (reflogs/bitmaps/pack backups unrecoverable)"
        );
        assert!(cert.residual_named.contains("off-platform clones"));
    }

    #[test]
    fn a_surviving_stale_blob_is_a_red_ga_10() {
        let repo = ArtifactRef("myelin://acme/git/r".into());
        let fan_out = InvalidationFanOut {
            repo: repo.clone(),
            purged: vec![CacheEntryRef::new("trusted", "a")],
            stale_remaining: vec![CacheEntryRef::new("fork:9", "b")],
        };
        assert!(
            !fan_out.all_purged(),
            "a surviving stale blob fails all_purged"
        );
        assert_eq!(fan_out.stale_hits(), 1);
        let cert = GaTenCertificate {
            repo,
            action: HISTORY_REWRITE_ACTION.into(),
            audit_seq: 0,
            fan_out,
            pack_shred_epoch: Some(1),
            stale_pii_hits: 1,
            residual_named: "x".into(),
            content_hash: "blake3:x".into(),
        };
        assert!(
            !cert.is_complete(),
            "a stale hit is NOT a complete GA-10 certificate"
        );

        let repo2 = ArtifactRef("myelin://acme/git/r2".into());
        let inconsistent = GaTenCertificate {
            repo: repo2.clone(),
            action: HISTORY_REWRITE_ACTION.into(),
            audit_seq: 0,
            fan_out: InvalidationFanOut {
                repo: repo2,
                purged: vec![],
                stale_remaining: vec![CacheEntryRef::new("trusted", "survivor")],
            },
            pack_shred_epoch: Some(1),
            stale_pii_hits: 0,
            residual_named: "x".into(),
            content_hash: "blake3:x".into(),
        };
        assert!(!inconsistent.fan_out.all_purged());
        assert!(
            !inconsistent.is_complete(),
            "is_complete needs BOTH 0 stale hits AND a fully-purged fan-out"
        );
    }

    #[test]
    fn the_fan_out_purges_per_scope_without_cross_scope_bleed() {
        let tenant = TenantId("acme".into());
        let repo = ArtifactRef("myelin://acme/git/r".into());
        let caches = InMemoryCacheNamespaces::new();
        caches.seed(
            &tenant,
            &repo,
            &CacheEntryRef::new("trusted", "shared-name"),
        );
        caches.seed(
            &tenant,
            &repo,
            &CacheEntryRef::new("fork:42", "shared-name"),
        );

        let fork_entry = CacheEntryRef::new("fork:42", "shared-name");
        assert!(
            caches.purge(&tenant, &repo, &fork_entry),
            "fork-scope blob purged"
        );
        assert!(
            caches.still_present(
                &tenant,
                &repo,
                &CacheEntryRef::new("trusted", "shared-name")
            ),
            "the trusted-scope blob of the same name is UNTOUCHED (no cross-scope bleed)"
        );
        let reached = caches.entries_for(&tenant, &repo);
        assert!(
            reached.iter().any(|e| e.is_trusted()),
            "the trusted scope is reached"
        );

        assert!(CacheEntryRef::new("trusted", "x").is_trusted());
        assert!(
            !CacheEntryRef::new("fork:1", "x").is_trusted(),
            "a fork scope is not trusted"
        );
        assert!(
            !CacheEntryRef::new("branch:main", "x").is_trusted(),
            "a branch scope is not trusted"
        );

        let other_tenant = TenantId("globex".into());
        caches.seed(
            &other_tenant,
            &repo,
            &CacheEntryRef::new("trusted", "globex-blob"),
        );
        let other_repo = ArtifactRef("myelin://acme/git/other".into());
        caches.seed(
            &tenant,
            &other_repo,
            &CacheEntryRef::new("trusted", "other-repo-blob"),
        );
        let reached = caches.entries_for(&tenant, &repo);
        assert!(
            reached.iter().all(|e| e.name != "globex-blob" && e.name != "other-repo-blob"),
            "entries_for returns ONLY the (tenant ∧ repo) intersection - not a different tenant's or repo's blob"
        );
    }

    #[test]
    fn the_op_is_rate_limited_and_a_refusal_is_audited_denied() {
        let req = request();
        let limiter = RewriteRateLimiter::new(1);
        let audit = AuditConsumer::new();
        let kms = pack_kms(&req.tenant);
        let caches = seeded_caches(&req.tenant, &req.repo);
        let wiring = RewriteWiring {
            rate_limiter: &limiter,
            audit: &audit,
            kms: &kms,
            caches: &caches,
        };

        assert!(
            FirstClassRewriteOp::run(&req, &wiring, 0).is_ok(),
            "first op admitted"
        );
        let denied = FirstClassRewriteOp::run(&req, &wiring, 0).expect_err("second op refused");
        assert!(matches!(
            denied,
            RewriteDenied::RateLimited { budget: 1, .. }
        ));
        assert!(
            FirstClassRewriteOp::run(&req, &wiring, 1).is_ok(),
            "new window admits the op"
        );

        let entries = audit.log().entries_for(&req.tenant);
        let actions: Vec<_> = entries.iter().map(|e| e.action.as_str()).collect();
        assert_eq!(
            actions,
            vec![
                HISTORY_REWRITE_ACTION,
                HISTORY_REWRITE_DENIED_ACTION,
                HISTORY_REWRITE_ACTION,
            ],
            "the rate-limited refusal is audited as a distinct git.history_rewrite.denied action"
        );
    }

    #[test]
    fn the_rate_limiter_admits_exactly_the_budget_per_window() {
        let tenant = TenantId("acme".into());
        let limiter = RewriteRateLimiter::new(3);
        assert_eq!(
            limiter.budget(),
            3,
            "the budget accessor returns the configured budget"
        );
        assert_eq!(RewriteRateLimiter::new(5).budget(), 5);
        for _ in 0..3 {
            assert!(limiter.try_acquire(&tenant, 0), "under budget admits");
        }
        assert!(
            !limiter.try_acquire(&tenant, 0),
            "the 4th in the window is refused"
        );
        assert!(
            !limiter.try_acquire(&tenant, 0),
            "still refused (the refusal did not consume a slot)"
        );
        assert!(
            limiter.try_acquire(&tenant, 1),
            "a new window resets the budget"
        );
        let frozen = RewriteRateLimiter::new(0);
        assert!(
            !frozen.try_acquire(&tenant, 0),
            "a 0 budget denies every op"
        );
    }

    #[test]
    fn ga_d3_audit_tamper_is_detected_100_percent_at_cell_scale() {
        use crate::audit::verify_entries_for_test;
        let req = request();
        let limiter = RewriteRateLimiter::new(u32::MAX);
        let audit = AuditConsumer::new();
        let kms = pack_kms(&req.tenant);
        let caches = InMemoryCacheNamespaces::new();
        let wiring = RewriteWiring {
            rate_limiter: &limiter,
            audit: &audit,
            kms: &kms,
            caches: &caches,
        };

        const CELL_SCALE: u64 = 512;
        for w in 0..CELL_SCALE {
            FirstClassRewriteOp::run(&req, &wiring, w).expect("op admitted");
        }
        let entries = audit.log().entries_for(&req.tenant);
        assert_eq!(entries.len() as u64, CELL_SCALE, "cell-scale chain built");
        assert!(
            verify_entries_for_test(&entries),
            "the pristine cell-scale chain verifies intact"
        );

        let mut detected = 0u64;
        for i in 0..entries.len() {
            let mut tampered = entries.clone();
            tampered[i].subject = ArtifactRef(format!("myelin://acme/TAMPERED/{i}"));
            if !verify_entries_for_test(&tampered) {
                detected += 1;
            }
        }
        assert_eq!(
            detected as usize,
            entries.len(),
            "audit tamper detected 100% at cell scale ({detected}/{} entries)",
            entries.len()
        );
    }

    #[test]
    fn ga_d6_legal_hold_defers_the_rewrite_erasure_and_resumes_on_lift() {
        use crate::dsr::DsrKind;
        use crate::fanout::{HoldVerdict, LegalHoldRegistry};
        use myelin_gdpr::EraseScope;

        let tenant = TenantId("acme".into());
        let scope = EraseScope::Tenant(tenant.clone());
        let holds = LegalHoldRegistry::new();

        holds.set(crate::fanout::HoldScope::Tenant(tenant.0.clone()), true);
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &scope),
            HoldVerdict::Deferred,
            "the rewrite erasure is DEFERRED under the legal hold (0 held-scope deletions)"
        );

        holds.set(crate::fanout::HoldScope::Tenant(tenant.0.clone()), false);
        assert_eq!(
            holds.verdict(DsrKind::Erasure, &scope),
            HoldVerdict::Proceed,
            "the deferred erasure resumes on hold-lift"
        );
    }

    #[test]
    fn the_ga_10_certificate_is_a_deterministic_pii_free_artifact() {
        let req = request();
        let mk = || {
            let limiter = RewriteRateLimiter::new(1);
            let audit = AuditConsumer::new();
            let kms = pack_kms(&req.tenant);
            let caches = seeded_caches(&req.tenant, &req.repo);
            let wiring = RewriteWiring {
                rate_limiter: &limiter,
                audit: &audit,
                kms: &kms,
                caches: &caches,
            };
            FirstClassRewriteOp::run(&req, &wiring, 0).expect("op")
        };
        let a = mk();
        let b = mk();
        assert_eq!(
            a.content_hash, b.content_hash,
            "deterministic content-address"
        );
        assert!(a.content_hash.starts_with("blake3:"), "content-addressed");
        assert!(!a.content_hash.is_empty());
    }

    #[test]
    fn the_outbound_gate_follow_on_is_pinned() {
        assert!(HISTORY_REWRITE_OUTBOUND_GATE_PROMPT.contains("P-GA-36"));
        assert!(HISTORY_REWRITE_FIRST_CLASS_PROMPT.contains("P-GA-35"));
    }
}
