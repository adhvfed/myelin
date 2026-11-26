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
use super::roles::OrgRole;
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

/// Each person's role in their organization, and the role a join link grants. Existing people get
/// roles from their join order: the earliest active person in each organization is its owner,
/// everyone else a member. Pending join links grant member, except an operator's link into an
/// organization nobody has joined yet, which grants owner as an operator's link now does.
///
/// Join links also gain an id to be revoked by, and a copy of their email sealed under the
/// roster key; links stored before this have none, so their address cannot be listed.
const ORG_ROLES_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS auth_org_member (
    tenant_id      text   NOT NULL,
    region         text   NOT NULL,
    principal_id   text   NOT NULL,
    role           text   NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    joined_at_unix bigint NOT NULL,
    PRIMARY KEY (tenant_id, region, principal_id),
    FOREIGN KEY (tenant_id, region, principal_id)
        REFERENCES principal (tenant_id, region, principal_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS auth_org_member_role_idx ON auth_org_member (tenant_id, region, role);
REVOKE ALL ON TABLE auth_org_member FROM PUBLIC;
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE auth_org_member TO myelin_app;

INSERT INTO auth_org_member (tenant_id, region, principal_id, role, joined_at_unix)
SELECT tenant_id, region, principal_id,
       CASE WHEN row_number() OVER (PARTITION BY tenant_id, region
                                    ORDER BY active DESC, joined_at_unix, principal_id) = 1
            THEN 'owner' ELSE 'member' END,
       joined_at_unix
  FROM (SELECT account.tenant_id, account.region, account.principal_id,
               person.status = '"Active"' AS active,
               CASE WHEN account.principal_id ~ '^person-[0-9a-hjkmnp-tv-z]{26}$'
                    THEN (SELECT sum((strpos('0123456789abcdefghjkmnpqrstvwxyz',
                                             substr(account.principal_id, 8 + i, 1)) - 1)
                                     * power(32::numeric, 9 - i))
                            FROM generate_series(0, 9) AS i)::bigint / 1000
                    ELSE account.changed_at_unix END AS joined_at_unix
          FROM auth_password_account AS account
          JOIN principal AS person USING (tenant_id, region, principal_id)) AS people
ON CONFLICT DO NOTHING;

ALTER TABLE auth_org_member ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth_org_member FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON auth_org_member;
CREATE POLICY myelin_tenant_isolation ON auth_org_member
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));

ALTER TABLE auth_invitation ADD COLUMN IF NOT EXISTS role text
    CHECK (role IN ('owner', 'admin', 'member'));
UPDATE auth_invitation AS link
   SET role = CASE WHEN link.origin = 'operator' AND NOT EXISTS (
                         SELECT 1 FROM auth_password_account AS account
                          WHERE account.tenant_id = link.tenant_id
                            AND account.region = link.region)
                   THEN 'owner' ELSE 'member' END
 WHERE link.kind = 'join' AND link.role IS NULL;
ALTER TABLE auth_invitation DROP CONSTRAINT IF EXISTS auth_invitation_role_check;
ALTER TABLE auth_invitation ADD CONSTRAINT auth_invitation_role_check
    CHECK ((kind = 'join') = (role IS NOT NULL));
ALTER TABLE auth_invitation ADD COLUMN IF NOT EXISTS id text NOT NULL DEFAULT gen_random_uuid()::text;
CREATE UNIQUE INDEX IF NOT EXISTS auth_invitation_id_idx ON auth_invitation (id);
ALTER TABLE auth_invitation ADD COLUMN IF NOT EXISTS roster_nonce bytea
    CHECK (octet_length(roster_nonce) = 12);
ALTER TABLE auth_invitation ADD COLUMN IF NOT EXISTS roster_ciphertext bytea
    CHECK (octet_length(roster_ciphertext) <= 512);
"#;

/// The account tables, the mail outbox that carries their links, each link's origin, and roles.
pub fn account_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("edge_0003_password_accounts", ACCOUNT_DDL),
        crate::mail::outbox_migration(),
        Migration::plain("edge_0005_invitation_origin", INVITATION_ORIGIN_DDL),
        Migration::plain("edge_0006_org_roles", ORG_ROLES_DDL),
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
    /// The role a join link grants; `None` for a reset link.
    pub role: Option<OrgRole>,
    pub expires_at_unix: i64,
}

