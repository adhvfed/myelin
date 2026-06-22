//! # The CDC pair for contract 4.9 — Id's compiled **Issues** ReBAC fragment (P-ID-29 / P-322)
//!
//! **Contract-index row 4.9** (per-subsystem ReBAC namespace fragment — each subsystem declares
//! relations and permissions, compiled into ONE cell schema; Identity owns the engine, the
//! admit-contract, and the core hierarchy and never invents object ids). The engine half is pinned by
//! `cdc_4_9_namespace_engine.rs` (P-068); the Git fragment by `cdc_4_9_git_fragment.rs` (P-247); the
//! Knowledge fragment by `cdc_4_9_knowledge_fragment.rs` (P-249); the CI fragment by
//! `cdc_4_9_ci_fragment.rs` (P-320). THIS file pins the Identity-side compiled Issues fragment (the
//! rich rewrites Id owns, P-ID-29 / P-322).
//!
//! - The **PROVIDER** is Identity's namespace engine ([`StoreBackedCheck`] over `with_core_hierarchy`):
//!   it admits Id's compiled Issues [`FragmentDef`]s, resolves the Issues permissions through the four
//!   userset operators, and never invents an id.
//! - The **CONSUMER** is the Issues subsystem, which gates an action ONLY on a resolved grant + lists
//!   the board via `list_objects(subject, view, issue)` keyed on `issue.id` (§7.3) — modelled here as
//!   the board scan + the per-action `check`/`CaveatContext` surface.
//!
//! The two sides are pinned together: Id's compiled fragment
//! ([`myelin_identity_service::issue_fragment`]) must agree on the relation/permission NAMES with the
//! Issues subsystem's names-only carrier (`myelin_issues::rebac_fragment`, ISS-P01) — but
//! `myelin-identity-service` does NOT depend on the Issues leaf crate (the DAG floor), so the
//! name-agreement is asserted against the architecture §5 frozen vocabulary here.
//!
//! **The headline invariant this CDC behaviourally pins (ISS-D3 authz side):**
//! - **the `− confidential` exclusion (§5):** a confidential issue is ABSENT from a normal
//!   project-reader's `list_objects(subject, view, issue)` BY CONSTRUCTION (the Exclusion removes it
//!   from the `view` set — never a post-filter, never a count leak); ONLY an explicit
//!   `issue#confidential_grant@subject` re-admits.
//! - **the field/transition caveats (§8.6):** a field caveat hides a field, a transition caveat gates
//!   a transition — both OFF the hot `list_objects` path, through the ONE `QueryAst` core; a denied
//!   field/transition is `Deny`, a missing-context one is `Conditional`, never a silent allow.

use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    ColRef, Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService,
    ListObjectsResult, Literal, ObjectId, ObjectType, Permission, Principal, PrincipalId,
    PrincipalKind, RelName, RelationTuple, SetExpr, TupleDelta, Zookie,
};
use myelin_identity_service::{
    eval_caveat, issue_field_view_caveat, transition_caveat, ListObjects, NamespaceEngine,
    ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore, CONFIDENTIAL,
    CONFIDENTIAL_GRANT, ISSUE_PERFORM_TRANSITION, ISSUE_VIEW, ISSUE_VIEW_FIELD,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("p-admin".into()),
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

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

/// The PROVIDER surface seeded with `tuples` — the core org/team/project hierarchy is preloaded, then
/// Id's compiled Issues fragment is admitted on top (so `issue.view`'s `parent_project->view`
/// inheritance terminates on the core `project.view`).
fn provider(scope: &TenantScope, tuples: &[TupleDelta]) -> StoreBackedCheck {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            tuples,
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed tuples");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env);
    }
    let svc = StoreBackedCheck::with_index(store, index);
    for admit in svc.admit_issue_fragment() {
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "Id's compiled Issues fragment admits: {admit:?}"
        );
    }
    svc
}

