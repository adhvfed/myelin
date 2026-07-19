use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use myelin_events::nats::{JetStreamProvisioner, NatsJetStreamPublisher};
use myelin_outbox_publisher::{
    ElectedPublisher, GitRefV2OperatorFence, PublisherConfig, PublisherDbProvider, PublisherRuntime,
};

#[derive(Clone, Copy)]
enum Mode {
    Provision,
    Serve,
    PreflightGitRefV2,
}

#[tokio::main]
async fn main() {
    let mode = match parse_mode() {
        Ok(mode) => mode,
        Err(()) => {
            eprintln!("myelin-outbox-publisher: expected exactly one mode: provision or serve");
            std::process::exit(2);
        }
    };
    let config = match PublisherConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("myelin-outbox-publisher: configuration refused: {error}");
            std::process::exit(1);
        }
    };
    let rt = tokio::runtime::Handle::current();
    if matches!(mode, Mode::Provision) {
        if JetStreamProvisioner::ensure(config.provision_nats_config(), rt).is_err() {
            eprintln!("myelin-outbox-publisher: JetStream provisioning refused");
            std::process::exit(1);
        }
        return;
    }

    let provider = match PublisherDbProvider::connect(&config).await {
        Ok(provider) => provider,
        Err(error) => {
            eprintln!("myelin-outbox-publisher: database capability refused: {error}");
            std::process::exit(1);
        }
    };
    if matches!(mode, Mode::PreflightGitRefV2) {
        let acknowledged = |name: &str| std::env::var(name).as_deref() == Ok("acknowledged");
        let fence = GitRefV2OperatorFence {
            consumer_upcaster_active: acknowledged("MYELIN_GIT_REF_V2_UPCASTER_ACTIVE"),
            writer_quiesced: acknowledged("MYELIN_GIT_REF_V2_WRITER_QUIESCED"),
        };
        if let Err(error) = provider
            .preflight_git_ref_v2(&config, "ci-dispatch-trigger", fence, rt)
            .await
        {
            eprintln!("myelin-outbox-publisher: Git ref v2 preflight refused: {error}");
            std::process::exit(1);
        }
        println!("myelin-outbox-publisher: Git ref v2 activation barrier is clear");
        return;
    }
    let relay = match provider.elected_relay(config.region(), config.max_envelope_bytes()) {
        Ok(relay) => relay,
        Err(error) => {
            eprintln!("myelin-outbox-publisher: relay configuration refused: {error}");
            std::process::exit(1);
        }
    };
    let publisher = match NatsJetStreamPublisher::connect_existing(config.publish_nats_config(), rt)
    {
        Ok(publisher) => publisher,
        Err(_) => {
            eprintln!("myelin-outbox-publisher: publish runtime refused");
            std::process::exit(1);
        }
    };
    let runtime = PublisherRuntime::new(
        ElectedPublisher::new(relay, publisher, config.batch()),
        config.poll(),
        config.backoff(),
        config.pass_timeout(),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let signal_stop = stop.clone();
    let signal = tokio::spawn(async move {
        shutdown_signal().await;
        signal_stop.store(true, Ordering::SeqCst);
    });
    runtime.serve_until(&stop).await;
    signal.abort();
}

fn parse_mode() -> Result<Mode, ()> {
    let mut args = std::env::args().skip(1);
    let mode = match args.next().as_deref() {
        Some("provision") => Mode::Provision,
        Some("serve") => Mode::Serve,
        Some("preflight-git-ref-v2") => Mode::PreflightGitRefV2,
        _ => return Err(()),
    };
    if args.next().is_some() {
        Err(())
    } else {
        Ok(mode)
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                    return;
                }
            };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
