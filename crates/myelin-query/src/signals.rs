//! # `signals` — Signal curation: match / severity-rank / dedup-window / auto-resolve /
//! publish (contract 3.1; Bus §4.4; P-138 / EB-18)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §4.4 (the Signal engine —
//! an infra consumer on the raw `evt.*` firehose; **match** the rule's [`EventMatcher`]
//! (§4.5), **severity-rank** (`info<notice<warning<error<critical`), **dedup within window**
//! (`dedup_key = render(tpl, envelope)`; `ON CONFLICT … count = count+1` — N identical
//! failures collapse to one Signal with `count=N`, the storm-control primitive Notif relies
//! on), **auto-resolve** (a resolving matcher resolves the matching Signal), **publish** to
//! `sig.<tenant>.<severity>.<rule>`). Contract-index row **3.1**
//! (`define_signal_rule(SignalRule{matcher, severity, dedup_key_tpl, dedup_window})` — the
//! curated/deduped/severity-ranked subset; consumers subscribe to Signals, never `evt.*`).
//!
//! ## Why the Signal engine lives in `myelin-query`, not `myelin-events` (DOCUMENTED DEVIATION)
//! The EB-18 prompt's DELIVERABLE field says "In `myelin-events`: `signals.rs`". That is
//! **genuinely unworkable against the frozen crate DAG** for the same reason the
//! [`EventMatcher`](crate::EventMatcher) (P-137 / EB-17) had to be built here and not in
//! `myelin-events` (see [`crate::matcher`] §"Why the matcher lives in `myelin-query`"): the
//! Signal engine's `matcher` column **IS** an [`EventMatcher`], whose predicate ENGINE was
//! promoted into `myelin-query` by P-133, and `myelin-query` **depends on `myelin-events`**
//! (architecture §2.9). Putting the Signal engine in `myelin-events` would require
//! `…-events → …-query` for the matcher type — the cycle the `no-cross-sync-cycle` lint (E-5)
//! and the events `Cargo.toml` forbid. So the Signal engine is built HERE, ON TOP of the one
//! [`EventMatcher`], over the upstream [`EventEnvelope`]. The Bus dispatch tier (EB-23) and
//! Notif (the storm-control consumer of curated Signals) reference `myelin_query::SignalEngine`.
//! This deviation is recorded here and in the P-138 report, per external-insights/01 §1.
//!
//! ## What this module adds (it does NOT re-define the matcher or the predicate engine)
//! - [`Severity`] — the frozen severity rank `info < notice < warning < error < critical`
//!   (its [`Ord`] IS the severity-ranking, §4.4).
//! - [`SignalRule`] + [`define_signal_rule`] — a curation rule: the [`EventMatcher`] that
//!   selects, the [`Severity`] it ranks at, the dedup-key template, the dedup window, and an
//!   optional resolving [`EventMatcher`] (auto-resolve).
//! - [`SignalEngine`] — the infra consumer state: the enabled rules + the per-`(tenant,
//!   dedup_key)` open-Signal store with the window-collapse counter. [`SignalEngine::ingest`]
//!   is the per-event reflex: match → severity-rank → dedup-window collapse → auto-resolve →
//!   the publish drafts (`sig.<tenant>.<severity>.<rule>`).
//! - [`Signal`] / [`SignalState`] — the curated Signal: subject, severity, the collapse
//!   `count`, the open/resolved state, first/last-seen.
//!
//! **Publish is outbox-only, by construction.** [`SignalEngine::ingest`] returns
//! [`PublishDraft`]s — it NEVER publishes itself. The Bus dispatch tier (EB-23) turns each
//! draft into an `OutboxTx::emit` in the SAME transaction that records the Signal-store
//! mutation (the no-raw-publish discipline, P-S10). This crate is DB-free; the durable
//! `signal` / `signal_rule` tables (architecture §3.3) are the dispatch tier's concern.

use crate::EventMatcher;
use myelin_events::{ArtifactRef, EventEnvelope, Visibility};
use myelin_identity::SetExpr;
use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::matcher::RelMembership;

