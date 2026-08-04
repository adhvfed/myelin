use crate::EventMatcher;
use myelin_events::{ArtifactRef, EventEnvelope, Visibility};
use myelin_identity::SetExpr;
use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::matcher::RelMembership;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Notice,
    Warning,
    Error,
    Critical,
}

impl Severity {
    pub fn token(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Notice => "notice",
            Severity::Warning => "warning",
            Severity::Error => "error",
            Severity::Critical => "critical",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RuleId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DedupKey(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DedupKeyTpl(pub String);

impl DedupKeyTpl {
    pub fn render(&self, envelope: &EventEnvelope) -> DedupKey {
        let src = &self.0;
        let mut out = String::with_capacity(src.len());
        let mut chars = src.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' if chars.peek() == Some(&'{') => {
                    chars.next();
                    out.push('{');
                }
                '}' if chars.peek() == Some(&'}') => {
                    chars.next();
                    out.push('}');
                }
                '{' => {
                    let mut name = String::new();
                    for nc in chars.by_ref() {
                        if nc == '}' {
                            break;
                        }
                        name.push(nc);
                    }
                    out.push_str(&project_field(envelope, name.trim()));
                }
                other => out.push(other),
            }
        }
        DedupKey(out)
    }
}

fn project_field(envelope: &EventEnvelope, name: &str) -> String {
    match name {
        "event.id" => envelope.event_id.0.clone(),
        "event.type" => envelope.type_.0.clone(),
        "event.subject" => envelope.subject.0.clone(),
        "event.tenant" => envelope.tenant.0.clone(),
        "event.region" => envelope.region.0.clone(),
        "event.correlation_id" => envelope.correlation_id.0.clone(),
        "event.aggregate" => envelope.aggregate.0.clone(),
        "event.visibility" => match envelope.visibility {
            Visibility::Public => "public",
            Visibility::Internal => "internal",
            Visibility::Private => "private",
        }
        .to_string(),
        _ => {
            if let Some(key) = name.strip_prefix("payload.") {
                if let serde_json::Value::Object(map) = &envelope.payload {
                    if let Some(v) = map.get(key) {
                        return match v {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            serde_json::Value::Number(n) => n.to_string(),
                            _ => "<missing>".to_string(),
                        };
                    }
                }
            }
            "<missing>".to_string()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DedupWindow {
    pub seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalRule {
    pub rule_id: RuleId,
    pub matcher: EventMatcher,
    pub severity: Severity,
    pub dedup_key_tpl: DedupKeyTpl,
    pub dedup_window: DedupWindow,
    pub resolves: Option<EventMatcher>,
}

pub fn define_signal_rule(
    rule_id: RuleId,
    matcher: EventMatcher,
    severity: Severity,
    dedup_key_tpl: DedupKeyTpl,
    dedup_window: DedupWindow,
    resolves: Option<EventMatcher>,
) -> SignalRule {
    SignalRule {
        rule_id,
        matcher,
        severity,
        dedup_key_tpl,
        dedup_window,
        resolves,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalState {
    Open,
    Resolved,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signal {
    pub rule_id: RuleId,
    pub tenant: TenantId,
    pub severity: Severity,
    pub dedup_key: DedupKey,
    pub subject: ArtifactRef,
    pub count: u64,
    pub state: SignalState,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishDraft {
    pub subject: String,
    pub signal: Signal,
    pub kind: PublishKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublishKind {
    Opened,
    Collapsed,
    Resolved,
}

fn publish_subject(tenant: &TenantId, severity: Severity, rule: &RuleId) -> String {
    format!("sig.{}.{}.{}", tenant.0, severity.token(), rule.0)
}

#[derive(Debug, Default)]
pub struct SignalEngine {
    rules: Vec<SignalRule>,
    store: BTreeMap<(TenantId, DedupKey), Signal>,
}

impl SignalEngine {
    pub fn new() -> SignalEngine {
        SignalEngine::default()
    }

    pub fn add_rule(&mut self, rule: SignalRule) -> &mut SignalEngine {
        self.rules.push(rule);
        self
    }

    pub fn signal(&self, tenant: &TenantId, dedup_key: &DedupKey) -> Option<&Signal> {
        self.store.get(&(tenant.clone(), dedup_key.clone()))
    }

    pub fn ingest(
        &mut self,
        envelope: &EventEnvelope,
        visible: &SetExpr,
        member_oracle: &dyn Fn(&RelMembership) -> bool,
    ) -> Vec<PublishDraft> {
        let mut drafts = Vec::new();
        let rules = self.rules.clone();
        for rule in &rules {
            if let Some(resolver) = &rule.resolves {
                if matches_fail_closed(resolver, envelope, visible, member_oracle) {
                    let key = rule.dedup_key_tpl.render(envelope);
                    let store_key = (envelope.tenant.clone(), key.clone());
                    if let Some(sig) = self.store.get_mut(&store_key) {
                        if sig.state == SignalState::Open {
                            sig.state = SignalState::Resolved;
                            sig.last_seen = envelope.recorded_at.0.clone();
                            drafts.push(PublishDraft {
                                subject: publish_subject(&sig.tenant, sig.severity, &sig.rule_id),
                                signal: sig.clone(),
                                kind: PublishKind::Resolved,
                            });
                        }
                    }
                    continue;
                }
            }

            if !matches_fail_closed(&rule.matcher, envelope, visible, member_oracle) {
                continue;
            }

            let key = rule.dedup_key_tpl.render(envelope);
            let store_key = (envelope.tenant.clone(), key.clone());
            let now = envelope.recorded_at.0.clone();

            let within_window = |first_seen: &str| -> bool {
                if rule.dedup_window.seconds == 0 {
                    return true;
                }
                match (parse_rfc3339_secs(first_seen), parse_rfc3339_secs(&now)) {
                    (Some(f), Some(n)) => n.saturating_sub(f) <= rule.dedup_window.seconds as i64,
                    _ => true,
                }
            };

            match self.store.get_mut(&store_key) {
                Some(sig) if sig.state == SignalState::Open && within_window(&sig.first_seen) => {
                    sig.count += 1;
                    sig.last_seen = now.clone();
                    drafts.push(PublishDraft {
                        subject: publish_subject(&sig.tenant, sig.severity, &sig.rule_id),
                        signal: sig.clone(),
                        kind: PublishKind::Collapsed,
                    });
                }
                _ => {
                    let sig = Signal {
                        rule_id: rule.rule_id.clone(),
                        tenant: envelope.tenant.clone(),
                        severity: rule.severity,
                        dedup_key: key.clone(),
                        subject: envelope.subject.clone(),
                        count: 1,
                        state: SignalState::Open,
                        first_seen: now.clone(),
                        last_seen: now.clone(),
                    };
                    drafts.push(PublishDraft {
                        subject: publish_subject(&sig.tenant, sig.severity, &sig.rule_id),
                        signal: sig.clone(),
                        kind: PublishKind::Opened,
                    });
                    self.store.insert(store_key, sig);
                }
            }
        }
        drafts
    }
}

fn matches_fail_closed(
    matcher: &EventMatcher,
    envelope: &EventEnvelope,
    visible: &SetExpr,
    member_oracle: &dyn Fn(&RelMembership) -> bool,
) -> bool {
    matcher
        .matches(envelope, visible, member_oracle)
        .unwrap_or(false)
}

fn parse_rfc3339_secs(ts: &str) -> Option<i64> {
    let bytes = ts.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let num = |s: &str| -> Option<i64> { s.parse::<i64>().ok() };
    let year = num(ts.get(0..4)?)?;
    let month = num(ts.get(5..7)?)?;
    let day = num(ts.get(8..10)?)?;
    let hour = num(ts.get(11..13)?)?;
    let min = num(ts.get(14..16)?)?;
    let sec = num(ts.get(17..19)?)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hour * 3600 + min * 60 + sec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CmpOp, Expr, Predicate};
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp,
    };
    use myelin_identity::{Literal, ObjectType, Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::Region;

    fn var(name: &str) -> Expr {
        Expr::Var(name.into())
    }
    fn str_(s: &str) -> Expr {
        Expr::Lit(Literal::Str(s.into()))
    }

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("alice".into()),
            PrincipalKind::Human,
            TenantId("t1".into()),
        )
    }

    fn type_matcher(type_: &str) -> EventMatcher {
        EventMatcher::compile(
            ObjectType("run".into()),
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("event.type"),
                rhs: str_(type_),
            },
        )
        .unwrap()
    }

    fn envelope_at(type_: &str, id: &str, recorded_at: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("evt-{id}-{recorded_at}")),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: TenantId("t1".into()),
            region: Region("fr-par".into()),
            actor: Actor(principal()),
            subject: ArtifactRef(format!("myelin://t1/ci/run/{id}")),
            aggregate: AggregateKey(format!("ci:{id}")),
            causation_id: None,
            correlation_id: CorrelationId("root".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp(recorded_at.into()),
            recorded_at: Timestamp(recorded_at.into()),
            payload: serde_json::json!({}),
        }
    }

    fn see_all(_m: &RelMembership) -> bool {
        false
    }

    fn failed_rule() -> SignalRule {
        define_signal_rule(
            RuleId("ci_run_failed".into()),
            type_matcher("ci.run.failed"),
            Severity::Error,
            DedupKeyTpl("ci.run.failed:{event.subject}".into()),
            DedupWindow { seconds: 0 },
            Some(type_matcher("ci.run.passed")),
        )
    }

    #[test]
    fn dedup_window_collapses_ten_failures_to_one_signal_count_ten() {
        let mut engine = SignalEngine::new();
        engine.add_rule(failed_rule());

        let mut opened = 0u32;
        let mut collapsed = 0u32;
        for i in 0..10 {
            let env = envelope_at(
                "ci.run.failed",
                "42",
                &format!("2026-06-20T00:00:{:02}Z", i),
            );
            let drafts = engine.ingest(&env, &SetExpr::All, &see_all);
            assert_eq!(drafts.len(), 1, "one rule → one draft per matching event");
            match drafts[0].kind {
                PublishKind::Opened => opened += 1,
                PublishKind::Collapsed => collapsed += 1,
                PublishKind::Resolved => panic!("a failure must not resolve"),
            }
        }
        assert_eq!(opened, 1, "exactly one Signal opened");
        assert_eq!(collapsed, 9, "the other nine collapsed into it");

        let key = DedupKey("ci.run.failed:myelin://t1/ci/run/42".into());
        let sig = engine.signal(&TenantId("t1".into()), &key).unwrap();
        assert_eq!(
            sig.count, 10,
            "N=10 identical failures → one Signal count=10"
        );
        assert_eq!(sig.state, SignalState::Open);
        assert_eq!(
            publish_subject(&sig.tenant, sig.severity, &sig.rule_id),
            "sig.t1.error.ci_run_failed"
        );
    }

    #[test]
    fn distinct_subjects_open_distinct_signals() {
        let mut engine = SignalEngine::new();
        engine.add_rule(failed_rule());
        let a = engine.ingest(
            &envelope_at("ci.run.failed", "1", "2026-06-20T00:00:00Z"),
            &SetExpr::All,
            &see_all,
        );
        let b = engine.ingest(
            &envelope_at("ci.run.failed", "2", "2026-06-20T00:00:00Z"),
            &SetExpr::All,
            &see_all,
        );
        assert_eq!(a[0].kind, PublishKind::Opened);
        assert_eq!(
            b[0].kind,
            PublishKind::Opened,
            "a different run is a new incident"
        );
    }

    #[test]
    fn severity_ranking_is_ordered() {
        let ranked = [
            Severity::Info,
            Severity::Notice,
            Severity::Warning,
            Severity::Error,
            Severity::Critical,
        ];
        for win in ranked.windows(2) {
            assert!(win[0] < win[1], "{:?} must rank below {:?}", win[0], win[1]);
        }
        assert_eq!(ranked.iter().copied().max(), Some(Severity::Critical));
        assert_eq!(ranked.iter().copied().min(), Some(Severity::Info));
        assert_eq!(Severity::Critical.token(), "critical");
        assert_eq!(Severity::Info.token(), "info");
    }

    #[test]
    fn auto_resolve_passed_resolves_matching_failed() {
        let mut engine = SignalEngine::new();
        engine.add_rule(failed_rule());

        let failed = envelope_at("ci.run.failed", "42", "2026-06-20T00:00:00Z");
        let opened = engine.ingest(&failed, &SetExpr::All, &see_all);
        assert_eq!(opened[0].kind, PublishKind::Opened);

        let key = DedupKey("ci.run.failed:myelin://t1/ci/run/42".into());
        assert_eq!(
            engine.signal(&TenantId("t1".into()), &key).unwrap().state,
            SignalState::Open
        );

        let passed = envelope_at("ci.run.passed", "42", "2026-06-20T00:05:00Z");
        let resolved = engine.ingest(&passed, &SetExpr::All, &see_all);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].kind, PublishKind::Resolved);
        assert_eq!(
            engine.signal(&TenantId("t1".into()), &key).unwrap().state,
            SignalState::Resolved
        );
        let other_pass = envelope_at("ci.run.passed", "99", "2026-06-20T00:06:00Z");
        let none = engine.ingest(&other_pass, &SetExpr::All, &see_all);
        assert!(none.is_empty(), "a different run's pass resolves nothing");
    }

    #[test]
    fn failure_after_resolve_opens_fresh_signal() {
        let mut engine = SignalEngine::new();
        engine.add_rule(failed_rule());
        engine.ingest(
            &envelope_at("ci.run.failed", "42", "2026-06-20T00:00:00Z"),
            &SetExpr::All,
            &see_all,
        );
        engine.ingest(
            &envelope_at("ci.run.passed", "42", "2026-06-20T00:01:00Z"),
            &SetExpr::All,
            &see_all,
        );
        let again = engine.ingest(
            &envelope_at("ci.run.failed", "42", "2026-06-20T00:02:00Z"),
            &SetExpr::All,
            &see_all,
        );
        assert_eq!(
            again[0].kind,
            PublishKind::Opened,
            "a failure after resolve is a new incident"
        );
        let key = DedupKey("ci.run.failed:myelin://t1/ci/run/42".into());
        let sig = engine.signal(&TenantId("t1".into()), &key).unwrap();
        assert_eq!(sig.count, 1, "the fresh Signal restarts at count=1");
        assert_eq!(sig.state, SignalState::Open);
    }

    #[test]
    fn repeat_past_window_opens_fresh_signal() {
        let mut engine = SignalEngine::new();
        engine.add_rule(define_signal_rule(
            RuleId("ci_run_failed".into()),
            type_matcher("ci.run.failed"),
            Severity::Error,
            DedupKeyTpl("ci.run.failed:{event.subject}".into()),
            DedupWindow { seconds: 60 },
            None,
        ));
        let first = engine.ingest(
            &envelope_at("ci.run.failed", "42", "2026-06-20T00:00:00Z"),
            &SetExpr::All,
            &see_all,
        );
        assert_eq!(first[0].kind, PublishKind::Opened);
        let inside = engine.ingest(
            &envelope_at("ci.run.failed", "42", "2026-06-20T00:00:30Z"),
            &SetExpr::All,
            &see_all,
        );
        assert_eq!(inside[0].kind, PublishKind::Collapsed);
        let outside = engine.ingest(
            &envelope_at("ci.run.failed", "42", "2026-06-20T00:02:00Z"),
            &SetExpr::All,
            &see_all,
        );
        assert_eq!(
            outside[0].kind,
            PublishKind::Opened,
            "a repeat past the window is a fresh incident"
        );
    }

    #[test]
    fn unviewable_event_curates_no_signal() {
        let mut engine = SignalEngine::new();
        engine.add_rule(failed_rule());
        let env = envelope_at("ci.run.failed", "42", "2026-06-20T00:00:00Z");
        let drafts = engine.ingest(&env, &SetExpr::None, &see_all);
        assert!(
            drafts.is_empty(),
            "an unviewable subject curates no Signal (0-leak)"
        );
        let key = DedupKey("ci.run.failed:myelin://t1/ci/run/42".into());
        assert!(engine.signal(&TenantId("t1".into()), &key).is_none());
    }

    #[test]
    fn dedup_key_template_renders_projection() {
        let env = EventEnvelope {
            payload: serde_json::json!({ "context": "build" }),
            ..envelope_at("ci.run.failed", "42", "2026-06-20T00:00:00Z")
        };
        let tpl = DedupKeyTpl("{event.type}:{event.subject}:{payload.context}".into());
        assert_eq!(
            tpl.render(&env).0,
            "ci.run.failed:myelin://t1/ci/run/42:build"
        );
        let tpl2 = DedupKeyTpl("{{lit}}:{payload.absent}".into());
        assert_eq!(tpl2.render(&env).0, "{lit}:<missing>");
    }

    #[test]
    fn ingest_is_replay_deterministic() {
        let stream: Vec<EventEnvelope> = (0..5)
            .map(|i| {
                envelope_at(
                    "ci.run.failed",
                    "42",
                    &format!("2026-06-20T00:00:{:02}Z", i),
                )
            })
            .collect();
        let run = || {
            let mut e = SignalEngine::new();
            e.add_rule(failed_rule());
            let mut all = Vec::new();
            for env in &stream {
                all.extend(e.ingest(env, &SetExpr::All, &see_all));
            }
            all
        };
        assert_eq!(
            run(),
            run(),
            "the same stream → the same drafts (deterministic)"
        );
    }

    #[test]
    fn signal_rule_round_trips_stably() {
        let rule = failed_rule();
        let json = serde_json::to_string(&rule).unwrap();
        let back: SignalRule = serde_json::from_str(&json).unwrap();
        assert_eq!(rule, back);
    }
}
