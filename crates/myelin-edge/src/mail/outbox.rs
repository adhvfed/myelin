//! Mail waiting to be sent. A letter is written in the same transaction as the change it reports,
//! so a link exists only if its mail is queued. Letters hold addresses and links, so each row is
//! sealed under a key derived from the cell's seal key and bound to its cell and id.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use myelin_storage::migration::Migration;
use rand::{rngs::OsRng, RngCore};
use sqlx::{PgConnection, PgPool, Row};
use zeroize::Zeroizing;

use super::templates::Letter;

const OUTBOX_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS edge_mail_outbox (
    message_id        bytea   PRIMARY KEY CHECK (octet_length(message_id) = 16),
    cell_id           text    NOT NULL,
    link_digest       bytea   CHECK (octet_length(link_digest) = 32),
    payload_nonce     bytea   NOT NULL CHECK (octet_length(payload_nonce) = 12),
    payload           bytea   NOT NULL CHECK (octet_length(payload) <= 16384),
    attempts          integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    created_at_unix   bigint  NOT NULL,
    next_attempt_unix bigint  NOT NULL
);
CREATE INDEX IF NOT EXISTS edge_mail_outbox_due_idx
    ON edge_mail_outbox (cell_id, next_attempt_unix);
CREATE INDEX IF NOT EXISTS edge_mail_outbox_link_idx
    ON edge_mail_outbox (link_digest) WHERE link_digest IS NOT NULL;
REVOKE ALL ON TABLE edge_mail_outbox FROM PUBLIC;
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE edge_mail_outbox TO myelin_app;
"#;

/// Applied with the account tables, whose links it carries.
pub(crate) fn outbox_migration() -> Migration {
    Migration::plain("edge_0004_mail_outbox", OUTBOX_DDL)
}

const DB_NOW: &str = "floor(extract(epoch FROM clock_timestamp()))::bigint";
const NONCE_BYTES: usize = 12;

pub(crate) type MessageId = [u8; 16];

/// Seals letters into this cell's outbox and opens them again.
#[derive(Clone)]
pub struct MailOutbox {
    cell: String,
    key: Zeroizing<[u8; 32]>,
}

/// A queued letter that one worker holds until `claimed_until`.
pub(crate) struct Claimed {
    pub id: MessageId,
    pub attempts: i32,
    pub claimed_until: i64,
    /// `None` when the row cannot be opened, which no retry would change.
    pub letter: Option<Letter>,
    /// False when the letter carries a link that has since expired or been used.
    pub link_live: bool,
}

/// What a failed attempt led to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AfterFailure {
    Retrying,
    GaveUp,
    /// Another worker took the row after this one's claim ran out.
    NotOurs,
}

impl MailOutbox {
    pub const SEAL_KEY_CONTEXT: &'static str = "myelin 2026-11-25 mail outbox seal v1";

    pub fn new(cell_id: impl Into<String>, key: Zeroizing<[u8; 32]>) -> Self {
        Self {
            cell: cell_id.into(),
            key,
        }
    }

