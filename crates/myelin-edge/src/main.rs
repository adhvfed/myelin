//! # `edge` — the edge-gateway binary (MR-014 / R4.0)
//!
//! The thin deployable shim. With NO arguments it composes the [`Gateway`] over the real auth
//! components and SERVES it on a TCP listener (as before). With the `bootstrap` / `revoke` operator
//! subcommands it runs the SAME config/provider/migrations/KMS/cell-root composition and then performs
//! an operator-plane action instead of serving:
//!
//! - `edge` (no args) → serve.
//! - `edge bootstrap --tenant <t> --principal <id> --issues-project <uuid> [--display-name <s>]
//!   [--ttl-days <n>] [--region <r>]`
//!   → idempotently seed the principal + mint a capability token that authenticates against a
//!   SEPARATELY-running serving edge (same DB + seal key). Prints the token to STDOUT exactly once.
//! - `edge revoke --jti <jti> --tenant <t> [--region <r>]` → durably revoke a token (S7 denylist).
//!
//! **The R4.0 make-or-break (P-527 / MR-025 follow-on):** the cell token authority is now DURABLE —
//! [`DurableCellRootBacking::load_or_generate`] recovers the sealed Ed25519 seed + macaroon MAC key
//! from the `cell_token_root` row under the SAME `MYELIN_KMS_SEAL_KEY` the KMS root uses, so a token
//! minted before a restart still verifies after it (the old `CellTokenAuthority::generate()` was
//! ephemeral per boot, orphaning every minted token — no mint path could exist). Fail-loud on every
//! error path; a wrong/absent seal key is fail-closed and NEVER regenerates the root.
//!
//! **The operator trust boundary (stated):** anyone with the runtime `DATABASE_URL`, the migration
//! `DATABASE_MIGRATION_URL`, and the seal key can run `bootstrap`/`revoke` and mint/revoke for any
//! principal. That is ACCEPTED operator-plane infrastructure; there is deliberately NO HTTP endpoint
//! that mints. The migration credential is destroyed before any serving store or listener exists.

use myelin_config::Mode;
use myelin_edge::{
    bootstrap_principal_and_mint, recover_placed_git_at_boot, register_git_durable,
    register_git_wire, register_issues, serve_edge, spawn_issue_authorization_reconciler,
    AuthProvider, AuthPublicConfig, AuthenticatedActionPolicy, BootstrapParams,
    CheckBackedRepoAuthorizer, DurableGitBackend, Gateway, IssueReconciliationConfig, Method,
    StoreBackedIssueAuthorizer, TupleRepoBootstrap, WhoamiHandler,
};
use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{FragmentAdmit, Principal, PrincipalId, PrincipalKind, RevokeTarget};
use myelin_identity_service::{
    CapabilityAuthenticator, CellTokenAuthority, HumanSsoAuthenticator, JwkSet, OidcConfig,
    PasetoCapabilityVerifier, PrincipalStore, RevocationStore, StoreBackedCheck, TupleStore,
};
use myelin_storage::{
    all_durable_migrations, seal_key_from_env, DurableCellRootBacking, DurableKmsBacking,
    DurablePrincipalBacking, DurableRevocationBacking, DurableTupleBacking, HotTables, KmsEngine,
    PgBootstrap, PgOutboxBacking, SubstrateProvider, TenantScope,
};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

/// The default operator-token scheme the edge resolves a bearer/basic credential under when the client
/// sends no `X-Myelin-Token-Scheme` header. R4.0 sets this to `agent` (the scheme `edge bootstrap`
/// mints under): it is the only mintable token type today (a `pat` requires DPoP that git/curl cannot
/// produce; human login is deferred), so a bootstrap token authenticates over git (Bearer or Basic) and
/// curl with ZERO extra headers. Every edge test already sets its own scheme explicitly, so this is a
/// no-op for them; a real `pat` presented WITH an explicit `pat` scheme header is still DPoP-enforced.
const EDGE_DEFAULT_TOKEN_SCHEME: &str = "agent";

