//! # `list_objects` — the return-shape dispatch + the S4 Ids materialise path (P-ID-11 → P-069)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §7.1 (the frozen return shape — `Ids | Filter` under a cardinality cap; the `Ids` materialise is
//! the S4 path, the `Filter` push-down is the S8 path), §2 (S4 — the flattened reachable-set index,
//! the `Ids` materialise; S8 — the JOIN target, the `Filter` path; both faces of the same Leopard-
//! class derivation of S3), §7.2 (the no-N+1/no-post-filter lowering — the `Filter` body, P-ID-12).
//!
//! **Contract-index:** row **4.3** (`list_objects(subject, permission, type, zookie?) → Ids | Filter`)
//! — **OWNED** here (the `Ids` path + the return-shape dispatch). The `Filter` SetExpr→SQL lowering
//! is the **P-ID-12 (P-070)** follow-on; this prompt ships the dispatch + the `Ids` materialise and
//! a `Filter` **stub** above the cap.
//!
//! ## What this module ships (P-ID-11, the list_objects half)
//! 1. **The return-shape dispatch** ([`ListObjects::list_objects`]) — given a verified
//!    `(tenant, region)` scope + `(subject, permission, type)`, materialise the reachable object set
//!    (the S4 `Ids` path) when it is **small** (at-or-under the cardinality cap), else return a
//!    `Filter` carrying the consumer-composable `SetExpr` (the S8 push-down). The decision is the
//!    **cardinality cap** (the §7.1 "default under a cardinality cap").
//! 2. **The S4 Ids materialise path** — the flattened reachable set: the object ids of `type` the
//!    `subject` may exercise `permission` on, computed by resolving the permission's rewrite over the
//!    reverse index (S8) + the namespace engine (the four-operator resolution P-ID-10 ships) and
//!    materialising the result, **leak-free** (a denied object never appears — the pre-filter is
//!    by-construction, never a post-filter; EI-01 §3 stop-the-bleeding). Returned with the read's
//!    `zookie` (the consistency token the consumer stamps).
//! 3. **The `Filter` stub** above the cap — a `SetExpr::InRelation` naming the consumer's own id
//!    column, the push-down shape the consumer's `myelin-query` compiler lowers (the full
//!    no-N+1/no-post-filter lowering against `authz_visible` is **P-ID-12 (P-070)**).
//!
//! ## The cardinality cap (a measured tunable — the SHAPE is frozen, the NUMBER is open)
//! The Ids↔Filter switch is the cardinality cap (§7.1). The default-to-beat
//! ([`DEFAULT_IDS_CARDINALITY_CAP`]) is written to `thresholds.toml` now; it is **re-measured at
//! world-scale** (the surge + cell-scale load) and finalised/dated in **P-ID-31 (P-074)** — that
//! prompt CLOSES the cardinality-cap floor. The SHAPE (Ids under the cap, Filter above) is frozen
//! here; only the threshold is open. Named, not silently assumed final.
//!
//! ## Floors (P-ID-11 named these; P-ID-12 → P-070 CLOSES the first two)
//! - **The `Filter` SetExpr→SQL lowering — CLOSED in P-ID-12 (P-070).** The
//!   [`ListObjects::lower_filter`] / [`crate::lowering::lower`] path is the real consumer-composable
//!   lowering (InRelation/TupleSet → the `authz_visible` JOIN; Union/Intersect/Difference →
//!   AND/OR/EXCEPT). The `filter_set_expr` arm below produces the `SetExpr` the lowering consumes.
//! - **The watermark consistency path (fall-back-to-check rather than serving stale) — CLOSED in
//!   P-ID-12 (P-070).** [`ListObjects::list_objects_consistent`] +
//!   [`crate::lowering::watermark_verdict`] apply the new-enemy guard.
//! - **The cardinality cap is finalised at scale in P-ID-31 (P-074).** Still OPEN (the SHAPE is
//!   frozen, the NUMBER is the measured tunable). Named above.

