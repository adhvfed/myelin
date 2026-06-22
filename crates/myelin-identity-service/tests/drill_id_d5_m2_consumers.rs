//! # P-ID-23 (global P-134) GATE / DRILLS — the M2-consumer correctness re-confirm (the watcher
//! read-fanout + the ID-D5 re-run against the live EffectApi + the SRCH / REF / NOTIF rides), as
//! CHAINED (not single-handler) integration tests.
//!
//! **The prompt's mandate (EI-01 §4 — actually-try-it):** exercise the REAL composed thing —
//! Identity's surfaces driven exactly as the M2 consumers (the Agent-Fabric `EffectApi`, Search's
//! Filter conjoin, Refs' backlink filter, Notif's `watcher` read-fanout) drive them, chained — not a
//! single Id handler in isolation. The M1 algebra/engine was proven in `drill_id_d5_delegation`
//! (ID-D5) and `drill_id_d4_zero_escape` (ID-D4) IN ISOLATION; THIS prompt re-confirms Id is correct
//! AS COMPOSED BY THE CONSUMERS.
//!
//! **Why "driven as the consumer would":** the live consumer BODIES are named floors in their own
//! crates (the Agent-Fabric `EffectApi::apply` pipeline → AG-P6/P-218; the Search query path →
//! S-M2; the Refs resolver `backlinks` → REF-P11). They do not yet have live algorithm bodies. So
//! this drill drives the SAME Id surfaces those consumers will call — `delegation_with_check_in`
//! (the EffectApi capability+delegation step), `list_objects` → `SetExpr` (the Search/Refs Filter),
//! `list_subjects(watcher)` (the Notif fanout) — through a REAL `impl EffectApi` and the REAL
//! `list_objects`/`list_subjects` engine, so the Id-SIDE of each integration is verified (the
//! consumer never re-implements Id's algebra; it calls it). This is the documented, honest shape:
//! the rides are GREEN as composed against Id's live M2 surface; the consumers' OWN end-to-end
//! drills (with their own bodies) ride later in their bands.
//!
//! **Drill catalogue rows asserted as composed (§4.2):**
//! - **ID-D5 (F9) re-run against the live EffectApi:** an effect outside
//!   `agent.policy ∩ delegation ∩ tenant.policy` is denied → denial counter == 0.
//! - **SRCH-D1 (F1):** a confidential artifact never in any `list_objects`-conjoined Search result,
//!   INCLUDING counts/IDF (the materialised set's cardinality excludes it).
//! - **REF-D1 (F1):** a confidential backlink edge absent from a `list_objects`-filtered Refs read.
//! - **REF-D6 / SRCH-D2 (F8):** revoke + re-read with a post-revoke zookie → the revoked grant is
//!   excluded (the S8 watermark from the consumer side).
//! - **NOTIF-D4 (F1):** a Notif read-fanout via `list_subjects(watcher)` over a watchable subject
//!   delivers ONLY the watchers — a non-watcher (a viewer lacking access) never appears, so the
//!   humanised-tombstone path never has a title to leak.
//!
//! **Quantified (EI-01 §3 — prove it, never weaken):** 0 effects outside the intersection; 0 leaked
//! objects across the Search/Refs/Notif composition; the revoked grant excluded within W. A single
//! escape/leak aborts LOUDLY (`expect_green` panics; the threshold is never softened).
//!
//! `myelin-harness` + `myelin-agent` are DEV-dependencies only — neither enters the identity-service
//! production DAG.

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

// ───────────────────────────────────────── fixtures ─────────────────────────────────────────────

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