/// A link about to be stored. Its expiry is set by the database clock when it is stored.
pub(crate) struct NewInvitation {
    pub tenant: String,
    pub region: String,
    pub email_digest: EmailDigest,
    pub email: SealedEmail,
    /// The email sealed for the organization's roster; join links only.
    pub roster: Option<SealedEmail>,
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
pub(crate) enum IssueRule<'a> {
    /// A member's join link: refused when the email has an account in this organization, or when
    /// `inviter`'s role may not grant `role`.
    MemberJoin { inviter: &'a str, role: OrgRole },
    /// The operator's join link: refused when the email has an account anywhere in the cell.
    OperatorJoin { role: OrgRole },
    /// The operator's reset link for the account the email has in this organization.
    OperatorReset,
    /// A reset link someone asked for by email, for the account in this organization.
    RequestedReset,
}

impl IssueRule<'_> {
    fn kind(self) -> InvitationKind {
        match self {
            Self::MemberJoin { .. } | Self::OperatorJoin { .. } => InvitationKind::Join,
            Self::OperatorReset | Self::RequestedReset => InvitationKind::Reset,
        }
    }

    fn origin(self) -> &'static str {
        match self {
            Self::MemberJoin { .. } => "member",
            Self::OperatorJoin { .. } | Self::OperatorReset => "operator",
            Self::RequestedReset => "requested",
        }
    }

    pub(crate) fn role(self) -> Option<OrgRole> {
        match self {
            Self::MemberJoin { role, .. } | Self::OperatorJoin { role } => Some(role),
            Self::OperatorReset | Self::RequestedReset => None,
        }
    }

    /// The earlier links for the same email in the same organization this one replaces. A new
    /// join link replaces every join link. An operator's reset link replaces the operator's
    /// earlier ones, so a link sent the wrong way can be revoked by issuing another; a requested
    /// one replaces nothing, so nobody can void a reset link by asking for another.
    fn supersedes(self) -> Option<&'static str> {
        match self {
            Self::MemberJoin { .. } | Self::OperatorJoin { .. } => Some("kind = 'join'"),
            Self::OperatorReset => Some("kind = 'reset' AND origin = 'operator'"),
            Self::RequestedReset => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IssueOutcome {
    Issued {
        expires_at_unix: i64,
    },
    AccountExists,
    NoSuchAccount,
    /// The inviter is not, or is no longer, allowed to grant the link's role.
    NotAllowed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AcceptOutcome {
    /// `issued_at_unix` is when the session made for the person counts as issued.
    Accepted {
        principal_id: String,
        issued_at_unix: i64,
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

impl From<sqlx::Error> for StoreError {
    fn from(error: sqlx::Error) -> Self {
        Self::database(error)
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
    /// The password hash checked against is no longer the stored one.
    Replaced,
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

fn settle(result: Result<(String, i64), Abort>) -> Result<AcceptOutcome, StoreError> {
    match result {
        Ok((principal_id, issued_at_unix)) => Ok(AcceptOutcome::Accepted {
            principal_id,
            issued_at_unix,
        }),
        Err(Abort::Gone | Abort::Replaced) => Ok(AcceptOutcome::Gone),
        Err(Abort::EmailTaken) => Ok(AcceptOutcome::EmailTaken),
        Err(Abort::Store(error)) => Err(error),
    }
}

/// One hit taken in one window of a counter. Giving it back touches only that window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Slot {
    pub bucket: [u8; 32],
    window_ends_at_unix: i64,
    /// This hit was the window's last.
    pub filled: bool,
}

/// What a password change did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeOutcome {
    /// Replaced; sessions issued before `cutoff` are ended.
    Changed { cutoff: i64 },
    /// A reset or another change replaced the password after it was verified.
    ChangedElsewhere,
    /// The person is no longer active.
    Inactive,
}

/// An active person in an organization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersonRow {
    pub principal_id: String,
    pub role: OrgRole,
    pub joined_at_unix: i64,
}

/// A live join link. `roster` is `None` for a link stored before roster copies were kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingRow {
    pub id: String,
    pub role: OrgRole,
    pub email_digest: EmailDigest,
    pub roster: Option<SealedEmail>,
    pub expires_at_unix: i64,
}

/// The organization as one person sees it. `pending` is empty unless they manage people.
pub(crate) struct Roster {
    pub viewer: OrgRole,
    pub people: Vec<PersonRow>,
    pub pending: Vec<PendingRow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemberChange {
    Role(OrgRole),
    Remove,
}

/// What a change to the roster did, or why it did nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RosterOutcome {
    Done,
    NotFound,
    /// The actor's role does not allow it, or the actor is no longer an active member.
    NotAllowed,
    /// It would leave the organization without an active owner.
    LastOwner,
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
        let taken: Option<(i64, i64)> = sqlx::query_as(
            "INSERT INTO auth_rate_limit (bucket, window_ends_at_unix, hits) VALUES ($1, $2, 1) \
             ON CONFLICT (bucket) DO UPDATE SET \
               hits = CASE WHEN auth_rate_limit.window_ends_at_unix <= $3 THEN 1 \
                           ELSE auth_rate_limit.hits + 1 END, \
               window_ends_at_unix = CASE WHEN auth_rate_limit.window_ends_at_unix <= $3 \
                                          THEN EXCLUDED.window_ends_at_unix \
                                          ELSE auth_rate_limit.window_ends_at_unix END \
             WHERE auth_rate_limit.window_ends_at_unix <= $3 OR auth_rate_limit.hits < $4 \
             RETURNING window_ends_at_unix, hits",
        )
        .bind(bucket.as_slice())
        .bind(now_unix.saturating_add(window_secs))
        .bind(now_unix)
        .bind(i64::from(limit))
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::database)?;
        if let Some((window_ends_at_unix, hits)) = taken {
            return Ok(Reservation::Taken(Slot {
                bucket,
                window_ends_at_unix,
                filled: hits >= i64::from(limit),
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

    pub(crate) async fn clear_hits(&self, buckets: &[[u8; 32]]) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(StoreError::database)?;
        clear_hits_in(&mut conn, buckets)
            .await
            .map_err(StoreError::database)
    }

    /// Queues a letter that carries no link.
    pub(crate) async fn queue(
        &self,
        outbox: &MailOutbox,
        letter: &Letter,
    ) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(StoreError::database)?;
        outbox
            .enqueue(&mut conn, letter, None)
            .await
            .map_err(StoreError::database)
    }

    /// Decides whether the link may be issued and stores it, under the per-email lock that
    /// accepting a link also takes. It replaces the earlier links `IssueRule::supersedes` names,
    /// and withdraws any mail still carrying them. `mail` is queued in the same transaction.
    pub(crate) async fn issue(
        &self,
        code: &CodeDigest,
        rule: IssueRule<'_>,
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
        if let IssueRule::MemberJoin { inviter, role } = rule {
            // Before anything about the email, so a member who may not invite learns nothing.
            scope_to(&mut transaction, &link.tenant, &link.region)
                .await
                .map_err(StoreError::database)?;
            lock_org(
                &mut transaction,
                &link.tenant,
                &link.region,
                OrgLock::Shared,
            )
            .await
            .map_err(StoreError::database)?;
            let allowed = role_in(&mut transaction, &link.tenant, &link.region, inviter)
                .await
                .map_err(StoreError::database)?
                .is_some_and(|inviter| inviter.may_grant(role));
            if !allowed {
                return Ok(IssueOutcome::NotAllowed);
            }
        }
        let account = account_in(&mut *transaction, &link.email_digest).await?;
        let in_this_organization = account
            .as_ref()
            .filter(|account| account.tenant == link.tenant && account.region == link.region);
        let principal_id = match rule {
            IssueRule::OperatorJoin { .. } if account.is_some() => {
                return Ok(IssueOutcome::AccountExists);
            }
            IssueRule::MemberJoin { .. } if in_this_organization.is_some() => {
                return Ok(IssueOutcome::AccountExists);
            }
            IssueRule::OperatorJoin { .. } | IssueRule::MemberJoin { .. } => None,
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
        let roster = link.roster.as_ref();
        let expires_at_unix: i64 = sqlx::query_scalar(&format!(
            "INSERT INTO auth_invitation \
               (code_digest, kind, tenant_id, region, email_digest, email_nonce, \
                email_ciphertext, principal_id, created_at_unix, expires_at_unix, origin, role, \
                roster_nonce, roster_ciphertext) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, {DB_NOW}, {DB_NOW} + $9, $10, $11, $12, $13) \
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
        .bind(rule.role().map(OrgRole::as_str))
        .bind(roster.map(|sealed| sealed.nonce.to_vec()))
        .bind(roster.map(|sealed| sealed.ciphertext.clone()))
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
                    principal_id, role, expires_at_unix \
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
            role: row
                .get::<Option<String>, _>("role")
                .map(|role| OrgRole::parse(&role).ok_or_else(|| StoreError::corrupt("role")))
                .transpose()?,
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
        let Some(role) = invitation.role else {
            return Ok(AcceptOutcome::Gone);
        };
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
                        let created: Option<i64> = sqlx::query_scalar(&format!(
                            "INSERT INTO auth_password_account \
                               (email_digest, tenant_id, region, principal_id, password_phc, \
                                changed_at_unix) \
                             VALUES ($1, $2, $3, $4, $5, {DB_NOW}) \
                             ON CONFLICT (email_digest) DO NOTHING \
                             RETURNING changed_at_unix"
                        ))
                        .bind(email_digest.as_slice())
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&principal.principal_id)
                        .bind(&password_phc)
                        .fetch_optional(&mut *conn)
                        .await?;
                        let Some(created_at_unix) = created else {
                            return Err(Abort::EmailTaken);
                        };
                        sqlx::query(
                            "INSERT INTO auth_org_member \
                               (tenant_id, region, principal_id, role, joined_at_unix) \
                             VALUES ($1, $2, $3, $4, $5)",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&principal.principal_id)
                        .bind(role.as_str())
                        .bind(created_at_unix)
                        .execute(&mut *conn)
                        .await?;
                        crate::mail::withdraw(conn, &[code.to_vec()]).await?;
                        #[cfg(any(test, feature = "test-support"))]
                        if let Some(hook) = &checkpoint {
                            hook(super::Checkpoint::JoinAccountStored);
                        }
                        Ok((principal.principal_id, created_at_unix))
                    })
                },
            )
            .await,
        )
    }

    /// Consumes a reset invitation and replaces the password of the active account it names,
    /// ending the person's sessions and other reset links and withdrawing any mail still carrying
    /// them, and clears the `unlock` counters. `notice`, the letter telling the owner, is queued
    /// only if the password changes.
    pub(crate) async fn accept_reset(
        &self,
        code: &CodeDigest,
        invitation: &Invitation,
        password_phc: String,
        unlock: Vec<[u8; 32]>,
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
                        let account = AccountRow {
                            email_digest,
                            tenant: tenant.clone(),
                            region: region.clone(),
                            principal_id: principal_id.clone(),
                            password_phc: String::new(),
                        };
                        if locked_password(conn, &account, "FOR UPDATE")
                            .await?
                            .is_none()
                            || !lock_active_person(conn, &tenant, &region, &principal_id).await?
                        {
                            return Err(Abort::Gone);
                        }
                        if !set_password(
                            conn,
                            &tenant,
                            &region,
                            &principal_id,
                            &email_digest,
                            &password_phc,
                            None,
                        )
                        .await?
                        {
                            return Err(Abort::Gone);
                        }
                        #[cfg(any(test, feature = "test-support"))]
                        if let Some(hook) = &checkpoint {
                            hook(super::Checkpoint::ResetPasswordReplaced);
                        }
                        end_reset_links(conn, &tenant, &region, &principal_id, Some(&code)).await?;
                        #[cfg(any(test, feature = "test-support"))]
                        if let Some(hook) = &checkpoint {
                            hook(super::Checkpoint::ResetLinksEnded);
                        }
                        let cutoff = end_sessions(conn, &tenant, &region, &principal_id).await?;
                        #[cfg(any(test, feature = "test-support"))]
                        if let Some(hook) = &checkpoint {
                            hook(super::Checkpoint::SessionsEnded);
                        }
                        clear_hits_in(conn, &unlock).await?;
                        if let Some((outbox, letter)) = notice {
                            outbox.enqueue(conn, &letter, None).await?;
                        }
                        Ok((principal_id, cutoff))
                    })
                },
            )
            .await,
        )
    }

    /// When the session for a sign-in counts as issued, if the account still has exactly the
    /// password hash that was just verified and its person is still active; `None` otherwise.
    /// Sign-in asks this last, so a password replaced while the old one was being verified does
    /// not sign anyone in. It is the database's present moment, raised to the person's session
    /// cutoff. The rows are locked for share in the order a reset or change locks them for update
    /// (account, then person), so a change in flight is waited for and then the hash no longer
    /// matches, and the two can never deadlock.
    pub(crate) async fn still_signs_in(
        &self,
        account: &AccountRow,
    ) -> Result<Option<i64>, StoreError> {
        let account = account.clone();
        let (tenant, region) = (account.tenant.clone(), account.region.clone());
        with_tenant_tx_error(&self.pool, &tenant, &region, move |conn| {
            Box::pin(async move {
                let current = locked_password(conn, &account, "FOR SHARE")
                    .await
                    .map_err(StoreError::database)?;
                if current.as_deref() != Some(account.password_phc.as_str()) {
                    return Ok(None);
                }
                sqlx::query_scalar(&format!(
                    "SELECT GREATEST({DB_NOW}, COALESCE(sessions_not_before, 0)) FROM principal \
                      WHERE tenant_id = $1 AND region = $2 AND principal_id = $3 AND status = $4 \
                      FOR SHARE"
                ))
                .bind(&account.tenant)
                .bind(&account.region)
                .bind(&account.principal_id)
                .bind(ACTIVE_PRINCIPAL_STATUS_JSON)
                .fetch_optional(&mut *conn)
                .await
                .map_err(StoreError::database)
            })
        })
        .await
    }

    /// The password account of a signed-in person, if they have one.
    pub(crate) async fn account_of(
        &self,
        tenant: &str,
        region: &str,
        principal_id: &str,
    ) -> Result<Option<AccountRow>, StoreError> {
        // @tenant-cross-scope: the account table is keyed by email digest, not tenant-scoped; the
        // lookup names the verified person's tenant itself.
        let row = sqlx::query(
            "SELECT email_digest, password_phc FROM auth_password_account \
              WHERE tenant_id = $1 AND region = $2 AND principal_id = $3",
        )
        .bind(tenant)
        .bind(region)
        .bind(principal_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::database)?;
        row.map(|row| {
            Ok(AccountRow {
                email_digest: digest(row.get("email_digest"))?,
                tenant: tenant.to_string(),
                region: region.to_string(),
                principal_id: principal_id.to_string(),
                password_phc: row.get("password_phc"),
            })
        })
        .transpose()
    }

    /// Replaces a signed-in person's password, if `account`'s hash (the one just verified) is
    /// still the stored one and the person is still active. Ends their sessions and reset links
    /// and queues `notice`.
    pub(crate) async fn change_password(
        &self,
        account: &AccountRow,
        password_phc: String,
        notice: Option<(MailOutbox, Letter)>,
    ) -> Result<ChangeOutcome, StoreError> {
        let (tenant, region) = (account.tenant.clone(), account.region.clone());
        let account = account.clone();
        #[cfg(any(test, feature = "test-support"))]
        let checkpoint = self.checkpoint.clone();
        let outcome = with_tenant_tx_error(&self.pool, &tenant, &region, move |conn| {
            Box::pin(async move {
                let (tenant, region) = (&account.tenant, &account.region);
                let principal_id = &account.principal_id;
                lock_email(conn, tenant, region, &account.email_digest).await?;
                let current = locked_password(conn, &account, "FOR UPDATE").await?;
                if !lock_active_person(conn, tenant, region, principal_id).await? {
                    return Err(Abort::Gone);
                }
                if current.as_deref() != Some(account.password_phc.as_str()) {
                    return Err(Abort::Replaced);
                }
                #[cfg(any(test, feature = "test-support"))]
                if let Some(hook) = &checkpoint {
                    hook(super::Checkpoint::PersonLocked);
                }
                if !set_password(
                    conn,
                    tenant,
                    region,
                    principal_id,
                    &account.email_digest,
                    &password_phc,
                    Some(&account.password_phc),
                )
                .await?
                {
                    return Err(Abort::Replaced);
                }
                end_reset_links(conn, tenant, region, principal_id, None).await?;
                let cutoff = end_sessions(conn, tenant, region, principal_id).await?;
                #[cfg(any(test, feature = "test-support"))]
                if let Some(hook) = &checkpoint {
                    hook(super::Checkpoint::SessionsEnded);
                }
                if let Some((outbox, letter)) = notice {
                    outbox.enqueue(conn, &letter, None).await?;
                }
                Ok(cutoff)
            })
        })
        .await;
        match outcome {
            Ok(cutoff) => Ok(ChangeOutcome::Changed { cutoff }),
            Err(Abort::Replaced) => Ok(ChangeOutcome::ChangedElsewhere),
            Err(Abort::Gone | Abort::EmailTaken) => Ok(ChangeOutcome::Inactive),
            Err(Abort::Store(error)) => Err(error),
        }
    }

    /// The person's role, if they are an active member of the organization.
    pub(crate) async fn role_of(
        &self,
        tenant: &str,
        region: &str,
        principal_id: &str,
    ) -> Result<Option<OrgRole>, StoreError> {
        let principal_id = principal_id.to_string();
        let (tenant_owned, region_owned) = (tenant.to_string(), region.to_string());
        with_tenant_tx_error(&self.pool, tenant, region, move |conn| {
            Box::pin(async move {
                role_in(conn, &tenant_owned, &region_owned, &principal_id)
                    .await
                    .map_err(StoreError::database)
            })
        })
        .await
    }

    /// The organization's active people and, for a viewer who manages people, its live join
    /// links. `None` when the viewer is not an active member.
    pub(crate) async fn roster(
        &self,
        tenant: &str,
        region: &str,
        viewer: &str,
    ) -> Result<Option<Roster>, StoreError> {
        let viewer = viewer.to_string();
        let (tenant_owned, region_owned) = (tenant.to_string(), region.to_string());
        with_tenant_tx_error(&self.pool, tenant, region, move |conn| {
            Box::pin(async move {
                let (tenant, region) = (&tenant_owned, &region_owned);
                let Some(role) = role_in(conn, tenant, region, &viewer)
                    .await
                    .map_err(StoreError::database)?
                else {
                    return Ok(None);
                };
                let people = sqlx::query(
                    "SELECT member.principal_id, member.role, member.joined_at_unix \
                       FROM auth_org_member AS member \
                       JOIN principal AS person USING (tenant_id, region, principal_id) \
                      WHERE member.tenant_id = $1 AND member.region = $2 AND person.status = $3 \
                      ORDER BY member.joined_at_unix, member.principal_id",
                )
                .bind(tenant)
                .bind(region)
                .bind(ACTIVE_PRINCIPAL_STATUS_JSON)
                .fetch_all(&mut *conn)
                .await
                .map_err(StoreError::database)?
                .into_iter()
                .map(|row| {
                    Ok(PersonRow {
                        principal_id: row.get("principal_id"),
                        role: stored_role(row.get("role"))?,
                        joined_at_unix: row.get("joined_at_unix"),
                    })
                })
                .collect::<Result<Vec<_>, StoreError>>()?;
                let pending = if role.manages_people() {
                    pending_in(conn, tenant, region).await?
                } else {
                    Vec::new()
                };
                Ok(Some(Roster {
                    viewer: role,
                    people,
                    pending,
                }))
            })
        })
        .await
    }

    /// Changes a person's role or removes them, as `actor` (or as the operator, with `None`).
    /// The organization is locked for the whole decision, so two changes cannot both count the
    /// same owner as the one left behind. Removing a person disables them, which ends their
    /// sessions.
    pub(crate) async fn change_member(
        &self,
        tenant: &str,
        region: &str,
        actor: Option<&str>,
        target: &str,
        change: MemberChange,
    ) -> Result<RosterOutcome, StoreError> {
        let actor = actor.map(str::to_string);
        let target = target.to_string();
        let (tenant_owned, region_owned) = (tenant.to_string(), region.to_string());
        #[cfg(any(test, feature = "test-support"))]
        let checkpoint = self.checkpoint.clone();
        with_tenant_tx_error(&self.pool, tenant, region, move |conn| {
            Box::pin(async move {
                let (tenant, region) = (&tenant_owned, &region_owned);
                lock_org(conn, tenant, region, OrgLock::Exclusive).await?;
                let actor_role = match &actor {
                    Some(actor) => match role_in(conn, tenant, region, actor).await? {
                        Some(role) if role.manages_people() => role,
                        _ => return Ok(RosterOutcome::NotAllowed),
                    },
                    None => OrgRole::Owner,
                };
                let Some(target_role) = role_in(conn, tenant, region, &target).await? else {
                    return Ok(RosterOutcome::NotFound);
                };
                let is_self = actor.as_deref() == Some(target.as_str());
                let allowed = actor_role.may_act_on(target_role, is_self)
                    && match change {
                        MemberChange::Role(role) => actor_role.may_grant(role),
                        MemberChange::Remove => true,
                    };
                if !allowed {
                    return Ok(RosterOutcome::NotAllowed);
                }
                let keeps_owner = change == MemberChange::Role(OrgRole::Owner);
                if target_role == OrgRole::Owner
                    && !keeps_owner
                    && active_owners(conn, tenant, region).await? <= 1
                {
                    return Ok(RosterOutcome::LastOwner);
                }
                #[cfg(any(test, feature = "test-support"))]
                if let Some(hook) = &checkpoint {
                    hook(super::Checkpoint::RolesRead);
                }
                match change {
                    MemberChange::Role(role) => {
                        sqlx::query(
                            "UPDATE auth_org_member SET role = $4 \
                              WHERE tenant_id = $1 AND region = $2 AND principal_id = $3",
                        )
                        .bind(tenant)
                        .bind(region)
                        .bind(&target)
                        .bind(role.as_str())
                        .execute(&mut *conn)
                        .await?;
                    }
                    MemberChange::Remove => {
                        sqlx::query(
                            "DELETE FROM auth_org_member \
                              WHERE tenant_id = $1 AND region = $2 AND principal_id = $3",
                        )
                        .bind(tenant)
                        .bind(region)
                        .bind(&target)
                        .execute(&mut *conn)
                        .await?;
                        sqlx::query(
                            "UPDATE principal SET status = '\"Disabled\"' \
                              WHERE tenant_id = $1 AND region = $2 AND principal_id = $3",
                        )
                        .bind(tenant)
                        .bind(region)
                        .bind(&target)
                        .execute(&mut *conn)
                        .await?;
                    }
                }
                Ok(RosterOutcome::Done)
            })
        })
        .await
    }

    /// The person an email belongs to in this organization, for the operator.
    pub(crate) async fn principal_by_email(
        &self,
        tenant: &str,
        region: &str,
        email: &EmailDigest,
    ) -> Result<Option<String>, StoreError> {
        Ok(account_in(&self.pool, email)
            .await?
            .filter(|account| account.tenant == tenant && account.region == region)
            .map(|account| account.principal_id))
    }

    /// Withdraws a live join link and the mail still carrying it, as `actor`.
    pub(crate) async fn revoke_invitation(
        &self,
        tenant: &str,
        region: &str,
        actor: &str,
        id: &str,
    ) -> Result<RosterOutcome, StoreError> {
        let (actor, id) = (actor.to_string(), id.to_string());
        let (tenant_owned, region_owned) = (tenant.to_string(), region.to_string());
        with_tenant_tx_error(&self.pool, tenant, region, move |conn| {
            Box::pin(async move {
                let (tenant, region) = (&tenant_owned, &region_owned);
                lock_org(conn, tenant, region, OrgLock::Shared).await?;
                if !role_in(conn, tenant, region, &actor)
                    .await?
                    .is_some_and(OrgRole::manages_people)
                {
                    return Ok(RosterOutcome::NotAllowed);
                }
                let revoked: Vec<Vec<u8>> = sqlx::query_scalar(
                    "DELETE FROM auth_invitation \
                      WHERE id = $1 AND tenant_id = $2 AND region = $3 AND kind = 'join' \
                     RETURNING code_digest",
                )
                .bind(&id)
                .bind(tenant)
                .bind(region)
                .fetch_all(&mut *conn)
                .await?;
                if revoked.is_empty() {
                    return Ok(RosterOutcome::NotFound);
                }
                crate::mail::withdraw(conn, &revoked).await?;
                Ok(RosterOutcome::Done)
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

/// The account's stored password hash, with its row locked (`lock` is `FOR SHARE` or
/// `FOR UPDATE`). Every transaction that locks both the account and its person locks the account
/// first, so none can wait on another in a cycle.
async fn locked_password(
    conn: &mut sqlx::PgConnection,
    account: &AccountRow,
    lock: &'static str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "SELECT password_phc FROM auth_password_account \
          WHERE email_digest = $1 AND tenant_id = $2 AND region = $3 AND principal_id = $4 \
          {lock}"
    ))
    .bind(account.email_digest.as_slice())
    .bind(&account.tenant)
    .bind(&account.region)
    .bind(&account.principal_id)
    .fetch_optional(&mut *conn)
    .await
}

/// Whether the person is active, with their row locked for update: the transactions that ask
/// go on to move the person's session cutoff, and locking for share first would let two of them
/// (or one and any other writer) each wait to upgrade a lock the other holds.
async fn lock_active_person(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    principal_id: &str,
) -> Result<bool, sqlx::Error> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM principal \
          WHERE tenant_id = $1 AND region = $2 AND principal_id = $3 \
          FOR UPDATE",
    )
    .bind(tenant)
    .bind(region)
    .bind(principal_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(status.as_deref() == Some(ACTIVE_PRINCIPAL_STATUS_JSON))
}

/// Stores the new password hash; with `replacing`, only while that is still the stored one.
async fn set_password(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    principal_id: &str,
    email_digest: &EmailDigest,
    password_phc: &str,
    replacing: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let changed = sqlx::query(&format!(
        "UPDATE auth_password_account \
            SET password_phc = $1, changed_at_unix = {DB_NOW} \
          WHERE email_digest = $2 AND tenant_id = $3 AND region = $4 AND principal_id = $5 \
            AND ($6::text IS NULL OR password_phc = $6)"
    ))
    .bind(password_phc)
    .bind(email_digest.as_slice())
    .bind(tenant)
    .bind(region)
    .bind(principal_id)
    .bind(replacing)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    Ok(changed == 1)
}

/// A new password ends every reset link the person holds, expired or not, and withdraws the
/// mail still carrying them. An expired one may be held by another issue's cleanup; that
/// cleanup is the last thing its transaction does, so this waits at most for that commit and
/// cannot close a cycle.
async fn end_reset_links(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    principal_id: &str,
    used: Option<&CodeDigest>,
) -> Result<(), sqlx::Error> {
    let mut ended: Vec<Vec<u8>> = sqlx::query_scalar(
        "DELETE FROM auth_invitation \
          WHERE tenant_id = $1 AND region = $2 AND principal_id = $3 AND kind = 'reset' \
         RETURNING code_digest",
    )
    .bind(tenant)
    .bind(region)
    .bind(principal_id)
    .fetch_all(&mut *conn)
    .await?;
    ended.extend(used.map(|code| code.to_vec()));
    crate::mail::withdraw(conn, &ended).await
}

/// Ends the person's sessions and returns the new cutoff, which the session made for them after
/// the change counts as issued at.
async fn end_sessions(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    principal_id: &str,
) -> Result<i64, Abort> {
    DurablePrincipalBacking::end_sessions_in_tx(conn, tenant, region, principal_id)
        .await?
        .ok_or(Abort::Gone)
}

async fn clear_hits_in(
    conn: &mut sqlx::PgConnection,
    buckets: &[[u8; 32]],
) -> Result<(), sqlx::Error> {
    let buckets: Vec<&[u8]> = buckets.iter().map(|bucket| bucket.as_slice()).collect();
    // @tenant-cross-scope: counters are opaque and keyed by digest.
    sqlx::query("DELETE FROM auth_rate_limit WHERE bucket = ANY($1)")
        .bind(buckets)
        .execute(conn)
        .await?;
    Ok(())
}

/// Scopes the rest of a plain transaction to one organization, for the row-level security of the
/// tables it goes on to read.
async fn scope_to(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
    .bind(tenant)
    .bind(region)
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Clone, Copy)]
enum OrgLock {
    /// Acting on the strength of a role: inviting, revoking.
    Shared,
    /// Changing roles or removing people.
    Exclusive,
}

/// Serializes changes to who holds which role in one organization against each other and against
/// everything that acts on a role. Taken after any email lock, never before one.
async fn lock_org(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    lock: OrgLock,
) -> Result<(), sqlx::Error> {
    let function = match lock {
        OrgLock::Shared => "pg_advisory_xact_lock_shared",
        OrgLock::Exclusive => "pg_advisory_xact_lock",
    };
    sqlx::query(&format!("SELECT {function}(hashtextextended($1, 0))"))
        .bind(format!("myelin.edge.org_roles.v1:{tenant}:{region}"))
        .execute(conn)
        .await?;
    Ok(())
}

/// The role of an active member; `None` for anyone else.
async fn role_in(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    principal_id: &str,
) -> Result<Option<OrgRole>, sqlx::Error> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT member.role FROM auth_org_member AS member \
           JOIN principal AS person USING (tenant_id, region, principal_id) \
          WHERE member.tenant_id = $1 AND member.region = $2 AND member.principal_id = $3 \
            AND person.status = $4",
    )
    .bind(tenant)
    .bind(region)
    .bind(principal_id)
    .bind(ACTIVE_PRINCIPAL_STATUS_JSON)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(role.as_deref().and_then(OrgRole::parse))
}