// =================================================================================================
// Shared composition — the config/provider/migrations/KMS/cell-root core BOTH serve and the operator
// subcommands build. Fail-loud (exit non-zero) on every error, never a silent fallback.
// =================================================================================================

/// The composed durable core: the provider pool + the durable KMS engine + the DURABLE cell token
/// authority + the runtime handle. Built once by [`compose_core`]; consumed by [`serve`] /
/// [`operator_bootstrap`] / [`operator_revoke`].
struct ComposedCore {
    provider: SubstrateProvider,
    kms: Arc<KmsEngine>,
    cell: Arc<CellTokenAuthority>,
    cell_id: String,
    handle: tokio::runtime::Handle,
}

/// Build the shared durable core (the DURABLE-BY-DEFAULT composition root, MR-009b + R4.0). Validates
/// separate migration/runtime roles, applies the substrate foundation + the full durable migration
/// aggregate (now including `0060_cell_token_root`), then destroys the privileged pool before any
/// runtime store is built. The KMS root and CELL TOKEN AUTHORITY ROOT are sealed under
/// `MYELIN_KMS_SEAL_KEY` and fail closed on a wrong/absent key (NEVER a fresh root).
async fn compose_core() -> ComposedCore {
    // This is a production binary: every endpoint and both PostgreSQL credentials must be explicit.
    // Pair validation runs before DDL and before the edge can bind its serving port.
    let bootstrap = match PgBootstrap::from_env(Mode::RequireEnv).await {
        Ok(bootstrap) => bootstrap,
        Err(e) => {
            eprintln!("edge: database bootstrap refused to start: {e}");
            std::process::exit(1);
        }
    };
    let handle = tokio::runtime::Handle::current();
    // The substrate foundation (outbox/consumer_dedup) first — FAIL LOUD, no fallback.
    if let Err(e) = bootstrap.migrate_foundation().await {
        eprintln!(
            "edge: cannot apply the substrate foundation migrations (outbox/consumer_dedup): {e}"
        );
        std::process::exit(1);
    }
    // The FULL durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure + the R4.0
    // `0060_cell_token_root`). Idempotent + advisory-locked (safe on re-boot). FAIL LOUD.
    if let Err(e) = bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("edge: cannot apply the durable migration aggregate: {e}");
        std::process::exit(1);
    }
    // The edge is the only deployable composition root that owns both the Issues subsystem and the
    // Identity service implementation. Apply the complete Issues schema before constructing the
    // restart-safe authorization reconciler; a missing binding table is a boot failure, never a
    // silent in-memory or disabled-worker fallback.
    if let Err(e) = bootstrap
        .migrate(
            &myelin_issues::issues_migrations(),
            &myelin_issues::issues_hot_tables(),
        )
        .await
    {
        eprintln!("edge: cannot apply the Issues authorization-saga migrations: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .verify_index_ready(myelin_issues::ISSUE_RECENT_LIST_INDEX)
        .await
    {
        eprintln!("edge: Issues recent-list index is not ready: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .verify_index_ready(myelin_issues::ISSUE_KEY_PREFIX_LIST_INDEX)
        .await
    {
        eprintln!("edge: Issues key-prefix list index is not ready: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(
            &myelin_git::pg_pr_store::git_pr_migrations(),
            &myelin_git::pg_pr_store::git_pr_hot_tables(),
        )
        .await
    {
        eprintln!("edge: cannot apply the Git PR lifecycle migrations: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap.verify_index_ready("git_pr_head_repo_idx").await {
        eprintln!("edge: Git PR provenance index is not ready: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .verify_index_ready("git_pr_command_operation_scope_uidx")
        .await
    {
        eprintln!("edge: Git PR operation-scope index is not ready: {e}");
        std::process::exit(1);
    }
    // Re-probe the constrained role, close the migration pool, and erase the migration DSN before
    // constructing the KMS, token authority, outbox, identity, or ReBAC runtime stores.
    let provider = match bootstrap.into_runtime().await {
        Ok(provider) => provider,
        Err(e) => {
            eprintln!("edge: database runtime handoff refused to start: {e}");
            std::process::exit(1);
        }
    };
    // The operator-held seal key (the SAME key unseals the KMS root AND the cell token-authority root
    // — one operator secret, one blast radius). Fail-closed at boot: a missing/malformed key exits.
    let seal_key = match seal_key_from_env() {
        Ok(k) => k,
        Err(e) => {
            eprintln!("edge: refused to start (durable-by-default requires the seal key): {e}");
            std::process::exit(1);
        }
    };
    // The cell whose sealed roots this edge serves (a namespace, not a secret — dev default). The KMS
    // root AND the cell token-authority root are both scoped to this cell id.
    let cell_id = std::env::var("MYELIN_CELL_ID").unwrap_or_else(|_| "cell-dev".to_string());
    // The shared cell KMS (crypto-shred substrate) — DURABLE-BY-DEFAULT. A missing/malformed seal
    // key, an unreachable store, or a root that does not unseal (WrongSealKey — fail-closed, NEVER a
    // fresh root that would orphan every ciphertext) each exit non-zero.
    let kms_backing = DurableKmsBacking::new(provider.db_pool().clone(), cell_id.clone());
    let kms = match kms_backing.load_or_generate(&seal_key).await {
        Ok(engine) => Arc::new(engine),
        Err(e) => {
            eprintln!(
                "edge: KMS refused to start (fail-closed, never a silent in-memory engine): {e}"
            );
            std::process::exit(1);
        }
    };
    // R4.0 — the DURABLE cell TOKEN AUTHORITY root (P-527 / MR-025 follow-on): the Ed25519 seed +
    // macaroon MAC key sealed in `cell_token_root` under the SAME seal key. This replaces the ephemeral
    // `CellTokenAuthority::generate()` — a token minted before a restart now verifies after it. A wrong/
    // absent seal key (a sealed root that does not unseal) is fail-closed and NEVER a fresh root (that
    // would orphan every token ever minted under the old root).
    let cell_backing = DurableCellRootBacking::new(provider.db_pool().clone(), cell_id.clone());
    let cell_material = match cell_backing.load_or_generate(&seal_key).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "edge: cell token authority refused to start (fail-closed, never a fresh root that \
                 would orphan every minted token): {e}"
            );
            std::process::exit(1);
        }
    };
    let cell = Arc::new(
        CellTokenAuthority::from_material(&cell_material).unwrap_or_else(|e| {
            eprintln!("edge: durable cell token-authority material is invalid: {e:?}");
            std::process::exit(1);
        }),
    );
    ComposedCore {
        provider,
        kms,
        cell,
        cell_id,
        handle,
    }
}

// =================================================================================================
// argv dispatch — no args = serve; `bootstrap` / `revoke` = operator-plane actions.
// =================================================================================================

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => serve(compose_core().await).await,
        Some("bootstrap") => operator_bootstrap(compose_core().await, &args[1..]).await,
        Some("revoke") => operator_revoke(compose_core().await, &args[1..]).await,
        Some(other) => {
            eprintln!(
                "edge: unknown subcommand `{other}` (expected: <none> = serve | bootstrap | revoke)"
            );
            std::process::exit(2);
        }
    }
}

// =================================================================================================
// serve — the request-lifecycle gateway (unchanged behaviour; now over the DURABLE cell authority).
// =================================================================================================

async fn serve(core: ComposedCore) {
    let ComposedCore {
        provider,
        kms,
        cell,
        cell_id,
        handle,
    } = core;

    // The DURABLE transactional outbox (SI-007) for the git backend's ref-CAS co-commit.
    let git_outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        handle.clone(),
    )));
    // The UNIQUE production id source (P-S12) — never the per-instance MonotonicMinter.
    let git_minter: Arc<dyn myelin_events::IdMinter> = Arc::new(myelin_events::UlidMinter::new());

    // The REAL PASETO Bearer verifier over the DURABLE cell authority (genuine Ed25519 — a forged
    // token is rejected; a token minted before a restart still verifies, R4.0). Arc'd because the
    // R2.1a StoreBackedCheck (the per-run-token minter) shares the SAME cell authority the verifier
    // trusts — one cell, one trust anchor.
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

    // R2.5 — the human/SSO login over the durable S1 directory (OIDC opt-in; refuse-not-mock otherwise).
    let oidc_settings = provider.config().oidc.clone();
    let human_store = PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(provider.clone()),
        handle.clone(),
    );
    // R3.5 / R4.0 — the UNAUTHENTICATED public auth surface (`GET /v1/auth/config`). `dev_login_enabled`
    // (`MYELIN_DEV_LOGIN`) + `token_login_enabled` (`MYELIN_TOKEN_LOGIN`, the R4.0 operator-token web
    // login gate) are env-driven at composition time and project NO secret.
    let dev_login_enabled = std::env::var("MYELIN_DEV_LOGIN")
        .map(|v| v == "1")
        .unwrap_or(false);
    let token_login_enabled = std::env::var("MYELIN_TOKEN_LOGIN")
        .map(|v| v == "1")
        .unwrap_or(false);
    let auth_config = match oidc_settings.as_ref() {
        Some(_) => AuthPublicConfig {
            sso_configured: true,
            providers: vec![AuthProvider {
                id: "oidc".to_string(),
                label: "Single sign-on".to_string(),
            }],
            dev_login_enabled,
            token_login_enabled,
        },
        None => AuthPublicConfig {
            sso_configured: false,
            providers: Vec::new(),
            dev_login_enabled,
            token_login_enabled,
        },
    };
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

    // ── R2.1a — the LIVE per-repo object authz (R0.3) + the git wire endpoints. ──
    let check =
        StoreBackedCheck::with_pg(provider.clone(), kms.clone(), cell.clone(), handle.clone());
    for admit in check.admit_git_fragment() {
        if let FragmentAdmit::Rejected { reason } = admit {
            eprintln!("edge: the Git ReBAC fragment did not admit (authz would deny everything): {reason}");
            std::process::exit(1);
        }
    }
    for admit in check.admit_issue_fragment() {
        if let FragmentAdmit::Rejected { reason } = admit {
            eprintln!(
                "edge: the Issues ReBAC fragment did not admit (issue authz would deny everything): {reason}"
            );
            std::process::exit(1);
        }
    }
    // Construct the real PostgreSQL Issues store over the SAME durable KMS + Identity engine used by
    // the edge. List is safe to expose because PgIssueStore accepts only the frozen effective
    // `issue:view` Filter and joins the durable ready projection in SQL before pagination. The same
    // authorizer is retained by the registration-time object guards, then checked again in-store.
    let issue_authorizer = StoreBackedIssueAuthorizer::new(check.clone());
    let issue_store = Arc::new(myelin_issues::PgIssueStore::new(
        provider.clone(),
        kms.clone(),
        issue_authorizer.clone(),
    ));
    let issue_reconciliation_config =
        IssueReconciliationConfig::from_env(Region::new(provider.config().region.clone()))
            .unwrap_or_else(|error| {
                eprintln!("edge: Issues authorization reconciler refused to start: {error}");
                std::process::exit(1);
            });
    let thresholds = match myelin_substrate::Thresholds::load_canonical() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("edge: cannot load the canonical thresholds file (the fail-static bound for the git-wire authz): {e}");
            std::process::exit(1);
        }
    };
    let revocation_sla_secs = thresholds.revocation.sla_mins * 60;
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
    let repo_bootstrap = Arc::new(TupleRepoBootstrap::new(check.tuples().clone()));

    // The Git subsystem over the DURABLE on-disk backend (GT-003), rooted at `MYELIN_GIT_ROOT`.
    let git_root = std::env::var("MYELIN_GIT_ROOT").unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("myelin-git-data")
            .to_string_lossy()
            .into()
    });
    let git_backend = Arc::new(
        DurableGitBackend::rooted(
            git_root,
            provider.clone(),
            kms.clone(),
            handle.clone(),
            git_outbox,
            git_minter,
        )
        .unwrap_or_else(|error| {
            eprintln!("edge: PostgreSQL Git PR store refused to construct: {error}");
            std::process::exit(1);
        })
        .with_repo_authorizer(repo_authz)
        .with_repo_bootstrap(repo_bootstrap),
    );
    let recovery = recover_placed_git_at_boot(&git_backend, &provider, &cell_id)
        .await
        .unwrap_or_else(|error| {
            eprintln!("edge: durable Git boot recovery failed: {error}");
            std::process::exit(1);
        });
    eprintln!(
        "edge: Git recovery complete (tenants={}, repos={}, refs={}, merges={})",
        recovery.tenants_recovered,
        recovery.repos_reconciled,
        recovery.refs_reapplied,
        recovery.merges_recovered
    );

    // R2.6 — the action-level allowlist gate + the DEFAULT operator-token scheme (R4.0: `agent`).
    let mut builder = Gateway::builder(
        authn,
        human_login,
        Arc::new(AuthenticatedActionPolicy::mounted()),
    )
    .default_token_scheme(EDGE_DEFAULT_TOKEN_SCHEME)
    .with_auth_config(auth_config)
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
    builder = register_git_wire(builder, git_backend);
    builder = register_issues(
        builder,
        issue_store.clone(),
        issue_authorizer,
        handle.clone(),
    );
    let gateway = Arc::new(builder.build());

    let addr = std::env::var("MYELIN_EDGE_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("edge: failed to bind {addr}: {e}");
            std::process::exit(1);
        }
    };
    let issue_reconciler =
        spawn_issue_authorization_reconciler(issue_store, check, issue_reconciliation_config);
    eprintln!("edge: listening on {addr}");
    let server_result = tokio::select! {
        result = serve_edge(listener, gateway) => result,
        signal = tokio::signal::ctrl_c() => signal,
    };
    let reconciliation_result = issue_reconciler.shutdown().await;
    if let Err(e) = reconciliation_result {
        eprintln!("edge: Issues authorization reconciler did not drain cleanly: {e}");
        std::process::exit(1);
    }
    if let Err(e) = server_result {
        eprintln!("edge: serve error: {e}");
        std::process::exit(1);
    }
}

