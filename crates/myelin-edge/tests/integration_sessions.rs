#![cfg(feature = "integration")]

//! Ending sessions against the real database: a password reset or change, signing out, and
//! signing out everywhere each end exactly the sessions they should, for every kind of human
//! session the Edge issues.

use myelin_config::{Mode, MyelinConfig};
use myelin_edge::accounts::{Acceptance, Checkpoint};
use myelin_edge::catalogue::Method;
use myelin_edge::{
    account_migrations, device_authorization_migrations, AccountError, AccountKeys, AccountService,
    AllowAll, DeviceAuthorizationBroker, EdgeRequest, Gateway, WhoamiHandler,
};
use myelin_identity::{Credential, RevokeTarget};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, CredentialAudience,
    CredentialPurpose, HumanSsoAuthenticator, PasetoCapabilityVerifier, PrincipalStore,
    RevocationStore,
};
use myelin_storage::{
    all_durable_migrations, with_tenant_tx, DurableKmsBacking, DurablePrincipalBacking,
    DurableRevocationBacking, HotTables, PgBootstrap, SealKey, SubstrateProvider, TenantScope,
};
use myelin_tenancy::{Region, TenantId};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use std::sync::Arc;

const SEAL_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const PASSWORD: &str = "correct horse battery";
const NEW_PASSWORD: &str = "a brand new passphrase";

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

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

struct Stack {
    provider: SubstrateProvider,
    principals: PrincipalStore,
    seal: SealKey,
    accounts: AccountService,
    cell: Arc<CellTokenAuthority>,
    authn: Arc<CapabilityAuthenticator>,
    gateway: Gateway,
}

async fn stack() -> Stack {
    let config = MyelinConfig::from_env(Mode::DevDefaults).expect("dev config");
    let bootstrap = PgBootstrap::connect(config, 8).await.expect("connect");
    bootstrap.migrate_foundation().await.expect("foundation");
    for migrations in [
        all_durable_migrations(),
        account_migrations(),
        device_authorization_migrations(),
    ] {
        bootstrap
            .migrate(&migrations, &HotTables::none())
            .await
            .expect("migrations");
    }
    let provider = bootstrap.into_runtime().await.expect("runtime role");
    let runtime = tokio::runtime::Handle::current();
    let seal = SealKey::from_encoded(SEAL_HEX).expect("seal key");
    let kms = Arc::new(
        DurableKmsBacking::new(provider.db_pool().clone(), unique("cell-sessions"))
            .load_or_generate(&seal)
            .await
            .expect("KMS"),
    );
    let principals = PrincipalStore::with_pg(
        kms,
        DurablePrincipalBacking::new(provider.clone()),
        runtime.clone(),
    );
    let accounts = account_service(&provider, &principals, &seal);
    let cell = Arc::new(CellTokenAuthority::generate());
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        principals.clone(),
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::with_pg(
            DurableRevocationBacking::new(provider.clone()),
            runtime.clone(),
        ),
    ));
    let human = Arc::new(HumanSsoAuthenticator::production(principals.clone()));
    // The Edge's own clock runs half a minute ahead of the database's; session issue times must
    // not follow it.
    let devices = DeviceAuthorizationBroker::with_pg(
        provider.db_pool().clone(),
        runtime,
        "https://myelin.example/cli/auth",
    )
    .expect("device broker")
    .with_clock(|| now() + 30);
    let gateway = Gateway::builder(authn.clone(), human, Arc::new(AllowAll))
        .default_token_scheme("session")
        .with_human_session_issuer(cell.clone())
        .with_accounts(accounts.clone())
        .with_device_authorization(devices)
        .route(
            Method::Get,
            "/v1/whoami",
            "edge.whoami",
            Arc::new(WhoamiHandler::default()),
        )
        .build();
    Stack {
        provider,
        principals,
        seal,
        accounts,
        cell,
        authn,
        gateway,
    }
}

fn account_service(
    provider: &SubstrateProvider,
    principals: &PrincipalStore,
    seal: &SealKey,
) -> AccountService {
    AccountService::new(
        provider.db_pool().clone(),
        principals.clone(),
        AccountKeys::derive(seal),
        provider.config().region.clone(),
        Some("https://myelin.example".into()),
        tokio::runtime::Handle::current(),
    )
}

