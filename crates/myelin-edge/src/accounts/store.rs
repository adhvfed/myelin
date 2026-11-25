//! Durable state for password sign-in: one account row per email, single-use invitations, and
//! fixed-window rate-limit counters.
//!
//! Sign-in and invitation links are used before anyone is authenticated, so the lookups here are
//! keyed by opaque digests and are not tenant-scoped (like `auth_device_authorization`). Every
//! write that creates or changes a person runs in a tenant transaction, so the `principal` row it
//! touches stays under row-level security.

use myelin_storage::migration::{Migration, Migrations};
use myelin_storage::{
    with_tenant_tx_error, DurablePrincipalBacking, DurablePrincipalRow, PgError,
    ACTIVE_PRINCIPAL_STATUS_JSON,
};
use sqlx::{PgPool, Row};

use super::credentials::{CodeDigest, EmailDigest, SealedEmail};
use crate::mail::{Letter, MailOutbox};

const ACCOUNT_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS auth_password_account (
    email_digest    bytea  PRIMARY KEY CHECK (octet_length(email_digest) = 32),
    tenant_id       text   NOT NULL,
    region          text   NOT NULL,
    principal_id    text   NOT NULL,
    password_phc    text   NOT NULL CHECK (octet_length(password_phc) BETWEEN 1 AND 512),
    changed_at_unix bigint NOT NULL,
    UNIQUE (tenant_id, region, principal_id),
    FOREIGN KEY (tenant_id, region, principal_id)
        REFERENCES principal (tenant_id, region, principal_id) ON DELETE CASCADE
);
REVOKE ALL ON TABLE auth_password_account FROM PUBLIC;
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE auth_password_account TO myelin_app;

CREATE TABLE IF NOT EXISTS auth_invitation (
    code_digest      bytea  PRIMARY KEY CHECK (octet_length(code_digest) = 32),
    kind             text   NOT NULL CHECK (kind IN ('join', 'reset')),
    tenant_id        text   NOT NULL,
    region           text   NOT NULL,
    email_digest     bytea  NOT NULL CHECK (octet_length(email_digest) = 32),
    email_nonce      bytea  NOT NULL CHECK (octet_length(email_nonce) = 12),
    email_ciphertext bytea  NOT NULL CHECK (octet_length(email_ciphertext) <= 512),
    principal_id     text,
    created_at_unix  bigint NOT NULL,
    expires_at_unix  bigint NOT NULL,
    CHECK ((kind = 'reset') = (principal_id IS NOT NULL)),
    FOREIGN KEY (tenant_id, region, principal_id)
        REFERENCES principal (tenant_id, region, principal_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS auth_invitation_email_idx
    ON auth_invitation (tenant_id, region, email_digest);
CREATE INDEX IF NOT EXISTS auth_invitation_expiry_idx ON auth_invitation (expires_at_unix);
REVOKE ALL ON TABLE auth_invitation FROM PUBLIC;
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE auth_invitation TO myelin_app;

CREATE TABLE IF NOT EXISTS auth_rate_limit (
    bucket              bytea  PRIMARY KEY CHECK (octet_length(bucket) = 32),
    window_ends_at_unix bigint NOT NULL,
    hits                bigint NOT NULL CHECK (hits > 0)
);
CREATE INDEX IF NOT EXISTS auth_rate_limit_expiry_idx ON auth_rate_limit (window_ends_at_unix);
REVOKE ALL ON TABLE auth_rate_limit FROM PUBLIC;
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE auth_rate_limit TO myelin_app;
"#;

/// Who made each link, so an operator's new reset link can revoke only the operator's earlier
/// ones. Rows stored before the column existed are marked the operator's; only reset links read
/// it.
const INVITATION_ORIGIN_DDL: &str = r#"
ALTER TABLE auth_invitation ADD COLUMN IF NOT EXISTS origin text NOT NULL DEFAULT 'operator';
ALTER TABLE auth_invitation ALTER COLUMN origin DROP DEFAULT;
ALTER TABLE auth_invitation DROP CONSTRAINT IF EXISTS auth_invitation_origin_check;
ALTER TABLE auth_invitation ADD CONSTRAINT auth_invitation_origin_check
    CHECK (origin IN ('operator', 'member', 'requested'));
"#;

/// The account tables, the mail outbox that carries their links, and each link's origin.
pub fn account_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("edge_0003_password_accounts", ACCOUNT_DDL),
        crate::mail::outbox_migration(),
        Migration::plain("edge_0005_invitation_origin", INVITATION_ORIGIN_DDL),
    ])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InvitationKind {
    Join,
    Reset,
}