use crate::check_engine::CheckEngine;
use crate::namespace::NamespaceEngine;
use crate::reverse_index::ReverseIndex;
use crate::tuple_store::TupleStore;
use myelin_identity::{
    ColRef, Consistency, ListObjectsResult, ObjectId, ObjectType, Permission, Principal,
    PrincipalStatus, RelName, SetExpr, Zookie,
};
use myelin_storage::TenantScope;
use myelin_tenancy::ArtifactRef;
use std::collections::BTreeSet;

/// **The Ids↔Filter cardinality cap default-to-beat (§7.1; a measured tunable).** When the
/// materialised reachable set is at-or-under this size, `list_objects` returns `Ids` (the S4
/// materialise); above it, `Filter` (the S8 push-down). The SHAPE is frozen here; this NUMBER is the
/// **default-to-beat written to `thresholds.toml`** ([`authz_index.ids_cardinality_cap`]) and
/// **re-measured + finalised at world-scale in P-ID-31 (P-074)** — that prompt CLOSES this floor.
/// 1000 is the seed: small enough that materialising + inlining `WHERE id IN (...)` is cheaper than a
/// JOIN, large enough that ordinary lists (a user's repos/channels/boards) materialise.
pub const DEFAULT_IDS_CARDINALITY_CAP: usize = 1000;

/// **`list_objects` — the return-shape dispatch + the S4 Ids materialise (contract 4.3).**
///
/// A thin evaluator over S3 ([`TupleStore`] via [`CheckEngine`]), the compiled namespace engine
/// ([`NamespaceEngine`], P-ID-10), and the S8 reverse index ([`ReverseIndex`]). It owns no tenant
/// state; every call takes a verified `(tenant, region)` scope (the `tenant-predicate` floor — a
/// tenant-less list is unconstructable). Cloneable handle.
#[derive(Clone)]
pub struct ListObjects {
    engine: CheckEngine,
    namespace: NamespaceEngine,
    index: ReverseIndex,
    /// The Ids↔Filter cardinality cap (the measured tunable; seeded from
    /// [`DEFAULT_IDS_CARDINALITY_CAP`], overridable so P-ID-31 can wire the finalised value).
    cap: usize,
}

impl ListObjects {
    /// Wire `list_objects` over the S3 store, a compiled namespace engine (the org/team/project core
    /// + any admitted fragments), and the S8 reverse index — at the default cardinality cap.
    pub fn new(tuples: TupleStore, namespace: NamespaceEngine, index: ReverseIndex) -> ListObjects {
        ListObjects {
            engine: CheckEngine::new(tuples),
            namespace,
            index,
            cap: DEFAULT_IDS_CARDINALITY_CAP,
        }
    }

    /// Wire `list_objects` with an explicit cardinality cap (so a test exercises the Ids↔Filter
    /// switch deterministically, and P-ID-31 can install the finalised measured value).
    pub fn with_cap(
        tuples: TupleStore,
        namespace: NamespaceEngine,
        index: ReverseIndex,
        cap: usize,
    ) -> ListObjects {
        ListObjects {
            engine: CheckEngine::new(tuples),
            namespace,
            index,
            cap,
        }
    }

