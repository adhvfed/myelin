//! # P-ID-26 (global P-249) GATE / DRILL — KN-D5 / KN-D13, the Knowledge no-leak-incl-COUNT +
//! cross-tenant guard (dated green artifacts)
//!
//! **Drill catalogue rows KN-D5 / KN-D13 (F1/F2):** *A confidential page / row / field is ABSENT from
//! any `list_objects` / search result for an unauthorized viewer **INCLUDING the COUNT** (no count
//! leak), and cross-tenant access is 0.* Survival signals: **0 leaked pages/rows/fields incl. COUNT;
//! 0 cross-tenant** (the `CrossTenantCount == 0` survival signal from the contract-1.8 set, exactly as
//! ID-D3 / GIT-D8). `myelin-harness` is a DEV-dependency only — it never enters the identity-service
//! production DAG.
//!
//! **The scenarios (all over Id's compiled Knowledge fragment, P-ID-26):**
//!
//! 1. **Page (the page-tree OVERRIDE).** A sub-page inherits read from its parent, but a `- direct_block`
//!    override removes the viewer. `list_objects(viewer, read, page)` must NOT contain the blocked
//!    sub-page — and the COUNT must drop by exactly one (the blocked page is absent from the count, not
//!    merely the body — the no-COUNT-leak guard; a post-filter that hid the row but kept the count
//!    would FAIL this).
//! 2. **Row (the row-level ACL).** A confidential `database_row` granted to someone else is ABSENT from
//!    the viewer's `list_objects(viewer, read, database_row)` — body AND count.
//! 3. **Field (the off-hot-path caveat, §8.6).** A redacted column is `Deny` for the under-cleared
//!    viewer — it is absent from the field projection AND any field count (a redacted column never
//!    contributes to a COUNT).
//! 4. **Cross-tenant.** A confidential page in `acme` is unreachable by an `evil-corp` viewer:
//!    `CrossTenantCount == 0`.
//!
//! A leak (a blocked page in the body OR an unchanged count) aborts LOUDLY — the threshold is NEVER
//! weakened to pass (EI-01 §3).

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

/// Build a `StoreBackedCheck` over the core hierarchy + Id's compiled Knowledge fragment, seeded with
/// `grants` and a LIVE S8 reverse index fed off the bus (so `list_objects` materialises the real
/// candidate set, not a stub).
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

