use myelin_lints::production_graph::{
    no_bare_tenant_pool, no_in_memory_durable_store, no_permissive_authorizer_in_prod,
    no_structural_crypto_in_prod, production_graph_absence_scanners,
    PRODUCTION_GRAPH_ABSENCE_SCANNERS,
};
use myelin_lints::{Lint, LintId};
use std::path::{Path, PathBuf};

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn read_fixture(name: &str) -> String {
    let path = format!("{FIXTURES_DIR}/{name}");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"))
}

struct Row {
    lint: fn() -> Lint,
    id: LintId,
    red: &'static str,
    green: &'static str,
}

fn matrix() -> Vec<Row> {
    vec![
        Row {
            lint: no_structural_crypto_in_prod,
            id: LintId("no-structural-crypto-in-prod"),
            red: "no_structural_crypto_in_prod.red.rs.txt",
            green: "no_structural_crypto_in_prod.green.rs.txt",
        },
        Row {
            lint: no_in_memory_durable_store,
            id: LintId("no-in-memory-durable-store"),
            red: "no_in_memory_durable_store.red.rs.txt",
            green: "no_in_memory_durable_store.green.rs.txt",
        },
        Row {
            lint: no_bare_tenant_pool,
            id: LintId("no-bare-tenant-pool"),
            red: "no_bare_tenant_pool.red.rs.txt",
            green: "no_bare_tenant_pool.green.rs.txt",
        },
        Row {
            lint: no_permissive_authorizer_in_prod,
            id: LintId("no-permissive-authorizer-in-prod"),
            red: "no_permissive_authorizer_in_prod.red.rs.txt",
            green: "no_permissive_authorizer_in_prod.green.rs.txt",
        },
    ]
}

#[test]
fn the_matrix_covers_exactly_the_four_scanners() {
    let rows = matrix();
    assert_eq!(
        rows.len(),
        4,
        "the matrix must cover all four absence scanners (the R2.6 permissive-authorizer \
         scanner appended to the original three)"
    );
    let ids: Vec<LintId> = rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, PRODUCTION_GRAPH_ABSENCE_SCANNERS.to_vec());
}

#[test]
fn every_red_fixture_bites() {
    for row in matrix() {
        let lint = (row.lint)();
        let v = lint.run(&read_fixture(row.red));
        assert!(
            !v.is_empty(),
            "scanner `{}` MUST reject its red fixture `{}`, but found 0 violations",
            row.id,
            row.red
        );
        assert!(
            v.iter().all(|x| x.lint == row.id),
            "scanner `{}` red-fixture violations must all carry its own id",
            row.id
        );
    }
}

#[test]
fn every_green_fixture_is_admitted() {
    for row in matrix() {
        let lint = (row.lint)();
        let v = lint.run(&read_fixture(row.green));
        assert!(
            v.is_empty(),
            "scanner `{}` MUST admit its green fixture `{}`, but found: {:?}",
            row.id,
            row.green,
            v
        );
    }
}

#[test]
fn each_red_fixture_trips_exactly_its_own_scanner() {
    let all = production_graph_absence_scanners();
    for row in matrix() {
        let red = read_fixture(row.red);
        let firing: Vec<LintId> = all
            .iter()
            .filter(|l| !l.run(&red).is_empty())
            .map(|l| l.id)
            .collect();
        assert_eq!(
            firing,
            vec![row.id],
            "red fixture `{}` must trip exactly `{}`, but tripped: {:?}",
            row.red,
            row.id,
            firing
        );
    }
}

#[test]
fn test_support_gate_admits_the_double_but_the_ungated_twin_still_bites() {
    let green = read_fixture("no_in_memory_durable_store.test_support.green.rs.txt");
    let admitted = no_in_memory_durable_store().run(&green);
    assert!(
        admitted.is_empty(),
        "a `test-support`-gated in-memory store/backing is a TEST DOUBLE and must be ADMITTED, \
         but found: {admitted:?}"
    );

    let red = read_fixture("no_in_memory_durable_store.ungated.red.rs.txt");
    let bites = no_in_memory_durable_store().run(&red);
    assert!(
        !bites.is_empty(),
        "the UN-gated twin of the test-support fixture MUST still fire - Wave 0 must admit ONLY the \
         `test-support` gate, never a real prod in-memory store"
    );
    assert!(
        bites
            .iter()
            .all(|v| v.lint == LintId("no-in-memory-durable-store")),
        "the un-gated fixture must fire ONLY the in-memory-durable-store scanner"
    );
}

struct Gate {
    lint: fn() -> Lint,
    scope: &'static [&'static str],
}

const SPINE: &[&str] = &[
    "crates/myelin-identity-service/",
    "crates/myelin-events/",
    "crates/myelin-control-plane/",
    "crates/myelin-storage/",
];

