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

use myelin_edge::{serve_edge, AllowAll, Gateway, Method, WhoamiHandler};
use myelin_identity_service::{
    CapabilityAuthenticator, CellTokenAuthority, HumanSsoAuthenticator, PasetoCapabilityVerifier,
    PrincipalStore, RevocationStore,
};
use myelin_storage::KmsEngine;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // The REAL PASETO Bearer verifier over a freshly-generated cell authority (genuine Ed25519 crypto
    // — a forged token is rejected). The production cell-root LOAD + the seeded S1 directory is the
    // MR-015+ composition root; here a generated cell makes the bootable shell do real crypto.
    let cell = CellTokenAuthority::generate();
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        PrincipalStore::new(Arc::new(KmsEngine::new())),
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    // The refuse-not-mock production human verifier (login refuses until JWKS/trust-anchors land).
    let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(Arc::new(
        KmsEngine::new(),
    ))));

    let gateway = Arc::new(
        Gateway::builder(authn, human_login, Arc::new(AllowAll))
            .route(Method::Get, "/v1/whoami", "edge.whoami", Arc::new(WhoamiHandler))
            .route(
                Method::Get,
                "/v1/t/{tenant}/whoami",
                "edge.whoami",
                Arc::new(WhoamiHandler),
            )
            .sse_route("/v1/t/{tenant}/events", "edge.events.subscribe", "edge")
            .build(),
    );

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
