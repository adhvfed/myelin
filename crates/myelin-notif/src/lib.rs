use myelin_events::{DedupLedger, InProcessBus, OutboxStore};
use myelin_refs::ArtifactRef;
use myelin_substrate::{
    boot, serve, AppSpec, Config, ConsumerReg, CriticalDependencies, HotTables, InternalRpc,
    Migrations, OutboxSpec, PublicRoutes, ServeError, ServeHandle, StoreManifest,
};
use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};

mod agent_approval;
pub mod automation_approval;
pub mod cli;
pub mod cross_cell;
pub mod define_rule;
pub mod delivery;
pub mod erasure_residual;
pub mod escalation;
pub mod eu_provider;
pub mod holder;
pub mod humanise;
pub mod list_inbox;
pub mod migrations;
pub mod pg_inbox;
pub mod prefs;
pub mod ranking;
pub mod read_fanout;
pub mod read_state;
pub mod reindex;
pub mod router;
pub mod schema;
pub mod snooze_resurface;
pub mod storm_control;
pub mod surge;
pub mod watch;
pub mod write_fanout;

pub use agent_approval::{
    agent_effect_approval_action, agent_effect_approval_item_id, agent_effect_approval_targets,
    pending_agent_effect_approval, AgentEffectApprovalAction, AgentEffectApprovalTarget,
};
pub use automation_approval::{
    automation_approval_action, automation_approval_item_id, pending_automation_approval,
    AutomationApprovalAction,
};

pub use cli::{
    inbox_list, inbox_read, inbox_show, inbox_snooze, inbox_watch, notify_prefs, notify_prefs_set,
    notify_test, render_prefs, render_watch, CliView, InboxShow, WatchView,
};
pub use cross_cell::{
    aggregation_carried_fields, cross_cell_inbox_pointer, erase_inbox_pointers_in_cell,
    migrate_item_home_cell, CellLocalInboxResolver, CrossCellInbox, InboxEraseReceipt,
    InboxProjectionSlice, InboxResolution, InboxTombstone, InboxTombstoneReason,
    CROSS_CELL_RAW_ROWS_SIGNAL, CROSS_CELL_RESOLVES_SIGNAL,
};
pub use define_rule::{
    define_notif_rule, platform_default_reason, platform_default_rules, Classification, DedupTpl,
    DefineRuleError, NotifRule, NotifRuleRegistry,
};
pub use delivery::{
    build_idem_key, channel_from_token, effective_delivery_count, is_eu_region, redact_for_offcell,
    DeliveryError, DeliveryFabric, DeliveryLedger, DeliveryOutcome, DeliveryRecord, MockAdapter,
};
pub use erasure_residual::{
    erase_residual, DeliveryShredError, ErasedNotifSubject, InMemoryDeliveryShredder,
    InlineDeliveryShredder, NotifErasureLedger, OffCellResidual, ResidualEraseError,
    ResidualEraseReceipt, ERASURE_RESIDUAL_PROMPT,
};
pub use escalation::{
    notify_for, oncall_now, render_oncall, render_page, DurableWheel, EscalationEngine,
    EscalationError, EscalationPolicy, EscalationRun, EscalationStep, EscalationTarget,
    InMemoryWheel, OncallSchedule, PageOutcome, RotationWindow, RunState, ESCALATION_REASON,
};
pub use eu_provider::{
    EuProviderError, EuSovereignAdapter, EuTransport, OpenLegalFlag, ProviderErasureOutcome,
    RecordingEuTransport, TransportReceipt, OPEN_LEGAL_PROVIDER_DPA,
};
pub use holder::{
    notif_history_holder, notif_store_classifier, register_notif_holder, NotifBacking,
    NotifHistoryHolder, NotifHolderRegistration, RestrictSet, NOTIF_OLTP_STORE,
};
pub use humanise::{
    humanise, humanise_item, parse_markdown, reason_template_key, render_html, render_markdown,
    render_message, render_plain, shared_platform_templates, tombstone_display, Channel,
    ContentDoc, HumaniseTemplate, RefProjection, RefResolution, RefResolvePort, Span,
    TemplateStore, Tombstone, TombstoneReason, DEFAULT_LOCALE, HUMANISE_RESOLVE_MODE,
    PLATFORM_DEFAULT_TEMPLATES, PLATFORM_DEFAULT_TENANT,
};
pub use list_inbox::{
    list_inbox, list_inbox_ranked, subsystem_of, AllowAllAuthorize, Cursor, InboxFilter, InboxPage,
    Page, RankedPage, ReadAuthorizePort, Subsystem,
};
pub use prefs::{
    build_routing_matcher, get_prefs, route, route_context, set_prefs, Channel as PrefChannel,
    DigestConfig, NotifPrefs, PrefStore, PrefView, QuietHours, QuietWindow, RoutingRule, Tz,
    PREFS_MAX_PREDICATE_DEPTH, PREFS_MAX_PREDICATE_NODES,
};
pub use ranking::{
    band_ceiling, band_floor, base_priority, class_for, rank_and_order, reason_base_class,
    AffinitySource, DeterministicV1, ExplainTrace, NeutralAffinity, RankStrategy, RankedItem,
    PRIORITY_MAX, PRIORITY_MIN,
};
pub use read_fanout::{
    read_fanout, subject_root_col, AmbientMarkerStore, ReadFanoutError, ReadFanoutMarker,
    RelationalLeaf, ReverseIndexAnswer, RevisionWatermark, SyntheticReverseIndex,
    WatcherResolvePort, SUBJECT_ROOT_TYPE, WATCHER_RELATION, WATCH_PERMISSION,
};
pub use read_state::{active_inbox, mark, mark_all_read, snooze, ReadState, ReadStateError};
pub use reindex::{
    inbox_parity_hash, notif_scope, signal_snapshot_draft, signal_snapshot_subject, NotifReindexer,
    ReindexError as NotifReindexError, ReindexReceipt as NotifReindexReceipt, RetentionWindow,
    SignalReindexSource, DEFAULT_RETENTION_DAYS, NOTIF_OWNER_TOKEN, NOTIF_SNAPSHOT_TYPE,
};
pub use router::{
    build_durable_router, build_router, signal_subject_prefix, InboxProjection, RoutedInboxItem,
    SignalRouter, NOTIF_ESCALATION_ACKED, NOTIF_ITEM_CREATED, ROUTER_CONSUMER_NAME,
    SIGNAL_MENTIONS_KEY,
};
pub use snooze_resurface::{
    snooze_and_arm, snooze_timer_key, ResurfaceOutcome, SnoozeResurfacer, SNOOZE_TIMER_NS,
};
pub use storm_control::{
    dedup_collapse_ratio_bps, is_self_notification, subject_root_of, Coalescer, RateConfig,
    StormContext, StormControl, StormDecision, StormPrefs, SuppressReason, TokenBucket,
};
pub use surge::{
    run_notif_surge, NotifShedGate, NotifShedRejection, NotifSurgeReport, ProviderBulkhead,
    ERASURE_RESIDUAL_FOLLOW_ON, EU_DELIVERY_PROVIDER_FOLLOW_ON, NOTIF_SURGE_MULTIPLIER,
    NOTIF_SURGE_SURFACE,
};
pub use watch::{
    cold_rebuild, cold_rebuild_item_ids, inbox_scope, inbox_stream, publish_inbox_frame,
    watch_open, watch_resume, InboxFrame, InboxWatch, WatchOutcome,
};
pub use write_fanout::{
    extract_mentions, CapVerdict, HotSubjectCap, DEFAULT_HOT_SUBJECT_WRITE_CAP,
};

