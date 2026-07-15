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
    register_git_durable, register_git_wire, serve_edge, AllowAll, DurableGitBackend,
    GitCheckRepoAuthorizer, Gateway, Method, TupleStoreGrantWriter, WhoamiHandler,
};
use myelin_events::OutboxStore;
use myelin_git::live_check::GitCheckGate;
use myelin_identity::FragmentAdmit;
use myelin_identity_service::{
    CapabilityAuthenticator, CellTokenAuthority, HumanSsoAuthenticator, PasetoCapabilityVerifier,
    PrincipalStore, RevocationStore, StoreBackedCheck, TupleStore,
};
use myelin_storage::{
    all_durable_migrations, seal_key_from_env, DurableKmsBacking, DurablePrincipalBacking,
    DurableRevocationBacking, DurableTupleBacking, HotTables, PgOutboxBacking, SubstrateProvider,
};
use myelin_substrate::Thresholds;
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
            eprintln!(
                "edge: cannot reach the durable OLTP pool (durable-by-default requires PG): {e}"
            );
            std::process::exit(1);
        }
    };
    let handle = tokio::runtime::Handle::current();
    // MR-009b W3b.4 — the DURABLE transactional outbox (SI-007): the git backend's ref-CAS
    // co-commits its `git.ref.updated` into the PG-backed `outbox` table (survives restart), never
    // a per-process in-memory buffer. The substrate foundation tables (the frozen `outbox` +
    // `consumer_dedup` DDL) are applied first through the MR-022 migrator — FAIL LOUD, no fallback.
    if let Err(e) = provider.migrate_foundation().await {
        eprintln!(
            "edge: cannot apply the substrate foundation migrations (outbox/consumer_dedup): {e}"
        );
        std::process::exit(1);
    }
    // W7.2 (doc-18 Part 5) — THE BOOT-MIGRATIONS FIX: apply the FULL durable migration aggregate
    // (identity 0010–0019, pseudonym 0020–0022, placement 0030–0039, kms 0040–0042, cost/erasure
    // 0050–0053) right after the foundation, so every durable store this main constructs has its
    // tables. Previously this main migrated ONLY foundation + KMS, so the identity tables the
    // `PrincipalStore::with_pg`/`RevocationStore::with_pg` stores below bind to (`principal`,
    // `revocation`, …) were NEVER migrated — the first principal write failed at runtime on a fresh
    // DB. The aggregate is idempotent + advisory-locked (safe on re-boot). FAIL LOUD, no fallback.
    if let Err(e) = provider
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("edge: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    let git_outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        handle.clone(),
    )));
    // THE W3b.3 NAMED CONDITION: the composition root wires the UNIQUE production id source
    // (`UlidMinter`, the P-S12 stand-in) — NEVER the default `MonotonicMinter`, which resets per
    // instance so two roots mint colliding `event_id`s that the durable co-commit path's
    // `ON CONFLICT (event_id) DO NOTHING` silently DROPS (probe-proven in W3b.3).
    let git_minter: Arc<dyn myelin_events::IdMinter> = Arc::new(myelin_events::UlidMinter::new());
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
    // The KMS tables (0040–0042) are migrated by the W7.2 durable aggregate applied above — no
    // separate per-group KMS migrate call here anymore (the aggregate folds it in exactly once).
    // The cell whose sealed root this edge serves (a namespace, not a secret — dev default).
    let cell_id = std::env::var("MYELIN_CELL_ID").unwrap_or_else(|_| "cell-dev".to_string());
    let kms_backing = DurableKmsBacking::new(provider.db_pool().clone(), cell_id);
    let kms = match kms_backing.load_or_generate(&seal_key).await {
        Ok(engine) => Arc::new(engine),
        Err(e) => {
            eprintln!(
                "edge: KMS refused to start (fail-closed, never a silent in-memory engine): {e}"
            );
            std::process::exit(1);
        }
    };

    // The REAL PASETO Bearer verifier over a freshly-generated cell authority (genuine Ed25519 crypto
    // — a forged token is rejected). The production cell-root LOAD + the seeded S1 directory is the
    // MR-015+ composition root; here a generated cell makes the bootable shell do real crypto. Held in
    // an `Arc` so the SAME cell authority roots both the token verifier AND the git-wire check slot's
    // run-token signer (R2.1a — one cell trust anchor, one signing key).
    let cell = Arc::new(CellTokenAuthority::generate());
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        PrincipalStore::with_pg(
            kms.clone(),
            DurablePrincipalBacking::new(provider.clone()),
            handle.clone(),
        ),
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::with_pg(
            DurableRevocationBacking::new(provider.clone()),
            handle.clone(),
        ),
    ));
    // The refuse-not-mock production human verifier (login refuses until JWKS/trust-anchors land),
    // over the durable S1 principal directory. `provider`/`handle` are CLONED (not moved) so the
    // R2.1a git-wire authz slot below wires its durable S3 tuple store + S7 denylist over the same pool.
    let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(provider.clone()),
        handle.clone(),
    )));

    // The Git subsystem wired through the edge over the DURABLE on-disk backend (GT-003): its
    // `/v1/git/...` write handlers PERSIST on real on-disk bare repos (GT-001) under the verified tenant
    // scope + the merge-gate/fork-trust policy, and its reads reflect the durable state. The on-disk root
    // is `MYELIN_GIT_ROOT` (default a per-host data dir) — the same `<tenant>/<region>/<repo>.git` layout
    // the read backend resolves against. The reconciler (`myelin_git::reconcile`) heals the
    // apply-after-outbox-commit window before the store serves; the cross-restart recovery runs in the
    // production composition root (here the shell boots a fresh-or-existing durable backend).
    let git_root = std::env::var("MYELIN_GIT_ROOT").unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("myelin-git-data")
            .to_string_lossy()
            .into()
    });
    // ── R2.1a — THE FLIP: wire the R0.2/R0.3 git-wire security gates LIVE in production. ──
    // The per-repo object-authz seam ([`RepoAuthorizer`]) is now backed by the REAL Identity `check`
    // over the durable ReBAC tuple store — the doctrinal `GitCheckGate` (one engine, one fail-static
    // cache, one check path). An in-tenant principal with NO grant on repo X can no longer clone/fetch
    // (0-leak 404) or push (403) X; the creator gets a bootstrap admin grant so their repo is usable.
    //
    // The durable S3 tuple store + S7 denylist ride the same MR-022 provider pool as the identity
    // stores. The frozen Git ReBAC fragment (repo/ref/pull_request/pr_comment) is admitted at boot so
    // `pull`/`push` resolve their rich rewrites — FAIL LOUD if admission rejects (never a silent
    // vacuous authorizer). The FailStatic bound (`static_max ≤ revocation SLA`) is sourced from the
    // canonical thresholds file; a bound violation does NOT construct (fail loud).
    let git_check =
        StoreBackedCheck::with_pg(provider.clone(), kms.clone(), cell.clone(), handle.clone());
    for admit in git_check.admit_git_fragment() {
        if let FragmentAdmit::Rejected { reason } = admit {
            eprintln!("edge: the Git ReBAC fragment failed to admit (the wire authz would be vacuous): {reason}");
            std::process::exit(1);
        }
    }
    // The wire authorizer consults the SAME S7 denylist the check slot does (one revocation oracle).
    let git_revocations = git_check.revocations().clone();
    let thresholds = Thresholds::load_canonical().unwrap_or_else(|e| {
        eprintln!("edge: cannot load the canonical thresholds file (the fail-static bound source): {e}");
        std::process::exit(1);
    });
    // N (the revocation SLA) in seconds — the upper bound on the fail-static staleness window.
    let revocation_sla_secs = thresholds.revocation.sla_mins * 60;
    let git_gate = GitCheckGate::try_new(git_check, revocation_sla_secs, &thresholds.fail_static)
        .unwrap_or_else(|e| {
            eprintln!("edge: the git→Id check gate rejected the fail-static bound (static_max ≤ revocation SLA): {e:?}");
            std::process::exit(1);
        });
    let repo_authorizer = Arc::new(GitCheckRepoAuthorizer::new(git_gate, git_revocations));
    // The create-repo bootstrap-grant writer over the durable `rebac_tuple` store (the SAME backing the
    // check reads) — a fresh repo's creator→admin grant is co-committed through the outbox (contract 4.6).
    let grant_writer = Arc::new(TupleStoreGrantWriter::new(TupleStore::with_pg(
        DurableTupleBacking::new(provider.clone()),
        handle.clone(),
    )));

    let git_backend = Arc::new(
        DurableGitBackend::rooted(git_root, git_outbox, git_minter)
            .with_repo_authorizer(repo_authorizer)
            .with_grant_writer(grant_writer),
    );

    let mut builder = Gateway::builder(authn, human_login, Arc::new(AllowAll))
        .route(
            Method::Get,
            "/v1/whoami",
            "edge.whoami",
            Arc::new(WhoamiHandler),
        )
        .route(
            Method::Get,
            "/v1/t/{tenant}/whoami",
            "edge.whoami",
            Arc::new(WhoamiHandler),
        )
        .sse_route("/v1/t/{tenant}/events", "edge.events.subscribe", "edge");
    // The durable `/v1/git/...` product routes AND the git smart-HTTP wire routes share the SAME
    // backend Arc (one authorizer, one grant-writer, one on-disk root). register_git_wire is THE FLIP:
    // the clone/fetch/push wire now exists in production, gated by the live per-repo authorizer above.
    builder = register_git_durable(builder, git_backend.clone());
    builder = register_git_wire(builder, git_backend);
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
