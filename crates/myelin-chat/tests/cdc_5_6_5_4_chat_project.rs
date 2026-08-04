use std::sync::Mutex;

use myelin_chat::content::{paragraph_body, MessageBody};
use myelin_chat::glue::register_chat_humanise_templates;
use myelin_chat::membership::channel_object;
use myelin_chat::project::{
    densest_edge_producer, ChannelMeta, ChatProjectionSource, MessageMeta, Projected, Projector,
    ThreadMeta, TombstoneReason,
};
use myelin_chat::subs::{mint_channel, mint_message, mint_thread};

use myelin_content::{InlineNode, OBJ};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, Result as IdResult, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree,
    TupleDelta, Zookie,
};
use myelin_notif::TemplateStore;
use myelin_refs::ArtifactRef;
use myelin_tenancy::TenantId;

#[derive(Default)]
struct GateId {
    allow: Mutex<Vec<(String, String)>>,
}
impl GateId {
    fn allow(&self, subject: &str, object: &str) {
        self.allow
            .lock()
            .unwrap()
            .push((subject.into(), object.into()));
    }
}
impl IdentityService for GateId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn check(
        &self,
        subject: &Principal,
        _permission: &Permission,
        object: &myelin_tenancy::ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        let allowed = self
            .allow
            .lock()
            .unwrap()
            .iter()
            .any(|(s, o)| s == &subject.principal_id.0 && o == &object.0);
        Ok(if allowed {
            Decision::Allow
        } else {
            Decision::Deny
        })
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _at: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _t: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Ok(FragmentAdmit::Admitted {
            fragment_id: "cdc".into(),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RenderedCard {
    Live {
        title: String,
        state: String,
        icon: String,
        render_hint: String,
        sub_anchor: Option<String>,
    },
    Tombstone {
        root: String,
        reason: TombstoneReason,
    },
}

fn consumer_render(projected: &Projected) -> RenderedCard {
    match projected {
        Projected::Visible(p) => RenderedCard::Live {
            title: p.title.clone(),
            state: p.state.clone(),
            icon: p.icon.clone(),
            render_hint: p.render_hint.as_str().to_string(),
            sub_anchor: p.sub_anchor.clone(),
        },
        Projected::Tombstoned(t) => RenderedCard::Tombstone {
            root: t.root.0.clone(),
            reason: t.reason,
        },
    }
}

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

const SECRET_PREVIEW: &str = "Q4 comp adjustments for the exec team";
const SECRET_CHANNEL: &str = "#board-comp";

fn templates() -> TemplateStore {
    let mut s = TemplateStore::new();
    register_chat_humanise_templates(&mut s);
    s
}

fn source() -> ChatProjectionSource {
    let mut src = ChatProjectionSource::new();
    src.put_channel(
        &mint_channel("acme", "C-board").unwrap(),
        ChannelMeta {
            label: SECRET_CHANNEL.into(),
            archived: false,
        },
    );
    src.put_message(
        &myelin_refs::strip_sub(&mint_message("acme", "01J0M").unwrap()),
        MessageMeta {
            channel_id: "C-board".into(),
            preview: SECRET_PREVIEW.into(),
            state: "active".into(),
        },
    );
    src.put_thread(
        &myelin_refs::strip_sub(&mint_thread("acme", "01J0T").unwrap()),
        ThreadMeta {
            channel_id: "C-board".into(),
            root_preview: SECRET_PREVIEW.into(),
            reply_count: 5,
            state: "active".into(),
        },
    );
    src
}

#[test]
fn cdc_5_6_member_projection_renders_the_frozen_shape() {
    let id = GateId::default();
    id.allow("member", &channel_object("C-board"));
    let p = Projector::new(id, source(), templates());
    let m = viewer("member");

    let channel = consumer_render(
        &p.project(
            &mint_channel("acme", "C-board").unwrap(),
            &m,
            Zookie(String::new()),
        )
        .unwrap(),
    );
    assert_eq!(
        channel,
        RenderedCard::Live {
            title: SECRET_CHANNEL.into(),
            state: "active".into(),
            icon: "channel".into(),
            render_hint: "ChannelChip".into(),
            sub_anchor: None,
        }
    );

    let message = consumer_render(
        &p.project(
            &mint_message("acme", "01J0M").unwrap(),
            &m,
            Zookie(String::new()),
        )
        .unwrap(),
    );
    assert_eq!(
        message,
        RenderedCard::Live {
            title: SECRET_PREVIEW.into(),
            state: "active".into(),
            icon: "message".into(),
            render_hint: "MessageChip".into(),
            sub_anchor: Some("01J0M".into()),
        }
    );

    let thread = consumer_render(
        &p.project(
            &mint_thread("acme", "01J0T").unwrap(),
            &m,
            Zookie(String::new()),
        )
        .unwrap(),
    );
    assert_eq!(
        thread,
        RenderedCard::Live {
            title: format!("{SECRET_PREVIEW} (5 replies)"),
            state: "active".into(),
            icon: "thread".into(),
            render_hint: "ThreadChip".into(),
            sub_anchor: Some("01J0T".into()),
        }
    );
}

#[test]
fn cdc_5_6_non_member_projection_is_a_tombstone_zero_leak() {
    let id = GateId::default();
    let p = Projector::new(id, source(), templates());
    let intruder = viewer("intruder");

    for reference in [
        mint_channel("acme", "C-board").unwrap(),
        mint_message("acme", "01J0M").unwrap(),
        mint_thread("acme", "01J0T").unwrap(),
    ] {
        let card = consumer_render(
            &p.project(&reference, &intruder, Zookie(String::new()))
                .unwrap(),
        );
        match card {
            RenderedCard::Tombstone { root, reason } => {
                assert_eq!(reason, TombstoneReason::Denied);
                assert_eq!(root, myelin_refs::strip_sub(&reference).0);
                assert!(!root.contains(SECRET_PREVIEW));
                assert!(!root.contains(SECRET_CHANNEL));
            }
            RenderedCard::Live { .. } => panic!("a non-member must NOT receive a live card"),
        }
    }
}

#[test]
fn cdc_5_6_erased_subject_is_a_content_free_tombstone() {
    let id = GateId::default();
    id.allow("member", &channel_object("C-board"));
    let mut src = source();
    src.mark_erased(&myelin_refs::strip_sub(
        &mint_message("acme", "01J0M").unwrap(),
    ));
    let p = Projector::new(id, src, templates());

    let card = consumer_render(
        &p.project(
            &mint_message("acme", "01J0M").unwrap(),
            &viewer("member"),
            Zookie(String::new()),
        )
        .unwrap(),
    );
    match card {
        RenderedCard::Tombstone { reason, .. } => assert_eq!(reason, TombstoneReason::Erased),
        RenderedCard::Live { .. } => panic!("an erased subject must render a tombstone"),
    }
}

#[test]
fn cdc_5_4_chat_is_the_densest_edge_producer() {
    let src = mint_message("acme", "01J0M").unwrap();
    let alice = viewer("alice");
    let issue = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
    let page = ArtifactRef("myelin://acme/knowledge/page/7c2".into());

    let corpus: Vec<MessageBody> = vec![
        paragraph_body(
            &format!("hi {OBJ} re {OBJ} see {OBJ}"),
            vec![
                InlineNode::Mention(alice.clone()),
                InlineNode::ArtifactRefNode(issue.clone()),
                InlineNode::Embed(page.clone()),
            ],
        ),
        paragraph_body(
            &format!("cc {OBJ}"),
            vec![InlineNode::Mention(alice.clone())],
        ),
        paragraph_body("ping @alice see myelin://acme/issue/ENG-2", vec![]),
    ];

    let total = densest_edge_producer(&src, &corpus);
    let structured: usize = corpus.iter().map(|b| b.structured_nodes().len()).sum();
    assert_eq!(
        total, structured,
        "0 missing edges: one edge per structured node"
    );
    assert_eq!(total, 4, "3 + 1 + 0 → 4 edges");
}
