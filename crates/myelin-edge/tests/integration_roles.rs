#![cfg(feature = "integration")]

//! Roles against the real database, where a black-box test cannot reach: two role changes racing
//! for the last owner, the migration that gives existing people their roles, erasure (which no
//! public endpoint performs), and SSO sign-in (which the system tests have no identity provider
//! for).

use myelin_config::{Mode, MyelinConfig};
use myelin_edge::accounts::MemberInvitation;
use myelin_edge::accounts::{Acceptance, Checkpoint};
use myelin_edge::{
    account_migrations, AccountError, AccountKeys, AccountService, AllowAll, EdgeRequest, Gateway,
    OrgRole,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::{
    CapabilityAuthenticator, CellTokenAuthority, CredentialVerifier, HumanSsoAuthenticator,
    PasetoCapabilityVerifier, PrincipalStore, RevocationStore, VerifiedAssertion,
};
use myelin_storage::{
    all_durable_migrations, identity_durable_migrations, with_tenant_tx, DurableKmsBacking,
    DurablePrincipalBacking, HotTables, PgBootstrap, SealKey, SubstrateProvider, TenantScope,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, Row};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SEAL_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

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

struct Cell {
    provider: SubstrateProvider,
    kms: Arc<myelin_storage::KmsEngine>,
    principals: PrincipalStore,
    accounts: AccountService,
}

async fn accounts() -> AccountService {
    cell().await.accounts
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
    let kms = Arc::new(
        DurableKmsBacking::new(provider.db_pool().clone(), unique("cell-roles"))
            .load_or_generate(&seal)
            .await
            .expect("KMS"),
    );
    let principals = PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(provider.clone()),
        tokio::runtime::Handle::current(),
    );
    let accounts = AccountService::new(
        provider.db_pool().clone(),
        principals.clone(),
        AccountKeys::derive(&seal),
        provider.config().region.clone(),
        Some("https://myelin.example".into()),
        tokio::runtime::Handle::current(),
    );
    Cell {
        provider,
        kms,
        principals,
        accounts,
    }
}

async fn owner(accounts: &AccountService, tenant: &str, name: &str) -> Principal {
    let link = accounts
        .operator_join_link(
            tenant,
            &format!("{}@example.com", unique(name)),
            None,
            Some(OrgRole::Owner),
        )
        .await
        .unwrap();
    accept(accounts, &link.url, name).unwrap()
}

fn accept(accounts: &AccountService, url: &str, name: &str) -> Result<Principal, AccountError> {
    let code = url.rsplit_once("/join/").unwrap().1;
    accounts
        .accept(
            code,
            Acceptance {
                display_name: Some(name.into()),
                password: "correct horse battery".into(),
            },
        )
        .map(|signed_in| signed_in.principal)
}

/// With mail off, an invitation's link comes back to the inviter.
fn link_from(
    accounts: &AccountService,
    inviter: &Principal,
    email: &str,
) -> Result<String, AccountError> {
    match accounts.invite_member(inviter, email, OrgRole::Member)? {
        MemberInvitation::HandedOver(issued) => Ok(issued.url),
        MemberInvitation::Emailed { .. } => panic!("mail is off"),
    }
}

/// Records what an erasure records: a permanent principal-wide revocation, which disables them.
async fn erase(provider: &SubstrateProvider, person: &Principal) {
    let handle = person.principal_id.0.clone();
    with_tenant_tx(
        provider.db_pool(),
        &person.tenant.0,
        &person.region.0,
        move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO revocation (tenant_id, region, kind, handle, revoked_at) \
                     VALUES (current_setting('myelin.tenant_id'), current_setting('myelin.region'), \
                             'principal', $1, '2026-11-26T00:00:00Z')",
                )
                .bind(&handle)
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok(())
            })
        },
    )
    .await
    .unwrap();
}