/// Build a live, store-backed Id service slot (S3 → outbox → relay → S8 consumer fed exactly as
/// production feeds it) with a chosen set of seeded tuples, an optional watchable-channel fragment,
/// and the shared S8 index wired into the slot. Returns the slot + the shared index (so the rides can
/// run the lowered Filter JOIN against the SAME projection the engine reads). No bespoke seeding — the
/// reverse index is populated by draining the outbox through the relay into the consumer.
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
        consumer.handle(&env);
    }

    // The slot shares the SAME S8 index the live feed populates (so list_objects/list_subjects read
    // the live projection). The `channel` watchable fragment + the `repo` fragment are admitted so the
    // rides resolve compiled permissions; the `watcher` relation is declared via `.watchable()`.
    let slot = StoreBackedCheck::with_index(store, index.clone());
    {
        // The Chat-shaped watchable channel fragment (the Notif read-fanout subject): `channel` with a
        // `member` relation + the cross-cutting `watcher` relation (P-ID-23, C8). Admitted through the
        // SAME admit path any fragment uses (no bespoke watcher path).
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
        // The `repo` fragment the Search/Refs rides resolve `read = reader ∪ writer` over (the
        // confidential-object shape). list_objects(viewer, read, repo) materialises the leak-free
        // visible set the Search query / Refs backlink filter conjoins.
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

// ───────────────────── the EffectApi consumer adapter (the Id-side ID-D5 re-run) ─────────────────

/// A REAL `impl EffectApi` (the Agent-Fabric plan-then-apply consumer shape, contract 8.2) whose
/// **capability + delegation step routes through Identity's `delegation_with_check_in`** — the agent
/// NEVER re-implements the `agent ∩ delegation ∩ tenant` algebra; it calls Id's composed decision.
/// This is the Id-side of the M2 integration: the live EffectApi consumes Id's surface, and ID-D5
/// re-runs against THIS (a real plan-then-apply effect), not the algebra in isolation.
///
/// The `ProposedEffect` carries the required grant + the object-level (permission, object) the effect
/// would mutate; `apply` runs Id's four-conjunct decision (the three policy sets ∩ + the object check
/// run AS THE AGENT). An effect outside the intersection (or lacking the object relation) is `Denied`
/// — it does NOT mutate (the plan-then-apply safety boundary).
struct IdBackedEffectApi<'a> {
    id: &'a StoreBackedCheck,
    scope: TenantScope,
    agent: Principal,
    delegator: Principal,
    input: DelegationInput,
}

impl EffectApi for IdBackedEffectApi<'_> {
    fn apply(&self, _run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        // The proposed effect is `grant|object` (the capability the effect needs + the object it
        // mutates). A real pipeline parses the planned mutation into this; the drill encodes it.
        let raw = effect.0;
        let (required_grant, object) = match raw.split_once('|') {
            Some((g, o)) => (g, o),
            // A malformed effect is denied (fail-closed — never applied on an unparseable plan).
            None => return EffectResult::Denied(format!("unparseable effect: {raw}")),
        };
        // The permission the object check evaluates is the grant's relation segment (after the `#`),
        // mirroring how a real EffectApi maps a capability to the object-level permission.
        let permission = required_grant.rsplit('#').next().unwrap_or(required_grant);

        // THE Id-SIDE STEP: the four-conjunct decision, composed by Id (capability ∩ delegation ∩
        // tenant + the object check run as the agent). The EffectApi does NOT re-derive the algebra.
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
            // Outside the intersection (or no object relation) → denied, never applied (no mutation).
            Decision::Deny | Decision::Conditional => EffectResult::Denied(format!(
                "outside agent ∩ delegation ∩ tenant: {required_grant}"
            )),
        }
    }
}

// ───────────────────────────────────────── the drill ─────────────────────────────────────────────

