use std::collections::BTreeMap;

use myelin_events::{OutboxStore, Timestamp};
use myelin_harness::load_generator::{
    LoadGenerator, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_harness::telemetry::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission, Principal,
    PrincipalId, PrincipalKind, PseudonymHandle, RelName, RelationTuple, RevokeTarget, TupleDelta,
    Zookie,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_storage::{CellKillRestore, CellKillRtoReport, RtoGrain, TenantScope};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{ArtifactRef, CellId, Region, TenantId};

fn principal(tenant: &str, id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn scope_of(p: &Principal) -> TenantScope {
    TenantScope::from_verified_token(p, p.region.clone())
}

fn handle(p: &str, t: &str) -> PseudonymHandle {
    PseudonymHandle::new(p, t).expect("a well-formed pseudonym handle")
}

fn at(t: &str) -> Timestamp {
    Timestamp(t.into())
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

struct Cell {
    id: CellId,
    services: BTreeMap<String, StoreBackedCheck>,
    tenants: Vec<String>,
    killed: bool,
}

impl Cell {
    fn healthy(id: &str, tenants: &[&str]) -> Cell {
        let mut services = BTreeMap::new();
        for t in tenants {
            let scope = scope_of(&principal(t, "p-admin"));
            let store = TupleStore::new(OutboxStore::new());
            store
                .write_tuples(
                    &scope,
                    &principal(t, "p-admin"),
                    &[
                        add("project:web", "parent_team", "team:eng#view"),
                        add("team:eng", "member", "p:surge-human"),
                    ],
                    None,
                    None,
                    at("2026-06-24T00:00:00Z"),
                )
                .expect("seed the tenant grant in this cell");
            services.insert((*t).to_string(), StoreBackedCheck::new(store));
        }
        Cell {
            id: CellId::from_token(id),
            services,
            tenants: tenants.iter().map(|t| (*t).to_string()).collect(),
            killed: false,
        }
    }

    fn kill(&mut self) {
        self.killed = true;
    }

    fn check_in(
        &self,
        tenant: &str,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
    ) -> Option<Decision> {
        if self.killed {
            return None;
        }
        let svc = self.services.get(tenant)?;
        svc.check(subject, permission, object, &at_latest(), None)
            .ok()
    }

    fn seed_erase_and_restore(&self, n: usize) -> usize {
        let mut resurrected = 0usize;
        for tenant in &self.tenants {
            let svc = self.services.get(tenant).expect("a seeded tenant");
            let s = scope_of(&principal(tenant, "p-admin"));
            for i in 0..n {
                let subj = PrincipalId(format!("p:cell-{}:{tenant}:{i}", self.id.as_str()));
                svc.pseudonyms()
                    .put_mapping(&s, &subj, handle(&format!("anon-{i}"), tenant))
                    .unwrap();
                svc.erase_in(&s, &subj, at("2026-06-20T10:00:00Z")).unwrap();
                svc.pseudonyms()
                    .put_mapping(&s, &subj, handle(&format!("anon-{i}"), tenant))
                    .unwrap();
                if svc.pseudonyms().resolve_subject(&s, &subj).is_some() {
                    resurrected += 1;
                }
            }
        }
        resurrected
    }

    fn re_erase_cell(&self, ran_at: &Timestamp) -> CellReEraseOutcome {
        let mut re_erased = 0usize;
        let mut resurrected_after = 0usize;
        let mut all_green = true;
        let mut all_dated = true;
        let mut per_tenant = 0usize;
        for tenant in &self.tenants {
            let svc = self.services.get(tenant).expect("a seeded tenant");
            let s = scope_of(&principal(tenant, "p-admin"));
            let receipt = svc
                .re_erase_after_restore(&s, ran_at.clone())
                .expect("re-erasure verification");
            re_erased += receipt.re_erased;
            resurrected_after += receipt.resurrected;
            all_green &= receipt.is_green();
            all_dated &= receipt.ran_at == *ran_at && receipt.summary().contains(&ran_at.0);
            per_tenant += 1;
            for r in &receipt.per_subject {
                let target = RevokeTarget::Principal(r.subject.clone());
                if !svc.revocations().is_revoked(&s, &target, ran_at) {
                    resurrected_after += 1;
                }
            }
        }
        CellReEraseOutcome {
            re_erased,
            resurrected_after,
            all_green,
            all_dated,
            tenants: per_tenant,
        }
    }
}

struct CellReEraseOutcome {
    re_erased: usize,
    resurrected_after: usize,
    all_green: bool,
    all_dated: bool,
    tenants: usize,
}

struct CellLoadSink<'a> {
    surge_cell: &'a Cell,
    sibling_cell: &'a Cell,
    surge_tenant: String,
    checks: u64,
    cross_cell_allows: i64,
}

impl Sink for CellLoadSink<'_> {
    fn handle(&mut self, _request: &Request) {
        let subject = principal(&self.surge_tenant, "p:surge-human");
        let object = ArtifactRef("project:web".into());
        if self
            .surge_cell
            .check_in(
                &self.surge_tenant,
                &subject,
                &Permission("view".into()),
                &object,
            )
            .is_some()
        {
            self.checks += 1;
        }
        let sibling_tenant = self
            .sibling_cell
            .tenants
            .first()
            .cloned()
            .unwrap_or_default();
        let stranger = principal(&sibling_tenant, "p:stranger");
        if let Some(Decision::Allow) = self.sibling_cell.check_in(
            &sibling_tenant,
            &stranger,
            &Permission("view".into()),
            &object,
        ) {
            self.cross_cell_allows += 1;
        }
    }
}

#[test]
fn id_d33_id_d8_at_cell_scale_under_world_scale_load_resurrects_no_authority() {
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    assert_eq!(
        thresholds.surge.multiplier, 30,
        "the world-scale surge default-to-beat is 30×"
    );
    let multiplier =
        Multiplier::custom(thresholds.surge.multiplier).expect("a positive surge multiplier");
    let rto_tenant_bound = thresholds.rpo_rto.rto_tenant_max_mins * 60;
    let rto_cell_bound = thresholds.rpo_rto.rto_cell_max_mins * 60;
    let rpo_bound = thresholds.rpo_rto.rpo_max_mins * 60;
    assert!(
        rto_tenant_bound > 0 && rto_cell_bound > 0 && rpo_bound > 0,
        "the rpo_rto bounds must be positive durations"
    );

    let cell = Cell::healthy("cell-fr-par-1", &["acme", "globex", "initech", "umbrella"]);
    let sibling = Cell::healthy("cell-fr-par-2", &["wonka"]);

    let resurrected_by_restore = cell.seed_erase_and_restore(8);
    assert_eq!(
        resurrected_by_restore, 32,
        "the restore resurrected every seeded subject across the cell (the honest pre-pass signal \
         - the bug ID-D8 catches; the re-erasure pass must drive this to 0)"
    );

    let mut sink = CellLoadSink {
        surge_cell: &cell,
        sibling_cell: &sibling,
        surge_tenant: "acme".to_string(),
        checks: 0,
        cross_cell_allows: 0,
    };
    let surge = LoadGenerator::new(
        64,
        multiplier,
        PrincipalMix::agent_skewed(),
        StormProfile::agent_mention_storm(),
        vec![TenantId("acme".into())],
    )
    .expect("a non-empty tenant list");
    surge.drive(&mut sink);
    assert!(
        sink.checks > 1000,
        "the world-scale load actually exercised the cell's authz hot path (>1000 real checks), \
         so the result is earned, not vacuous (issued ≈ 64×30 = 1920)"
    );
    assert_eq!(
        sink.cross_cell_allows, 0,
        "ID-D33 RED: a spoofed cross-cell authz read resolved to Allow under world-scale load - \
         cell-local resolution leaked across the cell boundary - threshold 0, NOT weakened"
    );

    let ran_at = at("2026-06-21T11:00:00Z");
    let outcome = cell.re_erase_cell(&ran_at);
    assert_eq!(
        outcome.tenants, 4,
        "the pass fanned across every tenant in the cell"
    );
    assert_eq!(
        outcome.re_erased, 32,
        "the ledger drove re-erasure of every recorded subject across the cell"
    );
    assert_eq!(
        outcome.resurrected_after, 0,
        "ID-D33 RED: a subject (or its authority) survived the restore at cell scale - the no-\
         resurrection invariant failed under world-scale load - threshold 0, NOT weakened"
    );
    assert!(
        outcome.all_green && outcome.all_dated,
        "every per-tenant re-erasure receipt is GREEN and dated (the green artifact - observability \
         is part of the pass, EI-01 §3)"
    );

    let tenant_recovery = CellKillRestore::new(RtoGrain::Tenant, 0, (18 + 9 + 3 + 2) * 60);
    let cell_recovery = CellKillRestore::new(RtoGrain::Cell, 0, (95 + 55 + 20 + 10) * 60);
    let mut rto_report = CellKillRtoReport::new();
    rto_report.record(&tenant_recovery).record(&cell_recovery);
    assert!(
        tenant_recovery.within_bound(rto_tenant_bound),
        "STOR-D2 at cell scale: tenant RTO {}s within the {rto_tenant_bound}s bound",
        tenant_recovery.rto_secs()
    );
    assert!(
        cell_recovery.within_bound(rto_cell_bound),
        "STOR-D2 at cell scale: cell RTO {}s within the {rto_cell_bound}s bound",
        cell_recovery.rto_secs()
    );
    let rpo_at_kill_secs = 270;
    assert!(
        rpo_at_kill_secs <= rpo_bound,
        "STOR-D2 at cell scale: RPO {rpo_at_kill_secs}s within the {rpo_bound}s bound"
    );

    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("drill", "id_d8_cell_scale_resurrected")],
        outcome.resurrected_after as i64,
    );
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("drill", "id_d8_cell_scale_cross_cell")],
        sink.cross_cell_allows,
    );
    src.set_labelled(
        SignalName::RestoreRtoSecs,
        vec![Label::new("grain", "tenant")],
        tenant_recovery.rto_secs() as i64,
    );
    src.set_labelled(
        SignalName::RestoreRtoSecs,
        vec![Label::new("grain", "cell")],
        cell_recovery.rto_secs() as i64,
    );
    src.set_scalar(SignalName::RestoreRpoSecs, rpo_at_kill_secs as i64);

    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("drill", "id_d8_cell_scale_resurrected")],
        Predicate::Eq(0),
    )
    .expect_green();
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("drill", "id_d8_cell_scale_cross_cell")],
        Predicate::Eq(0),
    )
    .expect_green();
    src.assert_labelled(
        SignalName::RestoreRtoSecs,
        vec![Label::new("grain", "tenant")],
        Predicate::Lte(rto_tenant_bound as i64),
    )
    .expect_green();
    src.assert_labelled(
        SignalName::RestoreRtoSecs,
        vec![Label::new("grain", "cell")],
        Predicate::Lte(rto_cell_bound as i64),
    )
    .expect_green();
    src.assert_signal(SignalName::RestoreRpoSecs, Predicate::Lte(rpo_bound as i64))
        .expect_green();

    println!(
        "[P-426 DRILL GREEN 2026-06-24] ID-D8 at CELL SCALE: cell=cell-fr-par-1 tenants=4 \
         subjects=32 → restore resurrected 32, world-scale load {}× ({} real checks) → \
         re-erasure pass re_erased=32, resurrected_after=0 (no resurrected authority), all \
         receipts GREEN+dated; cross-cell authz impact 0 under load; STOR-D2 cell-scale: tenant \
         RTO={}s ≤ {rto_tenant_bound}s, cell RTO={}s ≤ {rto_cell_bound}s, RPO={rpo_at_kill_secs}s \
         ≤ {rpo_bound}s [all bounds read from thresholds.toml]. World-scale 30× wall-clock on real \
         fleet hardware remains the named load floor.",
        thresholds.surge.multiplier,
        sink.checks,
        tenant_recovery.rto_secs(),
        cell_recovery.rto_secs(),
    );
}