impl InvitationKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Join => "join",
            Self::Reset => "reset",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "join" => Ok(Self::Join),
            "reset" => Ok(Self::Reset),
            _ => Err(StoreError::corrupt("invitation kind")),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AccountRow {
    pub email_digest: EmailDigest,
    pub tenant: String,
    pub region: String,
    pub principal_id: String,
    pub password_phc: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Invitation {
    pub kind: InvitationKind,
    pub tenant: String,
    pub region: String,
    pub email_digest: EmailDigest,
    pub email: SealedEmail,
    /// The account a reset link changes; `None` for a join link.
    pub principal_id: Option<String>,
    pub expires_at_unix: i64,
}

/// A link about to be stored. Its expiry is set by the database clock when it is stored.
pub(crate) struct NewInvitation {
    pub tenant: String,
    pub region: String,
    pub email_digest: EmailDigest,
    pub email: SealedEmail,
    pub ttl_secs: i64,
}

/// The letter a new link is emailed in.
pub(crate) enum LinkLetter {
    Invitation { organization: String },
    PasswordReset,
}

/// A link's letter, written once the link's expiry is known and queued with the link.
pub(crate) struct LinkMail {
    pub outbox: MailOutbox,
    pub letter: LinkLetter,
    pub to: String,
    pub url: String,
}

impl LinkMail {
    fn letter(&self, expires_at_unix: i64) -> Letter {
        let (to, link) = (self.to.clone(), self.url.clone());
        match &self.letter {
            LinkLetter::Invitation { organization } => Letter::Invitation {
                to,
                organization: organization.clone(),
                link,
                expires_at_unix,
            },
            LinkLetter::PasswordReset => Letter::PasswordReset {
                to,
                link,
                expires_at_unix,
            },
        }
    }
}

/// Who may get a link, decided under the same lock that stores it, and which earlier links it
/// replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IssueRule {
    /// A member's join link: refused when the email has an account in this organization.
    MemberJoin,
    /// The operator's join link: refused when the email has an account anywhere in the cell.
    OperatorJoin,
    /// The operator's reset link for the account the email has in this organization.
    OperatorReset,
    /// A reset link someone asked for by email, for the account in this organization.
    RequestedReset,
}

impl IssueRule {
    fn kind(self) -> InvitationKind {
        match self {
            Self::MemberJoin | Self::OperatorJoin => InvitationKind::Join,
            Self::OperatorReset | Self::RequestedReset => InvitationKind::Reset,
        }
    }

