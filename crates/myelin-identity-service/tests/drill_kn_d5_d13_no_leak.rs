use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ListObjectsResult, Literal, ObjectId,
    ObjectType, Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple,
    TupleDelta, Zookie,
};
use myelin_identity_service::{
    knowledge_fragment, ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

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

fn now() -> Timestamp {
    Timestamp("2026-06-21T00:00:00Z".into())
}

fn provider(scope: &TenantScope, grants: &[TupleDelta]) -> StoreBackedCheck {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());
    store
        .write_tuples(
            scope,
            &principal(&scope.tenant().0, "p-admin"),
            grants,
            None,
            None,
            now(),
        )
        .expect("seed grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    let svc = StoreBackedCheck::with_index(store, index);
    for admit in svc.admit_knowledge_fragment() {
        assert!(
            matches!(admit, myelin_identity::FragmentAdmit::Admitted { .. }),
            "Id's compiled Knowledge fragment admits"
        );
    }
    svc
}

fn ids_of(result: ListObjectsResult) -> Vec<ObjectId> {
    match result {
        ListObjectsResult::Ids { ids, .. } => ids,
        ListObjectsResult::Filter { .. } => {
            panic!("the small visible set must materialise as Ids for the COUNT assertion")
        }
    }
}

#[test]
fn kn_d5_blocked_page_absent_from_list_incl_count() {
    let s = scope_of(&principal("acme", "p-admin"));
    let svc = provider(
        &s,
        &[
            add("page:home", "direct_reader", "p:viewer"),
            add("page:open", "direct_reader", "p:viewer"),
            add("page:secret", "parent_page", "page:home#read"),
            add("page:secret", "direct_block", "p:viewer"),
        ],
    );
    let viewer = principal("acme", "p:viewer");

    let can = |obj: &str| {
        matches!(
            svc.check(
                &viewer,
                &Permission("read".into()),
                &ArtifactRef(obj.into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can("page:home"),
        "the viewer reads page:home (direct_reader)"
    );
    assert!(
        can("page:open"),
        "the viewer reads page:open (direct_reader)"
    );
    assert!(
        !can("page:secret"),
        "the - direct_block override narrows inherited access: the viewer does NOT read page:secret"
    );

    let listed = ids_of(
        svc.list_objects(
            &viewer,
            &Permission("read".into()),
            &ObjectType("page".into()),
            &at_latest(),
        )
        .expect("list pages"),
    );
    let leaked_pages = listed.iter().filter(|o| o.0 == "page:secret").count();
    assert_eq!(
        leaked_pages, 0,
        "0 leaked pages: page:secret is ABSENT from the viewer's list (KN-D5)"
    );
    assert_eq!(
        listed.len(),
        2,
        "no COUNT leak: exactly 2 pages in the count (home + open) - the blocked page contributes 0"
    );

    println!(
        "[P-249 DRILL GREEN 2026-06-21] KN-D5 page-tree override no-leak: tenant=acme \
         viewer=p:viewer pages=[home,open,secret(blocked)] → list_objects(read, page) = {} ids \
         (home+open), 0 leaked pages, COUNT={} (the - direct_block override removes page:secret from \
         BODY and COUNT by construction, no post-filter)",
        listed.len(),
        listed.len()
    );
}

#[test]
fn kn_d13_confidential_row_absent_from_list_incl_count() {
    let s = scope_of(&principal("acme", "p-admin"));
    let svc = provider(
        &s,
        &[
            add("database_row:r1", "direct_reader", "p:viewer"),
            add("database_row:r2", "direct_reader", "p:viewer"),
            add("database_row:r-confidential", "direct_reader", "p:other"),
        ],
    );
    let viewer = principal("acme", "p:viewer");
    let listed = ids_of(
        svc.list_objects(
            &viewer,
            &Permission("read".into()),
            &ObjectType("database_row".into()),
            &at_latest(),
        )
        .expect("list rows"),
    );
    let leaked_rows = listed
        .iter()
        .filter(|o| o.0 == "database_row:r-confidential")
        .count();
    assert_eq!(
        leaked_rows, 0,
        "0 leaked rows: the confidential row is ABSENT (KN-D13)"
    );
    assert_eq!(
        listed.len(),
        2,
        "no COUNT leak: exactly 2 rows in the count - the confidential row contributes 0"
    );

    println!(
        "[P-249 DRILL GREEN 2026-06-21] KN-D13 row-level ACL no-leak: tenant=acme viewer=p:viewer \
         rows=[r1,r2,r-confidential(other)] → list_objects(read, database_row) = {} ids, 0 leaked \
         rows, COUNT={} (database_row.read = direct_reader ∪ parent_page->read, db_row.id via_column \
         §7.3 - the row pre-filter is by construction, body AND count)",
        listed.len(),
        listed.len()
    );
}

#[test]
fn kn_d5_redacted_field_absent_from_field_count() {
    let s = scope_of(&principal("acme", "p-admin"));
    let svc = provider(
        &s,
        &[add("database_row:emp-1", "direct_reader", "p:viewer")],
    );
    let viewer = principal("acme", "p:viewer");
    let row = ArtifactRef("database_row:emp-1".into());

    assert_eq!(
        svc.check(
            &viewer,
            &Permission("read".into()),
            &row,
            &at_latest(),
            None
        ),
        Ok(Decision::Allow),
        "the viewer reads the row; the field caveat gates a column on top (§8.6)"
    );

    let columns = ["name", "salary"];
    let field_decision = |clearance: Option<i64>| {
        let count_visible = |col: &str| -> Decision {
            if col != "salary" {
                return Decision::Allow;
            }
            let ctx: Vec<(&str, Literal)> = match clearance {
                Some(c) => vec![("clearance", Literal::Int(c))],
                None => vec![],
            };
            let cav = knowledge_fragment::field_view_caveat(
                "database_row:emp-1",
                "salary",
                "ge",
                "clearance",
                Literal::Int(3),
                &ctx,
            );
            svc.check(
                &viewer,
                &Permission("view_field".into()),
                &row,
                &at_latest(),
                Some(&cav),
            )
            .expect("field check")
        };
        columns
            .iter()
            .filter(|c| matches!(count_visible(c), Decision::Allow))
            .count()
    };

    assert_eq!(
        field_decision(Some(1)),
        1,
        "an under-cleared viewer projects 1 column (name) - the redacted salary is ABSENT from the count"
    );
    assert_eq!(
        field_decision(Some(5)),
        2,
        "a cleared viewer projects both columns"
    );
    assert_eq!(
        field_decision(None),
        1,
        "a missing-context field is Conditional, never silently counted (the no-silent-allow branch)"
    );

    println!(
        "[P-249 DRILL GREEN 2026-06-21] KN-D5 field-caveat no-COUNT-leak: tenant=acme viewer=p:viewer \
         row=emp-1 columns=[name,salary] caveat='salary visible iff clearance≥3' → under-cleared \
         projects 1 col (salary redacted, absent from count), cleared projects 2, missing-context \
         Conditional (never counted) - the field caveat runs on the ONE QueryAst core, off the hot \
         list_objects path (§8.6)"
    );
}

#[test]
fn kn_d5_cross_tenant_page_access_reads_zero() {
    let mut signals = SignalSource::new();

    let acme = scope_of(&principal("acme", "p-admin"));
    let svc = provider(&acme, &[add("page:home", "direct_reader", "p:alice")]);
    let page = ArtifactRef("page:home".into());

    assert_eq!(
        svc.check(
            &principal("acme", "p:alice"),
            &Permission("read".into()),
            &page,
            &at_latest(),
            None
        ),
        Ok(Decision::Allow),
        "the legitimate acme reader reads page:home (Id resolves within acme's partition)"
    );

    let mut cross_tenant_reads: i64 = 0;
    const BATCH: usize = 64;
    for i in 0..BATCH {
        let mut attacker = principal("evil-corp", &format!("p:mallory-{i}"));
        attacker.principal_id = PrincipalId("p:alice".into());
        attacker.tenant = TenantId("evil-corp".into());
        let decision = svc.check(
            &attacker,
            &Permission("read".into()),
            &page,
            &at_latest(),
            None,
        );
        if decision == Ok(Decision::Allow) {
            cross_tenant_reads += 1;
        }
    }

    signals.set_scalar(SignalName::CrossTenantCount, cross_tenant_reads);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        cross_tenant_reads, 0,
        "0 cross-tenant page reads on a spoofed token-tenant ≠ path-tenant request (KN-D5)"
    );

    println!(
        "[P-249 DRILL GREEN 2026-06-21] KN-D5 cross-tenant page access: victim=acme \
         attacker=evil-corp batch={BATCH} spoofed read attempts on page:home (Id's compiled Knowledge \
         fragment, page.read = (parent_page->read ∪ parent_space->read ∪ direct_reader) − \
         direct_block) → CrossTenantCount=0 (tenant-from-token, never the URL path - no cross-tenant \
         query path, identity §6 / ID-3)"
    );
}