fn gates() -> Vec<Gate> {
    vec![
        Gate {
            lint: no_structural_crypto_in_prod,
            scope: &["crates/"],
        },
        Gate {
            lint: no_in_memory_durable_store,
            scope: SPINE,
        },
        Gate {
            lint: no_bare_tenant_pool,
            scope: &["crates/myelin-storage/"],
        },
        Gate {
            lint: no_permissive_authorizer_in_prod,
            scope: &["crates/myelin-edge/"],
        },
    ]
}

const BASELINE: &[(&str, &str, usize)] = &[
    (
        "no-structural-crypto-in-prod",
        "crates/myelin-ci-controlplane/src/residency_drill.rs",
        444,
    ),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest dir")
        .to_path_buf()
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn all_src(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let crates = root.join("crates");
    for c in std::fs::read_dir(&crates)
        .expect("crates/ must exist")
        .flatten()
    {
        let src = c.path().join("src");
        if src.is_dir() {
            collect_rs(&src, &mut out);
        }
    }
    out.sort();
    out
}

fn live_violations(root: &Path) -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    let mut scanned = 0usize;
    for gate in gates() {
        let lint = (gate.lint)();
        for file in all_src(root) {
            let rel = file
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if rel.contains("myelin-lints/")
                || rel.contains("/tests/")
                || rel.contains("/fixtures/")
            {
                continue;
            }
            if !gate.scope.iter().any(|c| rel.contains(c)) {
                continue;
            }
            let src = std::fs::read_to_string(&file).expect("readable source file");
            for v in lint.run(&src) {
                out.push((v.lint.0.to_string(), rel.clone(), v.line));
            }
            scanned += 1;
        }
    }
    assert!(
        scanned >= 8,
        "expected to scan the spine src tree, scanned {scanned}"
    );
    out.sort();
    out.dedup();
    out
}

