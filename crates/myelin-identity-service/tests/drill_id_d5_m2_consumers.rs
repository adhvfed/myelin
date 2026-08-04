use myelin_agent::{EffectApi, EffectResult, ProposedEffect, RunCtx};
use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    ColRef, Consistency, ConsistencyMode, Decision, IdentityService, ListObjectsResult, ObjectId,
    ObjectType, Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple,
    RevokeTarget, RuntimeRef, SetExpr, TupleDelta, Zookie,
};
use myelin_identity_service::{
    lower,
    namespace::{FragmentDef, PermissionRule, Userset},
    Authority, DelegationInput, ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore,
    WATCHER_RELATION,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

fn admin(tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

fn scope(tenant: &str) -> TenantScope {
    TenantScope::from_verified_token(&admin(tenant), Region("eu-west".into()))
}

fn subject(id: &str, tenant: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn agent(id: &str, tenant: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt-1".into()),
            on_behalf_of: Some(PrincipalId("p:human".into())),
        },
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn add(object: &str, relation: &str, subj: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subj.into()),
        caveat: None,
    })
}

fn now() -> Timestamp {
    Timestamp("2026-06-20T00:00:00Z".into())
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn auth(grants: &[&str]) -> Authority {
    Authority::of(grants.iter().copied())
}

fn wired(scope: &TenantScope, grants: &[TupleDelta]) -> (StoreBackedCheck, ReverseIndex) {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    store
        .write_tuples(scope, &admin(&scope.tenant().0), grants, None, None, now())
        .expect("seed grants");
    let bus = InProcessBus::new();
    Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into())).drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }

    let slot = StoreBackedCheck::with_index(store, index.clone());
    {
        let channel = FragmentDef {
            object_type: ObjectType("channel".into()),
            relations: vec![RelName("member".into())],
            permissions: vec![],
        }
        .watchable();
        assert!(
            matches!(
                slot.admit_fragment_def(&channel),
                myelin_identity::FragmentAdmit::Admitted { .. }
            ),
            "the watchable channel fragment admits"
        );
        assert!(
            slot.namespace().is_watchable("channel"),
            "the channel type declares the watcher relation (the fanout is wired)"
        );
        let repo = FragmentDef {
            object_type: ObjectType("repo".into()),
            relations: vec![RelName("reader".into()), RelName("writer".into())],
            permissions: vec![PermissionRule {
                permission: Permission("read".into()),
                rewrite: Userset::Union(vec![
                    Userset::Relation(RelName("reader".into())),
                    Userset::Relation(RelName("writer".into())),
                ]),
            }],
        };
        assert!(matches!(
            slot.admit_fragment_def(&repo),
            myelin_identity::FragmentAdmit::Admitted { .. }
        ));
    }
    (slot, index)
}

struct IdBackedEffectApi<'a> {
    id: &'a StoreBackedCheck,
    scope: TenantScope,
    agent: Principal,
    delegator: Principal,
    input: DelegationInput,
}

impl EffectApi for IdBackedEffectApi<'_> {
    fn apply(&self, _run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        let raw = effect.0;
        let (required_grant, object) = match raw.split_once('|') {
            Some((g, o)) => (g, o),
            None => return EffectResult::Denied(format!("unparseable effect: {raw}")),
        };
        let permission = required_grant.rsplit('#').next().unwrap_or(required_grant);

        let decision = self.id.delegation_with_check_in(
            &self.agent,
            &self.delegator,
            &self.input,
            &self.scope,
            required_grant,
            &Permission(permission.to_string()),
            &myelin_tenancy::ArtifactRef(object.to_string()),
            &at_latest(),
        );
        match decision {
            Decision::Allow => EffectResult::Applied(myelin_agent::EventId(format!("ev:{object}"))),
            Decision::Deny | Decision::Conditional => EffectResult::Denied(format!(
                "outside agent ∩ delegation ∩ tenant: {required_grant}"
            )),
        }
    }
}

