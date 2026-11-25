#![cfg(feature = "integration")]

//! Password accounts against the real database: links are single-use under concurrency, a newer
//! link supersedes an older one, expiry is enforced, and removing a person removes their sign-in.

use myelin_config::{Mode, MyelinConfig};
use myelin_edge::accounts::{Acceptance, Checkpoint, MemberInvitation};
use myelin_edge::{account_migrations, AccountError, AccountService, EmailDigester};
use myelin_identity::PrincipalStatus;
use myelin_identity_service::PrincipalStore;
use myelin_storage::{
    all_durable_migrations, with_tenant_tx, DurableKmsBacking, DurablePrincipalBacking, HotTables,
    PgBootstrap, SealKey, SubstrateProvider,
};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};

const SEAL_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const PASSWORD: &str = "correct horse battery";

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// With mail off, a member's invitation link comes back to the inviter.
fn handed_over(invitation: MemberInvitation) -> String {
    match invitation {
        MemberInvitation::HandedOver(issued) => issued.url,
        MemberInvitation::Emailed { .. } => panic!("mail is off, so the link is handed over"),
    }
}

fn code_of(url: &str) -> String {
    url.rsplit_once("/join/")
        .expect("a join link")
        .1
        .to_string()
}

struct Cell {
    provider: SubstrateProvider,
    accounts: AccountService,
    principals: PrincipalStore,
    digest_key: EmailDigester,
    clock: Arc<AtomicI64>,
}

/// Expired links and counters are swept cell-wide, so the tests take turns, and start from the
/// real time so they never sweep live rows of a stack sharing the database.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn real_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

impl Cell {
    /// Another service over the same database, with its own web origin and the shared clock.
    fn service(&self, web_origin: Option<&str>) -> AccountService {
        let now = self.clock.clone();
        AccountService::new(
            self.provider.db_pool().clone(),
            self.principals.clone(),
            self.digest_key.clone(),
            self.provider.config().region.clone(),
            web_origin.map(str::to_string),
            tokio::runtime::Handle::current(),
        )
        .with_clock(move || now.load(Ordering::Relaxed))
    }

    async fn count(&self, sql: &str, tenant: &str) -> i64 {
        sqlx::query_scalar(sql)
            .bind(tenant)
            .fetch_one(self.provider.db_pool())
            .await
            .unwrap()
    }

    /// Subject keys the KMS holds for people in `tenant`.
    async fn subject_keys(&self, tenant: &str) -> i64 {
        self.count(
            "SELECT count(*) FROM kms_wrapped_dek \
              WHERE tenant_id = $1 AND class LIKE 'subject:%'",
            tenant,
        )
        .await
    }

    async fn principal_rows(&self, tenant: &str) -> i64 {
        with_tenant_tx(
            self.provider.db_pool(),
            tenant,
            &self.provider.config().region,
            |conn| {
                Box::pin(async move {
                    sqlx::query_scalar("SELECT count(*) FROM principal")
                        .fetch_one(&mut *conn)
                        .await
                        .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
                })
            },
        )
        .await
        .unwrap()
    }

    /// Moves every link of `tenant` one second into the past, by the database clock.
    async fn expire_links(&self, tenant: &str) {
        sqlx::query(
            "UPDATE auth_invitation \
                SET expires_at_unix = floor(extract(epoch FROM clock_timestamp()))::bigint - 1 \
              WHERE tenant_id = $1",
        )
        .bind(tenant)
        .execute(self.provider.db_pool())
        .await
        .unwrap();
    }

    async fn join_person(
        &self,
        tenant: &str,
        email: &str,
        name: &str,
    ) -> myelin_identity::Principal {
        let code = code_of(
            &self
                .accounts
                .operator_join_link(tenant, email, None)
                .await
                .unwrap()
                .url,
        );
        self.accounts.accept(&code, join(name, PASSWORD)).unwrap()
    }
}

async fn cell() -> Cell {
    let config = MyelinConfig::from_env(Mode::DevDefaults).expect("dev config");
    let bootstrap = PgBootstrap::connect(config, 8).await.expect("connect");
    bootstrap.migrate_foundation().await.expect("foundation");
    bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .expect("durable migrations");
    bootstrap
        .migrate(&account_migrations(), &HotTables::none())
        .await
        .expect("account migration");
    let provider = bootstrap.into_runtime().await.expect("runtime role");
    let seal = SealKey::from_encoded(SEAL_HEX).expect("seal key");
    let kms = Arc::new(
        DurableKmsBacking::new(provider.db_pool().clone(), unique("cell-accounts"))
            .load_or_generate(&seal)
            .await
            .expect("KMS"),
    );
    let principals = PrincipalStore::with_pg(
        kms,
        DurablePrincipalBacking::new(provider.clone()),
        tokio::runtime::Handle::current(),
    );
    let mut cell = Cell {
        accounts: AccountService::new(
            provider.db_pool().clone(),
            principals.clone(),
            EmailDigester::new(seal.derive_service_key(EmailDigester::SEAL_KEY_CONTEXT)),
            provider.config().region.clone(),
            None,
            tokio::runtime::Handle::current(),
        ),
        digest_key: EmailDigester::new(seal.derive_service_key(EmailDigester::SEAL_KEY_CONTEXT)),
        provider,
        principals,
        clock: Arc::new(AtomicI64::new(real_now())),
    };
    cell.accounts = cell.service(Some("https://myelin.example"));
    cell
}

