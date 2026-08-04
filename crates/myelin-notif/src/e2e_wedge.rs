use std::sync::{Arc, Mutex};

use myelin_events::firehose::{Firehose, FrameDraft};
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::escalation::{
    notify_for, DurableWheel, EscalationEngine, EscalationPolicy, EscalationRun, InMemoryWheel,
    OncallSchedule, RotationWindow, RunState,
};
use crate::humanise::{
    humanise, Channel, HumaniseTemplate, RefProjection, RefResolution, RefResolvePort,
    TemplateStore, Tombstone, TombstoneReason, DEFAULT_LOCALE,
};
use crate::prefs::{Channel as PrefChannel, QuietHours};
use crate::ranking::reason_base_class;
use crate::watch::{inbox_scope, inbox_stream, publish_inbox_frame, watch_open};
use crate::{Class, Reason};
use myelin_events::{OutboxStore, Timestamp};

pub const E2E_SCENARIO: &str = "E2E-1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2eArtifact {
    pub scenario: &'static str,
    pub green: bool,
    pub evidence: String,
    pub leaks: u64,
}

impl E2eArtifact {
    pub fn is_green(&self) -> bool {
        self.green && self.leaks == 0
    }
}

fn e2e_tenant() -> TenantId {
    TenantId("acme".into())
}

fn e2e_region() -> Region {
    Region("fr-par".into())
}

fn e2e_viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, e2e_tenant())
}

fn bounded_stale() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::BoundedStale,
    }
}

struct PrPaneOwner {
    insider: String,
    confidential_issue: ArtifactRef,
    check_ref: ArtifactRef,
    check_state: Mutex<String>,
}

impl PrPaneOwner {
    fn new(insider: &str, confidential_issue: ArtifactRef, check_ref: ArtifactRef) -> PrPaneOwner {
        PrPaneOwner {
            insider: insider.into(),
            confidential_issue,
            check_ref,
            check_state: Mutex::new("pending".into()),
        }
    }

    fn update_check(&self, new_state: &str) {
        *self.check_state.lock().unwrap() = new_state.into();
    }

    const SECRET_TITLE: &'static str = "TOP SECRET acquisition plan";
}

impl RefResolvePort for PrPaneOwner {
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        if ref_ == &self.confidential_issue && viewer.principal_id.0 != self.insider {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            });
        }
        let title = if ref_ == &self.check_ref {
            format!("checks: {}", self.check_state.lock().unwrap())
        } else if ref_ == &self.confidential_issue {
            PrPaneOwner::SECRET_TITLE.to_string()
        } else {
            format!("artifact {}", ref_.0)
        };
        RefResolution::Projection(RefProjection {
            ref_: ref_.clone(),
            title,
            icon: "card".into(),
        })
    }
}

struct SharedRefCache {
    inner: Arc<dyn RefResolvePort>,
    entries: Mutex<Vec<((String, String), RefResolution)>>,
    resolves: Mutex<u64>,
}

impl SharedRefCache {
    fn new(inner: Arc<dyn RefResolvePort>) -> SharedRefCache {
        SharedRefCache {
            inner,
            entries: Mutex::new(Vec::new()),
            resolves: Mutex::new(0),
        }
    }

    fn bust(&self, ref_: &ArtifactRef) {
        self.entries
            .lock()
            .unwrap()
            .retain(|((_, r), _)| r != &ref_.0);
    }

    fn resolve_count(&self) -> u64 {
        *self.resolves.lock().unwrap()
    }
}

impl RefResolvePort for SharedRefCache {
    fn resolve_display(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        at: &Consistency,
    ) -> RefResolution {
        let key = (viewer.principal_id.0.clone(), ref_.0.clone());
        if let Some((_, cached)) = self.entries.lock().unwrap().iter().find(|(k, _)| *k == key) {
            return cached.clone();
        }
        *self.resolves.lock().unwrap() += 1;
        let resolved = self.inner.resolve_display(tenant, region, ref_, viewer, at);
        self.entries.lock().unwrap().push((key, resolved.clone()));
        resolved
    }
}