pub const SERVICE_NAME: &str = "notif";
pub const EVENT_STREAM_NAME: &str = "MYELIN_EVENTS";
pub const EVENT_SUBJECT_ROOT: &str = "myelin.events";
pub const EVENT_DURABLE_CONSUMER: &str = "notif-signal-router";

pub fn signal_intake_filter() -> String {
    format!("{EVENT_SUBJECT_ROOT}.evt.*.signal.>")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxItem {
    pub item_id: String,
    pub reason: Reason,
    pub class: Class,
    pub subject: ArtifactRef,
    pub template_key: String,
    pub template_args: Vec<ArtifactRef>,
    pub origin_event: ArtifactRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanisedString {
    pub text: String,
    pub links: Vec<String>,
    pub icon: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    ApprovalRequested,
    Escalated,
    Sla,
    ReviewRequested,
    Assigned,
    Mentioned,
    Replied,
    AgentProposal,
    Watched,
    StateChanged,
    Fyi,
    Blocked,
    Unblocked,
    ThreadWatched,
    Shared,
    Comments,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Critical,
    Direct,
    Participating,
    Watching,
    Fyi,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedMessage {
    pub rendered: HumanisedString,
    pub class: Class,
}

pub trait DeliveryAdapter {
    fn channel(&self) -> &str;

    fn region(&self) -> &myelin_tenancy::Region;

    fn send(&self, message: &RedactedMessage, idem_key: &str) -> Receipt;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub idem_key: String,
    pub accepted: bool,
}

fn notif_migrations() -> Migrations {
    migrations::migrations()
}

pub fn notif_app_spec(config: Config, outbox: OutboxStore) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: notif_migrations(),
        hot_tables: HotTables::none(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: Vec::new(),
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        outbox: OutboxSpec::external_relay(outbox),
        critical: CriticalDependencies::default(),
    }
}

pub fn notif_app_spec_with_router(
    config: Config,
    outbox: OutboxStore,
    tenants: &[TenantId],
    dedup: DedupLedger,
) -> (AppSpec, router::InboxProjection) {
    let inbox = router::InboxProjection::new();
    let mut consumers: Vec<ConsumerReg> = Vec::with_capacity(tenants.len());
    for tenant in tenants {
        if let Ok(router) =
            router::build_router(tenant, inbox.clone(), outbox.clone(), dedup.clone())
        {
            consumers.push(ConsumerReg::new(router));
        }
    }
    let mut spec = notif_app_spec(config, outbox.clone());
    spec.outbox = OutboxSpec::new(outbox, InProcessBus::new());
    spec.consumers = consumers;
    (spec, inbox)
}

pub fn notif_app_spec_with_ingestion(
    config: Config,
    outbox: OutboxStore,
    consumers: Vec<ConsumerReg>,
    intake: Box<dyn myelin_events::EventConsumer>,
    delivery_quarantine: std::sync::Arc<dyn myelin_events::DurableDeliveryQuarantine>,
) -> AppSpec {
    let mut spec = notif_app_spec(config, outbox.clone());
    spec.outbox = OutboxSpec::external_relay_with_consumer(outbox, intake, delivery_quarantine);
    spec.consumers = consumers;
    spec
}

pub fn boot_notif(config: Config, outbox: OutboxStore) -> Result<ServeHandle, ServeError> {
    boot(notif_app_spec(config, outbox))
}

pub fn run_notif(config: Config, outbox: OutboxStore) -> Result<(), ServeError> {
    serve(notif_app_spec(config, outbox))
}

pub async fn run_notif_until_shutdown<F>(
    config: Config,
    outbox: OutboxStore,
    shutdown: F,
) -> Result<(), ServeError>
where
    F: std::future::Future<Output = ()>,
{
    myelin_substrate::serve_until_shutdown(notif_app_spec(config, outbox), shutdown).await
}

pub async fn run_notif_ingestion_until_shutdown<F>(
    config: Config,
    outbox: OutboxStore,
    consumers: Vec<ConsumerReg>,
    intake: Box<dyn myelin_events::EventConsumer>,
    delivery_quarantine: std::sync::Arc<dyn myelin_events::DurableDeliveryQuarantine>,
    shutdown: F,
) -> Result<(), ServeError>
where
    F: std::future::Future<Output = ()>,
{
    myelin_substrate::serve_until_shutdown(
        notif_app_spec_with_ingestion(config, outbox, consumers, intake, delivery_quarantine),
        shutdown,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{Readiness, Startup, Surface};

    #[test]
    fn notif_shell_boots_and_three_ports_bind() {
        let handle =
            boot_notif(Config::default(), OutboxStore::new()).expect("the notif shell boots");
        assert_eq!(handle.name(), "notif");
        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports (public / internal-RPC / metrics-health) all bound (3/3)"
        );
    }

    #[test]
    fn readiness_is_false_pre_migrate_but_liveness_is_up() {
        let surface = myelin_substrate::MetricsHealthSurface::new(
            CriticalDependencies::new(["oltp"]),
            myelin_substrate::HealthTable::new(),
        );
        assert_eq!(surface.startup(), Startup::Booting);
        let r = surface.readiness();
        assert_eq!(
            r.verdict,
            Readiness::NotReady,
            "readiness is FALSE until the migrate-complete gate lifts"
        );
        assert!(
            r.startup_incomplete,
            "the not-ready reason names the startup (pre-migrate) gate"
        );
        assert!(r.sheds(), "a not-ready instance sheds new traffic");
        assert_eq!(
            surface.liveness(),
            myelin_substrate::Liveness::Up,
            "liveness ≠ readiness: a booting instance is not-killed (liveness stays Up)"
        );
        surface.mark_started();
        assert_eq!(
            surface.readiness().verdict,
            Readiness::Ready,
            "after migrate-complete the readiness gate lifts → ready"
        );
    }

    #[test]
    fn booted_instance_is_ready_after_migrate_complete() {
        let handle = boot_notif(Config::default(), OutboxStore::new()).expect("boot");
        assert_eq!(
            handle.metrics_health().startup(),
            Startup::Complete,
            "boot completed → the durable migration gate lifted"
        );
        assert_eq!(
            handle.metrics_health().readiness().verdict,
            Readiness::Ready,
            "a booted notif instance (migrations applied, deps up) is ready"
        );
    }

    #[tokio::test]
    async fn production_notif_waits_for_shutdown_then_drains() {
        assert_eq!(
            run_notif_until_shutdown(Config::default(), OutboxStore::new(), async {}).await,
            Ok(())
        );
    }

    #[test]
    fn shell_carries_empty_consumer_seam_and_durable_inbox_schema() {
        let spec = notif_app_spec(Config::default(), OutboxStore::new());
        assert!(
            spec.consumers.is_empty(),
            "the Signal-consumer router is the NOTIF-P3 floor"
        );
        assert_eq!(
            spec.migrations,
            migrations::migrations(),
            "the AppSpec wires the NOTIF-P2 set"
        );
        assert_eq!(spec.name, SERVICE_NAME);
    }

    #[test]
    fn notif_app_spec_with_router_registers_the_router_consumer() {
        let tenants = [TenantId("acme".into()), TenantId("globex".into())];
        let (spec, _inbox) = notif_app_spec_with_router(
            Config::default(),
            OutboxStore::new(),
            &tenants,
            DedupLedger::new(),
        );
        assert_eq!(
            spec.consumers.len(),
            2,
            "the Signal-consumer router is registered for each homed tenant (the seam is wired)"
        );
        assert_eq!(spec.name, SERVICE_NAME);
        assert_eq!(spec.migrations, migrations::migrations());
        let handle = boot(spec).expect("the router-wired notif spec boots under the harness");
        assert_eq!(
            handle.surfaces(),
            &[
                myelin_substrate::Surface::Public,
                myelin_substrate::Surface::Internal,
                myelin_substrate::Surface::MetricsHealth
            ],
            "the three ports bind around the router-wired spec"
        );
    }

    #[test]
    fn notif_app_spec_with_router_skips_overbroad_tenant_but_boots() {
        let tenants = [TenantId("acme".into()), TenantId("".into())];
        let (spec, _inbox) = notif_app_spec_with_router(
            Config::default(),
            OutboxStore::new(),
            &tenants,
            DedupLedger::new(),
        );
        assert_eq!(
            spec.consumers.len(),
            1,
            "only the valid tenant's router is wired (the empty one is skipped)"
        );
        boot(spec).expect("the shell still boots with the valid router");
    }
}

#[cfg(test)]
mod carrier_conformance {
    use super::*;

    #[test]
    fn inbox_item_stores_refs_not_payloads() {
        let item = InboxItem {
            item_id: "itm-1".into(),
            reason: Reason::Mentioned,
            class: Class::Direct,
            subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            template_key: "issue.mentioned".into(),
            template_args: vec![ArtifactRef("myelin://acme/identity/principal/u1".into())],
            origin_event: ArtifactRef("myelin://acme/bus/event/e1".into()),
        };
        let _subject: &ArtifactRef = &item.subject;
        let _args: &Vec<ArtifactRef> = &item.template_args;
        assert_eq!(item.subject.0, "myelin://acme/issues/issue/PROJ-1");
    }

    #[test]
    fn humanised_string_has_text_links_icon() {
        let h = HumanisedString {
            text: "you were mentioned".into(),
            links: vec!["myelin://acme/issues/issue/PROJ-1".into()],
            icon: "mention".into(),
        };
        assert_eq!(h.text, "you were mentioned");
        assert_eq!(h.links.len(), 1);
        assert_eq!(h.icon, "mention");
    }

    #[test]
    fn delivery_adapter_shape_is_implementable() {
        struct InAppAdapter {
            region: myelin_tenancy::Region,
        }
        impl DeliveryAdapter for InAppAdapter {
            fn channel(&self) -> &str {
                "in_app"
            }
            fn region(&self) -> &myelin_tenancy::Region {
                &self.region
            }
            fn send(&self, _message: &RedactedMessage, idem_key: &str) -> Receipt {
                Receipt {
                    idem_key: idem_key.to_string(),
                    accepted: true,
                }
            }
        }
        let adapter = InAppAdapter {
            region: myelin_tenancy::Region("fr-par".into()),
        };
        let msg = RedactedMessage {
            rendered: HumanisedString {
                text: "redacted".into(),
                links: vec![],
                icon: "fyi".into(),
            },
            class: Class::Fyi,
        };
        let r = adapter.send(&msg, "idem-1");
        assert_eq!(adapter.channel(), "in_app");
        assert_eq!(adapter.region().0, "fr-par");
        assert!(r.accepted);
        assert_eq!(r.idem_key, "idem-1");
    }

    #[test]
    fn reason_and_class_vocabularies_are_frozen() {
        let _issues_my_work = [
            Reason::Assigned,
            Reason::Mentioned,
            Reason::ReviewRequested,
            Reason::Sla,
            Reason::Watched,
            Reason::Blocked,
            Reason::ApprovalRequested,
        ];
        let _chat_activity = [
            Reason::Mentioned,
            Reason::Replied,
            Reason::ThreadWatched,
            Reason::ApprovalRequested,
        ];
        assert!(
            Class::Critical < Class::Fyi,
            "critical outranks fyi (the pierce ordering)"
        );
        assert!(Class::Direct < Class::Watching);
    }
}
