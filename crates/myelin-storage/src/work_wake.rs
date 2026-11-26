//! Commit-time wake-ups for durable worker loops.
//!
//! Workers that claim durable work (workflow partitions, agent-trigger queues) used to re-scan
//! every queue on a short timer, so an idle cell paid for every tenant it hosts. Database
//! triggers now `pg_notify` on [`WORK_WAKE_CHANNEL`] whenever a write can make work claimable,
//! and [`WorkWakeHub`] routes each notification to the loop that owns that work.
//!
//! Wakes make new work prompt; they are not a complete record of when work became claimable.
//! A claim's `SKIP LOCKED` passes over a row locked by a transaction that then commits without
//! a notifying change, a peer can claim silently after this loop read its deadlines and then
//! die, and a half-open listener receives nothing. Every idle loop therefore also re-checks on
//! a short jittered safety interval ([`jittered`]), which bounds all of these. A wake is still
//! only a hint: the lease and claim queries decide who drives work, so a lost, duplicated or
//! spurious wake can delay work but never drive it twice. While the listener is disconnected
//! the hub wakes every loop on the old polling interval.

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;
use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use sqlx::PgPool;
use tokio::sync::{watch, Notify};

use crate::migration::{Migration, Migrations};

pub const WORK_WAKE_CHANNEL: &str = "myelin_work_wake";

/// How often the listener proves its connection is alive, and how long that may take.
const LIVENESS_INTERVAL: Duration = Duration::from_secs(10);
const LIVENESS_TIMEOUT: Duration = Duration::from_secs(5);
/// How long connecting and subscribing may take together.
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(10);
/// How often a loop reports a read that keeps failing.
const FAILURE_REPORT_INTERVAL: Duration = Duration::from_secs(60);

/// Wakes the agent-trigger loops of a tenant when a firing changes state, or when a binding or
/// principal changes status (a queued firing waits for its binding, owner and agent to be
/// active). Payload: `[schema, "triggers", region, tenant]`, or `[schema, "all"]` when the
/// identifiers are too long for a notification.
pub const AGENT_TRIGGER_WORK_WAKE_MIGRATION: &str = r#"
CREATE OR REPLACE FUNCTION myelin_agent_trigger_work_wake()
RETURNS trigger
LANGUAGE plpgsql
AS $myelin$
BEGIN
  IF TG_OP = 'UPDATE' THEN
    IF TG_TABLE_NAME = 'principal' THEN
      IF NEW.status IS NOT DISTINCT FROM OLD.status THEN
        RETURN NULL;
      END IF;
    ELSIF NEW.state IS NOT DISTINCT FROM OLD.state THEN
      RETURN NULL;
    END IF;
  END IF;
  IF octet_length(NEW.tenant_id) + octet_length(NEW.region) > 4000 THEN
    PERFORM pg_notify('myelin_work_wake', json_build_array(TG_TABLE_SCHEMA, 'all')::text);
  ELSE
    PERFORM pg_notify('myelin_work_wake',
      json_build_array(TG_TABLE_SCHEMA, 'triggers', NEW.region, NEW.tenant_id)::text);
  END IF;
  RETURN NULL;
END
$myelin$;
CREATE OR REPLACE TRIGGER agent_trigger_firing_work_wake
AFTER INSERT OR UPDATE OF state ON agent_trigger_firing
FOR EACH ROW EXECUTE FUNCTION myelin_agent_trigger_work_wake();
CREATE OR REPLACE TRIGGER agent_trigger_binding_work_wake
AFTER UPDATE OF state ON agent_trigger_binding
FOR EACH ROW EXECUTE FUNCTION myelin_agent_trigger_work_wake();
CREATE OR REPLACE TRIGGER principal_work_wake
AFTER UPDATE OF status ON principal
FOR EACH ROW EXECUTE FUNCTION myelin_agent_trigger_work_wake();
"#;

