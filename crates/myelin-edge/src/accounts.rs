//! Email-and-password accounts. A person gets an account only by accepting an invitation link;
//! after that they sign in with their email and password and receive an ordinary human session.

mod credentials;
pub(crate) mod http;
mod roles;
mod store;

use myelin_events::clock::{system_clock_reading, ClockError};
use myelin_events::{IdMinter, UlidMinter};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::{PrincipalProfile, PrincipalStore};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};
use sqlx::PgPool;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;

use crate::mail::{Letter, MailOutbox};

pub use credentials::AccountKeys;
use credentials::{
    DeviceId, DeviceKey, DummyHash, Email, EmailDigest, EmailDigester, InvitationCode, Password,
    RosterKey, WeakPassword,
};
pub use roles::OrgRole;
pub use store::account_migrations;
pub(crate) use store::InvitationKind;
use store::{
    AcceptOutcome, AccountRow, AccountStore, ChangeOutcome, Invitation, IssueOutcome, IssueRule,
    LinkLetter, LinkMail, MemberChange, NewInvitation, Reservation, RosterOutcome, Slot, Standing,
};

pub const JOIN_INVITATION_TTL_SECS: i64 = 7 * 24 * 60 * 60;
pub const RESET_INVITATION_TTL_SECS: i64 = 24 * 60 * 60;
const MAX_INVITATION_TTL_SECS: i64 = 30 * 24 * 60 * 60;
const MAX_DISPLAY_NAME_CHARS: usize = 128;

/// Fixed-window limits, counted in the database so every Edge instance shares them.
struct Limit {
    name: &'static str,
    hits: u32,
    window_secs: i64,
}

/// Sign-in attempts per email from browsers without a device token for it, wherever they come
/// from, so spreading guesses over many sources does not help. Filling it turns away only those
/// browsers: one that has signed in to the email before is charged to its own allowance below.
const LOGIN_FAILURES_PER_EMAIL: Limit = Limit {
    name: "login.email",
    hits: 10,
    window_secs: 15 * 60,
};
/// Sign-in attempts per email from one browser holding a device token for it.
const LOGIN_FAILURES_PER_DEVICE: Limit = Limit {
    name: "login.device",
    hits: 10,
    window_secs: 15 * 60,
};
/// Emails telling a person that strangers' sign-ins to their account are being turned away: one
/// per lock episode. An episode lasts up to a day and ends early when the owner signs in,
/// changes their password or accepts a reset.
const LOCK_NOTICES_PER_EMAIL: Limit = Limit {
    name: "login.lock_notice",
    hits: 1,
    window_secs: 24 * 60 * 60,
};
/// Lock notices across every Edge sharing the database, so guessing at many emails cannot turn
/// the outbox into a mail cannon. Past it a notice is dropped.
const LOCK_NOTICES_PER_CELL: Limit = Limit {
    name: "login.lock_notice.cell",
    hits: 60,
    window_secs: 60,
};
const INVITES_PER_MEMBER: Limit = Limit {
    name: "invitation.create",
    hits: 50,
    window_secs: 60 * 60,
};
/// Invitations to one address from one organization, however many members send them, so
/// members cannot flood a stranger's inbox.
const INVITES_PER_RECIPIENT: Limit = Limit {
    name: "invitation.recipient",
    hits: 3,
    window_secs: 60 * 60,
};
/// Emailed reset links per address, so the form cannot be used to flood someone's inbox.
/// Requests past it are dropped without a word.
const RESET_REQUESTS_PER_EMAIL: Limit = Limit {
    name: "password.reset_request",
    hits: 3,
    window_secs: 60 * 60,
};
/// Reset requests being worked on at once. Past it a request is dropped, as when throttled.
const RESET_REQUESTS_IN_FLIGHT: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountError {
    InvalidEmail,
    InvalidDisplayName,
    /// A reset link changes only the password; it does not take a name.
    UnexpectedDisplayName,
    LoginFailed,
    /// The current password given to change it is not the person's password.
    WrongPassword,
    /// The person signs in another way and has no password to change.
    NoPassword,
    /// A reset or another change replaced the password while this change was being made.
    PasswordChangedElsewhere,
    /// The session asking was ended (its person was disabled) while it asked.
    SessionEnded,
    /// Unknown, expired, used, or superseded invitation; deliberately one answer.
    InvitationNotFound,
    WeakPassword,
    AccountExists,
    NoSuchAccount,
    RateLimited {
        retry_after_secs: u64,
    },
    /// This address has had as many invitations from the organization as it may for now.
    RecipientRateLimited {
        retry_after_secs: u64,
    },
    Busy,
    LinksUnavailable,
    /// The person's role in the organization does not allow this.
    RoleRequired,
    /// It would leave the organization without an owner.
    LastOwner,
    /// No active person in the organization has this id.
    NoSuchMember,
    /// A pending invitation for the address grants a role the inviter could not grant.
    InvitationExists,
    /// The address belongs to a person who was erased, who cannot join again.
    PersonErased,
    Unavailable(String),
}

impl core::fmt::Display for AccountError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidEmail => formatter.write_str("the email address is not valid"),
            Self::InvalidDisplayName => formatter.write_str(
                "the display name must be 1 to 128 characters without control characters",
            ),
            Self::UnexpectedDisplayName => {
                formatter.write_str("a password reset takes only a password")
            }
            Self::LoginFailed => formatter.write_str("login failed"),
            Self::WrongPassword => formatter.write_str("the current password is not right"),
            Self::NoPassword => formatter.write_str("this account signs in without a password"),
            Self::PasswordChangedElsewhere => {
                formatter.write_str("your password was just changed elsewhere")
            }
            Self::SessionEnded => formatter.write_str("this session has ended"),
            Self::InvitationNotFound => formatter.write_str("invitation not found"),
            Self::WeakPassword => {
                formatter.write_str("the password must be between 10 and 1024 characters long")
            }
            Self::AccountExists => {
                formatter.write_str("an account with this email address already exists")
            }
            Self::NoSuchAccount => formatter.write_str("no account has this email address"),
            Self::RateLimited { retry_after_secs } => {
                write!(formatter, "too many attempts; retry in {retry_after_secs}s")
            }
            Self::RecipientRateLimited { retry_after_secs } => write!(
                formatter,
                "this address was invited too often; retry in {retry_after_secs}s"
            ),
            Self::Busy => formatter.write_str("sign-in is busy; retry shortly"),
            Self::LinksUnavailable => formatter.write_str(
                "invitation links need MYELIN_WEB_PUBLIC_URL set to the web app's https origin",
            ),
            Self::RoleRequired => {
                formatter.write_str("your role in this organization does not allow this")
            }
            Self::LastOwner => formatter.write_str("an organization must keep at least one owner"),
            Self::NoSuchMember => formatter.write_str("no such person in this organization"),
            Self::InvitationExists => formatter.write_str(
                "this address already has a pending invitation that only an owner can replace",
            ),
            Self::PersonErased => formatter
                .write_str("this address belonged to a person who was erased and cannot rejoin"),
            Self::Unavailable(detail) => write!(formatter, "accounts are unavailable: {detail}"),
        }
    }
}

impl std::error::Error for AccountError {}

impl From<store::StoreError> for AccountError {
    fn from(error: store::StoreError) -> Self {
        Self::Unavailable(error.to_string())
    }
}

