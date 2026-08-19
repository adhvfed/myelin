use std::sync::Arc;

use myelin_agent_service::trigger_consumer::durable::{
    DurableApprovalInbox, DurableOwnerVisibility, DurableTriggerBindingStore,
};
use myelin_agent_service::{
    governed_trigger_consumer_reg, run_agent_ingestion_until_shutdown, trigger_intake_filter,
    EVENT_DURABLE_CONSUMER, EVENT_STREAM_NAME, EVENT_SUBJECT_ROOT,
};
use myelin_config::Mode;
use myelin_events::nats::{JetStreamConsumerConfig, NatsJetStreamBus};
use myelin_events::{DedupLedger, OutboxStore};
use myelin_identity::FragmentAdmit;
use myelin_identity_service::{CellTokenAuthority, StoreBackedCheck};
use myelin_storage::{
    all_durable_migrations, seal_key_from_env, DurableAgentTriggerBacking, DurableCellRootBacking,
    DurableKmsBacking, DurablePlacementBacking, HotTables, PgBootstrap, PgOutboxBacking,
};
use myelin_substrate::Config;
use myelin_tenancy::{Region, TenantId};

#[tokio::main]
async fn main() {
    myelin_events::install_payload_free_panic_hook("agent-service");
    let bootstrap = PgBootstrap::from_env(Mode::RequireEnv)
        .await
        .unwrap_or_else(|error| refuse_start("database bootstrap", error));
    bootstrap
        .migrate_foundation()
        .await
        .unwrap_or_else(|error| refuse_start("substrate foundation migration", error));
    bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap_or_else(|error| refuse_start("durable migration aggregate", error));
    let provider = bootstrap
        .into_runtime()
        .await
        .unwrap_or_else(|error| refuse_start("database runtime handoff", error));

    let cell_id = required_cell_id().unwrap_or_else(|error| refuse_start("cell binding", error));
    let tenants = DurablePlacementBacking::new(provider.db_pool().clone())
        .local_tenants(&cell_id)
        .await
        .unwrap_or_else(|error| refuse_start("local-tenant directory", error))
        .into_iter()
        .filter(|placement| placement.active)
        .map(|placement| TenantId(placement.tenant_id))
        .collect::<Vec<_>>();
    if tenants.is_empty() {
        refuse_start(
            "local-tenant directory",
            format!("cell `{cell_id}` has no active tenants"),
        );
    }

    let seal_key = seal_key_from_env().unwrap_or_else(|error| refuse_start("seal key", error));
    let kms = Arc::new(
        DurableKmsBacking::new(provider.db_pool().clone(), cell_id.clone())
            .load_or_generate(&seal_key)
            .await
            .unwrap_or_else(|error| refuse_start("durable KMS", error)),
    );
    let cell_material = DurableCellRootBacking::new(provider.db_pool().clone(), cell_id)
        .load_or_generate(&seal_key)
        .await
        .unwrap_or_else(|error| refuse_start("cell token authority", error));
    let cell = Arc::new(
        CellTokenAuthority::from_material(&cell_material).unwrap_or_else(|error| {
            refuse_start("cell token authority material", format!("{error:?}"))
        }),
    );

    let runtime = tokio::runtime::Handle::current();
    let identity = StoreBackedCheck::with_pg(provider.clone(), kms, cell, runtime.clone());
    for (name, admissions) in [
        ("Git", identity.admit_git_fragment()),
        ("Issues", identity.admit_issue_fragment()),
        ("Knowledge", identity.admit_knowledge_fragment()),
        ("Chat", identity.admit_chat_fragment()),
    ] {
        for admission in admissions {
            if let FragmentAdmit::Rejected { reason } = admission {
                refuse_start(&format!("{name} authorization schema"), reason);
            }
        }
    }
    let runs =
        myelin_ci_controlplane::ci_run_store::CiRunStore::with_pg(provider.db_pool().clone());
    let trigger_store: Arc<dyn myelin_agent_service::trigger_consumer::TriggerBindingStore> =
        Arc::new(DurableTriggerBindingStore::new(
            DurableAgentTriggerBacking::new(provider.clone()),
            runtime.clone(),
        ));
    let visibility: Arc<dyn myelin_agent_service::trigger_consumer::TriggerOwnerVisibility> =
        Arc::new(DurableOwnerVisibility::new(
            provider.clone(),
            runs,
            identity,
            runtime.clone(),
        ));
    let approvals: Arc<dyn myelin_agent_service::trigger_consumer::TriggerApprovalInbox> =
        Arc::new(DurableApprovalInbox::new(
            myelin_notif::pg_inbox::PgInboxStore::new(provider.db_pool().clone()),
            runtime.clone(),
        ));
    let dedup = DedupLedger::durable(Arc::new(
        myelin_storage::events_durable::DurableDedupBacking::new(
            provider.db_pool().clone(),
            runtime.clone(),
        ),
    ));
    let dead_letters: Arc<dyn myelin_events::DurableDeadLetter> = Arc::new(
        myelin_storage::events_durable::DurableDeadLetterBacking::new(
            provider.db_pool().clone(),
            runtime.clone(),
        ),
    );
    let region = Region(provider.config().region.clone());
    let consumers = tenants
        .iter()
        .map(|tenant| {
            governed_trigger_consumer_reg(
                tenant,
                &region,
                trigger_store.clone(),
                visibility.clone(),
                approvals.clone(),
                dedup.clone(),
                dead_letters.clone(),
            )
            .unwrap_or_else(|error| {
                refuse_start(
                    "tenant-bound trigger consumer",
                    format!("{}: {error:?}", tenant.0),
                )
            })
        })
        .collect();
    let intake = NatsJetStreamBus::connect_consumer(
        JetStreamConsumerConfig::bounded(
            &provider.config().nats_url,
            EVENT_STREAM_NAME,
            EVENT_SUBJECT_ROOT,
            trigger_intake_filter(),
            EVENT_DURABLE_CONSUMER,
        ),
        runtime.clone(),
    )
    .unwrap_or_else(|error| refuse_start("durable event intake", format!("{error:?}")));
    let quarantine: Arc<dyn myelin_events::DurableDeliveryQuarantine> = Arc::new(
        myelin_storage::events_durable::DurableDeliveryQuarantineBacking::new(
            provider.db_pool().clone(),
            runtime.clone(),
        ),
    );
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        runtime,
    )));

    run_agent_ingestion_until_shutdown(
        Config::default(),
        outbox,
        consumers,
        Box::new(intake),
        quarantine,
        Some(myelin_agent_service::placed_tenant_intake_scope(&tenants)),
        shutdown_signal(),
    )
    .await
    .unwrap_or_else(|error| refuse_start("service loop", error));
}

fn required_cell_id() -> Result<String, &'static str> {
    let value = std::env::var("MYELIN_CELL_ID").map_err(|_| "MYELIN_CELL_ID is required")?;
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err("MYELIN_CELL_ID must be a trimmed opaque token of at most 128 bytes");
    }
    Ok(value)
}

fn refuse_start(context: &str, error: impl std::fmt::Display) -> ! {
    eprintln!("agent-service: {context} refused to start: {error}");
    std::process::exit(1)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .unwrap_or_else(|error| refuse_start("SIGTERM handler", error));
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    refuse_start("SIGINT handler", error);
                }
            }
            signal = terminate.recv() => {
                if signal.is_none() {
                    refuse_start("SIGTERM stream", "closed unexpectedly");
                }
            }
        }
    }
    #[cfg(not(unix))]
    if let Err(error) = tokio::signal::ctrl_c().await {
        refuse_start("shutdown handler", error);
    }
}