/// Builds the payload before notifying and falls back to `[schema, "all"]` whenever the JSON
/// (whose escaping can grow identifiers several-fold) would not fit in a notification, so the
/// trigger never fails the write it observes.
pub const AGENT_TRIGGER_WORK_WAKE_PAYLOAD_MIGRATION: &str = r#"
CREATE OR REPLACE FUNCTION myelin_agent_trigger_work_wake()
RETURNS trigger
LANGUAGE plpgsql
AS $myelin$
DECLARE
  payload text;
BEGIN
  IF TG_OP = 'UPDATE' THEN
    IF TG_TABLE_NAME = 'principal' THEN
      IF NEW.status IS NOT DISTINCT FROM OLD.status THEN
        RETURN NULL;
      END IF;
    ELSIF NEW.state IS NOT DISTINCT FROM OLD.state THEN
      RETURN NULL;
    END IF;
  END IF;
  payload := json_build_array(TG_TABLE_SCHEMA, 'triggers', NEW.region, NEW.tenant_id)::text;
  IF octet_length(payload) >= 7000 THEN
    payload := json_build_array(TG_TABLE_SCHEMA, 'all')::text;
  END IF;
  PERFORM pg_notify('myelin_work_wake', payload);
  RETURN NULL;
END
$myelin$;
"#;

pub fn agent_trigger_work_wake_migrations() -> Migrations {
    Migrations::of([
        Migration::plain(
            "0189_agent_trigger_work_wake",
            AGENT_TRIGGER_WORK_WAKE_MIGRATION,
        ),
        Migration::plain(
            "0190_agent_trigger_work_wake_payload",
            AGENT_TRIGGER_WORK_WAKE_PAYLOAD_MIGRATION,
        ),
    ])
}

/// `interval` spread by ±20%, so loops started together do not re-check together. Intervals
/// above a day are treated as a day.
pub fn jittered(interval: Duration) -> Duration {
    let interval = interval.min(Duration::from_secs(86_400));
    let random = RandomState::new().build_hasher().finish();
    let factor = 0.8 + 0.4 * (random as f64 / u64::MAX as f64);
    interval.mul_f64(factor)
}

/// Retry pacing and log throttling for an idle loop's deadline read that keeps failing: the
/// retry delay doubles from `first` up to `max`, and failures are reported at most once a
/// minute, with how many happened since the last report.
#[derive(Debug)]
pub struct FailingRead {
    first: Duration,
    max: Duration,
    next: Duration,
    unreported: u64,
    last_report: Option<tokio::time::Instant>,
}

/// What a loop should do after a failed read.
#[derive(Debug, PartialEq, Eq)]
pub struct ReadFailure {
    /// How long to wait before reading again.
    pub retry_after: Duration,
    /// `Some(failures)` when this failure should be logged, counting it and every failure
    /// since the last report.
    pub report: Option<u64>,
}

impl FailingRead {
    pub fn new(first: Duration, max: Duration) -> Self {
        Self {
            first,
            max: max.max(first),
            next: first,
            unreported: 0,
            last_report: None,
        }
    }

    pub fn failed(&mut self) -> ReadFailure {
        let retry_after = self.next;
        self.next = self.next.saturating_mul(2).min(self.max);
        self.unreported += 1;
        let now = tokio::time::Instant::now();
        let due = self
            .last_report
            .is_none_or(|last| now.duration_since(last) >= FAILURE_REPORT_INTERVAL);
        let report = due.then(|| {
            self.last_report = Some(now);
            std::mem::take(&mut self.unreported)
        });
        ReadFailure {
            retry_after,
            report,
        }
    }

    pub fn succeeded(&mut self) {
        self.next = self.first;
    }
}

/// One loop's wake-up. Wakes that arrive while the loop is busy are kept (at most one), so a
/// commit that lands between a loop's empty claim and its next wait is never missed.
pub struct WorkWake(Arc<Notify>);

impl WorkWake {
    /// A wake that only the owner triggers; for loops run without a hub.
    pub fn detached() -> Self {
        Self(Arc::new(Notify::new()))
    }

    pub fn wake(&self) {
        self.0.notify_one();
    }

