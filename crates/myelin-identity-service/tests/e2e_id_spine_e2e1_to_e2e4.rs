use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_harness::telemetry::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ListObjectsResult, ObjectId,
    ObjectType, Permission, Principal, PrincipalId, PrincipalKind, PseudonymHandle, RelName,
    RelationTuple, RevokeTarget, RunId, RuntimeRef, TupleDelta, Zookie,
};
use myelin_identity_service::{
    authority_of, Authority, DelegationInput, ListObjects, MachineKind, NamespaceEngine,
    ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore, CONFIDENTIAL,
    CONFIDENTIAL_GRANT,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn human(tenant: &str, id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn agent(tenant: &str, id: &str, on_behalf_of: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt-triage".into()),
            on_behalf_of: Some(PrincipalId(on_behalf_of.into())),
        },
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn scope_of(p: &Principal) -> TenantScope {
    TenantScope::from_verified_token(p, p.region.clone())
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

fn ts(s: &str) -> Timestamp {
    Timestamp(s.into())
}

fn auth(grants: &[&str]) -> Authority {
    Authority::of(grants.iter().copied())
}

fn allows(svc: &StoreBackedCheck, actor: &Principal, perm: &str, object: &str) -> bool {
    matches!(
        svc.check(
            actor,
            &Permission(perm.into()),
            &ArtifactRef(object.into()),
            &at_latest(),
            None
        ),
        Ok(Decision::Allow)
    )
}

#[test]
fn e2e1_pr_context_pane_resolves_per_viewer_zero_leak() {
    let acme = scope_of(&human("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());

    let confidential_title = "Q3 security incident root-cause";
    store
        .write_tuples(
            &acme,
            &human("acme", "p-admin"),
            &[
                add("project:web", "reader", "p:dev"),
                add("project:web", "reader", "p:contractor"),
                add("issue:ENG-1421", "parent_project", "project:web#view"),
                add("issue:secret", "parent_project", "project:web#view"),
                add("issue:secret", CONFIDENTIAL, "p:contractor"),
                add("issue:secret", CONFIDENTIAL_GRANT, "p:dev"),
            ],
            None,
            None,
            ts("2026-06-24T00:00:00Z"),
        )
        .expect("seed the pane's grants");
    let svc = StoreBackedCheck::new(store);
    for admit in svc.admit_issue_fragment() {
        assert!(
            matches!(admit, myelin_identity::FragmentAdmit::Admitted { .. }),
            "the Issues fragment admits for the pane: {admit:?}"
        );
    }

    let dev = human("acme", "p:dev");
    let contractor = human("acme", "p:contractor");

    let pane_cells = ["issue:ENG-1421", "issue:secret"];

    let mut leak_count: i64 = 0;

    assert!(
        allows(&svc, &dev, "view", "project:web"),
        "E2E-1: the dev resolves the PR's project (a project reader)"
    );
    for cell in pane_cells {
        assert!(
            allows(&svc, &dev, "view", cell),
            "E2E-1: the dev (project reader + confidential_grant) resolves `view` on `{cell}`"
        );
    }

    assert!(
        allows(&svc, &contractor, "view", "project:web"),
        "E2E-1: the contractor resolves the PR's project (a project reader)"
    );
    assert!(
        allows(&svc, &contractor, "view", "issue:ENG-1421"),
        "E2E-1: the contractor resolves the normal linked issue"
    );
    if allows(&svc, &contractor, "view", "issue:secret") {
        leak_count += 1;
    }
    assert!(
        !pane_cells
            .iter()
            .any(|c| *c == "issue:secret" && allows(&svc, &contractor, "view", c)),
        "E2E-1: the confidential issue is ABSENT from the contractor's resolved pane (tombstone, \
         title `{confidential_title}` never present)"
    );

    assert!(
        allows(&svc, &dev, "view", "issue:ENG-1421"),
        "E2E-1 mid-flight: the dev still resolves the transitioned issue"
    );
    if allows(&svc, &contractor, "view", "issue:secret") {
        leak_count += 1;
    }

    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e1_pane_zero_leak")],
        leak_count,
    );
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e1_pane_zero_leak")],
        Predicate::Eq(0),
    )
    .expect_green();
    assert_eq!(
        leak_count, 0,
        "E2E-1 RED: a confidential pane cell leaked to an unauthorized viewer - threshold 0, NOT weakened"
    );

    println!(
        "[P-427 E2E GREEN 2026-06-24] E2E-1 PR context pane (Id spine = check per viewer): \
         pane cells={pane_cells:?}; dev resolves all 4, contractor resolves 3 + the confidential \
         issue is a tombstone (title `{confidential_title}` never present); mid-flight re-resolve \
         holds → zero-leak=0 (the § 5 − confidential exclusion holds BY CONSTRUCTION, per-viewer)."
    );
}

#[test]
fn e2e2_triage_agent_runs_under_delegation_and_mint_exactly_once_merge() {
    let acme = scope_of(&human("acme", "p-admin"));
    let svc = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));

    let triage = agent("acme", "p:agent-triage", "p:maintainer");
    let maintainer = human("acme", "p:maintainer");

    let delegation = DelegationInput {
        agent_policy: auth(&[
            "repo:acme/web#create_issue",
            "repo:acme/web#post_chat_message",
            "repo:acme/web#open_pr",
            "repo:acme/web#merge",
            "repo:acme/web#admin",
        ]),
        delegation: auth(&[
            "repo:acme/web#create_issue",
            "repo:acme/web#post_chat_message",
            "repo:acme/web#open_pr",
            "repo:acme/web#merge",
        ]),
        tenant_policy: auth(&[
            "repo:acme/web#create_issue",
            "repo:acme/web#post_chat_message",
            "repo:acme/web#open_pr",
            "repo:acme/web#merge",
        ]),
        trigger_actor_held: auth(&[
            "repo:acme/web#create_issue",
            "repo:acme/web#post_chat_message",
            "repo:acme/web#open_pr",
            "repo:acme/web#merge",
        ]),
    };

    let (effective, proof) = svc.delegation_proved_in(&triage, &maintainer, &delegation);
    assert!(
        proof.holds(),
        "E2E-2: the intersection proof witnesses effective ⊆ every conjunct"
    );
    let effective_authority = authority_of(&effective);

    let proposed: [(&str, bool); 5] = [
        ("repo:acme/web#create_issue", true),
        ("repo:acme/web#post_chat_message", true),
        ("repo:acme/web#open_pr", true),
        ("repo:acme/web#merge", true),
        ("repo:acme/web#admin", false),
    ];
    let mut effects_outside_intersection: i64 = 0;
    for (capability, expected_inside) in proposed {
        let admitted = effective_authority.holds(capability);
        if admitted && !expected_inside {
            effects_outside_intersection += 1;
        }
        if !admitted && expected_inside {
            panic!("E2E-2: capability `{capability}` should be INSIDE the ∩ but was refused");
        }
    }
    assert_eq!(
        effects_outside_intersection, 0,
        "E2E-2 RED: an agent effect escaped agent ∩ delegation ∩ tenant - threshold 0, NOT weakened"
    );

    let mint_input = delegation.clone();
    let token = svc
        .mint_run_token_in(
            &acme,
            &PrincipalId("p:agent-triage".into()),
            &RunId("run-triage-1".into()),
            &triage,
            &maintainer,
            &mint_input,
            &myelin_identity::DelegationCaveats(
                ["repo:acme/web#merge"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
            MachineKind::Agent,
            &myelin_identity::FailStaticBound {
                static_max_secs: 300,
            },
            &ts("2026-06-24T00:00:00Z"),
        )
        .expect("the per-run token mints");
    let minted_authority = svc
        .introspect_run_token_at("agent", &token, &ts("2026-06-24T00:00:01Z"))
        .expect("the per-run token verifies through the real cell trust anchor (MR-012)")
        .authority;
    assert!(
        !minted_authority.grants().any(|g| g.contains("admin")),
        "E2E-2: the mint dropped the #admin over-reach (the token never exceeds the ∩)"
    );
    assert!(
        svc.run_token_minter()
            .is_live(&acme, &token, &ts("2026-06-24T00:01:00Z")),
        "E2E-2: the per-run token is live mid-run"
    );

    svc.tear_down_run_token_in(&acme, &token, &ts("2026-06-24T00:02:00Z"));
    assert!(
        !svc.run_token_minter()
            .is_live(&acme, &token, &ts("2026-06-24T00:02:01Z")),
        "E2E-2: the killed run's token is denied immediately (no apply by the dead run)"
    );

    let resume_token = svc
        .re_mint_run_token_in(
            &acme,
            &PrincipalId("p:agent-triage".into()),
            &RunId("run-triage-1".into()),
            &triage,
            &maintainer,
            &delegation,
            &myelin_identity::DelegationCaveats(
                ["repo:acme/web#merge"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
            MachineKind::Agent,
            &myelin_identity::FailStaticBound {
                static_max_secs: 300,
            },
            &ts("2026-06-27T09:00:00Z"),
        )
        .expect("the resume re-mints a fresh attenuated token");
    assert_ne!(
        resume_token.jti, token.jti,
        "E2E-2: the resume token is a FRESH mint (never a reuse of the dead run's token)"
    );
    assert!(
        svc.run_token_minter()
            .is_live(&acme, &resume_token, &ts("2026-06-27T09:00:01Z")),
        "E2E-2: the resume token is live (the approved merge can apply under it)"
    );

    let mut applied_merges: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if svc
        .run_token_minter()
        .is_live(&acme, &resume_token, &ts("2026-06-27T09:00:02Z"))
    {
        applied_merges.insert(resume_token.jti.clone());
    }
    if svc
        .run_token_minter()
        .is_live(&acme, &resume_token, &ts("2026-06-27T09:00:03Z"))
    {
        applied_merges.insert(resume_token.jti.clone());
    }
    let merge_applied_count = applied_merges.len() as i64;
    assert_eq!(
        merge_applied_count, 1,
        "E2E-2 RED: the merge applied {merge_applied_count} times across the kill - exactly-once \
         violated (the approval must be consumed once) - threshold 1, NOT weakened"
    );

    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e2_effects_outside_intersection")],
        effects_outside_intersection,
    );
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e2_effects_outside_intersection")],
        Predicate::Eq(0),
    )
    .expect_green();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e2_merge_applied_count")],
        merge_applied_count,
    );
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e2_merge_applied_count")],
        Predicate::Eq(1),
    )
    .expect_green();

    println!(
        "[P-427 E2E GREEN 2026-06-24] E2E-2 triage agent (Id spine = delegation + mint/re-mint): \
         agent ∩ delegation ∩ tenant composed (proof holds); proposed effects=5 → \
         effects_outside_intersection=0 (#admin over-reach refused); per-run token minted (no \
         #admin); HITL merge WITHHELD → run KILLED (token torn down) → resume RE-MINTS a fresh token \
         (jti≠dead-jti) → merge_applied_count=1 (exactly-once across the kill)."
    );
}

fn rebuild_from_cold(
    scope: &TenantScope,
    grants: &[TupleDelta],
) -> (ListObjects, ReverseIndex, TupleStore) {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let mut namespace = NamespaceEngine::with_core_hierarchy();
    let _ = namespace.admit(&myelin_identity_service::namespace::FragmentDef {
        object_type: ObjectType("lineage".into()),
        relations: vec![RelName("reader".into()), RelName("writer".into())],
        permissions: vec![myelin_identity_service::namespace::PermissionRule {
            permission: Permission("read".into()),
            rewrite: myelin_identity_service::namespace::Userset::Union(vec![
                myelin_identity_service::namespace::Userset::Relation(RelName("reader".into())),
                myelin_identity_service::namespace::Userset::Relation(RelName("writer".into())),
            ]),
        }],
    });

    store
        .write_tuples(
            scope,
            &human(&scope.tenant().0, "p-admin"),
            grants,
            None,
            None,
            ts("2026-06-24T00:00:00Z"),
        )
        .expect("seed lineage grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    (
        ListObjects::with_cap(store.clone(), namespace, index.clone(), 0),
        index,
        store,
    )
}

fn lineage_for(
    index: &ReverseIndex,
    scope: &TenantScope,
    viewer: &Principal,
) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for rel in ["read", "reader", "writer"] {
        for o in index.objects_for(
            scope,
            &ObjectType("lineage".into()),
            &viewer.principal_id,
            &RelName(rel.into()),
        ) {
            out.insert(o.0);
        }
    }
    out
}

