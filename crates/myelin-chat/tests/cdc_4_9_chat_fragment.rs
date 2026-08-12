use myelin_chat::rebac_fragment::{self, object_types};
use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, NamespaceFragment,
    ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind, RelName,
    RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{FragmentDef, PermissionRule, StoreBackedCheck, TupleStore, Userset};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

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

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
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
            Timestamp("2026-06-20T00:00:00Z".into()),
        )
        .expect("seed tuples");
    StoreBackedCheck::new(store)
}

fn chat_fragment_defs_rich() -> Vec<FragmentDef> {
    let rel = |n: &str| Userset::Relation(RelName(n.into()));
    let ttu = |tupleset: &str, computed: &str| Userset::TupleToUserset {
        tupleset: RelName(tupleset.into()),
        computed: RelName(computed.into()),
    };
    vec![
        FragmentDef {
            object_type: ObjectType(object_types::CHANNEL.into()),
            relations: vec![
                RelName("parent_project".into()),
                RelName("member".into()),
                RelName("watcher".into()),
            ],
            permissions: vec![
                PermissionRule {
                    permission: Permission("read".into()),
                    rewrite: Userset::Union(vec![rel("member"), ttu("parent_project", "view")]),
                },
                PermissionRule {
                    permission: Permission("post".into()),
                    rewrite: rel("member"),
                },
                PermissionRule {
                    permission: Permission("manage".into()),
                    rewrite: Userset::Intersect(vec![rel("member"), ttu("parent_project", "view")]),
                },
            ],
        },
        FragmentDef {
            object_type: ObjectType(object_types::MESSAGE.into()),
            relations: vec![RelName("parent_channel".into())],
            permissions: vec![PermissionRule {
                permission: Permission("view".into()),
                rewrite: ttu("parent_channel", "read"),
            }],
        },
    ]
}

#[test]
fn cdc_4_9_chat_names_only_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);

    let consumer_fragment: Vec<NamespaceFragment> = rebac_fragment::chat_fragment();
    assert_eq!(consumer_fragment.len(), 2, "channel + message");
    let types: Vec<&str> = consumer_fragment
        .iter()
        .map(|f| f.object_type.0.as_str())
        .collect();
    assert_eq!(types, vec!["channel", "message"]);

    for def in chat_fragment_defs_rich() {
        let admit = svc.admit_fragment_def(&def);
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "the Chat `{}` fragment must admit into the cell schema: {admit:?}",
            def.object_type.0
        );
    }
}

#[test]
fn cdc_4_9_channel_read_member_plus_parent_project_resolves() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("channel:secret", "member", "p:alice"),
            add("channel:general", "parent_project", "project:web#view"),
            add("project:web", "reader", "p:bob"),
        ],
    );
    for def in chat_fragment_defs_rich() {
        assert!(
            matches!(svc.admit_fragment_def(&def), FragmentAdmit::Admitted { .. }),
            "Chat `{}` admits",
            def.object_type.0
        );
    }

    let can_read = |channel: &str, actor: &Principal| {
        matches!(
            svc.check(
                actor,
                &Permission("read".into()),
                &ArtifactRef(channel.into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };

    assert!(
        can_read("channel:secret", &subject("p:alice")),
        "a member reads the private channel (the `member` arm of channel.read)"
    );
    assert!(
        can_read("channel:general", &subject("p:bob")),
        "a project reader inherits read on a public channel (the + parent_project->read arm)"
    );
    assert!(
        !can_read("channel:secret", &subject("p:carol")),
        "a non-member, non-project-reader cannot read the private channel (no leak)"
    );
    assert!(
        !can_read("channel:general", &subject("p:carol")),
        "a non-member, non-project-reader cannot read the public channel (no leak)"
    );
}

#[test]
fn cdc_4_9_message_view_inherits_parent_channel_read() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("channel:secret", "member", "p:alice"),
            add("message:m1", "parent_channel", "channel:secret#read"),
        ],
    );
    for def in chat_fragment_defs_rich() {
        assert!(matches!(
            svc.admit_fragment_def(&def),
            FragmentAdmit::Admitted { .. }
        ));
    }

    let can_view = |actor: &Principal| {
        matches!(
            svc.check(
                actor,
                &Permission("view".into()),
                &ArtifactRef("message:m1".into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };

    assert!(
        can_view(&subject("p:alice")),
        "a member of the parent channel can view the message (message.view = parent_channel->read)"
    );
    assert!(
        !can_view(&subject("p:carol")),
        "a non-member cannot view the message (it inherits the channel's read - no leak)"
    );
}

#[test]
fn cdc_4_9_watcher_is_read_fanout_not_a_read_grant() {
    let s = scope("acme");
    let svc = provider(&s, &[add("channel:secret", "watcher", "p:dave")]);
    for def in chat_fragment_defs_rich() {
        assert!(matches!(
            svc.admit_fragment_def(&def),
            FragmentAdmit::Admitted { .. }
        ));
    }

    assert!(
        !matches!(
            svc.check(
                &subject("p:dave"),
                &Permission("read".into()),
                &ArtifactRef("channel:secret".into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        ),
        "a `watcher` is the Notif read-fanout relation, NOT a read grant (read = member + \
         parent_project->read; §5 / contract 4.9)"
    );
}