    fn origin(self) -> &'static str {
        match self {
            Self::MemberJoin => "member",
            Self::OperatorJoin | Self::OperatorReset => "operator",
            Self::RequestedReset => "requested",
        }
    }

    /// The earlier links for the same email in the same organization this one replaces. A new
    /// join link replaces every join link. An operator's reset link replaces the operator's
    /// earlier ones, so a link sent the wrong way can be revoked by issuing another; a requested
    /// one replaces nothing, so nobody can void a reset link by asking for another.
    fn supersedes(self) -> Option<&'static str> {
        match self {
            Self::MemberJoin | Self::OperatorJoin => Some("kind = 'join'"),
            Self::OperatorReset => Some("kind = 'reset' AND origin = 'operator'"),
            Self::RequestedReset => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IssueOutcome {
    Issued { expires_at_unix: i64 },
    AccountExists,
    NoSuchAccount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AcceptOutcome {
    Accepted {
        principal_id: String,
    },
    /// Unknown, expired, already used, or no longer applicable. Callers answer 404 for all.
    Gone,
    EmailTaken,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoreError(String);

impl StoreError {
    fn database(error: sqlx::Error) -> Self {
        Self(format!("account database operation failed: {error}"))
    }

    fn corrupt(field: &str) -> Self {
        Self(format!("stored account data has an invalid {field}"))
    }
}

impl core::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<PgError> for StoreError {
    fn from(error: PgError) -> Self {
        Self(format!("account database operation failed: {error}"))
    }
}

/// Why a tenant transaction stopped. Anything but `Store` is an expected outcome that must still
/// roll back what the transaction already did.
enum Abort {
    Gone,
    EmailTaken,
    Store(StoreError),
}

impl From<PgError> for Abort {
    fn from(error: PgError) -> Self {
        Self::Store(error.into())
    }
}

impl From<sqlx::Error> for Abort {
    fn from(error: sqlx::Error) -> Self {
        Self::Store(StoreError::database(error))
    }
}

fn settle(result: Result<String, Abort>) -> Result<AcceptOutcome, StoreError> {
    match result {
        Ok(principal_id) => Ok(AcceptOutcome::Accepted { principal_id }),
        Err(Abort::Gone) => Ok(AcceptOutcome::Gone),
        Err(Abort::EmailTaken) => Ok(AcceptOutcome::EmailTaken),
        Err(Abort::Store(error)) => Err(error),
    }
}

/// One hit taken in one window of a counter. Giving it back touches only that window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Slot {
    pub bucket: [u8; 32],
    window_ends_at_unix: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reservation {
    Taken(Slot),
    Full { retry_after_secs: u64 },
}

/// Unix seconds by the database clock. Link expiry is judged by it alone, at the moment of the
/// statement, so neither a skewed Edge clock nor a slow request can stretch a link's life.
const DB_NOW: &str = "floor(extract(epoch FROM clock_timestamp()))::bigint";

#[derive(Clone)]
pub(crate) struct AccountStore {
    pool: PgPool,
    #[cfg(any(test, feature = "test-support"))]
    checkpoint: Option<super::CheckpointHook>,
}

impl AccountStore {
    pub(crate) fn new(pool: PgPool) -> Self {
        Self {
            pool,
            #[cfg(any(test, feature = "test-support"))]
            checkpoint: None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn with_checkpoint(mut self, hook: super::CheckpointHook) -> Self {
        self.checkpoint = Some(hook);
        self
    }

    pub(crate) async fn account(
        &self,
        email: &EmailDigest,
    ) -> Result<Option<AccountRow>, StoreError> {
        account_in(&self.pool, email).await
    }

    /// Takes one slot of `limit` in a fixed window, or says how long until the window ends. The
    /// check and the increment are one statement, so concurrent callers cannot all see room: the
    /// counter never passes `limit`, however many ask at once. It is shared by every Edge
    /// instance that uses this database. Each call also sweeps some expired counters, so the
    /// table holds about one window's worth of hits however many distinct keys are tried.
    pub(crate) async fn reserve_hit(
        &self,
        bucket: [u8; 32],
        limit: u32,
        window_secs: i64,
        now_unix: i64,
    ) -> Result<Reservation, StoreError> {
        // @tenant-cross-scope: bounded cleanup of expired, opaque counters.
        sqlx::query(
            "DELETE FROM auth_rate_limit WHERE bucket IN \
               (SELECT bucket FROM auth_rate_limit WHERE window_ends_at_unix <= $1 LIMIT 64)",
        )
        .bind(now_unix)
        .execute(&self.pool)
        .await
        .map_err(StoreError::database)?;
        // A full window matches no row to update, so nothing is returned and nothing is written.
        let taken: Option<i64> = sqlx::query_scalar(
            "INSERT INTO auth_rate_limit (bucket, window_ends_at_unix, hits) VALUES ($1, $2, 1) \
             ON CONFLICT (bucket) DO UPDATE SET \
               hits = CASE WHEN auth_rate_limit.window_ends_at_unix <= $3 THEN 1 \
                           ELSE auth_rate_limit.hits + 1 END, \
               window_ends_at_unix = CASE WHEN auth_rate_limit.window_ends_at_unix <= $3 \
                                          THEN EXCLUDED.window_ends_at_unix \
                                          ELSE auth_rate_limit.window_ends_at_unix END \
             WHERE auth_rate_limit.window_ends_at_unix <= $3 OR auth_rate_limit.hits < $4 \
             RETURNING window_ends_at_unix",
        )
        .bind(bucket.as_slice())
        .bind(now_unix.saturating_add(window_secs))
        .bind(now_unix)
        .bind(i64::from(limit))
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::database)?;
        if let Some(window_ends_at_unix) = taken {
            return Ok(Reservation::Taken(Slot {
                bucket,
                window_ends_at_unix,
            }));
        }
        let window_ends_at_unix: Option<i64> = sqlx::query_scalar(
            "SELECT window_ends_at_unix FROM auth_rate_limit \
              WHERE bucket = $1 AND window_ends_at_unix > $2",
        )
        .bind(bucket.as_slice())
        .bind(now_unix)
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::database)?;
        // The window may have ended since the refusal; then a retry is already due.
        let ends = window_ends_at_unix.unwrap_or(now_unix);
        Ok(Reservation::Full {
            retry_after_secs: u64::try_from(ends.saturating_sub(now_unix).max(1)).unwrap_or(1),
        })
    }

    /// Gives back a slot taken by `reserve_hit`, if its window is still the counter's: a window
    /// that has since been replaced holds other people's hits, so it is left alone. The row is
    /// locked while it is read, so concurrent gives-back each remove exactly one hit; a single
    /// conditional statement would judge them all against the same stale count.
    pub(crate) async fn release_hit(&self, slot: Slot) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await.map_err(StoreError::database)?;
        let hits: Option<i64> = sqlx::query_scalar(
            "SELECT hits FROM auth_rate_limit \
              WHERE bucket = $1 AND window_ends_at_unix = $2 FOR UPDATE",
        )
        .bind(slot.bucket.as_slice())
        .bind(slot.window_ends_at_unix)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(StoreError::database)?;
        // A counter cannot hold zero hits, so the last one is removed with its row.
        let statement = match hits {
            None => return Ok(()),
            Some(hits) if hits <= 1 => "DELETE FROM auth_rate_limit WHERE bucket = $1",
            Some(_) => "UPDATE auth_rate_limit SET hits = hits - 1 WHERE bucket = $1",
        };
        sqlx::query(statement)
            .bind(slot.bucket.as_slice())
            .execute(&mut *transaction)
            .await
            .map_err(StoreError::database)?;
        transaction.commit().await.map_err(StoreError::database)?;
        Ok(())
    }

    pub(crate) async fn clear_hits(&self, bucket: [u8; 32]) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM auth_rate_limit WHERE bucket = $1")
            .bind(bucket.as_slice())
            .execute(&self.pool)
            .await
            .map_err(StoreError::database)?;
        Ok(())
    }

    /// Decides whether the link may be issued and stores it, under the per-email lock that
    /// accepting a link also takes. It replaces the earlier links `IssueRule::supersedes` names,
    /// and withdraws any mail still carrying them. `mail` is queued in the same transaction.
    pub(crate) async fn issue(
        &self,
        code: &CodeDigest,
        rule: IssueRule,
        link: &NewInvitation,
        mail: Option<&LinkMail>,
    ) -> Result<IssueOutcome, StoreError> {
        let mut transaction = self.pool.begin().await.map_err(StoreError::database)?;
        lock_email(
            &mut transaction,
            &link.tenant,
            &link.region,
            &link.email_digest,
        )
        .await
        .map_err(StoreError::database)?;
        let account = account_in(&mut *transaction, &link.email_digest).await?;
        let in_this_organization = account
            .as_ref()
            .filter(|account| account.tenant == link.tenant && account.region == link.region);
        let principal_id = match rule {
            IssueRule::OperatorJoin if account.is_some() => return Ok(IssueOutcome::AccountExists),
            IssueRule::MemberJoin if in_this_organization.is_some() => {
                return Ok(IssueOutcome::AccountExists)
            }
            IssueRule::OperatorJoin | IssueRule::MemberJoin => None,
            IssueRule::OperatorReset | IssueRule::RequestedReset => match in_this_organization {
                Some(account) => Some(account.principal_id.clone()),
                None => return Ok(IssueOutcome::NoSuchAccount),
            },
        };
        if let Some(replaced) = rule.supersedes() {
            let superseded: Vec<Vec<u8>> = sqlx::query_scalar(&format!(
                "DELETE FROM auth_invitation \
                  WHERE tenant_id = $1 AND region = $2 AND email_digest = $3 AND {replaced} \
                 RETURNING code_digest"
            ))
            .bind(&link.tenant)
            .bind(&link.region)
            .bind(link.email_digest.as_slice())
            .fetch_all(&mut *transaction)
            .await
            .map_err(StoreError::database)?;
            crate::mail::withdraw(&mut transaction, &superseded)
                .await
                .map_err(StoreError::database)?;
        }
        let expires_at_unix: i64 = sqlx::query_scalar(&format!(
            "INSERT INTO auth_invitation \
               (code_digest, kind, tenant_id, region, email_digest, email_nonce, \
                email_ciphertext, principal_id, created_at_unix, expires_at_unix, origin) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, {DB_NOW}, {DB_NOW} + $9, $10) \
             RETURNING expires_at_unix"
        ))
        .bind(code.as_slice())
        .bind(rule.kind().as_str())
        .bind(&link.tenant)
        .bind(&link.region)
        .bind(link.email_digest.as_slice())
        .bind(link.email.nonce.as_slice())
        .bind(&link.email.ciphertext)
        .bind(&principal_id)
        .bind(link.ttl_secs)
        .bind(rule.origin())
        .fetch_one(&mut *transaction)
        .await
        .map_err(StoreError::database)?;
        if let Some(mail) = mail {
            mail.outbox
                .enqueue(&mut transaction, &mail.letter(expires_at_unix), Some(code))
                .await
                .map_err(StoreError::database)?;
        }
        // @tenant-cross-scope: bounded cleanup of expired, unreadable invitations. It skips rows
        // others hold, so it never waits; it comes last, so the locks it takes are never held
        // while this transaction waits for anything else, as a supersede above might.
        sqlx::query(&format!(
            "DELETE FROM auth_invitation WHERE code_digest IN \
               (SELECT code_digest FROM auth_invitation WHERE expires_at_unix <= {DB_NOW} \
                LIMIT 256 FOR UPDATE SKIP LOCKED)"
        ))
        .execute(&mut *transaction)
        .await
        .map_err(StoreError::database)?;
        transaction.commit().await.map_err(StoreError::database)?;
        Ok(IssueOutcome::Issued { expires_at_unix })
    }

    /// A live invitation, or `None` when it is unknown, expired, or used. Read-only.
    pub(crate) async fn invitation(
        &self,
        code: &CodeDigest,
    ) -> Result<Option<Invitation>, StoreError> {
        // @tenant-cross-scope: an invitation link names its tenant only through its digest.
        let row = sqlx::query(&format!(
            "SELECT kind, tenant_id, region, email_digest, email_nonce, email_ciphertext, \
                    principal_id, expires_at_unix \
               FROM auth_invitation WHERE code_digest = $1 AND expires_at_unix > {DB_NOW}"
        ))
        .bind(code.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::database)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let nonce: Vec<u8> = row.get("email_nonce");
        Ok(Some(Invitation {
            kind: InvitationKind::parse(row.get("kind"))?,
            tenant: row.get("tenant_id"),
            region: row.get("region"),
            email_digest: digest(row.get("email_digest"))?,
            email: SealedEmail {
                nonce: nonce
                    .try_into()
                    .map_err(|_| StoreError::corrupt("email nonce"))?,
                ciphertext: row.get("email_ciphertext"),
            },
            principal_id: row.get("principal_id"),
            expires_at_unix: row.get("expires_at_unix"),
        }))
    }

    /// Consumes a join invitation and creates the person with their password in one transaction,
    /// withdrawing any mail still carrying the link. If the email already has an account nothing
    /// changes and the invitation stays unused.
    pub(crate) async fn accept_join(
        &self,
        code: &CodeDigest,
        invitation: &Invitation,
        principal: DurablePrincipalRow,
        password_phc: String,
    ) -> Result<AcceptOutcome, StoreError> {
        let code = *code;
        let tenant = invitation.tenant.clone();
        let region = invitation.region.clone();
        let email_digest = invitation.email_digest;
        #[cfg(any(test, feature = "test-support"))]
        let checkpoint = self.checkpoint.clone();
        settle(
            with_tenant_tx_error(
                &self.pool,
                &invitation.tenant,
                &invitation.region,
                move |conn| {
                    Box::pin(async move {
                        // Issuing takes the same lock, so a member's link for this address cannot
                        // be decided while the account is being made.
                        lock_email(conn, &tenant, &region, &email_digest).await?;
                        let consumed = sqlx::query(&format!(
                            "DELETE FROM auth_invitation \
                              WHERE code_digest = $1 AND kind = 'join' AND tenant_id = $2 \
                                AND region = $3 AND email_digest = $4 \
                                AND expires_at_unix > {DB_NOW}"
                        ))
                        .bind(code.as_slice())
                        .bind(&tenant)
                        .bind(&region)
                        .bind(email_digest.as_slice())
                        .execute(&mut *conn)
                        .await?
                        .rows_affected();
                        if consumed != 1 {
                            return Err(Abort::Gone);
                        }
                        if !DurablePrincipalBacking::insert_new_principal_in_tx(
                            conn, &tenant, &region, &principal,
                        )
                        .await?
                        {
                            return Err(Abort::Store(StoreError(
                                "a freshly minted principal id already exists".into(),
                            )));
                        }
                        let created = sqlx::query(&format!(
                            "INSERT INTO auth_password_account \
                               (email_digest, tenant_id, region, principal_id, password_phc, \
                                changed_at_unix) \
                             VALUES ($1, $2, $3, $4, $5, {DB_NOW}) \
                             ON CONFLICT (email_digest) DO NOTHING"
                        ))
                        .bind(email_digest.as_slice())
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&principal.principal_id)
                        .bind(&password_phc)
                        .execute(&mut *conn)
                        .await?
                        .rows_affected();
                        if created != 1 {
                            return Err(Abort::EmailTaken);
                        }
                        crate::mail::withdraw(conn, &[code.to_vec()]).await?;
                        #[cfg(any(test, feature = "test-support"))]
                        if let Some(hook) = &checkpoint {
                            hook(super::Checkpoint::JoinAccountStored);
                        }
                        Ok(principal.principal_id)
                    })
                },
            )
            .await,
        )
    }

    /// Consumes a reset invitation and replaces the password of the active account it names,
    /// ending the person's other reset links and withdrawing any mail still carrying them.
    /// `notice`, the letter telling the owner, is queued only if the password changes.
    pub(crate) async fn accept_reset(
        &self,
        code: &CodeDigest,
        invitation: &Invitation,
        password_phc: String,
        notice: Option<(MailOutbox, Letter)>,
    ) -> Result<AcceptOutcome, StoreError> {
        let Some(principal_id) = invitation.principal_id.clone() else {
            return Ok(AcceptOutcome::Gone);
        };
        let code = *code;
        let tenant = invitation.tenant.clone();
        let region = invitation.region.clone();
        let email_digest = invitation.email_digest;
        #[cfg(any(test, feature = "test-support"))]
        let checkpoint = self.checkpoint.clone();
        settle(
            with_tenant_tx_error(
                &self.pool,
                &invitation.tenant,
                &invitation.region,
                move |conn| {
                    Box::pin(async move {
                        // Taken before anything else, as issuing takes it: a link issued while
                        // this runs cannot outlive the new password, and two links accepted at
                        // once take their locks in one order.
                        lock_email(conn, &tenant, &region, &email_digest).await?;
                        let consumed = sqlx::query(&format!(
                            "DELETE FROM auth_invitation \
                              WHERE code_digest = $1 AND kind = 'reset' AND tenant_id = $2 \
                                AND region = $3 AND principal_id = $4 \
                                AND expires_at_unix > {DB_NOW}"
                        ))
                        .bind(code.as_slice())
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&principal_id)
                        .execute(&mut *conn)
                        .await?
                        .rows_affected();
                        if consumed != 1 {
                            return Err(Abort::Gone);
                        }
                        let active: Option<String> = sqlx::query_scalar(
                            "SELECT status FROM principal \
                              WHERE tenant_id = $1 AND region = $2 AND principal_id = $3 \
                              FOR SHARE",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&principal_id)
                        .fetch_optional(&mut *conn)
                        .await?;
                        if active.as_deref() != Some(ACTIVE_PRINCIPAL_STATUS_JSON) {
                            return Err(Abort::Gone);
                        }
                        let changed = sqlx::query(&format!(
                            "UPDATE auth_password_account \
                                SET password_phc = $1, changed_at_unix = {DB_NOW} \
                              WHERE email_digest = $2 AND tenant_id = $3 AND region = $4 \
                                AND principal_id = $5"
                        ))
                        .bind(&password_phc)
                        .bind(email_digest.as_slice())
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&principal_id)
                        .execute(&mut *conn)
                        .await?
                        .rows_affected();
                        if changed != 1 {
                            return Err(Abort::Gone);
                        }
                        #[cfg(any(test, feature = "test-support"))]
                        if let Some(hook) = &checkpoint {
                            hook(super::Checkpoint::ResetPasswordReplaced);
                        }
                        // The new password ends every other reset link the person holds, expired
                        // or not. An expired one may be held by another issue's cleanup; that
                        // cleanup is the last thing its transaction does, so this waits at most
                        // for that commit and cannot close a cycle.
                        let mut ended: Vec<Vec<u8>> = sqlx::query_scalar(
                            "DELETE FROM auth_invitation \
                              WHERE tenant_id = $1 AND region = $2 AND principal_id = $3 \
                                AND kind = 'reset' \
                             RETURNING code_digest",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&principal_id)
                        .fetch_all(&mut *conn)
                        .await?;
                        ended.push(code.to_vec());
                        crate::mail::withdraw(conn, &ended).await?;
                        #[cfg(any(test, feature = "test-support"))]
                        if let Some(hook) = &checkpoint {
                            hook(super::Checkpoint::ResetLinksEnded);
                        }
                        if let Some((outbox, letter)) = notice {
                            outbox.enqueue(conn, &letter, None).await?;
                        }
                        Ok(principal_id)
                    })
                },
            )
            .await,
        )
    }

    /// Whether the account still has exactly the password hash that was just verified and its
    /// person is still active. Sign-in asks this last, so a password replaced while the old one
    /// was being verified does not sign anyone in.
    pub(crate) async fn still_signs_in(&self, account: &AccountRow) -> Result<bool, StoreError> {
        let email_digest = account.email_digest;
        let tenant = account.tenant.clone();
        let region = account.region.clone();
        let principal_id = account.principal_id.clone();
        let password_phc = account.password_phc.clone();
        with_tenant_tx_error(&self.pool, &account.tenant, &account.region, move |conn| {
            Box::pin(async move {
                let found: Option<i32> = sqlx::query_scalar(
                    "SELECT 1 FROM auth_password_account account \
                       JOIN principal person USING (tenant_id, region, principal_id) \
                      WHERE account.email_digest = $1 AND account.tenant_id = $2 \
                        AND account.region = $3 AND account.principal_id = $4 \
                        AND account.password_phc = $5 AND person.status = $6",
                )
                .bind(email_digest.as_slice())
                .bind(&tenant)
                .bind(&region)
                .bind(&principal_id)
                .bind(&password_phc)
                .bind(ACTIVE_PRINCIPAL_STATUS_JSON)
                .fetch_optional(&mut *conn)
                .await
                .map_err(StoreError::database)?;
                Ok(found.is_some())
            })
        })
        .await
    }

    /// The tenant's human-facing name: its placement slug when it has one.
    pub(crate) async fn organization_name(&self, tenant: &str) -> Result<String, StoreError> {
        let slug: Option<String> =
            sqlx::query_scalar("SELECT slug FROM tenant_placement WHERE tenant_id = $1")
                .bind(tenant)
                .fetch_optional(&self.pool)
                .await
                .map_err(StoreError::database)?;
        Ok(slug
            .filter(|slug| !slug.trim().is_empty())
            .unwrap_or_else(|| tenant.to_string()))
    }
}

/// Serializes everything that issues or ends links for one email in one organization.
async fn lock_email(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    email: &EmailDigest,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "myelin.edge.invitation.v1:{tenant}:{region}:{}",
            hex(email)
        ))
        .execute(conn)
        .await?;
    Ok(())
}

async fn account_in<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    email: &EmailDigest,
) -> Result<Option<AccountRow>, StoreError> {
    // @tenant-cross-scope: sign-in finds the account's tenant from the email digest.
    let row = sqlx::query(
        "SELECT tenant_id, region, principal_id, password_phc \
           FROM auth_password_account WHERE email_digest = $1",
    )
    .bind(email.as_slice())
    .fetch_optional(executor)
    .await
    .map_err(StoreError::database)?;
    Ok(row.map(|row| AccountRow {
        email_digest: *email,
        tenant: row.get("tenant_id"),
        region: row.get("region"),
        principal_id: row.get("principal_id"),
        password_phc: row.get("password_phc"),
    }))
}

fn digest(bytes: Vec<u8>) -> Result<EmailDigest, StoreError> {
    bytes
        .try_into()
        .map_err(|_| StoreError::corrupt("email digest"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