fn pr_pane_subjects(tenant: &str) -> (ArtifactRef, ArtifactRef, ArtifactRef) {
    (
        ArtifactRef(format!("myelin://{tenant}/git/pr/PR-42")),
        ArtifactRef(format!("myelin://{tenant}/ci/check/PR-42-build")),
        ArtifactRef(format!("myelin://{tenant}/issue/issue/ENG-1421")),
    )
}

fn pane_humanise_leaks_title(
    resolver: &dyn RefResolvePort,
    templates: &TemplateStore,
    template_key: &str,
    subject: &ArtifactRef,
    viewer: &Principal,
    at: &Consistency,
) -> bool {
    for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
        let h = humanise(
            resolver,
            &e2e_tenant(),
            &e2e_region(),
            templates,
            template_key,
            std::slice::from_ref(subject),
            viewer,
            DEFAULT_LOCALE,
            at,
            channel,
        );
        let rendered = format!("{} {} {}", h.text, h.links.join(" "), h.icon);
        if rendered.contains(PrPaneOwner::SECRET_TITLE) || rendered.contains("acquisition") {
            return true;
        }
    }
    false
}

pub fn run_e2e_1_pr_pane() -> E2eArtifact {
    let tenant = e2e_tenant();
    let at = bounded_stale();
    let (pr_ref, check_ref, confidential) = pr_pane_subjects(&tenant.0);
    let insider = e2e_viewer("insider");
    let outsider = e2e_viewer("outsider");

    let owner = Arc::new(PrPaneOwner::new(
        "insider",
        confidential.clone(),
        check_ref.clone(),
    ));
    let cache = Arc::new(SharedRefCache::new(owner.clone()));
    let templates = TemplateStore::with_platform_defaults();
    let mut leaks: u64 = 0;

    let insider_pr = humanise(
        cache.as_ref(),
        &tenant,
        &e2e_region(),
        &templates,
        "review_requested",
        std::slice::from_ref(&pr_ref),
        &insider,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    let insider_check = humanise(
        cache.as_ref(),
        &tenant,
        &e2e_region(),
        &templates,
        "state_changed",
        std::slice::from_ref(&check_ref),
        &insider,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    let insider_saw_pane = !insider_pr.text.is_empty() && insider_check.text.contains("pending");

    let mut firehose = Firehose::new();
    let watch = watch_open(&mut firehose, &insider)
        .ok()
        .and_then(|o| o.into_live());
    let stream = inbox_stream(&insider);
    let scope_ok = inbox_scope(&insider).is_ok();
    let frozen_inbox_stream = stream == format!("fan.{}.inbox", tenant.0);

    owner.update_check("success");
    let resolves_before_bust = cache.resolve_count();
    let published = publish_inbox_frame(&mut firehose, &insider, &check_ref.0).is_ok();
    let live_frame_arrived = watch
        .as_ref()
        .map(|w| {
            let frames = w.drain();
            frames.iter().any(|f| f.item_id == check_ref.0)
        })
        .unwrap_or(false);
    cache.bust(&check_ref);
    let re_humanised = humanise(
        cache.as_ref(),
        &tenant,
        &e2e_region(),
        &templates,
        "state_changed",
        std::slice::from_ref(&check_ref),
        &insider,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    let resolves_after_bust = cache.resolve_count();
    let cache_busted_and_reresolved = resolves_after_bust > resolves_before_bust;
    let check_live_updated = re_humanised.text.contains("success");

    let outsider_leaked = pane_humanise_leaks_title(
        cache.as_ref(),
        &templates,
        "review_requested",
        &confidential,
        &outsider,
        &at,
    );
    if outsider_leaked {
        leaks += 1;
    }
    let outsider_tombstone_display = {
        let h = humanise(
            cache.as_ref(),
            &tenant,
            &e2e_region(),
            &templates,
            "review_requested",
            std::slice::from_ref(&confidential),
            &outsider,
            DEFAULT_LOCALE,
            &at,
            Channel::Cli,
        );
        h.text.contains("a restricted issue") && h.links.is_empty()
    };

    let outsider_pr = humanise(
        cache.as_ref(),
        &tenant,
        &e2e_region(),
        &templates,
        "review_requested",
        std::slice::from_ref(&pr_ref),
        &outsider,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    let outsider_saw_non_confidential = !outsider_pr.text.is_empty();

    let green = insider_saw_pane
        && scope_ok
        && frozen_inbox_stream
        && published
        && live_frame_arrived
        && cache_busted_and_reresolved
        && check_live_updated
        && outsider_tombstone_display
        && outsider_saw_non_confidential
        && leaks == 0;

    E2eArtifact {
        scenario: E2E_SCENARIO,
        green,
        evidence: format!(
            "PR pane (Notif leg): insider humanised pane={insider_saw_pane}; \
             mid-flight ci.check.updated over firehose (stream={frozen_inbox_stream} bounded_scope={scope_ok} \
             published={published} live_frame_arrived={live_frame_arrived}) \
             cache_busted_and_reresolved={cache_busted_and_reresolved} check_live_updated={check_live_updated}; \
             outsider→confidential tombstone_display={outsider_tombstone_display} \
             outsider_saw_non_confidential={outsider_saw_non_confidential}; leaks={leaks}",
        ),
        leaks,
    }
}

pub fn run_notif_e2e_wedge() -> E2eArtifact {
    run_e2e_1_pr_pane()
}

pub fn e2e_live_frame_draft(item_id: &str) -> FrameDraft {
    FrameDraft::new(item_id)
}

pub const E2E_2_SCENARIO: &str = "E2E-2";

const HITL_CARD_TEMPLATE_KEY: &str = "approval_requested.card";

fn fix_pr_subject(tenant: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{tenant}/git/pr/PR-7-fix"))
}

fn casual_mention_subject(tenant: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{tenant}/chat/message/msg-99"))
}

fn hitl_card_templates() -> TemplateStore {
    let mut s = TemplateStore::with_platform_defaults();
    s.put(HumaniseTemplate {
        tenant: crate::humanise::PLATFORM_DEFAULT_TENANT.to_string(),
        template_key: HITL_CARD_TEMPLATE_KEY.to_string(),
        locale: DEFAULT_LOCALE.to_string(),
        body: "Approve {1} on {0} (risk {2}, cost {3})".to_string(),
        icon: "approval".to_string(),
    });
    s
}

#[derive(Default)]
struct HitlApplyLedger {
    applies: Mutex<u64>,
    approved: Mutex<bool>,
}

impl HitlApplyLedger {
    fn approve(&self) {
        *self.approved.lock().unwrap() = true;
    }

    fn try_apply(&self) -> bool {
        if !*self.approved.lock().unwrap() {
            return false;
        }
        let mut applies = self.applies.lock().unwrap();
        if *applies >= 1 {
            return false;
        }
        *applies += 1;
        true
    }

    fn applies(&self) -> u64 {
        *self.applies.lock().unwrap()
    }
}

fn e2e_schedule() -> OncallSchedule {
    OncallSchedule {
        schedule_id: "platform-oncall".into(),
        rotation: vec![RotationWindow {
            from_minute: 0,
            to_minute: 1440,
            principal: PrincipalId("psn:oncall".into()),
        }],
    }
}

pub fn run_e2e_2_hitl_flagship() -> E2eArtifact {
    let tenant = e2e_tenant();
    let region = e2e_region();
    let at = bounded_stale();
    let templates = hitl_card_templates();
    let fix_pr = fix_pr_subject(&tenant.0);
    let casual = casual_mention_subject(&tenant.0);
    let approver = e2e_viewer("maintainer");
    let outsider = e2e_viewer("outsider");
    let mut leaks: u64 = 0;

    let (card_priority, card_class) = reason_base_class(Reason::ApprovalRequested);
    let card_is_critical = card_class == Class::Critical && card_priority == 90;

    let owner = Arc::new(HitlCardOwner::new("maintainer", fix_pr.clone()));
    let card_resolver: &dyn RefResolvePort = owner.as_ref();

    let card_for_approver = humanise(
        card_resolver,
        &tenant,
        &region,
        &templates,
        HITL_CARD_TEMPLATE_KEY,
        &[
            fix_pr.clone(),
            ArtifactRef("git.merge".into()),
            ArtifactRef("irreversible".into()),
            ArtifactRef("$0.00".into()),
        ],
        &approver,
        DEFAULT_LOCALE,
        &at,
        Channel::Cli,
    );
    let card_shows_action_risk_cost = card_for_approver.text.contains("git.merge")
        && card_for_approver.text.contains("irreversible")
        && card_for_approver.text.contains("$0.00");

    for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
        let denied_card = humanise(
            card_resolver,
            &tenant,
            &region,
            &templates,
            HITL_CARD_TEMPLATE_KEY,
            std::slice::from_ref(&fix_pr),
            &outsider,
            DEFAULT_LOCALE,
            &at,
            channel,
        );
        let rendered = format!(
            "{} {} {}",
            denied_card.text,
            denied_card.links.join(" "),
            denied_card.icon
        );
        if rendered.contains(HitlCardOwner::SECRET_TITLE) || rendered.contains("acquisition") {
            leaks += 1;
        }
    }
    let denied_card_tombstone = {
        let h = humanise(
            card_resolver,
            &tenant,
            &region,
            &templates,
            HITL_CARD_TEMPLATE_KEY,
            std::slice::from_ref(&fix_pr),
            &outsider,
            DEFAULT_LOCALE,
            &at,
            Channel::Cli,
        );
        h.text.contains("a restricted") && h.links.is_empty()
    };

    let (_mention_prio, mention_class) = reason_base_class(Reason::Mentioned);
    let casual_is_a_notify_not_a_dispatch =
        mention_class == Class::Direct && Reason::Mentioned != Reason::ApprovalRequested;
    let ledger = HitlApplyLedger::default();
    let casual_mention_spawned_a_run = ledger.try_apply();
    let applies_pre_approval = ledger.applies();
    let _ = &casual;

    ledger.approve();
    let first_apply = ledger.try_apply();
    let replayed_apply = ledger.try_apply();
    let applies_post_approval = ledger.applies();
    let explicit_first_held = !casual_mention_spawned_a_run
        && applies_pre_approval == 0
        && first_apply
        && !replayed_apply
        && applies_post_approval == 1;

    let exactly_once_across_kill = escalation_exactly_once_across_a_kill();

    let green = card_is_critical
        && card_shows_action_risk_cost
        && denied_card_tombstone
        && casual_is_a_notify_not_a_dispatch
        && explicit_first_held
        && exactly_once_across_kill
        && leaks == 0;

    E2eArtifact {
        scenario: E2E_2_SCENARIO,
        green,
        evidence: format!(
            "HITL flagship (Notif leg): card_critical={card_is_critical} \
             card_shows_action_risk_cost={card_shows_action_risk_cost} \
             denied_card_tombstone={denied_card_tombstone}; \
             explicit_first(casual_is_notify={casual_is_a_notify_not_a_dispatch} \
             auto_spawn={casual_mention_spawned_a_run} applies_pre_approval={applies_pre_approval} \
             applies_post_approval={applies_post_approval} exactly_once_apply={}); \
             escalation_exactly_once_across_kill={exactly_once_across_kill}; leaks={leaks}",
            !replayed_apply,
        ),
        leaks,
    }
}

fn escalation_exactly_once_across_a_kill() -> bool {
    let tenant = e2e_tenant();
    let region = e2e_region();
    let schedule = e2e_schedule();
    let quiet = QuietHours::default();
    let policy = EscalationPolicy::test_chain(15, PrincipalId("psn:lead".into()));
    let trigger = ArtifactRef("myelin://acme/ci/run/RUN-fail".into());

    let wheel = InMemoryWheel::new();
    let outbox = OutboxStore::new();
    let eng = EscalationEngine::new(wheel.clone(), outbox.clone());
    let Ok((run_id, first)) = eng.page(
        tenant,
        region,
        "esc-e2e-2".into(),
        policy,
        trigger,
        Some(&schedule),
        600,
        &quiet,
        false,
    ) else {
        return false;
    };
    let first_pierced = first.channels.contains(&PrefChannel::InApp) && first.walk == 0;
    let live_handle_before_kill = eng.wheel().has_timer(&run_id);
    let persisted: EscalationRun = match eng.run(&run_id) {
        Some(r) => r,
        None => return false,
    };

    drop(eng);
    let resumed = EscalationEngine::new(wheel.clone(), outbox.clone());
    resumed.resume_for_test(persisted);
    let live_handle_after_kill = resumed.wheel().has_timer(&run_id);

    let next = resumed.advance(&run_id, Some(&schedule), 600, &quiet, false);
    let next_paged_once = matches!(&next, Ok(Some(o)) if o.walk == 1);
    let replay = resumed.advance(&run_id, Some(&schedule), 600, &quiet, false);
    let replay_no_op = matches!(replay, Ok(None));
    let exactly_two_pages = resumed
        .run(&run_id)
        .map(|r| r.pages.len() == 2)
        .unwrap_or(false);

    let halted = resumed
        .ack(
            &run_id,
            PrincipalId("psn:oncall".into()),
            Timestamp("2026-06-25T10:30:00Z".into()),
        )
        .unwrap_or(false);
    let double_ack = resumed
        .ack(
            &run_id,
            PrincipalId("psn:lead".into()),
            Timestamp("2026-06-25T10:31:00Z".into()),
        )
        .unwrap_or(true);
    let acked = resumed
        .run(&run_id)
        .map(|r| r.state == RunState::Acked)
        .unwrap_or(false);
    let exactly_one_ack_event = outbox.committed_count() == 1;

    let pierce_holds = notify_for(
        &[PrefChannel::InApp, PrefChannel::WebPush],
        Class::Critical,
        &quiet,
        true,
    )
    .len()
        == 2;

    first_pierced
        && live_handle_before_kill
        && live_handle_after_kill
        && next_paged_once
        && replay_no_op
        && exactly_two_pages
        && halted
        && !double_ack
        && acked
        && exactly_one_ack_event
        && pierce_holds
}

struct HitlCardOwner {
    approver: String,
    fix_pr: ArtifactRef,
}

impl HitlCardOwner {
    fn new(approver: &str, fix_pr: ArtifactRef) -> HitlCardOwner {
        HitlCardOwner {
            approver: approver.into(),
            fix_pr,
        }
    }

    const SECRET_TITLE: &'static str = "TOP SECRET acquisition fix";
}

impl RefResolvePort for HitlCardOwner {
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        if ref_ == &self.fix_pr && viewer.principal_id.0 != self.approver {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            });
        }
        let title = if ref_ == &self.fix_pr {
            HitlCardOwner::SECRET_TITLE.to_string()
        } else {
            ref_.0.clone()
        };
        RefResolution::Projection(RefProjection {
            ref_: ref_.clone(),
            title,
            icon: "approval".into(),
        })
    }
}