/// A copy of `accounts` that stops inside its first role change, once both roles are read and
/// allow it, until released.
fn paused_after_reading_roles(
    accounts: &AccountService,
) -> (AccountService, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (reached, on_reached) = mpsc::channel();
    let (release, on_release) = mpsc::channel::<()>();
    let (reached, on_release) = (Mutex::new(reached), Mutex::new(on_release));
    let fired = AtomicBool::new(false);
    let paused = accounts.clone().with_checkpoint(move |checkpoint| {
        if checkpoint == Checkpoint::RolesRead && !fired.swap(true, Ordering::SeqCst) {
            reached.lock().unwrap().send(()).unwrap();
            on_release.lock().unwrap().recv().unwrap();
        }
    });
    (paused, on_reached, release)
}

fn roles(accounts: &AccountService, viewer: &Principal) -> Vec<(String, OrgRole)> {
    accounts
        .roster(viewer)
        .unwrap()
        .people
        .into_iter()
        .map(|person| (person.principal_id, person.role))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_owners_demoting_at_once_always_leave_one_owner() {
    let accounts = accounts().await;
    // Each owner demotes the other, then each demotes themself: either way the second change
    // waits for the first and is judged against what it left.
    for mutual in [true, false] {
        let tenant = unique("t-roles-race");
        let ada = owner(&accounts, &tenant, "Ada").await;
        let grace = owner(&accounts, &tenant, "Grace").await;
        let (first_target, second_target) = if mutual {
            (grace.principal_id.0.clone(), ada.principal_id.0.clone())
        } else {
            (ada.principal_id.0.clone(), grace.principal_id.0.clone())
        };

        let (paused, reached, release) = paused_after_reading_roles(&accounts);
        let first = {
            let ada = ada.clone();
            tokio::task::spawn_blocking(move || {
                paused.set_member_role(&ada, &first_target, OrgRole::Member)
            })
        };
        reached
            .recv_timeout(Duration::from_secs(10))
            .expect("the first change reads both roles");
        let second = {
            let (accounts, grace) = (accounts.clone(), grace.clone());
            tokio::task::spawn_blocking(move || {
                accounts.set_member_role(&grace, &second_target, OrgRole::Member)
            })
        };
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            !second.is_finished(),
            "the second change waits for the first instead of reading the same two owners"
        );
        release.send(()).unwrap();
        assert_eq!(first.await.unwrap(), Ok(()));
        let expected = if mutual {
            AccountError::RoleRequired
        } else {
            AccountError::LastOwner
        };
        assert_eq!(second.await.unwrap(), Err(expected));

        let survivor = if mutual { &ada } else { &grace };
        let owners: Vec<_> = roles(&accounts, survivor)
            .into_iter()
            .filter(|(_, role)| *role == OrgRole::Owner)
            .map(|(id, _)| id)
            .collect();
        assert_eq!(owners, vec![survivor.principal_id.0.clone()]);
    }
}

/// Runs `sql` on one admin connection whose search path is a fresh, empty schema.
struct FreshSchema {
    conn: sqlx::pool::PoolConnection<sqlx::Postgres>,
    name: String,
}

impl FreshSchema {
    async fn new() -> Self {
        let url = std::env::var("DATABASE_MIGRATION_URL").expect("DATABASE_MIGRATION_URL");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .expect("admin connection");
        let mut conn = pool.acquire().await.unwrap();
        let name = unique("roles_backfill").replace('-', "_");
        conn.execute(format!("CREATE SCHEMA {name}; SET search_path TO {name}").as_str())
            .await
            .unwrap();
        Self { conn, name }
    }

    async fn run(&mut self, sql: &str) {
        self.conn.execute(sql).await.unwrap();
    }

    async fn remove(mut self) {
        let sql = format!("DROP SCHEMA {} CASCADE", self.name);
        self.conn.execute(sql.as_str()).await.unwrap();
    }
}

fn person(tenant: &str, id: &str, status: &str, changed_at: i64) -> String {
    format!(
        "INSERT INTO principal (tenant_id, region, principal_id, kind, data_role, status) \
         VALUES ('{tenant}', 'fr-par', '{id}', '\"Human\"', '\"Controller\"', '\"{status}\"'); \
         INSERT INTO auth_password_account \
           (email_digest, tenant_id, region, principal_id, password_phc, changed_at_unix) \
         VALUES (sha256('{tenant}{id}'), '{tenant}', 'fr-par', '{id}', 'phc', {changed_at});"
    )
}

