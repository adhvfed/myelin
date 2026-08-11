use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_content::InlineNode;
use myelin_events::{
    consume, AggregateKey, ArtifactRef, Consumer, ConsumerName, ConsumerSpec, DataRole,
    DedupLedger, EmitContextBase, EventDraft, EventEnvelope, EventHandler, EventType,
    HandleOutcome, IdMinter, MonotonicMinter, OutboxStore, OutboxTransaction, OutboxTx,
    Reason as BusReason, SubjectPattern, SubscribeError, Visibility,
};
use myelin_identity::Principal;
use myelin_query::signals::{Severity, Signal, SignalState};
use myelin_tenancy::{Region, TenantId};

use crate::humanise::reason_template_key;
use crate::pg_inbox::{InboxUpsert, InboxUpsertOutcome, PgInboxStore};
use crate::prefs::QuietHours;
use crate::ranking::reason_base_class;
use crate::storm_control::{
    is_self_notification, subject_root_of, RateConfig, StormContext, StormControl, StormDecision,
    SuppressReason,
};
use crate::write_fanout::{extract_mentions, CapVerdict, HotSubjectCap};
use crate::{Class, Reason};

pub const NOTIF_ITEM_CREATED: &str = "notif.item.created";

pub const NOTIF_ESCALATION_ACKED: &str = "notif.escalation.acked";

pub const ROUTER_CONSUMER_NAME: &str = "notif-signal-router";

pub fn signal_subject_prefix(tenant: &TenantId) -> Option<String> {
    if tenant.0.is_empty() || tenant.0.contains('.') {
        return None;
    }
    Some(format!("sig.{}.", tenant.0))
}

pub fn is_signal_subject(subject: &str, tenant: &TenantId) -> bool {
    match signal_subject_prefix(tenant) {
        Some(prefix) => subject.starts_with(&prefix),
        None => false,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedInboxItem {
    pub tenant: TenantId,
    pub region: Region,
    pub item_id: String,
    pub recipient: String,
    pub subject: ArtifactRef,
    pub reason: Reason,
    pub class: Class,
    pub origin_event: ArtifactRef,
    pub dedup_key: String,
    pub coalesce_count: i32,
    pub state: String,
    pub snooze_until: Option<String>,
}

type InboxMap = HashMap<(String, String, String), RoutedInboxItem>;

#[derive(Clone, Default)]
pub struct InboxProjection {
    inner: Arc<Mutex<InboxMap>>,
}

impl InboxProjection {
    pub fn new() -> InboxProjection {
        InboxProjection::default()
    }

    fn upsert(&self, mut item: RoutedInboxItem) {
        let key = (
            item.tenant.0.clone(),
            item.recipient.clone(),
            item.dedup_key.clone(),
        );
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match guard.get_mut(&key) {
            Some(existing) => {
                existing.coalesce_count += 1;
            }
            None => {
                item.coalesce_count = 1;
                guard.insert(key, item);
            }
        }
    }

    #[doc(hidden)]
    pub fn upsert_for_test(&self, item: RoutedInboxItem) {
        self.upsert(item);
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, tenant: &TenantId, recipient: &str, dedup_key: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&(
                tenant.0.clone(),
                recipient.to_string(),
                dedup_key.to_string(),
            ))
    }

    pub fn get(
        &self,
        tenant: &TenantId,
        recipient: &str,
        dedup_key: &str,
    ) -> Option<RoutedInboxItem> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(
                tenant.0.clone(),
                recipient.to_string(),
                dedup_key.to_string(),
            ))
            .cloned()
    }

    pub fn mutate_state<F: FnOnce(&mut RoutedInboxItem)>(
        &self,
        tenant: &TenantId,
        recipient: &str,
        item_id: &str,
        f: F,
    ) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for row in guard.values_mut() {
            if row.tenant == *tenant && row.recipient == recipient && row.item_id == item_id {
                f(row);
                return true;
            }
        }
        false
    }

    pub fn mutate_matching<S, F>(
        &self,
        tenant: &TenantId,
        recipient: &str,
        select: S,
        mut f: F,
    ) -> usize
    where
        S: Fn(&RoutedInboxItem) -> bool,
        F: FnMut(&mut RoutedInboxItem),
    {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut n = 0;
        for row in guard.values_mut() {
            if row.tenant == *tenant && row.recipient == recipient && select(row) {
                f(row);
                n += 1;
            }
        }
        n
    }

    pub fn snapshot_for_tenant(&self, tenant: &TenantId) -> Vec<RoutedInboxItem> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|row| row.tenant == *tenant)
            .cloned()
            .collect()
    }

    pub fn wipe_tenant(&self, tenant: &TenantId) -> usize {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let before = guard.len();
        guard.retain(|_, row| row.tenant != *tenant);
        before - guard.len()
    }
}

impl RoutedInboxItem {
    pub fn references_subject(&self, subject_id: &str) -> bool {
        self.recipient == subject_id
            || self
                .subject
                .0
                .ends_with(&format!("/principal/{subject_id}"))
            || self
                .origin_event
                .0
                .ends_with(&format!("/principal/{subject_id}"))
    }
}

pub struct SignalRouter {
    bound_tenant: TenantId,
    expected_region: Option<String>,
    inbox: InboxProjection,
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
    durable: Option<DurableRouting>,
    storm: StormControl,
    hot_cap: HotSubjectCap,
    ambient: crate::read_fanout::AmbientMarkerStore,
    subjects: Vec<SubjectPattern>,
}

