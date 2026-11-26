//! Commit-time wake-ups for durable worker loops.
//!
//! Workers that claim durable work (workflow partitions, agent-trigger queues) used to re-scan
//! every queue on a short timer, so an idle cell paid for every tenant it hosts. Database
//! triggers now `pg_notify` on [`WORK_WAKE_CHANNEL`] whenever a write can make work claimable,
//! and [`WorkWakeHub`] routes each notification to the loop that owns that work.
//!
//! A wake is only a hint to look again: claiming stays the job of the existing lease and
//! `SKIP LOCKED` queries, so a lost, duplicated or spurious wake can delay work but never
//! drive it twice. Loops still wake at their own deadlines (timers, lease expiry) and on a
//! slow safety interval. While the listener is disconnected the hub wakes every loop on the
//! old polling interval, so a broken listener degrades to the previous behaviour.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use sqlx::postgres::PgListener;
use sqlx::PgPool;
use tokio::sync::{watch, Notify};

use crate::migration::{Migration, Migrations};

pub const WORK_WAKE_CHANNEL: &str = "myelin_work_wake";

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

pub fn agent_trigger_work_wake_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0189_agent_trigger_work_wake",
        AGENT_TRIGGER_WORK_WAKE_MIGRATION,
    )])
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

/// Routes notifications for one region of one schema to the loops registered for them.
pub struct WorkWakeHub {
    region: String,
    flow: HashMap<String, Vec<(i16, Arc<Notify>)>>,
    triggers: HashMap<String, Vec<Arc<Notify>>>,
}

impl WorkWakeHub {
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            region: region.into(),
            flow: HashMap::new(),
            triggers: HashMap::new(),
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

    /// Woken when the tenant's trigger queue changes or any of its workflow runs changes
    /// state (a terminal run is what trigger reconciliation waits for).
    pub fn tenant_triggers(&mut self, tenant: &str) -> WorkWake {
        let notify = Arc::new(Notify::new());
        self.triggers
            .entry(tenant.to_owned())
            .or_default()
            .push(notify.clone());
        WorkWake(notify)
    }

    /// Listens until shutdown. Every (re)connection wakes all loops once, covering commits
    /// made while no listener was subscribed; while disconnected, all loops are woken every
    /// `degraded_poll`.
    pub async fn run(
        self,
        pool: PgPool,
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
            let mut listener = match subscribe(&pool).await {
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
            loop {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { return Ok(()); }
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
                self.wake_triggers(&tenant);
            }
            Some(Wake::Triggers { tenant }) => self.wake_triggers(&tenant),
            None => {}
        }
    }

    fn wake_triggers(&self, tenant: &str) {
        for notify in self.triggers.get(tenant).into_iter().flatten() {
            notify.notify_one();
        }
    }

    fn wake_all(&self) {
        let flow = self.flow.values().flatten().map(|(_, notify)| notify);
        for notify in flow.chain(self.triggers.values().flatten()) {
            notify.notify_one();
        }
    }
}

async fn subscribe(pool: &PgPool) -> Result<PgListener, sqlx::Error> {
    let mut listener = PgListener::connect_with(pool).await?;
    listener.listen(WORK_WAKE_CHANNEL).await?;
    Ok(listener)
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
    async fn routing_wakes_the_exact_partition_and_the_tenant_trigger_loops() {
        let mut hub = WorkWakeHub::new("fr-par");
        let acme_3 = hub.flow_partition("acme", 3);
        let acme_4 = hub.flow_partition("acme", 4);
        let acme_triggers = hub.tenant_triggers("acme");
        let other_3 = hub.flow_partition("other", 3);

        hub.route("public", r#"["public","flow","fr-par","acme",3]"#);
        assert!(woken(&acme_3).await);
        assert!(woken(&acme_triggers).await);
        assert!(!woken(&acme_4).await);
        assert!(!woken(&other_3).await);

        hub.route("public", r#"["public","flow","fr-par","acme",null]"#);
        assert!(woken(&acme_3).await);
        assert!(woken(&acme_4).await);
        assert!(!woken(&other_3).await);

        hub.route("public", r#"["public","triggers","fr-par","acme"]"#);
        assert!(woken(&acme_triggers).await);
        assert!(!woken(&acme_3).await);
    }

    async fn woken(wake: &WorkWake) -> bool {
        tokio::time::timeout(Duration::from_millis(10), wake.woken())
            .await
            .is_ok()
    }
}
