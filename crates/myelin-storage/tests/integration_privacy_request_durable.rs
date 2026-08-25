#![cfg(feature = "integration")]

use chrono::{Duration, TimeZone, Utc};
use myelin_config::MyelinConfig;
use myelin_identity::{DataRole, PrincipalKind, PrincipalStatus};
use myelin_storage::{
    all_durable_migrations, ClaimPrivacyRequestOutcome, CompletePrivacyRequestOutcome,
    CreatePrivacyRequestOutcome, DurablePrivacyRequestStore, HotTables, NewPrivacyRequest,
    PrivacyHolderReceipt, PrivacyRequestCertificate, PrivacyRequestKind, PrivacyRequestScope,
    PrivacyRequestState, SubstrateProvider,
};
use sqlx::types::Uuid;

fn app_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    if let Ok(database_url) = std::env::var("MYELIN_TEST_DATABASE_URL") {
        if !database_url.trim().is_empty() {
            config.database_url = database_url;
        }
    }
    config
}

fn admin_config() -> MyelinConfig {
    let mut config = app_config();
    config.database_url = config
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    config
}

fn unique_tenant() -> String {
    format!(
        "privacy-request-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock follows the epoch")
            .as_nanos()
    )
}

async fn provider() -> SubstrateProvider {
    let admin = SubstrateProvider::connect(admin_config(), 2)
        .await
        .expect("connect the migration role for the privacy-request story");
    admin.migrate_foundation().await.unwrap();
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap();
    SubstrateProvider::connect(app_config(), 3)
        .await
        .expect("connect the constrained runtime role")
}

async fn seed_people(provider: &SubstrateProvider, tenant: &str) {
    let tenant = tenant.to_string();
    let region = provider.config().region.clone();
    provider
        .with_tenant_tx(&tenant.clone(), move |connection| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO principal \
                       (tenant_id, region, principal_id, kind, data_role, status) VALUES \
                       ($1, $2, 'alice', $3, $4, $5), \
                       ($1, $2, 'bob', $3, $4, $5)",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(serde_json::to_string(&PrincipalKind::Human).unwrap())
                .bind(serde_json::to_string(&DataRole::Controller).unwrap())
                .bind(serde_json::to_string(&PrincipalStatus::Active).unwrap())
                .execute(&mut *connection)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok(())
            })
        })
        .await
        .unwrap();
}