impl From<WeakPassword> for AccountError {
    fn from(_: WeakPassword) -> Self {
        Self::WeakPassword
    }
}

/// What an invitation link shows before it is accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvitationView {
    pub kind: &'static str,
    pub email: String,
    pub organization: String,
    pub expires_at_unix: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuedInvitation {
    pub url: String,
    pub email: String,
    pub expires_at_unix: i64,
    /// The role a join link grants; `None` for a reset link.
    pub role: Option<OrgRole>,
}

/// An active person, as the organization's roster shows them. A profile that can no longer be
/// read (erased) shows empty strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberView {
    pub principal_id: String,
    pub display_name: String,
    pub email: String,
    pub role: OrgRole,
    pub joined_at_unix: i64,
}

/// A live join link. `email` is `None` for a link made before addresses were kept for the roster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingInvitation {
    pub id: String,
    pub email: Option<String>,
    pub role: OrgRole,
    pub expires_at_unix: i64,
}

/// The organization as one member sees it: pending invitations only for owners and admins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RosterView {
    pub viewer: OrgRole,
    pub people: Vec<MemberView>,
    pub invitations: Option<Vec<PendingInvitation>>,
    /// Active owners; zero only when the last was erased or revoked outside the roster.
    pub owners: i64,
}

/// Whom `edge role` names.
#[derive(Clone, Copy, Debug)]
pub enum OperatorTarget<'a> {
    Email(&'a str),
    Principal(&'a str),
}

/// A member's invitation: emailed to the invitee when mail is on, otherwise handed to the
/// inviter to pass on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberInvitation {
    Emailed { email: String, expires_at_unix: i64 },
    HandedOver(IssuedInvitation),
}

/// A person whose password was just confirmed, and the moment the session made for them counts
/// as issued: by the database clock, and never before the person's session cutoff.
/// `device_token` lets this browser's later sign-ins to the email skip strangers' allowance.
#[derive(Clone, PartialEq, Eq)]
pub struct SignedIn {
    pub principal: Principal,
    pub issued_at_unix: i64,
    pub device_token: String,
}

impl core::fmt::Debug for SignedIn {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SignedIn")
            .field("principal", &self.principal)
            .field("issued_at_unix", &self.issued_at_unix)
            .field("device_token", &"<redacted>")
            .finish()
    }
}

/// The allowance an attempt for one email is charged to: its device's, when it brought a good
/// device token, otherwise the one every other browser shares.
struct Charge {
    limit: &'static Limit,
    bucket: [u8; 32],
    device: Option<DeviceId>,
}

/// What the person sends to accept a link. A join needs their name; a reset does not.
pub struct Acceptance {
    pub display_name: Option<String>,
    pub password: String,
}

/// Where invitation links point. A link carries a bearer secret, so an origin that would send it
/// over plain http to another machine is refused when the service is built, not when it is used.
#[derive(Clone)]
enum LinkOrigin {
    Unset,
    Refused(String),
    Ready(String),
}

impl LinkOrigin {
    fn from_config(origin: Option<String>) -> Self {
        match origin.map(|origin| crate::validated_web_origin(&origin)) {
            None => Self::Unset,
            Some(Ok(origin)) => Self::Ready(origin),
            Some(Err(reason)) => Self::Refused(reason),
        }
    }
}

/// Points a test can stop at, to line a concurrent change up with an exact moment.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checkpoint {
    /// Sign-in has verified the password and not yet confirmed it is still current.
    PasswordVerified,
    /// A link is about to be issued; nothing about the email has been decided yet.
    BeforeIssuing,
    /// A join has made the new person's key and not yet stored the person.
    JoinKeyMade,
    /// Inside a join's transaction: the account is stored; nothing is committed yet.
    JoinAccountStored,
    /// Inside a reset's transaction: the password is replaced and the person's other reset links
    /// are not yet ended.
    ResetPasswordReplaced,
    /// Inside a reset's transaction: the other reset links are ended; nothing is committed yet.
    ResetLinksEnded,
    /// Inside a change's transaction: the account and person rows are locked; nothing is
    /// written yet.
    PersonLocked,
    /// Inside a reset's or change's transaction: the person's sessions are ended; nothing is
    /// committed yet.
    SessionsEnded,
    /// Inside a role change or removal: both roles are read and allow it; nothing is written yet.
    RolesRead,
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) type CheckpointHook = Arc<dyn Fn(Checkpoint) + Send + Sync>;

/// A link that was live when it was looked up. Consuming it checks again.
struct LiveLink {
    code: InvitationCode,
    invitation: Invitation,
    email: Email,
}

#[derive(Clone)]
pub struct AccountService {
    store: AccountStore,
    principals: PrincipalStore,
    digester: EmailDigester,
    devices: DeviceKey,
    roster: RosterKey,
    links: LinkOrigin,
    mail: Option<MailOutbox>,
    reset_requests: Arc<Semaphore>,
    dropped_resets: Arc<DroppedRequests>,
    lock_notice_cap: Arc<NoticeCap>,
    dropped_notices: Arc<DroppedRequests>,
    device_allowances_filled: Arc<AtomicU64>,
    region: String,
    runtime: Handle,
    hashing: Arc<HashGate>,
    dummy: Arc<DummyHash>,
    ids: Arc<UlidMinter>,
    now: Arc<dyn Fn() -> Result<i64, ClockError> + Send + Sync>,
    #[cfg(any(test, feature = "test-support"))]
    checkpoint: Option<CheckpointHook>,
}

impl AccountService {
    /// `web_origin` is the web app's origin; links are `{origin}/join/{code}`. Without a usable
    /// one, sign-in and accepting existing links still work, but no new link can be made.
    pub fn new(
        pool: PgPool,
        principals: PrincipalStore,
        keys: AccountKeys,
        region: impl Into<String>,
        web_origin: Option<String>,
        runtime: Handle,
    ) -> Self {
        Self {
            store: AccountStore::new(pool),
            principals,
            digester: keys.email,
            devices: keys.devices,
            roster: keys.roster,
            links: LinkOrigin::from_config(web_origin),
            mail: None,
            reset_requests: Arc::new(Semaphore::new(RESET_REQUESTS_IN_FLIGHT)),
            dropped_resets: Arc::default(),
            lock_notice_cap: Arc::new(NoticeCap {
                hits: LOCK_NOTICES_PER_CELL.hits,
                key: b"cell".to_vec(),
                #[cfg(any(test, feature = "test-support"))]
                fail_queue: false,
            }),
            dropped_notices: Arc::default(),
            device_allowances_filled: Arc::default(),
            region: region.into(),
            runtime,
            hashing: Arc::new(HashGate::sized_for_host()),
            dummy: Arc::new(DummyHash::generate()),
            ids: Arc::new(UlidMinter::new()),
            now: Arc::new(|| system_clock_reading().map(|reading| reading.unix_seconds())),
            #[cfg(any(test, feature = "test-support"))]
            checkpoint: None,
        }
    }

    /// Mail on: members' invitations and reset links are emailed, and a changed password is
    /// reported to its owner.
    pub fn with_mail(mut self, outbox: MailOutbox) -> Self {
        self.mail = Some(outbox);
        self
    }

    pub fn mail_enabled(&self) -> bool {
        self.mail.is_some()
    }