#[test]
fn the_baseline_is_non_empty_and_internally_consistent() {
    assert_eq!(
        BASELINE.len(),
        1,
        "the committed baseline has 1 entry - the CI runner-attestation structural floor only. \
         The R2.6(+followup) permissive-authorizer set is a TRUE ZERO: the ACTION-level `AllowAll` \
         is gone from main.rs (wires `AuthenticatedActionPolicy`) AND the OBJECT-seam \
         `DurableGitBackend::rooted` default now fail-closes with `DenyAllRepos` (the only \
         `AllowAllRepos` construction moved into the test-support `rooted_inmem_for_test`). \
         The `no-in-memory-durable-store` set is EMPTY (R1 EXIT). (MR-009b Wave 2 flipped the 3 identity spine stores \
         GREEN: 17 → 14; Wave 3 flipped the events `DedupLedger` (SI-023) → 14 → 13; Wave 5 flipped \
         the `KmsEngine` (SI-006) → 13 → 12; Wave 6a flipped the 2 identity S2 pseudonym holders → \
         12 → 10; Wave 6b flipped the 2 in-crate storage ERASURE ledgers - `restore_verify.rs:175` \
         `ErasureLedger` + `reerase.rs:156` `InMemoryPostPitLedger` - durable-by-default → 10 → 8; \
         W6c-events flipped the events `BusErasureLedger` (SI-039) durable-by-default via the \
         DedupLedger trait-seam pattern → 8 → 7; W6c-cp flipped the control-plane `CellResolverRegistry` \
         (SI-052) durable-by-default via a boot-time projection of the durable `cell.endpoint` → 7 → 6; \
         W6d flipped the control-plane `Registry` (SI-011) + `MisrouteAudit` (SI-028) \
         durable-by-default - the whole-surface backend enum over the extended placement backing \
         (repo_placement/cell_provisioning/local_tenant, 0035–0039) + the durable audit sink wired \
         at the gateway → 6 → 4; W6b2 flipped the sibling `reserve_settle.rs:283` `CostLedger` \
         (SI-021) durable-by-default via the `CostBackend` role-struct + the \
         `myelin-flow::BudgetGate::with_pg` injection - closing the W6b honest-STOP → 4 → 3; and \
         W3b.6 flipped the events `outbox.rs:231` `OutboxStore` (SI-007) durable-by-default - THE \
         FLIP closing the W3b.1–.6 chain (`Memory` arm/`Inner`/`new`/`Default`/memory relay \
         mechanics `test-support`-gated; the always-compiled arm is `Durable(Arc<dyn \
         DurableOutboxBacking>)` over `PgOutboxBacking`) → 3 → 2)"
    );
    let structural = BASELINE
        .iter()
        .filter(|(s, ..)| *s == "no-structural-crypto-in-prod")
        .count();
    let in_memory = BASELINE
        .iter()
        .filter(|(s, ..)| *s == "no-in-memory-durable-store")
        .count();
    let bare_pool = BASELINE
        .iter()
        .filter(|(s, ..)| *s == "no-bare-tenant-pool")
        .count();
    let permissive = BASELINE
        .iter()
        .filter(|(s, ..)| *s == "no-permissive-authorizer-in-prod")
        .count();
    assert_eq!(
        structural, 1,
        "1 structural-crypto site remains: MR-012 flipped the 4 identity sites GREEN (HumanSso/\
         Capability authenticators + the two RunTokenMinter constructors no longer build Structural*; \
         production injects the real PASETO signer / refuse-not-mock dispatch). The surviving entry is \
         the CI runner-attestation floor (residency_drill.rs, P-527) - a different subsystem, deferred \
         to the CI track."
    );
    assert_eq!(
        in_memory, 0,
        "0 in-memory durable-store sites - R1 EXIT, the ratchet is fully green over the spine. \
         (History: Waves 2/3/5/6a flipped the identity spine + events \
         DedupLedger + KmsEngine + the 2 S2 pseudonym holders durable-by-default (16→9); \
         MR-009b Wave 6b FLIPPED the 2 in-crate storage ERASURE ledgers durable-by-default (9→7): \
         `restore_verify.rs:175` `ErasureLedger` (now a backend enum over the non-shred-erasable \
         `restore_erasure_ledger` table, migration 0051, carrying the R1 §7.6 completion-offset \
         fold-in) + `reerase.rs:156` `InMemoryPostPitLedger` (now the `test-support`-gated double \
         behind `DurablePostPitLedger` over `post_pit_erasure_ledger`, 0052); W6c-events FLIPPED \
         the events `BusErasureLedger` (SI-039) durable-by-default (7→6) via the DedupLedger \
         trait-seam pattern (`DurableBusErasure` trait in events; `DurableBusErasureBacking` over the \
         non-shred-erasable, NO-RLS `bus_erasure_ledger` table, migration 0053, in storage; wired at \
         `events_serve::EventsRuntime`); W6c-cp FLIPPED the control-plane `CellResolverRegistry` \
         (SI-052) durable-by-default (7→6) via a boot-time PROJECTION of the durable `cell.endpoint` \
         (the `Projected(Arc<dyn ResolverProjection>)` production arm; the `Memory(HashMap<…>)` arm + \
         `::new()`/`register()` are `test-support`-gated doubles; projection in \
         `cross_cell_bridge_durable`, fail-loud on a missing/unresolvable endpoint); W6d FLIPPED \
         the control-plane `Registry` (SI-011) + `MisrouteAudit` (SI-028) durable-by-default: \
         the whole-surface `RegistryBackend` enum over the EXTENDED placement backing (repo_placement \
         + its residency-pin DB trigger + tenant-delete-RESTRICT FK, cell_provisioning, local_tenant \
         - migrations 0035–0039) and the `MisrouteAuditBackend` enum over the MR-024 durable sink, \
         both `test-support`-gating their Memory arms + in-memory ctors (`Registry::new`, \
         `MisrouteAudit::new`, `CellGateway::new`), the self-host boot re-pointed via `with_pg`; and \
         W6b2 FLIPPED the sibling storage `reserve_settle.rs:283` `CostLedger` (SI-021) \
         durable-by-default via the `CostBackend` role struct (the `Memory(MemoryCostLedger)` arm + \
         `CostLedger::new()` are `test-support`-gated; the always-compiled `Durable(DurableCostLedger)` \
         arm over the FORCE-RLS `cost_reservation`/`cost_event` tables, 0050, is production) + the \
         `myelin-flow::BudgetGate::with_pg`/`new_durable` injection that closed the W6b honest-STOP; \
         W3b.6 FLIPPED the events `outbox.rs:231` `OutboxStore` (SI-007) durable-by-default \
         - THE FLIP closing the W3b.1–.6 chain: the `Memory(Arc<Mutex<Inner>>)` arm + `Inner` \
         + `::new()`/`Default` + the memory-arm relay mechanics + `restore_committed_row_for_test` \
         are `test-support`-gated TEST DOUBLES, and the always-compiled PRODUCTION backend is \
         `Durable(Arc<dyn DurableOutboxBacking>)` over `PgOutboxBacking` (proven live: \
         `integration_mr009b_outbox_durable` + `integration_w3b4_durable_spec_boot` + the kill-9 \
         emit drill, 0 lost / 0 ghost); and W7.3 FLIPPED storage `blob.rs:362` `FsBlobStore` \
         (SI-014/015/029) - the fs floor is a `test-support`-gated test double, the durable backing \
         is the always-compiled `s3blob::S3BlobStore`, the knowledge + chat production defaults \
         re-pointed to injection - completing the ledger-13 2→0.)"
    );
    assert_eq!(
        bare_pool, 0,
        "0 bare-tenant-pool sites: MR-013 (P-531) closed both legs of census SI-005 - pg.rs's \
         session-scoped `set_config(<tenant GUC>, .., false)` is now TRANSACTION-scoped via the \
         MR-022 `with_tenant_tx` convention (no cross-checkout bleed), and the bare \
         `PgStore::pool() -> &PgPool` hatch is REMOVED (replaced by `health_check()`). The scanner \
         is GREEN over the production tree."
    );
    assert_eq!(
        permissive, 0,
        "0 permissive-authorizer sites - the scanner is a TRUE ZERO over the edge production graph. \
         R2.6 removed the ACTION-level `Arc::new(AllowAll)` from the edge composition root (main.rs \
         wires the explicit `AuthenticatedActionPolicy` mounted-action allowlist; `AllowAll` is a \
         test-support-gated double), and the R2.6-followup fail-closed the OBJECT-seam \
         `DurableGitBackend::rooted` default (`Arc::new(DenyAllRepos)`); the only surviving \
         `Arc::new(AllowAllRepos)` moved into the `#[cfg(any(test, feature = \"test-support\"))]` \
         `rooted_inmem_for_test` helper (admitted by cfg-region detection). A NEW `Arc::new(AllowAll…)` \
         in any edge src/ file fails this gate loudly."
    );
    let has = |s: &str, p: &str, l: usize| BASELINE.contains(&(s, p, l));
    assert!(
        has(
            "no-structural-crypto-in-prod",
            "crates/myelin-ci-controlplane/src/residency_drill.rs",
            444
        ),
        "anchor residency_drill.rs:444 (the surviving CI attestation floor) must be in the baseline"
    );
    assert!(
        !has(
            "no-structural-crypto-in-prod",
            "crates/myelin-identity-service/src/authenticate.rs",
            289
        ),
        "authenticate.rs:289 was flipped GREEN by MR-012 and must NOT remain in the baseline"
    );
    assert!(
        !has(
            "no-bare-tenant-pool",
            "crates/myelin-storage/src/pg.rs",
            413
        ) && !has(
            "no-bare-tenant-pool",
            "crates/myelin-storage/src/pg.rs",
            150
        ),
        "the former anchors pg.rs:413 (session-scoped set_config) + pg.rs:150 (bare pool() hatch) \
         were flipped GREEN by MR-013 and must NOT remain in the baseline (the ratchet tightened)"
    );
    assert!(
        !BASELINE.iter().any(|(s, p, _)| *s == "no-permissive-authorizer-in-prod"
            && *p == "crates/myelin-edge/src/git_durable.rs"),
        "git_durable.rs's `AllowAllRepos` builder default was fail-closed to `DenyAllRepos` \
         (R2.6-followup) - it must NOT be in the baseline; the permissive-authorizer scanner is a \
         true zero. A re-added Arc::new(AllowAllRepos) in a src/ file fails the ratchet, not the \
         manifest."
    );
    assert!(
        !BASELINE.iter().any(|(s, p, _)| *s == "no-permissive-authorizer-in-prod"
            && *p == "crates/myelin-edge/src/main.rs"),
        "main.rs was flipped GREEN by R2.6 (AuthenticatedActionPolicy replaced AllowAll) and must \
         NOT appear in the baseline - a re-added Arc::new(AllowAll) there fails the ratchet, not \
         the manifest"
    );
}

