#![cfg(feature = "integration")]

//! Mail for password accounts against the real database and a fake SMTP server: a link exists
//! only if its mail is queued, queued mail is sealed, delivery retries and then deletes, and a
//! reset request never says whether an account exists.

use myelin_config::{Mode, MyelinConfig};
use myelin_edge::accounts::{Acceptance, MemberInvitation};
use myelin_edge::mail::{DeliveryPass, DeliverySchedule};
use myelin_edge::{
    account_migrations, AccountError, AccountService, AllowAll, EdgeRequest, EdgeResponse,
    EmailDigester, Gateway, MailOutbox, MailSettings, MailWorker,
};
use myelin_identity::{Principal, PrincipalStatus};
use myelin_identity_service::{
    CapabilityAuthenticator, CellTokenAuthority, HumanSsoAuthenticator, PasetoCapabilityVerifier,
    PrincipalStore, RevocationStore,
};
use myelin_storage::{
    all_durable_migrations, DurableKmsBacking, DurablePrincipalBacking, HotTables, KmsEngine,
    PgBootstrap, SealKey, SubstrateProvider,
};
use sha2::{Digest as _, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const SEAL_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const PASSWORD: &str = "correct horse battery";

/// Rate-limit counters and expired links are swept cell-wide, so the tests take turns.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

/// One message as the fake server saw it, and whether it said yes.
#[derive(Clone, Debug)]
struct Received {
    accepted: bool,
    raw: String,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.raw.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    /// The message with quoted-printable soft breaks and escapes undone. Headers are left alone
    /// where they hold no valid escape.
    fn decoded(&self) -> String {
        let joined = self.raw.replace("=\n", "");
        let mut decoded = Vec::with_capacity(joined.len());
        let mut rest = joined.as_bytes();
        while let Some((&byte, after)) = rest.split_first() {
            let escaped = after
                .get(..2)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok());
            match escaped {
                Some(value) if byte == b'=' => {
                    decoded.push(value);
                    rest = &after[2..];
                }
                _ => {
                    decoded.push(byte);
                    rest = after;
                }
            }
        }
        String::from_utf8(decoded).unwrap()
    }

    fn code(&self) -> String {
        let text = self.decoded();
        let start = text.find("/join/").expect("the message carries a link") + 6;
        text[start..start + 43].to_string()
    }
}

/// A minimal SMTP server on loopback that records every message. While `refuse` is set it
/// answers the end of the data with a temporary failure; while `hang` is set it never answers.
struct FakeSmtp {
    port: u16,
    refuse: Arc<AtomicBool>,
    hang: Arc<AtomicBool>,
    received: Arc<Mutex<Vec<Received>>>,
}

impl FakeSmtp {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let refuse = Arc::new(AtomicBool::new(false));
        let hang = Arc::new(AtomicBool::new(false));
        let received = Arc::new(Mutex::new(Vec::new()));
        let (session_refuse, session_hang, session_received) =
            (refuse.clone(), hang.clone(), received.clone());
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(session(
                    stream,
                    session_refuse.clone(),
                    session_hang.clone(),
                    session_received.clone(),
                ));
            }
        });
        Self {
            port,
            refuse,
            hang,
            received,
        }
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    fn delivered_to(&self, email: &str) -> Vec<Received> {
        self.received()
            .into_iter()
            .filter(|message| message.accepted && message.header("To") == Some(email))
            .collect()
    }
}