    /// The active cardinality cap (the measured tunable; the §7.1 Ids↔Filter switch point).
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// **`list_objects(subject, permission, type, zookie?) → Ids{ids, zookie} | Filter{set_expr,
    /// zookie}` (contract 4.3; architecture §7.1).** The return-shape dispatch:
    ///
    /// 1. A suspended/disabled subject sees nothing — `Ids{ ids: [], .. }` (the empty set, never a
    ///    permissive `All`; ID-D1 fail-closed at the list path too).
    /// 2. Materialise the reachable set (the S4 path) — the object ids of `ty` the `subject` may
    ///    exercise `permission` on, computed leak-free over S8 + the namespace engine (a denied
    ///    object never appears — the pre-filter is by-construction).
    /// 3. **Dispatch on the cardinality cap:** at-or-under the cap → `Ids{ ids, zookie }` (the S4
    ///    materialise); above the cap → `Filter{ set_expr, zookie }` (the S8 push-down — the
    ///    `SetExpr::InRelation` naming the consumer's own id column; the full lowering is P-ID-12).
    ///
    /// `at` is the consistency token; the returned `zookie` is the S8 partition watermark (the
    /// revision the read reflects). `via_column` for the `Filter` is derived from the object type
    /// (the §7.3 id-column mapping); the consumer JOINs against it.
    pub fn list_objects(
        &self,
        scope: &TenantScope,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> ListObjectsResult {
        // The read's consistency token: the S8 partition watermark (the revision the reverse index
        // reflects). The read-side guard that WAITS / falls back to per-row check when a scan needs a
        // fresher revision than the watermark is P-ID-12; here the returned zookie is the watermark.
        let zookie = self.read_zookie(scope, at);

        // (1) Fail-closed at the list path: a suspended/disabled subject sees the EMPTY set, never a
        // permissive All (ID-D1 — disabled-user → zero access, at list_objects too).
        if subject.status != PrincipalStatus::Active {
            return ListObjectsResult::Ids {
                ids: Vec::new(),
                zookie,
            };
        }

        // (2) Materialise the reachable set (the S4 path) — leak-free (a denied object never appears).
        let reachable = self.reachable_set(scope, subject, permission, ty, at);

        // (3) Dispatch on the cardinality cap (§7.1): small → Ids (materialise); large → Filter
        // (push down). The cap is the measured tunable (P-ID-31 finalises the NUMBER; the SHAPE is
        // frozen here).
        if reachable.len() <= self.cap {
            ListObjectsResult::Ids {
                ids: reachable.into_iter().map(ObjectId).collect(),
                zookie,
            }
        } else {
            ListObjectsResult::Filter {
                set_expr: self.filter_set_expr(subject, permission, ty),
                zookie,
            }
        }
    }

    /// The flattened reachable object set (the S4 materialise): the object ids of `ty` the `subject`
    /// may exercise `permission` on, at the read's snapshot. **Leak-free by construction:** an object
    /// is included IFF the permission-aware resolution grants it (the pre-filter is the grant
    /// evaluation, never a post-filter over a wider set). Bounded: only one extra candidate beyond
    /// the cap is materialised (so the dispatch decides Ids-vs-Filter without enumerating an
    /// unbounded set — the `Filter` path exists precisely so a huge set is never materialised).
    fn reachable_set(
        &self,
        scope: &TenantScope,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> BTreeSet<String> {
        // The candidate objects of this type the subject has ANY direct/reverse-indexed relation to
        // (the S8 reverse index is the candidate source — the objects the subject is reachable to,
        // keyed by `(subject, relation)` within the type partition). We then re-resolve each
        // candidate through the permission-aware engine (the namespace rewrite over the four
        // operators) so the materialised set reflects the COMPILED permission, not a raw relation —
        // a denied candidate (e.g. excluded by a `- confidential` exclusion) is dropped (leak-free).
        let candidates = self.candidate_objects(scope, subject, ty);

        let mut reachable: BTreeSet<String> = BTreeSet::new();
        for obj in candidates {
            // The permission-aware grant check on this candidate (the SAME `check`/`permits` the hot
            // path uses — one primitive, EI-01 §7). A granted candidate is in the reachable set; a
            // denied one is dropped (never a post-filter leak).
            let object_ref = ArtifactRef(obj.clone());
            let object_type = type_of_object_id(&obj);
            let granted = self.namespace.permits(
                &self.engine,
                scope,
                subject,
                &object_type,
                &permission.0,
                &object_ref,
                at,
            );
            if granted {
                reachable.insert(obj);
            }
            // Bounded materialise: once we are one past the cap, stop — the dispatch will choose
            // Filter (the set is "large"); we never enumerate an unbounded reachable set.
            if reachable.len() > self.cap {
                break;
            }
        }
        reachable
    }

    /// The candidate objects of `ty` the `subject` is reachable to (the S8 reverse-index candidate
    /// source). The reverse index is keyed `(subject, relation) → {object_id}` within the
    /// `(tenant, region, type)` partition; we union the candidates across every relation the subject
    /// has on objects of this type. Scoped — no cross-tenant read path (the reverse index reads only
    /// the verified scope's partition).
    fn candidate_objects(
        &self,
        scope: &TenantScope,
        subject: &Principal,
        ty: &ObjectType,
    ) -> BTreeSet<String> {
        // The relations a candidate could be reached through: the declared relations of this object
        // type in the namespace engine (the vocabulary). We union the reverse lookup over each
        // declared relation — the candidate set is every object the subject has ANY of those
        // relations on (direct grants in S8). The permission-aware re-resolution in `reachable_set`
        // then keeps only the granted ones (leak-free). Keyed by the id STRING (the frozen ObjectId
        // is not `Ord`) so the candidate set is deterministic.
        let mut candidates: BTreeSet<String> = BTreeSet::new();
        for relation in self.namespace.relations_of(&ty.0) {
            for obj in self.index.objects_for(
                scope,
                ty,
                &subject.principal_id,
                &RelName(relation.clone()),
            ) {
                candidates.insert(obj.0);
            }
        }
        candidates
    }

    /// The `Filter` push-down `SetExpr` (the S8 path) — a `SetExpr::InRelation` naming the consumer's
    /// own id column (§7.1/§7.2). The relation is the permission name (resolved against S8 keyed on
    /// `(subject, relation)`); the `via_column` is the §7.3 id-column for this type. **The full
    /// no-N+1/no-post-filter lowering against `authz_visible` is P-ID-12 (P-070);** this is the
    /// frozen push-down shape the consumer's compiler lowers (a stub of the lowering, not of the
    /// shape — the SHAPE is the wire contract).
    fn filter_set_expr(
        &self,
        _subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
    ) -> SetExpr {
        SetExpr::InRelation {
            relation: RelName(permission.0.clone()),
            via_column: via_column_for(ty),
        }
    }

    /// **Lower a returned `Filter` to the consumer-composable SQL + apply the S8 watermark
    /// consistency path (P-ID-12; architecture §7.2/§7.4/§8.7).**
    ///
    /// Given the `set_expr` the [`ListObjects::list_objects`] `Filter` arm returned, lower it to the
    /// `(sql_predicate, joins, params)` the consumer ANDs into its query (the no-N+1 JOIN against
    /// `authz_visible`), then decide the consistency path:
    /// - if the S8 watermark is **at-or-after** the scan's required revision (`at.at_least`) → the
    ///   JOIN serves ([`crate::lowering::WatermarkVerdict::JoinServes`]); the consumer runs the
    ///   lowered SQL against the (fresh-enough) reverse index;
    /// - if the watermark is **behind** → fall back to per-row `check` rather than serving the stale
    ///   grant (the new-enemy guard, ID-D7).
    ///
    /// Returns the [`crate::lowering::Lowered`] SQL **and** the
    /// [`crate::lowering::WatermarkVerdict`] so the consumer knows whether to run the JOIN or take
    /// the fall-back. This is the read-half of contract 4.10 that P-ID-08 floored — CLOSED here.
    pub fn lower_filter(
        &self,
        scope: &TenantScope,
        subject: &Principal,
        set_expr: &SetExpr,
        ty: &ObjectType,
        at: &Consistency,
    ) -> (crate::lowering::Lowered, crate::lowering::WatermarkVerdict) {
        let via = via_column_for(ty);
        let lowered = crate::lowering::lower(set_expr, subject, &via);
        let verdict = crate::lowering::watermark_verdict(&self.index, scope, &lowered, at);
        (lowered, verdict)
    }

    /// **The watermark-aware list (P-ID-12; ID-D4/ID-D7).** The whole-`list_objects` path that
    /// applies the consistency guard end-to-end: it dispatches to `Ids`/`Filter` as
    /// [`ListObjects::list_objects`] does, but when the result is a `Filter` whose S8 JOIN is BEHIND
    /// the scan's required revision, it **falls back to per-row `check`** over the authoritative S3
    /// store and returns the re-checked `Ids` (never a stale grant). When the watermark is fresh
    /// enough, the `Filter` is returned for the consumer to push down (the fast path). The `Ids`
    /// materialise path is already leak-free under staleness (each candidate is re-checked through
    /// the authoritative engine at the snapshot), so it is returned as-is.
    ///
    /// This is the ID-D7 new-enemy guard realised: a just-revoked grant read with the post-revoke
    /// zookie never survives — either the S8 watermark has caught up (and the JOIN reflects the
    /// revoke) or the scan falls back to `check` (which reads the authoritative post-revoke S3).
    pub fn list_objects_consistent(
        &self,
        scope: &TenantScope,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> ListObjectsResult {
        let result = self.list_objects(scope, subject, permission, ty, at);
        match result {
            // The Ids materialise path is already leak-free under staleness (every candidate is
            // re-checked through the authoritative engine at the snapshot — a revoked grant is
            // dropped). Return it as-is.
            ListObjectsResult::Ids { .. } => result,
            // The Filter push-down serves directly from S8 (no per-row re-check) — apply the
            // watermark guard. If the watermark is behind the required revision, fall back to a
            // per-row check over the authoritative store and return the re-checked Ids.
            ListObjectsResult::Filter {
                ref set_expr,
                ref zookie,
            } => {
                let via = via_column_for(ty);
                let lowered = crate::lowering::lower(set_expr, subject, &via);
                let verdict = crate::lowering::watermark_verdict(&self.index, scope, &lowered, at);
                if crate::lowering::is_fall_back(&verdict) {
                    // The S8 JOIN would serve a stale grant — fall back to per-row check over the
                    // authoritative S3 store at the required revision (the new-enemy guard, ID-D7).
                    // The candidate set is S8's reverse-index candidates (possibly stale); each is
                    // re-checked at the authoritative snapshot, so a revoked grant is dropped.
                    let candidates: Vec<ObjectId> = self
                        .candidate_objects(scope, subject, ty)
                        .into_iter()
                        .map(ObjectId)
                        .collect();
                    let allowed = crate::lowering::fall_back_to_check(
                        &self.engine,
                        &self.namespace,
                        scope,
                        subject,
                        permission,
                        ty,
                        &candidates,
                        at,
                    );
                    ListObjectsResult::Ids {
                        ids: allowed,
                        zookie: zookie.clone(),
                    }
                } else {
                    // The watermark is fresh enough — the consumer may push down the Filter JOIN.
                    result
                }
            }
        }
    }

    /// The read's consistency token — the S8 partition watermark (the revision the reverse index
    /// reflects). The wait/fall-back guard for a scan needing a fresher revision than the watermark
    /// is the [`ListObjects::list_objects_consistent`] / [`ListObjects::lower_filter`] path
    /// (P-ID-12). If the caller pinned a non-empty `at_least` zookie, that is the floor the read
    /// reflects.
    fn read_zookie(&self, scope: &TenantScope, at: &Consistency) -> Zookie {
        let watermark = self.index.watermark(scope);
        // The returned zookie is the LATER of the caller's pinned floor and the index watermark
        // (zookie strings are zero-padded `zk-<rev>` — lexical order == revision order).
        if !at.at_least.0.is_empty() && at.at_least.0 > watermark.0 {
            at.at_least.clone()
        } else {
            watermark
        }
    }
}

/// The §7.3 `via_column` for a consumer object type (the consumer's OWN id column the `Filter`
/// JOINs against). The frozen mapping: each consumer names its own table + id column (Git `pr.id` /
/// `repo.id`, CI `run.id`, Issues `issue.id`, Knowledge `db_row.id`, Chat `channel.id` /
/// `message.id`). On this floor the table is the type name and the column is `id` (the convention the
/// §7.3 table follows); the precise per-consumer column is the consumer's declaration (P-ID-12 wiring).
fn via_column_for(ty: &ObjectType) -> ColRef {
    ColRef {
        table: ty.0.clone(),
        column: "id".to_string(),
    }
}

/// Infer an object's TYPE from its id by the leading `type:` prefix (`repo:core` → `repo`). Mirrors
/// the same convention `namespace`/`reverse_index` use; kept local so this module does not reach into
/// a private helper.
fn type_of_object_id(object_id: &str) -> String {
    object_id
        .split_once(':')
        .map(|(ty, _)| ty.to_string())
        .unwrap_or_else(|| object_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reverse_index::{ReverseIndexConsumer, ReverseRow};
    use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
    use myelin_identity::{
        ConsistencyMode, PrincipalId, PrincipalKind, RelationTuple, TupleDelta,
    };
    use myelin_tenancy::{Region, TenantId};

    fn actor_in(tenant: &str) -> Principal {
        Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )
    }

    fn scope(tenant: &str) -> TenantScope {
        TenantScope::from_verified_token(&actor_in(tenant), Region("eu-west".into()))
    }

    fn subject(id: &str, tenant: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, TenantId(tenant.into()))
    }

    fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
        TupleDelta::Add(RelationTuple {
            object: ObjectId(object.into()),
            relation: RelName(relation.into()),
            subject: PrincipalId(subject.into()),
            caveat: None,
        })
    }

    fn now() -> Timestamp {
        Timestamp("2026-06-19T00:00:00Z".into())
    }

    fn latest() -> Consistency {
        Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        }
    }