#[test]
fn the_production_graph_absence_ratchet_equals_the_committed_baseline() {
    let root = workspace_root();
    let live = live_violations(&root);
    let mut baseline: Vec<(String, String, usize)> = BASELINE
        .iter()
        .map(|(s, p, l)| (s.to_string(), p.to_string(), *l))
        .collect();
    baseline.sort();

    let new: Vec<&(String, String, usize)> =
        live.iter().filter(|v| !baseline.contains(v)).collect();
    assert!(
        new.is_empty(),
        "NEW production-graph shortcut(s) introduced (loud, never swallowed - fix the code, do NOT \
         add to the baseline; the ratchet can only tighten toward zero):\n{}",
        new.iter()
            .map(|(s, p, l)| format!("  [{s}] {p}:{l}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let fixed: Vec<&(String, String, usize)> =
        baseline.iter().filter(|v| !live.contains(v)).collect();
    assert!(
        fixed.is_empty(),
        "baseline shortcut(s) FIXED or MOVED but still in the manifest (the ratchet tightens - \
         delete the removed entry, or re-pin a moved line):\n{}",
        fixed
            .iter()
            .map(|(s, p, l)| format!("  [{s}] {p}:{l}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    assert_eq!(
        live, baseline,
        "the live production-graph absence violation set must EQUAL the committed baseline"
    );
}
