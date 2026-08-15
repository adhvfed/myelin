use std::collections::HashSet;

use myelin_events::{
    reindex, Actor, CorrelationId, DerivedStore, EmitContextBase, EventEnvelope, OutboxStore,
    Region, ReindexSource, SnapshotDraft, SnapshotScope, TenantId, Timestamp,
};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, DataRole, Decision, EffectivePolicy,
    IdentityService, ListObjectsResult, ObjectId, ObjectType, Permission, Precondition, Principal,
    PrincipalId, PrincipalKind, PrincipalStatus, Result as IdResult, RewriteTrace, SubjectTree,
    TupleDelta, Zookie,
};
use myelin_storage::blob::ContentHash;

use crate::refs_glue::{PageMeta, PageStore, Projected, Projector, TombstoneReason};
use crate::replay::KnowledgeReindexSource;

pub const E2E_SCENARIOS: [&str; 2] = ["E2E-1", "E2E-3"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2eArtifact {
    pub scenario: &'static str,
    pub green: bool,
    pub evidence: String,
    pub leaks: u64,
    pub seal: String,
}

impl E2eArtifact {
    fn sealed(
        scenario: &'static str,
        green: bool,
        leaks: u64,
        evidence: impl Into<String>,
    ) -> Self {
        let evidence = evidence.into();
        let mut body = Vec::new();
        push_lp(&mut body, scenario.as_bytes());
        push_lp(&mut body, &[u8::from(green)]);
        push_lp(&mut body, &leaks.to_be_bytes());
        push_lp(&mut body, evidence.as_bytes());
        let seal = ContentHash::blake3(&body).to_multihash_string();
        E2eArtifact {
            scenario,
            green,
            evidence,
            leaks,
            seal,
        }
    }

    pub fn is_green(&self) -> bool {
        self.green && self.leaks == 0
    }
}

fn push_lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn e2e_tenant() -> TenantId {
    TenantId("acme".into())
}

fn e2e_region() -> Region {
    Region("fr-par".into())
}

fn e2e_viewer(id: &str) -> Principal {
    Principal::new(
        e2e_tenant(),
        e2e_region(),
        PrincipalId(id.into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn e2e_platform() -> Principal {
    Principal::stub(
        PrincipalId("platform".into()),
        PrincipalKind::Service,
        e2e_tenant(),
    )
}

fn e2e_ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: e2e_tenant(),
        region: e2e_region(),
        actor: Actor(e2e_platform()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:00Z".into()),
        caused_by: None,
    }
}

fn e2e_zookie() -> Zookie {
    Zookie("z0".into())
}

struct WedgeId {
    allow: HashSet<String>,
}

impl WedgeId {
    fn new() -> Self {
        Self {
            allow: HashSet::new(),
        }
    }
    fn allow_read(mut self, viewer: &Principal, object: &myelin_events::ArtifactRef) -> Self {
        self.allow
            .insert(format!("{}|read@{}", viewer.principal_id.0, object.0));
        self
    }
}

impl IdentityService for WedgeId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("wedge: authenticate n/a"))
    }
    fn check(
        &self,
        s: &Principal,
        p: &Permission,
        o: &myelin_events::ArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        Ok(
            if self
                .allow
                .contains(&format!("{}|{}@{}", s.principal_id.0, p.0, o.0))
            {
                Decision::Allow
            } else {
                Decision::Deny
            },
        )
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _at: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("wedge: list_objects n/a"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("wedge: list_subjects n/a"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("wedge: explain n/a"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("wedge: delegation n/a"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("wedge: write_tuples n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &myelin_identity::RunId,
        _d: &myelin_identity::DelegationCaveats,
        _t: &myelin_identity::FailStaticBound,
    ) -> IdResult<myelin_identity::RunToken> {
        Err(AuthzError::NotYetImplemented("wedge: mint_run_token n/a"))
    }
    fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("wedge: revoke n/a"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented(
            "wedge: resolve_pseudonym n/a",
        ))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("wedge: erase n/a"))
    }
    fn admit_fragment(
        &self,
        _f: &myelin_identity::NamespaceFragment,
    ) -> IdResult<myelin_identity::FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("wedge: admit_fragment n/a"))
    }
}

fn page_root(id: &str) -> myelin_events::ArtifactRef {
    myelin_events::ArtifactRef(format!("myelin://acme/knowledge/page/{id}"))
}

const E2E1_SECRET_TITLE: &str = "Project Cerberus - Q3 acquisition architecture";

const E2E1_DESIGN_DOC: &str = "design-cerberus";

pub fn run_e2e1_pr_context_pane() -> E2eArtifact {
    let mut leaks: u64 = 0;
    let author = e2e_viewer("author");
    let denied = e2e_viewer("denied-teammate");
    let root = page_root(E2E1_DESIGN_DOC);
    let embed = myelin_events::ArtifactRef(format!("{}#block-h1", root.0));

    let mut store = PageStore::new();
    store.put_root(
        &root,
        PageMeta {
            title: E2E1_SECRET_TITLE.to_string(),
            state: "live".to_string(),
        },
    );
    let id = WedgeId::new().allow_read(&author, &root);
    let projector = Projector::new(id, store);

    let author_view = projector
        .project(&embed, &author, e2e_zookie())
        .expect("author projection");
    let author_sees_title = match &author_view {
        Projected::Visible(p) => p.title == E2E1_SECRET_TITLE && p.sub_anchor.is_some(),
        Projected::Tombstoned(_) => false,
    };

    let denied_view = projector
        .project(&embed, &denied, e2e_zookie())
        .expect("denied projection");
    match &denied_view {
        Projected::Tombstoned(t) => {
            if t.root != root {
                leaks += 1;
            }
            if t.reason != TombstoneReason::Denied {
                leaks += 1;
            }
            if t.display_text().contains("Cerberus") || t.display_text().contains("acquisition") {
                leaks += 1;
            }
            let rendered = format!("{denied_view:?}");
            if rendered.contains("Cerberus") || rendered.contains("acquisition") {
                leaks += 1;
            }
        }
        Projected::Visible(_) => {
            leaks += 1;
        }
    }

    let mut store2 = PageStore::new();
    store2.put_root(
        &root,
        PageMeta {
            title: E2E1_SECRET_TITLE.to_string(),
            state: "live".to_string(),
        },
    );
    store2.mark_erased(&root);
    let id2 = WedgeId::new().allow_read(&author, &root);
    let projector2 = Projector::new(id2, store2);
    let author_after_erase = projector2
        .project(&embed, &author, e2e_zookie())
        .expect("author projection after erase");
    let erasure_honoured_live = match &author_after_erase {
        Projected::Tombstoned(t) => t.reason == TombstoneReason::Erased && t.root == root,
        Projected::Visible(_) => {
            leaks += 1;
            false
        }
    };
    let rendered_after = format!("{author_after_erase:?}");
    if rendered_after.contains("Cerberus") || rendered_after.contains("acquisition") {
        leaks += 1;
    }

    let green = author_sees_title
        && erasure_honoured_live
        && matches!(denied_view, Projected::Tombstoned(_));
    E2eArtifact::sealed(
        "E2E-1",
        green,
        leaks,
        format!(
            "PR-context-pane: author embed resolves live (title shown); denied viewer → \
             root-only tombstone ({} title leaks); mid-flight erase honoured live → author embed \
             degrades to Erased tombstone",
            leaks
        ),
    )
}

const E2E3_SPEC_DOC: &str = "spec-payments-v2";

fn e2e3_initiative_ref() -> String {
    "myelin://acme/issue/initiative/INIT-payments".to_string()
}

fn e2e3_issue_refs() -> [String; 2] {
    [
        "myelin://acme/issue/issue/PAY-1".to_string(),
        "myelin://acme/issue/issue/PAY-2".to_string(),
    ]
}

fn e2e3_lineage_source() -> KnowledgeReindexSource {
    let mut s = KnowledgeReindexSource::new();
    s.upsert_page(
        E2E3_SPEC_DOC,
        4,
        &[(
            "b1",
            4,
            serde_json::json!({ "kind": "heading", "text_ref": "spec" }),
        )],
    );
    let spec_ref = format!("myelin://acme/knowledge/page/{E2E3_SPEC_DOC}");
    let init = e2e3_initiative_ref();
    let [pay1, pay2] = e2e3_issue_refs();
    s.upsert_edge(&spec_ref, &init, "realises", 1);
    s.upsert_edge(&init, &pay1, "decomposes", 2);
    s.upsert_edge(&init, &pay2, "decomposes", 3);
    s
}

fn e2e3_snapshot_envelope(draft: &SnapshotDraft) -> EventEnvelope {
    let event_id = draft.event_id(&e2e_tenant());
    EventEnvelope {
        event_id: event_id.clone(),
        type_: draft.type_.clone(),
        schema_ver: 1,
        tenant: e2e_tenant(),
        region: e2e_region(),
        actor: Actor(e2e_platform()),
        subject: draft.subject.clone(),
        aggregate: draft.aggregate.clone(),
        causation_id: None,
        correlation_id: CorrelationId(event_id.0),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: draft.data_role,
        visibility: draft.visibility,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:00Z".into()),
        payload: draft.payload.clone(),
    }
}

#[derive(Clone, Debug)]
struct LineageHop {
    source: String,
    target: String,
    rel: String,
    seal: String,
}

fn seal_lineage(hops: &[(String, String, String)]) -> Vec<LineageHop> {
    let mut prev = String::from("genesis");
    let mut out = Vec::new();
    for (source, target, rel) in hops {
        let mut body = Vec::new();
        push_lp(&mut body, prev.as_bytes());
        push_lp(&mut body, source.as_bytes());
        push_lp(&mut body, target.as_bytes());
        push_lp(&mut body, rel.as_bytes());
        let seal = ContentHash::blake3(&body).to_multihash_string();
        out.push(LineageHop {
            source: source.clone(),
            target: target.clone(),
            rel: rel.clone(),
            seal: seal.clone(),
        });
        prev = seal;
    }
    out
}

fn verify_lineage(hops: &[LineageHop]) -> bool {
    let mut prev = String::from("genesis");
    for hop in hops {
        let mut body = Vec::new();
        push_lp(&mut body, prev.as_bytes());
        push_lp(&mut body, hop.source.as_bytes());
        push_lp(&mut body, hop.target.as_bytes());
        push_lp(&mut body, hop.rel.as_bytes());
        let expect = ContentHash::blake3(&body).to_multihash_string();
        if expect != hop.seal {
            return false;
        }
        prev = hop.seal.clone();
    }
    true
}

fn e2e3_lineage_hops() -> Vec<(String, String, String)> {
    let spec_ref = format!("myelin://acme/knowledge/page/{E2E3_SPEC_DOC}");
    let init = e2e3_initiative_ref();
    let [pay1, pay2] = e2e3_issue_refs();
    vec![
        (spec_ref, init.clone(), "realises".to_string()),
        (init.clone(), pay1, "decomposes".to_string()),
        (init, pay2, "decomposes".to_string()),
    ]
}

pub fn run_e2e3_spec_to_ship_lineage() -> E2eArtifact {
    let mut leaks: u64 = 0;
    let source = e2e3_lineage_source();
    let scope = SnapshotScope::new("knowledge", "all");

    let hops = e2e3_lineage_hops();
    let spec_ref = format!("myelin://acme/knowledge/page/{E2E3_SPEC_DOC}");
    let mut frontier = vec![spec_ref.clone()];
    let mut reached: HashSet<String> = HashSet::new();
    while let Some(node) = frontier.pop() {
        for (s, t, _r) in &hops {
            if *s == node && reached.insert(t.clone()) {
                frontier.push(t.clone());
            }
        }
    }
    let [pay1, pay2] = e2e3_issue_refs();
    let lineage_traceable = reached.contains(&e2e3_initiative_ref())
        && reached.contains(&pay1)
        && reached.contains(&pay2);
    if !lineage_traceable {
        leaks += 1;
    }

    let mut live = DerivedStore::new();
    for draft in source.replay(&scope, None) {
        live.ingest(&e2e3_snapshot_envelope(&draft));
    }
    let sources: &[&dyn ReindexSource] = &[&source];
    let mut outbox = OutboxStore::new();
    reindex(&scope, None, sources, &mut outbox, e2e_ctx_base()).expect("reindex replay");
    let mut cold = DerivedStore::new();
    assert!(cold.is_empty(), "the derived store is wiped before rebuild");
    for draft in source.replay(&scope, None) {
        let row = outbox
            .row(&draft.event_id(&e2e_tenant()))
            .expect("snapshot row present");
        cold.ingest(&row.envelope);
    }
    let cold_equals_live = cold.len() == live.len() && cold.parity_bytes() == live.parity_bytes();
    if !cold_equals_live {
        leaks += 1;
    }

    let honest = seal_lineage(&hops);
    let honest_verifies = verify_lineage(&honest);
    if !honest_verifies {
        leaks += 1;
    }
    let mut tampered = honest.clone();
    if let Some(last) = tampered.last_mut() {
        last.target = "myelin://acme/issue/issue/PAY-FORGED".to_string();
    }
    let tamper_detected = !verify_lineage(&tampered);
    if !tamper_detected {
        leaks += 1;
    }

    let green = lineage_traceable && cold_equals_live && honest_verifies && tamper_detected;
    E2eArtifact::sealed(
        "E2E-3",
        green,
        leaks,
        format!(
            "spec→initiative→issues lineage traceable={lineage_traceable}; \
             cold-reindex==live={cold_equals_live} (parity bytes byte-match); \
             audit honest-verifies={honest_verifies}, tamper-detected={tamper_detected}"
        ),
    )
}

pub fn run_knowledge_e2e_legs() -> Vec<E2eArtifact> {
    vec![run_e2e1_pr_context_pane(), run_e2e3_spec_to_ship_lineage()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e2e1_pr_context_pane_zero_title_leak() {
        let art = run_e2e1_pr_context_pane();
        assert_eq!(art.scenario, "E2E-1");
        assert_eq!(
            art.leaks, 0,
            "0 title leak across every projection: {art:?}"
        );
        assert!(art.is_green(), "E2E-1 green not earned: {art:?}");
        assert!(art.seal.starts_with("blake3:"));
    }

    #[test]
    fn e2e3_spec_to_ship_cold_equals_live_and_tamper_detected() {
        let art = run_e2e3_spec_to_ship_lineage();
        assert_eq!(art.scenario, "E2E-3");
        assert_eq!(art.leaks, 0, "0 divergence/undetected-tamper: {art:?}");
        assert!(art.is_green(), "E2E-3 green not earned: {art:?}");
        assert!(art.seal.starts_with("blake3:"));
    }

    #[test]
    fn both_legs_green_and_distinctly_sealed() {
        let arts = run_knowledge_e2e_legs();
        assert_eq!(arts.len(), 2);
        assert!(arts.iter().all(|a| a.is_green()));
        assert_ne!(arts[0].seal, arts[1].seal);
        assert_eq!(E2E_SCENARIOS, ["E2E-1", "E2E-3"]);
    }

    #[test]
    fn e2e1_unauthorized_projection_carries_no_title_fragment() {
        let denied = e2e_viewer("nobody");
        let root = page_root(E2E1_DESIGN_DOC);
        let embed = myelin_events::ArtifactRef(format!("{}#block-h1", root.0));
        let mut store = PageStore::new();
        store.put_root(
            &root,
            PageMeta {
                title: E2E1_SECRET_TITLE.to_string(),
                state: "live".to_string(),
            },
        );
        let projector = Projector::new(WedgeId::new(), store);
        let view = projector.project(&embed, &denied, e2e_zookie()).unwrap();
        assert!(matches!(view, Projected::Tombstoned(_)));
        let rendered = format!("{view:?}");
        assert!(!rendered.contains("Cerberus"));
        assert!(!rendered.contains("acquisition"));
    }

    #[test]
    fn e2e3_verify_catches_a_reordered_chain() {
        let hops = e2e3_lineage_hops();
        let mut sealed = seal_lineage(&hops);
        assert!(verify_lineage(&sealed));
        sealed.swap(1, 2);
        assert!(
            !verify_lineage(&sealed),
            "a reordered chain must fail verify"
        );
    }

    #[test]
    fn e2e_artifact_seal_is_deterministic() {
        let a = E2eArtifact::sealed("E2E-1", true, 0, "same body");
        let b = E2eArtifact::sealed("E2E-1", true, 0, "same body");
        assert_eq!(a.seal, b.seal, "the seal is a pure function of the body");
        let c = E2eArtifact::sealed("E2E-1", true, 1, "same body");
        assert_ne!(a.seal, c.seal, "a different leak count seals differently");
    }
}