// =================================================================================================
// operator subcommands — `bootstrap` (mint) + `revoke` (S7 denylist). Never an HTTP surface.
// =================================================================================================

async fn operator_bootstrap(core: ComposedCore, args: &[String]) {
    let ComposedCore {
        provider,
        kms,
        cell,
        handle,
        ..
    } = core;

    let tenant = required_flag(args, "--tenant");
    let principal = required_flag(args, "--principal");
    let issues_project = required_flag(args, "--issues-project");
    if !myelin_issues::api::is_canonical_uuid(&issues_project) {
        eprintln!("edge bootstrap: --issues-project must be a canonical lowercase UUID");
        std::process::exit(2);
    }
    let display_name = flag(args, "--display-name");
    let region = flag(args, "--region").unwrap_or_else(default_region);
    let ttl_days: u32 = flag(args, "--ttl-days")
        .map(|v| {
            v.parse().unwrap_or_else(|_| {
                eprintln!("edge bootstrap: --ttl-days must be a non-negative integer, got `{v}`");
                std::process::exit(2);
            })
        })
        .unwrap_or(30);

    // The DURABLE S1 store the serving edge also reads — the seeded principal + credential link
    // persist in PG, so the minted token authenticates against a separately-running serving edge.
    let store = PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(provider.clone()),
        handle.clone(),
    );
    let tuples = TupleStore::with_pg(DurableTupleBacking::new(provider.clone()), handle.clone());
    let now_unix = now_unix();
    let outcome = bootstrap_principal_and_mint(
        &store,
        &tuples,
        &cell,
        &BootstrapParams {
            tenant: &tenant,
            region: &region,
            principal: &principal,
            issues_project: &issues_project,
            display: display_name.as_deref(),
            ttl_days,
        },
        now_unix,
    )
    .unwrap_or_else(|e| {
        eprintln!("edge bootstrap: {e}");
        std::process::exit(1);
    });

    // Print the token to STDOUT EXACTLY ONCE (never a log/file/audit body). The rest of the metadata
    // is the operator's handle to the principal + the revocation id + the expiry.
    println!("{}", outcome.token);
    eprintln!("edge bootstrap: minted an operator capability token");
    eprintln!("  tenant       = {}", outcome.tenant);
    eprintln!("  region       = {}", outcome.region);
    eprintln!("  principal    = {}", outcome.principal_id);
    eprintln!("  issues grant = project:{issues_project}#reader");
    eprintln!("  subject_key  = {}", outcome.subject_key);
    eprintln!("  jti          = {}", outcome.jti);
    eprintln!("  expiry_unix  = {}", outcome.expiry_unix);
    eprintln!(
        "  scheme       = {} (send `Authorization: Bearer <token>` or, on the git wire, HTTP Basic \
         with the token as the password)",
        myelin_edge::BOOTSTRAP_SCHEME
    );
    eprintln!(
        "  revoke with  : edge revoke --jti {} --tenant {}",
        outcome.jti, outcome.tenant
    );
    // Never leave the token in this process' env / anywhere else.
}