    /// Build the S3 store + S8 index fed live off the bus from a set of grants, returning the wired
    /// `list_objects` evaluator (over the core hierarchy + an admitted `repo` fragment).
    fn wired(cap: usize, scope: &TenantScope, grants: &[TupleDelta]) -> ListObjects {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let index = ReverseIndex::new();
        let consumer = ReverseIndexConsumer::new(index.clone());

        // Admit a `repo` fragment so `repo` is a known type with a `reader` relation + `read`
        // permission (the candidate source + the permission resolution).
        let mut namespace = NamespaceEngine::with_core_hierarchy();
        use crate::namespace::{FragmentDef, PermissionRule, Userset};
        let _ = namespace.admit(&FragmentDef {
            object_type: ObjectType("repo".into()),
            relations: vec![RelName("reader".into()), RelName("writer".into())],
            permissions: vec![PermissionRule {
                permission: Permission("read".into()),
                rewrite: Userset::Union(vec![
                    Userset::Relation(RelName("reader".into())),
                    Userset::Relation(RelName("writer".into())),
                ]),
            }],
        });

        // Write each grant through S3 and feed S8 off the bus (the live feed).
        store
            .write_tuples(scope, &actor_in(&scope.tenant().0), grants, None, None, now())
            .expect("seed grants");
        let bus = InProcessBus::new();
        let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
        relay.drain_to_empty();
        for env in bus.consume("") {
            consumer.handle(&env);
        }

        ListObjects::with_cap(store, namespace, index, cap)
    }