fn certificate(request_id: Uuid) -> PrivacyRequestCertificate {
    PrivacyRequestCertificate::build(
        request_id,
        PrivacyRequestKind::Erasure,
        PrivacyRequestScope::AgentData,
        vec![PrivacyHolderReceipt::erasure("agent_data", 7).unwrap()],
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_message_erasure_is_a_distinct_durable_request_scope() {
    let provider = provider().await;
    let tenant = unique_tenant();
    seed_people(&provider, &tenant).await;
    let submitted_at = Utc.timestamp_opt(1_788_912_000, 0).single().unwrap();
    let request_id = Uuid::from_u128(0x135);
    let requests = DurablePrivacyRequestStore::new(provider);

    let created = requests
        .create(
            &tenant,
            NewPrivacyRequest {
                request_id,
                owner_principal_id: "alice".into(),
                client_nonce: "erase-my-chat-messages-once".into(),
                kind: PrivacyRequestKind::Erasure,
                scope: PrivacyRequestScope::ChatMessages,
                submitted_at,
            },
        )
        .await
        .expect("persist the user-visible Chat erasure request");
    let CreatePrivacyRequestOutcome::Created(created) = created else {
        panic!("the scoped request should be new: {created:?}");
    };
    assert_eq!(created.scope, PrivacyRequestScope::ChatMessages);

    let claimed = requests
        .claim_owned(
            &tenant,
            "alice",
            request_id,
            "chat-erasure-worker",
            submitted_at,
            30,
        )
        .await
        .expect("claim the Chat erasure request");
    let ClaimPrivacyRequestOutcome::Claimed(lease) = claimed else {
        panic!("the Chat erasure request should be claimable: {claimed:?}");
    };
    let certificate = PrivacyRequestCertificate::build(
        request_id,
        PrivacyRequestKind::Erasure,
        PrivacyRequestScope::ChatMessages,
        vec![PrivacyHolderReceipt::erasure("chat_messages", 2).unwrap()],
    )
    .unwrap();
    let completed = requests
        .complete(
            &tenant,
            &lease,
            &certificate,
            submitted_at + Duration::seconds(1),
        )
        .await
        .expect("complete the Chat erasure request");
    let CompletePrivacyRequestOutcome::Completed(completed) = completed else {
        panic!("the Chat erasure certificate should be durable: {completed:?}");
    };
    assert_eq!(completed.scope, PrivacyRequestScope::ChatMessages);
    assert_eq!(completed.certificate, Some(certificate));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_privacy_request_survives_a_lost_worker_and_returns_one_private_certificate() {
    let provider = provider().await;
    let tenant = unique_tenant();
    seed_people(&provider, &tenant).await;
    let submitted_at = Utc.timestamp_opt(1_788_912_000, 0).single().unwrap();
    let intended = NewPrivacyRequest {
        request_id: Uuid::from_u128(0x131),
        owner_principal_id: "alice".into(),
        client_nonce: "erase-my-agent-data-once".into(),
        kind: PrivacyRequestKind::Erasure,
        scope: PrivacyRequestScope::AgentData,
        submitted_at,
    };
    let requests = DurablePrivacyRequestStore::new(provider.clone());

    let created = requests.create(&tenant, intended.clone()).await.unwrap();
    let CreatePrivacyRequestOutcome::Created(created) = created else {
        panic!("the first submission should create one request: {created:?}");
    };
    assert_eq!(created.state, PrivacyRequestState::Pending);
    assert_eq!(created.attempt_count, 0);
    assert_eq!(
        created.deadline_at,
        submitted_at + Duration::days(30),
        "the person sees the regulatory deadline on the durable request"
    );

    let replay = requests
        .create(
            &tenant,
            NewPrivacyRequest {
                request_id: Uuid::from_u128(0x132),
                submitted_at: submitted_at + Duration::hours(1),
                ..intended.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        replay,
        CreatePrivacyRequestOutcome::Replayed(created.clone()),
        "a lost submit response cannot create a second erasure"
    );
    assert!(
        requests
            .get_owned(&tenant, "bob", intended.request_id)
            .await
            .unwrap()
            .is_none(),
        "another person cannot enumerate the request"
    );

    let first = requests
        .claim_owned(
            &tenant,
            "alice",
            intended.request_id,
            "privacy-worker-before-crash",
            submitted_at,
            1,
        )
        .await
        .unwrap();
    let ClaimPrivacyRequestOutcome::Claimed(first_lease) = first else {
        panic!("the first worker should own the request: {first:?}");
    };
    assert_eq!(first_lease.request().attempt_count, 1);

    let restarted = DurablePrivacyRequestStore::new(provider.clone());
    let resumed = restarted
        .claim_owned(
            &tenant,
            "alice",
            intended.request_id,
            "privacy-worker-after-restart",
            submitted_at + Duration::seconds(2),
            30,
        )
        .await
        .unwrap();
    let ClaimPrivacyRequestOutcome::Claimed(resumed_lease) = resumed else {
        panic!("a fresh worker should recover the expired lease: {resumed:?}");
    };
    assert_eq!(resumed_lease.request().attempt_count, 2);

    assert_eq!(
        requests
            .complete(
                &tenant,
                &first_lease,
                &certificate(intended.request_id),
                submitted_at + Duration::seconds(3),
            )
            .await
            .unwrap(),
        CompletePrivacyRequestOutcome::LeaseLost,
        "the worker that disappeared cannot publish after another worker recovered the request"
    );
    assert!(restarted
        .release_after_failure(
            &tenant,
            &resumed_lease,
            "the holder was temporarily unavailable",
        )
        .await
        .unwrap());
    let awaiting_retry = restarted
        .get_owned(&tenant, "alice", intended.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(awaiting_retry.state, PrivacyRequestState::Pending);
    assert_eq!(
        awaiting_retry.last_failure.as_deref(),
        Some("the holder was temporarily unavailable")
    );

    let final_claim = restarted
        .claim_owned(
            &tenant,
            "alice",
            intended.request_id,
            "privacy-worker-final",
            submitted_at + Duration::seconds(4),
            30,
        )
        .await
        .unwrap();
    let ClaimPrivacyRequestOutcome::Claimed(final_lease) = final_claim else {
        panic!("the released request should be retryable: {final_claim:?}");
    };
    let expected_certificate = certificate(intended.request_id);
    let completed = restarted
        .complete(
            &tenant,
            &final_lease,
            &expected_certificate,
            submitted_at + Duration::seconds(5),
        )
        .await
        .unwrap();
    let CompletePrivacyRequestOutcome::Completed(completed) = completed else {
        panic!("the final worker should publish the certificate: {completed:?}");
    };
    assert_eq!(completed.state, PrivacyRequestState::Completed);
    assert_eq!(completed.attempt_count, 3);
    assert_eq!(completed.certificate, Some(expected_certificate.clone()));
    assert!(completed.last_failure.is_none());

    let completed_replay = restarted.create(&tenant, intended).await.unwrap();
    assert_eq!(
        completed_replay,
        CreatePrivacyRequestOutcome::Replayed(completed)
    );

    let tenant_for_query = tenant.clone();
    let certificate_json: String = provider
        .with_tenant_tx(&tenant, move |connection| {
            Box::pin(async move {
                sqlx::query_scalar(
                    "SELECT certificate::text FROM privacy_request \
                      WHERE tenant_id = $1 AND request_id = $2",
                )
                .bind(&tenant_for_query)
                .bind(Uuid::from_u128(0x131))
                .fetch_one(&mut *connection)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
            })
        })
        .await
        .unwrap();
    assert!(!certificate_json.contains("alice"));
    assert!(certificate_json.contains(&expected_certificate.content_hash));

    let tenant_for_tamper = tenant.clone();
    provider
        .with_tenant_tx(&tenant, move |connection| {
            Box::pin(async move {
                sqlx::query(
                    "UPDATE privacy_request SET certificate = jsonb_set(\
                       certificate, '{holder_receipts,0,records_erased}', '999'::jsonb\
                     ) WHERE tenant_id = $1 AND request_id = $2",
                )
                .bind(&tenant_for_tamper)
                .bind(Uuid::from_u128(0x131))
                .execute(&mut *connection)
                .await
                .map_err(|error| myelin_storage::PgError::Query(error.to_string()))?;
                Ok(())
            })
        })
        .await
        .unwrap();
    let corrupt = restarted
        .get_owned(&tenant, "alice", Uuid::from_u128(0x131))
        .await
        .expect_err("a durable certificate with a changed holder count must not be returned");
    assert!(corrupt
        .to_string()
        .contains("privacy holder receipt failed content verification"));
}