async fn session(
    stream: tokio::net::TcpStream,
    refuse: Arc<AtomicBool>,
    hang: Arc<AtomicBool>,
    received: Arc<Mutex<Vec<Received>>>,
) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let _ = write.write_all(b"220 fake ESMTP\r\n").await;
    let mut data: Option<String> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(body) = data.as_mut() {
            if line != "." {
                body.push_str(line.strip_prefix('.').unwrap_or(&line));
                body.push('\n');
                continue;
            }
            if hang.load(Ordering::SeqCst) {
                received.lock().unwrap().push(Received {
                    accepted: false,
                    raw: std::mem::take(body),
                });
                std::future::pending::<()>().await;
            }
            let accepted = !refuse.load(Ordering::SeqCst);
            received.lock().unwrap().push(Received {
                accepted,
                raw: std::mem::take(body),
            });
            data = None;
            let reply: &[u8] = if accepted {
                b"250 2.0.0 queued\r\n"
            } else {
                b"451 4.3.0 try again later\r\n"
            };
            let _ = write.write_all(reply).await;
            continue;
        }
        let verb = line
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let reply: &[u8] = match verb.as_str() {
            "EHLO" | "HELO" => b"250-fake\r\n250 8BITMIME\r\n",
            "MAIL" | "RCPT" | "RSET" | "NOOP" => b"250 2.0.0 ok\r\n",
            "DATA" => {
                data = Some(String::new());
                b"354 go ahead\r\n"
            }
            "QUIT" => {
                let _ = write.write_all(b"221 bye\r\n").await;
                return;
            }
            _ => b"502 5.5.2 unknown command\r\n",
        };
        let _ = write.write_all(reply).await;
    }
}

struct Cell {
    provider: SubstrateProvider,
    principals: PrincipalStore,
    seal: SealKey,
    cell_id: String,
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
        .expect("account migrations");
    let provider = bootstrap.into_runtime().await.expect("runtime role");
    let seal = SealKey::from_encoded(SEAL_HEX).expect("seal key");
    let cell_id = unique("cell-mail");
    let kms = Arc::new(
        DurableKmsBacking::new(provider.db_pool().clone(), cell_id.clone())
            .load_or_generate(&seal)
            .await
            .expect("KMS"),
    );
    let principals = PrincipalStore::with_pg(
        kms,
        DurablePrincipalBacking::new(provider.clone()),
        tokio::runtime::Handle::current(),
    );
    Cell {
        provider,
        principals,
        seal,
        cell_id,
    }
}

impl Cell {
    fn outbox(&self) -> MailOutbox {
        MailOutbox::new(
            self.cell_id.clone(),
            self.seal.derive_service_key(MailOutbox::SEAL_KEY_CONTEXT),
        )
    }

    fn accounts(&self, mail: bool) -> AccountService {
        let service = AccountService::new(
            self.provider.db_pool().clone(),
            self.principals.clone(),
            EmailDigester::new(
                self.seal
                    .derive_service_key(EmailDigester::SEAL_KEY_CONTEXT),
            ),
            self.provider.config().region.clone(),
            Some("https://myelin.example".into()),
            tokio::runtime::Handle::current(),
        );
        if mail {
            service.with_mail(self.outbox())
        } else {
            service
        }
    }

    fn worker(&self, smtp: &FakeSmtp, schedule: DeliverySchedule) -> MailWorker {
        let settings = MailSettings::parse(
            Some(format!("smtp://127.0.0.1:{}", smtp.port)),
            None,
            None,
            Some("Myelin <no-reply@myelin.test>".into()),
        )
        .unwrap()
        .unwrap();
        MailWorker::new(
            self.provider.db_pool().clone(),
            self.outbox(),
            &settings,
            schedule,
        )
        .unwrap()
    }

    async fn queued(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM edge_mail_outbox WHERE cell_id = $1")
            .bind(&self.cell_id)
            .fetch_one(self.provider.db_pool())
            .await
            .unwrap()
    }

    /// Queued letters that carry a link; a password-change notice carries none.
    async fn queued_links(&self) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM edge_mail_outbox \
              WHERE cell_id = $1 AND link_digest IS NOT NULL",
        )
        .bind(&self.cell_id)
        .fetch_one(self.provider.db_pool())
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

    fn set_status(&self, person: &Principal, status: PrincipalStatus) {
        let scope = myelin_storage::TenantScope::from_verified_token(person, person.region.clone());
        self.principals
            .put_principal(
                &scope,
                person.principal_id.clone(),
                person.kind.clone(),
                person.data_role,
                status,
                None,
            )
            .unwrap();
    }

    async fn invitations(&self, tenant: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM auth_invitation WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_one(self.provider.db_pool())
            .await
            .unwrap()
    }

    /// Lets the operator invite a first person and returns them, signed in.
    async fn person(&self, accounts: &AccountService, tenant: &str, email: &str) -> Principal {
        let link = accounts
            .operator_join_link(tenant, email, None)
            .await
            .unwrap()
            .url;
        let code = link.rsplit_once("/join/").unwrap().1.to_string();
        accounts
            .accept(
                &code,
                Acceptance {
                    display_name: Some("Ada".into()),
                    password: PASSWORD.into(),
                },
            )
            .unwrap()
    }
}

