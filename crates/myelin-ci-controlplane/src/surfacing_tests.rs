use super::*;
use crate::check_emitter::CheckState;

#[test]
fn ci_artifact_refs_round_trip_canonical_keys() {
    let run = ci_run_ref("acme", "01J7RUN").unwrap();
    assert_eq!(myelin_refs::format(&run), "myelin://acme/ci/run/01J7RUN");
    assert_eq!(myelin_refs::parse(&myelin_refs::format(&run)).unwrap(), run);

    assert_eq!(
        myelin_refs::format(&ci_deployment_ref("acme", "dep9").unwrap()),
        "myelin://acme/ci/deployment/dep9"
    );
    assert_eq!(
        myelin_refs::format(&ci_pipeline_ref("acme", "pl3").unwrap()),
        "myelin://acme/ci/pipeline/pl3"
    );
    assert_eq!(
        myelin_refs::format(&ci_runner_ref("acme", "rn1").unwrap()),
        "myelin://acme/ci/runner/rn1"
    );
    assert_eq!(
        myelin_refs::format(&ci_artifact_ref("acme", "art2").unwrap()),
        "myelin://acme/ci/artifact/art2"
    );
}

#[test]
fn ci_artifact_refs_reject_ambiguous_root_components() {
    assert_eq!(
        ci_run_ref("", "run-1"),
        Err(CiRefError::InvalidComponent {
            component: "tenant"
        })
    );
    assert_eq!(
        ci_run_ref("acme/eu", "run-1"),
        Err(CiRefError::InvalidComponent {
            component: "tenant"
        })
    );
    assert_eq!(
        ci_artifact_ref("acme", "manifest#latest"),
        Err(CiRefError::InvalidComponent { component: "id" })
    );
}

#[test]
fn step_mint_is_stable_and_matches_check_details() {
    let run = ci_run_ref("acme", "01J7RUN").unwrap();
    let first = run_step_ref(&run, 3).unwrap();
    let second = run_step_ref(&run, 3).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        myelin_refs::format(&first),
        "myelin://acme/ci/run/01J7RUN#step-3"
    );
    assert_eq!(
        myelin_refs::format(&first),
        crate::check_emitter::details_ref(&run.0, CheckState::Failure, Some(3))
    );
}

#[test]
fn line_range_and_check_mints_use_the_shared_codec() {
    let run = ci_run_ref("acme", "01J7RUN").unwrap();
    let range = run_step_line_ref(&run, 42, 88).unwrap();
    assert_eq!(
        myelin_refs::format(&range),
        "myelin://acme/ci/run/01J7RUN#L42-L88"
    );
    assert!(run_step_line_ref(&run, 88, 42).is_err());

    let check = commit_check_ref(&run, "build").unwrap();
    assert_eq!(
        myelin_refs::format(&check),
        "myelin://acme/ci/run/01J7RUN#check-build"
    );
}