    pub async fn woken(&self) {
        self.0.notified().await;
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Wake {
    All,
    Flow {
        tenant: String,
        partition: Option<i16>,
    },
    Triggers {
        tenant: String,
    },
}

type Loops = HashMap<String, Vec<Arc<Notify>>>;

/// Routes notifications for one region of one schema to the loops registered for them.
pub struct WorkWakeHub {
    region: String,
    flow: HashMap<String, Vec<(i16, Arc<Notify>)>>,
    trigger_claims: Loops,
    trigger_reconciliation: Loops,
}

impl WorkWakeHub {
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            region: region.into(),
            flow: HashMap::new(),
            trigger_claims: HashMap::new(),
            trigger_reconciliation: HashMap::new(),
        }
    }

    /// Woken when a workflow run, signal or timer in this exact partition changes.
    pub fn flow_partition(&mut self, tenant: &str, partition: i16) -> WorkWake {
        let notify = Arc::new(Notify::new());
        self.flow
            .entry(tenant.to_owned())
            .or_default()
            .push((partition, notify.clone()));
        WorkWake(notify)
    }

    /// Woken when the tenant's trigger queue, a binding or a principal changes.
    pub fn trigger_claims(&mut self, tenant: &str) -> WorkWake {
        register(&mut self.trigger_claims, tenant)
    }

    /// Woken by trigger-queue changes and by any workflow change in the tenant, since a
    /// terminal run is what reconciliation waits for.
    pub fn trigger_reconciliation(&mut self, tenant: &str) -> WorkWake {
        register(&mut self.trigger_reconciliation, tenant)
    }

    /// Listens on a dedicated connection made from `listen` until shutdown. Every
    /// (re)subscription wakes all loops once, covering commits made while no listener was
    /// subscribed; while disconnected, all loops are woken every `degraded_poll`. The
    /// connection is pinged regularly so a half-open one is replaced.
    pub async fn run(
        self,
        pool: PgPool,
        listen: PgConnectOptions,
        mut shutdown: watch::Receiver<bool>,
        degraded_poll: Duration,
    ) -> Result<(), String> {
        // @tenant-cross-scope: reads the session schema so wakes from other schemas are ignored.
        let schema: String = sqlx::query_scalar("SELECT current_schema()::text")
            .fetch_one(&pool)
            .await
            .map_err(|error| format!("work wake schema: {error}"))?;
        let mut reported_outage = false;
        loop {
            if *shutdown.borrow() {
                return Ok(());
            }
            // A subscribe attempt can block for the whole connect timeout; loops keep being
            // woken on the degraded interval meanwhile.
            let subscribed = {
                let subscribing = subscribe(&listen);
                tokio::pin!(subscribing);
                let mut degraded = tokio::time::interval_at(
                    tokio::time::Instant::now() + degraded_poll,
                    degraded_poll,
                );
                loop {
                    tokio::select! {
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() { return Ok(()); }
                        }
                        _ = degraded.tick() => self.wake_all(),
                        result = &mut subscribing => break result,
                    }
                }
            };
            let mut listener = match subscribed {
                Ok(listener) => {
                    if reported_outage {
                        eprintln!("work wake listener reconnected");
                        reported_outage = false;
                    }
                    listener
                }
                Err(error) => {
                    if !reported_outage {
                        eprintln!(
                            "work wake listener unavailable; polling every {degraded_poll:?}: {error}"
                        );
                        reported_outage = true;
                    }
                    self.wake_all();
                    tokio::select! {
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() { return Ok(()); }
                        }
                        () = tokio::time::sleep(degraded_poll) => {}
                    }
                    continue;
                }
            };
            self.wake_all();
            let mut liveness = tokio::time::interval_at(
                tokio::time::Instant::now() + LIVENESS_INTERVAL,
                LIVENESS_INTERVAL,
            );
            loop {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { return Ok(()); }
                    }
                    _ = liveness.tick() => {
                        let ping = sqlx::query("SELECT 1").execute(&mut listener);
                        match tokio::time::timeout(LIVENESS_TIMEOUT, ping).await {
                            Ok(Ok(_)) => {}
                            Ok(Err(error)) => {
                                eprintln!("work wake listener failed its liveness check: {error}");
                                break;
                            }
                            Err(_) => {
                                eprintln!("work wake listener did not answer its liveness check");
                                break;
                            }
                        }
                    }
                    received = listener.try_recv() => match received {
                        Ok(Some(notification)) => self.route(&schema, notification.payload()),
                        Ok(None) => {
                            eprintln!("work wake listener lost its connection; resubscribing");
                            break;
                        }
                        Err(error) => {
                            eprintln!("work wake listener failed: {error}");
                            break;
                        }
                    },
                }
            }
            self.wake_all();
        }
    }

    fn route(&self, schema: &str, payload: &str) {
        match decode(schema, &self.region, payload) {
            Some(Wake::All) => self.wake_all(),
            Some(Wake::Flow { tenant, partition }) => {
                for (owned, notify) in self.flow.get(&tenant).into_iter().flatten() {
                    if partition.is_none_or(|partition| partition == *owned) {
                        notify.notify_one();
                    }
                }
                wake(&self.trigger_reconciliation, &tenant);
            }
            Some(Wake::Triggers { tenant }) => {
                wake(&self.trigger_claims, &tenant);
                wake(&self.trigger_reconciliation, &tenant);
            }
            None => {}
        }
    }

    fn wake_all(&self) {
        let flow = self.flow.values().flatten().map(|(_, notify)| notify);
        let claims = self.trigger_claims.values().flatten();
        let reconciliation = self.trigger_reconciliation.values().flatten();
        for notify in flow.chain(claims).chain(reconciliation) {
            notify.notify_one();
        }
    }
}