async fn operator_revoke(core: ComposedCore, args: &[String]) {
    let ComposedCore {
        provider, handle, ..
    } = core;

    let jti = required_flag(args, "--jti");
    // The S7 denylist is `(tenant, region)`-partitioned, so a revoke MUST name the token's partition.
    let tenant = required_flag(args, "--tenant");
    let region = flag(args, "--region").unwrap_or_else(default_region);

    let revocations = RevocationStore::with_pg(
        DurableRevocationBacking::new(provider.clone()),
        handle.clone(),
    );
    // The verified `(tenant, region)` scope (operator plane — the trust boundary is the DB creds +
    // seal key, stated above). A plain jti revoke denies regardless of the recorded instant.
    let scope = TenantScope::from_verified_token(
        &Principal::stub(
            PrincipalId("revoke-operator".into()),
            PrincipalKind::Human,
            TenantId(tenant.clone()),
        ),
        Region(region.clone()),
    );
    revocations.revoke(&scope, &RevokeTarget::Jti(jti.clone()), now_rfc3339());
    eprintln!("edge revoke: token jti `{jti}` revoked in tenant `{tenant}` region `{region}` (durable S7 denylist — the deny survives restart)");
}

// =================================================================================================
// Tiny hand-rolled argv helpers (no clap — clap is not a workspace dep; the repo idiom for a main is
// std::env::args parsing). Each is total over a malformed argv.
// =================================================================================================

/// The value of `--name <value>` in `args`, or `None`. A flag with no following value is `None`.
fn flag(args: &[String], name: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().cloned();
        }
        // Also accept `--name=value`.
        if let Some(v) = a.strip_prefix(&format!("{name}=")) {
            return Some(v.to_string());
        }
    }
    None
}

/// A required `--name <value>`; a missing one is a loud usage error (exit 2).
fn required_flag(args: &[String], name: &str) -> String {
    flag(args, name).unwrap_or_else(|| {
        eprintln!("edge: missing required flag `{name}`");
        std::process::exit(2);
    })
}

/// The default residency region — `MYELIN_REGION` (the dev-stack contract) or `fr-par`.
fn default_region() -> String {
    std::env::var("MYELIN_REGION").unwrap_or_else(|_| "fr-par".to_string())
}

/// The current Unix-seconds instant (for the token `exp`).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The current instant as an RFC-3339 [`Timestamp`] (the revocation record's `now`).
fn now_rfc3339() -> Timestamp {
    let dt = chrono::DateTime::from_timestamp(now_unix(), 0).unwrap_or_default();
    Timestamp(dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}