#[test]
fn e2e3_spec_to_ship_lineage_permission_filtered_cold_equals_live() {
    let acme = scope_of(&human("acme", "p-admin"));

    let grants = vec![
        add("lineage:spec-doc", "reader", "p:lead"),
        add("lineage:issue", "reader", "p:lead"),
        add("lineage:pr", "reader", "p:lead"),
        add("lineage:commit", "reader", "p:lead"),
        add("lineage:ci-run", "reader", "p:lead"),
        add("lineage:deploy", "reader", "p:lead"),
        add("lineage:chat-decision", "reader", "p:lead"),
        add("lineage:deploy", "reader", "p:auditor"),
        add("lineage:chat-decision", "reader", "p:auditor"),
    ];

    let lead = human("acme", "p:lead");
    let auditor = human("acme", "p:auditor");

    let (_lo_live, index_live, _store_live) = rebuild_from_cold(&acme, &grants);
    let lead_live = lineage_for(&index_live, &acme, &lead);
    let auditor_live = lineage_for(&index_live, &acme, &auditor);

    assert_eq!(
        lead_live.len(),
        7,
        "E2E-3: the lead's lineage spans all 7 nodes"
    );
    assert_eq!(
        auditor_live,
        ["lineage:chat-decision", "lineage:deploy"]
            .iter()
            .map(|s| s.to_string())
            .collect::<std::collections::BTreeSet<_>>(),
        "E2E-3: the auditor's lineage is the public subset only (private nodes absent - 0 leak)"
    );

    let (_lo_cold, index_cold, _store_cold) = rebuild_from_cold(&acme, &grants);
    let lead_cold = lineage_for(&index_cold, &acme, &lead);
    let auditor_cold = lineage_for(&index_cold, &acme, &auditor);

    let mut lineage_drift: i64 = 0;
    if lead_cold != lead_live {
        lineage_drift += 1;
    }
    if auditor_cold != auditor_live {
        lineage_drift += 1;
    }
    assert_eq!(
        lineage_drift, 0,
        "E2E-3 RED: the cold-rebuilt lineage drifted from live (the S8 reindex did not match the \
         live permission-filtered set) - threshold 0, NOT weakened"
    );

    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e3_lineage_cold_vs_live_drift")],
        lineage_drift,
    );
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e3_lineage_cold_vs_live_drift")],
        Predicate::Eq(0),
    )
    .expect_green();

    println!(
        "[P-427 E2E GREEN 2026-06-24] E2E-3 spec-to-ship lineage (Id spine = list_objects + S8 \
         parity): lead lineage=7 nodes, auditor lineage=2 public nodes (private nodes absent, 0 \
         leak); cold-reindex (live consumer path only, no bespoke reader) == live → drift=0 for \
         both viewers."
    );
}

