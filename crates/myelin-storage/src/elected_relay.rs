//! Single-leader drain for the shared PostgreSQL outbox.
//!
//! The `outbox` table is shared by service producers, so independently running relays are unsafe:
//! `SKIP LOCKED` permits them to publish later aggregate sequence numbers before an earlier locked
//! row, and repeated broker failures can multiply retry/dead-letter accounting. This primitive uses
//! one PostgreSQL advisory lock to elect exactly one cooperating publisher for a drain pass, then
//! delegates to [`PgRelay::relay_once`]. A broker outage therefore rolls the outbox transaction
//! back: rows remain unsent and their permanent-failure `attempts` budget is untouched.

use myelin_events::relay::EventPublisher;
use sqlx::postgres::PgPool;

use crate::pg::PgError;
use crate::pgrelay::PgRelay;

/// Stable cell-local election key for the one shared-outbox publisher.
///
/// The value is an application namespace constant, not derived from stream/service names. Every
/// cooperating publisher attached to the same database must use this key.
pub const SHARED_OUTBOX_PUBLISHER_LOCK_ID: i64 = 0x4d59_454c_494e_4f42;

/// Result of one elected drain attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElectedDrainOutcome {
    /// Another cooperating instance holds the publisher election lock.
    Standby,
    /// This instance was elected and committed a pass, including an empty pass.
    Published(usize),
}

/// Loud failures from election, relay publication, or lock release.
#[derive(Debug)]
pub enum ElectedRelayError {
    InvalidConfiguration(String),
    Election(PgError),
    Relay(PgError),
    Unlock(PgError),
    RelayAndUnlock { relay: PgError, unlock: PgError },
}

impl core::fmt::Display for ElectedRelayError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidConfiguration(message) => {
                write!(f, "invalid elected-relay configuration: {message}")
            }
            Self::Election(error) => write!(f, "shared-outbox election failed: {error}"),
            Self::Relay(error) => write!(f, "elected shared-outbox publish failed: {error}"),
            Self::Unlock(error) => write!(f, "shared-outbox election unlock failed: {error}"),
            Self::RelayAndUnlock { relay, unlock } => write!(
                f,
                "elected shared-outbox publish failed ({relay}) and lock release failed ({unlock})"
            ),
        }
    }
}

impl std::error::Error for ElectedRelayError {}

/// PostgreSQL-advisory-lock elected wrapper around [`PgRelay::relay_once`].
#[derive(Clone)]
pub struct ElectedPgRelay {
    pool: PgPool,
    relay: PgRelay,
}

impl ElectedPgRelay {
    /// Use the stable shared-outbox election namespace.
    pub fn new(pool: PgPool) -> Result<Self, ElectedRelayError> {
        // One connection holds the session advisory lock while PgRelay opens the transaction that
        // claims rows. Refuse a one-connection pool instead of deadlocking at runtime.
        if pool.options().get_max_connections() < 2 {
            return Err(ElectedRelayError::InvalidConfiguration(
                "at least two connections are required (one election session and one relay tx)"
                    .into(),
            ));
        }
        Ok(Self {
            relay: PgRelay::new(pool.clone()),
            pool,
        })
    }

    /// Try to become publisher leader for one ordered drain pass.
    ///
    /// The election connection is marked `close_on_drop` before attempting the session lock. This
    /// is load-bearing: cancellation or panic cannot return a still-locked session to the pool.
    /// Normal completion also explicitly unlocks and verifies PostgreSQL reports ownership.
    pub async fn drain_once<P: EventPublisher + ?Sized>(
        &self,
        publisher: &P,
        batch: i64,
    ) -> Result<ElectedDrainOutcome, ElectedRelayError> {
        if batch <= 0 {
            return Err(ElectedRelayError::InvalidConfiguration(
                "relay batch must be positive".into(),
            ));
        }

        let mut election = self
            .pool
            .acquire()
            .await
            .map_err(|e| ElectedRelayError::Election(PgError::Query(e.to_string())))?;
        // If the lock query is cancelled after PostgreSQL acquired the lock, dropping this handle
        // closes the server session instead of returning a poisoned locked session to the pool.
        election.close_on_drop();

        let elected: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(SHARED_OUTBOX_PUBLISHER_LOCK_ID)
            .fetch_one(&mut *election)
            .await
            .map_err(|e| ElectedRelayError::Election(PgError::Query(e.to_string())))?;
        if !elected {
            return Ok(ElectedDrainOutcome::Standby);
        }

        let relay_result = self.relay.relay_once(publisher, batch).await;
        let unlock_result = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
            .bind(SHARED_OUTBOX_PUBLISHER_LOCK_ID)
            .fetch_one(&mut *election)
            .await
            .map_err(|e| PgError::Query(e.to_string()))
            .and_then(|unlocked| {
                if unlocked {
                    Ok(())
                } else {
                    Err(PgError::Query(
                        "pg_advisory_unlock reported this session did not own the lock".into(),
                    ))
                }
            });

        match (relay_result, unlock_result) {
            (Ok(published), Ok(())) => Ok(ElectedDrainOutcome::Published(published)),
            (Err(relay), Ok(())) => Err(ElectedRelayError::Relay(relay)),
            (Ok(_), Err(unlock)) => Err(ElectedRelayError::Unlock(unlock)),
            (Err(relay), Err(unlock)) => Err(ElectedRelayError::RelayAndUnlock { relay, unlock }),
        }
    }
}

#[cfg(test)]
mod tests {
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    #[tokio::test]
    async fn refuses_pool_that_cannot_hold_election_and_relay_connections() {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://myelin:myelin@127.0.0.1/myelin")
            .expect("lazy pool");
        let error = match ElectedPgRelay::new(pool) {
            Ok(_) => panic!("one connection would deadlock"),
            Err(error) => error,
        };
        assert!(matches!(error, ElectedRelayError::InvalidConfiguration(_)));
    }
}