impl Stack {
    fn call(&self, method: &str, path: &str, token: Option<&str>, body: Value) -> (u16, Value) {
        let headers = token
            .map(|token| vec![("authorization".into(), format!("Bearer {token}"))])
            .unwrap_or_default();
        let response = self.gateway.handle(EdgeRequest::new(
            method,
            path,
            "",
            headers,
            serde_json::to_vec(&body).unwrap(),
        ));
        (
            response.status(),
            response.json_body().unwrap_or(Value::Null),
        )
    }

    fn whoami(&self, token: &str) -> u16 {
        self.call("GET", "/v1/whoami", Some(token), Value::Null).0
    }

    fn sign_in(&self, email: &str, password: &str) -> String {
        let (status, body) = self.call(
            "POST",
            "/v1/auth/password/login",
            None,
            json!({ "email": email, "password": password }),
        );
        assert_eq!(status, 200, "{body}");
        token_of(&body)
    }

    fn sign_in_status(&self, email: &str, password: &str) -> u16 {
        self.call(
            "POST",
            "/v1/auth/password/login",
            None,
            json!({ "email": email, "password": password }),
        )
        .0
    }

    fn accept(&self, url: &str, body: Value) -> String {
        let code = url.rsplit_once("/join/").expect("a link").1;
        let (status, body) = self.call(
            "POST",
            &format!("/v1/auth/invitations/{code}/accept"),
            None,
            body,
        );
        assert_eq!(status, 200, "{body}");
        token_of(&body)
    }

    /// A new person in a new organization, and the session their join gave them.
    async fn person(&self, email: &str) -> (Principal, String) {
        let tenant = unique("t-sessions");
        let link = self
            .accounts
            .operator_join_link(&tenant, email, None)
            .await
            .unwrap();
        let token = self.accept(
            &link.url,
            json!({ "display_name": "Ada", "password": PASSWORD }),
        );
        let principal = self
            .accounts
            .login(email, PASSWORD.into(), None)
            .unwrap()
            .principal;
        (principal, token)
    }

    /// A CLI session from the device flow, approved by `browser`.
    fn device_session(&self, browser: &str) -> String {
        use base64::Engine as _;
        let verifier =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        let (status, started) = self.call(
            "POST",
            "/v1/auth/device/authorization",
            None,
            json!({ "code_challenge": challenge }),
        );
        assert_eq!(status, 201, "{started}");
        let (status, body) = self.call(
            "POST",
            "/v1/auth/device/approval",
            Some(browser),
            json!({ "user_code": started["user_code"] }),
        );
        assert_eq!(status, 200, "{body}");
        let (status, claimed) = self.call(
            "POST",
            "/v1/auth/device/token",
            None,
            json!({ "device_code": started["device_code"], "code_verifier": verifier }),
        );
        assert_eq!(status, 200, "{claimed}");
        token_of(&claimed)
    }

    /// A session as an SSO sign-in mints it: issued `issued_at_unix`, by the Edge's clock.
    fn sso_session(&self, person: &Principal, issued_at_unix: i64) -> String {
        self.cell.mint(&CapabilityMintSpec {
            tenant: person.tenant.0.clone(),
            region: person.region.0.clone(),
            subject_key: person.principal_id.0.clone(),
            jti: unique("session-sso"),
            issued_at_unix: Some(issued_at_unix),
            exp_unix: now() + 3600,
            authority: myelin_edge::authz::human_session_authority(),
            dpop_jkt: None,
            purpose: CredentialPurpose::HumanSession,
            audience: CredentialAudience::Edge,
        })
    }

    fn change_password(&self, token: &str, current: &str, new: &str) -> (u16, Value) {
        self.call(
            "POST",
            "/v1/auth/password/change",
            Some(token),
            json!({ "current_password": current, "new_password": new }),
        )
    }

    /// Sets the person's cutoff directly. Only this test's own person row is touched.
    async fn set_cutoff(&self, person: &Principal, cutoff: i64) {
        let (tenant, id) = (person.tenant.0.clone(), person.principal_id.0.clone());
        with_tenant_tx(
            self.provider.db_pool(),
            &person.tenant.0,
            &person.region.0,
            move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "UPDATE principal SET sessions_not_before = $3 \
                          WHERE tenant_id = $1 AND principal_id = $2",
                    )
                    .bind(&tenant)
                    .bind(&id)
                    .bind(cutoff)
                    .execute(&mut *conn)
                    .await
                    .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
                })
            },
        )
        .await
        .unwrap();
    }

    async fn jti_revocation_expiries(&self, person: &Principal) -> Vec<Option<String>> {
        let tenant = person.tenant.0.clone();
        with_tenant_tx(
            self.provider.db_pool(),
            &person.tenant.0,
            &person.region.0,
            move |conn| {
                Box::pin(async move {
                    sqlx::query_scalar(
                        "SELECT expires_at FROM revocation WHERE tenant_id = $1 AND kind = 'jti'",
                    )
                    .bind(&tenant)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
                })
            },
        )
        .await
        .unwrap()
    }
}

