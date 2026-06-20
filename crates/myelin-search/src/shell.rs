//! The **Search service shell** — boot from `serve(AppSpec)` (SRCH-P03 / P-166; contracts 1.1
//! `serve(AppSpec)`, 1.2/1.3 the three ports + liveness ≠ readiness, 1.5 the forward-only
//! migration, all consumed — Search owns no contract crate here).
//!
//! **Owning architecture doc:** `search-and-indexing.md`
//! - §2.1 (`engine.search` is private; the only public entry composes the ACL filter first — the
//!   search-requires-acl-filter ratchet, SRCH-P01); §3.4 (the per-tenant residency-pinned index
//!   layout the migration creates).
//! - `contract-index.md` rows 1.1 (`serve(AppSpec)` — the service shell; a service is an `AppSpec`
//!   with handlers, **not a hand-rolled `main`**), 1.2 / 1.3 (the three ports; liveness ≠
//!   readiness), and 1.5 (the forward-only online migration that creates the index directory).
//!
//! ## What SRCH-P03 ships here — the bootable SHELL, NOT a working engine
//! [`search_app_spec`] assembles the Search [`AppSpec`] the harness's ONE call drives (boot →
//! migrate → outbox relay → consumers → three ports → graceful drain, liveness ≠ readiness). It is
//! the EXACT analog of `notif_app_spec` / the Refs service shell — Search is an `AppSpec`, not a
//! hand-rolled lifecycle. The shell:
//!   - declares the **three ports** (public / internal / metrics-health) via the harness (1.2/1.3);
//!   - runs the **forward-only migration** that creates the per-tenant index directory (1.5,
//!     [`SEARCH_INDEX_DIR_MIGRATION`]) — the encrypted-from-birth per-tenant index layout
//!     ([`crate::layout::PerTenantIndexLayout`]);
//!   - declares the per-tenant **search index store** in the [`StoreManifest`]
//!     ([`myelin_substrate::StoreKind::SearchIndex`]) so it auto-registers as the H7 holder
//!     (the SRCH-P02 holder is now opened through the harness's one door — reconciled, not
//!     duplicated);
//!   - declares its critical downstreams (`identity` — the permission-aware query's `check`; the
//!     OLTP store is implicitly critical) for the readiness probe (§4.3, SUB-D9).
//!
//! **Floor named (the engine-shapes follow-on, [`crate::layout::srch_p03_floors`]):** the
//! `IndexBackend` trait + the three index shapes (SRCH-P04/P05), the indexer (SRCH-P06), and the
//! query path (SRCH-P08) are the follow-ons that make this answer anything. This prompt ships the
//! shell + the encrypted layout only — there is **no `IndexBackend`, no Tantivy, no vector HNSW, no
//! indexer, no query path** here. `engine.search` stays private by construction (the SRCH-P01 lint
//! holds — there is no public search path in this crate to add a bypass to).

use myelin_substrate::{
    boot, serve, AppSpec, Config, CriticalDependencies, DeclaredStore, HotTables, InternalRpc,
    Migration, Migrations, OutboxSpec, PublicRoutes, ServeError, ServeHandle, StoreKind,
    StoreManifest,
};

use crate::holder::SEARCH_INDEX_STORE;

/// The deployable service name (the `AppSpec::name` + the telemetry/trace service identifier). The
/// `search` binary (`src/main.rs`) and the `AppSpec` both read this.
pub const SERVICE_NAME: &str = "search";

/// **The forward-only migration that creates the per-tenant index directory (contract 1.5; §3.4).**
/// The per-tenant index *directory* lives in the tenant's cell, `(tenant, region)`-keyed,
/// residency-pinned, and is **encrypted-from-birth** under the per-tenant index DEK
/// ([`crate::layout::PerTenantIndexLayout`]). This is a CREATE (additive, forward-only — never a
/// `DROP`); the migration runner admits it and a destructive variant would be rejected at boot
/// (§9.1). The concrete on-disk directory + the Tantivy/vector segment FORMAT is SRCH-P04 (the
/// `IndexBackend`); here the migration declares the directory's existence so the shell boots with
/// the encrypted layout in place.
///
/// The DDL is the directory catalog row (the per-tenant index directory's metadata: its
/// `(tenant, region)` key + its `pii_key_ref`), NOT a hot table (a directory-create is one-time per
/// tenant, not a write-QPS table) — so it is a `Plain` forward migration.
pub const SEARCH_INDEX_DIR_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS search_index_directory (
    tenant         TEXT NOT NULL,
    region         TEXT NOT NULL,
    index_dek_ref  TEXT NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, region)
);";

/// The Search forward-only migration set (contract 1.5; §3.4 / §9). One forward-only migration: the
/// per-tenant index directory create. The substrate co-located `outbox` + `consumer_dedup` tables
/// are prepended by the harness; the indexer's dedup ledger (S3), the reindex cursor (S4), and the
/// filter cache (S5) tables are the later slices' migrations (SRCH-P06/P16/P13 — named floors, not
/// shipped here).
fn search_migrations() -> Migrations {
    Migrations::of([Migration::plain(
        "0010_search_index_directory",
        SEARCH_INDEX_DIR_MIGRATION,
    )])
}