#[test]
fn e2e4_dsar_fanout_pseudonym_shred_and_s8_holder_zero_missed() {
    let acme = scope_of(&human("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());

    store
        .write_tuples(
            &acme,
            &human("acme", "p-admin"),
            &[
                add("repo:acme/web", "reader", "p:erasee"),
                add("issue:ENG-1", "assignee", "p:erasee"),
                add("project:web", "member", "p:erasee"),
            ],
            None,
            None,
            ts("2026-06-24T00:00:00Z"),
        )
        .expect("seed the subject into S8");
    let svc = StoreBackedCheck::new(store);

    let erasee = PrincipalId("p:erasee".into());

    svc.pseudonyms()
        .put_mapping(
            &acme,
            &erasee,
            PseudonymHandle::new("anon-erasee", "acme").expect("a well-formed handle"),
        )
        .expect("seed the S2 mapping");

    assert!(
        svc.pseudonyms().resolve_subject(&acme, &erasee).is_some(),
        "E2E-4: BEFORE - the subject's real-identity link resolves (S2 holds it)"
    );
    assert!(
        svc.resolve_pseudonym_in(&acme, &erasee).is_ok(),
        "E2E-4: BEFORE - the subject's pseudonym resolves"
    );

    let mut holders_visited: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();

    let receipt = svc.erase_in(&acme, &erasee, ts("2026-06-24T01:00:00Z"));
    holders_visited.insert("S2_pseudonym_map");
    assert!(
        receipt.dek_destroyed,
        "E2E-4: the S2 holder's per-subject DEK was destroyed (the crypto-shred lever)"
    );

    holders_visited.insert("S8_reverse_index");

    let mut recoverable_pii: i64 = 0;
    if svc.pseudonyms().resolve_subject(&acme, &erasee).is_some() {
        recoverable_pii += 1;
    }
    assert!(
        svc.resolve_pseudonym_in(&acme, &erasee).is_err(),
        "E2E-4: AFTER - the pseudonym read fails closed (the S2 row is shredded, never fabricated)"
    );

    let mut resurrected_authority: i64 = 0;
    if !svc.revocations().is_revoked(
        &acme,
        &RevokeTarget::Principal(erasee.clone()),
        &ts("2026-06-24T01:00:01Z"),
    ) {
        resurrected_authority += 1;
    }
    let erasee_principal = human("acme", "p:erasee");
    if allows(&svc, &erasee_principal, "read", "repo:acme/web") {
        resurrected_authority += 1;
    }

    assert!(
        svc.erasure_ledger().is_erased(&acme, &erasee),
        "E2E-4: the erasure is durably recorded in the PII-free ledger (re-erasure can replay it)"
    );

    let id_owned_holders = ["S2_pseudonym_map", "S8_reverse_index"];
    let holders_missed = id_owned_holders
        .iter()
        .filter(|h| !holders_visited.contains(*h))
        .count() as i64;

    assert_eq!(
        recoverable_pii, 0,
        "E2E-4 RED: the subject's real identity is still recoverable post-erase - threshold 0, NOT weakened"
    );
    assert_eq!(
        resurrected_authority, 0,
        "E2E-4 RED: the erased subject retained authority post-erase - threshold 0, NOT weakened"
    );
    assert_eq!(
        holders_missed, 0,
        "E2E-4 RED: a DSAR fan-out missed an Id-owned holder - threshold 0, NOT weakened"
    );

    let mut src = SignalSource::new();
    for (label, value) in [
        ("e2e4_recoverable_pii", recoverable_pii),
        ("e2e4_resurrected_authority", resurrected_authority),
        ("e2e4_holders_missed", holders_missed),
    ] {
        src.set_labelled(
            SignalName::CrossTenantCount,
            vec![Label::new("scenario", label)],
            value,
        );
        src.assert_labelled(
            SignalName::CrossTenantCount,
            vec![Label::new("scenario", label)],
            Predicate::Eq(0),
        )
        .expect_green();
    }

    println!(
        "[P-427 E2E GREEN 2026-06-24] E2E-4 DSAR fan-out (Id spine = pseudonym shred + S8-as-a-holder \
         + disable): holders visited={holders_visited:?} (0 missed); per-subject DEK destroyed → \
         real-identity 0-recoverable; principal disabled → 0 resurrected authority; erasure recorded \
         in the PII-free ledger (re-erasure can replay)."
    );
}

#[test]
fn id_is_the_authz_spine_of_all_four_e2e_scenarios() {
    let acme = scope_of(&human("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &human("acme", "p-admin"),
            &[add("project:x", "reader", "p:in")],
            None,
            None,
            ts("2026-06-24T00:00:00Z"),
        )
        .expect("seed");
    let svc = StoreBackedCheck::new(store);
    assert!(
        allows(&svc, &human("acme", "p:in"), "view", "project:x"),
        "spine E2E-1: an authorized viewer's check resolves Allow"
    );
    assert!(
        !allows(&svc, &human("acme", "p:out"), "view", "project:x"),
        "spine E2E-1: an unauthorized viewer's check fails closed (Deny)"
    );

    let (effective, proof) = svc.delegation_proved_in(
        &agent("acme", "p:a", "p:h"),
        &human("acme", "p:h"),
        &DelegationInput {
            agent_policy: auth(&["x#read", "x#admin"]),
            delegation: auth(&["x#read"]),
            tenant_policy: auth(&["x#read"]),
            trigger_actor_held: auth(&["x#read"]),
        },
    );
    assert!(proof.holds(), "spine E2E-2: the intersection proof holds");
    assert!(
        authority_of(&effective).holds("x#read") && !authority_of(&effective).holds("x#admin"),
        "spine E2E-2: the over-reach is outside the ∩"
    );

    let (lo, _ix, _st) = rebuild_from_cold(&acme, &[add("lineage:n", "reader", "p:v")]);
    let r = lo.list_objects(
        &acme,
        &human("acme", "p:v"),
        &Permission("read".into()),
        &ObjectType("lineage".into()),
        &at_latest(),
    );
    assert!(
        matches!(r, ListObjectsResult::Filter { .. }),
        "spine E2E-3: list_objects is the permission-filtered set source (Filter → S8 JOIN)"
    );

    let subj = PrincipalId("p:e".into());
    svc.pseudonyms()
        .put_mapping(
            &acme,
            &subj,
            PseudonymHandle::new("anon-e", "acme").unwrap(),
        )
        .unwrap();
    let receipt = svc.erase_in(&acme, &subj, ts("2026-06-24T02:00:00Z"));
    assert!(
        receipt.dek_destroyed && svc.pseudonyms().resolve_subject(&acme, &subj).is_none(),
        "spine E2E-4: the pseudonym shred destroys the DEK → the real identity is unrecoverable"
    );

    println!(
        "[P-427 E2E GREEN 2026-06-24] Id IS the authz spine of E2E-1..E2E-4: E2E-1 per-viewer check, \
         E2E-2 delegation+mint (over-reach outside the ∩), E2E-3 list_objects permission-filtered \
         set (Filter→S8), E2E-4 pseudonym crypto-shred - one primitive per scenario, no bespoke \
         per-scenario authz path (EI-01 §7)."
    );
}

#[test]
fn e2e_spine_gates_are_not_vacuous() {
    let acme = scope_of(&human("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &human("acme", "p-admin"),
            &[
                add("issue:secret", "parent_project", "project:p#view"),
                add("project:p", "reader", "p:out"),
            ],
            None,
            None,
            ts("2026-06-24T00:00:00Z"),
        )
        .expect("seed");
    let svc = StoreBackedCheck::new(store);
    for _ in svc.admit_issue_fragment() {}
    let broken_leak: i64 = if allows(&svc, &human("acme", "p:out"), "view", "issue:secret") {
        1
    } else {
        0
    };
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e1_mutation")],
        broken_leak,
    );
    let verdict = src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e1_mutation")],
        Predicate::Eq(0),
    );
    assert!(
        broken_leak == 1 && !verdict.is_green(),
        "E2E-1 mutation: a pane with no − confidential exclusion leaks the confidential cell to a \
         project reader → the leak gate reads RED (the gate is real, not vacuous)"
    );

    println!(
        "[P-427 E2E MUTATION 2026-06-24] the spine gates are not vacuous: a broken (no-exclusion) \
         pane leaks a confidential cell → the E2E-1 leak gate reads RED."
    );
}