    /// Queues `letter` in the caller's transaction, due at once. `link` is the digest of the link
    /// it carries, so superseding or using the link withdraws the letter.
    pub(crate) async fn enqueue(
        &self,
        conn: &mut PgConnection,
        letter: &Letter,
        link: Option<&[u8; 32]>,
    ) -> Result<(), sqlx::Error> {
        let mut id = [0_u8; 16];
        OsRng.fill_bytes(&mut id);
        let (nonce, payload) = self.seal(&id, letter);
        // @tenant-cross-scope: the outbox belongs to the cell, not a tenant; rows are sealed.
        sqlx::query(&format!(
            "INSERT INTO edge_mail_outbox \
               (message_id, cell_id, link_digest, payload_nonce, payload, created_at_unix, \
                next_attempt_unix) \
             VALUES ($1, $2, $3, $4, $5, {DB_NOW}, {DB_NOW})"
        ))
        .bind(id.as_slice())
        .bind(&self.cell)
        .bind(link.map(|digest| digest.as_slice()))
        .bind(nonce.as_slice())
        .bind(payload)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Takes the next due letter for `lease_secs`. Several Edge instances can claim at once: a
    /// row one of them holds is skipped, not waited on.
    pub(crate) async fn claim(
        &self,
        pool: &PgPool,
        lease_secs: i64,
    ) -> Result<Option<Claimed>, sqlx::Error> {
        // @tenant-cross-scope: a worker claims its cell's mail; a link digest names one link.
        let row = sqlx::query(&format!(
            "UPDATE edge_mail_outbox \
                SET attempts = attempts + 1, next_attempt_unix = {DB_NOW} + $2 \
              WHERE message_id = ( \
                SELECT message_id FROM edge_mail_outbox \
                 WHERE cell_id = $1 AND next_attempt_unix <= {DB_NOW} \
                 ORDER BY next_attempt_unix LIMIT 1 FOR UPDATE SKIP LOCKED) \
             RETURNING message_id, attempts, next_attempt_unix, payload_nonce, payload, \
               (link_digest IS NULL OR EXISTS ( \
                  SELECT 1 FROM auth_invitation \
                   WHERE code_digest = edge_mail_outbox.link_digest \
                     AND expires_at_unix > {DB_NOW})) AS link_live"
        ))
        .bind(&self.cell)
        .bind(lease_secs)
        .fetch_optional(pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let id: Vec<u8> = row.get("message_id");
        let Ok(id) = MessageId::try_from(id.as_slice()) else {
            return Err(sqlx::Error::Protocol("a mail id is not 16 bytes".into()));
        };
        let nonce: Vec<u8> = row.get("payload_nonce");
        let payload: Vec<u8> = row.get("payload");
        Ok(Some(Claimed {
            id,
            attempts: row.get("attempts"),
            claimed_until: row.get("next_attempt_unix"),
            letter: self.open(&id, &nonce, &payload),
            link_live: row.get("link_live"),
        }))
    }

    /// Deletes this cell's letters queued at least `give_up_after_secs` ago that no worker holds,
    /// and returns their ids and attempts. A letter whose attempts never end in a recorded
    /// failure (a worker killed mid-send, say) is dropped here.
    pub(crate) async fn drop_expired(
        &self,
        pool: &PgPool,
        give_up_after_secs: i64,
    ) -> Result<Vec<(MessageId, i32)>, sqlx::Error> {
        // @tenant-cross-scope: the outbox belongs to the cell, not a tenant.
        let dropped: Vec<(Vec<u8>, i32)> = sqlx::query_as(&format!(
            "DELETE FROM edge_mail_outbox \
              WHERE message_id IN ( \
                SELECT message_id FROM edge_mail_outbox \
                 WHERE cell_id = $1 AND created_at_unix + $2 <= {DB_NOW} \
                   AND next_attempt_unix <= {DB_NOW} \
                 LIMIT 64 FOR UPDATE SKIP LOCKED) \
             RETURNING message_id, attempts"
        ))
        .bind(&self.cell)
        .bind(give_up_after_secs)
        .fetch_all(pool)
        .await?;
        Ok(dropped
            .into_iter()
            .filter_map(|(id, attempts)| Some((MessageId::try_from(id.as_slice()).ok()?, attempts)))
            .collect())
    }

    pub(crate) async fn sent(&self, pool: &PgPool, id: &MessageId) -> Result<(), sqlx::Error> {
        // @tenant-cross-scope: a message id names one row of the cell's outbox.
        sqlx::query("DELETE FROM edge_mail_outbox WHERE message_id = $1")
            .bind(id.as_slice())
            .execute(pool)
            .await?;
        Ok(())
    }

    /// Schedules another try in `retry_in_secs`, or drops the letter once it has been queued for
    /// `give_up_after_secs` (or at once when `give_up_after_secs` is zero).
    pub(crate) async fn failed(
        &self,
        pool: &PgPool,
        claimed: &Claimed,
        retry_in_secs: i64,
        give_up_after_secs: i64,
    ) -> Result<AfterFailure, sqlx::Error> {
        // @tenant-cross-scope: a message id names one row of the cell's outbox.
        let dropped = sqlx::query(&format!(
            "DELETE FROM edge_mail_outbox \
              WHERE message_id = $1 AND next_attempt_unix = $2 \
                AND created_at_unix + $3 <= {DB_NOW}"
        ))
        .bind(claimed.id.as_slice())
        .bind(claimed.claimed_until)
        .bind(give_up_after_secs)
        .execute(pool)
        .await?
        .rows_affected();
        if dropped == 1 {
            return Ok(AfterFailure::GaveUp);
        }
        // @tenant-cross-scope: a message id names one row of the cell's outbox.
        let rescheduled = sqlx::query(&format!(
            "UPDATE edge_mail_outbox SET next_attempt_unix = {DB_NOW} + $3 \
              WHERE message_id = $1 AND next_attempt_unix = $2"
        ))
        .bind(claimed.id.as_slice())
        .bind(claimed.claimed_until)
        .bind(retry_in_secs)
        .execute(pool)
        .await?
        .rows_affected();
        Ok(if rescheduled == 1 {
            AfterFailure::Retrying
        } else {
            AfterFailure::NotOurs
        })
    }

    fn seal(&self, id: &MessageId, letter: &Letter) -> ([u8; NONCE_BYTES], Vec<u8>) {
        let plain = Zeroizing::new(serde_json::to_vec(letter).expect("a letter serializes"));
        let mut nonce = [0_u8; NONCE_BYTES];
        OsRng.fill_bytes(&mut nonce);
        let sealed = self
            .cipher()
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &plain,
                    aad: &self.binding(id),
                },
            )
            .expect("AES-256-GCM encryption of a letter cannot fail");
        (nonce, sealed)
    }