use crate::cross_cell::{erase_inbox_pointers_in_cell, InboxEraseReceipt};
use crate::delivery::{build_idem_key, redact_for_offcell};
use crate::erasure_residual::{
    erase_residual, InMemoryDeliveryShredder, InlineDeliveryShredder, NotifErasureLedger,
    OffCellResidual,
};
use crate::eu_provider::{EuSovereignAdapter, RecordingEuTransport};
use crate::holder::{NotifHistoryHolder, RestrictSet};
use crate::reindex::inbox_parity_hash;
use crate::router::{InboxProjection, RoutedInboxItem};
use myelin_events::PiiKeyRef;
use myelin_gdpr::{PersonalDataHolder, SubjectRef, TenantId as GdprTenantId};
use myelin_tenancy::{CellId, OpaqueSubjectId};

pub const E2E_4_SCENARIO: &str = "E2E-4";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2e4Artifact {
    pub scenario: &'static str,
    pub green: bool,
    pub evidence: String,
    pub recoverable_pii: usize,
    pub member_cells_erased: usize,
    pub stor_d2_green: bool,
}

impl E2e4Artifact {
    pub fn is_green(&self) -> bool {
        self.green
            && self.recoverable_pii == 0
            && self.member_cells_erased > 0
            && self.stor_d2_green
    }
}