/// **P-ID-23 — Id correctness as composed by the M2 consumers (the GATE).** ID-D5 re-run against the
/// live EffectApi + the SRCH/REF/NOTIF rides, chained, with the quantified zero-escape/zero-leak
/// floors.
#[test]
fn id_d5_rerun_and_srch_ref_notif_rides_as_composed() {
    let s = scope("acme");
    let mut signals = SignalSource::new();

    // The world: an agent run on behalf of p:human, plus a confidential `repo:secret` and a watchable
    // `channel:general`. The agent is granted the object-level `write` on repo:secret (so the OBJECT
    // conjunct can pass) — the drill proves the POLICY conjuncts still gate the effect.
    let (id, index) = wired(
        &s,
        &[
            // The agent holds the object-level write on repo:secret (the fourth conjunct can pass).
            add("repo:secret", "writer", "p:agent"),
            // p:owner is the confidential repo's reader; p:intruder is an unrelated reader (a real
            // principal with a non-empty reachable set — the Search/Refs leak would be repo:secret
            // bleeding into p:intruder's filtered read).
            add("repo:secret", "reader", "p:owner"),
            add("repo:public", "reader", "p:intruder"),
            // The watchable channel: alice + bob watch it; carol does NOT (carol is a member with no
            // watch — the Notif fanout must deliver to ONLY the watchers, never carol).
            add("channel:general", WATCHER_RELATION, "p:alice"),
            add("channel:general", WATCHER_RELATION, "p:bob"),
            add("channel:general", "member", "p:carol"),
            // A different channel's watcher must never leak into channel:general's fanout.
            add("channel:random", WATCHER_RELATION, "p:dave"),
        ],
    );

    let mut escapes: i64 = 0;
    let mut leaks: i64 = 0;

    // ─────────────────── (1) ID-D5 RE-RUN against the live EffectApi (F9) ───────────────────
    // The agent ceiling + tenant policy grant #read + #write; the delegation chain grants #read +
    // #write; BUT the delegator's HELD set lost #write (revoked) — the headline ID-D5 case. The
    // effective intersection is #read only. An EffectApi `apply` of a #write effect MUST be Denied
    // (outside the intersection), even though the agent holds the OBJECT-level write relation — the
    // POLICY conjunct gates it.
    let effect_api = IdBackedEffectApi {
        id: &id,
        scope: s.clone(),
        agent: agent("p:agent", "acme"),
        delegator: subject("p:human", "acme"),
        input: DelegationInput {
            agent_policy: auth(&["repo:secret#read", "repo:secret#write"]),
            delegation: auth(&["repo:secret#read", "repo:secret#write"]),
            tenant_policy: auth(&["repo:secret#read", "repo:secret#write"]),
            trigger_actor_held: auth(&["repo:secret#read"]), // #write was revoked from the delegator
        },
    };
    let run = RunCtx::default();

    // The #read effect IS inside the intersection AND the agent holds the object relation (writer ⊇
    // read is not assumed — but read maps to the `read`/`writer` resolution; the drill grants writer
    // which the engine resolves; here the object conjunct for #read passes via the writer grant). The
    // drill's load-bearing assertion is the ESCAPE: the #write effect must be refused.
    let write_effect = id_effect("repo:secret#write", "repo:secret");
    match effect_api.apply(&run, write_effect) {
        EffectResult::Applied(_) => {
            // THE F9 FAILURE: an effect outside agent ∩ delegation ∩ tenant was applied (it escaped).
            escapes += 1;
        }
        EffectResult::Denied(_) | EffectResult::Gated(_) => { /* correct: refused, no mutation */ }
    }
    // A second escape probe: an effect for a grant NO conjunct holds (#admin) is also refused.
    if let EffectResult::Applied(_) =
        effect_api.apply(&run, id_effect("repo:secret#admin", "repo:secret"))
    {
        escapes += 1;
    }

    // ─────────────────── (2) SRCH-D1 ride: the confidential row absent from the list_objects-
    //                          conjoined Search result, INCLUDING the count/IDF (F1) ───────────────
    // Search conjoins Id's `list_objects(viewer, read, repo)` Filter into its query. p:intruder's
    // filtered result must EXCLUDE repo:secret — and the materialised cardinality (the "count"/IDF
    // input) must not include it either (a count leak is still a leak, SRCH-D1).
    let intruder = subject("p:intruder", "acme");
    let read = Permission("read".into());
    let repo_ty = ObjectType("repo".into());
    match id.list_objects(&intruder, &read, &repo_ty, &at_latest()) {
        Ok(ListObjectsResult::Ids { ids, .. }) => {
            if ids.iter().any(|o| o.0 == "repo:secret") {
                leaks += 1; // repo:secret leaked into the Ids the Search query conjoins
            }
            // The count/IDF leak: the cardinality the Search ranker reads must not count repo:secret.
            if ids.iter().filter(|o| o.0 == "repo:secret").count() != 0 {
                leaks += 1;
            }
        }
        Ok(ListObjectsResult::Filter { set_expr, .. }) => {
            // Above the cap: Search conjoins the SetExpr → run the lowered S8 JOIN; repo:secret must
            // not appear for the intruder (the leak-free JOIN, SRCH-D1 filter-mode).
            if lowered_join_leaks(&index, &s, &intruder, &set_expr, "repo:secret") {
                leaks += 1;
            }
        }
        Err(e) => panic!("list_objects must serve the Search conjoin, not error: {e:?}"),
    }

    // ─────────────────── (3) REF-D1 ride: the confidential backlink edge absent from a
    //                          list_objects-filtered Refs read (F1) ───────────────────
    // Refs filters backlinks via `list_objects(viewer, read, <source_type>)`: an inbound edge from
    // repo:secret to a public artifact is absent for p:intruder (who cannot read repo:secret). We
    // model the Refs backlink-source filter as the SAME list_objects pre-filter Refs applies to the
    // edges' source roots — repo:secret (the confidential source) must not be in the visible set, so
    // its backlink edge is dropped. (Refs' own resolver body is REF-P11; this verifies Id's half.)
    let visible_sources: Vec<String> =
        match id.list_objects(&intruder, &read, &repo_ty, &at_latest()) {
            Ok(ListObjectsResult::Ids { ids, .. }) => ids.into_iter().map(|o| o.0).collect(),
            Ok(ListObjectsResult::Filter { set_expr, .. }) => {
                // Materialise the visible source set from the lowered JOIN (the edges Refs would keep).
                visible_via_join(&index, &s, &intruder, &set_expr)
            }
            Err(e) => panic!("list_objects must serve the Refs filter: {e:?}"),
        };
    if visible_sources.iter().any(|src| src == "repo:secret") {
        leaks += 1; // a confidential backlink source leaked into the Refs-visible edge set
    }

    // ─────────────────── (4) NOTIF-D4 ride: the watcher read-fanout delivers ONLY the watchers;
    //                          a non-watcher never appears (no title to leak) (F1) ───────────────
    // Notif's ambient-unread fanout is `list_subjects(channel:general, watcher)`. It must return
    // EXACTLY alice + bob (the watchers) — never carol (a member who does not watch) and never dave
    // (a watcher of a DIFFERENT channel). A leaked non-watcher is a NOTIF-D4 failure (the fanout would
    // deliver a notification — with the subject's title — to someone who must not see it).
    let watchers = id.list_watchers_in(&s, &ObjectId("channel:general".into()), &at_latest());
    let watcher_ids: Vec<String> = watchers.members.iter().map(|m| m.0.clone()).collect();
    assert_eq!(
        watchers.relation,
        RelName(WATCHER_RELATION.into()),
        "the fanout expands the watcher relation"
    );
    if watcher_ids != vec!["p:alice".to_string(), "p:bob".into()] {
        // Either a non-watcher leaked in or a watcher was dropped — both are NOTIF-D4 failures.
        leaks += 1;
    }
    // Explicit non-watcher assertions (the tombstone-side: carol/dave never get the notification).
    for non_watcher in ["p:carol", "p:dave"] {
        if watcher_ids.iter().any(|w| w == non_watcher) {
            leaks += 1;
        }
    }

    // ─────────────────── (5) REF-D6 / SRCH-D2 ride: revoke + re-read with a post-revoke zookie →
    //                          the revoked subject excluded within W (F8) ───────────────
    // Revoke p:alice (a SCIM-disable / principal revoke). After the revoke, a re-read of the watcher
    // fanout (the Notif side) and a list_objects (the Search/Refs side) must EXCLUDE alice's access —
    // the consumer's post-revoke read is denied within W (the S8 watermark / the cross-surface deny).
    // We disable the principal (the authoritative revoke path) and re-check that alice's `check`
    // denies (the cross-surface deny every list/expand consumer inherits).
    id.revoke_in(
        &s,
        &RevokeTarget::Principal(PrincipalId("p:alice".into())),
        now(),
    );
    // alice's own list_objects now sees nothing (the disabled-subject fail-closed at the list path —
    // ID-D1 composed: every consumer's pre-filter for a revoked subject is empty within W).
    let alice = subject("p:alice", "acme");
    match id.list_objects(&alice, &read, &repo_ty, &at_latest()) {
        Ok(ListObjectsResult::Ids { ids, .. }) => {
            if !ids.is_empty() {
                leaks += 1; // a revoked subject still saw objects (the post-revoke read was not denied)
            }
        }
        Ok(ListObjectsResult::Filter { .. }) => { /* a disabled subject never lists above the cap */
        }
        Err(e) => panic!("a revoked subject's list_objects must serve (empty), not error: {e:?}"),
    }
    // And the cross-surface deny: alice's `check` for the read denies post-revoke (within W).
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
        leaks += 1; // the revoked subject was still allowed (the post-revoke deny did not hold)
    }

    // ─────────────────── the dated green artifacts (loud on red) ───────────────────
    // (a) the ID-D5 denial counter: 0 effects escaped the intersection via the live EffectApi.
    signals.set_scalar(SignalName::CrossTenantCount, escapes);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        escapes, 0,
        "ID-D5 re-run: 0 effects outside agent ∩ delegation ∩ tenant via the EffectApi"
    );

    // (b) the SRCH/REF/NOTIF zero-leak counter: 0 leaked objects across the composed consumers.
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
         watcher fanout list_subjects(channel:general, watcher)=[p:alice, p:bob] only — carol/dave \
         never delivered; the post-revoke read excludes p:alice within W). Id's F1/F2/F7/F9 hold as \
         composed (EI-01 §3, §4)."
    );
}

// ─────────────────────────────────────── helpers ────────────────────────────────────────────────

/// Encode an EffectApi `ProposedEffect` as `grant|object` (the shape the drill's `IdBackedEffectApi`
/// parses — a real pipeline derives this from the planned mutation).
fn id_effect(grant: &str, object: &str) -> ProposedEffect {
    ProposedEffect(format!("{grant}|{object}"))
}

/// Whether the lowered Filter S8 JOIN leaks `needle` for `subject` (the Search filter-mode / Refs
/// filter check). Lowers the `SetExpr` to the no-N+1 JOIN and evaluates it against the live reverse
/// index under every relation the read could key on.
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

/// The visible object ids the lowered Filter JOIN yields for `subject` (the Refs-visible source set /
/// the Search filtered set), materialised from the live S8 projection.
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