    /// The clock for rate-limit windows. Link expiry always follows the database clock.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_clock(mut self, now: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        self.now = Arc::new(move || Ok(now()));
        self
    }

    /// A cell-wide lock notice cap of `hits` a minute, counted apart from every other service's.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_lock_notice_cap(mut self, hits: u32) -> Self {
        self.lock_notice_cap = Arc::new(NoticeCap {
            hits,
            key: self.ids.mint().0.into_bytes(),
            fail_queue: self.lock_notice_cap.fail_queue,
        });
        self
    }

    /// Every lock notice fails to queue, as if the outbox were unreachable.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_failing_lock_notices(mut self) -> Self {
        self.lock_notice_cap = Arc::new(NoticeCap {
            hits: self.lock_notice_cap.hits,
            key: self.lock_notice_cap.key.clone(),
            fail_queue: true,
        });
        self
    }

    /// How many times a device's own allowance has filled since this service started.
    pub fn device_allowances_filled(&self) -> u64 {
        self.device_allowances_filled.load(Ordering::Relaxed)
    }

    /// A hash gate that admits `capacity` hashes at once; 0 refuses every hash as busy.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_hash_capacity(mut self, capacity: usize) -> Self {
        self.hashing = Arc::new(HashGate::with_capacity(capacity));
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_checkpoint(mut self, hook: impl Fn(Checkpoint) + Send + Sync + 'static) -> Self {
        let hook: CheckpointHook = Arc::new(hook);
        self.store = self.store.with_checkpoint(hook.clone());
        self.checkpoint = Some(hook);
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    fn reached(&self, checkpoint: Checkpoint) {
        if let Some(hook) = &self.checkpoint {
            hook(checkpoint);
        }
    }

    fn now_unix(&self) -> Result<i64, AccountError> {
        (self.now)().map_err(|error| AccountError::Unavailable(format!("clock: {error}")))
    }

    fn block<T>(
        &self,
        future: impl std::future::Future<Output = Result<T, AccountError>>,
    ) -> Result<T, AccountError> {
        crate::runtime::drive_result_on_runtime(
            &self.runtime,
            future,
            AccountError::Unavailable("accounts require the Edge multi-thread runtime".into()),
        )
    }

    fn bucket(limit: &Limit, key: &[u8]) -> [u8; 32] {
        let mut bucket = blake3::Hasher::new();
        bucket.update(b"myelin.edge.account.rate.v1\0");
        bucket.update(limit.name.as_bytes());
        bucket.update(b"\0");
        bucket.update(key);
        *bucket.finalize().as_bytes()
    }

    /// Takes one slot of `limit` for `bucket`, or fails with how long to wait. The slot is taken
    /// before the work it pays for, so a burst cannot all pass a check made ahead of the work.
    async fn reserve(&self, limit: &Limit, bucket: [u8; 32]) -> Result<Slot, AccountError> {
        match self
            .store
            .reserve_hit(bucket, limit.hits, limit.window_secs, self.now_unix()?)
            .await?
        {
            Reservation::Taken(slot) => Ok(slot),
            Reservation::Full { retry_after_secs } => {
                Err(AccountError::RateLimited { retry_after_secs })
            }
        }
    }

    /// Which allowance an attempt for `email` is charged to. A token that is malformed, forged,
    /// expired or for another email counts as no token.
    fn charge(
        &self,
        email: &EmailDigest,
        device_token: Option<&str>,
    ) -> Result<Charge, AccountError> {
        let now = self.now_unix()?;
        Ok(
            match device_token.and_then(|token| self.devices.verify(token, email, now)) {
                Some(device) => {
                    let mut key = email.to_vec();
                    key.extend_from_slice(&device);
                    Charge {
                        limit: &LOGIN_FAILURES_PER_DEVICE,
                        bucket: Self::bucket(&LOGIN_FAILURES_PER_DEVICE, &key),
                        device: Some(device),
                    }
                }
                None => Charge {
                    limit: &LOGIN_FAILURES_PER_EMAIL,
                    bucket: Self::bucket(&LOGIN_FAILURES_PER_EMAIL, email),
                    device: None,
                },
            },
        )
    }

    fn signed_in(
        &self,
        principal: Principal,
        issued_at_unix: i64,
        email: &EmailDigest,
        device: Option<DeviceId>,
    ) -> Result<SignedIn, AccountError> {
        Ok(SignedIn {
            principal,
            issued_at_unix,
            device_token: self.devices.issue(email, device, self.now_unix()?),
        })
    }

    /// Runs one password hash or verification under the host-wide concurrency cap.
    fn hashing<T>(&self, work: impl FnOnce() -> T) -> Result<T, AccountError> {
        let _permit = self.hashing.enter()?;
        Ok(work())
    }

    /// Signs a person in. Unknown email, wrong password, and an inactive person all fail the
    /// same way and cost the same password verification.
    ///
    /// Each attempt takes a slot in its allowance (see `charge`) before it verifies anything, so a
    /// burst of guesses gets at most the window's worth of verifications per allowance. A slot is
    /// only given back for an attempt that never verified (the hash gate was busy) or that
    /// succeeded.
    pub fn login(
        &self,
        email: &str,
        password: String,
        device_token: Option<&str>,
    ) -> Result<SignedIn, AccountError> {
        // A malformed email cannot belong to an account and is not counted, but it pays for a
        // verification like any other attempt so the answer reveals nothing.
        let email = Email::parse(email);
        let digest = email.as_ref().map(|email| self.digester.digest(email));
        let charge = digest
            .map(|digest| self.charge(&digest, device_token))
            .transpose()?;
        let (account, slot) = self.block(async {
            let (Some(digest), Some(charge)) = (&digest, &charge) else {
                return Ok((None, None));
            };
            let slot = self.reserve(charge.limit, charge.bucket).await?;
            Ok((self.store.account(digest).await?, Some(slot)))
        })?;
        let stored_phc = account
            .as_ref()
            .map(|account| account.password_phc.as_str());
        let verified = match self.hashing(|| self.verify(password, stored_phc)) {
            Ok(verified) => verified,
            Err(busy) => {
                return Err(match slot {
                    Some(slot) => self.refund(slot, busy),
                    None => busy,
                });
            }
        };
        #[cfg(any(test, feature = "test-support"))]
        self.reached(Checkpoint::PasswordVerified);
        let signed_in = match &account {
            Some(account) if verified => self.still_signs_in(account)?,
            _ => None,
        };
        match (signed_in, slot, charge, digest) {
            (Some((principal, issued_at)), Some(slot), Some(charge), Some(digest)) => {
                self.clear_after_success(slot, &digest)?;
                self.signed_in(principal, issued_at, &digest, charge.device)
            }
            // The failure was already counted when its slot was taken.
            (_, Some(slot), Some(charge), _) if slot.filled => {
                match (charge.device, email, account) {
                    (Some(_), _, _) => self.note_device_allowance_filled(),
                    (None, Some(email), Some(account)) => self.send_lock_notice(email, account),
                    (None, _, None) | (None, None, _) => {}
                }
                Err(AccountError::LoginFailed)
            }
            _ => Err(AccountError::LoginFailed),
        }
    }

    /// After the owner proves their password: the allowance the attempt was charged to is clear,
    /// and the lock episode is over, so the next lock warns them again.
    fn clear_after_success(&self, slot: Slot, email: &EmailDigest) -> Result<(), AccountError> {
        let episode = Self::bucket(&LOCK_NOTICES_PER_EMAIL, email);
        self.block(async { Ok(self.store.clear_hits(&[slot.bucket, episode]).await?) })
    }

    /// A device kept failing until its own allowance filled: a mistyping owner, or a stolen token
    /// being tried. Nobody is mailed; it is counted and logged, without the email or device.
    fn note_device_allowance_filled(&self) {
        let total = self
            .device_allowances_filled
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        eprintln!(
            "{}",
            serde_json::json!({
                "event": "edge.accounts.device_allowance_filled",
                "total": total,
            })
        );
    }

    /// Tells the account's owner, off the request path so the attempt's timing says nothing about
    /// whether the account exists. See `LOCK_NOTICES_PER_EMAIL` and `LOCK_NOTICES_PER_CELL`.
    fn send_lock_notice(&self, email: Email, account: AccountRow) {
        let Some(outbox) = self.mail.clone() else {
            return;
        };
        let service = self.clone();
        self.runtime.spawn_blocking(move || {
            if let Err(error) = service.queue_lock_notice(&outbox, &email, &account) {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "event": "edge.accounts.lock_notice_failed",
                        "error": error.to_string(),
                    })
                );
            }
        });
    }

    fn queue_lock_notice(
        &self,
        outbox: &MailOutbox,
        email: &Email,
        account: &AccountRow,
    ) -> Result<(), AccountError> {
        if self
            .active_human(&account.tenant, &account.region, &account.principal_id)?
            .is_none()
        {
            return Ok(());
        }
        let letter = Letter::SignInLocked {
            to: email.as_str().to_string(),
            reset: self.reset_help(),
        };
        let cap = Limit {
            hits: self.lock_notice_cap.hits,
            ..LOCK_NOTICES_PER_CELL
        };
        self.block(async {
            let bucket = Self::bucket(&LOCK_NOTICES_PER_EMAIL, &account.email_digest);
            let episode = match self.reserve(&LOCK_NOTICES_PER_EMAIL, bucket).await {
                Ok(slot) => slot,
                Err(AccountError::RateLimited { .. }) => return Ok(()),
                Err(error) => return Err(error),
            };
            let cell = match self
                .reserve(&cap, Self::bucket(&cap, &self.lock_notice_cap.key))
                .await
            {
                Ok(slot) => slot,
                Err(refused) => {
                    // The episode is not spent on a notice that was never sent.
                    self.give_back(&[episode]).await;
                    if !matches!(refused, AccountError::RateLimited { .. }) {
                        return Err(refused);
                    }
                    self.log_dropped_notice();
                    return Ok(());
                }
            };
            let queued = self.queue_notice(outbox, &letter).await;
            if queued.is_err() {
                self.give_back(&[episode, cell]).await;
            }
            queued
        })
    }

    async fn queue_notice(&self, outbox: &MailOutbox, letter: &Letter) -> Result<(), AccountError> {
        #[cfg(any(test, feature = "test-support"))]
        if self.lock_notice_cap.fail_queue {
            return Err(AccountError::Unavailable(
                "lock notices fail in this test".into(),
            ));
        }
        Ok(self.store.queue(outbox, letter).await?)
    }

    fn log_dropped_notice(&self) {
        if let Some(dropped) = self
            .now_unix()
            .ok()
            .and_then(|now| self.dropped_notices.record(now))
        {
            eprintln!(
                "{}",
                serde_json::json!({
                    "event": "edge.accounts.lock_notices_dropped",
                    "reason": "too many lock notices across the cell",
                    "dropped": dropped,
                })
            );
        }
    }

    /// Verifies against the stored hash, or against the dummy when there is none or the
    /// password is too short to be anyone's.
    fn verify(&self, password: String, stored_phc: Option<&str>) -> bool {
        match Password::new(password) {
            Ok(password) => password.verify(stored_phc, &self.dummy),
            Err(WeakPassword) => Password::new("x".repeat(credentials::MIN_PASSWORD_CHARS))
                .expect("the filler meets the policy")
                .verify(None, &self.dummy),
        }
    }

    /// The person behind a verified password and when their session counts as issued, if the
    /// password is still the stored one and the person is still active. A reset that lands during
    /// verification makes this `None`.
    fn still_signs_in(
        &self,
        account: &AccountRow,
    ) -> Result<Option<(Principal, i64)>, AccountError> {
        let Some(principal) =
            self.active_human(&account.tenant, &account.region, &account.principal_id)?
        else {
            return Ok(None);
        };
        let issued_at = self.block(async { Ok(self.store.still_signs_in(account).await?) })?;
        Ok(issued_at.map(|issued_at| (principal, issued_at)))
    }

    fn active_human(
        &self,
        tenant: &str,
        region: &str,
        principal_id: &str,
    ) -> Result<Option<Principal>, AccountError> {
        let scope = scope_for(tenant, region);
        let row = self
            .principals
            .get_principal(&scope, &PrincipalId(principal_id.to_string()))
            .map_err(|error| AccountError::Unavailable(error.to_string()))?;
        Ok(row
            .filter(|row| {
                matches!(row.kind, PrincipalKind::Human) && row.status == PrincipalStatus::Active
            })
            .map(|row| {
                Principal::new(
                    TenantId(tenant.to_string()),
                    Region(region.to_string()),
                    row.principal_id,
                    row.kind,
                    row.data_role,
                    row.status,
                )
            }))
    }

    /// What a link shows. A code that is not even well-formed is refused without touching the
    /// database, and a lookup never writes.
    pub fn invitation(&self, code: &str) -> Result<InvitationView, AccountError> {
        self.block(async {
            let live = self.live_link(code).await?;
            Ok(InvitationView {
                kind: live.invitation.kind.as_str(),
                email: live.email.as_str().to_string(),
                organization: self
                    .store
                    .organization_name(&live.invitation.tenant)
                    .await?,
                expires_at_unix: live.invitation.expires_at_unix,
            })
        })
    }

    async fn live_link(&self, code: &str) -> Result<LiveLink, AccountError> {
        let code = InvitationCode::parse(code).ok_or(AccountError::InvitationNotFound)?;
        let invitation = self
            .store
            .invitation(&code.digest())
            .await?
            .ok_or(AccountError::InvitationNotFound)?;
        let email = code
            .open_email(&invitation.email)
            .filter(|email| self.digester.digest(email) == invitation.email_digest)
            .ok_or_else(|| {
                AccountError::Unavailable("a stored invitation does not match its link".into())
            })?;
        // A member's link holds only while they could still send it; accepting checks again.
        if invitation.member_sent {
            let still_may = match (&invitation.invited_by, invitation.role) {
                (Some(inviter), Some(role)) => self
                    .store
                    .role_of(&invitation.tenant, &invitation.region, inviter)
                    .await?
                    .is_some_and(|inviter| inviter.may_grant(role)),
                _ => false,
            };
            if !still_may {
                return Err(AccountError::InvitationNotFound);
            }
        }
        Ok(LiveLink {
            code,
            invitation,
            email,
        })
    }

    /// Accepts a link: a join creates the person, a reset replaces their password. Either way the
    /// person is signed in as the result. The link is found before any password is hashed, so
    /// only someone holding a live link can make this Edge run Argon2.
    pub fn accept(&self, code: &str, acceptance: Acceptance) -> Result<SignedIn, AccountError> {
        let live = self.block(self.live_link(code))?;
        match live.invitation.kind {
            InvitationKind::Join => self.accept_join(live, acceptance),
            InvitationKind::Reset => self.accept_reset(live, acceptance),
        }
    }

    fn accept_join(
        &self,
        live: LiveLink,
        acceptance: Acceptance,
    ) -> Result<SignedIn, AccountError> {
        let display_name = display_name(acceptance.display_name.as_deref())?;
        let password = Password::new(acceptance.password)?;
        // The common conflicts are answered before a hash or a key is made for them. An address
        // whose person was removed from this organization joins again as a new person: nothing
        // tied to the old one (agents, delegations, tokens, grants) comes back with them.
        let invitation = &live.invitation;
        let digest = invitation.email_digest;
        let replacing = match self.block(async { Ok(self.store.account(&digest).await?) })? {
            None => None,
            Some(account)
                if account.tenant != invitation.tenant || account.region != invitation.region =>
            {
                return Err(AccountError::AccountExists);
            }
            Some(account) => {
                let standing = self.block(async {
                    Ok(self
                        .store
                        .standing(&account.tenant, &account.region, &account.principal_id)
                        .await?)
                })?;
                match standing {
                    Standing::Removed => Some(account.principal_id),
                    Standing::Erased => return Err(AccountError::PersonErased),
                    Standing::Present => return Err(AccountError::AccountExists),
                }
            }
        };
        let password_phc = self.hashing(|| password.hash())?;
        let scope = scope_for(&live.invitation.tenant, &live.invitation.region);
        let principal_id =
            PrincipalId(format!("person-{}", self.ids.mint().0.to_ascii_lowercase()));
        let row = self
            .principals
            .sealed_new_human(
                &scope,
                &principal_id,
                &PrincipalProfile {
                    email: live.email.as_str().to_string(),
                    display_name,
                },
            )
            .map_err(|error| AccountError::Unavailable(error.to_string()))?;
        #[cfg(any(test, feature = "test-support"))]
        self.reached(Checkpoint::JoinKeyMade);
        let outcome = self.block(async {
            Ok(self
                .store
                .accept_join(
                    &live.code.digest(),
                    &live.invitation,
                    row,
                    password_phc,
                    replacing,
                )
                .await?)
        });
        // A join the database refused rolled back, so nobody holds the new person's key. A
        // failed call may still have committed, so its key is kept rather than risk erasing a
        // stored profile.
        if matches!(
            outcome,
            Ok(AcceptOutcome::Gone | AcceptOutcome::EmailTaken | AcceptOutcome::Erased)
        ) {
            self.discard_key(&scope, &principal_id);
        }
        self.signed_in_after(&live.invitation, outcome?)
    }

    fn discard_key(&self, scope: &TenantScope, principal_id: &PrincipalId) {
        if let Err(error) = self
            .principals
            .discard_unstored_subject_key(scope, principal_id)
        {
            // The id lets an operator find the key; it is not the person's email.
            eprintln!(
                "{}",
                serde_json::json!({
                    "event": "edge.accounts.join_key_left_behind",
                    "reason": "the key made for a refused join could not be destroyed",
                    "principal_id": principal_id.0,
                    "error": error.to_string(),
                })
            );
        }
    }

    fn accept_reset(
        &self,
        live: LiveLink,
        acceptance: Acceptance,
    ) -> Result<SignedIn, AccountError> {
        if acceptance.display_name.is_some() {
            return Err(AccountError::UnexpectedDisplayName);
        }
        let password = Password::new(acceptance.password)?;
        let password_phc = self.hashing(|| password.hash())?;
        let notice = self.password_changed_notice(live.email.as_str());
        // A reset proves the owner, so strangers' failed attempts no longer hold them back, and
        // the lock episode is over.
        let digest = &live.invitation.email_digest;
        let unlock = vec![
            Self::bucket(&LOGIN_FAILURES_PER_EMAIL, digest),
            Self::bucket(&LOCK_NOTICES_PER_EMAIL, digest),
        ];
        let outcome = self.block(async {
            Ok(self
                .store
                .accept_reset(
                    &live.code.digest(),
                    &live.invitation,
                    password_phc,
                    unlock,
                    notice,
                )
                .await?)
        })?;
        self.signed_in_after(&live.invitation, outcome)
    }

    fn password_changed_notice(&self, email: &str) -> Option<(MailOutbox, Letter)> {
        self.mail.as_ref().map(|outbox| {
            let letter = Letter::PasswordChanged {
                to: email.to_string(),
                reset: self.reset_help(),
            };
            (outbox.clone(), letter)
        })
    }

    fn signed_in_after(
        &self,
        invitation: &Invitation,
        outcome: AcceptOutcome,
    ) -> Result<SignedIn, AccountError> {
        match outcome {
            AcceptOutcome::Accepted {
                principal_id,
                issued_at_unix,
            } => {
                let principal = self
                    .active_human(&invitation.tenant, &invitation.region, &principal_id)?
                    .ok_or(AccountError::InvitationNotFound)?;
                self.signed_in(principal, issued_at_unix, &invitation.email_digest, None)
            }
            AcceptOutcome::Gone => Err(AccountError::InvitationNotFound),
            AcceptOutcome::EmailTaken => Err(AccountError::AccountExists),
            AcceptOutcome::Erased => Err(AccountError::PersonErased),
        }
    }

    /// A signed-in person changes their password. A wrong current password counts against the
    /// same allowance as a failed sign-in from this browser would. On success every session the
    /// person holds ends, and they are signed in afresh.
    pub fn change_password(
        &self,
        person: &Principal,
        current: String,
        new: String,
        device_token: Option<&str>,
    ) -> Result<SignedIn, AccountError> {
        let new = Password::new(new)?;
        let (tenant, region) = (&person.tenant.0, &person.region.0);
        let (account, slot, device) = self.block(async {
            let account = self
                .store
                .account_of(tenant, region, &person.principal_id.0)
                .await?
                .ok_or(AccountError::NoPassword)?;
            let charge = self.charge(&account.email_digest, device_token)?;
            let slot = self.reserve(charge.limit, charge.bucket).await?;
            Ok((account, slot, charge.device))
        })?;
        let verified = self.hashing(|| self.verify(current, Some(&account.password_phc)));
        #[cfg(any(test, feature = "test-support"))]
        self.reached(Checkpoint::PasswordVerified);
        let password_phc = match verified.and_then(|verified| {
            if verified {
                self.hashing(|| new.hash())
            } else {
                Err(AccountError::WrongPassword)
            }
        }) {
            Ok(password_phc) => password_phc,
            // A wrong guess keeps the slot it took; that is the point of taking it first.
            Err(AccountError::WrongPassword) => {
                if slot.filled && device.is_some() {
                    self.note_device_allowance_filled();
                }
                return Err(AccountError::WrongPassword);
            }
            Err(refused) => return Err(self.refund(slot, refused)),
        };
        let notice = match self.mail.as_ref().map(|_| self.profile(person)) {
            Some(Ok(profile)) => {
                profile.and_then(|profile| self.password_changed_notice(&profile.email))
            }
            Some(Err(error)) => return Err(self.refund(slot, error)),
            None => None,
        };
        let outcome = self.block(async {
            Ok(self
                .store
                .change_password(&account, password_phc, notice)
                .await?)
        });
        let cutoff = match outcome {
            Ok(ChangeOutcome::Changed { cutoff }) => cutoff,
            Ok(ChangeOutcome::ChangedElsewhere) => {
                return Err(self.refund(slot, AccountError::PasswordChangedElsewhere));
            }
            Ok(ChangeOutcome::Inactive) => {
                return Err(self.refund(slot, AccountError::SessionEnded));
            }
            Err(error) => return Err(self.refund(slot, error)),
        };
        self.clear_after_success(slot, &account.email_digest)?;
        self.signed_in(person.clone(), cutoff, &account.email_digest, device)
    }

    /// Gives back a slot taken for an attempt that never got to verify a guess, and passes its
    /// error on.
    fn refund(&self, slot: Slot, error: AccountError) -> AccountError {
        let _ = self.block(async {
            self.give_back(&[slot]).await;
            Ok(())
        });
        error
    }

    /// A signed-in owner or admin invites a teammate into their own organization, to join with
    /// `role`. They can only make join links, and a join link can never touch an existing account
    /// or a reset link. A role that may not grant `role` is refused before anything is counted.
    pub fn invite_member(
        &self,
        inviter: &Principal,
        email: &str,
        role: OrgRole,
    ) -> Result<MemberInvitation, AccountError> {
        if !self
            .role_of(inviter)?
            .is_some_and(|inviter| inviter.may_grant(role))
        {
            return Err(AccountError::RoleRequired);
        }
        self.block(async {
            let email = Email::parse(email).ok_or(AccountError::InvalidEmail)?;
            let mut taken = Vec::new();
            // The address's allowance is checked first, so a refusal there costs the member
            // nothing. It applies only with mail on: otherwise nothing reaches the address, and
            // a member remaking a lost link is limited by their own allowance alone.
            if self.mail.is_some() {
                let mut invitee_key = inviter.tenant.0.as_bytes().to_vec();
                invitee_key.push(0);
                invitee_key.extend_from_slice(&self.digester.digest(&email));
                let bucket = Self::bucket(&INVITES_PER_RECIPIENT, &invitee_key);
                let slot = self
                    .reserve(&INVITES_PER_RECIPIENT, bucket)
                    .await
                    .map_err(|error| match error {
                        AccountError::RateLimited { retry_after_secs } => {
                            AccountError::RecipientRateLimited { retry_after_secs }
                        }
                        other => other,
                    })?;
                taken.push(slot);
            }
            let mut inviter_key = inviter.tenant.0.as_bytes().to_vec();
            inviter_key.push(0);
            inviter_key.extend_from_slice(inviter.principal_id.0.as_bytes());
            let bucket = Self::bucket(&INVITES_PER_MEMBER, &inviter_key);
            let outcome = match self.reserve(&INVITES_PER_MEMBER, bucket).await {
                Ok(slot) => {
                    taken.push(slot);
                    self.member_link(inviter, email, role).await
                }
                Err(refused) => Err(refused),
            };
            // An invitation that was not made is not charged.
            if outcome.is_err() {
                self.give_back(&taken).await;
            }
            outcome
        })
    }

    async fn member_link(
        &self,
        inviter: &Principal,
        email: Email,
        role: OrgRole,
    ) -> Result<MemberInvitation, AccountError> {
        // Only an account in the inviter's own organization is reported, so a member cannot
        // probe which addresses have accounts elsewhere in the cell. An address taken in
        // another organization gets a link that fails when it is accepted.
        let tenant = &inviter.tenant.0;
        let rule = IssueRule::MemberJoin {
            inviter: &inviter.principal_id.0,
            role,
        };
        let Some(outbox) = &self.mail else {
            return self
                .issue(
                    rule,
                    tenant,
                    &inviter.region.0,
                    email,
                    JOIN_INVITATION_TTL_SECS,
                    None,
                )
                .await
                .map(MemberInvitation::HandedOver);
        };
        // Mailed, the link goes only to the invitee, so opening it proves they hold the address.
        let organization = self.store.organization_name(tenant).await?;
        let issued = self
            .issue(
                rule,
                tenant,
                &inviter.region.0,
                email,
                JOIN_INVITATION_TTL_SECS,
                Some((outbox, LinkLetter::Invitation { organization })),
            )
            .await?;
        Ok(MemberInvitation::Emailed {
            email: issued.email,
            expires_at_unix: issued.expires_at_unix,
        })
    }

    /// Best effort: a slot that cannot be given back only throttles a little early.
    async fn give_back(&self, slots: &[Slot]) {
        for slot in slots {
            let _ = self.store.release_hit(*slot).await;
        }
    }

    /// `POST /v1/auth/password/forgot`. The caller always gets the same answer at once; the work
    /// happens afterwards, so neither the answer nor its timing says whether the email has an
    /// account.
    pub fn forgot_password(&self, email: &str) {
        let Ok(permit) = self.reset_requests.clone().try_acquire_owned() else {
            if let Some(dropped) = self
                .now_unix()
                .ok()
                .and_then(|now| self.dropped_resets.record(now))
            {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "event": "edge.accounts.reset_requests_dropped",
                        "reason": "too many reset requests in progress",
                        "dropped": dropped,
                    })
                );
            }
            return;
        };
        let service = self.clone();
        let email = email.to_string();
        self.runtime.spawn_blocking(move || {
            let _permit = permit;
            if let Err(error) = service.request_password_reset(&email) {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "event": "edge.accounts.reset_request_failed",
                        "error": error.to_string(),
                    })
                );
            }
        });
    }

    /// Emails a reset link when the address belongs to an active person and its hourly allowance
    /// is not used up; otherwise does nothing. Says nothing about which happened.
    pub fn request_password_reset(&self, email: &str) -> Result<(), AccountError> {
        let (Some(outbox), Some(email)) = (&self.mail, Email::parse(email)) else {
            return Ok(());
        };
        let digest = self.digester.digest(&email);
        let account = self.block(async {
            let bucket = Self::bucket(&RESET_REQUESTS_PER_EMAIL, &digest);
            match self.reserve(&RESET_REQUESTS_PER_EMAIL, bucket).await {
                Ok(_) => Ok(self.store.account(&digest).await?),
                Err(AccountError::RateLimited { .. }) => Ok(None),
                Err(error) => Err(error),
            }
        })?;
        let Some(account) = account else {
            return Ok(());
        };
        if self
            .active_human(&account.tenant, &account.region, &account.principal_id)?
            .is_none()
        {
            return Ok(());
        }
        let issued = self.block(self.issue(
            IssueRule::RequestedReset,
            &account.tenant,
            &account.region,
            email,
            RESET_INVITATION_TTL_SECS,
            Some((outbox, LinkLetter::PasswordReset)),
        ));
        match issued {
            Ok(_) | Err(AccountError::NoSuchAccount) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Where a person who did not change their own password asks for a reset.
    fn reset_help(&self) -> Option<String> {
        match &self.links {
            LinkOrigin::Ready(origin) => Some(format!("{origin}/login?help=password")),
            LinkOrigin::Unset | LinkOrigin::Refused(_) => None,
        }
    }

    /// The operator's join link, for the first person in an organization or anyone after. It
    /// grants `role`, or without one: owner for an organization with no people and no pending
    /// owner's link yet, member otherwise. `IssuedInvitation::role` says which.
    pub async fn operator_join_link(
        &self,
        tenant: &str,
        email: &str,
        ttl_secs: Option<i64>,
        role: Option<OrgRole>,
    ) -> Result<IssuedInvitation, AccountError> {
        let email = Email::parse(email).ok_or(AccountError::InvalidEmail)?;
        self.issue(
            IssueRule::OperatorJoin { role },
            tenant,
            &self.region,
            email,
            ttl_secs.unwrap_or(JOIN_INVITATION_TTL_SECS),
            None,
        )
        .await
    }

    /// The operator's password reset link for an existing account in `tenant`.
    pub async fn operator_reset_link(
        &self,
        tenant: &str,
        email: &str,
        ttl_secs: Option<i64>,
    ) -> Result<IssuedInvitation, AccountError> {
        let email = Email::parse(email).ok_or(AccountError::InvalidEmail)?;
        self.issue(
            IssueRule::OperatorReset,
            tenant,
            &self.region,
            email,
            ttl_secs.unwrap_or(RESET_INVITATION_TTL_SECS),
            None,
        )
        .await
    }

    fn link_origin(&self) -> Result<&str, AccountError> {
        match &self.links {
            LinkOrigin::Ready(origin) => Ok(origin),
            LinkOrigin::Unset => Err(AccountError::LinksUnavailable),
            LinkOrigin::Refused(reason) => {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "event": "edge.accounts.link_refused",
                        "reason": reason,
                    })
                );
                Err(AccountError::LinksUnavailable)
            }
        }
    }

    /// Stores a new link. With `mail`, the letter it writes for the link is queued in the same
    /// transaction, so the link exists only if its mail does.
    async fn issue(
        &self,
        rule: IssueRule<'_>,
        tenant: &str,
        region: &str,
        email: Email,
        ttl_secs: i64,
        mail: Option<(&MailOutbox, LinkLetter)>,
    ) -> Result<IssuedInvitation, AccountError> {
        let origin = self.link_origin()?;
        if tenant.trim().is_empty() || region.trim().is_empty() {
            return Err(AccountError::Unavailable(
                "tenant and region are required".into(),
            ));
        }
        #[cfg(any(test, feature = "test-support"))]
        self.reached(Checkpoint::BeforeIssuing);
        let code = InvitationCode::generate();
        let email_digest = self.digester.digest(&email);
        let link = NewInvitation {
            tenant: tenant.to_string(),
            region: region.to_string(),
            email_digest,
            email: code.seal_email(&email),
            roster: rule
                .is_join()
                .then(|| self.roster.seal(&email, &email_digest)),
            ttl_secs: ttl_secs.clamp(60, MAX_INVITATION_TTL_SECS),
        };
        let url = format!("{origin}/join/{}", code.encoded());
        let mail = mail.map(|(outbox, letter)| LinkMail {
            outbox: outbox.clone(),
            letter,
            to: email.as_str().to_string(),
            url: url.clone(),
        });
        match self
            .store
            .issue(&code.digest(), rule, &link, mail.as_ref())
            .await?
        {
            IssueOutcome::Issued {
                expires_at_unix,
                role,
            } => Ok(IssuedInvitation {
                url,
                email: email.as_str().to_string(),
                expires_at_unix,
                role,
            }),
            IssueOutcome::AccountExists => Err(AccountError::AccountExists),
            IssueOutcome::NoSuchAccount => Err(AccountError::NoSuchAccount),
            IssueOutcome::NotAllowed => Err(AccountError::RoleRequired),
            IssueOutcome::InvitationExists => Err(AccountError::InvitationExists),
        }
    }

    /// Whether the principal was removed from an organization: such a principal stays out, and an
    /// operator may not bring it back with `edge bootstrap`.
    pub async fn was_removed(
        &self,
        tenant: &str,
        region: &str,
        principal_id: &str,
    ) -> Result<bool, AccountError> {
        Ok(self.store.was_removed(tenant, region, principal_id).await?)
    }

    /// A person who signed in another way (SSO) becomes a member, if they have no role yet.
    pub fn enrol_member(&self, person: &Principal) -> Result<(), AccountError> {
        self.block(async {
            Ok(self
                .store
                .enrol_member(&person.tenant.0, &person.region.0, &person.principal_id.0)
                .await?)
        })
    }

    /// The person's role in their organization; `None` unless they are an active member.
    pub fn role_of(&self, person: &Principal) -> Result<Option<OrgRole>, AccountError> {
        self.block(async {
            Ok(self
                .store
                .role_of(&person.tenant.0, &person.region.0, &person.principal_id.0)
                .await?)
        })
    }

    /// The organization's people, and its pending invitations when `viewer` manages people.
    pub fn roster(&self, viewer: &Principal) -> Result<RosterView, AccountError> {
        let (tenant, region) = (&viewer.tenant.0, &viewer.region.0);
        let roster = self
            .block(async {
                Ok(self
                    .store
                    .roster(tenant, region, &viewer.principal_id.0)
                    .await?)
            })?
            .ok_or(AccountError::RoleRequired)?;
        let scope = scope_for(tenant, region);
        let people = roster
            .people
            .into_iter()
            .map(|person| {
                let (display_name, email) = self
                    .principals
                    .get_profile(&scope, &PrincipalId(person.principal_id.clone()))
                    .ok()
                    .flatten()
                    .map(|profile| (profile.display_name, profile.email))
                    .unwrap_or_default();
                MemberView {
                    principal_id: person.principal_id,
                    display_name,
                    email,
                    role: person.role,
                    joined_at_unix: person.joined_at_unix,
                }
            })
            .collect();
        let invitations = roster.viewer.manages_people().then(|| {
            roster
                .pending
                .into_iter()
                .map(|link| PendingInvitation {
                    email: link
                        .roster
                        .and_then(|sealed| self.roster.open(&sealed, &link.email_digest))
                        .map(|email| email.as_str().to_string()),
                    id: link.id,
                    role: link.role,
                    expires_at_unix: link.expires_at_unix,
                })
                .collect()
        });
        if roster.owners == 0 {
            eprintln!(
                "{}",
                serde_json::json!({
                    "event": "edge.accounts.organization_without_owner",
                    "tenant": tenant,
                    "region": region,
                    "recovery": "edge role --tenant T --email E --role owner",
                })
            );
        }
        Ok(RosterView {
            viewer: roster.viewer,
            people,
            invitations,
            owners: roster.owners,
        })
    }

    /// An owner or admin gives a person `role`.
    pub fn set_member_role(
        &self,
        actor: &Principal,
        target: &str,
        role: OrgRole,
    ) -> Result<(), AccountError> {
        self.change_member(actor, target, MemberChange::Role(role))
    }

    /// An owner or admin removes a person: they are disabled, and every session they hold ends.
    pub fn remove_member(&self, actor: &Principal, target: &str) -> Result<(), AccountError> {
        self.change_member(actor, target, MemberChange::Remove)
    }

    fn change_member(
        &self,
        actor: &Principal,
        target: &str,
        change: MemberChange,
    ) -> Result<(), AccountError> {
        let outcome = self.block(async {
            Ok(self
                .store
                .change_member(
                    &actor.tenant.0,
                    &actor.region.0,
                    Some(&actor.principal_id.0),
                    target,
                    change,
                )
                .await?)
        })?;
        roster_result(outcome, AccountError::NoSuchMember)
    }

    /// An owner or admin withdraws a pending invitation and any mail still carrying it.
    pub fn revoke_invitation(&self, actor: &Principal, id: &str) -> Result<(), AccountError> {
        let outcome = self.block(async {
            Ok(self
                .store
                .revoke_invitation(&actor.tenant.0, &actor.region.0, &actor.principal_id.0, id)
                .await?)
        })?;
        roster_result(outcome, AccountError::InvitationNotFound)
    }

    /// The operator gives a person in `tenant` a role, as an owner could; an active person with
    /// no role yet (one who signs in another way) gets one. The organization still keeps an
    /// owner: promote another before demoting the last.
    pub async fn operator_set_role(
        &self,
        tenant: &str,
        target: OperatorTarget<'_>,
        role: OrgRole,
    ) -> Result<(), AccountError> {
        let principal_id = match target {
            OperatorTarget::Principal(id) => id.to_string(),
            OperatorTarget::Email(email) => {
                let email = Email::parse(email).ok_or(AccountError::InvalidEmail)?;
                let digest = self.digester.digest(&email);
                self.store
                    .principal_by_email(tenant, &self.region, &digest)
                    .await?
                    .ok_or(AccountError::NoSuchAccount)?
            }
        };
        let outcome = self
            .store
            .change_member(
                tenant,
                &self.region,
                None,
                &principal_id,
                MemberChange::Role(role),
            )
            .await?;
        roster_result(outcome, AccountError::NoSuchMember)
    }

    /// The signed-in person's own name and email, for `/v1/whoami`.
    pub fn profile(&self, principal: &Principal) -> Result<Option<PrincipalProfile>, AccountError> {
        self.principals
            .get_profile(
                &scope_for(&principal.tenant.0, &principal.region.0),
                &principal.principal_id,
            )
            .map_err(|error| AccountError::Unavailable(error.to_string()))
    }
}