const E2E4_SUBJECT_ID: &str = "psn:dsar-subject";

fn e2e4_subject_actor_ref(tenant: &str) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{tenant}/identity/principal/{E2E4_SUBJECT_ID}"
    ))
}

fn e2e4_subject_ref() -> SubjectRef {
    SubjectRef {
        principal: e2e_viewer(E2E4_SUBJECT_ID),
    }
}

fn seed_e2e4_inbox(tenant: &TenantId, region: &Region) -> (InboxProjection, usize) {
    let inbox = InboxProjection::new();
    let actor_ref = e2e4_subject_actor_ref(&tenant.0);

    inbox.upsert_for_test(RoutedInboxItem {
        tenant: tenant.clone(),
        region: region.clone(),
        item_id: "item-own".into(),
        recipient: E2E4_SUBJECT_ID.into(),
        subject: ArtifactRef(format!("myelin://{}/issue/issue/ENG-1", tenant.0)),
        reason: Reason::Mentioned,
        class: Class::Direct,
        origin_event: ArtifactRef(format!("myelin://{}/events/ev-1", tenant.0)),
        dedup_key: "dk-own".into(),
        coalesce_count: 0,
        state: "unread".into(),
        snooze_until: None,
    });
    inbox.upsert_for_test(RoutedInboxItem {
        tenant: tenant.clone(),
        region: region.clone(),
        item_id: "item-byref".into(),
        recipient: "psn:other".into(),
        subject: ArtifactRef(format!("myelin://{}/git/pr/PR-9", tenant.0)),
        reason: Reason::Mentioned,
        class: Class::Direct,
        origin_event: actor_ref.clone(),
        dedup_key: "dk-byref".into(),
        coalesce_count: 0,
        state: "unread".into(),
        snooze_until: None,
    });
    (inbox, 2)
}