fn quick_retries() -> DeliverySchedule {
    DeliverySchedule {
        first_retry_secs: 2,
        max_retry_secs: 2,
        ..DeliverySchedule::default()
    }
}

/// Runs passes until the mail due within two seconds has been tried.
async fn deliver(worker: &MailWorker) -> DeliveryPass {
    let mut total = DeliveryPass::default();
    for _ in 0..3 {
        let pass = worker.deliver_due().await.unwrap();
        total.sent += pass.sent;
        total.retrying += pass.retrying;
        total.dropped += pass.dropped;
        if pass != DeliveryPass::default() {
            return total;
        }
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
    total
}

fn code_digest(code: &str) -> Vec<u8> {
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(code)
        .unwrap();
    Sha256::digest(raw).to_vec()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_emailed_invitation_reaches_only_the_invitee_and_its_link_works() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let smtp = FakeSmtp::start().await;
    let accounts = cell.accounts(true);
    let tenant = unique("t-mail-invite");
    let ada = cell
        .person(
            &accounts,
            &tenant,
            &format!("{}@example.com", unique("ada")),
        )
        .await;
    let grace = format!("{}@example.com", unique("grace"));

    let first = accounts.invite_member(&ada, &grace.to_uppercase()).unwrap();
    let MemberInvitation::Emailed {
        email,
        expires_at_unix,
    } = first
    else {
        panic!("with mail on the link is not handed to the inviter: {first:?}");
    };
    assert_eq!(email, grace);
    assert!(expires_at_unix > 0);
    // A second invitation to the same address replaces the first, and withdraws its mail.
    accounts.invite_member(&ada, &grace).unwrap();
    assert_eq!(
        cell.queued().await,
        1,
        "the superseded letter was withdrawn"
    );

    let worker = cell.worker(&smtp, quick_retries());
    assert_eq!(deliver(&worker).await.sent, 1);
    assert_eq!(cell.queued().await, 0, "a sent letter is deleted");
    let sent = smtp.delivered_to(&grace);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].header("Subject").unwrap().contains(&tenant));
    let code = sent[0].code();
    let view = accounts
        .invitation(&code)
        .expect("the emailed link is live");
    assert_eq!((view.kind, view.email.as_str()), ("join", grace.as_str()));
    let joined = accounts
        .accept(
            &code,
            Acceptance {
                display_name: Some("Grace".into()),
                password: "compile the future".into(),
            },
        )
        .unwrap();
    assert_eq!(joined.tenant.0, tenant);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_and_its_mail_are_stored_together_or_not_at_all() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let accounts = cell.accounts(true);
    let tenant = unique("t-mail-atomic");
    let ada = cell
        .person(
            &accounts,
            &tenant,
            &format!("{}@example.com", unique("ada")),
        )
        .await;
    let grace = format!("{}@example.com", unique("grace"));
    accounts.invite_member(&ada, &grace).unwrap();
    let links: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT code_digest FROM auth_invitation WHERE tenant_id = $1 AND kind = 'join'",
    )
    .bind(&tenant)
    .fetch_all(cell.provider.db_pool())
    .await
    .unwrap();
    let mailed: Vec<Vec<u8>> =
        sqlx::query_scalar("SELECT link_digest FROM edge_mail_outbox WHERE cell_id = $1")
            .bind(&cell.cell_id)
            .fetch_all(cell.provider.db_pool())
            .await
            .unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(
        mailed, links,
        "the queued letter carries exactly the stored link"
    );

    // An organization whose name cannot fit in a queued letter: the letter is refused, and the
    // link written just before it in the same transaction is rolled back with it.
    let oversized = format!("{}-{}", unique("t-oversized"), "x".repeat(17 * 1024));
    let inviter = ada.clone().in_tenant(&oversized);
    let refused = accounts.invite_member(&inviter, &format!("{}@example.com", unique("hedy")));
    assert!(
        matches!(&refused, Err(AccountError::Unavailable(detail))
            if detail.contains("edge_mail_outbox_payload_check")),
        "the letter, not the link, was refused: {refused:?}"
    );
    assert_eq!(
        cell.invitations(&oversized).await,
        0,
        "no link without its mail"
    );
    assert_eq!(cell.queued().await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_mail_is_sealed_at_rest() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let accounts = cell.accounts(true);
    let tenant = unique("t-mail-sealed");
    let email = format!("{}@example.com", unique("ada"));
    cell.person(&accounts, &tenant, &email).await;
    accounts.request_password_reset(&email).unwrap();

    let smtp = FakeSmtp::start().await;
    let (link_digest, payload): (Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT link_digest, payload FROM edge_mail_outbox WHERE cell_id = $1")
            .bind(&cell.cell_id)
            .fetch_one(cell.provider.db_pool())
            .await
            .unwrap();
    assert_eq!(deliver(&cell.worker(&smtp, quick_retries())).await.sent, 1);
    let code = smtp.delivered_to(&email)[0].code();
    let stored = String::from_utf8_lossy(&payload);
    for secret in [email.as_str(), code.as_str(), "myelin.example", "reset"] {
        assert!(!stored.contains(secret), "the payload shows {secret:?}");
    }
    assert_eq!(
        link_digest,
        code_digest(&code),
        "only the link's digest is clear"
    );

    // Another cell's outbox, even under the same seal key, neither claims nor opens it.
    accounts.request_password_reset(&email).unwrap();
    let stranger = MailOutbox::new(
        unique("cell-other"),
        cell.seal.derive_service_key(MailOutbox::SEAL_KEY_CONTEXT),
    );
    let settings = MailSettings::parse(
        Some(format!("smtp://127.0.0.1:{}", smtp.port)),
        None,
        None,
        Some("Myelin <no-reply@myelin.test>".into()),
    )
    .unwrap()
    .unwrap();
    let strange = MailWorker::new(
        cell.provider.db_pool().clone(),
        stranger,
        &settings,
        quick_retries(),
    )
    .unwrap();
    assert_eq!(
        strange.deliver_due().await.unwrap(),
        DeliveryPass::default()
    );
    assert_eq!(cell.queued().await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delivery_retries_after_a_refusal_keeps_its_message_id_and_then_deletes() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let smtp = FakeSmtp::start().await;
    let accounts = cell.accounts(true);
    let tenant = unique("t-mail-retry");
    let email = format!("{}@example.com", unique("ada"));
    cell.person(&accounts, &tenant, &email).await;
    accounts.request_password_reset(&email).unwrap();
    let worker = cell.worker(&smtp, quick_retries());

    smtp.refuse.store(true, Ordering::SeqCst);
    let refused = deliver(&worker).await;
    assert_eq!((refused.sent, refused.retrying), (0, 1));
    let attempts: i32 =
        sqlx::query_scalar("SELECT attempts FROM edge_mail_outbox WHERE cell_id = $1")
            .bind(&cell.cell_id)
            .fetch_one(cell.provider.db_pool())
            .await
            .unwrap();
    assert_eq!(attempts, 1, "the letter stays queued after a refusal");
    assert_eq!(
        worker.deliver_due().await.unwrap(),
        DeliveryPass::default(),
        "a refused letter waits for its retry"
    );

    smtp.refuse.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert_eq!(deliver(&worker).await.sent, 1);
    assert_eq!(cell.queued().await, 0);
    let tries: Vec<Received> = smtp
        .received()
        .into_iter()
        .filter(|message| message.header("To") == Some(email.as_str()))
        .collect();
    assert_eq!(
        tries
            .iter()
            .map(|message| message.accepted)
            .collect::<Vec<_>>(),
        [false, true]
    );
    let id = tries[0].header("Message-ID").expect("a Message-ID");
    assert!(id.ends_with("@myelin.test>"), "{id}");
    assert_eq!(
        tries[1].header("Message-ID"),
        Some(id),
        "a retry is the same message"
    );

    // A letter that is still refused when its time is up is dropped.
    let impatient = cell.worker(
        &smtp,
        DeliverySchedule {
            give_up_after_secs: 0,
            ..quick_retries()
        },
    );
    accounts.request_password_reset(&email).unwrap();
    smtp.refuse.store(true, Ordering::SeqCst);
    assert_eq!(deliver(&impatient).await.dropped, 1);
    assert_eq!(cell.queued().await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreadable_letter_waits_for_a_reader_and_an_old_letter_is_dropped_unsent() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let smtp = FakeSmtp::start().await;
    let accounts = cell.accounts(true);
    let email = format!("{}@example.com", unique("ada"));
    cell.person(&accounts, &unique("t-mail-old"), &email).await;
    // A row this edge cannot open, as a newer edge might write.
    sqlx::query(
        "INSERT INTO edge_mail_outbox \
           (message_id, cell_id, payload_nonce, payload, created_at_unix, next_attempt_unix) \
         VALUES ($1, $2, $3, $4, \
                 floor(extract(epoch FROM clock_timestamp()))::bigint, \
                 floor(extract(epoch FROM clock_timestamp()))::bigint)",
    )
    .bind([9_u8; 16].as_slice())
    .bind(&cell.cell_id)
    .bind([0_u8; 12].as_slice())
    .bind(b"not a sealed letter".as_slice())
    .execute(cell.provider.db_pool())
    .await
    .unwrap();
    let worker = cell.worker(&smtp, quick_retries());
    let pass = deliver(&worker).await;
    assert_eq!((pass.retrying, pass.dropped), (1, 0), "{pass:?}");
    assert_eq!(cell.queued().await, 1);

    // Letters queued longer than a day are dropped before anything is sent.
    accounts.request_password_reset(&email).unwrap();
    sqlx::query(
        "UPDATE edge_mail_outbox SET created_at_unix = created_at_unix - 2 * 86400, \
                next_attempt_unix = 0 \
          WHERE cell_id = $1",
    )
    .bind(&cell.cell_id)
    .execute(cell.provider.db_pool())
    .await
    .unwrap();
    assert_eq!(deliver(&worker).await.dropped, 2);
    assert_eq!(cell.queued().await, 0);
    assert!(smtp.received().is_empty(), "nothing was sent");
}

fn gateway(accounts: AccountService) -> Gateway {
    let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell");
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        PrincipalStore::new(Arc::new(KmsEngine::new())),
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
        Arc::new(KmsEngine::new()),
    )));
    Gateway::builder(authn, human, Arc::new(AllowAll))
        .with_accounts(accounts)
        .build()
}