    /// **A small reachable set returns `Ids` (the S4 materialise path).** alice is a reader of two
    /// repos; `list_objects(alice, read, repo)` under a cap of 10 materialises both ids.
    #[test]
    fn small_set_returns_ids() {
        let s = scope("acme");
        let lo = wired(
            10,
            &s,
            &[
                add("repo:core", "reader", "p:alice"),
                add("repo:web", "writer", "p:alice"),
                add("repo:secret", "reader", "p:bob"), // bob's, not alice's
            ],
        );
        let r = lo.list_objects(&s, &subject("p:alice", "acme"), &Permission("read".into()), &ObjectType("repo".into()), &latest());
        match r {
            ListObjectsResult::Ids { mut ids, .. } => {
                ids.sort_by(|a, b| a.0.cmp(&b.0));
                assert_eq!(
                    ids,
                    vec![ObjectId("repo:core".into()), ObjectId("repo:web".into())],
                    "alice's two readable repos materialise (leak-free — bob's repo is absent)"
                );
            }
            ListObjectsResult::Filter { .. } => panic!("a small set must materialise as Ids"),
        }
    }

    /// **The Ids↔Filter switch honours the cardinality cap.** With a cap of 1, alice's TWO readable
    /// repos exceed it → `Filter` (the S8 push-down), carrying the `SetExpr::InRelation` naming the
    /// consumer's id column. Under a cap of 10 the same set is `Ids` (above).
    #[test]
    fn ids_filter_switch_honours_the_cardinality_cap() {
        let s = scope("acme");
        let grants = [
            add("repo:core", "reader", "p:alice"),
            add("repo:web", "reader", "p:alice"),
        ];
        // cap = 1: two readable repos exceed it → Filter.
        let lo = wired(1, &s, &grants);
        let r = lo.list_objects(&s, &subject("p:alice", "acme"), &Permission("read".into()), &ObjectType("repo".into()), &latest());
        match r {
            ListObjectsResult::Filter { set_expr, .. } => match set_expr {
                SetExpr::InRelation { relation, via_column } => {
                    assert_eq!(relation, RelName("read".into()), "the push-down names the permission relation");
                    assert_eq!(via_column, ColRef { table: "repo".into(), column: "id".into() }, "the push-down names the consumer's own id column (§7.3)");
                }
                other => panic!("the Filter is the InRelation push-down shape, got {other:?}"),
            },
            ListObjectsResult::Ids { .. } => panic!("above the cap must dispatch to Filter"),
        }
    }