/// **PROVIDER → the compiled Issues fragment ADMITS into the cell schema (the engine-only-floor
/// progression).** Id declares + compiles its Issues fragment via the fragment-admit contract; every
/// Issues object type admits on top of the core hierarchy; the headline permissions are compiled.
#[test]
fn cdc_4_9_id_compiled_issue_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);
    let ns = svc.namespace();
    for ty in ["issue", "field", "transition"] {
        assert!(
            ns.object_types().contains(&ty.to_string()),
            "`{ty}` admitted into the cell schema"
        );
    }
    assert!(ns.resolve_permission("issue", ISSUE_VIEW).is_some());
    assert!(ns.resolve_permission("field", ISSUE_VIEW_FIELD).is_some());
    assert!(ns
        .resolve_permission("transition", ISSUE_PERFORM_TRANSITION)
        .is_some());
}

/// **CONSUMER → PROVIDER: a confidential issue DISAPPEARS from a normal project-reader's `check` BY
/// CONSTRUCTION (the `− confidential` exclusion, ISS-D3).** alice is a project member (so she inherits
/// `parent_project->view` on every issue under the project); a normal issue she sees, a `confidential`
/// issue she does NOT — until she is explicitly `confidential_grant`ed.
#[test]
fn cdc_4_9_confidential_issue_disappears_for_a_normal_reader() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            // alice is a project reader → inherits view on issues via parent_project->view.
            add("project:proj", "reader", "p:alice"),
            // bob is a project reader too — but gets an explicit confidential_grant on the secret issue.
            add("project:proj", "reader", "p:bob"),
            // A NORMAL issue under the project (no confidential stamp).
            add("issue:normal", "parent_project", "project:proj#view"),
            // A CONFIDENTIAL issue: same project parent, but stamped confidential AND alice is
            // subtracted by the exclusion (the stamp is on the issue, removing the ambient reader set).
            add("issue:secret", "parent_project", "project:proj#view"),
            add("issue:secret", CONFIDENTIAL, "p:alice"),
            // bob is explicitly re-admitted to the confidential issue.
            add("issue:secret", CONFIDENTIAL_GRANT, "p:bob"),
        ],
    );
    let can_view = |actor: &Principal, issue: &str| {
        matches!(
            svc.check(
                actor,
                &Permission(ISSUE_VIEW.into()),
                &ArtifactRef(issue.into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    // alice sees the normal issue (inherits parent_project->view)…
    assert!(
        can_view(&subject("p:alice"), "issue:normal"),
        "a project reader views a normal issue (parent_project->view)"
    );
    // …but the confidential issue DISAPPEARS for her (the − confidential exclusion, by construction).
    assert!(
        !can_view(&subject("p:alice"), "issue:secret"),
        "a confidential issue disappears for a normal reader (the − confidential exclusion, ISS-D3)"
    );
    // bob, explicitly re-admitted, sees it (the ∪ confidential_grant arm).
    assert!(
        can_view(&subject("p:bob"), "issue:secret"),
        "an explicit confidential_grant re-admits the issue (the ∪ confidential_grant arm)"
    );
    // An outsider sees neither (fail-closed).
    assert!(
        !can_view(&subject("p:carol"), "issue:normal"),
        "an outsider views nothing (fail-closed)"
    );
}

/// Build a wired `list_objects` over Id's compiled Issues fragment + a LIVE S8 index fed off the bus
/// from `grants`, at an explicit cardinality `cap` (so the Ids↔Filter switch is deterministic, as
/// `drill_git_d11_pr_list.rs::wired` does). The S8 reverse index projects only DIRECT principal-subject
/// grants (an inheritance/userset edge is NOT a candidate row — `reverse_index.rs`), so the materialise
/// candidate source for an issue is a direct relation on it (e.g. `confidential_grant`); the inherited
/// `parent_project->view` arm of the board scan is resolved by the consumer's JOIN against
/// `authz_visible` on the `Filter` push-down (P-ID-12).
fn wired(cap: usize, scope: &TenantScope, grants: &[TupleDelta]) -> ListObjects {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let mut namespace = NamespaceEngine::with_core_hierarchy();
    for def in myelin_identity_service::issue_fragment::issue_fragment_defs() {
        let admit = namespace.admit(&def);
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "the Issues `{}` fragment admits",
            def.object_type.0
        );
    }
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            grants,
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env);
    }
    ListObjects::with_cap(store, namespace, index, cap)
}