#[derive(Clone)]
struct DurableRouting {
    inbox: PgInboxStore,
    runtime: tokio::runtime::Handle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteError {
    MalformedSignal(String),
    Transient(String),
}

impl SignalRouter {
    fn new(
        bound_tenant: TenantId,
        inbox: InboxProjection,
        outbox: OutboxStore,
        minter: Arc<dyn IdMinter>,
        subjects: impl AsRef<[SubjectPattern]>,
    ) -> SignalRouter {
        SignalRouter {
            bound_tenant,
            expected_region: None,
            inbox,
            outbox,
            minter,
            durable: None,
            storm: StormControl::new(),
            hot_cap: HotSubjectCap::new(),
            ambient: crate::read_fanout::AmbientMarkerStore::new(),
            subjects: subjects.as_ref().to_vec(),
        }
    }

    fn new_durable(
        bound_tenant: TenantId,
        expected_region: String,
        inbox: PgInboxStore,
        outbox: OutboxStore,
        minter: Arc<dyn IdMinter>,
        runtime: tokio::runtime::Handle,
        subjects: impl AsRef<[SubjectPattern]>,
    ) -> SignalRouter {
        let mut router = SignalRouter::new(
            bound_tenant,
            InboxProjection::new(),
            outbox,
            minter,
            subjects,
        );
        router.expected_region = Some(expected_region);
        router.durable = Some(DurableRouting { inbox, runtime });
        router
    }

    pub fn inbox(&self) -> &InboxProjection {
        &self.inbox
    }

    pub fn ambient(&self) -> &crate::read_fanout::AmbientMarkerStore {
        &self.ambient
    }

    pub fn storm(&self) -> &StormControl {
        &self.storm
    }

    pub fn hot_cap(&self) -> &HotSubjectCap {
        &self.hot_cap
    }

    fn route(
        &self,
        signal_event: &EventEnvelope,
        handler_tx: &mut myelin_events::HandlerTx<'_>,
    ) -> Result<(), RouteError> {
        let signal: Signal = serde_json::from_value(signal_event.payload.clone())
            .map_err(|e| RouteError::MalformedSignal(e.to_string()))?;
        if signal.tenant != signal_event.tenant {
            return Err(RouteError::MalformedSignal(
                "signal tenant does not match its envelope".into(),
            ));
        }

        if signal.state == SignalState::Resolved {
            return self.resolve(signal_event, &signal, handler_tx);
        }

        self.write_fanout(signal_event, &signal, handler_tx)?;

        self.ambient.record(
            &signal.tenant,
            &signal.subject,
            Reason::Watched,
            &ArtifactRef(format!(
                "myelin://{}/bus/event/{}",
                signal_event.tenant.0, signal_event.event_id.0
            )),
        );

        let item = self.derive_item(signal_event, &signal);
        let subject_root = subject_root_of(&item.subject.0);
        self.route_one_candidate(signal_event, item, &subject_root, handler_tx)?;
        Ok(())
    }

    fn resolve(
        &self,
        signal_event: &EventEnvelope,
        signal: &Signal,
        handler_tx: &mut myelin_events::HandlerTx<'_>,
    ) -> Result<(), RouteError> {
        let mentions = mentions_of(signal_event);
        if !mentions.is_empty() {
            let reason = notification_reason_of(signal_event)?;
            for principal in &mentions {
                let item = self.derive_direct_item(signal_event, signal, principal, reason);
                self.mark_done(&item, handler_tx)?;
            }
        }
        self.mark_done(&self.derive_item(signal_event, signal), handler_tx)?;
        Ok(())
    }

    fn mark_done(
        &self,
        item: &RoutedInboxItem,
        handler_tx: &mut myelin_events::HandlerTx<'_>,
    ) -> Result<(), RouteError> {
        if let Some(durable) = &self.durable {
            durable
                .inbox
                .co_commit_mark_done(handler_tx, item, &durable.runtime)
                .map_err(|_| RouteError::Transient("durable inbox resolution failed".into()))?;
        } else {
            self.inbox.mutate_matching(
                &item.tenant,
                &item.recipient,
                |row| row.dedup_key == item.dedup_key,
                |row| {
                    row.state = "done".into();
                    row.snooze_until = None;
                },
            );
        }
        Ok(())
    }

    fn write_fanout(
        &self,
        signal_event: &EventEnvelope,
        signal: &Signal,
        handler_tx: &mut myelin_events::HandlerTx<'_>,
    ) -> Result<(), RouteError> {
        let mentions = mentions_of(signal_event);
        if mentions.is_empty() {
            return Ok(());
        }
        let reason = notification_reason_of(signal_event)?;
        let subject_root = subject_root_of(&signal.subject.0);
        for (index, principal) in mentions.iter().enumerate() {
            let item = self.derive_direct_item(signal_event, signal, principal, reason);
            let cap_verdict = if self.durable.is_some() {
                if index < self.hot_cap.cap() as usize {
                    CapVerdict::Admit
                } else {
                    CapVerdict::Overflow
                }
            } else {
                self.hot_cap.admit(&item.recipient, &subject_root)
            };
            match cap_verdict {
                CapVerdict::Overflow => {
                    continue;
                }
                CapVerdict::Admit => {
                    self.route_one_candidate(signal_event, item, &subject_root, handler_tx)?;
                }
            }
        }
        Ok(())
    }

