use super::*;
use std::sync::Mutex as StdMutex;

use myelin_content::{InlineNode, OBJ};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, Result as IdResult, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree,
    TupleDelta, Zookie,
};
use myelin_tenancy::TenantId;

use crate::content::{paragraph_body, MessageBody};
use crate::glue::register_chat_humanise_templates;
use crate::membership::channel_object;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

const SECRET_PREVIEW: &str = "the board comp numbers are";
const SECRET_CHANNEL: &str = "#board-leadership-comp";

const BODY_BYTES_LEAK_PROBE: &str = "ENTIRE-CONFIDENTIAL-MESSAGE-BODY-BYTES";

fn channel_ref() -> ArtifactRef {
    crate::subs::mint_channel("acme", "C-board").unwrap()
}
fn message_ref() -> ArtifactRef {
    crate::subs::mint_message("acme", "01J0MSG").unwrap()
}
fn thread_ref() -> ArtifactRef {
    crate::subs::mint_thread("acme", "01J0THR").unwrap()
}

#[test]
fn foreign_commit_check_subanchors_preserve_their_canonical_opaque_body() {
    let check = ArtifactRef(
        "myelin://acme/chat/channel/C-board#commit-deadbeef/check-build".into(),
    );
    let result =
        ArtifactRef("myelin://acme/chat/channel/C-board#commit-deadbeef/ci-result".into());
    assert_eq!(
        sub_opaque(&check).as_deref(),
        Some("commit-deadbeef/check-build")
    );
    assert_eq!(
        sub_opaque(&result).as_deref(),
        Some("commit-deadbeef/ci-result")
    );
}

