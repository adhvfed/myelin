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

pub const DEFAULT_IDS_CARDINALITY_CAP: usize = 1000;

#[derive(Clone)]
pub struct ListObjects {
    engine: CheckEngine,
    namespace: NamespaceEngine,
    index: ReverseIndex,
    cap: usize,
}

impl ListObjects {
    pub fn new(tuples: TupleStore, namespace: NamespaceEngine, index: ReverseIndex) -> ListObjects {
        ListObjects {
            engine: CheckEngine::new(tuples),
            namespace,
            index,
            cap: DEFAULT_IDS_CARDINALITY_CAP,
        }
    }

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

    pub fn cap(&self) -> usize {
        self.cap
    }

    pub fn list_objects(
        &self,
        scope: &TenantScope,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> myelin_identity::Result<ListObjectsResult> {
        let zookie = self.read_zookie(scope, at);

        if subject.status != PrincipalStatus::Active {
            return Ok(ListObjectsResult::Ids {
                ids: Vec::new(),
                zookie,
            });
        }

        let reachable = self.reachable_set(scope, subject, permission, ty, at)?;

        if reachable.len() <= self.cap {
            Ok(ListObjectsResult::Ids {
                ids: reachable.into_iter().map(ObjectId).collect(),
                zookie,
            })
        } else {
            Ok(ListObjectsResult::Filter {
                set_expr: self.filter_set_expr(subject, permission, ty),
                zookie,
            })
        }
    }

    fn reachable_set(
        &self,
        scope: &TenantScope,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> myelin_identity::Result<BTreeSet<String>> {
        let candidates = self.candidate_objects(scope, subject, ty);
        let snapshot = self.engine.snapshot(scope, &at.at_least)?;

        let mut reachable: BTreeSet<String> = BTreeSet::new();
        for obj in candidates {
            let object_ref = ArtifactRef(obj.clone());
            let object_type = type_of_object_id(&obj);
            let granted = self.namespace.permits_snapshot(
                &snapshot,
                subject,
                &object_type,
                &permission.0,
                &object_ref,
            );
            if granted {
                reachable.insert(obj);
            }
            if reachable.len() > self.cap {
                break;
            }
        }
        Ok(reachable)
    }

    fn candidate_objects(
        &self,
        scope: &TenantScope,
        subject: &Principal,
        ty: &ObjectType,
    ) -> BTreeSet<String> {
        let mut candidates: BTreeSet<String> = BTreeSet::new();
        for relation in self.namespace.relations_of(&ty.0) {
            for obj in
                self.index
                    .objects_for(scope, ty, &subject.principal_id, &RelName(relation.clone()))
            {
                candidates.insert(obj.0);
            }
        }
        candidates
    }

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

    pub fn list_objects_consistent(
        &self,
        scope: &TenantScope,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> myelin_identity::Result<ListObjectsResult> {
        let result = self.list_objects(scope, subject, permission, ty, at)?;
        match result {
            ListObjectsResult::Ids { .. } => Ok(result),
            ListObjectsResult::Filter {
                ref set_expr,
                ref zookie,
            } => {
                let via = via_column_for(ty);
                let lowered = crate::lowering::lower(set_expr, subject, &via);
                let verdict = crate::lowering::watermark_verdict(&self.index, scope, &lowered, at);
                if crate::lowering::is_fall_back(&verdict) {
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
                    Ok(ListObjectsResult::Ids {
                        ids: allowed,
                        zookie: zookie.clone(),
                    })
                } else {
                    Ok(result)
                }
            }
        }
    }

    fn read_zookie(&self, scope: &TenantScope, at: &Consistency) -> Zookie {
        let watermark = self.index.watermark(scope);
        if !at.at_least.0.is_empty() && at.at_least.0 > watermark.0 {
            at.at_least.clone()
        } else {
            watermark
        }
    }
}

fn via_column_for(ty: &ObjectType) -> ColRef {
    let table = match ty.0.as_str() {
        knowledge_db_row::OBJECT_TYPE => knowledge_db_row::TABLE.to_string(),
        _ => ty.0.clone(),
    };
    ColRef {
        table,
        column: "id".to_string(),
    }
}

mod knowledge_db_row {
    pub const OBJECT_TYPE: &str = "database_row";
    pub const TABLE: &str = "db_row";
}

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
    use myelin_events::{
        BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp,
    };
    use myelin_identity::{ConsistencyMode, PrincipalId, PrincipalKind, RelationTuple, TupleDelta};
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
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )
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

    fn wired(cap: usize, scope: &TenantScope, grants: &[TupleDelta]) -> ListObjects {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let index = ReverseIndex::new();
        let consumer = ReverseIndexConsumer::new(index.clone());

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

        store
            .write_tuples(
                scope,
                &actor_in(&scope.tenant().0),
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

        ListObjects::with_cap(store, namespace, index, cap)
    }

    #[test]
    fn small_set_returns_ids() {
        let s = scope("acme");
        let lo = wired(
            10,
            &s,
            &[
                add("repo:core", "reader", "p:alice"),
                add("repo:web", "writer", "p:alice"),
                add("repo:secret", "reader", "p:bob"),
            ],
        );
        let r = lo
            .list_objects(
                &s,
                &subject("p:alice", "acme"),
                &Permission("read".into()),
                &ObjectType("repo".into()),
                &latest(),
            )
            .expect("read relationships for the small object set");
        match r {
            ListObjectsResult::Ids { mut ids, .. } => {
                ids.sort_by(|a, b| a.0.cmp(&b.0));
                assert_eq!(
                    ids,
                    vec![ObjectId("repo:core".into()), ObjectId("repo:web".into())],
                    "alice's two readable repos materialise (leak-free - bob's repo is absent)"
                );
            }
            ListObjectsResult::Filter { .. } => panic!("a small set must materialise as Ids"),
        }
    }

    #[test]
    fn ids_filter_switch_honours_the_cardinality_cap() {
        let s = scope("acme");
        let grants = [
            add("repo:core", "reader", "p:alice"),
            add("repo:web", "reader", "p:alice"),
        ];
        let lo = wired(1, &s, &grants);
        let r = lo
            .list_objects(
                &s,
                &subject("p:alice", "acme"),
                &Permission("read".into()),
                &ObjectType("repo".into()),
                &latest(),
            )
            .expect("read relationships for the over-cap object set");
        match r {
            ListObjectsResult::Filter { set_expr, .. } => match set_expr {
                SetExpr::InRelation {
                    relation,
                    via_column,
                } => {
                    assert_eq!(
                        relation,
                        RelName("read".into()),
                        "the push-down names the permission relation"
                    );
                    assert_eq!(
                        via_column,
                        ColRef {
                            table: "repo".into(),
                            column: "id".into()
                        },
                        "the push-down names the consumer's own id column (§7.3)"
                    );
                }
                other => panic!("the Filter is the InRelation push-down shape, got {other:?}"),
            },
            ListObjectsResult::Ids { .. } => panic!("above the cap must dispatch to Filter"),
        }
    }

    #[test]
    fn no_grants_returns_empty_ids() {
        let s = scope("acme");
        let lo = wired(10, &s, &[add("repo:core", "reader", "p:alice")]);
        let r = lo
            .list_objects(
                &s,
                &subject("p:nobody", "acme"),
                &Permission("read".into()),
                &ObjectType("repo".into()),
                &latest(),
            )
            .expect("read relationships for the ungranted subject");
        match r {
            ListObjectsResult::Ids { ids, .. } => {
                assert!(ids.is_empty(), "no grants → the empty set, never All")
            }
            ListObjectsResult::Filter { .. } => panic!("no grants is a small (empty) set → Ids"),
        }
    }

    #[test]
    fn relationship_outage_is_not_reported_as_an_empty_authorization_set() {
        let s = scope("acme");
        let store = TupleStore::new(OutboxStore::new())
            .with_unavailable_reads("relationship database is offline");
        let lo = ListObjects::new(
            store,
            NamespaceEngine::with_core_hierarchy(),
            ReverseIndex::new(),
        );

        let result = lo.list_objects(
            &s,
            &subject("p:alice", "acme"),
            &Permission("read".into()),
            &ObjectType("repo".into()),
            &latest(),
        );

        assert!(
            matches!(result, Err(myelin_identity::AuthzError::Unavailable(_))),
            "an unavailable relationship snapshot is operational failure, not an empty set"
        );
    }

    #[test]
    fn suspended_subject_sees_empty_set() {
        let s = scope("acme");
        let store = TupleStore::new(OutboxStore::new())
            .with_unavailable_reads("relationship database is offline");
        let lo = ListObjects::new(
            store,
            NamespaceEngine::with_core_hierarchy(),
            ReverseIndex::new(),
        );
        let mut suspended = subject("p:alice", "acme");
        suspended.status = PrincipalStatus::Disabled;
        let r = lo
            .list_objects(
                &s,
                &suspended,
                &Permission("read".into()),
                &ObjectType("repo".into()),
                &latest(),
            )
            .expect("resolve the disabled subject without reading a grant");
        match r {
            ListObjectsResult::Ids { ids, .. } => {
                assert!(ids.is_empty(), "a disabled subject sees nothing (ID-D1)")
            }
            ListObjectsResult::Filter { .. } => panic!("a disabled subject is the empty Ids set"),
        }
    }

    #[test]
    fn ids_carry_the_s8_watermark_zookie() {
        let s = scope("acme");
        let lo = wired(10, &s, &[add("repo:core", "reader", "p:alice")]);
        let r = lo
            .list_objects(
                &s,
                &subject("p:alice", "acme"),
                &Permission("read".into()),
                &ObjectType("repo".into()),
                &latest(),
            )
            .expect("read relationships at the reverse-index watermark");
        let zookie = match r {
            ListObjectsResult::Ids { zookie, .. } => zookie,
            ListObjectsResult::Filter { zookie, .. } => zookie,
        };
        assert_eq!(
            zookie,
            lo.index.watermark(&s),
            "the list reflects the S8 partition watermark"
        );
        assert!(
            !zookie.0.is_empty(),
            "the watermark advanced after the write"
        );
    }

    #[test]
    fn no_cross_tenant_list_path() {
        let acme = scope("acme");
        let lo = wired(10, &acme, &[add("repo:core", "reader", "p:alice")]);
        let globex = scope("globex");
        let r = lo
            .list_objects(
                &globex,
                &subject("p:alice", "globex"),
                &Permission("read".into()),
                &ObjectType("repo".into()),
                &latest(),
            )
            .expect("read only the other tenant's relationship partition");
        match r {
            ListObjectsResult::Ids { ids, .. } => {
                assert!(ids.is_empty(), "a grant in acme does not list under globex")
            }
            ListObjectsResult::Filter { .. } => panic!("the cross-tenant set is empty → Ids"),
        }
    }

    #[test]
    fn default_cap_is_the_default_to_beat() {
        let s = scope("acme");
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox);
        let index = ReverseIndex::new();
        let lo = ListObjects::new(store, NamespaceEngine::with_core_hierarchy(), index);
        assert_eq!(lo.cap(), DEFAULT_IDS_CARDINALITY_CAP);
        assert_eq!(
            DEFAULT_IDS_CARDINALITY_CAP, 1000,
            "the seed default-to-beat written to thresholds.toml"
        );
        let _ = s;
    }

    #[test]
    fn reachable_set_reads_candidates_from_s8() {
        let s = scope("acme");
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox);
        let index = ReverseIndex::new();
        store
            .write_tuples(
                &s,
                &actor_in("acme"),
                &[add("repo:core", "reader", "p:alice")],
                None,
                None,
                now(),
            )
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
        let r = lo
            .list_objects(
                &s,
                &subject("p:alice", "acme"),
                &Permission("read".into()),
                &ObjectType("repo".into()),
                &latest(),
            )
            .expect("read the indexed candidate's relationship snapshot");
        match r {
            ListObjectsResult::Ids { ids, .. } => {
                assert_eq!(ids, vec![ObjectId("repo:core".into())])
            }
            ListObjectsResult::Filter { .. } => panic!("a single candidate materialises as Ids"),
        }
    }
}