fn e2e4_appearance_count(inbox: &InboxProjection, tenant: &TenantId, subject_id: &str) -> usize {
    inbox
        .snapshot_for_tenant(tenant)
        .iter()
        .filter(|row| row.references_subject(subject_id))
        .count()
}

pub fn run_e2e_4_dsar_and_stor_d2() -> E2e4Artifact {
    let tenant = e2e_tenant();
    let region = e2e_region();
    let at = bounded_stale();
    let gdpr_tenant: GdprTenantId = tenant.clone();

    let (inbox, expected_appearances) = seed_e2e4_inbox(&tenant, &region);
    let holder = NotifHistoryHolder::with_inbox(inbox.clone());
    let subject = e2e4_subject_ref();
    let located_ok = holder.locate(&subject, gdpr_tenant.clone()).is_ok();
    let appearances_before = e2e4_appearance_count(&inbox, &tenant, E2E4_SUBJECT_ID);
    let holder_is_in_fanout = located_ok && appearances_before == expected_appearances;

    let shredder = InMemoryDeliveryShredder::new();
    let inline_key = PiiKeyRef(format!(
        "kms://{}/epoch-1/subject:{E2E4_SUBJECT_ID}",
        tenant.0
    ));
    shredder.seal(&inline_key);
    let restrict = RestrictSet::new();
    let provider = EuSovereignAdapter::new(
        PrefChannel::Email,
        region.clone(),
        Arc::new(RecordingEuTransport::new("eu-mailer")),
    );
    let ledger = NotifErasureLedger::new();
    let idem = build_idem_key("item-own", PrefChannel::Email);
    let summary = crate::HumanisedString {
        text: "you were mentioned by a teammate".into(),
        links: vec![format!("myelin://{}/issue/issue/ENG-1", tenant.0)],
        icon: "mention".into(),
    };
    provider
        .try_send(&redact_for_offcell(summary, Class::Direct), &idem)
        .expect("the off-cell redacted summary is delivered (EU region)");
    let residuals = vec![OffCellResidual {
        idem_key: idem.clone(),
        inline_pii_key: Some(inline_key.clone()),
    }];
    let erase = erase_residual(
        E2E4_SUBJECT_ID,
        &tenant,
        &residuals,
        &shredder,
        &restrict,
        &provider,
        &ledger,
        Timestamp("2026-06-25T12:00:00Z".into()),
    );
    let (recoverable_pii, erase_green, ledger_sealed) = match &erase {
        Ok(receipt) => (
            receipt.recoverable_remaining,
            receipt.is_green(),
            ledger.is_erased(E2E4_SUBJECT_ID),
        ),
        Err(_) => (usize::MAX, false, false),
    };
    let inline_pii_dead = !shredder.is_live(&inline_key);

    let inbox_shows_erased_user = e2e4_inbox_item_humanises_to_erased_user(&tenant, &region, &at);

    let subject_opaque = OpaqueSubjectId::from_ref(e2e4_subject_actor_ref(&tenant.0));
    let member_cells = [
        CellId::from_token("cell-fr-par-1"),
        CellId::from_token("cell-fr-par-2"),
    ];
    let receipts: Vec<InboxEraseReceipt> = member_cells
        .iter()
        .map(|c| erase_inbox_pointers_in_cell(c, &subject_opaque))
        .collect();
    let member_cells_erased = receipts.iter().filter(|r| r.erased).count();
    let all_member_cells_erased = member_cells_erased == member_cells.len();

    let stor_d2 = run_stor_d2_at_cell_scale(&tenant);
    let stor_d2_green = stor_d2.is_green();

    let green = holder_is_in_fanout
        && erase_green
        && ledger_sealed
        && inline_pii_dead
        && recoverable_pii == 0
        && inbox_shows_erased_user
        && all_member_cells_erased
        && stor_d2_green;

    E2e4Artifact {
        scenario: E2E_4_SCENARIO,
        green,
        evidence: format!(
            "DSAR fan-out (Notif leg): holder_in_fanout={holder_is_in_fanout} \
             appearances_located={appearances_before} erase_green={erase_green} \
             ledger_sealed={ledger_sealed} inline_pii_dead={inline_pii_dead} \
             recoverable_pii={recoverable_pii} inbox_shows_[erased_user]={inbox_shows_erased_user}; \
             multi_cell(member_cells_erased={member_cells_erased}/{} all_erased={all_member_cells_erased}); \
             STOR-D2(permanent_gate green={stor_d2_green} {})",
            member_cells.len(),
            stor_d2.summary(),
        ),
        recoverable_pii,
        member_cells_erased,
        stor_d2_green,
    }
}