/// **KN-D5 / KN-D13 (F1) — a blocked sub-page is absent from `list_objects` INCLUDING the COUNT.**
///
/// `page:secret` inherits read from `page:home` (the viewer reads home) but a `- direct_block` override
/// removes the viewer from `page:secret`. The viewer's `list_objects(read, page)` must contain
/// `page:home` and `page:open` but NOT `page:secret` — and the COUNT must be exactly the visible
/// count (the blocked page contributes 0 to the count; a post-filter that kept the count would fail).
#[test]
fn kn_d5_blocked_page_absent_from_list_incl_count() {
    let s = scope_of(&principal("acme", "p-admin"));
    let svc = provider(
        &s,
        &[
            // The viewer reads page:home directly.
            add("page:home", "direct_reader", "p:viewer"),
            // page:open also readable directly.
            add("page:open", "direct_reader", "p:viewer"),
            // page:secret INHERITS read from page:home (parent_page->read)...
            add("page:secret", "parent_page", "page:home#read"),
            // ...but the OVERRIDE blocks the viewer on the sub-page (- direct_block).
            add("page:secret", "direct_block", "p:viewer"),
        ],
    );
    let viewer = principal("acme", "p:viewer");

    // Sanity: the viewer reads home + open, NOT secret (the override narrows the inherited access).
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

    // THE NO-COUNT-LEAK GUARD: list_objects(read, page) returns the visible pages — and the COUNT
    // (ids.len()) is exactly the visible count. The blocked sub-page is absent from BODY and COUNT.
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
        "no COUNT leak: exactly 2 pages in the count (home + open) — the blocked page contributes 0"
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

/// **KN-D5 / KN-D13 (F1) — a confidential row is absent from the row-ACL `list_objects` incl. COUNT.**
///
/// The viewer reads two rows; a confidential `database_row` granted to someone else is ABSENT from the
/// viewer's `list_objects(read, database_row)` — body and count.
#[test]
fn kn_d13_confidential_row_absent_from_list_incl_count() {
    let s = scope_of(&principal("acme", "p-admin"));
    let svc = provider(
        &s,
        &[
            add("database_row:r1", "direct_reader", "p:viewer"),
            add("database_row:r2", "direct_reader", "p:viewer"),
            // The confidential row — granted to someone else, never to the viewer.
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
        "no COUNT leak: exactly 2 rows in the count — the confidential row contributes 0"
    );

    println!(
        "[P-249 DRILL GREEN 2026-06-21] KN-D13 row-level ACL no-leak: tenant=acme viewer=p:viewer \
         rows=[r1,r2,r-confidential(other)] → list_objects(read, database_row) = {} ids, 0 leaked \
         rows, COUNT={} (database_row.read = direct_reader ∪ parent_page->read, db_row.id via_column \
         §7.3 — the row pre-filter is by construction, body AND count)",
        listed.len(),
        listed.len()
    );
}

/// **KN-D5 / KN-D13 (F1) — a redacted FIELD never contributes to a field count (§8.6, C3).**
///
/// The row is readable; the `salary` column is gated "visible iff clearance ≥ 3". An under-cleared
/// viewer's column is `Deny` (redacted) — so when the consumer projects the visible columns, the
/// redacted one is ABSENT from the projection AND the field count. A cleared viewer's column is Allow.
/// A missing-clearance viewer is Conditional (never a silent allow).
#[test]
fn kn_d5_redacted_field_absent_from_field_count() {
    let s = scope_of(&principal("acme", "p-admin"));
    let svc = provider(
        &s,
        &[add("database_row:emp-1", "direct_reader", "p:viewer")],
    );
    let viewer = principal("acme", "p:viewer");
    let row = ArtifactRef("database_row:emp-1".into());

    // The row is readable (the row ACL).
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

    // Model a two-column projection [name, salary]; salary is gated by the field caveat. We COUNT the
    // visible columns the viewer may project — a Deny column is absent from the count (no count leak).
    let columns = ["name", "salary"];
    let field_decision = |clearance: Option<i64>| {
        // `name` is ungated (always visible); `salary` is the gated column.
        let count_visible = |col: &str| -> Decision {
            if col != "salary" {
                return Decision::Allow; // ungated column
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

    // Under-cleared (clearance 1): salary is REDACTED → only `name` is in the field count (no leak).
    assert_eq!(
        field_decision(Some(1)),
        1,
        "an under-cleared viewer projects 1 column (name) — the redacted salary is ABSENT from the count"
    );
    // Cleared (clearance 5): both columns visible.
    assert_eq!(
        field_decision(Some(5)),
        2,
        "a cleared viewer projects both columns"
    );
    // Missing clearance → salary is Conditional (NOT Allow) → absent from the count (never a silent allow).
    assert_eq!(
        field_decision(None),
        1,
        "a missing-context field is Conditional, never silently counted (the no-silent-allow branch)"
    );

    println!(
        "[P-249 DRILL GREEN 2026-06-21] KN-D5 field-caveat no-COUNT-leak: tenant=acme viewer=p:viewer \
         row=emp-1 columns=[name,salary] caveat='salary visible iff clearance≥3' → under-cleared \
         projects 1 col (salary redacted, absent from count), cleared projects 2, missing-context \
         Conditional (never counted) — the field caveat runs on the ONE QueryAst core, off the hot \
         list_objects path (§8.6)"
    );
}

/// **KN-D5 / KN-D13 (F2) — cross-tenant page access reads 0 (`CrossTenantCount == 0`).**
///
/// A confidential page in victim tenant `acme` is read by a batch of `evil-corp` attackers (spoofing
/// the viewer's id + pointing at acme's page). Because the scope is the SUBJECT's own verified
/// `(tenant, region)` (tenant-from-token, never the path — ID-3), each attacker reads evil-corp's
/// empty partition → Deny. `CrossTenantCount == 0`.
#[test]
fn kn_d5_cross_tenant_page_access_reads_zero() {
    let mut signals = SignalSource::new();

    // The victim tenant `acme`: alice reads page:home.
    let acme = scope_of(&principal("acme", "p-admin"));
    let svc = provider(&acme, &[add("page:home", "direct_reader", "p:alice")]);
    let page = ArtifactRef("page:home".into());

    // Sanity: the legitimate acme reader DOES read (within acme's partition).
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

    // THE ATTACK: a batch of evil-corp attackers spoof alice's id AND point at acme's page — but the
    // VERIFIED tenant is evil-corp, so acme's partition is unreachable.
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
         direct_block) → CrossTenantCount=0 (tenant-from-token, never the URL path — no cross-tenant \
         query path, identity §6 / ID-3)"
    );
}