fn roster_result(outcome: RosterOutcome, not_found: AccountError) -> Result<(), AccountError> {
    match outcome {
        RosterOutcome::Done => Ok(()),
        RosterOutcome::NotFound => Err(not_found),
        RosterOutcome::NotAllowed => Err(AccountError::RoleRequired),
        RosterOutcome::LastOwner => Err(AccountError::LastOwner),
    }
}

fn display_name(raw: Option<&str>) -> Result<String, AccountError> {
    let name = raw.map(str::trim).unwrap_or_default();
    let chars = name.chars().count();
    if chars == 0 || chars > MAX_DISPLAY_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(AccountError::InvalidDisplayName);
    }
    Ok(name.to_string())
}

fn scope_for(tenant: &str, region: &str) -> TenantScope {
    TenantScope::from_verified_token(
        &Principal::stub(
            PrincipalId("account-service".into()),
            PrincipalKind::Human,
            TenantId(tenant.to_string()),
        ),
        Region(region.to_string()),
    )
}

/// The cell-wide lock notice cap and the bucket key it counts under.
struct NoticeCap {
    hits: u32,
    key: Vec<u8>,
    #[cfg(any(test, feature = "test-support"))]
    fail_queue: bool,
}

/// Counts dropped requests so a flood of them is logged once a minute, not once each.
#[derive(Default)]
struct DroppedRequests {
    minute: AtomicI64,
    since_logged: AtomicU64,
}