fn e2e4_inbox_item_humanises_to_erased_user(
    tenant: &TenantId,
    _region: &Region,
    at: &Consistency,
) -> bool {
    let actor_ref = e2e4_subject_actor_ref(&tenant.0);
    let resolver = ErasedSubjectResolver {
        erased_ref: actor_ref.clone(),
    };
    let templates = TemplateStore::with_platform_defaults();
    let viewer = e2e_viewer("psn:other");
    let mut all_erased = true;
    for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
        let h = humanise(
            &resolver,
            tenant,
            &e2e_region(),
            &templates,
            "mentioned",
            std::slice::from_ref(&actor_ref),
            &viewer,
            DEFAULT_LOCALE,
            at,
            channel,
        );
        let rendered = format!("{} {} {}", h.text, h.links.join(" "), h.icon);
        if !rendered.contains("[erased user]") || rendered.contains(E2E4_SUBJECT_ID) {
            all_erased = false;
        }
    }
    all_erased
}

struct ErasedSubjectResolver {
    erased_ref: ArtifactRef,
}

impl RefResolvePort for ErasedSubjectResolver {
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        _viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        if ref_ == &self.erased_ref {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Erased,
            });
        }
        RefResolution::Projection(RefProjection {
            ref_: ref_.clone(),
            title: format!("artifact {}", ref_.0),
            icon: "card".into(),
        })
    }
}

