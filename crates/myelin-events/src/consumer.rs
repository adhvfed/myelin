use crate::dead_letter::{DeadLetterError, DeadLetterRecord, DeadLetterSink};
use crate::{
    DedupError, DedupLedger, EventEnvelope, EventHandler, HandleOutcome, HandlerTx, Reason,
    SubjectPattern,
};
use myelin_tenancy::TenantId;
use std::collections::HashMap;
use std::sync::Mutex;

pub fn install_payload_free_panic_hook(service: &'static str) {
    std::panic::set_hook(Box::new(move |panic| {
        if let Some(location) = panic.location() {
            eprintln!(
                "{service}: an internal task panicked; payload suppressed; location={}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            );
        } else {
            eprintln!("{service}: an internal task panicked; payload suppressed; location=unknown");
        }
    }));
}

const DEFAULT_COMMIT_RETRY_BACKOFF_SECS: u64 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConsumerName(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PrefetchBound(u32);

impl PrefetchBound {
    pub const DEFAULT: PrefetchBound = PrefetchBound(64);

    pub fn new(n: u32) -> Option<PrefetchBound> {
        if n == 0 {
            None
        } else {
            Some(PrefetchBound(n))
        }
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl Default for PrefetchBound {
    fn default() -> Self {
        PrefetchBound::DEFAULT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PerTenantInflight(u32);

impl PerTenantInflight {
    pub const DEFAULT: PerTenantInflight = PerTenantInflight(16);

    pub fn new(n: u32) -> Option<PerTenantInflight> {
        if n == 0 {
            None
        } else {
            Some(PerTenantInflight(n))
        }
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl Default for PerTenantInflight {
    fn default() -> Self {
        PerTenantInflight::DEFAULT
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsumerSpec {
    pub durable: ConsumerName,
    pub subjects: Vec<String>,
    pub max_ack_pending: PrefetchBound,
    pub per_tenant_inflight: PerTenantInflight,
}

impl ConsumerSpec {
    pub fn new(durable: ConsumerName, subjects: &[&str]) -> ConsumerSpec {
        ConsumerSpec {
            durable,
            subjects: subjects.iter().map(|s| (*s).to_string()).collect(),
            max_ack_pending: PrefetchBound::DEFAULT,
            per_tenant_inflight: PerTenantInflight::DEFAULT,
        }
    }
}

pub fn consume<H: EventHandler>(
    spec: ConsumerSpec,
    handler: H,
    dedup: DedupLedger,
) -> Result<Consumer<H>, SubscribeError> {
    let subjects: Vec<&str> = spec.subjects.iter().map(|s| s.as_str()).collect();
    let subscription = Subscription::bind(spec.durable, &subjects, spec.max_ack_pending)?;
    Ok(Consumer::new(handler, subscription, dedup)
        .with_per_tenant_inflight(spec.per_tenant_inflight))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubscribeError {
    WildcardSubject(String),
    NoSubjects,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subscription {
    name: ConsumerName,
    subjects: Vec<SubjectPattern>,
    prefetch: PrefetchBound,
}

impl Subscription {
    pub fn bind(
        name: ConsumerName,
        subjects: &[&str],
        prefetch: PrefetchBound,
    ) -> Result<Subscription, SubscribeError> {
        if subjects.is_empty() {
            return Err(SubscribeError::NoSubjects);
        }
        for s in subjects {
            if is_wildcard_subject(s) {
                return Err(SubscribeError::WildcardSubject((*s).to_string()));
            }
        }
        Ok(Subscription {
            name,
            subjects: subjects
                .iter()
                .map(|s| SubjectPattern((*s).to_string()))
                .collect(),
            prefetch,
        })
    }

    pub fn name(&self) -> &ConsumerName {
        &self.name
    }

    pub fn subjects(&self) -> &[SubjectPattern] {
        &self.subjects
    }

    pub fn prefetch(&self) -> PrefetchBound {
        self.prefetch
    }

    pub fn matches(&self, subject: &str) -> bool {
        self.subjects.iter().any(|p| subject.starts_with(&p.0))
    }
}

fn is_wildcard_subject(s: &str) -> bool {
    s.is_empty() || s.split('.').any(|seg| seg == "*" || seg == ">")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivered {
    Acked,
    Deduplicated,
    DeadLettered(Reason),
    Retried(u64),
    DependencyUnavailable(crate::relay::IntakeDependency, u64),
    Throttled(TenantId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub subject: String,
    pub envelope: EventEnvelope,
}

type Upcaster = Box<dyn Fn(EventEnvelope) -> Result<EventEnvelope, Reason> + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadLetter {
    pub envelope: EventEnvelope,
    pub reason: Reason,
}

pub struct Consumer<H: EventHandler> {
    handler: H,
    subscription: Subscription,
    dedup: DedupLedger,
    upcaster: Upcaster,
    pending: Mutex<HashMap<String, std::collections::HashSet<crate::EventId>>>,
    dead_letters: DeadLetterSink,
    per_tenant_cap: PerTenantInflight,
    tenant_inflight: Mutex<HashMap<TenantId, std::collections::HashSet<crate::EventId>>>,
}

impl<H: EventHandler> Consumer<H> {
    pub fn new(handler: H, subscription: Subscription, dedup: DedupLedger) -> Self {
        Consumer {
            handler,
            subscription,
            dedup,
            upcaster: Box::new(Ok),
            pending: Mutex::new(HashMap::new()),
            dead_letters: DeadLetterSink::in_memory(),
            per_tenant_cap: PerTenantInflight::DEFAULT,
            tenant_inflight: Mutex::new(HashMap::new()),
        }
    }

    pub fn accepts(&self, subject: &str) -> bool {
        self.subscription.matches(subject)
    }

    pub fn is_handled(&self, event_id: &crate::EventId) -> Result<bool, DedupError> {
        self.dedup.is_handled(self.name(), event_id)
    }

    pub fn dead_letter_exhausted_retry(&self, msg: &Message, delivery_attempt: u64) -> Delivered {
        let reason = Reason(format!(
            "retry budget exhausted after {delivery_attempt} broker deliveries"
        ));
        match self.push_dead_letter(msg.envelope.clone(), reason.clone()) {
            Ok(()) => {
                self.clear_pending(&msg.subject, &msg.envelope.event_id);
                self.clear_tenant_inflight(&msg.envelope.tenant, &msg.envelope.event_id);
                Delivered::DeadLettered(reason)
            }
            Err(_) => {
                self.bump_pending(&msg.subject, &msg.envelope.event_id);
                Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS)
            }
        }
    }

    pub fn with_per_tenant_inflight(mut self, cap: PerTenantInflight) -> Self {
        self.per_tenant_cap = cap;
        self
    }

    pub fn per_tenant_inflight_cap(&self) -> PerTenantInflight {
        self.per_tenant_cap
    }

    pub fn with_dead_letter_sink(mut self, sink: DeadLetterSink) -> Self {
        self.dead_letters = sink;
        self
    }

    pub fn tenant_inflight(&self, tenant: &TenantId) -> u32 {
        self.tenant_inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tenant)
            .map(|s| s.len() as u32)
            .unwrap_or(0)
    }

    pub fn with_upcaster(
        mut self,
        upcaster: impl Fn(EventEnvelope) -> Result<EventEnvelope, Reason> + Send + Sync + 'static,
    ) -> Self {
        self.upcaster = Box::new(upcaster);
        self
    }

    pub fn name(&self) -> &ConsumerName {
        self.subscription.name()
    }

    pub fn dedup(&self) -> &DedupLedger {
        &self.dedup
    }

    pub fn handler(&self) -> &H {
        &self.handler
    }

    pub fn lag(&self) -> u64 {
        self.pending()
            .values()
            .map(|events| events.len() as u64)
            .sum()
    }

    pub fn lag_on(&self, subject: &str) -> u64 {
        self.pending()
            .get(subject)
            .map(|events| events.len() as u64)
            .unwrap_or(0)
    }

    pub fn dead_letters(&self) -> Vec<DeadLetter> {
        self.dead_letters.surfaced()
    }

    pub fn durable_dead_letters(&self) -> Result<Vec<DeadLetterRecord>, DeadLetterError> {
        self.dead_letters.durable_dead_letters(self.name())
    }

    pub fn deliver(&self, msg: &Message) -> Delivered {
        if !self.subscription.matches(&msg.subject) {
            let reason = Reason(format!("subject {} not on consumer whitelist", msg.subject));
            return match self.push_dead_letter(msg.envelope.clone(), reason.clone()) {
                Ok(()) => {
                    self.clear_pending(&msg.subject, &msg.envelope.event_id);
                    Delivered::DeadLettered(reason)
                }
                Err(_) => {
                    self.bump_pending(&msg.subject, &msg.envelope.event_id);
                    Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS)
                }
            };
        }

        let tenant = msg.envelope.tenant.clone();
        let event_id = msg.envelope.event_id.clone();
        if !self.tenant_has_inflight(&tenant, &event_id)
            && self.tenant_inflight(&tenant) >= self.per_tenant_cap.get()
        {
            self.bump_pending(&msg.subject, &event_id);
            return Delivered::Throttled(tenant);
        }

        let envelope = match (self.upcaster)(msg.envelope.clone()) {
            Ok(env) => env,
            Err(reason) => {
                return match self.push_dead_letter(msg.envelope.clone(), reason.clone()) {
                    Ok(()) => {
                        self.clear_pending(&msg.subject, &event_id);
                        Delivered::DeadLettered(reason)
                    }
                    Err(_) => {
                        self.bump_pending(&msg.subject, &event_id);
                        Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS)
                    }
                };
            }
        };
        debug_assert_eq!(
            envelope.event_id, event_id,
            "an upcaster never changes event_id"
        );

        let (mut cotx, fresh) =
            match self
                .dedup
                .begin_co_commit(self.name(), &event_id, &tenant, &msg.envelope.region)
            {
                Ok(acquired) => acquired,
                Err(DedupError::Unavailable) => {
                    self.bump_pending(&msg.subject, &event_id);
                    return Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS);
                }
            };
        if !fresh {
            cotx.rollback();
            self.clear_pending(&msg.subject, &event_id);
            return Delivered::Deduplicated;
        }

        self.bump_tenant_inflight(&tenant, &event_id);

        let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut htx = match cotx.connection() {
                Some(conn) => HandlerTx::with_connection(conn),
                None => HandlerTx::none(),
            };
            self.handler.handle(&envelope, &mut htx)
        }));
        let outcome = match handled {
            Ok(outcome) => outcome,
            Err(_) => {
                cotx.rollback();
                eprintln!(
                    "[consumer:{}] handler PANICKED for event {} - dead-lettered (no tombstone; effect \
                     replayable); payload suppressed",
                    self.name().0,
                    event_id.0
                );
                let reason = Reason(
                    "handler PANICKED (a bug, dead-lettered without a dedup tombstone so the valid \
                     effect stays replayable; panic payload suppressed at production log boundaries)"
                        .to_string(),
                );
                return match self.push_dead_letter(envelope, reason.clone()) {
                    Ok(()) => {
                        self.clear_pending(&msg.subject, &event_id);
                        self.clear_tenant_inflight(&tenant, &event_id);
                        Delivered::DeadLettered(reason)
                    }
                    Err(_) => {
                        self.bump_pending(&msg.subject, &event_id);
                        Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS)
                    }
                };
            }
        };
        match outcome {
            HandleOutcome::Done => match cotx.commit() {
                Ok(()) => {
                    self.clear_pending(&msg.subject, &event_id);
                    self.clear_tenant_inflight(&tenant, &event_id);
                    Delivered::Acked
                }
                Err(_e) => {
                    self.bump_pending(&msg.subject, &event_id);
                    Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS)
                }
            },
            HandleOutcome::NonRetryable(reason) => {
                cotx.rollback();
                match self.push_dead_letter(envelope, reason.clone()) {
                    Ok(()) => match self.dedup.mark_handled(self.name(), &event_id) {
                        Ok(_) => {
                            self.clear_pending(&msg.subject, &event_id);
                            self.clear_tenant_inflight(&tenant, &event_id);
                            Delivered::DeadLettered(reason)
                        }
                        Err(DedupError::Unavailable) => {
                            self.bump_pending(&msg.subject, &event_id);
                            Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS)
                        }
                    },
                    Err(_) => {
                        self.bump_pending(&msg.subject, &event_id);
                        Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS)
                    }
                }
            }
            HandleOutcome::Retry(backoff) => {
                cotx.rollback();
                self.bump_pending(&msg.subject, &event_id);
                Delivered::Retried(backoff.seconds)
            }
            HandleOutcome::DependencyUnavailable {
                dependency,
                backoff,
            } => {
                cotx.rollback();
                self.bump_pending(&msg.subject, &event_id);
                Delivered::DependencyUnavailable(dependency, backoff.seconds)
            }
        }
    }

    pub fn deliver_lane(&self, subject: &str, lane: &[Message]) -> Vec<Delivered> {
        let bound = self.subscription.prefetch().get() as usize;
        lane.iter()
            .take(bound)
            .map(|m| {
                debug_assert_eq!(m.subject, subject, "a lane carries one subject's messages");
                self.deliver(m)
            })
            .collect()
    }

    fn push_dead_letter(
        &self,
        envelope: EventEnvelope,
        reason: Reason,
    ) -> Result<(), DeadLetterError> {
        self.dead_letters
            .push(self.name(), DeadLetter { envelope, reason })
    }

    fn pending(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, std::collections::HashSet<crate::EventId>>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn bump_pending(&self, subject: &str, event_id: &crate::EventId) {
        self.pending()
            .entry(subject.to_string())
            .or_default()
            .insert(event_id.clone());
    }

    fn clear_pending(&self, subject: &str, event_id: &crate::EventId) {
        let mut pending = self.pending();
        if let Some(events) = pending.get_mut(subject) {
            events.remove(event_id);
            if events.is_empty() {
                pending.remove(subject);
            }
        }
    }

    fn tenant_has_inflight(&self, tenant: &TenantId, event_id: &crate::EventId) -> bool {
        self.tenant_inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tenant)
            .map(|s| s.contains(event_id))
            .unwrap_or(false)
    }

    fn bump_tenant_inflight(&self, tenant: &TenantId, event_id: &crate::EventId) {
        self.tenant_inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(tenant.clone())
            .or_default()
            .insert(event_id.clone());
    }

    fn clear_tenant_inflight(&self, tenant: &TenantId, event_id: &crate::EventId) {
        let mut guard = self
            .tenant_inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(set) = guard.get_mut(tenant) {
            set.remove(event_id);
            if set.is_empty() {
                guard.remove(tenant);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Actor, AggregateKey, ArtifactRef, Backoff, CausedBy, CorrelationId, DataRole, EventId,
        EventType, PiiKeyRef, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;

    fn available<T>(result: Result<T, DedupError>) -> T {
        result.expect("in-memory dedup storage is available")
    }

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn envelope(id: &str, subject: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType("issues.issue.created".into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(principal()),
            subject: ArtifactRef(subject.into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            causation_id: None,
            correlation_id: CorrelationId(id.into()),
            caused_by: Some(CausedBy("session:abc".into())),
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None::<PiiKeyRef>,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            payload: serde_json::json!({ "ref": "x" }),
        }
    }

    fn msg(id: &str, subject: &str) -> Message {
        Message {
            subject: subject.into(),
            envelope: envelope(id, subject),
        }
    }

    struct CountingHandler {
        runs: AtomicU32,
        subjects: &'static [SubjectPattern],
        outcome: fn(&EventEnvelope) -> HandleOutcome,
    }
    impl EventHandler for CountingHandler {
        fn subjects(&self) -> &'static [SubjectPattern] {
            self.subjects
        }
        fn handle(&self, ev: &EventEnvelope, _tx: &mut HandlerTx<'_>) -> HandleOutcome {
            self.runs.fetch_add(1, Ordering::SeqCst);
            (self.outcome)(ev)
        }
    }

    static SUBJECTS: &[SubjectPattern] = &[];

    fn done_handler() -> CountingHandler {
        CountingHandler {
            runs: AtomicU32::new(0),
            subjects: SUBJECTS,
            outcome: |_| HandleOutcome::Done,
        }
    }

    fn sub(name: &str, subjects: &[&str]) -> Subscription {
        Subscription::bind(ConsumerName(name.into()), subjects, PrefetchBound::DEFAULT).unwrap()
    }

    #[test]
    fn wildcard_subscription_is_rejected_at_registration() {
        let name = ConsumerName("indexer".into());
        for bad in ["*", ">", "issues.*", "issues.>", "issues.*.created", ""] {
            let r = Subscription::bind(name.clone(), &[bad], PrefetchBound::DEFAULT);
            assert!(
                matches!(r, Err(SubscribeError::WildcardSubject(_))),
                "`{bad}` must be rejected"
            );
        }
        assert_eq!(
            Subscription::bind(name.clone(), &[], PrefetchBound::DEFAULT),
            Err(SubscribeError::NoSubjects)
        );
        let ok = Subscription::bind(name, &["issues.issue.created"], PrefetchBound::DEFAULT);
        assert!(ok.is_ok(), "a concrete (non-wildcard) subject is admitted");
    }

    #[test]
    fn subscription_matches_only_whitelisted_subjects() {
        let s = sub("indexer", &["myelin://acme/issues/"]);
        assert!(s.matches("myelin://acme/issues/issue/PROJ-1"));
        assert!(
            !s.matches("myelin://acme/chat/message/1"),
            "off-whitelist subject does not match"
        );
    }

    #[test]
    fn subscription_carries_name_subjects_and_prefetch() {
        let s = Subscription::bind(
            ConsumerName("indexer".into()),
            &["myelin://acme/issues/", "myelin://acme/refs/"],
            PrefetchBound::new(8).unwrap(),
        )
        .unwrap();
        assert_eq!(s.name(), &ConsumerName("indexer".into()));
        assert_eq!(s.prefetch().get(), 8);
        assert_eq!(
            s.subjects(),
            &[
                SubjectPattern("myelin://acme/issues/".into()),
                SubjectPattern("myelin://acme/refs/".into()),
            ],
            "the whitelist is exactly the (non-wildcard) subjects bound"
        );
    }

    #[test]
    fn consumer_routing_and_handled_status_accessors_reflect_live_state() {
        let consumer = Consumer::new(
            done_handler(),
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        );
        let message = msg("01J-STATUS", "myelin://acme/issues/issue/PROJ-1");

        assert!(consumer.accepts(&message.subject));
        assert!(!consumer.accepts("myelin://acme/chat/message/1"));
        assert!(!available(consumer.is_handled(&message.envelope.event_id)));
        assert_eq!(consumer.deliver(&message), Delivered::Acked);
        assert!(available(consumer.is_handled(&message.envelope.event_id)));
    }

    #[test]
    fn redelivered_event_id_is_a_no_op_handler_runs_once() {
        let h = done_handler();
        let c = Consumer::new(
            h,
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        );
        let m = msg("01J-1", "myelin://acme/issues/issue/PROJ-1");

        assert_eq!(
            c.deliver(&m),
            Delivered::Acked,
            "first delivery runs + acks"
        );
        assert_eq!(
            c.deliver(&m),
            Delivered::Deduplicated,
            "redelivery is deduped"
        );
        assert_eq!(c.deliver(&m), Delivered::Deduplicated, "and again");
        assert_eq!(
            c.handler.runs.load(Ordering::SeqCst),
            1,
            "the handler ran EXACTLY once"
        );
        assert_eq!(c.dedup().len(), 1, "one (consumer, event_id) pair recorded");
    }

    #[test]
    fn dedup_key_is_per_consumer_two_consumers_each_process_once() {
        let ledger = DedupLedger::new();
        let a = Consumer::new(
            done_handler(),
            sub("indexer", &["myelin://acme/issues/"]),
            ledger.clone(),
        );
        let b = Consumer::new(
            done_handler(),
            sub("notifier", &["myelin://acme/issues/"]),
            ledger.clone(),
        );
        let m = msg("01J-1", "myelin://acme/issues/issue/PROJ-1");

        assert_eq!(a.deliver(&m), Delivered::Acked, "consumer A processes it");
        assert_eq!(
            b.deliver(&m),
            Delivered::Acked,
            "consumer B ALSO processes it (different PK)"
        );
        assert_eq!(a.deliver(&m), Delivered::Deduplicated);
        assert_eq!(b.deliver(&m), Delivered::Deduplicated);
        assert_eq!(ledger.len(), 2, "two distinct (consumer, event_id) pairs");
    }

    #[test]
    fn dedup_ledger_is_empty_and_is_handled_track_state() {
        let ledger = DedupLedger::new();
        let consumer = ConsumerName("indexer".into());
        let id = EventId("01J-1".into());
        assert!(ledger.is_empty(), "a fresh ledger is empty");
        assert!(
            !available(ledger.is_handled(&consumer, &id)),
            "nothing handled yet"
        );

        assert!(
            available(ledger.mark_handled(&consumer, &id)),
            "first mark is fresh"
        );
        assert!(
            !ledger.is_empty(),
            "the ledger is no longer empty after a mark"
        );
        assert!(
            available(ledger.is_handled(&consumer, &id)),
            "the exact pair is handled"
        );
        assert!(!available(
            ledger.is_handled(&ConsumerName("other".into()), &id)
        ));
        assert!(
            !available(ledger.mark_handled(&consumer, &id)),
            "re-mark is a duplicate, not fresh"
        );
    }

    #[test]
    fn reconnect_rebinds_by_name_zero_lost_zero_dup() {
        let ledger = DedupLedger::new();
        let m1 = msg("01J-1", "myelin://acme/issues/issue/PROJ-1");
        let m2 = msg("01J-2", "myelin://acme/issues/issue/PROJ-1");

        {
            let c = Consumer::new(
                done_handler(),
                sub("indexer", &["myelin://acme/issues/"]),
                ledger.clone(),
            );
            assert_eq!(c.deliver(&m1), Delivered::Acked);
        }

        let h = done_handler();
        let c2 = Consumer::new(
            h,
            sub("indexer", &["myelin://acme/issues/"]),
            ledger.clone(),
        );
        assert_eq!(
            c2.deliver(&m1),
            Delivered::Deduplicated,
            "m1 already handled → 0 dup"
        );
        assert_eq!(
            c2.deliver(&m2),
            Delivered::Acked,
            "m2 handled after reconnect → 0 lost"
        );
        assert_eq!(
            c2.handler.runs.load(Ordering::SeqCst),
            1,
            "only m2 re-ran the handler"
        );
        assert_eq!(ledger.len(), 2, "both events are now in the ledger");
    }

    #[test]
    fn poison_message_dead_letters_immediately_and_is_surfaced() {
        let h = CountingHandler {
            runs: AtomicU32::new(0),
            subjects: SUBJECTS,
            outcome: |_| HandleOutcome::NonRetryable(Reason("malformed".into())),
        };
        let c = Consumer::new(
            h,
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        );
        let poison = msg("01J-bad", "myelin://acme/issues/issue/PROJ-1");

        let out = c.deliver(&poison);
        assert_eq!(out, Delivered::DeadLettered(Reason("malformed".into())));
        assert_eq!(
            c.dead_letters().len(),
            1,
            "the poison is SURFACED, not silently dropped"
        );
        assert_eq!(c.dead_letters()[0].reason, Reason("malformed".into()));
        assert_eq!(
            c.lag(),
            0,
            "a dead-lettered message does not sit in lag (it is terminal)"
        );

        assert_eq!(
            c.deliver(&poison),
            Delivered::Deduplicated,
            "a redelivered dead-letter is deduped"
        );
        assert_eq!(
            c.dead_letters().len(),
            1,
            "still exactly one dead-letter (not re-poisoned)"
        );
    }

    #[test]
    fn durable_dead_letter_failure_is_retryable_never_terminally_acked() {
        struct FailingDlq;
        impl crate::DurableDeadLetter for FailingDlq {
            fn record(
                &self,
                _consumer: &ConsumerName,
                _event_id: &crate::EventId,
                _reason: &str,
            ) -> Result<(), crate::DeadLetterError> {
                Err(crate::DeadLetterError::Unavailable)
            }

            fn dead_letters(
                &self,
                _consumer: &ConsumerName,
            ) -> Result<Vec<crate::DeadLetterRecord>, crate::DeadLetterError> {
                Err(crate::DeadLetterError::Unavailable)
            }
        }

        let h = CountingHandler {
            runs: AtomicU32::new(0),
            subjects: SUBJECTS,
            outcome: |_| HandleOutcome::NonRetryable(Reason("malformed".into())),
        };
        let ledger = DedupLedger::new();
        let c = Consumer::new(
            h,
            sub("indexer", &["myelin://acme/issues/"]),
            ledger.clone(),
        )
        .with_dead_letter_sink(DeadLetterSink::durable(Arc::new(FailingDlq)));
        let poison = msg("01J-dlq-down", "myelin://acme/issues/issue/PROJ-1");

        assert_eq!(
            c.deliver(&poison),
            Delivered::Retried(DEFAULT_COMMIT_RETRY_BACKOFF_SECS),
            "a failed durable quarantine write is non-terminal"
        );
        assert!(!available(
            ledger.is_handled(c.name(), &poison.envelope.event_id)
        ));
        assert_eq!(c.lag(), 1, "the unacked poison remains visible as lag");
        assert_eq!(
            c.dead_letters().len(),
            1,
            "the process-local fallback remains operator-visible without authorizing ack"
        );
    }

    #[test]
    fn slow_or_poison_subject_does_not_block_a_fast_one() {
        let h = CountingHandler {
            runs: AtomicU32::new(0),
            subjects: SUBJECTS,
            outcome: |ev| {
                if ev.subject.0.contains("/A/") {
                    HandleOutcome::NonRetryable(Reason("poison A".into()))
                } else {
                    HandleOutcome::Done
                }
            },
        };
        let c = Consumer::new(
            h,
            sub("indexer", &["myelin://acme/A/", "myelin://acme/B/"]),
            DedupLedger::new(),
        );
        let a = msg("01J-A", "myelin://acme/A/x");
        let b = msg("01J-B", "myelin://acme/B/y");

        assert!(matches!(c.deliver(&a), Delivered::DeadLettered(_)));
        assert_eq!(
            c.deliver(&b),
            Delivered::Acked,
            "subject B is not head-of-line-blocked by A"
        );
        assert_eq!(
            c.lag_on("myelin://acme/A/x"),
            0,
            "the poison subject did not accumulate lag"
        );
        assert_eq!(c.lag_on("myelin://acme/B/y"), 0, "B drained");
    }

    #[test]
    fn multiple_retries_track_one_pending_event_then_success_clears_lag() {
        struct Flaky {
            calls: AtomicU32,
        }
        impl EventHandler for Flaky {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(&self, _ev: &EventEnvelope, _tx: &mut HandlerTx<'_>) -> HandleOutcome {
                if self.calls.fetch_add(1, Ordering::SeqCst) < 2 {
                    HandleOutcome::Retry(Backoff { seconds: 2 })
                } else {
                    HandleOutcome::Done
                }
            }
        }
        let c = Consumer::new(
            Flaky {
                calls: AtomicU32::new(0),
            },
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        );
        let m = msg("01J-1", "myelin://acme/issues/issue/PROJ-1");

        assert_eq!(c.deliver(&m), Delivered::Retried(2));
        assert_eq!(c.lag(), 1, "an un-acked retry sits in consumer lag");
        assert!(
            !available(c.dedup().is_handled(c.name(), &m.envelope.event_id)),
            "a retry leaves NO dedup mark"
        );

        assert_eq!(c.deliver(&m), Delivered::Retried(2));
        assert_eq!(c.lag(), 1, "redelivery does not double-count pending lag");

        assert_eq!(
            c.deliver(&m),
            Delivered::Acked,
            "the redelivery re-ran the handler and succeeded"
        );
        assert_eq!(
            c.lag(),
            0,
            "lag recovers to 0 after the successful redelivery (SUB-D2)"
        );
        assert!(
            available(c.dedup().is_handled(c.name(), &m.envelope.event_id)),
            "now it is durably handled"
        );
    }

    #[test]
    fn prefetch_bound_rejects_zero() {
        assert_eq!(
            PrefetchBound::new(0),
            None,
            "a zero prefetch is meaningless, rejected"
        );
        assert_eq!(PrefetchBound::new(8).unwrap().get(), 8);
        assert_eq!(PrefetchBound::DEFAULT.get(), 64);
    }

    #[test]
    fn deliver_lane_honours_bounded_prefetch() {
        let bound = PrefetchBound::new(2).unwrap();
        let s = Subscription::bind(
            ConsumerName("indexer".into()),
            &["myelin://acme/issues/"],
            bound,
        )
        .unwrap();
        let c = Consumer::new(done_handler(), s, DedupLedger::new());

        let lane: Vec<Message> = (0..5)
            .map(|i| msg(&format!("01J-{i}"), "myelin://acme/issues/issue/PROJ-1"))
            .collect();
        let out = c.deliver_lane("myelin://acme/issues/issue/PROJ-1", &lane);
        assert_eq!(
            out.len(),
            2,
            "bounded prefetch: only 2 of 5 delivered this drain"
        );
        assert!(out.iter().all(|o| *o == Delivered::Acked));
        assert_eq!(
            c.handler.runs.load(Ordering::SeqCst),
            2,
            "the handler ran exactly twice"
        );
    }

    #[test]
    fn upcaster_runs_before_handle() {
        let seen_ver = Arc::new(AtomicU32::new(0));
        let seen2 = seen_ver.clone();
        struct VerHandler {
            seen: Arc<AtomicU32>,
        }
        impl EventHandler for VerHandler {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(&self, ev: &EventEnvelope, _tx: &mut HandlerTx<'_>) -> HandleOutcome {
                self.seen.store(ev.schema_ver, Ordering::SeqCst);
                HandleOutcome::Done
            }
        }
        let c = Consumer::new(
            VerHandler { seen: seen2 },
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        )
        .with_upcaster(|mut e| {
            if e.schema_ver == 1 {
                e.schema_ver = 2;
            }
            Ok(e)
        });
        c.deliver(&msg("01J-1", "myelin://acme/issues/issue/PROJ-1"));
        assert_eq!(
            seen_ver.load(Ordering::SeqCst),
            2,
            "the handler saw the upcasted schema_ver"
        );
    }

    #[test]
    fn unbridgeable_gap_dead_letters_loudly_never_silently_passes() {
        let h = done_handler();
        let c = Consumer::new(
            h,
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        )
        .with_upcaster(|_e| Err(Reason("unbridgeable schema gap: no upcaster".into())));

        let m = msg("01J-gap", "myelin://acme/issues/issue/PROJ-1");
        let out = c.deliver(&m);

        assert!(
            matches!(out, Delivered::DeadLettered(_)),
            "a gap dead-letters → DLQ"
        );
        assert_eq!(
            c.handler.runs.load(Ordering::SeqCst),
            0,
            "the handler never saw the wrong shape"
        );
        assert_eq!(
            c.dead_letters().len(),
            1,
            "the gap is surfaced, not silently dropped"
        );
        assert!(
            available(c.dedup().mark_handled(c.name(), &EventId("01J-gap".into()))),
            "the gapped event_id is still FRESH - it was never marked handled"
        );
    }

    #[test]
    fn off_whitelist_message_is_dead_lettered_not_silently_processed() {
        let c = Consumer::new(
            done_handler(),
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        );
        let off = msg("01J-off", "myelin://acme/chat/message/1");
        assert!(matches!(c.deliver(&off), Delivered::DeadLettered(_)));
        assert_eq!(
            c.handler.runs.load(Ordering::SeqCst),
            0,
            "the handler never ran for an off-whitelist subject"
        );
        assert_eq!(c.dead_letters().len(), 1);
    }

    #[test]
    fn off_whitelist_dlq_failure_then_success_clears_pending_lag() {
        struct FailOnce(AtomicBool);
        impl crate::DurableDeadLetter for FailOnce {
            fn record(
                &self,
                _consumer: &ConsumerName,
                _event_id: &crate::EventId,
                _reason: &str,
            ) -> Result<(), crate::DeadLetterError> {
                if self.0.swap(false, Ordering::SeqCst) {
                    Err(crate::DeadLetterError::Unavailable)
                } else {
                    Ok(())
                }
            }
            fn dead_letters(
                &self,
                _consumer: &ConsumerName,
            ) -> Result<Vec<crate::DeadLetterRecord>, crate::DeadLetterError> {
                Ok(Vec::new())
            }
        }
        let c = Consumer::new(
            done_handler(),
            sub("indexer", &["myelin://acme/issues/"]),
            DedupLedger::new(),
        )
        .with_dead_letter_sink(DeadLetterSink::durable(Arc::new(FailOnce(
            AtomicBool::new(true),
        ))));
        let off = msg("01J-off-retry", "myelin://acme/chat/message/1");

        assert!(matches!(c.deliver(&off), Delivered::Retried(_)));
        assert_eq!(c.lag(), 1);
        assert!(matches!(c.deliver(&off), Delivered::DeadLettered(_)));
        assert_eq!(c.lag(), 0);
    }

    #[test]
    fn consume_entrypoint_validates_whitelist_and_wires_the_runtime() {
        let bad = ConsumerSpec::new(ConsumerName("indexer".into()), &["issues.*"]);
        assert!(matches!(
            consume(bad, done_handler(), DedupLedger::new()),
            Err(SubscribeError::WildcardSubject(_))
        ));

        let spec = ConsumerSpec {
            durable: ConsumerName("indexer".into()),
            subjects: vec!["myelin://acme/issues/".into()],
            max_ack_pending: PrefetchBound::new(8).unwrap(),
            per_tenant_inflight: PerTenantInflight::new(4).unwrap(),
        };
        let c = consume(spec, done_handler(), DedupLedger::new()).unwrap();
        assert_eq!(c.name(), &ConsumerName("indexer".into()));
        assert_eq!(
            c.per_tenant_inflight_cap().get(),
            4,
            "the per-tenant cap is wired from the spec"
        );

        assert_eq!(
            c.deliver(&msg("01J-1", "myelin://acme/issues/issue/PROJ-1")),
            Delivered::Acked
        );
    }

    #[test]
    fn per_tenant_inflight_rejects_zero() {
        assert_eq!(
            PerTenantInflight::new(0),
            None,
            "a zero per-tenant cap is meaningless, rejected"
        );
        assert_eq!(PerTenantInflight::new(4).unwrap().get(), 4);
        assert_eq!(PerTenantInflight::DEFAULT.get(), 16);
    }

    #[test]
    fn surging_tenant_is_throttled_at_its_cap_other_tenant_flows() {
        struct SurgeHandler;
        impl EventHandler for SurgeHandler {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(&self, ev: &EventEnvelope, _tx: &mut HandlerTx<'_>) -> HandleOutcome {
                if ev.tenant.0 == "surge" {
                    HandleOutcome::Retry(Backoff { seconds: 5 })
                } else {
                    HandleOutcome::Done
                }
            }
        }
        let spec = ConsumerSpec {
            durable: ConsumerName("indexer".into()),
            subjects: vec!["myelin://".into()],
            max_ack_pending: PrefetchBound::DEFAULT,
            per_tenant_inflight: PerTenantInflight::new(2).unwrap(),
        };
        let c = consume(spec, SurgeHandler, DedupLedger::new()).unwrap();

        let surge = |id: &str| Message {
            subject: "myelin://surge/issues/x".into(),
            envelope: tenant_envelope(id, "myelin://surge/issues/x", "surge"),
        };

        assert_eq!(c.deliver(&surge("01J-s1")), Delivered::Retried(5));
        assert_eq!(c.deliver(&surge("01J-s2")), Delivered::Retried(5));
        assert_eq!(
            c.tenant_inflight(&TenantId("surge".into())),
            2,
            "the surge tenant holds its 2 slots"
        );

        assert_eq!(
            c.deliver(&surge("01J-s3")),
            Delivered::Throttled(TenantId("surge".into())),
            "the surge tenant is bounded to its cap"
        );
        assert_eq!(
            c.tenant_inflight(&TenantId("surge".into())),
            2,
            "still 2 - the throttled message took no slot"
        );

        let other = Message {
            subject: "myelin://other/issues/y".into(),
            envelope: tenant_envelope("01J-o1", "myelin://other/issues/y", "other"),
        };
        assert_eq!(
            c.deliver(&other),
            Delivered::Acked,
            "the other tenant is not starved by the surge"
        );
        assert_eq!(
            c.tenant_inflight(&TenantId("other".into())),
            0,
            "the other tenant's Done released its slot"
        );
    }

    #[test]
    fn throttled_message_is_re_offerable_after_the_tenant_drains() {
        struct DrainHandler {
            failed: Mutex<HashSet<String>>,
        }
        impl EventHandler for DrainHandler {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(&self, ev: &EventEnvelope, _tx: &mut HandlerTx<'_>) -> HandleOutcome {
                let mut f = self.failed.lock().unwrap();
                if f.insert(ev.event_id.0.clone()) {
                    HandleOutcome::Retry(Backoff { seconds: 1 })
                } else {
                    HandleOutcome::Done
                }
            }
        }
        let spec = ConsumerSpec {
            durable: ConsumerName("indexer".into()),
            subjects: vec!["myelin://surge/".into()],
            max_ack_pending: PrefetchBound::DEFAULT,
            per_tenant_inflight: PerTenantInflight::new(1).unwrap(),
        };
        let c = consume(
            spec,
            DrainHandler {
                failed: Mutex::new(HashSet::new()),
            },
            DedupLedger::new(),
        )
        .unwrap();
        let m = |id: &str| Message {
            subject: "myelin://surge/x".into(),
            envelope: tenant_envelope(id, "myelin://surge/x", "surge"),
        };

        assert_eq!(c.deliver(&m("01J-1")), Delivered::Retried(1));
        assert_eq!(
            c.deliver(&m("01J-2")),
            Delivered::Throttled(TenantId("surge".into()))
        );

        assert_eq!(c.deliver(&m("01J-1")), Delivered::Acked);
        assert_eq!(
            c.tenant_inflight(&TenantId("surge".into())),
            0,
            "the slot freed"
        );

        assert_eq!(
            c.deliver(&m("01J-2")),
            Delivered::Retried(1),
            "the previously-throttled message is re-offered"
        );
        assert_eq!(
            c.deliver(&m("01J-2")),
            Delivered::Acked,
            "and eventually processed - 0 loss"
        );
    }

    fn tenant_envelope(id: &str, subject: &str, tenant: &str) -> EventEnvelope {
        let mut e = envelope(id, subject);
        e.tenant = TenantId(tenant.into());
        e
    }
}