fn join(display_name: &str, password: &str) -> Acceptance {
    Acceptance {
        display_name: Some(display_name.into()),
        password: password.into(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_invited_person_joins_signs_in_and_is_reset_by_the_operator() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-accounts");
    let email = format!("{}@Example.com", unique("Ada"));

    let issued = cell
        .accounts
        .operator_join_link(&tenant, &email, None)
        .await
        .expect("the operator invites the first person");
    assert!(issued.url.starts_with("https://myelin.example/join/"));
    assert_eq!(
        issued.email,
        email.to_lowercase(),
        "the link carries the normalized email"
    );
    let code = code_of(&issued.url);

    let view = cell.accounts.invitation(&code).expect("the link is live");
    assert_eq!(view.kind, "join");
    assert_eq!(view.email, email.to_lowercase());
    assert_eq!(
        view.organization, tenant,
        "no placement slug falls back to the tenant id"
    );
    assert!(
        (view.expires_at_unix - (real_now() + 7 * 24 * 3600)).abs() <= 5,
        "a join link lasts a week by the database clock"
    );

    assert_eq!(
        cell.accounts
            .accept(&code, join("Ada", "too short"))
            .unwrap_err(),
        AccountError::WeakPassword
    );
    assert_eq!(
        cell.accounts
            .accept(
                &code,
                Acceptance {
                    display_name: None,
                    password: PASSWORD.into()
                }
            )
            .unwrap_err(),
        AccountError::InvalidDisplayName
    );
    let joined = cell
        .accounts
        .accept(&code, join("Ada Lovelace", PASSWORD))
        .expect("a failed attempt does not use up the link");
    assert_eq!(joined.tenant.0, tenant);
    assert_eq!(joined.status, PrincipalStatus::Active);
    let profile = cell.accounts.profile(&joined).unwrap().expect("a profile");
    assert_eq!(profile.display_name, "Ada Lovelace");
    assert_eq!(profile.email, email.to_lowercase());

    assert_eq!(
        cell.accounts.invitation(&code).unwrap_err(),
        AccountError::InvitationNotFound,
        "a used link is gone"
    );
    assert_eq!(
        cell.accounts
            .accept(&code, join("Eve", PASSWORD))
            .unwrap_err(),
        AccountError::InvitationNotFound
    );

    let signed_in = cell
        .accounts
        .login(&format!("  {}  ", email.to_uppercase()), PASSWORD.into())
        .expect("sign-in normalizes the email");
    assert_eq!(signed_in.principal_id, joined.principal_id);
    for (who, password) in [
        (email.as_str(), "wrong password!"),
        ("nobody@example.com", PASSWORD),
    ] {
        assert_eq!(
            cell.accounts.login(who, password.into()).unwrap_err(),
            AccountError::LoginFailed
        );
    }

    assert_eq!(
        cell.accounts
            .operator_join_link(&tenant, &email, None)
            .await
            .unwrap_err(),
        AccountError::AccountExists
    );
    assert_eq!(
        cell.accounts.invite_member(&joined, &email).unwrap_err(),
        AccountError::AccountExists,
        "a member cannot make a link for an existing account"
    );
    assert_eq!(
        cell.accounts
            .operator_reset_link("another-tenant", &email, None)
            .await
            .unwrap_err(),
        AccountError::NoSuchAccount,
        "a reset link is only for an account in the named tenant"
    );

    let reset = cell
        .accounts
        .operator_reset_link(&tenant, &email, None)
        .await
        .expect("the operator issues a reset link");
    let reset_code = code_of(&reset.url);
    assert_eq!(cell.accounts.invitation(&reset_code).unwrap().kind, "reset");
    assert!((reset.expires_at_unix - (real_now() + 24 * 3600)).abs() <= 5);
    let after_reset = cell
        .accounts
        .accept(
            &reset_code,
            Acceptance {
                display_name: None,
                password: "a brand new passphrase".into(),
            },
        )
        .expect("the reset is accepted");
    assert_eq!(after_reset.principal_id, joined.principal_id);
    assert_eq!(
        cell.accounts.login(&email, PASSWORD.into()).unwrap_err(),
        AccountError::LoginFailed
    );
    cell.accounts
        .login(&email, "a brand new passphrase".into())
        .expect("the new password works");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_people_racing_for_one_link_produce_exactly_one_account() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-race");
    let email = format!("{}@example.com", unique("grace"));
    let code = code_of(
        &cell
            .accounts
            .operator_join_link(&tenant, &email, None)
            .await
            .unwrap()
            .url,
    );

    let racers: Vec<_> = (0..4)
        .map(|n| {
            let accounts = cell.accounts.clone();
            let code = code.clone();
            tokio::task::spawn_blocking(move || {
                accounts.accept(&code, join(&format!("Racer {n}"), PASSWORD))
            })
        })
        .collect();
    let mut outcomes = Vec::new();
    for racer in racers {
        outcomes.push(racer.await.unwrap());
    }
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    assert!(outcomes
        .iter()
        .filter_map(|outcome| outcome.as_ref().err())
        .all(|error| *error == AccountError::InvitationNotFound));
    let scope_principals: i64 = with_tenant_tx(
        cell.provider.db_pool(),
        &tenant,
        &cell.provider.config().region,
        |conn| {
            Box::pin(async move {
                sqlx::query_scalar("SELECT count(*) FROM principal")
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
            })
        },
    )
    .await
    .unwrap();
    assert_eq!(
        scope_principals, 1,
        "the losers left no half-created person"
    );
    assert_eq!(
        cell.subject_keys(&tenant).await,
        1,
        "the losers' encryption keys were destroyed"
    );

    // Two organizations inviting the same address: the second acceptance finds it taken and
    // creates nothing, and its link stays unused.
    let other_tenant = unique("t-race-other");
    let second = code_of(
        &cell
            .accounts
            .invite_member(
                &outcomes
                    .into_iter()
                    .find_map(Result::ok)
                    .unwrap()
                    .with_tenant(&other_tenant),
                &email,
            )
            .map(handed_over)
            .expect("another organization may still send a link"),
    );
    assert_eq!(
        cell.accounts
            .accept(&second, join("Grace", PASSWORD))
            .unwrap_err(),
        AccountError::AccountExists
    );
    assert!(cell.accounts.invitation(&second).is_ok());
    assert_eq!(
        cell.subject_keys(&other_tenant).await,
        0,
        "a join refused for a taken address leaves no key behind"
    );
}

trait InTenant {
    fn with_tenant(self, tenant: &str) -> Self;
}

impl InTenant for myelin_identity::Principal {
    fn with_tenant(mut self, tenant: &str) -> Self {
        self.tenant = myelin_tenancy::TenantId(tenant.into());
        self
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_the_newest_link_works_and_none_outlives_its_expiry() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-supersede");
    let email = format!("{}@example.com", unique("hopper"));
    let first = code_of(
        &cell
            .accounts
            .operator_join_link(&tenant, &email, None)
            .await
            .unwrap()
            .url,
    );
    let second = code_of(
        &cell
            .accounts
            .operator_join_link(&tenant, &email, Some(3600))
            .await
            .unwrap()
            .url,
    );
    assert_eq!(
        cell.accounts.invitation(&first).unwrap_err(),
        AccountError::InvitationNotFound,
        "a newer link for the same email supersedes the older one"
    );
    assert!(cell.accounts.invitation(&second).is_ok());

    cell.expire_links(&tenant).await;
    assert_eq!(
        cell.accounts.invitation(&second).unwrap_err(),
        AccountError::InvitationNotFound
    );
    assert_eq!(
        cell.accounts
            .accept(&second, join("Grace", PASSWORD))
            .unwrap_err(),
        AccountError::InvitationNotFound,
        "an expired link cannot create an account"
    );
    assert_eq!(
        cell.accounts.invitation("not-a-code").unwrap_err(),
        AccountError::InvitationNotFound
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sign_in_is_throttled_per_email_and_refused_to_inactive_or_removed_people() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-signin");
    let email = format!("{}@example.com", unique("lamarr"));
    let code = code_of(
        &cell
            .accounts
            .operator_join_link(&tenant, &email, None)
            .await
            .unwrap()
            .url,
    );
    let person = cell.accounts.accept(&code, join("Hedy", PASSWORD)).unwrap();
    let scope = myelin_storage::TenantScope::from_verified_token(&person, person.region.clone());

    cell.principals
        .put_principal(
            &scope,
            person.principal_id.clone(),
            person.kind.clone(),
            person.data_role,
            PrincipalStatus::Suspended,
            None,
        )
        .unwrap();
    assert_eq!(
        cell.accounts.login(&email, PASSWORD.into()).unwrap_err(),
        AccountError::LoginFailed,
        "a suspended person cannot sign in, even with the right password"
    );
    let reset = code_of(
        &cell
            .accounts
            .operator_reset_link(&tenant, &email, None)
            .await
            .unwrap()
            .url,
    );
    assert_eq!(
        cell.accounts
            .accept(
                &reset,
                Acceptance {
                    display_name: None,
                    password: "another passphrase".into()
                }
            )
            .unwrap_err(),
        AccountError::InvitationNotFound,
        "a reset link cannot sign a suspended person in"
    );

    // One attempt so far; nine more fill the window of ten.
    for _ in 0..9 {
        let _ = cell.accounts.login(&email, "wrong password!".into());
    }
    assert!(matches!(
        cell.accounts.login(&email, PASSWORD.into()),
        Err(AccountError::RateLimited { retry_after_secs }) if retry_after_secs <= 15 * 60
    ));
    let other = format!("{}@example.com", unique("unrelated"));
    assert_eq!(
        cell.accounts.login(&other, PASSWORD.into()).unwrap_err(),
        AccountError::LoginFailed,
        "the limit belongs to one email, not to everyone"
    );

    let tenant_owned = tenant.clone();
    let principal_id = person.principal_id.0.clone();
    with_tenant_tx(
        cell.provider.db_pool(),
        &tenant,
        &cell.provider.config().region,
        move |conn| {
            Box::pin(async move {
                sqlx::query("DELETE FROM principal WHERE tenant_id = $1 AND principal_id = $2")
                    .bind(&tenant_owned)
                    .bind(&principal_id)
                    .execute(&mut *conn)
                    .await
                    .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
            })
        },
    )
    .await
    .unwrap();
    let remaining: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM auth_password_account WHERE tenant_id = $1) + \
                (SELECT count(*) FROM auth_invitation WHERE tenant_id = $1)",
    )
    .bind(&tenant)
    .fetch_one(cell.provider.db_pool())
    .await
    .unwrap();
    assert_eq!(
        remaining, 0,
        "removing the person removes their password and any reset link for them"
    );
    cell.clock.fetch_add(15 * 60, Ordering::Relaxed);
    assert_eq!(
        cell.accounts.login(&email, PASSWORD.into()).unwrap_err(),
        AccountError::LoginFailed
    );
}

fn random_code() -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

fn reset_to(password: &str) -> Acceptance {
    Acceptance {
        display_name: None,
        password: password.into(),
    }
}

async fn account_table_rows(cell: &Cell) -> i64 {
    sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM auth_rate_limit) + (SELECT count(*) FROM auth_invitation)",
    )
    .fetch_one(cell.provider.db_pool())
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn junk_codes_never_throttle_real_links_and_write_nothing() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-junk");
    let email = format!("{}@example.com", unique("noether"));
    let code = code_of(
        &cell
            .accounts
            .operator_join_link(&tenant, &email, None)
            .await
            .unwrap()
            .url,
    );

    let before = account_table_rows(&cell).await;
    for _ in 0..1300 {
        assert_eq!(
            cell.accounts.invitation(&random_code()).unwrap_err(),
            AccountError::InvitationNotFound
        );
    }
    for junk in ["", "short", &"!".repeat(43), &format!("{code}=")] {
        assert_eq!(
            cell.accounts.invitation(junk).unwrap_err(),
            AccountError::InvitationNotFound
        );
    }
    for _ in 0..320 {
        assert_eq!(
            cell.accounts
                .accept(&random_code(), join("Mallory", PASSWORD))
                .unwrap_err(),
            AccountError::InvitationNotFound
        );
    }
    assert_eq!(
        account_table_rows(&cell).await,
        before,
        "refused lookups and acceptances write nothing"
    );

    assert!(cell.accounts.invitation(&code).is_ok());
    cell.accounts
        .accept(&code, join("Emmy", PASSWORD))
        .expect("junk traffic does not stop a real link");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_link_is_refused_even_when_the_edge_clock_lags() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-expiry");
    let joiner = format!("{}@example.com", unique("meitner"));
    let member = format!("{}@example.com", unique("franklin"));
    let join_code = code_of(
        &cell
            .accounts
            .operator_join_link(&tenant, &joiner, None)
            .await
            .unwrap()
            .url,
    );
    cell.join_person(&tenant, &member, "Rosalind").await;
    let reset_code = code_of(
        &cell
            .accounts
            .operator_reset_link(&tenant, &member, None)
            .await
            .unwrap()
            .url,
    );

    cell.expire_links(&tenant).await;
    cell.clock
        .store(real_now() - 2 * 24 * 3600, Ordering::Relaxed);
    assert_eq!(
        cell.accounts.invitation(&join_code).unwrap_err(),
        AccountError::InvitationNotFound
    );
    assert_eq!(
        cell.accounts
            .accept(&join_code, join("Lise", PASSWORD))
            .unwrap_err(),
        AccountError::InvitationNotFound,
        "the database clock decides, not the Edge's"
    );
    assert_eq!(
        cell.accounts
            .accept(&reset_code, reset_to("a brand new passphrase"))
            .unwrap_err(),
        AccountError::InvitationNotFound
    );
    cell.clock.store(real_now(), Ordering::Relaxed);
    cell.accounts
        .login(&member, PASSWORD.into())
        .expect("the expired reset changed nothing");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn links_are_made_only_for_an_https_or_loopback_origin() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-origin");
    let member = cell
        .join_person(
            &tenant,
            &format!("{}@example.com", unique("johnson")),
            "Katherine",
        )
        .await;
    let invitee = format!("{}@example.com", unique("vaughan"));

    let plain_http = cell.service(Some("http://myelin.example"));
    assert_eq!(
        plain_http
            .operator_join_link(&tenant, &invitee, None)
            .await
            .unwrap_err(),
        AccountError::LinksUnavailable
    );
    assert_eq!(
        plain_http.invite_member(&member, &invitee).unwrap_err(),
        AccountError::LinksUnavailable
    );
    assert_eq!(
        cell.count(
            "SELECT count(*) FROM auth_invitation WHERE tenant_id = $1",
            &tenant
        )
        .await,
        0,
        "no link was stored for an origin that would leak it"
    );

    let loopback = cell.service(Some("http://127.0.0.1:3000"));
    assert!(
        handed_over(loopback.invite_member(&member, &invitee).unwrap())
            .starts_with("http://127.0.0.1:3000/join/")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_sign_ins_are_throttled_exactly_and_a_success_clears_them() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-throttle");
    let email = format!("{}@example.com", unique("hamilton"));
    cell.join_person(&tenant, &email, "Margaret").await;
    let wrong = || cell.accounts.login(&email, "wrong password!".into());
    let right = || cell.accounts.login(&email, PASSWORD.into());

    for _ in 0..9 {
        assert_eq!(wrong().unwrap_err(), AccountError::LoginFailed);
    }
    right().expect("nine failures are under the limit");
    for attempt in 1..=10 {
        assert_eq!(
            wrong().unwrap_err(),
            AccountError::LoginFailed,
            "failure {attempt} after a success is still answered: the success cleared the count"
        );
    }
    assert_eq!(
        right().unwrap_err(),
        AccountError::RateLimited {
            retry_after_secs: 15 * 60
        },
        "the eleventh attempt in the window is refused, even with the right password"
    );
    cell.clock.fetch_add(15 * 60 - 1, Ordering::Relaxed);
    assert_eq!(
        right().unwrap_err(),
        AccountError::RateLimited {
            retry_after_secs: 1
        }
    );
    cell.clock.fetch_add(1, Ordering::Relaxed);
    right().expect("the window has ended");
    for _ in 0..10 {
        assert_eq!(wrong().unwrap_err(), AccountError::LoginFailed);
    }
    assert!(matches!(
        wrong().unwrap_err(),
        AccountError::RateLimited { .. }
    ));

    for _ in 0..12 {
        assert_eq!(
            cell.accounts
                .login("not an email", PASSWORD.into())
                .unwrap_err(),
            AccountError::LoginFailed,
            "a malformed email is neither counted nor limited"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_invitation_racing_a_reset_link_never_removes_it() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-issue-race");
    let member = cell
        .join_person(
            &tenant,
            &format!("{}@example.com", unique("lovelace")),
            "Ada",
        )
        .await;
    let invitee = format!("{}@example.com", unique("babbage"));
    let invitee_join = code_of(
        &cell
            .accounts
            .operator_join_link(&tenant, &invitee, None)
            .await
            .unwrap()
            .url,
    );

    // While the member's link is being issued, the invitee joins and the operator sends them a
    // reset link. The member's issuance must then see the account and leave the reset alone.
    let reset = Arc::new(Mutex::new(None::<String>));
    let fired = Arc::new(AtomicBool::new(false));
    let hooked = cell
        .service(Some("https://myelin.example"))
        .with_checkpoint({
            let (plain, reset, tenant, invitee) = (
                cell.accounts.clone(),
                reset.clone(),
                tenant.clone(),
                invitee.clone(),
            );
            let runtime = tokio::runtime::Handle::current();
            move |checkpoint| {
                if checkpoint != Checkpoint::BeforeIssuing || fired.swap(true, Ordering::SeqCst) {
                    return;
                }
                let (plain, tenant, invitee, code, runtime) = (
                    plain.clone(),
                    tenant.clone(),
                    invitee.clone(),
                    invitee_join.clone(),
                    runtime.clone(),
                );
                let url = std::thread::spawn(move || {
                    plain.accept(&code, join("Charles", PASSWORD)).unwrap();
                    runtime
                        .block_on(plain.operator_reset_link(&tenant, &invitee, None))
                        .unwrap()
                        .url
                })
                .join()
                .unwrap();
                *reset.lock().unwrap() = Some(code_of(&url));
            }
        });

    assert_eq!(
        hooked.invite_member(&member, &invitee).unwrap_err(),
        AccountError::AccountExists
    );
    let reset = reset.lock().unwrap().clone().expect("the checkpoint ran");
    assert_eq!(cell.accounts.invitation(&reset).unwrap().kind, "reset");
    cell.accounts
        .accept(&reset, reset_to("a brand new passphrase"))
        .expect("the reset link still works");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sign_in_racing_a_password_reset_does_not_use_the_old_password() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-login-race");
    let email = format!("{}@example.com", unique("goldberg"));
    cell.join_person(&tenant, &email, "Adele").await;
    let reset = code_of(
        &cell
            .accounts
            .operator_reset_link(&tenant, &email, None)
            .await
            .unwrap()
            .url,
    );

    let fired = Arc::new(AtomicBool::new(false));
    let hooked = cell
        .service(Some("https://myelin.example"))
        .with_checkpoint({
            let plain = cell.accounts.clone();
            move |checkpoint| {
                if checkpoint != Checkpoint::PasswordVerified || fired.swap(true, Ordering::SeqCst)
                {
                    return;
                }
                let (plain, reset) = (plain.clone(), reset.clone());
                std::thread::spawn(move || {
                    plain.accept(&reset, reset_to("a brand new passphrase"))
                })
                .join()
                .unwrap()
                .expect("the reset lands while the old password is being verified");
            }
        });

    assert_eq!(
        hooked.login(&email, PASSWORD.into()).unwrap_err(),
        AccountError::LoginFailed,
        "the old password no longer signs in once the reset has landed"
    );
    cell.accounts
        .login(&email, "a brand new passphrase".into())
        .expect("the new password works");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_disabled_persons_reset_link_does_not_change_their_password() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-disabled");
    let email = format!("{}@example.com", unique("wu"));
    let person = cell.join_person(&tenant, &email, "Chien-Shiung").await;
    let scope = myelin_storage::TenantScope::from_verified_token(&person, person.region.clone());
    let reset = code_of(
        &cell
            .accounts
            .operator_reset_link(&tenant, &email, None)
            .await
            .unwrap()
            .url,
    );
    let set_status = |status| {
        cell.principals
            .put_principal(
                &scope,
                person.principal_id.clone(),
                person.kind.clone(),
                person.data_role,
                status,
                None,
            )
            .unwrap();
    };

    set_status(PrincipalStatus::Disabled);
    assert_eq!(
        cell.accounts
            .accept(&reset, reset_to("a brand new passphrase"))
            .unwrap_err(),
        AccountError::InvitationNotFound
    );
    set_status(PrincipalStatus::Active);
    cell.accounts
        .login(&email, PASSWORD.into())
        .expect("the old password still works: the refused reset changed nothing");
    assert_eq!(
        cell.accounts
            .login(&email, "a brand new passphrase".into())
            .unwrap_err(),
        AccountError::LoginFailed
    );
}

/// Runs `attempts` sign-ins at once, released together, and returns what each one answered.
async fn burst_of_logins(
    accounts: &AccountService,
    email: &str,
    password: &str,
    attempts: usize,
) -> Vec<Result<myelin_identity::Principal, AccountError>> {
    let start = Arc::new(Barrier::new(attempts));
    let attempts: Vec<_> = (0..attempts)
        .map(|_| {
            let (accounts, start) = (accounts.clone(), start.clone());
            let (email, password) = (email.to_string(), password.to_string());
            tokio::task::spawn_blocking(move || {
                start.wait();
                accounts.login(&email, password)
            })
        })
        .collect();
    let mut answers = Vec::new();
    for attempt in attempts {
        answers.push(attempt.await.unwrap());
    }
    answers
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_of_wrong_passwords_at_one_email_is_verified_no_more_than_the_limit_allows() {
    const LIMIT: usize = 10;
    const BURST: usize = 40;
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-burst");
    let email = format!("{}@example.com", unique("curie"));
    cell.join_person(&tenant, &email, "Marie").await;

    let verified = Arc::new(AtomicUsize::new(0));
    let counting = cell
        .service(Some("https://myelin.example"))
        .with_checkpoint({
            let verified = verified.clone();
            move |checkpoint| {
                if checkpoint == Checkpoint::PasswordVerified {
                    verified.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
    let answers = burst_of_logins(&counting, &email, "wrong password!", BURST).await;

    let count = |wanted: fn(&AccountError) -> bool| {
        answers
            .iter()
            .filter(|answer| answer.as_ref().is_err_and(wanted))
            .count()
    };
    let failed = count(|error| *error == AccountError::LoginFailed);
    let limited = count(|error| matches!(error, AccountError::RateLimited { .. }));
    let busy = count(|error| *error == AccountError::Busy);
    assert_eq!(
        failed + limited + busy,
        BURST,
        "no other answer is possible"
    );
    assert!(
        verified.load(Ordering::SeqCst) <= LIMIT,
        "{} passwords were verified for one email in one window; the limit is {LIMIT}",
        verified.load(Ordering::SeqCst)
    );
    assert!(
        limited >= BURST - LIMIT - busy,
        "the attempts past the limit are refused, not verified (limited {limited})"
    );
    assert!(answers.iter().all(|answer| !matches!(
        answer,
        Err(AccountError::RateLimited { retry_after_secs }) if *retry_after_secs > 15 * 60
    )));

    assert!(matches!(
        cell.accounts.login(&email, PASSWORD.into()),
        Err(AccountError::RateLimited { .. })
    ));
    cell.clock.fetch_add(15 * 60, Ordering::Relaxed);
    cell.accounts
        .login(&email, PASSWORD.into())
        .expect("the right password works once the window has ended");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sign_in_turned_away_because_hashing_is_busy_keeps_its_slot() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-busy");
    let email = format!("{}@example.com", unique("tereshkova"));
    cell.join_person(&tenant, &email, "Valentina").await;

    // A gate with no capacity turns every attempt away after taking its slot.
    let saturated = cell
        .service(Some("https://myelin.example"))
        .with_hash_capacity(0);
    let answers = burst_of_logins(&saturated, &email, "wrong password!", 10).await;
    assert!(
        answers
            .iter()
            .all(|answer| matches!(answer, Err(AccountError::Busy))),
        "{:?}",
        answers.iter().map(|a| a.as_ref().err()).collect::<Vec<_>>()
    );

    for attempt in 1..=10 {
        assert_eq!(
            cell.accounts
                .login(&email, "wrong password!".into())
                .unwrap_err(),
            AccountError::LoginFailed,
            "attempt {attempt}: the busy refusals used none of the ten slots"
        );
    }
    assert!(matches!(
        cell.accounts.login(&email, "wrong password!".into()),
        Err(AccountError::RateLimited { .. })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slot_given_back_after_its_window_ended_leaves_the_next_window_alone() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-window");
    let email = format!("{}@example.com", unique("noether"));
    cell.join_person(&tenant, &email, "Emmy").await;

    // The saturated attempt takes its slot, then waits two seconds for the hash gate. Meanwhile
    // its window ends and a wrong guess opens the next one.
    let saturated = cell
        .service(Some("https://myelin.example"))
        .with_hash_capacity(0);
    let stale = tokio::task::spawn_blocking({
        let email = email.clone();
        move || saturated.login(&email, "wrong password!".into())
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    cell.clock.fetch_add(15 * 60, Ordering::Relaxed);
    assert_eq!(
        cell.accounts
            .login(&email, "wrong password!".into())
            .unwrap_err(),
        AccountError::LoginFailed
    );
    assert_eq!(stale.await.unwrap().unwrap_err(), AccountError::Busy);

    for attempt in 2..=10 {
        assert_eq!(
            cell.accounts
                .login(&email, "wrong password!".into())
                .unwrap_err(),
            AccountError::LoginFailed,
            "attempt {attempt}"
        );
    }
    assert!(
        matches!(
            cell.accounts.login(&email, "wrong password!".into()),
            Err(AccountError::RateLimited { .. })
        ),
        "the late give-back did not take back the new window's first hit"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_link_replaces_only_the_same_kind_for_the_same_email_in_the_same_organization() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let (org, other_org) = (unique("t-scope-a"), unique("t-scope-b"));
    let invitee = format!("{}@example.com", unique("pascal"));
    let teammate = format!("{}@example.com", unique("fermat"));
    let member = format!("{}@example.com", unique("euler"));
    cell.join_person(&org, &member, "Leonhard").await;
    let link = |tenant: &str, email: &str| {
        let accounts = cell.accounts.clone();
        let (tenant, email) = (tenant.to_string(), email.to_string());
        async move {
            code_of(
                &accounts
                    .operator_join_link(&tenant, &email, None)
                    .await
                    .unwrap()
                    .url,
            )
        }
    };

    let replaced = link(&org, &invitee).await;
    let another_email_same_org = link(&org, &teammate).await;
    let same_email_other_org = link(&other_org, &invitee).await;
    let reset_for_member = code_of(
        &cell
            .accounts
            .operator_reset_link(&org, &member, None)
            .await
            .unwrap()
            .url,
    );

    let replacement = link(&org, &invitee).await;

    assert_eq!(
        cell.accounts.invitation(&replaced).unwrap_err(),
        AccountError::InvitationNotFound,
        "the older link for this email in this organization is replaced"
    );
    assert!(cell.accounts.invitation(&replacement).is_ok());
    for (what, code, kind) in [
        (
            "another email, same organization",
            &another_email_same_org,
            "join",
        ),
        (
            "same email, another organization",
            &same_email_other_org,
            "join",
        ),
        ("a reset link for an account", &reset_for_member, "reset"),
    ] {
        assert_eq!(
            cell.accounts
                .invitation(code)
                .unwrap_or_else(|error| panic!("{what} was removed: {error}"))
                .kind,
            kind,
            "{what}"
        );
    }
    cell.accounts
        .accept(&another_email_same_org, join("Pierre", PASSWORD))
        .expect("the other email's link still creates its person");
    cell.accounts
        .accept(&same_email_other_org, join("Blaise", PASSWORD))
        .expect("the other organization's link still creates its person");
    cell.accounts
        .accept(&reset_for_member, reset_to("a brand new passphrase"))
        .expect("the reset link still works");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operators_new_reset_link_revokes_their_earlier_one_for_that_person_only() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let tenant = unique("t-reset-scope");
    let (first, second) = (
        format!("{}@example.com", unique("noyce")),
        format!("{}@example.com", unique("kilby")),
    );
    for (email, name) in [(&first, "Robert"), (&second, "Jack")] {
        cell.join_person(&tenant, email, name).await;
    }
    let pending_join = code_of(
        &cell
            .accounts
            .operator_join_link(&tenant, &format!("{}@example.com", unique("moore")), None)
            .await
            .unwrap()
            .url,
    );
    let reset = |email: String| {
        let accounts = cell.accounts.clone();
        let tenant = tenant.clone();
        async move {
            code_of(
                &accounts
                    .operator_reset_link(&tenant, &email, None)
                    .await
                    .unwrap()
                    .url,
            )
        }
    };

    let older = reset(first.clone()).await;
    let other_person = reset(second.clone()).await;
    let newer = reset(first.clone()).await;

    assert_eq!(
        cell.accounts.invitation(&older).unwrap_err(),
        AccountError::InvitationNotFound,
        "reissuing revokes the operator's earlier link, say one sent the wrong way"
    );
    cell.accounts
        .accept(&newer, reset_to("a brand new passphrase"))
        .expect("the newer link resets the password");
    assert!(
        cell.accounts.invitation(&other_person).is_ok(),
        "another person's reset link is untouched"
    );
    assert!(
        cell.accounts.invitation(&pending_join).is_ok(),
        "a join link for someone else is untouched"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_join_that_loses_the_email_inside_its_transaction_rolls_back_and_keeps_its_link() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let (tenant, rival_tenant) = (unique("t-taken"), unique("t-taken-rival"));
    let email = format!("{}@example.com", unique("bell"));
    let code = code_of(
        &cell
            .accounts
            .operator_join_link(&tenant, &email, None)
            .await
            .unwrap()
            .url,
    );
    let rival_code = code_of(
        &cell
            .accounts
            .operator_join_link(&rival_tenant, &email, None)
            .await
            .unwrap()
            .url,
    );

    // The address is free when the join starts, so it passes the early check; another
    // organization's person takes it before the join's own transaction inserts the account.
    let fired = Arc::new(AtomicBool::new(false));
    let hooked = cell
        .service(Some("https://myelin.example"))
        .with_checkpoint({
            let plain = cell.accounts.clone();
            move |checkpoint| {
                if checkpoint != Checkpoint::JoinKeyMade || fired.swap(true, Ordering::SeqCst) {
                    return;
                }
                let (plain, rival_code) = (plain.clone(), rival_code.clone());
                std::thread::spawn(move || plain.accept(&rival_code, join("Alexander", PASSWORD)))
                    .join()
                    .unwrap()
                    .expect("the rival takes the address first");
            }
        });

    assert_eq!(
        hooked.accept(&code, join("Graham", PASSWORD)).unwrap_err(),
        AccountError::AccountExists
    );
    assert!(
        cell.accounts.invitation(&code).is_ok(),
        "the refused join did not use up its link"
    );
    assert_eq!(
        cell.subject_keys(&tenant).await,
        0,
        "the key made for the rolled-back person was destroyed"
    );
    assert_eq!(
        cell.principal_rows(&tenant).await,
        0,
        "no half-created person"
    );
    assert_eq!(
        cell.count(
            "SELECT count(*) FROM auth_password_account WHERE tenant_id = $1",
            &tenant
        )
        .await,
        0
    );
    assert_eq!(
        cell.subject_keys(&rival_tenant).await,
        1,
        "the rival keeps theirs"
    );
}