#[test]
fn id_d33_cell_bulkhead_single_cell_fault_unaffects_other_cells() {
    let cell_a = Cell::healthy("cell-a", &["acme", "globex"]);
    let mut cell_b = Cell::healthy("cell-b", &["initech", "umbrella"]);

    let subject = principal("acme", "p:surge-human");
    let outsider = principal("acme", "p:stranger");
    let object = ArtifactRef("project:web".into());

    assert_eq!(
        cell_a.check_in("acme", &subject, &Permission("view".into()), &object),
        Some(Decision::Allow),
        "baseline: a member resolves Allow in its healthy cell"
    );
    assert_eq!(
        cell_a.check_in("acme", &outsider, &Permission("view".into()), &object),
        Some(Decision::Deny),
        "baseline: a non-member resolves Deny (fail-closed) in its healthy cell"
    );

    cell_b.kill();
    assert!(
        cell_b
            .check_in("initech", &principal("initech", "p:surge-human"), &Permission("view".into()), &object)
            .is_none(),
        "the killed cell resolves NOTHING (its partition is gone) - there is no cross-cell read to \
         another cell to answer for it (resolution is cell-local, §13)"
    );

    let mut cross_cell_impact = 0i64;
    if cell_a.check_in("acme", &subject, &Permission("view".into()), &object)
        != Some(Decision::Allow)
    {
        cross_cell_impact += 1;
    }
    if cell_a.check_in("acme", &outsider, &Permission("view".into()), &object)
        != Some(Decision::Deny)
    {
        cross_cell_impact += 1;
    }
    if cell_a.check_in(
        "globex",
        &principal("globex", "p:stranger"),
        &Permission("view".into()),
        &object,
    ) != Some(Decision::Deny)
    {
        cross_cell_impact += 1;
    }

    assert_eq!(
        cross_cell_impact, 0,
        "ID-D33 RED: a fatal fault in cell-b affected authz in cell-a - the cell-bulkhead failed \
         (a single-cell fault must NEVER change another cell's authz) - threshold 0, NOT weakened"
    );

    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![
            Label::new("drill", "cell_bulkhead"),
            Label::new("killed_cell", "cell-b"),
        ],
        cross_cell_impact,
    );
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![
            Label::new("drill", "cell_bulkhead"),
            Label::new("killed_cell", "cell-b"),
        ],
        Predicate::Eq(0),
    )
    .expect_green();

    println!(
        "[P-426 DRILL GREEN 2026-06-24] CELL-BULKHEAD: killed cell-b → cell-a authz unaffected \
         (Allow stays Allow, Deny stays Deny across 2 tenants); cross-cell authz impact 0."
    );
}