/// The frozen severity rank (Bus §4.4): **`info < notice < warning < error < critical`**. The
/// derived [`Ord`] IS the severity-ranking — the variants are declared low-to-high so a
/// `>=`/`max` comparison ranks correctly, and the publish subject token is [`Severity::token`].
///
/// This is a closed set: the Signal engine never invents a severity, and a consumer
/// subscribing to `sig.<tenant>.error.>` gets `error` and (by a `>=` filter at the dispatch
/// tier) `critical`, never `info`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Severity {
    /// Lowest rank — purely informational.
    Info,
    /// Above `info` — a noteworthy but non-actionable event.
    Notice,
    /// A warning — actionable-but-not-urgent.
    Warning,
    /// An error — actionable.
    Error,
    /// Highest rank — critical / page-worthy.
    Critical,
}

impl Severity {
    /// The lowercase subject token used in `sig.<tenant>.<severity>.<rule>` (Bus §4.4).
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

/// A stable rule identifier (the `<rule>` token in `sig.<tenant>.<severity>.<rule>`). Frozen
/// per tenant; the publish subject and the dedup-key namespace are scoped by it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RuleId(pub String);

/// The rendered dedup key for one Signal — `render(dedup_key_tpl, envelope)` (§4.4). N events
/// rendering to the SAME key (within the window) collapse to one Signal with `count=N`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DedupKey(pub String);

/// A **dedup-key template** (§4.4): a string with `{<field>}` placeholders rendered against
/// the envelope projection (the same `event.*` / `payload.*` namespace the matcher reads, see
/// [`crate::matcher::project_envelope`]). E.g. `"ci.run.failed:{event.subject}"` renders to
/// `"ci.run.failed:myelin://t1/ci/run/42"`. A `{field}` the envelope does not project renders
/// the literal token `<missing>` (deterministic — never a panic, never a silent collapse of
/// unrelated events).
///
/// This is a **literal substitution**, not a second predicate/expression language: there are
/// no functions, no conditionals — exactly the placeholders, exactly the projected fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DedupKeyTpl(pub String);

impl DedupKeyTpl {
    /// Render the template against the envelope projection. `{<field>}` is replaced by the
    /// projected value; an unprojected field renders `<missing>`; a literal `{`/`}` is
    /// produced by `{{`/`}}`. Total and side-effect-free.
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

/// Project one `{<field>}` placeholder to its string value from the envelope. The namespace
/// mirrors [`crate::matcher::project_envelope`] (the matcher and the dedup key read the SAME
/// envelope fields — no drift): the dotted `event.*` identifiers and the flat scalar
/// `payload.*` fields. An unknown / unprojected field is `"<missing>"` (deterministic).
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
                            // A non-scalar payload leaf is not a stable dedup token.
                            _ => "<missing>".to_string(),
                        };
                    }
                }
            }
            "<missing>".to_string()
        }
    }
}

/// A **dedup window**, in seconds. Two events rendering to the same [`DedupKey`] within this
/// many seconds of the open Signal's first-seen collapse into it (`count += 1`); a later
/// event past the window opens a NEW Signal (a fresh incident). `window.seconds == 0` means
/// "collapse for as long as the Signal stays open" (no time bound — pure key collapse).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DedupWindow {
    pub seconds: u64,
}

