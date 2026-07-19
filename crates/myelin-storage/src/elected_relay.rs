//! Single-leader drain for the shared PostgreSQL outbox.
//!
//! The `outbox` table is shared by service producers, so independently running relays are unsafe:
//! `SKIP LOCKED` permits them to publish later aggregate sequence numbers before an earlier locked
//! row, and repeated broker failures can multiply retry/dead-letter accounting. This primitive uses
//! one transaction-scoped PostgreSQL advisory lock to elect exactly one cooperating publisher for
//! a drain pass. Election, claims, publication, quarantine, sent marks, and commit use the same
//! transaction and connection. A broker outage therefore rolls the transaction back: rows remain
//! unsent and their permanent-failure `attempts` budget is untouched.

use myelin_events::relay::EventPublisher;
use sqlx::postgres::PgPool;

use crate::pg::PgError;
use crate::pgrelay::{PgRelay, RelayValidationConfig};

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
    validation: RelayValidationConfig,
}

impl ElectedPgRelay {
    /// Use the stable shared-outbox election namespace.
    pub fn new(pool: PgPool, validation: RelayValidationConfig) -> Result<Self, ElectedRelayError> {
        Ok(Self { pool, validation })
    }

    /// Try to become publisher leader for one ordered drain pass.
    ///
    /// The transaction-scoped lock is released atomically by commit or rollback. Cancellation,
    /// panic, connection loss, and publish failure therefore cannot leave a stale elected session.
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

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| ElectedRelayError::Election(PgError::Query(e.to_string())))?;

        // @tenant-cross-scope: the cell-local publisher election lock protects the shared outbox.
        let elected: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
            .bind(SHARED_OUTBOX_PUBLISHER_LOCK_ID)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| ElectedRelayError::Election(PgError::Query(e.to_string())))?;
        if !elected {
            return Ok(ElectedDrainOutcome::Standby);
        }

        let relay = PgRelay::new(self.pool.clone());
        let published = relay
            .relay_once_scoped_in_tx(&mut tx, publisher, batch, &self.validation)
            .await
            .map_err(ElectedRelayError::Relay)?;
        tx.commit()
            .await
            .map_err(|e| ElectedRelayError::Relay(PgError::Query(e.to_string())))?;
        Ok(ElectedDrainOutcome::Published(published))
    }
}

#[cfg(test)]
mod tests {
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    #[tokio::test]
    async fn one_connection_pool_is_valid() {
        // @residency-cell-pinned: lazy invalid test pool; the relay scope below pins its region.
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://myelin:myelin@127.0.0.1/myelin")
            .expect("lazy pool");
        let validation = RelayValidationConfig::new(myelin_events::Region("no-osl".into()), 1024)
            .expect("valid scope");
        ElectedPgRelay::new(pool, validation).expect("election and relay share one transaction");
    }
}
