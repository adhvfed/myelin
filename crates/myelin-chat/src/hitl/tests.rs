use super::*;
use myelin_identity::{
    AuthzError, CaveatContext, Consistency as IdConsistency, Decision, FailStaticBound,
    IdentityService, ListObjectsResult, ObjectId, ObjectType, Permission, Principal, PrincipalId,
    PrincipalKind, RewriteTrace, RunId as IdRunId, RunToken, SubjectTree, TupleDelta, Zookie,
};
use myelin_notif::{
    RefProjection, RefResolution, RefResolvePort, TemplateStore, Tombstone, TombstoneReason,
    DEFAULT_LOCALE,
};
use myelin_tenancy::{ArtifactRef as TArtifactRef, Region, TenantId};
use std::cell::RefCell;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn human(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

struct MockId {
    allow: Decision,
    err_check: bool,
    minted: RefCell<Vec<(String, String)>>,
}

impl MockId {
    fn allowing() -> MockId {
        MockId {
            allow: Decision::Allow,
            err_check: false,
            minted: RefCell::new(vec![]),
        }
    }
    fn denying() -> MockId {
        MockId {
            allow: Decision::Deny,
            err_check: false,
            minted: RefCell::new(vec![]),
        }
    }
    fn erroring() -> MockId {
        MockId {
            allow: Decision::Allow,
            err_check: true,
            minted: RefCell::new(vec![]),
        }
    }
    fn conditional() -> MockId {
        MockId {
            allow: Decision::Conditional,
            err_check: false,
            minted: RefCell::new(vec![]),
        }
    }
}

impl IdentityService for MockId {
    fn authenticate(
        &self,
        _credential: &myelin_identity::Credential,
    ) -> myelin_identity::Result<Principal> {
        Err(AuthzError::NotYetImplemented("mock"))
    }
    fn check(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &TArtifactRef,
        _at: &IdConsistency,
        _caveat: Option<&CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        if self.err_check {
            return Err(AuthzError::Unavailable("mock id hiccup".into()));
        }
        Ok(self.allow)
    }
    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &IdConsistency,
    ) -> myelin_identity::Result<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("mock"))
    }
    fn list_subjects(
        &self,
        _object: &ObjectId,
        _permission: &Permission,
        _at: &IdConsistency,
    ) -> myelin_identity::Result<SubjectTree> {
        Err(AuthzError::NotYetImplemented("mock"))
    }
    fn explain(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &ObjectId,
        _at: &IdConsistency,
    ) -> myelin_identity::Result<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("mock"))
    }
    fn delegation(
        &self,
        _agent: &Principal,
        _trigger_actor: &Principal,
    ) -> myelin_identity::Result<myelin_identity::EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("mock"))
    }
    fn write_tuples(
        &self,
        _deltas: &[TupleDelta],
        _precondition: Option<&myelin_identity::Precondition>,
    ) -> myelin_identity::Result<Zookie> {
        Err(AuthzError::NotYetImplemented("mock"))
    }
    fn mint_run_token(
        &self,
        agent_id: &PrincipalId,
        run_id: &IdRunId,
        _delegation_caveats: &DelegationCaveats,
        ttl: &FailStaticBound,
    ) -> myelin_identity::Result<RunToken> {
        assert_eq!(
            ttl.static_max_secs,
            FailStaticBound::DEFAULT_W.static_max_secs
        );
        self.minted
            .borrow_mut()
            .push((agent_id.0.clone(), run_id.0.clone()));
        Ok(RunToken {
            token: format!("fresh-token-for-{}", run_id.0),
            jti: format!("jti-{}", run_id.0),
        })
    }
    fn revoke(&self, _target: &myelin_identity::RevokeTarget) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("mock"))
    }
    fn resolve_pseudonym(
        &self,
        _subject: &PrincipalId,
        _tenant: &TenantId,
    ) -> myelin_identity::Result<String> {
        Err(AuthzError::NotYetImplemented("mock"))
    }
    fn erase(&self, _subject: &PrincipalId) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("mock"))
    }
    fn admit_fragment(
        &self,
        _fragment: &myelin_identity::NamespaceFragment,
    ) -> myelin_identity::Result<myelin_identity::FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("mock"))
    }
}