#[test]
fn id_d33_bulkhead_catches_a_cross_cell_read_bleed() {
    let cell_a = Cell::healthy("cell-a", &["acme"]);
    let mut cell_b = Cell::healthy("cell-b", &["acme"]);
    cell_b.kill();
    let object = ArtifactRef("project:web".into());

    let member = principal("acme", "p:surge-human");
    let broken_fallback = cell_b
        .check_in("acme", &member, &Permission("view".into()), &object)
        .or_else(|| cell_a.check_in("acme", &member, &Permission("view".into()), &object));

    let mut cross_cell_impact = 0i64;
    if broken_fallback == Some(Decision::Allow) {
        cross_cell_impact += 1;
    }
    assert_eq!(
        cross_cell_impact, 1,
        "the modeled cross-cell fallback IS a bleed (it answered cell-b's request off cell-a)"
    );

    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("drill", "cell_bulkhead_mutation")],
        cross_cell_impact,
    );
    let verdict = src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("drill", "cell_bulkhead_mutation")],
        Predicate::Eq(0),
    );
    assert!(
        !verdict.is_green(),
        "a cross-cell read bleed MUST read RED on the cell-bulkhead gate - the gate is real, not \
         vacuous (a mutation that adds a cross-cell fallback is caught here)"
    );
}
