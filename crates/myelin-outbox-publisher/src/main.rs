use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use myelin_events::nats::{JetStreamProvisioner, NatsJetStreamPublisher};
use myelin_outbox_publisher::{
    ElectedPublisher, PublisherConfig, PublisherDbProvider, PublisherRuntime,
};

#[derive(Clone, Copy)]
enum Mode {
    Provision,
    Serve,
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