fn join_link(tenant: &str, origin: &str, seed: &str) -> String {
    format!(
        "INSERT INTO auth_invitation \
           (code_digest, kind, tenant_id, region, email_digest, email_nonce, email_ciphertext, \
            principal_id, created_at_unix, expires_at_unix, origin) \
         VALUES (sha256('{seed}'), 'join', '{tenant}', 'fr-par', sha256('e{seed}'), \
                 substr(sha256('n{seed}'), 1, 12), 'x', NULL, 0, 4000000000, '{origin}');"
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_roles_migration_makes_each_organizations_earliest_active_person_its_owner() {
    let mut schema = FreshSchema::new().await;
    let principal = identity_durable_migrations()
        .0
        .into_iter()
        .find(|migration| migration.id == "0012_principal")
        .expect("the principal table");
    schema.run(&principal.ddl).await;
    let principal_rls = identity_durable_migrations()
        .0
        .into_iter()
        .find(|migration| migration.id == "0013_principal_rls")
        .expect("the principal table's row-level security");
    schema.run(&principal_rls.ddl).await;
    let (before, roles): (Vec<_>, Vec<_>) = account_migrations()
        .0
        .into_iter()
        .filter(|migration| migration.id.as_ref() <= "edge_0006_org_roles")
        .partition(|migration| migration.id != "edge_0006_org_roles");
    for migration in before {
        schema.run(&migration.ddl).await;
    }

    // In `acme` the earliest person (by the time in their id) is disabled, so the next one owns
    // it; insertion order and password changes do not matter. `globex` predates ULID ids.
    let earliest = "person-01h00000000000000000000000";
    let next = "person-01h00100000000000000000000";
    let later = "person-01h00200000000000000000000";
    for statement in [
        person("acme", later, "Active", 50),
        person("acme", next, "Active", 9_999_999_999),
        person("acme", earliest, "Disabled", 10),
        person("globex", "founder", "Active", 1_700_000_000),
        join_link("acme", "member", "a"),
        join_link("acme", "operator", "b"),
        join_link("initech", "operator", "c"),
    ] {
        schema.run(&statement).await;
    }
    for migration in roles {
        schema.run(&migration.ddl).await;
    }

    let members = sqlx::query(
        "SELECT tenant_id, principal_id, role, joined_at_unix FROM auth_org_member \
          ORDER BY tenant_id, principal_id",
    )
    .fetch_all(&mut *schema.conn)
    .await
    .unwrap()
    .into_iter()
    .map(|row| {
        (
            row.get::<String, _>("tenant_id"),
            row.get::<String, _>("principal_id"),
            row.get::<String, _>("role"),
            row.get::<i64, _>("joined_at_unix"),
        )
    })
    .collect::<Vec<_>>();
    assert_eq!(
        members
            .iter()
            .map(|(tenant, id, role, _)| (tenant.as_str(), id.as_str(), role.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("acme", earliest, "member"),
            ("acme", next, "owner"),
            ("acme", later, "member"),
            ("globex", "founder", "owner"),
        ]
    );
    // A ULID's first ten characters are its milliseconds since the epoch, in base 32; the sixth
    // counts 32^4 ms.
    let joined = |id: &str| members.iter().find(|row| row.1 == id).unwrap().3;
    assert_eq!(joined(earliest), 1_683_627_180);
    assert_eq!(joined(next), 1_683_628_228);
    assert_eq!(joined(later), 1_683_629_277);
    assert_eq!(
        joined("founder"),
        1_700_000_000,
        "an id without a time joins at its last password change"
    );

    let links = sqlx::query(
        "SELECT tenant_id, origin, role, id, roster_nonce FROM auth_invitation \
          ORDER BY tenant_id, origin",
    )
    .fetch_all(&mut *schema.conn)
    .await
    .unwrap();
    let granted: Vec<_> = links
        .iter()
        .map(|row| {
            (
                row.get::<String, _>("tenant_id"),
                row.get::<String, _>("origin"),
                row.get::<String, _>("role"),
            )
        })
        .collect();
    assert_eq!(
        granted,
        vec![
            ("acme".into(), "member".into(), "member".into()),
            ("acme".into(), "operator".into(), "member".into()),
            ("initech".into(), "operator".into(), "owner".into()),
        ]
    );
    assert!(links
        .iter()
        .all(|row| !row.get::<String, _>("id").is_empty()
            && row.get::<Option<Vec<u8>>, _>("roster_nonce").is_none()));
    schema.remove().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_erased_person_cannot_rejoin_and_an_erased_last_owner_leaves_no_owner() {
    let cell = cell().await;
    let accounts = &cell.accounts;
    let tenant = unique("t-roles-erased");
    let ada = owner(accounts, &tenant, "Ada").await;
    let hopper = accept(
        accounts,
        &link_from(accounts, &ada, &format!("{}@example.com", unique("hopper"))).unwrap(),
        "Hopper",
    )
    .unwrap();
    let grace_email = format!("{}@example.com", unique("grace"));
    let grace = accept(
        accounts,
        &link_from(accounts, &ada, &grace_email).unwrap(),
        "Grace",
    )
    .unwrap();

    // Removed, Grace may be invited back; erased before she accepts, she may not come back.
    accounts.remove_member(&ada, &grace.principal_id.0).unwrap();
    let back = link_from(accounts, &ada, &grace_email).unwrap();
    erase(&cell.provider, &grace).await;
    assert_eq!(
        accept(accounts, &back, "Grace").unwrap_err(),
        AccountError::PersonErased
    );
    assert_eq!(
        link_from(accounts, &ada, &grace_email).unwrap_err(),
        AccountError::AccountExists,
        "an erased person's address is never invited again"
    );

    // Erasing the only owner is not blocked; the organization says it has none, and the links
    // the owner sent die with them.
    let pending = link_from(
        accounts,
        &ada,
        &format!("{}@example.com", unique("pending")),
    )
    .unwrap();
    assert_eq!(accounts.roster(&hopper).unwrap().owners, 1);
    erase(&cell.provider, &ada).await;
    assert_eq!(accounts.roster(&hopper).unwrap().owners, 0);
    assert_eq!(
        accounts.invitation(code_of(&pending)).unwrap_err(),
        AccountError::InvitationNotFound
    );
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_invitation WHERE tenant_id = $1 AND kind = 'join'",
    )
    .bind(&tenant)
    .fetch_one(cell.provider.db_pool())
    .await
    .unwrap();
    assert_eq!(left, 0, "every link Ada sent is gone with her");
}

fn code_of(url: &str) -> &str {
    url.rsplit_once("/join/").unwrap().1
}

/// Sets a person's status directly, as a writer other than the roster (SCIM, an operator) would.
async fn set_status(provider: &SubstrateProvider, person: &Principal, status: &'static str) {
    let handle = person.principal_id.0.clone();
    with_tenant_tx(
        provider.db_pool(),
        &person.tenant.0,
        &person.region.0,
        move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "UPDATE principal SET status = $2 \
                      WHERE tenant_id = current_setting('myelin.tenant_id') AND principal_id = $1",
                )
                .bind(&handle)
                .bind(format!("\"{status}\""))
                .execute(&mut *conn)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok(())
            })
        },
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_invited_back_is_a_new_principal_and_the_removed_one_stays_disabled() {
    let cell = cell().await;
    let accounts = &cell.accounts;
    let tenant = unique("t-roles-return");
    let ada = owner(accounts, &tenant, "Ada").await;
    let grace_email = format!("{}@example.com", unique("grace"));
    let grace = accept(
        accounts,
        &link_from(accounts, &ada, &grace_email).unwrap(),
        "Grace",
    )
    .unwrap();
    let reset = accounts
        .operator_reset_link(&tenant, &grace_email, None)
        .await
        .unwrap();

    accounts.remove_member(&ada, &grace.principal_id.0).unwrap();
    let returned = accept(
        accounts,
        &link_from(accounts, &ada, &grace_email).unwrap(),
        "Grace Again",
    )
    .unwrap();
    assert_ne!(returned.principal_id, grace.principal_id);
    assert_eq!(
        accounts.invitation(code_of(&reset.url)).unwrap_err(),
        AccountError::InvitationNotFound,
        "the removed person's reset links end when their address moves on"
    );
    let scope = TenantScope::from_verified_token(&grace, grace.region.clone());
    let removed = cell
        .principals
        .get_principal(&scope, &grace.principal_id)
        .unwrap()
        .expect("the removed person is kept, with their history");
    // Agents, triggers, delegations and linked tokens all require their person to be active.
    assert_eq!(removed.status, PrincipalStatus::Disabled);
    assert_eq!(accounts.role_of(&grace).unwrap(), None);
    assert_eq!(accounts.role_of(&returned).unwrap(), Some(OrgRole::Member));
    let profile = accounts.profile(&returned).unwrap().unwrap();
    assert_eq!(profile.display_name, "Grace Again");
    assert_eq!(profile.email, grace_email);
    let signed_in = accounts
        .login(&grace_email, "correct horse battery".into(), None)
        .unwrap();
    assert_eq!(signed_in.principal.principal_id, returned.principal_id);
    assert_eq!(
        link_from(accounts, &ada, &grace_email).unwrap_err(),
        AccountError::AccountExists,
        "once back, the address has an account again"
    );

    // Erasing the person who came back erases who they were before, too.
    let identity = myelin_identity_service::StoreBackedCheck::with_pg(
        cell.provider.clone(),
        cell.kms.clone(),
        Arc::new(CellTokenAuthority::generate()),
        tokio::runtime::Handle::current(),
    );
    let readable = |person: &Principal| matches!(accounts.profile(person), Ok(Some(profile)) if !profile.email.is_empty());
    assert!(readable(&grace) && readable(&returned));
    tokio::task::block_in_place(|| {
        identity.erase_in(
            &TenantScope::from_verified_token(&returned, returned.region.clone()),
            &returned.principal_id,
            myelin_events::Timestamp("2026-11-26T00:00:00Z".into()),
        )
    })
    .unwrap();
    assert!(!readable(&returned), "the person's profile is shredded");
    assert!(!readable(&grace), "and so is the profile they had before");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removed_person_made_active_again_keeps_their_address() {
    let cell = cell().await;
    let accounts = &cell.accounts;
    let tenant = unique("t-roles-reactivated");
    let ada = owner(accounts, &tenant, "Ada").await;
    let grace_email = format!("{}@example.com", unique("grace"));
    let grace = accept(
        accounts,
        &link_from(accounts, &ada, &grace_email).unwrap(),
        "Grace",
    )
    .unwrap();
    accounts.remove_member(&ada, &grace.principal_id.0).unwrap();
    let back = link_from(accounts, &ada, &grace_email).unwrap();

    // Made active again by another writer (an operator's bootstrap upserts the status), Grace is
    // present: her address is not moved away from her.
    set_status(&cell.provider, &grace, "Active").await;
    assert_eq!(
        accept(accounts, &back, "Grace Again").unwrap_err(),
        AccountError::AccountExists
    );
    assert_eq!(
        link_from(accounts, &ada, &grace_email).unwrap_err(),
        AccountError::AccountExists
    );
    assert!(accounts
        .was_removed(&tenant, &grace.region.0, &grace.principal_id.0)
        .await
        .unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_removal_lets_an_address_be_invited_back_and_old_member_links_are_dead() {
    let cell = cell().await;
    let accounts = &cell.accounts;
    let tenant = unique("t-roles-disabled");
    let ada = owner(accounts, &tenant, "Ada").await;
    let grace_email = format!("{}@example.com", unique("grace"));
    let grace = accept(
        accounts,
        &link_from(accounts, &ada, &grace_email).unwrap(),
        "Grace",
    )
    .unwrap();

    // Disabled some other way, Grace was not removed, so her address is still taken.
    set_status(&cell.provider, &grace, "Disabled").await;
    assert_eq!(
        link_from(accounts, &ada, &grace_email).unwrap_err(),
        AccountError::AccountExists
    );

    // A member's link stored before senders were recorded has no sender, and is dead.
    let old = link_from(accounts, &ada, &format!("{}@example.com", unique("hopper"))).unwrap();
    sqlx::query(
        "UPDATE auth_invitation SET invited_by = NULL WHERE tenant_id = $1 AND kind = 'join'",
    )
    .bind(&tenant)
    .execute(cell.provider.db_pool())
    .await
    .unwrap();
    assert_eq!(
        accounts.invitation(code_of(&old)).unwrap_err(),
        AccountError::InvitationNotFound
    );
    assert_eq!(
        accept(accounts, &old, "Hopper").unwrap_err(),
        AccountError::InvitationNotFound
    );
    assert_eq!(accounts.roster(&ada).unwrap().invitations, Some(vec![]));
}

/// Verifies any OIDC assertion as one fixed subject in one organization.
struct FixedOidcSubject {
    tenant: String,
    region: String,
}

impl CredentialVerifier for FixedOidcSubject {
    fn verify(
        &self,
        _credential: &myelin_identity::Credential,
    ) -> myelin_identity::Result<VerifiedAssertion> {
        Ok(VerifiedAssertion {
            tenant: TenantId(self.tenant.clone()),
            region: Region(self.region.clone()),
            scheme: myelin_identity_service::scheme::OIDC.into(),
            subject_key: "oidc-subject".into(),
            expires_at_unix: None,
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_signing_in_with_sso_becomes_a_member_once() {
    let cell = cell().await;
    let tenant = unique("t-roles-sso");
    let region = cell.provider.config().region.clone();
    let person = Principal::new(
        TenantId(tenant.clone()),
        Region(region.clone()),
        PrincipalId(unique("sso-person")),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    let scope = TenantScope::from_verified_token(&person, Region(region.clone()));
    cell.principals
        .put_principal(
            &scope,
            person.principal_id.clone(),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .unwrap();
    cell.principals
        .link_credential(
            &scope,
            myelin_identity_service::scheme::OIDC,
            "oidc-subject",
            &person.principal_id,
        )
        .unwrap();
    let authority = Arc::new(CellTokenAuthority::generate());
    let gateway = Gateway::builder(
        Arc::new(CapabilityAuthenticator::with_verifier(
            cell.principals.clone(),
            Arc::new(PasetoCapabilityVerifier::new(authority.trust_anchor())),
            RevocationStore::new(),
        )),
        Arc::new(HumanSsoAuthenticator::with_verifier(
            cell.principals.clone(),
            Arc::new(FixedOidcSubject {
                tenant: tenant.clone(),
                region,
            }),
        )),
        Arc::new(AllowAll),
    )
    .with_human_session_issuer(authority)
    .with_accounts(cell.accounts.clone())
    .build();
    let sign_in = || {
        let body = serde_json::to_vec(&serde_json::json!({
            "scheme": "oidc",
            "material": "an id token",
            "nonce": "A".repeat(43),
        }))
        .unwrap();
        let gateway = &gateway;
        tokio::task::block_in_place(|| {
            gateway.handle(EdgeRequest::new("POST", "/v1/auth/login", "", vec![], body))
        })
    };

    assert_eq!(cell.accounts.role_of(&person).unwrap(), None);
    // Someone who signs in another way is a person: the operator's next link grants member.
    let link = cell
        .accounts
        .operator_join_link(
            &tenant,
            &format!("{}@example.com", unique("next")),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(link.role, Some(OrgRole::Member));
    assert_eq!(sign_in().status(), 200);
    assert_eq!(
        cell.accounts.role_of(&person).unwrap(),
        Some(OrgRole::Member)
    );
    // A later sign-in leaves the role they were given alone.
    cell.accounts
        .operator_set_role(
            &tenant,
            myelin_edge::OperatorTarget::Principal(&person.principal_id.0),
            OrgRole::Admin,
        )
        .await
        .unwrap();
    assert_eq!(sign_in().status(), 200);
    assert_eq!(
        cell.accounts.role_of(&person).unwrap(),
        Some(OrgRole::Admin)
    );
}