#[derive(Default)]
struct GateId {
    allow: StdMutex<Vec<(String, String)>>,
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
        Err(AuthzError::NotYetImplemented("test"))
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
        Err(AuthzError::NotYetImplemented("test"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("test"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("test"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("test"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("test"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _t: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("test"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("test"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("test"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("test"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Ok(FragmentAdmit::Admitted {
            fragment_id: "test".into(),
        })
    }
}

fn templates() -> TemplateStore {
    let mut store = TemplateStore::new();
    register_chat_humanise_templates(&mut store);
    store
}

fn seeded_source() -> ChatProjectionSource {
    let mut src = ChatProjectionSource::new();
    src.put_channel(
        &channel_ref(),
        ChannelMeta {
            label: SECRET_CHANNEL.into(),
            archived: false,
        },
    );
    src.put_message(
        &myelin_refs::strip_sub(&message_ref()),
        MessageMeta {
            channel_id: "C-board".into(),
            preview: SECRET_PREVIEW.into(),
            state: "active".into(),
        },
    );
    src.put_thread(
        &myelin_refs::strip_sub(&thread_ref()),
        ThreadMeta {
            channel_id: "C-board".into(),
            root_preview: SECRET_PREVIEW.into(),
            reply_count: 3,
            state: "active".into(),
        },
    );
    src
}

fn projector(id: GateId) -> Projector<GateId> {
    Projector::new(id, seeded_source(), templates())
}

#[test]
fn projection_is_title_plus_metadata_never_the_body() {
    let id = GateId::default();
    let obj = channel_object("C-board");
    id.allow("alice", &obj);
    let p = projector(id);

    match p
        .project(&channel_ref(), &viewer("alice"), Zookie(String::new()))
        .unwrap()
    {
        Projected::Visible(proj) => {
            assert_eq!(proj.title, SECRET_CHANNEL);
            assert_eq!(proj.render_hint, RenderHint::ChannelChip);
            assert_eq!(proj.render_hint.as_str(), "ChannelChip");
            assert_eq!(proj.icon, "channel");
            assert_eq!(proj.state, "active");
            assert!(proj.sub_anchor.is_none(), "a bare channel root has no #sub");
            assert!(!proj.title.contains(BODY_BYTES_LEAK_PROBE));
        }
        other => panic!("expected a visible channel projection, got {other:?}"),
    }

    match p
        .project(&message_ref(), &viewer("alice"), Zookie(String::new()))
        .unwrap()
    {
        Projected::Visible(proj) => {
            assert_eq!(proj.title, SECRET_PREVIEW);
            assert_eq!(proj.render_hint, RenderHint::MessageChip);
            assert_eq!(proj.sub_anchor.as_deref(), Some("01J0MSG"));
            assert!(!proj.title.contains(BODY_BYTES_LEAK_PROBE));
        }
        other => panic!("expected a visible message projection, got {other:?}"),
    }

    match p
        .project(&thread_ref(), &viewer("alice"), Zookie(String::new()))
        .unwrap()
    {
        Projected::Visible(proj) => {
            assert_eq!(proj.title, format!("{SECRET_PREVIEW} (3 replies)"));
            assert_eq!(proj.render_hint, RenderHint::ThreadChip);
            assert_eq!(proj.sub_anchor.as_deref(), Some("01J0THR"));
        }
        other => panic!("expected a visible thread projection, got {other:?}"),
    }
}

#[test]
fn thread_reply_count_pluralises_through_humanise() {
    let id = GateId::default();
    id.allow("alice", &channel_object("C-board"));
    let mut src = seeded_source();
    src.put_thread(
        &myelin_refs::strip_sub(&thread_ref()),
        ThreadMeta {
            channel_id: "C-board".into(),
            root_preview: "deploy plan".into(),
            reply_count: 1,
            state: "active".into(),
        },
    );
    let p = Projector::new(id, src, templates());
    assert_eq!(
        p.project(&thread_ref(), &viewer("alice"), Zookie(String::new()))
            .unwrap()
            .title(),
        Some("deploy plan (1 reply)"),
        "one reply pluralises to the singular branch"
    );
}

#[test]
fn non_member_gets_denied_tombstone_for_every_type_zero_leak() {
    let id = GateId::default();
    let p = projector(id);
    let intruder = viewer("intruder");

    for reference in [channel_ref(), message_ref(), thread_ref()] {
        let projected = p
            .project(&reference, &intruder, Zookie(String::new()))
            .unwrap();
        assert!(
            projected.is_tombstone(),
            "a non-member sees a tombstone for {}",
            reference.0
        );
        assert_eq!(projected.title(), None, "0 title leak for {}", reference.0);
        match projected {
            Projected::Tombstoned(t) => {
                assert_eq!(t.reason, TombstoneReason::Denied);
                assert_eq!(t.root, myelin_refs::strip_sub(&reference));
                assert!(!t.root.0.contains(SECRET_PREVIEW));
                assert!(!t.root.0.contains(SECRET_CHANNEL));
            }
            other => panic!("expected a tombstone, got {other:?}"),
        }
    }
}

#[test]
fn per_viewer_member_sees_title_non_member_tombstones() {
    let id = GateId::default();
    id.allow("member", &channel_object("C-board"));
    let p = projector(id);

    assert_eq!(
        p.project(&message_ref(), &viewer("member"), Zookie(String::new()))
            .unwrap()
            .title(),
        Some(SECRET_PREVIEW),
        "a member sees the preview"
    );
    assert!(
        p.project(&message_ref(), &viewer("stranger"), Zookie(String::new()))
            .unwrap()
            .is_tombstone(),
        "a non-member sees a tombstone for the SAME message"
    );
}

#[test]
fn message_inherits_its_home_channel_gate() {
    let id = GateId::default();
    id.allow("alice", &channel_object("C-other"));
    let p = projector(id);
    assert!(
        p.project(&message_ref(), &viewer("alice"), Zookie(String::new()))
            .unwrap()
            .is_tombstone(),
        "read on another channel does not unlock this message"
    );
}

#[test]
fn gone_root_tombstones_carrying_the_root() {
    let id = GateId::default();
    id.allow("alice", &channel_object("C-gone"));
    let mut src = ChatProjectionSource::new();
    let gone_channel = crate::subs::mint_channel("acme", "C-gone").unwrap();
    let _ = &mut src;
    let p = Projector::new(id, src, templates());
    match p
        .project(&gone_channel, &viewer("alice"), Zookie(String::new()))
        .unwrap()
    {
        Projected::Tombstoned(t) => {
            assert_eq!(t.reason, TombstoneReason::Gone);
            assert_eq!(t.root, gone_channel);
        }
        other => panic!("expected a Gone tombstone, got {other:?}"),
    }
}

#[test]
fn erased_subject_tombstones_even_for_allowed_viewer() {
    let id = GateId::default();
    id.allow("alice", &channel_object("C-board"));
    let mut src = seeded_source();
    src.mark_erased(&channel_ref());
    src.mark_erased(&myelin_refs::strip_sub(&message_ref()));
    let p = Projector::new(id, src, templates());

    for reference in [channel_ref(), message_ref()] {
        match p
            .project(&reference, &viewer("alice"), Zookie(String::new()))
            .unwrap()
        {
            Projected::Tombstoned(t) => assert_eq!(t.reason, TombstoneReason::Erased),
            other => panic!(
                "expected an Erased tombstone for {}, got {other:?}",
                reference.0
            ),
        }
    }
}

#[test]
fn restricted_subject_tombstones() {
    let id = GateId::default();
    id.allow("alice", &channel_object("C-board"));
    let mut src = seeded_source();
    src.mark_restricted(&myelin_refs::strip_sub(&message_ref()));
    let p = Projector::new(id, src, templates());
    match p
        .project(&message_ref(), &viewer("alice"), Zookie(String::new()))
        .unwrap()
    {
        Projected::Tombstoned(t) => assert_eq!(t.reason, TombstoneReason::Erased),
        other => panic!("expected an Erased tombstone (restriction), got {other:?}"),
    }
}

#[test]
fn non_chat_ref_is_a_loud_error() {
    let id = GateId::default();
    let p = projector(id);
    let issue = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
    match p.project(&issue, &viewer("alice"), Zookie(String::new())) {
        Err(ProjectError::NotChat { subsystem }) => assert_eq!(subsystem, "issue"),
        other => panic!("expected NotChat, got {other:?}"),
    }
}

#[test]
fn unknown_chat_type_is_a_loud_error() {
    let id = GateId::default();
    let p = projector(id);
    let read_state = myelin_refs::parse("myelin://acme/chat/read_state/rs1").unwrap();
    match p.project(&read_state, &viewer("alice"), Zookie(String::new())) {
        Err(ProjectError::UnknownChatType { ty }) => assert_eq!(ty, "read_state"),
        other => panic!("expected UnknownChatType, got {other:?}"),
    }
}

#[test]
fn chat_is_the_densest_edge_producer_zero_missing_edges() {
    let src = message_ref();
    let alice = viewer("alice");
    let issue = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
    let page = ArtifactRef("myelin://acme/knowledge/page/7c2".into());

    let corpus: Vec<MessageBody> = vec![
        paragraph_body(
            &format!("hi {OBJ} see {OBJ} and {OBJ}"),
            vec![
                InlineNode::Mention(alice.clone()),
                InlineNode::ArtifactRefNode(issue.clone()),
                InlineNode::Embed(page.clone()),
            ],
        ),
        paragraph_body(
            &format!("ping {OBJ}"),
            vec![InlineNode::Mention(alice.clone())],
        ),
        paragraph_body("see myelin://acme/issue/ENG-1 and ping @alice", vec![]),
    ];

    let total = densest_edge_producer(&src, &corpus);
    let expected: usize = corpus.iter().map(|b| b.structured_nodes().len()).sum();
    assert_eq!(
        total, expected,
        "0 missing edges: one edge per structured node"
    );
    assert_eq!(total, 4, "3 + 1 + 0 structured nodes → 4 edges");
}