/// The store manifest the harness auto-registers as `PersonalDataHolder`s (§3.4, GD-3). Beyond the
/// implicit OLTP store (the harness adds it), Search declares its ONE derived store: the per-tenant
/// **search index** ([`StoreKind::SearchIndex`]). Declaring it here means it is opened — and
/// therefore registered as the H7 holder — through the harness's one door (reconciling the SRCH-P02
/// holder registration into the boot path, not duplicating it).
fn search_stores() -> StoreManifest {
    StoreManifest::of([DeclaredStore::new(StoreKind::SearchIndex, SEARCH_INDEX_STORE)])
}

/// The critical-dependency set the metrics-health readiness probe reads (§4.3, SUB-D9). The OLTP
/// store is implicitly critical (the harness adds it). Search declares `identity` — the
/// permission-aware query path (SRCH-P08) composes the `list_objects`/`check` ACL filter through
/// Identity; a dead Identity means Search cannot serve correct (permission-filtered) results, so it
/// reports not-ready + sheds rather than serving unfiltered. (Search degrades to bounded-staleness
/// on a hiccup, never fail-open — it inherits ID-D2, named in [`crate::dek::srch_p03_inherited_gates`].)
fn search_critical() -> CriticalDependencies {
    CriticalDependencies::new(["identity"])
}

/// **Assemble the Search service [`AppSpec`] (contract 1.1; the service shell).** The harness owns
/// the lifecycle around it (boot → migrate → relay → consumers → three ports → graceful drain,
/// liveness ≠ readiness). Search is an `AppSpec` + handlers, NOT a hand-rolled `main`.
///
/// `config` is the validated, env-first config (§3.2; `Config::from_env()` lands with the driver,
/// P-S15 — the shell boots over the validated default today). The forward-only migration creates
/// the per-tenant index directory; the per-tenant search-index store is declared (auto-registered
/// as the H7 holder); `identity` is declared critical. No consumers are registered here — the
/// indexer (the `evt.*` consumer) is SRCH-P06 (named floor); the shell carries no query path
/// (engine.search stays private, the SRCH-P01 lint holds).
pub fn search_app_spec(config: Config) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: search_migrations(),
        // No declared-hot table at the shell: the per-tenant index directory create is one-time per
        // tenant, not a write-QPS table. The high-write index-mutation tables (the indexer's, S1–S3)
        // declare their hot set as they land (SRCH-P06+ — measured-not-predicted, §9.4).
        hot_tables: HotTables::none(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        // No consumers at the shell — the near-real-time indexer (the evt.* consumer) is SRCH-P06.
        consumers: Vec::new(),
        holders: AppSpec::auto(),
        stores: search_stores(),
        outbox: OutboxSpec::default(),
        critical: search_critical(),
    }
}

/// **Boot the Search service to the pre-serve [`ServeHandle`]** (the harness's
/// [`boot`](myelin_substrate::boot) of [`search_app_spec`]). Separated from [`run_search`] so a
/// test/drill can boot, assert the three ports opened + the migration ran + the index store
/// registered, drive ticks, and drive the drain deterministically.
pub fn boot_search(config: Config) -> Result<ServeHandle, ServeError> {
    boot(search_app_spec(config))
}