impl DroppedRequests {
    /// Counts one drop, and returns how many to log when this is the first in a new minute.
    fn record(&self, now_unix: i64) -> Option<u64> {
        self.since_logged.fetch_add(1, Ordering::Relaxed);
        let minute = now_unix.div_euclid(60);
        let last = self.minute.load(Ordering::Relaxed);
        (last != minute
            && self
                .minute
                .compare_exchange(last, minute, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok())
        .then(|| self.since_logged.swap(0, Ordering::Relaxed))
    }
}

/// Bounds how many password hashes run at once. Each Argon2id run holds 19 MiB and a core, so an
/// unbounded burst would exhaust the host; callers past the bound wait briefly, then get `Busy`.
struct HashGate {
    running: Mutex<usize>,
    freed: Condvar,
    capacity: usize,
}

struct HashPermit<'a>(&'a HashGate);

impl HashGate {
    const WAIT: Duration = Duration::from_secs(2);

    fn sized_for_host() -> Self {
        let cores = std::thread::available_parallelism().map_or(2, usize::from);
        Self::with_capacity((cores / 2).clamp(2, 8))
    }

    fn with_capacity(capacity: usize) -> Self {
        Self {
            running: Mutex::new(0),
            freed: Condvar::new(),
            capacity,
        }
    }