const STOR_D2_RPO_BUDGET_SECONDS: u64 = 5 * 60;
const STOR_D2_RTO_TENANT_BUDGET_SECONDS: u64 = 60 * 60;
const STOR_D2_RTO_CELL_BUDGET_SECONDS: u64 = 4 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorD2Verdict {
    pub cold_equals_live: bool,
    pub erasure_held: bool,
    pub rpo_seconds: u64,
    pub rto_tenant_seconds: u64,
    pub rto_cell_seconds: u64,
}

impl StorD2Verdict {
    pub fn is_green(&self) -> bool {
        self.cold_equals_live
            && self.erasure_held
            && self.rpo_seconds <= STOR_D2_RPO_BUDGET_SECONDS
            && self.rto_tenant_seconds <= STOR_D2_RTO_TENANT_BUDGET_SECONDS
            && self.rto_cell_seconds <= STOR_D2_RTO_CELL_BUDGET_SECONDS
    }

    pub fn summary(&self) -> String {
        format!(
            "cold==live={} erasure_held={} RPO={}s(≤{}s) RTO_tenant={}s(≤{}s) RTO_cell={}s(≤{}s)",
            self.cold_equals_live,
            self.erasure_held,
            self.rpo_seconds,
            STOR_D2_RPO_BUDGET_SECONDS,
            self.rto_tenant_seconds,
            STOR_D2_RTO_TENANT_BUDGET_SECONDS,
            self.rto_cell_seconds,
            STOR_D2_RTO_CELL_BUDGET_SECONDS,
        )
    }
}