    fn route_one_candidate(
        &self,
        signal_event: &EventEnvelope,
        item: RoutedInboxItem,
        subject_root: &str,
        handler_tx: &mut myelin_events::HandlerTx<'_>,
    ) -> Result<StormDecision, RouteError> {
        let recipient = item.recipient.clone();
        let dedup_key = item.dedup_key.clone();

        let row_exists = match &self.durable {
            Some(durable) => durable
                .inbox
                .co_commit_contains(handler_tx, &item, &durable.runtime)
                .map_err(|_| RouteError::Transient("durable inbox lookup failed".into()))?,
            None => self.inbox.contains(&item.tenant, &recipient, &dedup_key),
        };
        let decision = if self.durable.is_some() {
            if is_self_notification(signal_event, &item.recipient) {
                StormDecision::Suppress(SuppressReason::SelfAction)
            } else if row_exists {
                StormDecision::Collapse
            } else {
                StormDecision::Deliver
            }
        } else {
            let quiet = QuietHours::default();
            let storm_ctx = StormContext {
                tick: 0,
                utc_minute_of_day: 0,
                utc_weekday: 0,
                quiet: &quiet,
                rate: RateConfig::default(),
            };
            self.storm
                .decide(signal_event, &item, subject_root, row_exists, &storm_ctx)
        };

        if !decision.writes_row() {
            return Ok(decision);
        }

        if let Some(durable) = &self.durable {
            let input = InboxUpsert {
                item: item.clone(),
                subject_root: ArtifactRef(subject_root.to_string()),
                template_key: reason_template_key(item.reason).to_string(),
                template_args: vec![item.subject.clone()],
                occurred_at: signal_event.occurred_at.0.clone(),
                dek_ref: format!("kms://{}/notif/inbox", item.tenant.0),
            };
            let outcome = durable
                .inbox
                .co_commit_upsert(handler_tx, &input, &durable.runtime)
                .map_err(|_| RouteError::Transient("durable inbox write failed".into()))?;
            let committed_decision = match outcome {
                InboxUpsertOutcome::Inserted => decision,
                InboxUpsertOutcome::Collapsed { .. } => StormDecision::Collapse,
            };
            if committed_decision.delivers() {
                let mut detached =
                    OutboxTransaction::detached(self.minter.clone(), emit_base_from(signal_event));
                detached
                    .emit(self.item_created_draft(&item), Some(signal_event))
                    .map_err(|_| RouteError::Transient("outbox event derivation failed".into()))?;
                let rows = detached
                    .into_staged_rows()
                    .map_err(|_| RouteError::Transient("outbox event staging failed".into()))?;
                let conn = handler_tx
                    .connection::<sqlx::PgConnection>()
                    .ok_or_else(|| RouteError::Transient("durable co-commit tx missing".into()))?;
                tokio::task::block_in_place(|| {
                    durable.runtime.block_on(
                        myelin_storage::pgrelay::PgRelay::co_commit_rows_in_tx(conn, &rows),
                    )
                })
                .map_err(|_| RouteError::Transient("durable outbox co-commit failed".into()))?;
            }
            return Ok(committed_decision);
        }

        let mut tx = self
            .outbox
            .begin(self.minter.clone(), emit_base_from(signal_event));

        self.inbox.upsert(item.clone());
        tx.stage_state_change(format!(
            "UPSERT notif_inbox_item ({}, {}, {})",
            item.tenant.0, recipient, dedup_key
        ));

        if decision.delivers() {
            tx.emit(self.item_created_draft(&item), Some(signal_event))
                .map_err(|e| RouteError::Transient(format!("outbox emit failed: {e:?}")))?;
        }

        tx.commit()
            .map_err(|e| RouteError::Transient(format!("outbox commit failed: {e:?}")))?;

        Ok(decision)
    }

    fn derive_direct_item(
        &self,
        env: &EventEnvelope,
        signal: &Signal,
        principal: &Principal,
        reason: Reason,
    ) -> RoutedInboxItem {
        let recipient = principal.principal_id.0.clone();
        // `mention:` is a durable namespace: retaining it prevents an upgrade from
        // recreating existing mention rows under new item IDs. Other explicit direct
        // reasons use their own namespace.
        let direct_namespace = if reason == Reason::Mentioned {
            "mention"
        } else {
            "direct"
        };
        let dedup_key = format!(
            "{direct_namespace}:{}:{}:{}",
            signal.rule_id.0, signal.dedup_key.0, recipient
        );
        let item_id = item_id_for(&env.tenant, &recipient, &dedup_key);
        RoutedInboxItem {
            tenant: env.tenant.clone(),
            region: env.region.clone(),
            item_id,
            recipient,
            subject: signal.subject.clone(),
            reason,
            class: reason_base_class(reason).1,
            origin_event: ArtifactRef(format!(
                "myelin://{}/bus/event/{}",
                env.tenant.0, env.event_id.0
            )),
            dedup_key,
            coalesce_count: 1,
            state: "unread".to_string(),
            snooze_until: None,
        }
    }

    fn derive_item(&self, env: &EventEnvelope, signal: &Signal) -> RoutedInboxItem {
        let recipient = format!("psn:watcher:{}", signal.rule_id.0);
        let dedup_key = format!("{}:{}", signal.rule_id.0, signal.dedup_key.0);
        let item_id = item_id_for(&env.tenant, &recipient, &dedup_key);
        RoutedInboxItem {
            tenant: env.tenant.clone(),
            region: env.region.clone(),
            item_id,
            recipient,
            subject: signal.subject.clone(),
            reason: Reason::StateChanged,
            class: class_from_severity(signal.severity),
            origin_event: ArtifactRef(format!(
                "myelin://{}/bus/event/{}",
                env.tenant.0, env.event_id.0
            )),
            dedup_key,
            coalesce_count: 1,
            state: "unread".to_string(),
            snooze_until: None,
        }
    }