    /// **A subject with no grants returns the EMPTY `Ids` set (leak-free, never a permissive All).**
    #[test]
    fn no_grants_returns_empty_ids() {
        let s = scope("acme");
        let lo = wired(10, &s, &[add("repo:core", "reader", "p:alice")]);
        let r = lo.list_objects(&s, &subject("p:nobody", "acme"), &Permission("read".into()), &ObjectType("repo".into()), &latest());
        match r {
            ListObjectsResult::Ids { ids, .. } => assert!(ids.is_empty(), "no grants → the empty set, never All"),
            ListObjectsResult::Filter { .. } => panic!("no grants is a small (empty) set → Ids"),
        }
    }

    /// **A suspended/disabled subject sees the empty set (ID-D1 fail-closed at the list path).**
    #[test]
    fn suspended_subject_sees_empty_set() {
        let s = scope("acme");
        let lo = wired(10, &s, &[add("repo:core", "reader", "p:alice")]);
        let mut suspended = subject("p:alice", "acme");
        suspended.status = PrincipalStatus::Disabled;
        let r = lo.list_objects(&s, &suspended, &Permission("read".into()), &ObjectType("repo".into()), &latest());
        match r {
            ListObjectsResult::Ids { ids, .. } => assert!(ids.is_empty(), "a disabled subject sees nothing (ID-D1)"),
            ListObjectsResult::Filter { .. } => panic!("a disabled subject is the empty Ids set"),
        }
    }

