use std::path::{Path, PathBuf};

use myelin_lints::{
    flow_determinism, forward_only_migration, no_cross_db, no_untagged_personal_data,
    tenant_predicate,
};

fn fixture(name: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

#[test]
fn forward_only_migration_rejects_destructive_admits_expand() {
    let red = fixture("forward_only_migration.flow.red.rs.txt");
    assert!(
        !forward_only_migration().run(&red).is_empty(),
        "a down/blocking-ALTER migration must be REJECTED by forward-only-migration (§9, the data-loss floor)"
    );
    let green = fixture("forward_only_migration.flow.green.rs.txt");
    assert!(
        forward_only_migration().run(&green).is_empty(),
        "the expand step (nullable add + CONCURRENTLY index) must be ADMITTED by forward-only-migration"
    );
}

#[test]
fn no_untagged_personal_data_rejects_untagged_admits_tagged() {
    let red = fixture("no_untagged_personal_data.flow.red.rs.txt");
    assert!(
        !no_untagged_personal_data().run(&red).is_empty(),
        "an untagged inline-PII column (`message_body`) must be REJECTED by no-untagged-personal-data"
    );
    let green = fixture("no_untagged_personal_data.flow.green.rs.txt");
    assert!(
        no_untagged_personal_data().run(&green).is_empty(),
        "a #[personal_data(...)]-tagged column must be ADMITTED (the flow schema's CryptoShred tag shape)"
    );
}

#[test]
fn tenant_predicate_rejects_tenantless_admits_tenant_scoped() {
    let red = fixture("tenant_predicate.flow.red.rs.txt");
    assert!(
        !tenant_predicate().run(&red).is_empty(),
        "a tenant-less query must be REJECTED by tenant-predicate (ID-3, the IDOR floor)"
    );
    let green = fixture("tenant_predicate.flow.green.rs.txt");
    assert!(
        tenant_predicate().run(&green).is_empty(),
        "a tenant-scoped query must be ADMITTED by tenant-predicate"
    );
}

#[test]
fn no_cross_db_rejects_storage_reach_admits_contract_coupling() {
    let red = fixture("no_cross_db.flow.red.rs.txt");
    assert!(
        !no_cross_db().run(&red).is_empty(),
        "a reach into a sibling service's internal store must be REJECTED by no-cross-db (ADR-01)"
    );
    let green = fixture("no_cross_db.flow.green.rs.txt");
    assert!(
        no_cross_db().run(&green).is_empty(),
        "coupling over the frozen contract surface (ArtifactRef/TenantId) must be ADMITTED by no-cross-db"
    );
}

#[test]
fn flow_determinism_rejects_raw_nondeterminism_admits_wfctx() {
    let red = fixture("flow_determinism.flow.red.rs.txt");
    let violations = flow_determinism().run(&red);
    assert!(
        !violations.is_empty(),
        "a workflow body reading SystemTime/RNG/IO outside WfCtx must be REJECTED by \
         flow-determinism (the non-deterministic-replay floor, index 9.2/§10.3)"
    );
    assert!(
        violations.len() >= 4,
        "all four raw non-deterministic reads (SystemTime::now / rand:: / tokio::time::sleep / \
         Uuid::new_v4) must each be flagged, got {}: {violations:?}",
        violations.len()
    );

    let green = fixture("flow_determinism.flow.green.rs.txt");
    assert!(
        flow_determinism().run(&green).is_empty(),
        "the same logic expressed via ctx.now()/ctx.rand()/ctx.activity(..) must be ADMITTED by \
         flow-determinism (it reads no clock/RNG/IO outside the deterministic WfCtx surface)"
    );
}

mod green_compiles {
    include!("fixtures/flow_determinism.flow.green.rs.txt");

    #[test]
    fn flow_determinism_green_fixture_compiles_against_real_wfctx() {
        let _body: fn(&mut myelin_flow::WfCtx) = nightly_digest_workflow;
        let _ = make_error();
    }
}

#[test]
fn flow_determinism_rejects_raw_ci_pipeline_body_admits_wfctx_ci_pipeline() {
    let red = fixture("ci_pipeline.flow.red.rs.txt");
    let violations = flow_determinism().run(&red);
    assert!(
        !violations.is_empty(),
        "a CI-pipeline body reading SystemTime/RNG/IO outside WfCtx must be REJECTED by \
         flow-determinism (the non-deterministic-replay floor, index 9.2/§4.9)"
    );
    assert!(
        violations.len() >= 4,
        "all four raw non-deterministic reads (SystemTime::now / rand:: / tokio::time::sleep / \
         Uuid::new_v4) in the CI-pipeline body must each be flagged, got {}: {violations:?}",
        violations.len()
    );

    let green = fixture("ci_pipeline.flow.green.rs.txt");
    assert!(
        flow_determinism().run(&green).is_empty(),
        "the CI-pipeline body expressed via run_ci_pipeline → SCHEDULE_AND_RUN_JOB must be ADMITTED \
         by flow-determinism (it reads no clock/RNG/IO outside the deterministic WfCtx surface)"
    );
}

mod ci_pipeline_green_compiles {
    use myelin_flow::JobKind;

    include!("fixtures/ci_pipeline.flow.green.rs.txt");

    struct NoopCiRunner;
    impl JobRunner for NoopCiRunner {
        fn dispatch(&self, _spec: &JobSpec) -> Result<(), myelin_flow::ActivityError> {
            Ok(())
        }
    }

    #[test]
    fn ci_pipeline_green_fixture_compiles_against_real_wfctx() {
        let _body: fn(
            &mut myelin_flow::WfCtx,
            &NoopCiRunner,
        ) -> myelin_flow::WfResult<myelin_flow::PipelineOutcome> =
            ci_pipeline_workflow::<NoopCiRunner>;
        let _ = JobKind::Ci;
        let _ = _example_spec();
    }
}