fn token_of(body: &Value) -> String {
    body["access_token"]
        .as_str()
        .unwrap_or_else(|| panic!("a session: {body}"))
        .to_string()
}

fn email() -> String {
    format!("{}@example.com", unique("ada"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_password_reset_ends_every_kind_of_session_the_person_holds() {
    let stack = stack().await;
    let email = email();
    let (person, joined) = stack.person(&email).await;
    let password = stack.sign_in(&email, PASSWORD);
    let cli = stack.device_session(&password);
    let sso = stack.sso_session(&person, now() - 1);
    for token in [&joined, &password, &cli, &sso] {
        assert_eq!(stack.whoami(token), 200);
    }

    let link = stack
        .accounts
        .operator_reset_link(&person.tenant.0, &email, None)
        .await
        .unwrap();
    let after_reset = stack.accept(&link.url, json!({ "password": NEW_PASSWORD }));

    for (kind, token) in [
        ("join", &joined),
        ("password", &password),
        ("CLI", &cli),
        ("SSO", &sso),
    ] {
        assert_eq!(stack.whoami(token), 401, "the {kind} session ended");
    }
    assert_eq!(
        stack.whoami(&after_reset),
        200,
        "the reset signs the person in"
    );
    assert_eq!(stack.sign_in_status(&email, PASSWORD), 401);
    assert_eq!(stack.whoami(&stack.sign_in(&email, NEW_PASSWORD)), 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changing_the_password_keeps_only_the_session_it_returns() {
    let stack = stack().await;
    let email = email();
    let (_person, laptop) = stack.person(&email).await;
    let phone = stack.sign_in(&email, PASSWORD);
    let cli = stack.device_session(&phone);

    let (status, body) = stack.change_password(&laptop, PASSWORD, "too short");
    assert_eq!(
        (status, &body["error"]["code"]),
        (422, &json!("weak_password"))
    );
    assert_eq!(stack.whoami(&phone), 200, "a refused change ends nothing");

    let (status, body) = stack.change_password(&laptop, PASSWORD, NEW_PASSWORD);
    assert_eq!(status, 200, "{body}");
    let fresh = token_of(&body);
    assert_eq!(stack.whoami(&fresh), 200, "the returned session works");
    for (device, token) in [("laptop", &laptop), ("phone", &phone), ("CLI", &cli)] {
        assert_eq!(stack.whoami(token), 401, "the {device} session ended");
    }
    assert_eq!(stack.sign_in_status(&email, PASSWORD), 401);
    stack.sign_in(&email, NEW_PASSWORD);

    let (status, body) = stack.change_password(&fresh, PASSWORD, "yet another passphrase");
    assert_eq!(
        (status, &body["error"]["code"]),
        (403, &json!("wrong_password")),
        "the old password no longer changes anything"
    );
    assert_eq!(
        stack.whoami(&fresh),
        200,
        "a wrong password is not a signed-out session"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_current_password_counts_against_the_sign_in_allowance() {
    let stack = stack().await;
    let email = email();
    let (_person, session) = stack.person(&email).await;
    for attempt in 1..=10 {
        let (status, _) = stack.change_password(&session, "not my password", NEW_PASSWORD);
        assert_eq!(status, 403, "attempt {attempt}");
    }
    let (status, _) = stack.change_password(&session, PASSWORD, NEW_PASSWORD);
    assert_eq!(status, 429, "even the right password waits out the window");
    assert_eq!(
        stack.sign_in_status(&email, PASSWORD),
        429,
        "sign-in shares the allowance"
    );
    assert_eq!(stack.whoami(&session), 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signing_out_ends_exactly_that_session_until_it_would_have_expired() {
    let stack = stack().await;
    let email = email();
    let (person, laptop) = stack.person(&email).await;
    let (status, body) = stack.call(
        "POST",
        "/v1/auth/password/login",
        None,
        json!({ "email": email, "password": PASSWORD }),
    );
    assert_eq!(status, 200);
    let phone = token_of(&body);
    let phone_expiry = body["expires_at"].as_i64().unwrap();

    let (status, _) = stack.call("POST", "/v1/auth/logout", Some(&phone), json!({}));
    assert_eq!(status, 200);
    assert_eq!(stack.whoami(&phone), 401);
    assert_eq!(stack.whoami(&laptop), 200, "the other session goes on");
    let (status, _) = stack.call("POST", "/v1/auth/logout", Some(&phone), json!({}));
    assert_eq!(status, 401, "an ended session cannot sign out again");

    let expiries = stack.jti_revocation_expiries(&person).await;
    assert_eq!(expiries.len(), 1, "one session was denied");
    let expiry = chrono::DateTime::parse_from_rfc3339(expiries[0].as_deref().unwrap())
        .unwrap()
        .timestamp();
    assert_eq!(
        expiry, phone_expiry,
        "the denial lasts as long as the session would have"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signing_out_everywhere_ends_every_session_and_a_new_sign_in_works() {
    let stack = stack().await;
    let email = email();
    let (person, laptop) = stack.person(&email).await;
    let phone = stack.sign_in(&email, PASSWORD);
    let cli = stack.device_session(&phone);
    let sso = stack.sso_session(&person, now() - 1);

    let (status, body) = stack.call(
        "POST",
        "/v1/auth/logout/everywhere",
        Some(&laptop),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    for token in [&laptop, &phone, &cli, &sso] {
        assert_eq!(stack.whoami(token), 401);
    }
    // Usually within the same second as the cutoff: the sign-in still counts as after it.
    assert_eq!(stack.whoami(&stack.sign_in(&email, PASSWORD)), 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cutoff_refuses_sessions_issued_before_its_second_and_admits_its_own() {
    let stack = stack().await;
    let email = email();
    let (person, _) = stack.person(&email).await;
    let cutoff = now() + 30;
    stack.set_cutoff(&person, cutoff).await;

    assert_eq!(stack.whoami(&stack.sso_session(&person, cutoff - 1)), 401);
    let at_cutoff = stack.sso_session(&person, cutoff);
    assert_eq!(
        stack.whoami(&at_cutoff),
        200,
        "issued in the cutoff's second"
    );
    assert_eq!(stack.whoami(&stack.sso_session(&person, cutoff + 1)), 200);

    let signed_in = stack.sign_in(&email, PASSWORD);
    assert_eq!(
        stack.whoami(&signed_in),
        200,
        "a sign-in after the cutoff counts as issued at it, even by a clock behind it"
    );

    // A new cutoff always passes the old one, so a session issued at the old one ends too.
    let (status, _) = stack.call(
        "POST",
        "/v1/auth/logout/everywhere",
        Some(&at_cutoff),
        json!({}),
    );
    assert_eq!(status, 200);
    assert_eq!(stack.whoami(&at_cutoff), 401);
    assert_eq!(stack.whoami(&signed_in), 401);
    assert_eq!(stack.whoami(&stack.sso_session(&person, cutoff + 1)), 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_disabled_person_is_refused_whatever_session_they_hold() {
    let stack = stack().await;
    let email = email();
    let (person, joined) = stack.person(&email).await;
    let fresh = stack.sso_session(&person, now() + 5);
    let scope = TenantScope::from_verified_token(
        &Principal::stub(
            PrincipalId("test".into()),
            PrincipalKind::Human,
            TenantId(person.tenant.0.clone()),
        ),
        Region(person.region.0.clone()),
    );
    stack
        .principals
        .put_principal(
            &scope,
            person.principal_id.clone(),
            PrincipalKind::Human,
            person.data_role,
            PrincipalStatus::Disabled,
            None,
        )
        .unwrap();
    assert_eq!(stack.whoami(&joined), 401);
    assert_eq!(
        stack.whoami(&fresh),
        401,
        "a session past any cutoff is still refused"
    );
    assert_eq!(stack.sign_in_status(&email, PASSWORD), 401);
    let (status, _) = stack.call(
        "POST",
        "/v1/auth/logout/everywhere",
        Some(&fresh),
        json!({}),
    );
    assert_eq!(status, 401);
}

impl Stack {
    fn service(&self) -> AccountService {
        account_service(&self.provider, &self.principals, &self.seal)
    }

    fn identity(&self, token: &str) -> myelin_identity_service::RequestIdentity {
        self.authn
            .authenticate_identity(
                &Credential {
                    scheme: "session".into(),
                    material: token.into(),
                },
                None,
            )
            .expect("a live session")
    }

    fn set_status(&self, person: &Principal, status: PrincipalStatus) {
        self.principals
            .put_principal(
                &scope_of(person),
                person.principal_id.clone(),
                PrincipalKind::Human,
                person.data_role,
                status,
                None,
            )
            .unwrap();
    }
}

fn scope_of(person: &Principal) -> TenantScope {
    TenantScope::from_verified_token(
        &Principal::stub(
            PrincipalId("test".into()),
            PrincipalKind::Human,
            TenantId(person.tenant.0.clone()),
        ),
        Region(person.region.0.clone()),
    )
}

fn code_of(url: &str) -> String {
    url.rsplit_once("/join/").expect("a link").1.to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sign_in_racing_a_reset_waits_for_it_and_then_fails() {
    let stack = stack().await;
    let email = email();
    let (person, _) = stack.person(&email).await;
    let (reached, reached_rx) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let gate = std::sync::Mutex::new((reached, released));
    let resetting = stack.service().with_checkpoint(move |checkpoint| {
        if checkpoint == Checkpoint::SessionsEnded {
            let gate = gate.lock().unwrap();
            gate.0.send(()).unwrap();
            gate.1.recv().unwrap();
        }
    });
    let link = stack
        .accounts
        .operator_reset_link(&person.tenant.0, &email, None)
        .await
        .unwrap();
    let code = code_of(&link.url);
    let reset = tokio::task::spawn_blocking(move || {
        resetting.accept(
            &code,
            Acceptance {
                display_name: None,
                password: NEW_PASSWORD.into(),
            },
        )
    });
    tokio::task::spawn_blocking(move || reached_rx.recv().unwrap())
        .await
        .unwrap();

    // The reset has replaced the password and ended the sessions but not committed.
    let signing_in = {
        let (accounts, email) = (stack.accounts.clone(), email.clone());
        tokio::task::spawn_blocking(move || accounts.login(&email, PASSWORD.into(), None))
    };
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        !signing_in.is_finished(),
        "a sign-in with the old password waits for the reset to commit"
    );
    release.send(()).unwrap();
    reset.await.unwrap().expect("the reset commits");
    assert_eq!(
        signing_in.await.unwrap().unwrap_err(),
        AccountError::LoginFailed,
        "then the old password no longer matches"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_open_stream_learns_when_its_session_ends() {
    let stack = stack().await;
    let email = email();
    let (person, laptop) = stack.person(&email).await;
    let phone = stack.sign_in(&email, PASSWORD);
    let (laptop_identity, phone_identity) = (stack.identity(&laptop), stack.identity(&phone));
    assert!(stack.authn.still_live(&laptop_identity));

    stack.call("POST", "/v1/auth/logout", Some(&laptop), json!({}));
    assert!(!stack.authn.still_live(&laptop_identity), "signed out");
    assert!(
        stack.authn.still_live(&phone_identity),
        "the other session holds"
    );

    stack.call(
        "POST",
        "/v1/auth/logout/everywhere",
        Some(&phone),
        json!({}),
    );
    assert!(
        !stack.authn.still_live(&phone_identity),
        "signed out everywhere"
    );

    let tablet = stack.identity(&stack.sign_in(&email, PASSWORD));
    let link = stack
        .accounts
        .operator_reset_link(&person.tenant.0, &email, None)
        .await
        .unwrap();
    let after_reset = stack.accept(&link.url, json!({ "password": NEW_PASSWORD }));
    assert!(!stack.authn.still_live(&tablet), "ended by the reset");

    let after_reset = stack.identity(&after_reset);
    assert!(stack.authn.still_live(&after_reset));
    stack.set_status(&person, PrincipalStatus::Disabled);
    assert!(
        !stack.authn.still_live(&after_reset),
        "the person was disabled"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabling_a_person_ends_their_sessions_for_good() {
    let stack = stack().await;
    let email = email();
    let (person, joined) = stack.person(&email).await;
    let cli = stack.device_session(&joined);

    stack.set_status(&person, PrincipalStatus::Disabled);
    stack.set_status(&person, PrincipalStatus::Active);
    assert_eq!(
        stack.whoami(&joined),
        401,
        "re-enabling does not revive a session"
    );
    assert_eq!(stack.whoami(&cli), 401);
    let fresh = stack.sign_in(&email, PASSWORD);
    assert_eq!(stack.whoami(&fresh), 200, "the person signs in again");

    // A principal-wide revocation (as an erasure records) ends sessions the same way.
    stack
        .authn
        .revocations()
        .disable_principal(
            &scope_of(&person),
            &person.principal_id,
            myelin_events::clock::system_clock_reading()
                .unwrap()
                .timestamp(),
        )
        .unwrap();
    assert_eq!(stack.whoami(&fresh), 401);
    assert_eq!(
        stack.sign_in_status(&email, PASSWORD),
        401,
        "an erased person cannot sign in again"
    );
    assert_eq!(
        stack
            .principals
            .session_issue_time(&scope_of(&person), &person.principal_id, None)
            .unwrap(),
        None,
        "nor get a session any other way"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signing_out_sweeps_denials_whose_sessions_have_expired() {
    let stack = stack().await;
    let email = email();
    let (person, session) = stack.person(&email).await;
    let scope = scope_of(&person);
    let clock = |unix| {
        myelin_events::clock::clock_reading_from_unix(unix)
            .unwrap()
            .timestamp()
    };
    stack
        .authn
        .revocations()
        .revoke_until(
            &scope,
            &RevokeTarget::Jti("session-long-gone".into()),
            clock(now() - 7_200),
            clock(now() - 3_600),
        )
        .unwrap();
    // An agent run's registered life is a jti row too; once ended it still says `Expired`.
    stack
        .authn
        .revocations()
        .register_run_token_ttl(
            &scope,
            "run-ended",
            clock(now() - 7_200),
            clock(now() - 3_600),
        )
        .unwrap();
    assert_eq!(stack.jti_revocation_expiries(&person).await.len(), 2);

    let (status, _) = stack.call("POST", "/v1/auth/logout", Some(&session), json!({}));
    assert_eq!(status, 200);
    let mut left = stack.jti_revocation_expiries(&person).await;
    left.sort();
    assert_eq!(left.len(), 2, "the expired sign-out is gone: {left:?}");
    let expiry = |at: &Option<String>| {
        chrono::DateTime::parse_from_rfc3339(at.as_deref().unwrap())
            .unwrap()
            .timestamp()
    };
    assert!(expiry(&left[0]) < now(), "the ended run's row is kept");
    assert!(
        expiry(&left[1]) > now(),
        "and so is the session just signed out"
    );
    assert_eq!(
        stack.authn.revocations().run_token_state(
            &scope,
            &RevokeTarget::Jti("run-ended".into()),
            &clock(now()),
        ),
        myelin_identity_service::RunTokenState::Expired
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cli_approval_by_a_session_ended_mid_approval_is_refused() {
    let base = stack().await;
    let email = email();
    let (person, browser) = base.person(&email).await;
    // The same edge, except that the person signs out everywhere while it approves.
    let gateway = {
        let principals = base.principals.clone();
        let scope = scope_of(&person);
        let id = person.principal_id.clone();
        Gateway::builder(
            base.authn.clone(),
            Arc::new(HumanSsoAuthenticator::production(base.principals.clone())),
            Arc::new(AllowAll),
        )
        .default_token_scheme("session")
        .with_human_session_issuer(base.cell.clone())
        .with_device_authorization(
            DeviceAuthorizationBroker::with_pg(
                base.provider.db_pool().clone(),
                tokio::runtime::Handle::current(),
                "https://myelin.example/cli/auth",
            )
            .unwrap(),
        )
        .with_approval_checkpoint(move || {
            principals.end_sessions(&scope, &id).unwrap();
        })
        .build()
    };
    let stack = Stack { gateway, ..base };

    use base64::Engine as _;
    let verifier =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let (status, started) = stack.call(
        "POST",
        "/v1/auth/device/authorization",
        None,
        json!({ "code_challenge": challenge }),
    );
    assert_eq!(status, 201, "{started}");
    let (status, body) = stack.call(
        "POST",
        "/v1/auth/device/approval",
        Some(&browser),
        json!({ "user_code": started["user_code"] }),
    );
    assert_eq!(
        status, 401,
        "the vouching session ended before it vouched: {body}"
    );
    let (status, _) = stack.call(
        "POST",
        "/v1/auth/device/token",
        None,
        json!({ "device_code": started["device_code"], "code_verifier": verifier }),
    );
    assert_eq!(status, 202, "no CLI session was approved");
}