    /// **The returned `Ids` carry the S8 watermark zookie (the consistency token).** After a write,
    /// the list reflects the partition watermark.
    #[test]
    fn ids_carry_the_s8_watermark_zookie() {
        let s = scope("acme");
        let lo = wired(10, &s, &[add("repo:core", "reader", "p:alice")]);
        let r = lo.list_objects(&s, &subject("p:alice", "acme"), &Permission("read".into()), &ObjectType("repo".into()), &latest());
        let zookie = match r {
            ListObjectsResult::Ids { zookie, .. } => zookie,
            ListObjectsResult::Filter { zookie, .. } => zookie,
        };
        assert_eq!(zookie, lo.index.watermark(&s), "the list reflects the S8 partition watermark");
        assert!(!zookie.0.is_empty(), "the watermark advanced after the write");
    }

    /// **No cross-tenant list path.** alice's repos in `acme` do not appear in a list under `globex`
    /// (the reverse index + the engine read only the verified scope's partition).
    #[test]
    fn no_cross_tenant_list_path() {
        let acme = scope("acme");
        let lo = wired(10, &acme, &[add("repo:core", "reader", "p:alice")]);
        let globex = scope("globex");
        let r = lo.list_objects(&globex, &subject("p:alice", "globex"), &Permission("read".into()), &ObjectType("repo".into()), &latest());
        match r {
            ListObjectsResult::Ids { ids, .. } => assert!(ids.is_empty(), "a grant in acme does not list under globex"),
            ListObjectsResult::Filter { .. } => panic!("the cross-tenant set is empty → Ids"),
        }
    }

