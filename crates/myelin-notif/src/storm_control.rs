use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_events::EventEnvelope;

use crate::prefs::QuietHours;
use crate::router::RoutedInboxItem;
use crate::Class;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuppressReason {
    SelfAction,
    RateDamped,
    Muted,
    QuietHours,
}

impl SuppressReason {
    pub fn token(self) -> &'static str {
        match self {
            SuppressReason::SelfAction => "self_action",
            SuppressReason::RateDamped => "rate_damped",
            SuppressReason::Muted => "muted",
            SuppressReason::QuietHours => "quiet_hours",
        }
    }

    pub fn writes_row(self) -> bool {
        match self {
            SuppressReason::Muted | SuppressReason::QuietHours => true,
            SuppressReason::SelfAction | SuppressReason::RateDamped => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StormDecision {
    Deliver,
    Collapse,
    Coalesce,
    Suppress(SuppressReason),
}

impl StormDecision {
    pub fn touches_audit(self) -> bool {
        false
    }

    pub fn delivers(self) -> bool {
        matches!(self, StormDecision::Deliver)
    }

    pub fn writes_row(self) -> bool {
        match self {
            StormDecision::Deliver | StormDecision::Collapse | StormDecision::Coalesce => true,
            StormDecision::Suppress(reason) => reason.writes_row(),
        }
    }
}

pub fn is_self_notification(env: &EventEnvelope, recipient: &str) -> bool {
    env.actor.0.principal_id.0 == recipient
}

#[derive(Clone, Default)]
pub struct Coalescer {
    seen: Arc<Mutex<HashMap<(String, String), u32>>>,
}

impl Coalescer {
    pub fn new() -> Coalescer {
        Coalescer::default()
    }

    pub fn should_coalesce(&self, recipient: &str, subject_root: &str, class: Class) -> bool {
        if is_break_out_class(class) {
            return false;
        }
        let key = (recipient.to_string(), subject_root.to_string());
        let mut g = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let count = g.entry(key).or_insert(0);
        let already = *count > 0;
        *count += 1;
        already
    }
}

fn is_break_out_class(class: Class) -> bool {
    matches!(class, Class::Direct | Class::Critical)
}

#[derive(Clone, Default)]
pub struct TokenBucket {
    inner: Arc<Mutex<HashMap<(String, String), BucketState>>>,
}

#[derive(Clone, Copy, Debug)]
struct BucketState {
    tokens: f64,
    last_tick: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RateConfig {
    pub capacity: f64,
    pub refill_per_tick: f64,
}

impl Default for RateConfig {
    fn default() -> RateConfig {
        RateConfig {
            capacity: 5.0,
            refill_per_tick: 1.0,
        }
    }
}

impl TokenBucket {
    pub fn new() -> TokenBucket {
        TokenBucket::default()
    }

    pub fn try_take(
        &self,
        recipient: &str,
        subject_root: &str,
        tick: u64,
        cfg: RateConfig,
    ) -> bool {
        let key = (recipient.to_string(), subject_root.to_string());
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let state = g.entry(key).or_insert(BucketState {
            tokens: cfg.capacity,
            last_tick: tick,
        });
        let elapsed = tick.saturating_sub(state.last_tick);
        if elapsed > 0 {
            state.tokens = (state.tokens + elapsed as f64 * cfg.refill_per_tick).min(cfg.capacity);
            state.last_tick = tick;
        }
        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Default)]
pub struct StormPrefs {
    muted: Arc<Mutex<std::collections::HashSet<(String, String)>>>,
}

impl StormPrefs {
    pub fn new() -> StormPrefs {
        StormPrefs::default()
    }

    pub fn mute(&self, recipient: &str, subject_root: &str) {
        self.muted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((recipient.to_string(), subject_root.to_string()));
    }

    pub fn is_muted(&self, recipient: &str, subject_root: &str) -> bool {
        self.muted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(recipient.to_string(), subject_root.to_string()))
    }
}

#[derive(Clone)]
pub struct StormContext<'a> {
    pub tick: u64,
    pub utc_minute_of_day: i32,
    pub utc_weekday: u8,
    pub quiet: &'a QuietHours,
    pub rate: RateConfig,
}

#[derive(Clone, Default)]
pub struct StormControl {
    coalescer: Coalescer,
    buckets: TokenBucket,
    prefs: StormPrefs,
}

impl StormControl {
    pub fn new() -> StormControl {
        StormControl::default()
    }

    pub fn prefs(&self) -> &StormPrefs {
        &self.prefs
    }

    pub fn decide(
        &self,
        env: &EventEnvelope,
        item: &RoutedInboxItem,
        subject_root: &str,
        row_exists: bool,
        ctx: &StormContext<'_>,
    ) -> StormDecision {
        if is_self_notification(env, &item.recipient) {
            return StormDecision::Suppress(SuppressReason::SelfAction);
        }

        if row_exists {
            return StormDecision::Collapse;
        }

        let pierces = ctx.quiet.pierces(item.class);

        if self
            .coalescer
            .should_coalesce(&item.recipient, subject_root, item.class)
        {
            return StormDecision::Coalesce;
        }

        if !pierces
            && !self
                .buckets
                .try_take(&item.recipient, subject_root, ctx.tick, ctx.rate)
        {
            return StormDecision::Suppress(SuppressReason::RateDamped);
        }

        if self.prefs.is_muted(&item.recipient, subject_root) {
            return StormDecision::Suppress(SuppressReason::Muted);
        }
        if !pierces
            && ctx
                .quiet
                .is_quiet_at(ctx.utc_minute_of_day, ctx.utc_weekday)
        {
            return StormDecision::Suppress(SuppressReason::QuietHours);
        }

        StormDecision::Deliver
    }
}

pub fn subject_root_of(subject: &str) -> String {
    match subject.split_once('#') {
        Some((root, _frag)) => root.to_string(),
        None => subject.to_string(),
    }
}

pub fn dedup_collapse_ratio_bps(inbound: u64, collapsed: u64) -> i64 {
    if inbound == 0 {
        return 0;
    }
    let collapsed = collapsed.min(inbound);
    ((10_000 * collapsed) / inbound) as i64
}

#[cfg(test)]
mod tests;