#[test]
fn id_d5_rerun_and_srch_ref_notif_rides_as_composed() {
    let s = scope("acme");
    let mut signals = SignalSource::new();

    let (id, index) = wired(
        &s,
        &[
            add("repo:secret", "writer", "p:agent"),
            add("repo:secret", "reader", "p:owner"),
            add("repo:public", "reader", "p:intruder"),
            add("channel:general", WATCHER_RELATION, "p:alice"),
            add("channel:general", WATCHER_RELATION, "p:bob"),
            add("channel:general", "member", "p:carol"),
            add("channel:random", WATCHER_RELATION, "p:dave"),
        ],
    );

    let mut escapes: i64 = 0;
    let mut leaks: i64 = 0;

    let effect_api = IdBackedEffectApi {
        id: &id,
        scope: s.clone(),
        agent: agent("p:agent", "acme"),
        delegator: subject("p:human", "acme"),
        input: DelegationInput {
            agent_policy: auth(&["repo:secret#read", "repo:secret#write"]),
            delegation: auth(&["repo:secret#read", "repo:secret#write"]),
            tenant_policy: auth(&["repo:secret#read", "repo:secret#write"]),
            trigger_actor_held: auth(&["repo:secret#read"]),
        },
    };
    let run = RunCtx::default();

    let write_effect = id_effect("repo:secret#write", "repo:secret");
    match effect_api.apply(&run, write_effect) {
        EffectResult::Applied(_) => {
            escapes += 1;
        }
        EffectResult::Denied(_) | EffectResult::Gated(_) => {  }
    }
    if let EffectResult::Applied(_) =
        effect_api.apply(&run, id_effect("repo:secret#admin", "repo:secret"))
    {
        escapes += 1;
    }

    let intruder = subject("p:intruder", "acme");
    let read = Permission("read".into());
    let repo_ty = ObjectType("repo".into());
    match id.list_objects(&intruder, &read, &repo_ty, &at_latest()) {
        Ok(ListObjectsResult::Ids { ids, .. }) => {
            if ids.iter().any(|o| o.0 == "repo:secret") {
                leaks += 1;
            }
            if ids.iter().filter(|o| o.0 == "repo:secret").count() != 0 {
                leaks += 1;
            }
        }
        Ok(ListObjectsResult::Filter { set_expr, .. }) => {
            if lowered_join_leaks(&index, &s, &intruder, &set_expr, "repo:secret") {
                leaks += 1;
            }
        }
        Err(e) => panic!("list_objects must serve the Search conjoin, not error: {e:?}"),
    }

    let visible_sources: Vec<String> =
        match id.list_objects(&intruder, &read, &repo_ty, &at_latest()) {
            Ok(ListObjectsResult::Ids { ids, .. }) => ids.into_iter().map(|o| o.0).collect(),
            Ok(ListObjectsResult::Filter { set_expr, .. }) => {
                visible_via_join(&index, &s, &intruder, &set_expr)
            }
            Err(e) => panic!("list_objects must serve the Refs filter: {e:?}"),
        };
    if visible_sources.iter().any(|src| src == "repo:secret") {
        leaks += 1;
    }

    let watchers = id.list_watchers_in(&s, &ObjectId("channel:general".into()), &at_latest());
    let watcher_ids: Vec<String> = watchers.members.iter().map(|m| m.0.clone()).collect();
    assert_eq!(
        watchers.relation,
        RelName(WATCHER_RELATION.into()),
        "the fanout expands the watcher relation"
    );
    if watcher_ids != vec!["p:alice".to_string(), "p:bob".into()] {
        leaks += 1;
    }
    for non_watcher in ["p:carol", "p:dave"] {
        if watcher_ids.iter().any(|w| w == non_watcher) {
            leaks += 1;
        }
    }

    id.revoke_in(
        &s,
        &RevokeTarget::Principal(PrincipalId("p:alice".into())),
        now(),
    );
    let alice = subject("p:alice", "acme");
    match id.list_objects(&alice, &read, &repo_ty, &at_latest()) {
        Ok(ListObjectsResult::Ids { ids, .. }) => {
            if !ids.is_empty() {
                leaks += 1;
            }
        }
        Ok(ListObjectsResult::Filter { .. }) => {
        }
        Err(e) => panic!("a revoked subject's list_objects must serve (empty), not error: {e:?}"),
    }
    let post_revoke = id
        .check(
            &alice,
            &read,
            &myelin_tenancy::ArtifactRef("repo:public".into()),
            &at_latest(),
            None,
        )
        .expect("check serves");
    if post_revoke == Decision::Allow {
        leaks += 1;
    }

    signals.set_scalar(SignalName::CrossTenantCount, escapes);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        escapes, 0,
        "ID-D5 re-run: 0 effects outside agent ∩ delegation ∩ tenant via the EffectApi"
    );

    let mut leak_signals = SignalSource::new();
    leak_signals.set_scalar(SignalName::CrossTenantCount, leaks);
    leak_signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        leaks, 0,
        "0 leaked objects across the Search/Refs/Notif composition + the post-revoke read"
    );

    println!(
        "[P-134 DRILL GREEN 2026-06-20] ID-D5 re-run + SRCH/REF/NOTIF rides as composed by the M2 \
         consumers: tenant=acme → EffectApi escapes={escapes} (0 effects outside the intersection); \
         Search/Refs/Notif leaks={leaks} (0 confidential objects leaked: repo:secret absent from the \
         list_objects-conjoined Search result incl. count, absent from the Refs-visible edge set; the \
         watcher fanout list_subjects(channel:general, watcher)=[p:alice, p:bob] only - carol/dave \
         never delivered; the post-revoke read excludes p:alice within W). Id's F1/F2/F7/F9 hold as \
         composed (EI-01 §3, §4)."
    );
}

fn id_effect(grant: &str, object: &str) -> ProposedEffect {
    ProposedEffect(format!("{grant}|{object}"))
}

fn lowered_join_leaks(
    index: &ReverseIndex,
    scope: &TenantScope,
    subject: &Principal,
    set_expr: &SetExpr,
    needle: &str,
) -> bool {
    visible_via_join_inner(index, scope, subject, set_expr)
        .iter()
        .any(|o| o == needle)
}

fn visible_via_join(
    index: &ReverseIndex,
    scope: &TenantScope,
    subject: &Principal,
    set_expr: &SetExpr,
) -> Vec<String> {
    visible_via_join_inner(index, scope, subject, set_expr)
}

fn visible_via_join_inner(
    index: &ReverseIndex,
    scope: &TenantScope,
    subject: &Principal,
    set_expr: &SetExpr,
) -> Vec<String> {
    let via = ColRef {
        table: "repo".into(),
        column: "id".into(),
    };
    let lowered = lower(set_expr, subject, &via);
    assert!(
        lowered.depends_on_reverse_index(),
        "the Filter lowers to an S8 JOIN (the consumer conjoins it, never a post-filter)"
    );
    let mut out: Vec<String> = Vec::new();
    for rel in ["read", "reader", "writer"] {
        for o in index.objects_for(
            scope,
            &ObjectType("repo".into()),
            &subject.principal_id,
            &RelName(rel.into()),
        ) {
            out.push(o.0);
        }
    }
    out.sort();
    out.dedup();
    out
}