fn register(loops: &mut Loops, tenant: &str) -> WorkWake {
    let notify = Arc::new(Notify::new());
    loops
        .entry(tenant.to_owned())
        .or_default()
        .push(notify.clone());
    WorkWake(notify)
}

fn wake(loops: &Loops, tenant: &str) {
    for notify in loops.get(tenant).into_iter().flatten() {
        notify.notify_one();
    }
}

/// Subscribes on its own one-connection pool, so listening never holds a slot of the pool
/// the workers claim through.
async fn subscribe(listen: &PgConnectOptions) -> Result<PgListener, sqlx::Error> {
    let subscribing = async {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .acquire_timeout(LIVENESS_TIMEOUT)
            .connect_with(listen.clone())
            .await?;
        let mut listener = PgListener::connect_with(&pool).await?;
        listener.ignore_pool_close_event(true);
        // A lost connection ends this listener; `run` subscribes afresh and wakes every loop.
        listener.eager_reconnect(false);
        listener.listen(WORK_WAKE_CHANNEL).await?;
        Ok(listener)
    };
    tokio::time::timeout(SUBSCRIBE_TIMEOUT, subscribing)
        .await
        .unwrap_or(Err(sqlx::Error::PoolTimedOut))
}

fn decode(schema: &str, region: &str, payload: &str) -> Option<Wake> {
    let fields: Vec<serde_json::Value> = serde_json::from_str(payload).ok()?;
    if fields.first()?.as_str()? != schema {
        return None;
    }
    let kind = fields.get(1)?.as_str()?;
    if kind == "all" {
        return Some(Wake::All);
    }
    if fields.get(2)?.as_str()? != region {
        return None;
    }
    let tenant = fields.get(3)?.as_str()?.to_owned();
    match kind {
        "flow" => {
            let partition = match fields.get(4) {
                None | Some(serde_json::Value::Null) => None,
                Some(value) => Some(i16::try_from(value.as_i64()?).ok()?),
            };
            Some(Wake::Flow { tenant, partition })
        }
        "triggers" => Some(Wake::Triggers { tenant }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payloads_route_only_within_their_schema_and_region() {
        assert_eq!(
            decode("public", "fr-par", r#"["public","flow","fr-par","acme",3]"#),
            Some(Wake::Flow {
                tenant: "acme".into(),
                partition: Some(3)
            })
        );
        assert_eq!(
            decode(
                "public",
                "fr-par",
                r#"["public","flow","fr-par","acme",null]"#
            ),
            Some(Wake::Flow {
                tenant: "acme".into(),
                partition: None
            })
        );
        assert_eq!(
            decode(
                "public",
                "fr-par",
                r#"["public","triggers","fr-par","acme"]"#
            ),
            Some(Wake::Triggers {
                tenant: "acme".into()
            })
        );
        assert_eq!(
            decode("public", "fr-par", r#"["public","all"]"#),
            Some(Wake::All)
        );
        for foreign in [
            r#"["other","flow","fr-par","acme",3]"#,
            r#"["public","flow","se-sto","acme",3]"#,
            r#"["public","flow","fr-par","acme",99999]"#,
            r#"["public","unknown","fr-par","acme"]"#,
            "not json",
        ] {
            assert_eq!(decode("public", "fr-par", foreign), None, "{foreign}");
        }
    }

    #[tokio::test]
    async fn routing_wakes_only_the_loops_whose_work_changed() {
        let mut hub = WorkWakeHub::new("fr-par");
        let acme_3 = hub.flow_partition("acme", 3);
        let acme_4 = hub.flow_partition("acme", 4);
        let acme_claims = hub.trigger_claims("acme");
        let acme_reconciliation = hub.trigger_reconciliation("acme");
        let other_3 = hub.flow_partition("other", 3);

        hub.route("public", r#"["public","flow","fr-par","acme",3]"#);
        assert!(woken(&acme_3).await);
        assert!(woken(&acme_reconciliation).await);
        assert!(
            !woken(&acme_claims).await,
            "workflow changes never unblock a claim"
        );
        assert!(!woken(&acme_4).await);
        assert!(!woken(&other_3).await);

        hub.route("public", r#"["public","flow","fr-par","acme",null]"#);
        assert!(woken(&acme_3).await);
        assert!(woken(&acme_4).await);
        assert!(!woken(&other_3).await);

        hub.route("public", r#"["public","triggers","fr-par","acme"]"#);
        assert!(woken(&acme_claims).await);
        assert!(woken(&acme_reconciliation).await);
        assert!(!woken(&acme_3).await);
    }

    #[test]
    fn huge_intervals_jitter_without_overflowing() {
        assert!(jittered(Duration::MAX) <= Duration::from_secs(86_400 * 6 / 5));
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_read_backs_off_and_reports_once_a_minute() {
        let mut read = FailingRead::new(Duration::from_millis(250), Duration::from_secs(5));
        let delays: Vec<_> = (0..6).map(|_| read.failed().retry_after).collect();
        assert_eq!(
            delays,
            [250, 500, 1_000, 2_000, 4_000, 5_000].map(Duration::from_millis)
        );

        let mut read = FailingRead::new(Duration::from_millis(250), Duration::from_secs(5));
        assert_eq!(
            read.failed().report,
            Some(1),
            "the first failure is reported"
        );
        for _ in 0..10 {
            tokio::time::advance(Duration::from_secs(5)).await;
            assert_eq!(read.failed().report, None, "then held back for a minute");
        }
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(
            read.failed().report,
            Some(11),
            "with a count of what was held back"
        );

        read.succeeded();
        assert_eq!(read.failed().retry_after, Duration::from_millis(250));
    }

    #[test]
    fn jitter_stays_within_a_fifth_of_the_interval() {
        let interval = Duration::from_secs(5);
        let samples: Vec<_> = (0..1000).map(|_| jittered(interval)).collect();
        assert!(samples
            .iter()
            .all(|sample| (Duration::from_secs(4)..=Duration::from_secs(6)).contains(sample)));
        assert!(
            samples.iter().any(|sample| *sample != samples[0]),
            "samples vary"
        );
    }

    async fn woken(wake: &WorkWake) -> bool {
        tokio::time::timeout(Duration::from_millis(10), wake.woken())
            .await
            .is_ok()
    }
}