#[derive(Default)]
struct RecordingPort {
    posted: RefCell<Vec<CardSignal>>,
}

impl SignalPort for RecordingPort {
    fn post_signal(&self, signal: &CardSignal) -> Result<SignalDelivery, SignalPostError> {
        let mut posted = self.posted.borrow_mut();
        let dup = posted.iter().any(|s| {
            s.run_id == signal.run_id
                && s.signal_name == signal.signal_name
                && s.idem_key == signal.idem_key
        });
        if dup {
            return Ok(SignalDelivery::Duplicate);
        }
        posted.push(signal.clone());
        Ok(SignalDelivery::Buffered)
    }
}

impl RecordingPort {
    fn count(&self) -> usize {
        self.posted.borrow().len()
    }
    fn applies(&self) -> usize {
        self.posted
            .borrow()
            .iter()
            .filter(|s| s.payload_key_ref.is_none())
            .count()
    }
    fn withholds(&self) -> usize {
        self.posted
            .borrow()
            .iter()
            .filter(|s| s.payload_key_ref.is_some())
            .count()
    }
}

fn effect(subject: &str, action: &str, refs: &[&str]) -> CardEffect {
    CardEffect {
        subject: TArtifactRef(subject.into()),
        action: action.into(),
        risk: "irreversible".into(),
        cost: "$0.40".into(),
        effect_refs: refs.iter().map(|r| TArtifactRef((*r).into())).collect(),
    }
}

fn single_effect_card() -> ChatApprovalCard {
    ChatApprovalCard {
        run_id: RunId("R1".into()),
        card_id: "card-1".into(),
        effects: vec![effect(
            "myelin://acme/git/pr/88",
            "merge",
            &["myelin://acme/agent/effect/merge-88"],
        )],
    }
}

fn three_effect_card() -> ChatApprovalCard {
    ChatApprovalCard {
        run_id: RunId("R7".into()),
        card_id: "card-7".into(),
        effects: vec![
            effect("myelin://acme/git/pr/88", "open PR #88", &["e0"]),
            effect("myelin://acme/issue/412", "link ENG-412", &["e1"]),
            effect(
                "myelin://acme/chat/channel/incidents",
                "post #incidents",
                &["e2"],
            ),
        ],
    }
}

#[test]
fn per_effect_key_single_vs_multi() {
    assert_eq!(per_effect_idem_key("card-1", 0, 1), "card-1");
    assert_eq!(per_effect_idem_key("card-7", 0, 3), "card-7:0");
    assert_eq!(per_effect_idem_key("card-7", 1, 3), "card-7:1");
    assert_eq!(per_effect_idem_key("card-7", 2, 3), "card-7:2");
    assert_eq!(single_effect_card().idem_key_for(0), "card-1");
    assert_eq!(three_effect_card().idem_key_for(1), "card-7:1");
}

#[test]
fn signal_name_is_approval_colon_card() {
    assert_eq!(approval_signal_name("card-7"), "approval:card-7");
    assert_eq!(three_effect_card().signal_name(), "approval:card-7");
}

#[test]
fn a_declined_effect_carries_no_payload_so_the_engine_never_applies_it() {
    let card = single_effect_card();
    let click = CardClick {
        effect_idx: 0,
        decision: CardDecision::Decline,
        decline_reason: DECLINE_MARKER.to_string(),
    };
    let sig = build_card_signal(&card, &click);
    assert!(
        sig.payload.is_empty(),
        "a withheld effect carries no payload"
    );
    assert_eq!(sig.payload_key_ref.as_deref(), Some(DECLINE_MARKER));
}