/// A Signal-curation rule (contract 3.1, frozen Bus §4.4): the selector, its severity rank,
/// the dedup-key template + window, and an optional **resolving** matcher (auto-resolve).
///
/// Built via [`define_signal_rule`]. The `matcher` and `resolves` are [`EventMatcher`]s — the
/// one bounded, permission-aware predicate surface — so a Signal rule can never select (or
/// resolve against) an artifact the rule's principal can't see (§4.5; the 0-leak property
/// rides through [`SignalEngine::ingest`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalRule {
    /// The stable rule id (the `<rule>` publish-subject token + the dedup-key namespace).
    pub rule_id: RuleId,
    /// The selector — the [`EventMatcher`] (= the frozen `QueryAst`, §4.5). An event matches
    /// the rule iff it matches this matcher (after the permission compose).
    pub matcher: EventMatcher,
    /// The severity this rule ranks a matching event at (§4.4).
    pub severity: Severity,
    /// The dedup-key template (`render(tpl, envelope)` → the collapse key).
    pub dedup_key_tpl: DedupKeyTpl,
    /// The dedup window (the collapse horizon).
    pub dedup_window: DedupWindow,
    /// The **auto-resolve** matcher (§4.4): a matching resolving event (e.g. `ci.run.passed`)
    /// resolves the OPEN Signal that shares its rendered dedup key. `None` ⇒ this rule's
    /// Signals never auto-resolve (they are resolved by retention / a human ack elsewhere).
    pub resolves: Option<EventMatcher>,
}

/// **`define_signal_rule(SignalRule{matcher, severity, dedup_key_tpl, dedup_window})`**
/// (contract 3.1) — the registration verb. Constructs a [`SignalRule`]. The `matcher` /
/// `resolves` [`EventMatcher`]s were already cost-validated at their own `compile` (the
/// over-budget AST was rejected at construction, §4.5), so this verb is total.
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

/// The open/resolved state of a curated Signal (§4.4). A Signal opens on the first matching
/// event and stays `Open` (collapsing repeats) until a resolving event auto-resolves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalState {
    /// Open — actionable; further matching events within the window collapse into it.
    Open,
    /// Resolved — an auto-resolve matcher matched; no further collapse occurs.
    Resolved,
}

/// A **curated Signal** (§4.4) — the deduped, severity-ranked unit consumers subscribe to.
/// `count` is the window-collapse counter (N identical events → one Signal `count=N`, the
/// storm-control primitive). References-not-payloads: it carries the `subject` [`ArtifactRef`]
/// and the rule/severity, never a PII body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signal {
    /// The rule that curated this Signal.
    pub rule_id: RuleId,
    /// The tenant partition (the `<tenant>` publish-subject token).
    pub tenant: TenantId,
    /// The severity rank.
    pub severity: Severity,
    /// The rendered dedup key (the collapse identity, `(tenant, dedup_key)` is the store PK).
    pub dedup_key: DedupKey,
    /// The subject of the FIRST event that opened this Signal (the representative subject).
    pub subject: ArtifactRef,
    /// The window-collapse counter: how many matching events have collapsed into this Signal.
    pub count: u64,
    /// Open / resolved.
    pub state: SignalState,
    /// The `recorded_at` of the first event (the window anchor; RFC-3339 UTC, §2.10).
    pub first_seen: String,
    /// The `recorded_at` of the most-recent collapsed event.
    pub last_seen: String,
}

/// A **publish draft** the engine yields — the dispatch tier (EB-23) turns it into an
/// `OutboxTx::emit` in the SAME transaction as the Signal-store mutation (publish is
/// outbox-only by construction; this crate never publishes). The `subject` is the frozen
/// `sig.<tenant>.<severity>.<rule>` (§4.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishDraft {
    /// `sig.<tenant>.<severity>.<rule>` — the curated-Signal subject.
    pub subject: String,
    /// The Signal as it stands after this event (opened / collapsed / resolved).
    pub signal: Signal,
    /// What this event did to the Signal (the dispatch tier maps it to the `signal.opened` /
    /// `signal.collapsed` / `signal.resolved` outbox event).
    pub kind: PublishKind,
}

/// Why a [`PublishDraft`] was emitted (the lifecycle transition this event caused).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublishKind {
    /// The first matching event opened a new Signal (`count=1`).
    Opened,
    /// A repeat collapsed into an open Signal within the window (`count += 1`).
    Collapsed,
    /// A resolving event auto-resolved an open Signal.
    Resolved,
}

/// Compute the `sig.<tenant>.<severity>.<rule>` publish subject (§4.4).
fn publish_subject(tenant: &TenantId, severity: Severity, rule: &RuleId) -> String {
    format!("sig.{}.{}.{}", tenant.0, severity.token(), rule.0)
}