    fn open(&self, id: &MessageId, nonce: &[u8], payload: &[u8]) -> Option<Letter> {
        if nonce.len() != NONCE_BYTES {
            return None;
        }
        let plain = Zeroizing::new(
            self.cipher()
                .decrypt(
                    Nonce::from_slice(nonce),
                    Payload {
                        msg: payload,
                        aad: &self.binding(id),
                    },
                )
                .ok()?,
        );
        serde_json::from_slice(&plain).ok()
    }

    /// A row opens only as itself, in its own cell.
    fn binding(&self, id: &MessageId) -> Vec<u8> {
        let mut binding = b"myelin.edge.mail.v1\0".to_vec();
        binding.extend_from_slice(self.cell.as_bytes());
        binding.push(0);
        binding.extend_from_slice(id);
        binding
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(self.key.as_slice()).expect("AES-256-GCM takes a 32-byte key")
    }
}

/// Withdraws letters that carry any of `links`, in the caller's transaction.
pub(crate) async fn withdraw(
    conn: &mut PgConnection,
    links: &[Vec<u8>],
) -> Result<(), sqlx::Error> {
    if links.is_empty() {
        return Ok(());
    }
    // @tenant-cross-scope: the outbox belongs to the cell; a link digest names one row.
    sqlx::query("DELETE FROM edge_mail_outbox WHERE link_digest = ANY($1)")
        .bind(links)
        .execute(conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letter() -> Letter {
        Letter::PasswordReset {
            to: "ada@example.com".into(),
            link: "https://myelin.example/join/secret-code".into(),
            expires_at_unix: 1,
        }
    }

    #[test]
    fn a_sealed_letter_opens_only_as_itself_in_its_own_cell() {
        let outbox = MailOutbox::new("cell-a", Zeroizing::new([7; 32]));
        let (nonce, sealed) = outbox.seal(&[1; 16], &letter());
        let printed = String::from_utf8_lossy(&sealed);
        assert!(!printed.contains("ada@example.com") && !printed.contains("secret-code"));
        assert_eq!(outbox.open(&[1; 16], &nonce, &sealed), Some(letter()));
        assert_eq!(outbox.open(&[2; 16], &nonce, &sealed), None);
        let other_cell = MailOutbox::new("cell-b", Zeroizing::new([7; 32]));
        assert_eq!(other_cell.open(&[1; 16], &nonce, &sealed), None);
        let other_key = MailOutbox::new("cell-a", Zeroizing::new([8; 32]));
        assert_eq!(other_key.open(&[1; 16], &nonce, &sealed), None);
    }
}