    fn enter(&self) -> Result<HashPermit<'_>, AccountError> {
        let running = self
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (mut running, timeout) = self
            .freed
            .wait_timeout_while(running, Self::WAIT, |running| *running >= self.capacity)
            .unwrap_or_else(|error| error.into_inner());
        if timeout.timed_out() && *running >= self.capacity {
            return Err(AccountError::Busy);
        }
        *running += 1;
        Ok(HashPermit(self))
    }
}

impl Drop for HashPermit<'_> {
    fn drop(&mut self) {
        let mut running = self
            .0
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *running -= 1;
        self.0.freed.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_are_trimmed_and_bounded() {
        assert_eq!(
            display_name(Some("  Ada Lovelace ")).unwrap(),
            "Ada Lovelace"
        );
        assert_eq!(display_name(None), Err(AccountError::InvalidDisplayName));
        assert_eq!(
            display_name(Some("   ")),
            Err(AccountError::InvalidDisplayName)
        );
        assert_eq!(
            display_name(Some("Ada\nLovelace")),
            Err(AccountError::InvalidDisplayName)
        );
        assert!(display_name(Some(&"é".repeat(128))).is_ok());
        assert!(display_name(Some(&"é".repeat(129))).is_err());
    }

    #[tokio::test]
    async fn the_dummy_hash_is_built_with_the_service_not_on_the_first_unknown_email() {
        // A lazy pool never connects, so no database is needed to build a service.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .expect("a lazy pool");
        let service = AccountService::new(
            pool,
            PrincipalStore::new(Arc::new(myelin_storage::KmsEngine::new())),
            AccountKeys::fixed(0),
            "eu-west",
            None,
            Handle::current(),
        );
        assert!(service.dummy.is_a_full_cost_hash());
    }

    #[test]
    fn dropped_reset_requests_are_logged_at_most_once_a_minute() {
        let dropped = DroppedRequests::default();
        assert_eq!(dropped.record(600), Some(1));
        assert_eq!(dropped.record(610), None);
        assert_eq!(dropped.record(659), None);
        assert_eq!(
            dropped.record(660),
            Some(3),
            "the next minute reports what it held back"
        );
    }

    #[test]
    fn the_hash_gate_admits_its_capacity_and_then_refuses() {
        let gate = HashGate {
            running: Mutex::new(0),
            freed: Condvar::new(),
            capacity: 1,
        };
        let held = gate.enter().expect("the first hash runs");
        let started = std::time::Instant::now();
        assert!(matches!(gate.enter(), Err(AccountError::Busy)));
        assert!(started.elapsed() >= HashGate::WAIT);
        drop(held);
        assert!(gate.enter().is_ok(), "a released permit is reusable");
    }
}