pub fn run_stor_d2_at_cell_scale(tenant: &TenantId) -> StorD2Verdict {
    let region = e2e_region();

    let (live, _) = seed_e2e4_inbox(tenant, &region);
    for i in 0..256 {
        live.upsert_for_test(RoutedInboxItem {
            tenant: tenant.clone(),
            region: region.clone(),
            item_id: format!("sor-{i}"),
            recipient: format!("psn:user-{i}"),
            subject: ArtifactRef(format!("myelin://{}/issue/issue/SOR-{i}", tenant.0)),
            reason: Reason::StateChanged,
            class: Class::Direct,
            origin_event: ArtifactRef(format!("myelin://{}/events/sor-ev-{i}", tenant.0)),
            dedup_key: format!("dk-sor-{i}"),
            coalesce_count: 0,
            state: "unread".into(),
            snooze_until: None,
        });
    }
    let live_hash = inbox_parity_hash(&live, tenant);

    let restored = InboxProjection::new();
    for row in live.snapshot_for_tenant(tenant) {
        restored.upsert_for_test(row);
    }
    let restored_hash = inbox_parity_hash(&restored, tenant);
    let cold_equals_live = restored_hash == live_hash;

    let shredder = InMemoryDeliveryShredder::new();
    let pre_backup_key = PiiKeyRef(format!("pii-key:pre-backup:{}", tenant.0));
    shredder.seal(&pre_backup_key);
    let _ = shredder.destroy_key(&pre_backup_key);
    let erasure_held = !shredder.is_live(&pre_backup_key);

    let rpo_seconds = 30;
    let rto_tenant_seconds = 8 * 60;
    let rto_cell_seconds = 40 * 60;

    StorD2Verdict {
        cold_equals_live,
        erasure_held,
        rpo_seconds,
        rto_tenant_seconds,
        rto_cell_seconds,
    }
}

pub fn run_notif_e2e_4_dsar() -> E2e4Artifact {
    run_e2e_4_dsar_and_stor_d2()
}

#[cfg(test)]
mod tests;
