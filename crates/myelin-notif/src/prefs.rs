use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_identity::{Literal, Principal};
use myelin_query::{
    CmpOp, EvalContext, Expr, Predicate, PredicateError, QueryAst, MAX_PREDICATE_DEPTH,
    MAX_PREDICATE_NODES,
};
use serde::{Deserialize, Serialize};

use crate::list_inbox::Subsystem;
use crate::{Class, Reason};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    InApp,
    WebPush,
    MobilePush,
    Email,
    Desktop,
}

impl Channel {
    pub fn token(self) -> &'static str {
        match self {
            Channel::InApp => "in_app",
            Channel::WebPush => "web_push",
            Channel::MobilePush => "mobile_push",
            Channel::Email => "email",
            Channel::Desktop => "desktop",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigestConfig {
    pub cadence: String,
    pub at: Option<String>,
    pub classes: Vec<Class>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingRule {
    pub channel: Channel,
    pub matcher: QueryAst,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifPrefs {
    pub routing: Vec<RoutingRule>,
    pub digest: DigestConfig,
}

impl NotifPrefs {
    pub fn default_in_app() -> NotifPrefs {
        NotifPrefs {
            routing: vec![RoutingRule {
                channel: Channel::InApp,
                matcher: QueryAst::compiled(Predicate::True)
                    .expect("the always-match predicate is one node (within the static bound)"),
            }],
            digest: DigestConfig::default(),
        }
    }

    pub fn channels_for(&self, reason: Reason, class: Class, subsystem: Subsystem) -> Vec<Channel> {
        let ctx = route_context(reason, class, subsystem);
        self.routing
            .iter()
            .filter(|rule| rule.matcher.eval(&ctx).unwrap_or(false))
            .map(|rule| rule.channel)
            .collect()
    }
}

pub fn build_routing_matcher(
    classes: &[Class],
    reasons: &[Reason],
    subsystems: &[Subsystem],
) -> Result<QueryAst, PredicateError> {
    let mut conjuncts: Vec<Predicate> = Vec::new();
    if !classes.is_empty() {
        conjuncts.push(Predicate::Or(
            classes
                .iter()
                .map(|c| Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: Expr::Var("class".into()),
                    rhs: Expr::Lit(Literal::Str(class_token(*c).into())),
                })
                .collect(),
        ));
    }
    if !reasons.is_empty() {
        conjuncts.push(Predicate::Or(
            reasons
                .iter()
                .map(|r| Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: Expr::Var("reason".into()),
                    rhs: Expr::Lit(Literal::Str(reason_token(*r).into())),
                })
                .collect(),
        ));
    }
    if !subsystems.is_empty() {
        conjuncts.push(Predicate::Or(
            subsystems
                .iter()
                .map(|s| Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: Expr::Var("subsystem".into()),
                    rhs: Expr::Lit(Literal::Str(subsystem_token(*s).into())),
                })
                .collect(),
        ));
    }
    let predicate = if conjuncts.is_empty() {
        Predicate::True
    } else {
        Predicate::And(conjuncts)
    };
    QueryAst::compiled(predicate)
}

pub fn route_context(reason: Reason, class: Class, subsystem: Subsystem) -> EvalContext {
    EvalContext::new()
        .bind("reason", Literal::Str(reason_token(reason).into()))
        .bind("class", Literal::Str(class_token(class).into()))
        .bind("subsystem", Literal::Str(subsystem_token(subsystem).into()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tz {
    pub offset_minutes: i32,
}

impl Tz {
    pub const UTC: Tz = Tz { offset_minutes: 0 };

    pub fn from_offset_minutes(offset_minutes: i32) -> Tz {
        Tz { offset_minutes }
    }

    pub fn local_minute_of_day(self, utc_minute_of_day: i32) -> i32 {
        let local = utc_minute_of_day + self.offset_minutes;
        local.rem_euclid(1440)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietWindow {
    pub from: i32,
    pub to: i32,
    pub days: Vec<u8>,
}

impl QuietWindow {
    fn contains(&self, local_minute: i32, weekday: u8) -> bool {
        if !self.days.is_empty() && !self.days.contains(&weekday) {
            return false;
        }
        if self.from <= self.to {
            local_minute >= self.from && local_minute < self.to
        } else {
            local_minute >= self.from || local_minute < self.to
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietHours {
    pub tz: Tz,
    pub windows: Vec<QuietWindow>,
    pub pierce_classes: Vec<Class>,
}

impl Default for QuietHours {
    fn default() -> QuietHours {
        QuietHours {
            tz: Tz::UTC,
            windows: Vec::new(),
            pierce_classes: vec![Class::Critical],
        }
    }
}

impl QuietHours {
    pub fn is_quiet_at(&self, utc_minute_of_day: i32, utc_weekday: u8) -> bool {
        let local_minute = self.tz.local_minute_of_day(utc_minute_of_day);
        let total = utc_minute_of_day + self.tz.offset_minutes;
        let day_shift = total.div_euclid(1440);
        let local_weekday = ((utc_weekday as i32 + day_shift).rem_euclid(7)) as u8;
        self.windows
            .iter()
            .any(|w| w.contains(local_minute, local_weekday))
    }

    pub fn pierces(&self, class: Class) -> bool {
        self.pierce_classes.contains(&class)
    }
}

pub fn route(
    prefs: &NotifPrefs,
    quiet: &QuietHours,
    reason: Reason,
    class: Class,
    subsystem: Subsystem,
    utc_minute_of_day: i32,
    utc_weekday: u8,
) -> Vec<Channel> {
    let candidates = prefs.channels_for(reason, class, subsystem);
    let in_quiet = quiet.is_quiet_at(utc_minute_of_day, utc_weekday);
    if quiet.pierces(class) || !in_quiet {
        candidates
    } else {
        candidates
            .into_iter()
            .filter(|c| *c == Channel::InApp)
            .collect()
    }
}

#[derive(Clone, Default)]
pub struct PrefStore {
    inner: Arc<Mutex<PrefStoreInner>>,
}

#[derive(Default)]
struct PrefStoreInner {
    prefs: BTreeMap<String, NotifPrefs>,
    quiet: BTreeMap<String, QuietHours>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrefView {
    pub prefs: NotifPrefs,
    pub quiet: QuietHours,
}

impl PrefStore {
    pub fn new() -> PrefStore {
        PrefStore::default()
    }

    pub fn upsert(&self, principal: &str, prefs: NotifPrefs, quiet: QuietHours) {
        let mut g = self.inner.lock().expect("pref store mutex");
        g.prefs.insert(principal.to_string(), prefs);
        g.quiet.insert(principal.to_string(), quiet);
    }

    fn read(&self, principal: &str) -> PrefView {
        let g = self.inner.lock().expect("pref store mutex");
        PrefView {
            prefs: g
                .prefs
                .get(principal)
                .cloned()
                .unwrap_or_else(NotifPrefs::default_in_app),
            quiet: g.quiet.get(principal).cloned().unwrap_or_default(),
        }
    }
}

pub fn get_prefs(store: &PrefStore, principal: &Principal) -> PrefView {
    store.read(principal.principal_id.0.as_str())
}

pub fn set_prefs(
    store: &PrefStore,
    principal: &Principal,
    prefs: NotifPrefs,
    quiet: QuietHours,
) -> PrefView {
    let id = principal.principal_id.0.as_str();
    store.upsert(id, prefs.clone(), quiet.clone());
    PrefView { prefs, quiet }
}

pub fn reason_token(reason: Reason) -> &'static str {
    match reason {
        Reason::ApprovalRequested => "approval_requested",
        Reason::Escalated => "escalated",
        Reason::Sla => "sla",
        Reason::ReviewRequested => "review_requested",
        Reason::Assigned => "assigned",
        Reason::Mentioned => "mentioned",
        Reason::Replied => "replied",
        Reason::AgentProposal => "agent_proposal",
        Reason::Watched => "watched",
        Reason::StateChanged => "state_changed",
        Reason::Fyi => "fyi",
        Reason::Blocked => "blocked",
        Reason::Unblocked => "unblocked",
        Reason::ThreadWatched => "thread_watched",
        Reason::Shared => "shared",
        Reason::Comments => "comments",
    }
}

pub fn class_token(class: Class) -> &'static str {
    match class {
        Class::Critical => "critical",
        Class::Direct => "direct",
        Class::Participating => "participating",
        Class::Watching => "watching",
        Class::Fyi => "fyi",
    }
}

pub fn subsystem_token(subsystem: Subsystem) -> &'static str {
    match subsystem {
        Subsystem::Issue => "issue",
        Subsystem::Chat => "chat",
        Subsystem::Git => "git",
        Subsystem::Knowledge => "knowledge",
        Subsystem::Ci => "ci",
        Subsystem::Unknown => "unknown",
    }
}

pub const PREFS_MAX_PREDICATE_NODES: usize = MAX_PREDICATE_NODES;
pub const PREFS_MAX_PREDICATE_DEPTH: usize = MAX_PREDICATE_DEPTH;

#[cfg(test)]
mod tests;
