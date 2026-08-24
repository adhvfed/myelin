#![cfg(feature = "integration")]

use myelin_chat::conversation::{Conversation, ConversationKind};
use myelin_chat::store::pg_conversation::{chat_migrations, PgConversationStore};
use myelin_chat::store::{ConversationId, SystemUlidSource, UlidSource};
use myelin_config::MyelinConfig;
use myelin_events::Timestamp;
use myelin_identity::{
    DataRole, ObjectId, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RelName,
    RelationTuple, TupleDelta,
};
use myelin_identity_service::TupleStore;
use myelin_storage::{all_durable_migrations, DurableTupleBacking, SubstrateProvider, TenantScope};
use myelin_substrate::HotTables;
use myelin_tenancy::{Region, TenantId};

fn test_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    if let Ok(database_url) = std::env::var("MYELIN_TEST_DATABASE_URL") {
        if !database_url.trim().is_empty() {
            config.database_url = database_url;
        }
    }
    config
}

fn admin_config(config: &MyelinConfig) -> MyelinConfig {
    let mut admin = config.clone();
    admin.database_url = admin
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    admin
}

fn unique() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn principal(tenant: &str, region: &str, id: &str) -> Principal {
    Principal::new(
        TenantId::from_token(tenant),
        Region::new(region),
        PrincipalId(id.into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn add(object: impl Into<String>, relation: &str, subject: impl Into<String>) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

async fn write_until(
    tuples: &TupleStore,
    scope: &TenantScope,
    actor: &Principal,
    deltas: Vec<TupleDelta>,
    expires_at: Option<Timestamp>,
) {
    let tuples = tuples.clone();
    let scope = scope.clone();
    let actor = actor.clone();
    tokio::task::spawn_blocking(move || {
        tuples.write_tuples(
            &scope,
            &actor,
            &deltas,
            None,
            expires_at,
            Timestamp("2020-01-01T00:00:00Z".into()),
        )
    })
    .await
    .expect("tuple writer joins")
    .expect("relationship write commits");
}

fn public_channel(
    tenant: &str,
    region: &str,
    conversation_id: &str,
    project_id: &str,
    name: &str,
    creator: &str,
) -> Conversation {
    Conversation {
        id: ConversationId::new(tenant, region, conversation_id),
        kind: ConversationKind::ChannelPublic,
        home_cell: format!("{region}:{tenant}"),
        parent_project: Some(project_id.into()),
        name: Some(name.into()),
        topic: Some("Temporary access must end on time".into()),
        linked_ref: None,
        pinned_canvas: None,
        retention_days: None,
        archived: false,
        created_by: creator.into(),
        acl_zookie: Some("zk-00000000000000000001".into()),
    }
}

fn private_channel(
    tenant: &str,
    region: &str,
    conversation_id: &str,
    name: &str,
    creator: &str,
) -> Conversation {
    Conversation {
        kind: ConversationKind::ChannelPrivate,
        parent_project: None,
        retention_days: Some(3),
        ..public_channel(
            tenant,
            region,
            conversation_id,
            "11111111-1111-4111-8111-111111111111",
            name,
            creator,
        )
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn public_channel_list_forgets_every_expired_authority_path() {
    let config = test_config();
    let admin = SubstrateProvider::connect(admin_config(&config), 4)
        .await
        .expect("connect to the PostgreSQL required by the public-channel story");
    admin.migrate_foundation().await.unwrap();
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap();
    admin
        .migrate(&chat_migrations(), &HotTables::none())
        .await
        .unwrap();
    let app = SubstrateProvider::connect(config, 6)
        .await
        .expect("connect constrained runtime role");
    let region = app.config().region.clone();
    let tenant = format!("chat-expiry-{}", unique());
    let alice = principal(&tenant, &region, "p:alice");
    let scope = TenantScope::from_verified_token(&alice, alice.region.clone());
    let tuples = TupleStore::with_pg(
        DurableTupleBacking::new(app.clone()),
        tokio::runtime::Handle::current(),
    );
    let conversations = PgConversationStore::new(app.db_pool().clone());

    let live_project = "11111111-1111-4111-8111-111111111111";
    let expired_parent_project = "22222222-2222-4222-8222-222222222222";
    let expired_member_project = "33333333-3333-4333-8333-333333333333";
    let expired_reader_project = "44444444-4444-4444-8444-444444444444";
    let live_channel = format!("channel-{}-live", unique());
    let expired_parent_channel = format!("channel-{}-parent", unique());
    let expired_member_channel = format!("channel-{}-member", unique());
    let expired_reader_channel = format!("channel-{}-reader", unique());

    for conversation in [
        public_channel(
            &tenant,
            &region,
            &live_channel,
            live_project,
            "live-authority",
            &alice.principal_id.0,
        ),
        public_channel(
            &tenant,
            &region,
            &expired_parent_channel,
            expired_parent_project,
            "expired-parent",
            &alice.principal_id.0,
        ),
        public_channel(
            &tenant,
            &region,
            &expired_member_channel,
            expired_member_project,
            "expired-member",
            &alice.principal_id.0,
        ),
        public_channel(
            &tenant,
            &region,
            &expired_reader_channel,
            expired_reader_project,
            "expired-reader",
            &alice.principal_id.0,
        ),
    ] {
        conversations
            .create(&conversation)
            .await
            .expect("create the public channel metadata");
    }

    write_until(
        &tuples,
        &scope,
        &alice,
        vec![
            add(
                format!("project:{live_project}"),
                "reader",
                &alice.principal_id.0,
            ),
            add(
                format!("channel:{live_channel}"),
                "parent_project",
                format!("project:{live_project}#view"),
            ),
            add(
                format!("channel:{live_channel}"),
                "member",
                format!("project:{live_project}#view"),
            ),
            add(
                format!("project:{expired_parent_project}"),
                "reader",
                &alice.principal_id.0,
            ),
            add(
                format!("channel:{expired_parent_channel}"),
                "member",
                format!("project:{expired_parent_project}#view"),
            ),
            add(
                format!("project:{expired_member_project}"),
                "reader",
                &alice.principal_id.0,
            ),
            add(
                format!("channel:{expired_member_channel}"),
                "parent_project",
                format!("project:{expired_member_project}#view"),
            ),
            add(
                format!("channel:{expired_reader_channel}"),
                "parent_project",
                format!("project:{expired_reader_project}#view"),
            ),
            add(
                format!("channel:{expired_reader_channel}"),
                "member",
                format!("project:{expired_reader_project}#view"),
            ),
        ],
        None,
    )
    .await;
    write_until(
        &tuples,
        &scope,
        &alice,
        vec![
            add(
                format!("channel:{expired_parent_channel}"),
                "parent_project",
                format!("project:{expired_parent_project}#view"),
            ),
            add(
                format!("channel:{expired_member_channel}"),
                "member",
                format!("project:{expired_member_project}#view"),
            ),
            add(
                format!("project:{expired_reader_project}"),
                "reader",
                &alice.principal_id.0,
            ),
        ],
        Some(Timestamp("2020-01-01T00:01:00Z".into())),
    )
    .await;

    let visible = conversations
        .list_visible(&tenant, &region, &alice.principal_id.0, None, 100)
        .await
        .expect("list Alice's current public channels");
    assert_eq!(
        visible
            .iter()
            .map(|conversation| conversation.id.conversation_id.as_str())
            .collect::<Vec<_>>(),
        vec![live_channel.as_str()],
        "the user sees the live channel, while expired project, parent, and membership paths reveal nothing"
    );

    sqlx::query("DELETE FROM chat_conversation WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM rebac_tuple WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox WHERE envelope->>'tenant' = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_channels_are_visible_only_through_live_direct_membership() {
    let config = test_config();
    let admin = SubstrateProvider::connect(admin_config(&config), 4)
        .await
        .expect("connect to PostgreSQL for the private-channel story");
    admin.migrate_foundation().await.unwrap();
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap();
    admin
        .migrate(&chat_migrations(), &HotTables::none())
        .await
        .unwrap();
    let app = SubstrateProvider::connect(config, 6)
        .await
        .expect("connect the constrained runtime role");
    let region = app.config().region.clone();
    let tenant = format!("chat-private-{}", unique());
    let alice = principal(&tenant, &region, "p:alice");
    let scope = TenantScope::from_verified_token(&alice, alice.region.clone());
    let tuples = TupleStore::with_pg(
        DurableTupleBacking::new(app.clone()),
        tokio::runtime::Handle::current(),
    );
    let conversations = PgConversationStore::new(app.db_pool().clone());
    let ids = SystemUlidSource::new();
    let alice_channel = ids.mint().0;
    let bob_channel = ids.mint().0;
    let expired_channel = ids.mint().0;
    let missing_channel = ids.mint().0;

    for conversation in [
        private_channel(
            &tenant,
            &region,
            &alice_channel,
            "Alice and her agent",
            &alice.principal_id.0,
        ),
        private_channel(&tenant, &region, &bob_channel, "Bob and his agent", "p:bob"),
        private_channel(
            &tenant,
            &region,
            &expired_channel,
            "Alice's expired thread",
            &alice.principal_id.0,
        ),
    ] {
        conversations
            .create(&conversation)
            .await
            .expect("create private channel metadata");
    }
    write_until(
        &tuples,
        &scope,
        &alice,
        vec![
            add(
                format!("channel:{alice_channel}"),
                "member",
                &alice.principal_id.0,
            ),
            add(format!("channel:{bob_channel}"), "member", "p:bob"),
        ],
        None,
    )
    .await;
    write_until(
        &tuples,
        &scope,
        &alice,
        vec![add(
            format!("channel:{expired_channel}"),
            "member",
            &alice.principal_id.0,
        )],
        Some(Timestamp("2020-01-01T00:01:00Z".into())),
    )
    .await;

    let visible = conversations
        .list_visible(&tenant, &region, &alice.principal_id.0, None, 100)
        .await
        .expect("list Alice's private conversations");
    assert_eq!(
        visible
            .iter()
            .map(|conversation| conversation.id.conversation_id.as_str())
            .collect::<Vec<_>>(),
        vec![alice_channel.as_str()],
        "Alice sees her live private thread, never Bob's or an expired membership"
    );
    let exact = conversations
        .get_visible_exact(
            &tenant,
            &region,
            &alice.principal_id.0,
            &[
                bob_channel,
                alice_channel.clone(),
                expired_channel,
                alice_channel.clone(),
                missing_channel,
            ],
        )
        .await
        .expect("resolve Alice's exact private conversation references");
    assert_eq!(
        exact
            .iter()
            .map(|conversation| conversation.id.conversation_id.as_str())
            .collect::<Vec<_>>(),
        [alice_channel.as_str()],
        "exact resolution deduplicates ids and applies live membership before returning metadata"
    );

    sqlx::query("DELETE FROM chat_conversation WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM rebac_tuple WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox WHERE envelope->>'tenant' = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
}