    fn item_created_draft(&self, item: &RoutedInboxItem) -> EventDraft {
        EventDraft {
            type_: EventType(NOTIF_ITEM_CREATED.into()),
            subject: item.subject.clone(),
            aggregate: AggregateKey(format!("notif-item:{}", item.item_id)),
            payload: serde_json::json!({
                "item_id": item.item_id,
                "recipient": item.recipient,
                "subject": item.subject.0,
                "subject_root": item.subject.0,
                "reason": serde_json::to_value(item.reason).unwrap_or(serde_json::Value::Null),
                "class": serde_json::to_value(item.class).unwrap_or(serde_json::Value::Null),
                "origin_event": item.origin_event.0,
                "dedup_key": item.dedup_key,
                "state": item.state,
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }
}

fn notification_reason_of(signal_event: &EventEnvelope) -> Result<Reason, RouteError> {
    match signal_event.payload.get("notification_reason") {
        None => Ok(Reason::Mentioned),
        Some(value) => serde_json::from_value(value.clone()).map_err(|_| {
            RouteError::MalformedSignal(
                "signal notification_reason is outside the closed reason vocabulary".into(),
            )
        }),
    }
}

impl EventHandler for SignalRouter {
    fn subjects(&self) -> &[SubjectPattern] {
        &self.subjects
    }

    fn handle(&self, ev: &EventEnvelope, tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        if ev.tenant != self.bound_tenant
            || self
                .expected_region
                .as_ref()
                .is_some_and(|region| ev.region.0 != *region)
        {
            return HandleOutcome::NonRetryable(BusReason(
                "signal envelope is outside the router's tenant/region binding".into(),
            ));
        }
        match self.route(ev, tx) {
            Ok(_) => HandleOutcome::Done,
            Err(RouteError::MalformedSignal(why)) => HandleOutcome::NonRetryable(BusReason(why)),
            Err(RouteError::Transient(_why)) => {
                HandleOutcome::Retry(myelin_events::Backoff { seconds: 2 })
            }
        }
    }
}

pub fn build_router(
    tenant: &TenantId,
    inbox: InboxProjection,
    outbox: OutboxStore,
    dedup: DedupLedger,
) -> Result<Consumer<SignalRouter>, SubscribeError> {
    let prefix = signal_subject_prefix(tenant)
        .ok_or_else(|| SubscribeError::WildcardSubject(format!("sig.{}.", tenant.0)))?;
    let subjects = vec![SubjectPattern(prefix.clone())];
    let router = SignalRouter::new(
        tenant.clone(),
        inbox,
        outbox,
        Arc::new(MonotonicMinter::new()),
        subjects,
    );
    consume(
        ConsumerSpec::new(
            ConsumerName(ROUTER_CONSUMER_NAME.into()),
            &[prefix.as_str()],
        ),
        router,
        dedup,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_durable_router(
    tenant: &TenantId,
    expected_region: impl Into<String>,
    inbox: PgInboxStore,
    outbox: OutboxStore,
    dedup: DedupLedger,
    dead_letters: Arc<dyn myelin_events::DurableDeadLetter>,
    minter: Arc<dyn IdMinter>,
    runtime: tokio::runtime::Handle,
) -> Result<Consumer<SignalRouter>, SubscribeError> {
    let prefix = signal_subject_prefix(tenant)
        .ok_or_else(|| SubscribeError::WildcardSubject(format!("sig.{}.", tenant.0)))?;
    let subjects = vec![SubjectPattern(prefix.clone())];
    let router = SignalRouter::new_durable(
        tenant.clone(),
        expected_region.into(),
        inbox,
        outbox,
        minter,
        runtime,
        subjects,
    );
    consume(
        ConsumerSpec::new(
            ConsumerName(ROUTER_CONSUMER_NAME.into()),
            &[prefix.as_str()],
        ),
        router,
        dedup,
    )
    .map(|consumer| {
        consumer.with_dead_letter_sink(myelin_events::DeadLetterSink::durable(dead_letters))
    })
}

fn class_from_severity(severity: Severity) -> Class {
    match severity {
        Severity::Critical => Class::Critical,
        Severity::Error | Severity::Warning => Class::Direct,
        Severity::Notice | Severity::Info => Class::Fyi,
    }
}

fn item_id_for(tenant: &TenantId, recipient: &str, dedup_key: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    tenant.0.hash(&mut h);
    0u8.hash(&mut h);
    recipient.hash(&mut h);
    0u8.hash(&mut h);
    dedup_key.hash(&mut h);
    format!("itm-{:016x}", h.finish())
}

pub const SIGNAL_MENTIONS_KEY: &str = "mentions";

fn mentions_of(env: &EventEnvelope) -> Vec<Principal> {
    let Some(value) = env.payload.get(SIGNAL_MENTIONS_KEY) else {
        return Vec::new();
    };
    let nodes: Vec<InlineNode> = serde_json::from_value(value.clone()).unwrap_or_default();
    extract_mentions(&nodes)
}

fn emit_base_from(env: &EventEnvelope) -> EmitContextBase {
    EmitContextBase {
        tenant: env.tenant.clone(),
        region: env.region.clone(),
        actor: env.actor.clone(),
        schema_ver: 1,
        occurred_at: env.occurred_at.clone(),
        recorded_at: env.recorded_at.clone(),
        caused_by: env.caused_by.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, BusTransport, CorrelationId, DedupLedger, Delivered, EventId, InProcessBus, Message,
        Relay, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_query::signals::{DedupKey, RuleId, SignalState};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-1".into()),
            PrincipalKind::Human,
            tenant(),
        )
    }

    fn signal(rule: &str, severity: Severity, subject: &str, dedup: &str) -> Signal {
        Signal {
            rule_id: RuleId(rule.into()),
            tenant: tenant(),
            severity,
            dedup_key: DedupKey(dedup.into()),
            subject: ArtifactRef(subject.into()),
            count: 1,
            state: SignalState::Open,
            first_seen: "2026-06-20T00:00:00Z".into(),
            last_seen: "2026-06-20T00:00:00Z".into(),
        }
    }

    fn signal_envelope(id: &str, sig: &Signal) -> EventEnvelope {
        let subject = format!(
            "sig.{}.{}.{}",
            sig.tenant.0,
            sig.severity.token(),
            sig.rule_id.0
        );
        EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType("signal.opened".into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(principal()),
            subject: ArtifactRef(subject),
            aggregate: AggregateKey(format!("signal:{}", sig.dedup_key.0)),
            causation_id: None,
            correlation_id: CorrelationId(id.into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload: serde_json::to_value(sig).unwrap(),
        }
    }

    fn signal_msg(id: &str, sig: &Signal) -> Message {
        let env = signal_envelope(id, sig);
        Message {
            subject: env.subject.0.clone(),
            envelope: env,
        }
    }

    fn router_over(outbox: &OutboxStore) -> (Consumer<SignalRouter>, InboxProjection) {
        let inbox = InboxProjection::new();
        let consumer =
            build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();
        (consumer, inbox)
    }

    #[test]
    fn signal_subject_prefix_is_per_tenant_never_wildcard() {
        assert_eq!(signal_subject_prefix(&tenant()), Some("sig.acme.".into()));
        assert_eq!(
            signal_subject_prefix(&TenantId("".into())),
            None,
            "empty tenant refused"
        );
        assert_eq!(
            signal_subject_prefix(&TenantId("a.b".into())),
            None,
            "a dotted tenant (extra segments → aliasing) is refused"
        );
        assert!(!signal_subject_prefix(&tenant()).unwrap().contains('*'));
        assert!(!signal_subject_prefix(&tenant()).unwrap().contains('>'));
    }

    #[test]
    fn build_router_binds_sig_tenant_whitelist_never_star() {
        let outbox = OutboxStore::new();
        let (consumer, _) = router_over(&outbox);
        assert_eq!(consumer.name(), &ConsumerName(ROUTER_CONSUMER_NAME.into()));
        assert_eq!(
            consumer.handler().subjects(),
            &[SubjectPattern("sig.acme.".into())],
            "the whitelist is the sig.<tenant>. prefix (never `*`)"
        );
        assert!(is_signal_subject("sig.acme.error.ci_run_failed", &tenant()));
        assert!(!is_signal_subject(
            "sig.other.error.ci_run_failed",
            &tenant()
        ));
    }

    #[test]
    fn build_router_refuses_overbroad_tenant() {
        let r = build_router(
            &TenantId("".into()),
            InboxProjection::new(),
            OutboxStore::new(),
            DedupLedger::new(),
        );
        assert!(
            matches!(r, Err(SubscribeError::WildcardSubject(_))),
            "an empty tenant is refused"
        );
    }

    #[test]
    fn signal_upserts_one_item_and_emits_notif_item_created() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let sig = signal(
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/42",
            "run-42",
        );

        assert_eq!(
            consumer.deliver(&signal_msg("evt-1", &sig)),
            Delivered::Acked
        );
        assert_eq!(inbox.len(), 1, "one inbox row UPSERTed");

        assert_eq!(
            outbox.committed_count(),
            1,
            "one event committed (the emit)"
        );
        let row = inbox
            .get(
                &tenant(),
                "psn:watcher:ci_run_failed",
                "ci_run_failed:run-42",
            )
            .expect("the UPSERTed row exists at its (tenant, recipient, dedup_key) key");
        assert_eq!(
            row.coalesce_count, 1,
            "a fresh row starts at coalesce_count = 1"
        );
        assert_eq!(
            row.state, "unread",
            "a fresh inbox row is unread (the ONE read-state column)"
        );
        assert_eq!(
            row.class,
            Class::Direct,
            "an `error` Signal maps to the Direct class"
        );
        assert_eq!(row.subject.0, "myelin://acme/ci/run/42");
    }

    #[test]
    fn router_rejects_payload_or_envelope_tenant_outside_its_binding() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let signal = signal(
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/42",
            "run-42",
        );

        let mut payload_mismatch = signal_envelope("evt-tenant-payload", &signal);
        let mut wrong_signal = signal.clone();
        wrong_signal.tenant = TenantId("other".into());
        payload_mismatch.payload = serde_json::to_value(wrong_signal).unwrap();
        assert!(matches!(
            consumer.deliver(&Message {
                subject: payload_mismatch.subject.0.clone(),
                envelope: payload_mismatch,
            }),
            Delivered::DeadLettered(_)
        ));

        let mut envelope_mismatch = signal_envelope("evt-tenant-envelope", &signal);
        envelope_mismatch.tenant = TenantId("other".into());
        assert!(matches!(
            consumer.deliver(&Message {
                subject: envelope_mismatch.subject.0.clone(),
                envelope: envelope_mismatch,
            }),
            Delivered::DeadLettered(_)
        ));
        assert!(inbox.is_empty());
        assert_eq!(outbox.committed_count(), 0);
    }

    #[test]
    fn emitted_event_is_notif_item_created_refs_not_payloads_caused_by_signal() {
        let outbox = OutboxStore::new();
        let (consumer, _) = router_over(&outbox);
        let sig = signal(
            "ci_run_failed",
            Severity::Critical,
            "myelin://acme/ci/run/7",
            "run-7",
        );
        let env = signal_envelope("evt-c1", &sig);
        consumer.deliver(&Message {
            subject: env.subject.0.clone(),
            envelope: env.clone(),
        });

        let bus = InProcessBus::new();
        let relay = Relay::new(outbox.clone(), bus.clone(), || {
            Timestamp("2026-06-20T00:00:02Z".into())
        });
        relay.drain_to_empty();
        let published = bus.consume("");
        assert_eq!(published.len(), 1, "exactly one notif.item.created emitted");
        let emitted = &published[0];
        assert_eq!(emitted.type_.0, NOTIF_ITEM_CREATED);
        assert!(
            !emitted.contains_personal_data,
            "references-not-payloads: no inline PII"
        );
        assert!(emitted.pii_key_ref.is_none());
        assert_eq!(
            emitted.correlation_id, env.correlation_id,
            "the correlation root carries from the Signal"
        );
        assert_eq!(
            emitted.causation_id,
            Some(env.event_id.clone()),
            "causation = the Signal"
        );
        assert_eq!(
            emitted.depth,
            env.depth + 1,
            "depth+1 (the loop-guard stamp)"
        );
        assert_eq!(emitted.tenant, env.tenant);
        assert_eq!(emitted.region, env.region);
    }

    #[test]
    fn redelivered_signal_is_deduped_one_row_one_emit() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let sig = signal(
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/42",
            "run-42",
        );
        let m = signal_msg("evt-dup", &sig);

        assert_eq!(
            consumer.deliver(&m),
            Delivered::Acked,
            "first delivery routes + acks"
        );
        assert_eq!(
            consumer.deliver(&m),
            Delivered::Deduplicated,
            "redelivery is deduped (0 dup)"
        );
        assert_eq!(consumer.deliver(&m), Delivered::Deduplicated, "and again");
        assert_eq!(
            inbox.len(),
            1,
            "exactly one inbox row (the redelivery did not double-notify)"
        );
        assert_eq!(
            outbox.committed_count(),
            1,
            "exactly one emit (the redelivery emitted nothing)"
        );
    }

    #[test]
    fn same_key_signals_collapse_to_one_row_coalesce_count_bumps() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let sig = signal(
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/42",
            "run-42",
        );
        assert_eq!(
            consumer.deliver(&signal_msg("evt-a", &sig)),
            Delivered::Acked
        );
        assert_eq!(
            consumer.deliver(&signal_msg("evt-b", &sig)),
            Delivered::Acked
        );

        assert_eq!(
            inbox.len(),
            1,
            "same (tenant, recipient, dedup_key) → ONE row (collapse, §3.2)"
        );
        let row = inbox
            .get(
                &tenant(),
                "psn:watcher:ci_run_failed",
                "ci_run_failed:run-42",
            )
            .unwrap();
        assert_eq!(
            row.coalesce_count, 2,
            "the second same-key Signal bumped coalesce_count to 2"
        );
    }

    #[test]
    fn distinct_keys_open_distinct_rows() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let a = signal(
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/1",
            "run-1",
        );
        let b = signal(
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/2",
            "run-2",
        );
        consumer.deliver(&signal_msg("evt-1", &a));
        consumer.deliver(&signal_msg("evt-2", &b));
        assert_eq!(
            inbox.len(),
            2,
            "two distinct runs → two distinct inbox rows"
        );
    }