/// **CONSUMER → PROVIDER: the board conjoins in ONE query — `list_objects(subject, view, issue)` pushes
/// down to the `issue.id` Filter (§7.3, contract 4.3).** Above the cap the board scan returns the
/// S8 push-down naming the consumer's OWN id column (`issue.id`) which the board's query planner
/// conjoins in ONE query (no N+1, never a post-filter) — the inherited `parent_project->view` arm is
/// resolved by the JOIN against `authz_visible`, exactly as the Git PR-list board does (GIT-D11).
#[test]
fn cdc_4_9_board_conjoins_in_one_query() {
    let s = scope("acme");
    // A confidential-grant'd board reader holds a direct relation on a handful of issues; with the cap
    // BELOW that slice the scan pushes down to the Filter (the board's one-query conjoin).
    let mut grants: Vec<TupleDelta> = Vec::new();
    for i in 0..8 {
        grants.push(add(&format!("issue:b-{i}"), CONFIDENTIAL_GRANT, "p:alice"));
    }
    let lo = wired(2, &s, &grants);
    let result = lo.list_objects(
        &s,
        &subject("p:alice"),
        &Permission(ISSUE_VIEW.into()),
        &ObjectType("issue".into()),
        &at_latest(),
    );
    match result {
        ListObjectsResult::Filter { set_expr, .. } => match set_expr {
            SetExpr::InRelation { via_column, .. } => {
                assert_eq!(
                    via_column,
                    ColRef {
                        table: "issue".into(),
                        column: "id".into()
                    },
                    "the board Filter names the consumer's own id column (issue.id, §7.3) — \
                     one query, no N+1"
                );
            }
            other => panic!("the board Filter is the InRelation push-down shape, got {other:?}"),
        },
        ListObjectsResult::Ids { .. } => {
            panic!("above the cap the board must push down to the issue.id Filter (the one-query conjoin)")
        }
    }
}

/// **CONSUMER → PROVIDER: the board materialise is LEAK-FREE — a confidential issue alice is NOT
/// granted is ABSENT from her board (no count leak, ISS-D3).** Below the cap the board materialises
/// `Ids` carrying ONLY alice's directly-visible issues; a confidential issue she has no grant on never
/// becomes a candidate (the S8 reverse index keys on `(subject, relation)` — an un-granted issue is not
/// even a candidate, never a post-filter). Combined with the `check`-side exclusion proof
/// ([`cdc_4_9_confidential_issue_disappears_for_a_normal_reader`]), this is the by-construction no-leak.
#[test]
fn cdc_4_9_board_materialise_is_leak_free_confidential_absent() {
    let s = scope("acme");
    let grants = vec![
        // alice is re-admitted (direct confidential_grant) to two issues → her visible board.
        add("issue:visible-1", CONFIDENTIAL_GRANT, "p:alice"),
        add("issue:visible-2", CONFIDENTIAL_GRANT, "p:alice"),
        // A confidential issue granted to SOMEONE ELSE — the leak witness alice must never see.
        add("issue:secret", CONFIDENTIAL_GRANT, "p:other"),
        add("issue:secret", CONFIDENTIAL, "p:alice"),
    ];
    let lo = wired(100, &s, &grants);
    let result = lo.list_objects(
        &s,
        &subject("p:alice"),
        &Permission(ISSUE_VIEW.into()),
        &ObjectType("issue".into()),
        &at_latest(),
    );
    let ids = match result {
        ListObjectsResult::Ids { ids, .. } => ids.into_iter().map(|o| o.0).collect::<Vec<String>>(),
        ListObjectsResult::Filter { .. } => panic!("below the cap the board materialises Ids"),
    };
    assert!(
        ids.iter().any(|i| i == "issue:visible-1") && ids.iter().any(|i| i == "issue:visible-2"),
        "alice's board lists her two re-admitted issues: {ids:?}"
    );
    assert!(
        !ids.iter().any(|i| i == "issue:secret"),
        "the confidential issue alice has no grant on is ABSENT from her board (leak-free, no count \
         leak — ISS-D3): {ids:?}"
    );
}

