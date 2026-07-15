//! # `edge` — the edge-gateway binary (MR-014)
//!
//! The thin deployable shim: build the [`Gateway`] over the real auth components and serve it on a
//! TCP listener with the hyper transport. Like the other service mains it does NOTHING but compose +
//! hand off to the one transport call ([`serve_edge`]).
//!
//! **Floors named (EI-01 §4):** the production composition root that injects the cell trust anchor +
//! the seeded S1 principal store + the Identity-M1 authorizer (so real tokens authenticate against a
//! real directory) is the MR-015+ wiring; here the binary boots a structurally-complete gateway over
//! a freshly-generated cell authority + the refuse-not-mock human verifier + the `AllowAll` seam
//! authorizer, with the `whoami` proof handler + an SSE stream registered. The OS-signal → graceful
//! drain wiring lands with the rest of the transport. The bind address is `MYELIN_EDGE_ADDR` (default
//! `127.0.0.1:8080`).

use myelin_config::{Mode, MyelinConfig};
use myelin_edge::{
    register_git_durable, serve_edge, AllowAll, DurableGitBackend, Gateway, Method, WhoamiHandler,
};
use myelin_identity_service::{
    CapabilityAuthenticator, CellTokenAuthority, HumanSsoAuthenticator, PasetoCapabilityVerifier,
    PrincipalStore, RevocationStore,
};
use myelin_storage::{
    kms_durable_migrations, seal_key_from_env, DurableKmsBacking, DurablePrincipalBacking,
    DurableRevocationBacking, HotTables, SubstrateProvider,
};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // MR-009b Wave 2 — the DURABLE-BY-DEFAULT composition root: the identity S1 principal + S7
    // revocation stores are wired via `with_pg` over the MR-022 SubstrateProvider pool (the in-memory
    // doubles moved behind `test-support`, so the production edge binary never constructs them). The
    // provider connects to the dev docker stack by default (`MyelinConfig::from_env`); a boot that
    // cannot reach the durable pool FAILS LOUD (exit non-zero) — never a silent in-memory fallback.
    let config = MyelinConfig::from_env(Mode::DevDefaults).unwrap_or_else(|e| {
        eprintln!("edge: invalid config: {e}");
        std::process::exit(1);
    });
    let provider = match SubstrateProvider::connect(config, 8).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("edge: cannot reach the durable OLTP pool (durable-by-default requires PG): {e}");
            std::process::exit(1);
        }
    };
    let handle = tokio::runtime::Handle::current();
    // The shared cell KMS (the crypto-shred substrate) — DURABLE-BY-DEFAULT (MR-009b Wave 5 /
    // SI-006): the software-sealed root + wrapped KEKs/DEKs load from the `kms_sealed_root`/
    // `kms_wrapped_kek`/`kms_wrapped_dek` tables via `load_or_generate` (MR-025), and every key
    // minted at runtime writes through — keys survive a kill-9 restart. FAIL LOUD, never a silent
    // in-memory fallback: a missing/malformed MYELIN_KMS_SEAL_KEY, an unreachable store, or a
    // sealed root that does not unseal under the supplied key (WrongSealKey — fail-closed, NEVER a
    // fresh root that would orphan every existing ciphertext) each exit non-zero.
    let seal_key = match seal_key_from_env() {
        Ok(k) => k,
        Err(e) => {
            eprintln!("edge: KMS refused to start (durable-by-default requires the seal key): {e}");
            std::process::exit(1);
        }
    };
    // The KMS tables are forward-only + idempotent — apply them at boot via the MR-022 migrator.
    if let Err(e) = provider
        .migrate(&kms_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("edge: cannot apply the durable KMS migrations: {e}");
        std::process::exit(1);
    }
    // The cell whose sealed root this edge serves (a namespace, not a secret — dev default).
    let cell_id = std::env::var("MYELIN_CELL_ID").unwrap_or_else(|_| "cell-dev".to_string());
    let kms_backing = DurableKmsBacking::new(provider.db_pool().clone(), cell_id);
    let kms = match kms_backing.load_or_generate(&seal_key).await {
        Ok(engine) => Arc::new(engine),
        Err(e) => {
            eprintln!("edge: KMS refused to start (fail-closed, never a silent in-memory engine): {e}");
            std::process::exit(1);
        }
    };

    // The REAL PASETO Bearer verifier over a freshly-generated cell authority (genuine Ed25519 crypto
    // — a forged token is rejected). The production cell-root LOAD + the seeded S1 directory is the
    // MR-015+ composition root; here a generated cell makes the bootable shell do real crypto.
    let cell = CellTokenAuthority::generate();
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        PrincipalStore::with_pg(
            kms.clone(),
            DurablePrincipalBacking::new(provider.clone()),
            handle.clone(),
        ),
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::with_pg(DurableRevocationBacking::new(provider.clone()), handle.clone()),
    ));
    // The refuse-not-mock production human verifier (login refuses until JWKS/trust-anchors land),
    // over the durable S1 principal directory.
    let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(provider),
        handle,
    )));

    // The Git subsystem wired through the edge over the DURABLE on-disk backend (GT-003): its
    // `/v1/git/...` write handlers PERSIST on real on-disk bare repos (GT-001) under the verified tenant
    // scope + the merge-gate/fork-trust policy, and its reads reflect the durable state. The on-disk root
    // is `MYELIN_GIT_ROOT` (default a per-host data dir) — the same `<tenant>/<region>/<repo>.git` layout
    // the read backend resolves against. The reconciler (`myelin_git::reconcile`) heals the
    // apply-after-outbox-commit window before the store serves; the cross-restart recovery runs in the
    // production composition root (here the shell boots a fresh-or-existing durable backend).
    let git_root = std::env::var("MYELIN_GIT_ROOT")
        .unwrap_or_else(|_| std::env::temp_dir().join("myelin-git-data").to_string_lossy().into());
    let git_backend = Arc::new(DurableGitBackend::rooted(git_root));

    let mut builder = Gateway::builder(authn, human_login, Arc::new(AllowAll))
        .route(Method::Get, "/v1/whoami", "edge.whoami", Arc::new(WhoamiHandler))
        .route(
            Method::Get,
            "/v1/t/{tenant}/whoami",
            "edge.whoami",
            Arc::new(WhoamiHandler),
        )
        .sse_route("/v1/t/{tenant}/events", "edge.events.subscribe", "edge");
    builder = register_git_durable(builder, git_backend);
    let gateway = Arc::new(builder.build());

    let addr = std::env::var("MYELIN_EDGE_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("edge: failed to bind {addr}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("edge: listening on {addr}");
    if let Err(e) = serve_edge(listener, gateway).await {
        eprintln!("edge: serve error: {e}");
        std::process::exit(1);
    }
}