/// The **Signal engine** (Bus §4.4) — the infra consumer on the raw `evt.*` firehose (one of
/// the handful of excepted full-firehose consumers; the rest of the platform subscribes to
/// curated Signals, the upstream defence BUS-4). It is built ON the [`EventMatcher`]: each
/// rule's selector / resolver is a matcher, so the permission compose (the 0-leak property,
/// §4.5) rides through every match.
///
/// The store here is **in-memory and deterministic** (a `BTreeMap` keyed `(tenant,
/// dedup_key)`), which is exactly what the BUS-D3 replay-determinism drill (EB-23) needs: the
/// same event sequence yields the same Signals + counts. The durable `signal` table
/// (architecture §3.3) is the dispatch tier's persistence concern; the curation ALGORITHM —
/// match / rank / collapse / resolve — is this pure, replayable engine.
#[derive(Debug, Default)]
pub struct SignalEngine {
    rules: Vec<SignalRule>,
    /// Open + resolved Signals, keyed `(tenant, dedup_key)` (the §3.3 store PK). A resolved
    /// Signal is retained so a stale repeat does not silently re-open a resolved incident
    /// inside the same window.
    store: BTreeMap<(TenantId, DedupKey), Signal>,
}

impl SignalEngine {
    /// A fresh engine with no rules and an empty store.
    pub fn new() -> SignalEngine {
        SignalEngine::default()
    }

    /// Register a rule (contract 3.1 — `define_signal_rule`). Returns `&mut self` for builder
    /// chaining.
    pub fn add_rule(&mut self, rule: SignalRule) -> &mut SignalEngine {
        self.rules.push(rule);
        self
    }

    /// The current curated Signal for `(tenant, dedup_key)`, if any (read-only inspection;
    /// the dispatch tier reads the durable table, this is the in-engine view).
    pub fn signal(&self, tenant: &TenantId, dedup_key: &DedupKey) -> Option<&Signal> {
        self.store.get(&(tenant.clone(), dedup_key.clone()))
    }

    /// **The per-event curation reflex** (§4.4): match → severity-rank → dedup-window collapse
    /// → auto-resolve → publish drafts. Permission-aware BY CONSTRUCTION: `visible` is the
    /// `list_objects(viewer, read, type)` [`SetExpr`] result (4.3) the matcher composes with;
    /// an event for an artifact the rule's principal can't see NEVER opens or collapses a
    /// Signal (the 0-leak property rides through [`EventMatcher::matches`]).
    ///
    /// Returns the [`PublishDraft`]s this event produced (often 0 or 1; a single event can
    /// both collapse into one rule's Signal and resolve another's, so the result is a `Vec`).
    /// A matcher that errors over this envelope (a mis-authored predicate) is treated as "no
    /// match" for THAT rule (fail-closed) — never a silent match, never a panic.
    ///
    /// `member_oracle` answers the relational `SetExpr` arms (the consumer's authz
    /// reverse-index lookup; the in-process path supplies a closure). It is shared across the
    /// rules evaluated for this one event.
    pub fn ingest(
        &mut self,
        envelope: &EventEnvelope,
        visible: &SetExpr,
        member_oracle: &dyn Fn(&RelMembership) -> bool,
    ) -> Vec<PublishDraft> {
        let mut drafts = Vec::new();
        // Snapshot the rule list so the borrow of `self.rules` does not conflict with the
        // mutation of `self.store` below. Rules are small + registered once; this clone is the
        // per-event cost and keeps the engine a clean value.
        let rules = self.rules.clone();
        for rule in &rules {
            // (1) AUTO-RESOLVE first: a resolving event resolves the matching OPEN Signal.
            // We check resolution before opening so a single event that is BOTH a resolver of
            // rule A and a match of rule B does the right thing per rule.
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
                    // A resolving event is not ALSO a fresh match of the same rule's selector
                    // (resolver and selector are disjoint by construction, e.g. passed vs
                    // failed); skip the open/collapse path for this rule.
                    continue;
                }
            }