/// **CONSUMER → PROVIDER: a FIELD caveat hides a field OFF the hot path (§8.6, C3).** The board lists
/// the visible issues cheaply; `check(subject, view_field, field, CaveatContext)` then redacts an
/// individual field through the ONE `QueryAst` core. A cleared viewer sees the field (Allow); an
/// under-cleared one has it redacted (Deny); a missing-context one is Conditional (never a silent
/// allow).
#[test]
fn cdc_4_9_field_caveat_hides_a_field_off_the_hot_path() {
    // Cleared (clearance 5 ≥ threshold 3) → Allow.
    let cleared = issue_field_view_caveat(
        "field:issue-1/salary",
        "salary",
        "ge",
        "clearance",
        Literal::Int(3),
        &[("clearance", Literal::Int(5))],
    );
    assert_eq!(
        eval_caveat(&cleared),
        Decision::Allow,
        "cleared viewer sees the field"
    );

    // Under-cleared (clearance 1 < 3) → Deny (redacted, absent from the projection).
    let blocked = issue_field_view_caveat(
        "field:issue-1/salary",
        "salary",
        "ge",
        "clearance",
        Literal::Int(3),
        &[("clearance", Literal::Int(1))],
    );
    assert_eq!(
        eval_caveat(&blocked),
        Decision::Deny,
        "under-cleared viewer's field is redacted"
    );

    // Missing context → Conditional (the caller supplies it) — never a silent allow.
    let missing = issue_field_view_caveat(
        "field:issue-1/salary",
        "salary",
        "ge",
        "clearance",
        Literal::Int(3),
        &[],
    );
    assert_eq!(
        eval_caveat(&missing),
        Decision::Conditional,
        "a field caveat needing missing context is Conditional, never a silent allow (§8.6)"
    );
    assert!(
        missing.field.is_some() && missing.transition.is_none(),
        "it is a FIELD caveat (field? set, transition? unset)"
    );
}

/// **CONSUMER → PROVIDER: a TRANSITION caveat gates a transition OFF the hot path (§8.6, C3).** Who may
/// ATTEMPT the transition is the resolved `perform_transition` permission; the actual move is gated by
/// the transition caveat ("needs an approver edge") through the ONE `QueryAst` core. Enough approvers →
/// Allow; too few → Deny (gated); missing context → Conditional (never a silent allow).
#[test]
fn cdc_4_9_transition_caveat_gates_a_transition_off_the_hot_path() {
    let approved = transition_caveat(
        "transition:issue-1/approve",
        "approve",
        "ge",
        "approver_count",
        Literal::Int(2),
        &[("approver_count", Literal::Int(2))],
    );
    assert_eq!(
        eval_caveat(&approved),
        Decision::Allow,
        "a transition with enough approvers is permitted"
    );

    let gated = transition_caveat(
        "transition:issue-1/approve",
        "approve",
        "ge",
        "approver_count",
        Literal::Int(2),
        &[("approver_count", Literal::Int(1))],
    );
    assert_eq!(
        eval_caveat(&gated),
        Decision::Deny,
        "a transition lacking the approver edge is gated (Deny)"
    );

    let missing = transition_caveat(
        "transition:issue-1/approve",
        "approve",
        "ge",
        "approver_count",
        Literal::Int(2),
        &[],
    );
    assert_eq!(
        eval_caveat(&missing),
        Decision::Conditional,
        "a transition caveat needing missing context is Conditional, never a silent allow (§8.6)"
    );
    assert!(
        missing.transition.is_some() && missing.field.is_none(),
        "it is a TRANSITION caveat (transition? set, field? unset)"
    );
}