#[test]
fn an_approved_effect_carries_its_apply_refs() {
    let card = single_effect_card();
    let click = CardClick {
        effect_idx: 0,
        decision: CardDecision::Approve,
        decline_reason: String::new(),
    };
    let sig = build_card_signal(&card, &click);
    assert_eq!(
        sig.payload,
        vec![TArtifactRef("myelin://acme/agent/effect/merge-88".into())]
    );
    assert_eq!(sig.payload_key_ref, None);
}

#[test]
fn double_click_approve_is_one_approval() {
    let card = single_effect_card();
    let gate = ClickGate::new(MockId::allowing());
    let port = RecordingPort::default();
    let approve = CardClick {
        effect_idx: 0,
        decision: CardDecision::Approve,
        decline_reason: String::new(),
    };
    let o1 = post_decision(&gate, &port, &card, &approve, &human("alice"), None).unwrap();
    assert_eq!(o1, CardOutcome::Approved(SignalDelivery::Buffered));
    let o2 = post_decision(&gate, &port, &card, &approve, &human("alice"), None).unwrap();
    assert_eq!(o2, CardOutcome::Approved(SignalDelivery::Duplicate));
    assert_eq!(
        port.count(),
        1,
        "a double-click is one approval (0 double-apply)"
    );
    assert_eq!(port.applies(), 1);
}

#[test]
fn partial_approval_two_of_three_applies_two_withholds_one_independently() {
    let card = three_effect_card();
    let gate = ClickGate::new(MockId::allowing());
    let port = RecordingPort::default();
    let alice = human("alice");
    let o0 = post_decision(
        &gate,
        &port,
        &card,
        &CardClick {
            effect_idx: 0,
            decision: CardDecision::Approve,
            decline_reason: String::new(),
        },
        &alice,
        None,
    )
    .unwrap();
    let o1 = post_decision(
        &gate,
        &port,
        &card,
        &CardClick {
            effect_idx: 1,
            decision: CardDecision::Decline,
            decline_reason: DECLINE_MARKER.to_string(),
        },
        &alice,
        None,
    )
    .unwrap();
    let o2 = post_decision(
        &gate,
        &port,
        &card,
        &CardClick {
            effect_idx: 2,
            decision: CardDecision::Approve,
            decline_reason: String::new(),
        },
        &alice,
        None,
    )
    .unwrap();
    assert_eq!(o0, CardOutcome::Approved(SignalDelivery::Buffered));
    assert!(matches!(
        o1,
        CardOutcome::Withheld(SignalDelivery::Buffered, _)
    ));
    assert_eq!(o2, CardOutcome::Approved(SignalDelivery::Buffered));
    let posted = port.posted.borrow();
    assert_eq!(posted.len(), 3);
    assert_eq!(posted[0].idem_key, "card-7:0");
    assert_eq!(posted[1].idem_key, "card-7:1");
    assert_eq!(posted[2].idem_key, "card-7:2");
    drop(posted);
    assert_eq!(port.applies(), 2, "exactly two effects approved");
    assert_eq!(
        port.withholds(),
        1,
        "exactly one effect withheld - 0 mutation"
    );
}

#[test]
fn double_click_approve_all_applies_each_once() {
    let card = three_effect_card();
    let gate = ClickGate::new(MockId::allowing());
    let port = RecordingPort::default();
    let alice = human("alice");
    for _round in 0..2 {
        for idx in 0..3 {
            post_decision(
                &gate,
                &port,
                &card,
                &CardClick {
                    effect_idx: idx,
                    decision: CardDecision::Approve,
                    decline_reason: String::new(),
                },
                &alice,
                None,
            )
            .unwrap();
        }
    }
    assert_eq!(
        port.count(),
        3,
        "approve-all double-click → 3 applies, not 6"
    );
    assert_eq!(port.applies(), 3);
}