    /// **The default cap is the thresholds default-to-beat (the SHAPE frozen, the NUMBER open).**
    #[test]
    fn default_cap_is_the_default_to_beat() {
        let s = scope("acme");
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox);
        let index = ReverseIndex::new();
        let lo = ListObjects::new(store, NamespaceEngine::with_core_hierarchy(), index);
        assert_eq!(lo.cap(), DEFAULT_IDS_CARDINALITY_CAP);
        assert_eq!(DEFAULT_IDS_CARDINALITY_CAP, 1000, "the seed default-to-beat written to thresholds.toml");
        let _ = s;
    }

    /// **The reverse-index candidate path is exercised directly (the S4 source is S8).** A row added
    /// straight into S8 is a candidate the permission resolution then admits/denies.
    #[test]
    fn reachable_set_reads_candidates_from_s8() {
        let s = scope("acme");
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox);
        let index = ReverseIndex::new();
        // Seed a tuple in S3 so the permission resolution finds the direct grant, AND mirror it into
        // S8 (the candidate source) directly.
        store
            .write_tuples(&s, &actor_in("acme"), &[add("repo:core", "reader", "p:alice")], None, None, now())
            .expect("seed");
        index.apply_delta(
            &s,
            "add",
            &ObjectType("repo".into()),
            ReverseRow {
                subject: PrincipalId("p:alice".into()),
                relation: RelName("reader".into()),
                object_id: ObjectId("repo:core".into()),
            },
            &Zookie("zk-00000000000000000001".into()),
        );
        let mut namespace = NamespaceEngine::with_core_hierarchy();
        use crate::namespace::{FragmentDef, PermissionRule, Userset};
        let _ = namespace.admit(&FragmentDef {
            object_type: ObjectType("repo".into()),
            relations: vec![RelName("reader".into())],
            permissions: vec![PermissionRule {
                permission: Permission("read".into()),
                rewrite: Userset::Relation(RelName("reader".into())),
            }],
        });
        let lo = ListObjects::with_cap(store, namespace, index, 10);
        let r = lo.list_objects(&s, &subject("p:alice", "acme"), &Permission("read".into()), &ObjectType("repo".into()), &latest());
        match r {
            ListObjectsResult::Ids { ids, .. } => assert_eq!(ids, vec![ObjectId("repo:core".into())]),
            ListObjectsResult::Filter { .. } => panic!("a single candidate materialises as Ids"),
        }
    }
}