fn forgot(gateway: &Gateway, email: &str) -> EdgeResponse {
    gateway.handle(EdgeRequest::new(
        "POST",
        "/v1/auth/password/forgot",
        "",
        vec![],
        serde_json::to_vec(&serde_json::json!({ "email": email })).unwrap(),
    ))
}

fn shape(response: &EdgeResponse) -> (u16, Vec<(String, String)>, Vec<u8>) {
    match response {
        EdgeResponse::Bytes {
            status,
            headers,
            body,
            ..
        } => (*status, headers.clone(), body.clone()),
        EdgeResponse::Sse { .. } => panic!("not an event stream"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forgot_password_answers_alike_and_mails_only_a_capped_few() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let smtp = FakeSmtp::start().await;
    let accounts = cell.accounts(true);
    let worker = cell.worker(&smtp, quick_retries());
    let tenant = unique("t-mail-forgot");
    let ada = format!("{}@example.com", unique("ada"));
    cell.person(&accounts, &tenant, &ada).await;
    let throttled = format!("{}@example.com", unique("throttled"));
    cell.person(&accounts, &tenant, &throttled).await;
    for _ in 0..3 {
        accounts.request_password_reset(&throttled).unwrap();
        deliver(&worker).await;
    }
    assert_eq!(smtp.delivered_to(&throttled).len(), 3);

    let gateway = gateway(accounts.clone());
    let answers = [
        forgot(&gateway, &ada.to_uppercase()),
        forgot(&gateway, &format!("{}@example.com", unique("nobody"))),
        forgot(&gateway, &throttled),
        forgot(&gateway, "not an email"),
    ]
    .map(|response| shape(&response));
    assert_eq!(answers[0].0, 202);
    assert!(answers[0]
        .1
        .contains(&("cache-control".to_string(), "no-store".to_string())));
    for other in &answers[1..] {
        assert_eq!(other, &answers[0], "every email gets the same answer");
    }

    // The answer came before the work; the work then mails only the existing, unthrottled one.
    for _ in 0..50 {
        if cell.queued().await == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(deliver(&worker).await.sent, 1);
    let reset = smtp.delivered_to(&ada);
    assert_eq!(reset.len(), 1);
    assert_eq!(
        smtp.delivered_to(&throttled).len(),
        3,
        "the fourth was dropped"
    );
    assert!(reset[0].header("Subject").unwrap().contains("Reset"));

    // Resetting sends the owner a notice, queued with the change.
    let code = reset[0].code();
    accounts
        .accept(
            &code,
            Acceptance {
                display_name: None,
                password: "a fresh passphrase".into(),
            },
        )
        .unwrap();
    assert_eq!(cell.queued().await, 1);
    assert_eq!(deliver(&worker).await.sent, 1);
    let notices = smtp.delivered_to(&ada);
    assert_eq!(notices.len(), 2);
    assert!(notices[1].header("Subject").unwrap().contains("changed"));
    assert!(notices[1]
        .decoded()
        .contains("\n\nhttps://myelin.example/login?help=password\n"));
    accounts.login(&ada, "a fresh passphrase".into()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_mail_off_links_go_to_the_inviter_and_nothing_is_queued() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let accounts = cell.accounts(false);
    assert!(!accounts.mail_enabled());
    let tenant = unique("t-mail-off");
    let ada = format!("{}@example.com", unique("ada"));
    let member = cell.person(&accounts, &tenant, &ada).await;

    let invited = accounts
        .invite_member(&member, &format!("{}@example.com", unique("grace")))
        .unwrap();
    assert!(
        matches!(&invited, MemberInvitation::HandedOver(issued)
            if issued.url.starts_with("https://myelin.example/join/")),
        "{invited:?}"
    );
    accounts.request_password_reset(&ada).unwrap();
    assert_eq!(cell.invitations(&tenant).await, 1, "no reset link was made");

    let gateway = gateway(accounts.clone());
    assert_eq!(forgot(&gateway, &ada).status(), 503);
    let config = gateway.handle(EdgeRequest::new(
        "GET",
        "/v1/auth/config",
        "",
        vec![],
        vec![],
    ));
    let config = config.json_body().unwrap();
    assert_eq!(config["password_login_enabled"], true);
    assert_eq!(config["mail_delivery_enabled"], false);

    let reset = accounts
        .operator_reset_link(&tenant, &ada, None)
        .await
        .unwrap();
    let code = reset.url.rsplit_once("/join/").unwrap().1;
    accounts
        .accept(
            code,
            Acceptance {
                display_name: None,
                password: "a fresh passphrase".into(),
            },
        )
        .unwrap();
}

fn new_password(password: &str) -> Acceptance {
    Acceptance {
        display_name: None,
        password: password.into(),
    }
}

fn code_in(url: &str) -> String {
    url.rsplit_once("/join/").unwrap().1.to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_anonymous_reset_request_leaves_an_operator_reset_link_working() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let smtp = FakeSmtp::start().await;
    let accounts = cell.accounts(true);
    let tenant = unique("t-mail-operator-reset");
    let email = format!("{}@example.com", unique("ada"));
    cell.person(&accounts, &tenant, &email).await;
    let handed = code_in(
        &accounts
            .operator_reset_link(&tenant, &email, None)
            .await
            .unwrap()
            .url,
    );

    accounts.request_password_reset(&email).unwrap();
    assert_eq!(deliver(&cell.worker(&smtp, quick_retries())).await.sent, 1);
    let emailed = smtp.delivered_to(&email)[0].code();
    assert!(accounts.invitation(&emailed).is_ok());
    accounts
        .accept(&handed, new_password("an operator's fresh start"))
        .expect("the operator's link still works after someone asked for another");
    assert_eq!(
        accounts.invitation(&emailed).unwrap_err(),
        AccountError::InvitationNotFound,
        "the new password ends the other reset links"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepting_one_reset_link_ends_the_others_and_withdraws_all_their_unsent_mail() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let smtp = FakeSmtp::start().await;
    let accounts = cell.accounts(true);
    let tenant = unique("t-mail-reset-ends");
    let email = format!("{}@example.com", unique("ada"));
    cell.person(&accounts, &tenant, &email).await;
    let worker = cell.worker(&smtp, quick_retries());

    // The first letter is refused, so it stays queued; the fake server still saw its link.
    accounts.request_password_reset(&email).unwrap();
    smtp.refuse.store(true, Ordering::SeqCst);
    assert_eq!(deliver(&worker).await.retrying, 1);
    let used = smtp.received()[0].code();
    accounts.request_password_reset(&email).unwrap();
    let handed = code_in(
        &accounts
            .operator_reset_link(&tenant, &email, None)
            .await
            .unwrap()
            .url,
    );
    assert_eq!(cell.queued_links().await, 2);

    accounts
        .accept(&used, new_password("a fresh passphrase"))
        .unwrap();
    assert_eq!(
        accounts.invitation(&handed).unwrap_err(),
        AccountError::InvitationNotFound
    );
    assert_eq!(
        cell.queued_links().await,
        0,
        "no letter still carries a reset link, the used one included"
    );
    assert_eq!(cell.queued().await, 1, "only the change notice is queued");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_used_invitation_withdraws_its_unsent_mail() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let smtp = FakeSmtp::start().await;
    let accounts = cell.accounts(true);
    let tenant = unique("t-mail-join-used");
    let ada = cell
        .person(
            &accounts,
            &tenant,
            &format!("{}@example.com", unique("ada")),
        )
        .await;
    let grace = format!("{}@example.com", unique("grace"));
    accounts.invite_member(&ada, &grace).unwrap();
    smtp.refuse.store(true, Ordering::SeqCst);
    assert_eq!(
        deliver(&cell.worker(&smtp, quick_retries())).await.retrying,
        1
    );
    assert_eq!(cell.queued().await, 1);

    let code = smtp.received()[0].code();
    accounts
        .accept(
            &code,
            Acceptance {
                display_name: Some("Grace".into()),
                password: "compile the future".into(),
            },
        )
        .unwrap();
    assert_eq!(
        cell.queued().await,
        0,
        "the used link's letter was withdrawn"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mail_whose_link_has_expired_is_dropped_unsent() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let smtp = FakeSmtp::start().await;
    let accounts = cell.accounts(true);
    let tenant = unique("t-mail-expired-link");
    let email = format!("{}@example.com", unique("ada"));
    cell.person(&accounts, &tenant, &email).await;
    accounts.request_password_reset(&email).unwrap();
    cell.expire_links(&tenant).await;

    let pass = deliver(&cell.worker(&smtp, quick_retries())).await;
    assert_eq!((pass.sent, pass.dropped), (0, 1), "{pass:?}");
    assert_eq!(cell.queued().await, 0);
    assert!(smtp.received().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_workers_racing_send_each_letter_once() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let smtp = FakeSmtp::start().await;
    let accounts = cell.accounts(true);
    let tenant = unique("t-mail-race");
    let ada = cell
        .person(
            &accounts,
            &tenant,
            &format!("{}@example.com", unique("ada")),
        )
        .await;
    let invitees: Vec<String> = (0..12)
        .map(|n| format!("{}-{n}@example.com", unique("invitee")))
        .collect();
    for invitee in &invitees {
        accounts.invite_member(&ada, invitee).unwrap();
    }

    let (first, second) = (
        cell.worker(&smtp, quick_retries()),
        cell.worker(&smtp, quick_retries()),
    );
    let (first, second) = tokio::join!(first.deliver_due(), second.deliver_due());
    let (first, second) = (first.unwrap(), second.unwrap());
    assert_eq!(first.sent + second.sent, invitees.len());
    assert_eq!(cell.queued().await, 0);
    for invitee in &invitees {
        assert_eq!(smtp.delivered_to(invitee).len(), 1, "{invitee}");
    }
    assert_eq!(
        smtp.received().len(),
        invitees.len(),
        "nothing was sent twice"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_claim_that_runs_out_mid_send_is_taken_over_and_nothing_is_lost() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let (stuck, healthy) = (FakeSmtp::start().await, FakeSmtp::start().await);
    stuck.hang.store(true, Ordering::SeqCst);
    let accounts = cell.accounts(true);
    let tenant = unique("t-mail-lease");
    let email = format!("{}@example.com", unique("ada"));
    cell.person(&accounts, &tenant, &email).await;
    accounts.request_password_reset(&email).unwrap();

    let short_claim = DeliverySchedule {
        claim_secs: 2,
        ..quick_retries()
    };
    let stalled = cell.worker(&stuck, short_claim);
    let stalled = tokio::spawn(async move { stalled.deliver_due().await });
    // Until the claim runs out, another worker finds nothing to do.
    while stuck.received().is_empty() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let rescuer = cell.worker(&healthy, quick_retries());
    assert_eq!(
        rescuer.deliver_due().await.unwrap(),
        DeliveryPass::default()
    );

    tokio::time::sleep(Duration::from_millis(2_100)).await;
    assert_eq!(rescuer.deliver_due().await.unwrap().sent, 1);
    stalled.abort();
    assert_eq!(cell.queued().await, 0);
    assert_eq!(
        stuck.received().len(),
        1,
        "the stalled send got as far as the data"
    );
    assert_eq!(
        healthy.delivered_to(&email).len(),
        1,
        "and the letter still arrived"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_reset_queues_no_change_notice() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let cell = cell().await;
    let accounts = cell.accounts(true);
    let tenant = unique("t-mail-refused-reset");
    let email = format!("{}@example.com", unique("ada"));
    let person = cell.person(&accounts, &tenant, &email).await;
    let reset = code_in(
        &accounts
            .operator_reset_link(&tenant, &email, None)
            .await
            .unwrap()
            .url,
    );

    cell.set_status(&person, PrincipalStatus::Disabled);
    assert_eq!(
        accounts
            .accept(&reset, new_password("a fresh passphrase"))
            .unwrap_err(),
        AccountError::InvitationNotFound
    );
    assert_eq!(
        cell.queued().await,
        0,
        "an inactive person's reset sends no notice"
    );

    cell.set_status(&person, PrincipalStatus::Active);
    cell.expire_links(&tenant).await;
    assert_eq!(
        accounts
            .accept(&reset, new_password("a fresh passphrase"))
            .unwrap_err(),
        AccountError::InvitationNotFound
    );
    assert_eq!(cell.queued().await, 0, "an expired link sends no notice");
    accounts.login(&email, PASSWORD.into()).unwrap();
}

trait InTenant {
    fn in_tenant(self, tenant: &str) -> Self;
}

impl InTenant for Principal {
    fn in_tenant(mut self, tenant: &str) -> Self {
        self.tenant = myelin_tenancy::TenantId(tenant.into());
        self
    }
}
