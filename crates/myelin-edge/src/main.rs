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
    register_git_durable, register_git_wire, serve_edge, AllowAll, CheckBackedRepoAuthorizer,
    DurableGitBackend, Gateway, Method, TupleRepoBootstrap, WhoamiHandler,
};
use myelin_events::OutboxStore;
use myelin_identity::FragmentAdmit;
use myelin_identity_service::{
    CapabilityAuthenticator, CellTokenAuthority, HumanSsoAuthenticator, JwkSet, OidcConfig,
    PasetoCapabilityVerifier, PrincipalStore, RevocationStore, StoreBackedCheck,
};
use myelin_storage::{
    all_durable_migrations, seal_key_from_env, DurableKmsBacking, DurablePrincipalBacking,
    DurableRevocationBacking, HotTables, PgOutboxBacking, SubstrateProvider,
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
    // MR-015+ composition root; here a generated cell makes the bootable shell do real crypto.
    // Arc'd because the R2.1a StoreBackedCheck (the per-run-token minter inside it) shares the SAME
    // cell authority the Bearer verifier trusts — one cell, one trust anchor.
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
    // R2.5 — the human/SSO login over the durable S1 principal directory. If an OIDC IdP is
    // configured (`MYELIN_OIDC_ISSUER` + `MYELIN_OIDC_AUDIENCE` + a static JWKS via
    // `MYELIN_OIDC_JWKS`/`MYELIN_OIDC_JWKS_FILE`), the REAL OidcVerifier is wired for the `oidc`
    // scheme — a genuinely IdP-signed ID token authenticates (tenant/region from the VERIFIED
    // claims, never a path). If OIDC is UNCONFIGURED, login stays refuse-not-mock (every scheme
    // refuses) — boot still succeeds (OIDC login is opt-in). A configured-but-MALFORMED JWKS JSON is
    // a FAIL-LOUD boot abort (never a silent no-OIDC fallback), matching the rest of this main.
    let oidc_settings = provider.config().oidc.clone();
    let human_store = PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(provider.clone()),
        handle.clone(),
    );
    let human_login = Arc::new(match oidc_settings {
        Some(oidc) => {
            let jwks = JwkSet::from_jwks_json(&oidc.jwks_json).unwrap_or_else(|e| {
                eprintln!(
                    "edge: OIDC is configured but the JWKS JSON \
                     (MYELIN_OIDC_JWKS/MYELIN_OIDC_JWKS_FILE) is malformed: {e:?}"
                );
                std::process::exit(1);
            });
            eprintln!(
                "edge: OIDC login wired (issuer={}, {} JWKS key(s))",
                oidc.issuer,
                jwks.len()
            );
            HumanSsoAuthenticator::production_with_oidc(
                human_store,
                Some((OidcConfig::new(oidc.issuer, oidc.audience), jwks)),
            )
        }
        None => {
            eprintln!("edge: OIDC not configured — human login refuses (refuse-not-mock)");
            HumanSsoAuthenticator::production_with_oidc(human_store, None)
        }
    });

    // ── R2.1a — the LIVE per-repo object authz (R0.3) + the git wire endpoints (R0.2 fires with
    // them). The production `check` slot: the depth-bounded Zanzibar engine over the DURABLE S3
    // tuple store (`rebac_tuple`) + S7 revocation denylist, through the SAME provider pool/KMS/cell
    // this main already composes. FAIL LOUD on every construction step — never a silent
    // allow-everything git wire. ──
    let check =
        StoreBackedCheck::with_pg(provider.clone(), kms.clone(), cell.clone(), handle.clone());
    // Admit the frozen Git ReBAC fragment (contract 4.9) so the compiled `pull`/`push`/
    // `protected_push` permissions resolve; an un-admitted fragment would deny every repo (fail-
    // closed but useless) — a rejected admit is a boot failure, loudly.
    for admit in check.admit_git_fragment() {
        if let FragmentAdmit::Rejected { reason } = admit {
            eprintln!("edge: the Git ReBAC fragment did not admit (authz would deny everything): {reason}");
            std::process::exit(1);
        }
    }
    // The fail-static bound (contract 1.10/4.11) from the canonical thresholds file:
    // static_max ≤ revocation SLA, enforced structurally by the gate constructor (P-S18).
    let thresholds = match myelin_substrate::Thresholds::load_canonical() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("edge: cannot load the canonical thresholds file (the fail-static bound for the git-wire authz): {e}");
            std::process::exit(1);
        }
    };
    let revocation_sla_secs = thresholds.revocation.sla_mins * 60;
    // The production RepoAuthorizer: Read → check(pull), Write → check(push), against the live
    // fragment through the GIT-P14 fail-static gate. Replaces the AllowAllRepos fixture at the
    // wire seam — deny-by-default, per-repo, tenant-partitioned.
    let repo_authz = match CheckBackedRepoAuthorizer::try_new(
        check.clone(),
        revocation_sla_secs,
        &thresholds.fail_static,
    ) {
        Ok(a) => Arc::new(a),
        Err(e) => {
            eprintln!(
                "edge: the git-wire repo authorizer refused to construct (staleness bound): {e:?}"
            );
            std::process::exit(1);
        }
    };
    // The creator→admin bootstrap grant writer (over the SAME S3 store the checker reads): a repo
    // created through the edge is immediately reachable by its creator — without this, deny-by-
    // default would orphan every fresh repo (the R2.1a make-or-break).
    let repo_bootstrap = Arc::new(TupleRepoBootstrap::new(check.tuples().clone()));

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
    // R2.1a: the durable git backend carries the LIVE CheckEngine-backed per-repo authorizer (R0.3
    // no longer latent — AllowAllRepos is out of the production composition root) + the bootstrap
    // grant writer the create path consults.
    let git_backend = Arc::new(
        DurableGitBackend::rooted(git_root, git_outbox, git_minter)
            .with_repo_authorizer(repo_authz)
            .with_repo_bootstrap(repo_bootstrap),
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
    builder = register_git_durable(builder, git_backend.clone());
    // R2.1a: mount the git smart-HTTP WIRE endpoints (info/refs + upload-pack + receive-pack) in
    // PROD — previously only the oracle tests registered them, so clone/fetch/push did not exist on
    // the production edge at all. With the wire live, the R0.2 branch-protection gate fires on the
    // receive-pack path (`evaluate_protected_ref_push`) and the R0.3 per-repo authorizer above
    // gates every wire byte (READ-deny → 0-leak 404, WRITE-deny → 403).
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