#[test]
fn deny_conditional_and_id_error_all_fail_closed_and_post_nothing() {
    let card = single_effect_card();
    let approve = CardClick {
        effect_idx: 0,
        decision: CardDecision::Approve,
        decline_reason: String::new(),
    };
    for id in [MockId::denying(), MockId::conditional(), MockId::erroring()] {
        let gate = ClickGate::new(id);
        let port = RecordingPort::default();
        let res = post_decision(&gate, &port, &card, &approve, &human("mallory"), None);
        assert!(
            matches!(res, Err(PostDecisionError::Denied(_))),
            "a non-Allow verdict fails the click gate (fail-closed)"
        );
        assert_eq!(port.count(), 0, "a denied click posts NO signal");
    }
}

#[test]
fn an_allowed_clicker_passes_the_gate() {
    let card = single_effect_card();
    let gate = ClickGate::new(MockId::allowing());
    assert!(gate
        .check_click(&human("alice"), &card, Some("zk-1"))
        .is_ok());
}

#[test]
fn timeout_auto_denies_with_the_timeout_marker_zero_mutation() {
    let card = single_effect_card();
    let click = auto_deny_on_timeout(0);
    assert_eq!(click.decision, CardDecision::Decline);
    assert_eq!(click.decline_reason, TIMEOUT_REASON);
    let sig = build_card_signal(&card, &click);
    assert!(sig.payload.is_empty());
    assert_eq!(sig.payload_key_ref.as_deref(), Some(TIMEOUT_REASON));
}

#[test]
fn resume_token_is_freshly_minted_with_the_w_bounded_ttl() {
    let id = MockId::allowing();
    let minter = ResumeTokenMinter::new(id);
    let token = minter
        .mint_resume_token(
            &PrincipalId("agent-x".into()),
            &RunId("R7".into()),
            &DelegationCaveats(vec!["scope:merge".into()]),
        )
        .unwrap();
    assert_eq!(token.token, "fresh-token-for-R7");
    assert_eq!(token.jti, "jti-R7");
    assert_eq!(minter.id.minted.borrow().len(), 1);
    assert_eq!(
        minter.id.minted.borrow()[0],
        ("agent-x".into(), "R7".into())
    );
}

struct PerViewerResolver;
impl RefResolvePort for PerViewerResolver {
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &TArtifactRef,
        viewer: &Principal,
        _at: &IdConsistency,
    ) -> RefResolution {
        if viewer.principal_id.0 == "alice" {
            RefResolution::Projection(RefProjection {
                ref_: ref_.clone(),
                title: "PR #88: fix the leak".into(),
                icon: "pr".into(),
            })
        } else {
            RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            })
        }
    }
}

fn card_templates() -> TemplateStore {
    let mut store = TemplateStore::with_platform_defaults();
    for row in crate::glue::chat_humanise_templates() {
        store.put(row);
    }
    store
}

#[test]
fn card_renders_per_viewer_subject_leak_free_with_facets() {
    let templates = card_templates();
    let card = single_effect_card();
    let at = IdConsistency {
        at_least: Zookie("zk".into()),
        mode: ConsistencyMode::Strong,
    };
    let alice = render_card(
        &PerViewerResolver,
        &templates,
        &tenant(),
        &region(),
        &card,
        0,
        &human("alice"),
        DEFAULT_LOCALE,
        &at,
        Channel::Markdown,
    );
    assert!(
        alice.subject_line.text.contains("PR #88"),
        "the allowed viewer sees the title: {}",
        alice.subject_line.text
    );
    assert!(alice.facets_line.contains("merge"));
    assert!(alice.facets_line.contains("irreversible"));
    assert!(alice.facets_line.contains("$0.40"));
    assert_eq!(alice.idem_key, "card-1");

    let bob = render_card(
        &PerViewerResolver,
        &templates,
        &tenant(),
        &region(),
        &card,
        0,
        &human("bob"),
        DEFAULT_LOCALE,
        &at,
        Channel::Markdown,
    );
    assert!(
        !bob.subject_line.text.contains("PR #88"),
        "the denied viewer NEVER sees the title (NOTIF-D4): {}",
        bob.subject_line.text
    );
    assert_ne!(alice.subject_line.text, bob.subject_line.text);
    assert_eq!(alice.facets_line, bob.facets_line);
}