async fn active_owners(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM auth_org_member AS member \
           JOIN principal AS person USING (tenant_id, region, principal_id) \
          WHERE member.tenant_id = $1 AND member.region = $2 AND member.role = 'owner' \
            AND person.status = $3",
    )
    .bind(tenant)
    .bind(region)
    .bind(ACTIVE_PRINCIPAL_STATUS_JSON)
    .fetch_one(&mut *conn)
    .await
}

async fn pending_in(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
) -> Result<Vec<PendingRow>, StoreError> {
    sqlx::query(&format!(
        "SELECT id, role, email_digest, roster_nonce, roster_ciphertext, expires_at_unix \
           FROM auth_invitation \
          WHERE tenant_id = $1 AND region = $2 AND kind = 'join' AND expires_at_unix > {DB_NOW} \
          ORDER BY created_at_unix, id"
    ))
    .bind(tenant)
    .bind(region)
    .fetch_all(&mut *conn)
    .await
    .map_err(StoreError::database)?
    .into_iter()
    .map(|row| {
        let nonce: Option<Vec<u8>> = row.get("roster_nonce");
        let ciphertext: Option<Vec<u8>> = row.get("roster_ciphertext");
        let roster = match (nonce, ciphertext) {
            (Some(nonce), Some(ciphertext)) => Some(SealedEmail {
                nonce: nonce
                    .try_into()
                    .map_err(|_| StoreError::corrupt("roster nonce"))?,
                ciphertext,
            }),
            _ => None,
        };
        Ok(PendingRow {
            id: row.get("id"),
            role: stored_role(row.get("role"))?,
            email_digest: digest(row.get("email_digest"))?,
            roster,
            expires_at_unix: row.get("expires_at_unix"),
        })
    })
    .collect()
}

fn stored_role(value: String) -> Result<OrgRole, StoreError> {
    OrgRole::parse(&value).ok_or_else(|| StoreError::corrupt("role"))
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