            // (2) MATCH the selector (after the permission compose).
            if !matches_fail_closed(&rule.matcher, envelope, visible, member_oracle) {
                continue;
            }

            // (3) DEDUP-WINDOW COLLAPSE: render the key; collapse into the open Signal within
            // the window, else open a fresh one.
            let key = rule.dedup_key_tpl.render(envelope);
            let store_key = (envelope.tenant.clone(), key.clone());
            let now = envelope.recorded_at.0.clone();

            let within_window = |first_seen: &str| -> bool {
                if rule.dedup_window.seconds == 0 {
                    return true; // 0 ⇒ collapse for as long as the Signal stays open.
                }
                match (parse_rfc3339_secs(first_seen), parse_rfc3339_secs(&now)) {
                    (Some(f), Some(n)) => n.saturating_sub(f) <= rule.dedup_window.seconds as i64,
                    // Unparseable timestamp ⇒ fall back to pure key collapse (never split an
                    // incident on a clock-format quirk).
                    _ => true,
                }
            };

            match self.store.get_mut(&store_key) {
                Some(sig) if sig.state == SignalState::Open && within_window(&sig.first_seen) => {
                    // Collapse: N identical events → one Signal, count += 1.
                    sig.count += 1;
                    sig.last_seen = now.clone();
                    drafts.push(PublishDraft {
                        subject: publish_subject(&sig.tenant, sig.severity, &sig.rule_id),
                        signal: sig.clone(),
                        kind: PublishKind::Collapsed,
                    });
                }
                _ => {
                    // Open a fresh Signal (no open Signal, or the prior one is resolved /
                    // outside the window → a new incident).
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

/// Run an [`EventMatcher`] over the envelope, treating an evaluation error (a mis-authored
/// predicate, a missing projected field) as **no match** (fail-closed) — never a panic, never
/// a silent match. This is the Signal engine's poison-tolerance: one bad rule can't crash the
/// firehose consumer or wrongly fire.
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

/// Parse an RFC-3339 UTC timestamp to whole seconds since the epoch (a tiny, dependency-free
/// parser sufficient for the dedup-window arithmetic — the envelope's `recorded_at` is always
/// `YYYY-MM-DDTHH:MM:SS[.fff]Z`, §2.10). Returns `None` on an unrecognised shape (the caller
/// falls back to pure key collapse). Not a general date library — exactly the envelope shape.
fn parse_rfc3339_secs(ts: &str) -> Option<i64> {
    // YYYY-MM-DDTHH:MM:SS , ignoring any fractional seconds and the trailing Z.
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
    // Days from civil (Howard Hinnant's algorithm) → days since 1970-01-01.
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

    /// `event.type == <type>` matcher over the `run` object type.
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
            // Collapse all failures of the SAME run into one Signal.
            DedupKeyTpl("ci.run.failed:{event.subject}".into()),
            DedupWindow { seconds: 0 },
            Some(type_matcher("ci.run.passed")),
        )
    }

    /// **Dedup-window collapse: 10 identical failures → ONE Signal with count=10** (the
    /// storm-control primitive Notif relies on, §4.4). This is the EB-18 mandatory-core
    /// drill.
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
        // The publish subject is the frozen sig.<tenant>.<severity>.<rule>.
        assert_eq!(
            publish_subject(&sig.tenant, sig.severity, &sig.rule_id),
            "sig.t1.error.ci_run_failed"
        );
    }

    /// **Distinct subjects do NOT collapse** — the dedup key is per-run, so two different runs
    /// failing open two distinct Signals (the collapse is by rendered key, not by type).
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

