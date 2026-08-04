use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    CaveatContext, Consistency, ConsistencyMode, Decision, IdentityService, Literal, ObjectId,
    Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use std::collections::BTreeMap;

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

fn subject(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn provider(scope: &TenantScope, tuples: &[TupleDelta]) -> StoreBackedCheck {
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            tuples,
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .expect("seed tuples");
    StoreBackedCheck::new(store)
}

fn write_path_gated_action<S: IdentityService>(
    svc: &S,
    actor: &Principal,
    permission: &str,
    object: &ArtifactRef,
    caveat: Option<&CaveatContext>,
) -> bool {
    let decision = svc.check(
        actor,
        &Permission(permission.to_string()),
        object,
        &at_latest(),
        caveat,
    );
    matches!(decision, Ok(Decision::Allow))
}

fn grant(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

#[test]
fn cdc_4_2_granted_action_proceeds() {
    let s = scope("acme");
    let svc = provider(&s, &[grant("repo:core", "write", "p:alice")]);
    let obj = ArtifactRef("myelin://acme/git/repo/repo:core".into());
    assert!(
        write_path_gated_action(&svc, &subject("p:alice"), "write", &obj, None),
        "the write-path caller proceeds when check Allows the granted action"
    );
}

#[test]
fn cdc_4_2_ungranted_action_refused() {
    let s = scope("acme");
    let svc = provider(&s, &[grant("repo:core", "write", "p:alice")]);
    let obj = ArtifactRef("myelin://acme/git/repo/repo:core".into());
    assert!(
        !write_path_gated_action(&svc, &subject("p:bob"), "write", &obj, None),
        "the write-path caller refuses the action when check does not Allow (fail-closed)"
    );
}

#[test]
fn cdc_4_2_caveat_gates_the_action() {
    let s = scope("acme");
    let svc = provider(&s, &[grant("issue:PROJ-1", "view_field", "p:alice")]);
    let obj = ArtifactRef("myelin://acme/issues/issue/issue:PROJ-1".into());

    let mut ok = BTreeMap::new();
    ok.insert("__caveat_op".to_string(), Literal::Str("lt".into()));
    ok.insert("__caveat_lhs".to_string(), Literal::Int(3));
    ok.insert("__caveat_rhs".to_string(), Literal::Int(5));
    let cav_ok = CaveatContext {
        object: obj.clone(),
        field: Some(myelin_identity::FieldId("salary".into())),
        transition: None,
        attrs: ok,
    };
    assert!(
        write_path_gated_action(&svc, &subject("p:alice"), "view_field", &obj, Some(&cav_ok)),
        "a satisfied literal caveat proceeds (the field is visible)"
    );

    let mut bad = BTreeMap::new();
    bad.insert("__caveat_op".to_string(), Literal::Str("lt".into()));
    bad.insert("__caveat_lhs".to_string(), Literal::Int(9));
    bad.insert("__caveat_rhs".to_string(), Literal::Int(5));
    let cav_bad = CaveatContext {
        object: obj.clone(),
        field: Some(myelin_identity::FieldId("salary".into())),
        transition: None,
        attrs: bad,
    };
    assert!(
        !write_path_gated_action(
            &svc,
            &subject("p:alice"),
            "view_field",
            &obj,
            Some(&cav_bad)
        ),
        "a violated literal caveat refuses (the field is redacted)"
    );
}

#[test]
fn cdc_4_2_non_literal_caveat_gates_through_query_ast_core() {
    let s = scope("acme");
    let svc = provider(&s, &[grant("issue:PROJ-1", "view_field", "p:alice")]);
    let obj = ArtifactRef("myelin://acme/issues/issue/issue:PROJ-1".into());

    let predicate_keys = |severity: i64, threshold: i64| {
        let mut attrs = BTreeMap::new();
        attrs.insert("__caveat_op".to_string(), Literal::Str("lt".into()));
        attrs.insert(
            "__caveat_lhs_var".to_string(),
            Literal::Str("severity".into()),
        );
        attrs.insert(
            "__caveat_rhs_var".to_string(),
            Literal::Str("threshold".into()),
        );
        attrs.insert("severity".to_string(), Literal::Int(severity));
        attrs.insert("threshold".to_string(), Literal::Int(threshold));
        CaveatContext {
            object: obj.clone(),
            field: Some(myelin_identity::FieldId("salary".into())),
            transition: None,
            attrs,
        }
    };

    let cav_ok = predicate_keys(2, 5);
    assert!(
        write_path_gated_action(&svc, &subject("p:alice"), "view_field", &obj, Some(&cav_ok)),
        "a satisfied NON-LITERAL caveat (variable operands) proceeds through the QueryAst core"
    );

    let cav_bad = predicate_keys(8, 5);
    assert!(
        !write_path_gated_action(
            &svc,
            &subject("p:alice"),
            "view_field",
            &obj,
            Some(&cav_bad)
        ),
        "a violated NON-LITERAL caveat redacts (the field is hidden)"
    );

    let mut missing = BTreeMap::new();
    missing.insert("__caveat_op".to_string(), Literal::Str("lt".into()));
    missing.insert(
        "__caveat_lhs_var".to_string(),
        Literal::Str("severity".into()),
    );
    missing.insert(
        "__caveat_rhs_var".to_string(),
        Literal::Str("threshold".into()),
    );
    let cav_missing = CaveatContext {
        object: obj.clone(),
        field: Some(myelin_identity::FieldId("salary".into())),
        transition: None,
        attrs: missing,
    };
    assert_eq!(
        svc.check(
            &subject("p:alice"),
            &Permission("view_field".into()),
            &obj,
            &at_latest(),
            Some(&cav_missing),
        ),
        Ok(Decision::Conditional),
        "a non-literal caveat whose variable is unbound is Conditional, never a silent Allow"
    );
}

#[test]
fn cdc_4_2_missing_context_caveat_does_not_proceed() {
    let s = scope("acme");
    let svc = provider(&s, &[grant("issue:PROJ-1", "view_field", "p:alice")]);
    let obj = ArtifactRef("myelin://acme/issues/issue/issue:PROJ-1".into());
    let mut attrs = BTreeMap::new();
    attrs.insert("__caveat_op".to_string(), Literal::Str("lt".into()));
    let cav = CaveatContext {
        object: obj.clone(),
        field: Some(myelin_identity::FieldId("salary".into())),
        transition: None,
        attrs,
    };
    let decision = svc.check(
        &subject("p:alice"),
        &Permission("view_field".into()),
        &obj,
        &at_latest(),
        Some(&cav),
    );
    assert_eq!(
        decision,
        Ok(Decision::Conditional),
        "a missing-context caveat is Conditional (never a silent Allow)"
    );
    assert!(
        !write_path_gated_action(&svc, &subject("p:alice"), "view_field", &obj, Some(&cav)),
        "the write path does not proceed on Conditional (fail-closed)"
    );
}