    #[test]
    fn notif_d10_poison_signal_does_not_stall_other_subjects() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);

        let poison = Message {
            subject: "sig.acme.error.broken_rule".into(),
            envelope: EventEnvelope {
                payload: serde_json::json!({ "not": "a signal" }),
                ..signal_envelope(
                    "evt-poison",
                    &signal("x", Severity::Error, "myelin://acme/ci/run/0", "k"),
                )
            },
        };
        let good = signal(
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/42",
            "run-42",
        );
        let good_msg = signal_msg("evt-good", &good);

        let out = consumer.deliver(&poison);
        assert!(
            matches!(out, Delivered::DeadLettered(_)),
            "the poison terminates (NonRetryable)"
        );
        assert_eq!(
            consumer.dead_letters().len(),
            1,
            "the poison is SURFACED, not silently dropped"
        );

        assert_eq!(
            consumer.deliver(&good_msg),
            Delivered::Acked,
            "the good Signal is not blocked"
        );
        assert_eq!(
            inbox.len(),
            1,
            "the good Signal UPSERTed its row (the poison wrote none)"
        );

        assert_eq!(
            consumer.lag(),
            0,
            "NOTIF-D10: 0 head-of-line stalls; lag recovered to 0"
        );
        assert_eq!(
            outbox.committed_count(),
            1,
            "only the good Signal emitted (the poison did not)"
        );
    }

    #[test]
    fn poison_redelivery_is_deduped_not_repoisoned() {
        let outbox = OutboxStore::new();
        let (consumer, _) = router_over(&outbox);
        let poison = Message {
            subject: "sig.acme.error.broken".into(),
            envelope: EventEnvelope {
                payload: serde_json::json!({ "bad": true }),
                ..signal_envelope(
                    "evt-p",
                    &signal("x", Severity::Error, "myelin://acme/ci/run/0", "k"),
                )
            },
        };
        assert!(matches!(
            consumer.deliver(&poison),
            Delivered::DeadLettered(_)
        ));
        assert_eq!(
            consumer.deliver(&poison),
            Delivered::Deduplicated,
            "a re-delivered poison dedups"
        );
        assert_eq!(
            consumer.dead_letters().len(),
            1,
            "still exactly one dead-letter (not re-poisoned)"
        );
    }

    #[test]
    fn class_from_severity_is_the_frozen_skeleton_mapping() {
        assert_eq!(class_from_severity(Severity::Critical), Class::Critical);
        assert_eq!(class_from_severity(Severity::Error), Class::Direct);
        assert_eq!(class_from_severity(Severity::Warning), Class::Direct);
        assert_eq!(class_from_severity(Severity::Notice), Class::Fyi);
        assert_eq!(class_from_severity(Severity::Info), Class::Fyi);
    }

    #[test]
    fn item_id_is_deterministic_and_field_unambiguous() {
        let t = tenant();
        let a = item_id_for(&t, "psn:alice", "k1");
        assert_eq!(
            a,
            item_id_for(&t, "psn:alice", "k1"),
            "the same tuple → the same id (idempotent)"
        );
        assert_ne!(
            a,
            item_id_for(&t, "psn:alice", "k2"),
            "a different dedup_key → a different id"
        );
        assert_ne!(
            a,
            item_id_for(&t, "psn:bob", "k1"),
            "a different recipient → a different id"
        );
        assert_ne!(
            a,
            item_id_for(&TenantId("other".into()), "psn:alice", "k1"),
            "tenant-scoped id"
        );
        assert_ne!(
            item_id_for(&t, "ab", "c"),
            item_id_for(&t, "a", "bc"),
            "field boundaries are unambiguous (NUL-separated)"
        );
    }

    #[test]
    fn inbox_projection_is_empty_len_and_router_inbox_accessor_track_state() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        assert!(inbox.is_empty(), "a fresh projection is empty");
        assert_eq!(inbox.len(), 0);
        assert!(consumer.handler().inbox().is_empty());

        let sig = signal(
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/42",
            "run-42",
        );
        consumer.deliver(&signal_msg("evt-1", &sig));
        assert!(
            !inbox.is_empty(),
            "after a route the projection is NOT empty"
        );
        assert_eq!(inbox.len(), 1);
        assert!(
            !consumer.handler().inbox().is_empty(),
            "router.inbox() is the live projection"
        );
        assert_eq!(consumer.handler().inbox().len(), 1);
    }

    #[test]
    fn router_emit_tokens_are_frozen() {
        assert_eq!(NOTIF_ITEM_CREATED, "notif.item.created");
        assert_eq!(NOTIF_ESCALATION_ACKED, "notif.escalation.acked");
        assert!(myelin_events::validate_event_type(NOTIF_ITEM_CREATED).is_ok());
        assert!(myelin_events::validate_event_type(NOTIF_ESCALATION_ACKED).is_ok());
        assert_eq!(ROUTER_CONSUMER_NAME, "notif-signal-router");
    }

    use myelin_content::InlineNode;

    fn signal_msg_with_mentions(id: &str, sig: &Signal, mentions: &[Principal]) -> Message {
        let mut env = signal_envelope(id, sig);
        let nodes: Vec<InlineNode> = mentions.iter().cloned().map(InlineNode::Mention).collect();
        if let serde_json::Value::Object(map) = &mut env.payload {
            map.insert(
                SIGNAL_MENTIONS_KEY.into(),
                serde_json::to_value(&nodes).unwrap(),
            );
        }
        Message {
            subject: env.subject.0.clone(),
            envelope: env,
        }
    }

    fn mentioned(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }

    #[test]
    fn write_fanout_materialises_one_item_per_mentioned_recipient() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let sig = signal(
            "pr_review",
            Severity::Info,
            "myelin://acme/git/pr/9",
            "pr-9",
        );
        let mentions = [
            mentioned("p-alice"),
            mentioned("p-bob"),
            mentioned("p-carol"),
        ];

        assert_eq!(
            consumer.deliver(&signal_msg_with_mentions("evt-m1", &sig, &mentions)),
            Delivered::Acked
        );

        for p in &mentions {
            let dedup = format!("mention:pr_review:pr-9:{}", p.principal_id.0);
            let row = inbox
                .get(&tenant(), &p.principal_id.0, &dedup)
                .unwrap_or_else(|| panic!("a mention row for {}", p.principal_id.0));
            assert_eq!(
                row.reason,
                Reason::Mentioned,
                "a mention → reason Mentioned"
            );
            assert_eq!(
                row.class,
                Class::Direct,
                "a mention is directly addressed → Direct"
            );
            assert_eq!(
                row.recipient, p.principal_id.0,
                "the recipient is the mentioned principal"
            );
            assert_eq!(row.subject.0, "myelin://acme/git/pr/9");
        }
        assert_eq!(
            inbox.len(),
            4,
            "one row per mentioned recipient (3) + the ambient row (1)"
        );
    }

    #[test]
    fn write_fanout_repeat_mention_collapses_one_row_per_recipient() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let sig = signal(
            "pr_review",
            Severity::Info,
            "myelin://acme/git/pr/9",
            "pr-9",
        );
        let mentions = [mentioned("p-alice")];

        consumer.deliver(&signal_msg_with_mentions("evt-a", &sig, &mentions));
        consumer.deliver(&signal_msg_with_mentions("evt-b", &sig, &mentions));

        let dedup = "mention:pr_review:pr-9:p-alice";
        let row = inbox.get(&tenant(), "p-alice", dedup).unwrap();
        assert_eq!(
            row.coalesce_count, 2,
            "the repeated mention collapsed (one row, count 2)"
        );
        let alice_rows = inbox
            .snapshot_for_tenant(&tenant())
            .into_iter()
            .filter(|r| r.recipient == "p-alice")
            .count();
        assert_eq!(
            alice_rows, 1,
            "exactly one row for the mentioned recipient (no duplicate)"
        );
    }

    #[test]
    fn no_mention_nodes_means_no_write_fanout() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let sig = signal(
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/42",
            "run-42",
        );
        consumer.deliver(&signal_msg("evt-1", &sig));
        assert_eq!(
            inbox.len(),
            1,
            "only the ambient skeleton row - 0 mention write-fanout rows"
        );
    }

    #[test]
    fn direct_signal_reason_is_explicit_closed_and_defaults_to_mentioned() {
        let outbox = OutboxStore::new();
        let (consumer, inbox) = router_over(&outbox);
        let sig = signal(
            "git.review_requested",
            Severity::Notice,
            "myelin://acme/git/pr/core:9",
            "review-core-9",
        );
        let mut envelope = signal_envelope("evt-review", &sig);
        assert_eq!(notification_reason_of(&envelope), Ok(Reason::Mentioned));

        envelope.payload["notification_reason"] = serde_json::json!("review_requested");
        assert_eq!(
            notification_reason_of(&envelope),
            Ok(Reason::ReviewRequested)
        );

        let mut message =
            signal_msg_with_mentions("evt-routed-review", &sig, &[mentioned("p-reviewer")]);
        message.envelope.payload["notification_reason"] = serde_json::json!("review_requested");
        assert_eq!(consumer.deliver(&message), Delivered::Acked);
        let row = inbox
            .get(
                &tenant(),
                "p-reviewer",
                "direct:git.review_requested:review-core-9:p-reviewer",
            )
            .expect("an explicit review request is routed to its recipient");
        assert_eq!(row.reason, Reason::ReviewRequested);
        assert_eq!(row.class, Class::Direct);

        let mut resolved_signal = sig.clone();
        resolved_signal.state = SignalState::Resolved;
        let mut resolved = signal_msg_with_mentions(
            "evt-resolved-review",
            &resolved_signal,
            &[mentioned("p-reviewer")],
        );
        resolved.envelope.type_ = EventType("signal.resolved".into());
        resolved.envelope.payload["notification_reason"] = serde_json::json!("review_requested");
        assert_eq!(consumer.deliver(&resolved), Delivered::Acked);
        let row = inbox
            .get(
                &tenant(),
                "p-reviewer",
                "direct:git.review_requested:review-core-9:p-reviewer",
            )
            .expect("the resolved review request retains its durable inbox history");
        assert_eq!(row.state, "done");

        envelope.payload["notification_reason"] = serde_json::json!("invented_reason");
        assert!(matches!(
            notification_reason_of(&envelope),
            Err(RouteError::MalformedSignal(_))
        ));
    }

    #[test]
    fn write_fanout_hot_subject_cap_bounds_a_mention_storm() {
        let outbox = OutboxStore::new();
        let inbox = InboxProjection::new();
        let mut router = SignalRouter::new(
            tenant(),
            inbox.clone(),
            outbox.clone(),
            Arc::new(MonotonicMinter::new()),
            [SubjectPattern("sig.acme.".into())],
        );
        router.hot_cap = HotSubjectCap::with_cap(5);

        let sig = signal(
            "mention_spray",
            Severity::Info,
            "myelin://acme/chat/thread/hot",
            "spray",
        );
        let storm: Vec<Principal> = (0..50).map(|i| mentioned(&format!("p-{i}"))).collect();
        let _ = router.route(
            &signal_envelope("evt-storm", &sig),
            &mut myelin_events::HandlerTx::none(),
        );

        let subject_root = "myelin://acme/chat/thread/hot";
        let env = {
            let mut e = signal_envelope("evt-storm-2", &sig);
            let nodes: Vec<InlineNode> = storm.iter().cloned().map(InlineNode::Mention).collect();
            if let serde_json::Value::Object(map) = &mut e.payload {
                map.insert(
                    SIGNAL_MENTIONS_KEY.into(),
                    serde_json::to_value(&nodes).unwrap(),
                );
            }
            e
        };
        router
            .write_fanout(&env, &sig, &mut myelin_events::HandlerTx::none())
            .unwrap();

        assert_eq!(
            router.hot_cap().admitted_count(subject_root),
            5,
            "the mention-storm is bounded to `cap` write rows (0 unbounded write amplification)"
        );
        assert_eq!(
            router.hot_cap().overflow_count(subject_root),
            45,
            "the rest overflowed into the coalesced marker (the +N more were mentioned counter)"
        );
        let mention_rows = inbox
            .snapshot_for_tenant(&tenant())
            .into_iter()
            .filter(|r| r.reason == Reason::Mentioned)
            .count();
        assert_eq!(
            mention_rows, 5,
            "exactly `cap` mention rows materialised (bounded write-fanout)"
        );
    }

    #[test]
    fn signal_mentions_key_is_frozen() {
        assert_eq!(SIGNAL_MENTIONS_KEY, "mentions");
    }
}