/// **The Search service entry — the one `serve(AppSpec)` call (contract 1.1).** The `search` binary
/// (`src/main.rs`) does nothing but hand [`search_app_spec`] to this. A failed boot / incomplete
/// drain returns non-zero (§3.1) — loud, never a silent success.
pub fn run_search(config: Config) -> Result<(), ServeError> {
    serve(search_app_spec(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{HolderRegistration, Liveness, Surface};

    /// **THE Search shell boot test (contract 1.1/1.2/1.3): boots from `serve(AppSpec)` with three
    /// ports + liveness ≠ readiness; the forward-only migration creates the per-tenant index
    /// directory; the per-tenant search-index store auto-registers as a holder.** This is the
    /// prompt's GATE: the service compiles + boots from `serve(AppSpec)` with three ports and
    /// liveness ≠ readiness, and a forward-only migration creates a per-tenant index directory.
    #[test]
    fn search_boots_from_serve_appspec_with_three_ports() {
        let handle = boot_search(Config::default()).expect("the Search shell boots from serve(AppSpec)");
        assert_eq!(handle.name(), SERVICE_NAME, "the deployable service name");

        // (1.2) the three ports opened in the lifecycle (public / internal / metrics-health).
        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports opened (contract 1.2)"
        );

        // (1.3) liveness ≠ readiness: after a successful boot the startup gate is Complete, so
        // readiness is governed by the critical-dependency health (not the same signal as liveness).
        let mh = handle.metrics_health();
        assert_eq!(mh.liveness(), Liveness::Up, "liveness = not-wedged (never checks a dependency)");
        assert!(
            mh.readiness().is_ready(),
            "readiness = can-serve-now (all critical deps healthy at boot) — distinct from liveness"
        );

        // (1.5) the per-tenant search-index store auto-registered as the H7 holder through the
        // harness's one door (the SRCH-P02 holder, reconciled into the boot path, not duplicated).
        assert!(
            handle.holder_registry().is_registered(StoreKind::SearchIndex, SEARCH_INDEX_STORE),
            "the per-tenant search index auto-registered as a holder (§3.4, GD-3)"
        );
        assert!(
            handle.registered_holders().contains(&HolderRegistration {
                kind: StoreKind::SearchIndex,
                name: SEARCH_INDEX_STORE,
            }),
            "the search-index holder registration receipt is present"
        );
        // No store escaped registration (opening IS registering — the holder list cannot drift
        // below the data map).
        assert!(handle.holder_registered().is_ok(), "every declared store registered");
    }

    /// **A dead critical dependency (`identity`) flips readiness to not-ready WITHOUT flipping
    /// liveness (liveness ≠ readiness, contract 1.3 / SUB-D9).** Search cannot serve correct
    /// permission-filtered results without Identity, so it reports not-ready + sheds — but it stays
    /// live (no restart storm). This proves the two signals are distinct, not aliased.
    #[test]
    fn dead_identity_flips_readiness_not_liveness() {
        let handle = boot_search(Config::default()).expect("boot");
        let mh = handle.metrics_health();
        assert!(mh.readiness().is_ready(), "ready while identity is healthy");

        // Mark the declared-critical `identity` dependency down.
        handle.health_probe().mark_down("identity");

        assert!(!mh.readiness().is_ready(), "a dead critical dep → not-ready + shed");
        assert_eq!(
            mh.liveness(),
            Liveness::Up,
            "liveness stays UP (not-ready is NOT not-alive — no restart storm)"
        );
    }

    /// **The Search shell runs the whole lifecycle end-to-end and drains cleanly (contract 1.1).**
    /// `run_search` boots → migrates (creates the per-tenant index directory) → … → graceful-drains
    /// (outbox_depth == 0) → returns Ok. The CDC consumer side of 1.1 (a service `main` that just
    /// calls the one entry).
    #[test]
    fn run_search_runs_lifecycle_and_returns_ok() {
        assert_eq!(run_search(Config::default()), Ok(()), "the Search shell boots → … → drains cleanly");
    }

    /// **A failed boot returns non-zero (§3.1).** A config that fails boot-time validation aborts
    /// boot loudly — the shell never starts half-booted.
    #[test]
    fn failed_boot_returns_non_zero() {
        let r = run_search(Config("BAD_POOL".into()));
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
        assert!(r.unwrap_err().0.contains("fail-fast"), "the error names the §3.2 fail-fast validation");
    }

    /// **The Search forward-only migration is additive (a CREATE), not destructive (§9.1).** The
    /// per-tenant index directory migration is a forward-only CREATE — the runner admits it; a
    /// destructive DROP would be rejected at boot. Asserts the migration text is non-destructive so
    /// the directory-create cannot silently become a data-losing migration.
    #[test]
    fn the_index_directory_migration_is_forward_only() {
        assert!(
            !myelin_substrate::is_destructive(SEARCH_INDEX_DIR_MIGRATION),
            "the per-tenant index directory migration is forward-only (a CREATE, never a DROP)"
        );
        assert!(
            SEARCH_INDEX_DIR_MIGRATION.contains("search_index_directory"),
            "the migration creates the per-tenant index directory catalog"
        );
        // And boot actually applies it (it is in the spec's migration set).
        let spec = search_app_spec(Config::default());
        assert!(
            spec.migrations.0.iter().any(|m| m.id == "0010_search_index_directory"),
            "the index-directory migration is in the Search AppSpec's forward-only set"
        );
    }

    /// **The shell declares the per-tenant search-index store + `identity` critical, and NO
    /// consumers / NO query path (the engine-shapes floor).** Pins the shell's surface so a later
    /// edit that smuggles in a consumer/query path without the lint, or drops the index-store
    /// declaration, is loud.
    #[test]
    fn the_shell_declares_the_index_store_and_no_engine() {
        let spec = search_app_spec(Config::default());
        assert!(
            spec.stores.stores().iter().any(|s| s.kind == StoreKind::SearchIndex),
            "the per-tenant search index store is declared (auto-registered as H7)"
        );
        assert!(spec.consumers.is_empty(), "no indexer consumer at the shell (SRCH-P06 floor)");
        // The engine-shapes floor is named (the follow-on slices).
        assert_eq!(crate::layout::srch_p03_floors().len(), 5, "the engine-shapes floor is named");
    }
}