    /// **Severity-ranking ordering is correct: `info < notice < warning < error < critical`.**
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
        // The subject tokens are the frozen lowercase names.
        assert_eq!(Severity::Critical.token(), "critical");
        assert_eq!(Severity::Info.token(), "info");
    }

    /// **Auto-resolve: a `ci.run.passed` resolves the matching `ci.run.failed` Signal**
    /// (§4.4). The resolving event shares the dedup key (same run) and flips the open Signal
    /// to `Resolved`, emitting a `Resolved` publish draft.
    #[test]
    fn auto_resolve_passed_resolves_matching_failed() {
        let mut engine = SignalEngine::new();
        engine.add_rule(failed_rule());

        // Open the failure Signal.
        let failed = envelope_at("ci.run.failed", "42", "2026-06-20T00:00:00Z");
        let opened = engine.ingest(&failed, &SetExpr::All, &see_all);
        assert_eq!(opened[0].kind, PublishKind::Opened);

        let key = DedupKey("ci.run.failed:myelin://t1/ci/run/42".into());
        assert_eq!(
            engine.signal(&TenantId("t1".into()), &key).unwrap().state,
            SignalState::Open
        );

        // The passing run of the SAME subject auto-resolves it.
        let passed = envelope_at("ci.run.passed", "42", "2026-06-20T00:05:00Z");
        let resolved = engine.ingest(&passed, &SetExpr::All, &see_all);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].kind, PublishKind::Resolved);
        assert_eq!(
            engine.signal(&TenantId("t1".into()), &key).unwrap().state,
            SignalState::Resolved
        );
        // A passing run of a DIFFERENT subject does not resolve this one.
        let other_pass = envelope_at("ci.run.passed", "99", "2026-06-20T00:06:00Z");
        let none = engine.ingest(&other_pass, &SetExpr::All, &see_all);
        assert!(none.is_empty(), "a different run's pass resolves nothing");
    }

    /// **A failure AFTER a resolve opens a FRESH Signal** (a resolved incident does not
    /// silently re-collapse — the next failure is a new incident).
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

    /// **The dedup window bounds the collapse: a repeat PAST the window opens a fresh Signal.**
    #[test]
    fn repeat_past_window_opens_fresh_signal() {
        let mut engine = SignalEngine::new();
        engine.add_rule(define_signal_rule(
            RuleId("ci_run_failed".into()),
            type_matcher("ci.run.failed"),
            Severity::Error,
            DedupKeyTpl("ci.run.failed:{event.subject}".into()),
            DedupWindow { seconds: 60 }, // 60-second window.
            None,
        ));
        let first = engine.ingest(
            &envelope_at("ci.run.failed", "42", "2026-06-20T00:00:00Z"),
            &SetExpr::All,
            &see_all,
        );
        assert_eq!(first[0].kind, PublishKind::Opened);
        // 30s later — inside the window → collapse.
        let inside = engine.ingest(
            &envelope_at("ci.run.failed", "42", "2026-06-20T00:00:30Z"),
            &SetExpr::All,
            &see_all,
        );
        assert_eq!(inside[0].kind, PublishKind::Collapsed);
        // 120s after first — past the 60s window → a fresh Signal.
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

    /// **Permission compose 0-leak rides through the engine**: an event for a run the rule's
    /// principal can't see (visible = `None`) opens NO Signal.
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

    /// **The dedup-key template renders the envelope projection** (the same `event.*` /
    /// `payload.*` namespace the matcher reads — no drift). `{{`/`}}` escape; an unprojected
    /// field renders `<missing>`.
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
        // Escapes + a missing field.
        let tpl2 = DedupKeyTpl("{{lit}}:{payload.absent}".into());
        assert_eq!(tpl2.render(&env).0, "{lit}:<missing>");
    }

    /// **The engine is replay-deterministic**: the same event sequence yields the same
    /// Signals + counts (what BUS-D3 in EB-23 relies on). Two independent engines fed the same
    /// stream agree.
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

    /// **`SignalRule` round-trips stably (the wire contract — the durable `signal_rule` row).**
    #[test]
    fn signal_rule_round_trips_stably() {
        let rule = failed_rule();
        let json = serde_json::to_string(&rule).unwrap();
        let back: SignalRule = serde_json::from_str(&json).unwrap();
        assert_eq!(rule, back);
    }
}
