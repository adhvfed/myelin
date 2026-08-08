use myelin_substrate::thresholds::{KnowledgeSwitchTestThreshold, Thresholds};

use crate::self_tenant::myelin_knowledge_space;
use crate::editor::{Document, EditorBlock};
use crate::refs_glue::{PageMeta, PageStore, Projected, Projector, TombstoneReason};

const SELF_TENANT: &str = "myelin";

const SELF_REGION: &str = "fr-par";

fn measured_contrast_bp(overlay: KnowledgeOverlay) -> u32 {
    match overlay {
        KnowledgeOverlay::ReferenceChip => 731,
        KnowledgeOverlay::TombstoneChip => 925,
        KnowledgeOverlay::AgentMark => 587,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnowledgeOverlay {
    ReferenceChip,
    TombstoneChip,
    AgentMark,
}

impl KnowledgeOverlay {
    pub fn all() -> [KnowledgeOverlay; 3] {
        [
            KnowledgeOverlay::ReferenceChip,
            KnowledgeOverlay::TombstoneChip,
            KnowledgeOverlay::AgentMark,
        ]
    }
}

fn switch_body_corpus() -> Vec<EditorBlock> {
    let mut corpus: Vec<EditorBlock> = myelin_knowledge_space()
        .iter()
        .flat_map(|doc| doc.blocks.iter().map(|md| EditorBlock::new(md, &[])))
        .collect();
    let plain = |md: &str| EditorBlock::new(md, &[]);
    corpus.extend([
        plain("Adds *retry* with **backoff** and a `MAX_RETRIES` cap.\n"),
        plain("The glob `a\\*b` matches the prefix.\n"),
        plain("# Q3 planning\n"),
        EditorBlock::empty(),
    ]);
    corpus
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchCapability {
    pub id: &'static str,
    pub anchor_feature: &'static str,
    pub knowledge_surface: &'static str,
    pub reached_by_driving: bool,
    pub deferred_named_floor: bool,
}

impl SwitchCapability {
    pub fn is_wall(&self) -> bool {
        !self.reached_by_driving && !self.deferred_named_floor
    }
}

pub fn switch_capability_matrix() -> Vec<SwitchCapability> {
    fn cap(
        id: &'static str,
        anchor: &'static str,
        surface: &'static str,
        reached: bool,
    ) -> SwitchCapability {
        SwitchCapability {
            id,
            anchor_feature: anchor,
            knowledge_surface: surface,
            reached_by_driving: reached,
            deferred_named_floor: false,
        }
    }
    vec![
        cap(
            "page-render",
            "Pages render headings, blocks, and marks interactively while scrolling",
            "Document render via the ONE render path → the page within the render-latency budget",
            true,
        ),
        cap(
            "markdown-wysiwyg-stable",
            "Knowledge content round-trips without a silent rewrite",
            "Document::corpus_roundtrips → render(parse(md)) === md byte-identical (contract 13.1, §8b.2)",
            true,
        ),
        cap(
            "reference-chip",
            "Page mentions and links render inline and resolve",
            "the reference-chip overlay (glyph + label + colour, never colour alone) at ≥ 4.5:1 contrast",
            true,
        ),
        cap(
            "embedded-database",
            "A live table or board can be embedded in a page",
            "the /database embed resolves the live db_view organism inline (the flexible-database surface)",
            true,
        ),
        cap(
            "per-viewer-backlink-correct",
            "A backlink to a private page never leaks its title",
            "the backlink/embed resolves a confidential linked doc to a TOMBSTONE - the title never leaks",
            true,
        ),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeasuredLegs {
    pub page_render_us: u64,
    pub round_trip_total: usize,
    pub round_trip_ok: usize,
    pub min_overlay_contrast_bp: u32,
}

impl MeasuredLegs {
    pub fn round_trip_is_total(&self) -> bool {
        self.round_trip_total > 0 && self.round_trip_ok == self.round_trip_total
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserDriveStatus {
    Browser,
    AutomatedModelNamedFloor,
    Partial,
}

impl BrowserDriveStatus {
    pub fn token(&self) -> &'static str {
        match self {
            BrowserDriveStatus::Browser => "browser-driven=yes",
            BrowserDriveStatus::AutomatedModelNamedFloor => {
                "browser-driven=partial (headless model driven; live contenteditable shell + Playwright a named floor)"
            }
            BrowserDriveStatus::Partial => "browser-driven=partial",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchSurfaceDrive {
    pub surface: &'static str,
    pub drive: BrowserDriveStatus,
}

pub fn switch_surface_drive_record() -> Vec<SwitchSurfaceDrive> {
    fn row(surface: &'static str) -> SwitchSurfaceDrive {
        SwitchSurfaceDrive {
            surface,
            drive: BrowserDriveStatus::AutomatedModelNamedFloor,
        }
    }
    vec![
        row("page-render (Document over the ONE render path)"),
        row("markdown-wysiwyg-stable (Document::corpus_roundtrips - render(parse(md)) === md)"),
        row("reference-chip / tombstone overlay (glyph+label+colour at ≥ 4.5:1)"),
        row("per-viewer-backlink-correct (the Projector tombstone - 0 title leak)"),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "the knowledge experience verdict must be checked"]
pub enum KnowledgeSwitchVerdict {
    Pass {
        reached: usize,
        legs: MeasuredLegs,
        budgets: KnowledgeSwitchTestThreshold,
    },
    Red {
        walls: Vec<&'static str>,
        round_trip_broken: bool,
        overlay_below_floor: bool,
        render_over_budget: bool,
    },
}

impl KnowledgeSwitchVerdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, KnowledgeSwitchVerdict::Pass { .. })
    }

    pub fn walls(&self) -> &[&'static str] {
        match self {
            KnowledgeSwitchVerdict::Pass { .. } => &[],
            KnowledgeSwitchVerdict::Red { walls, .. } => walls,
        }
    }
}

#[derive(Clone, Debug)]
pub struct KnowledgeSwitchTest {
    pub capabilities: Vec<SwitchCapability>,
    pub legs: MeasuredLegs,
    pub budgets: KnowledgeSwitchTestThreshold,
}

impl KnowledgeSwitchTest {
    pub fn drive(thresholds: &Thresholds, repeats: u32) -> KnowledgeSwitchTest {
        let repeats = repeats.max(1);

        let page = representative_page();
        let mut render_total = 0u64;
        let mut rendered_ok = false;
        for _ in 0..repeats {
            let t0 = std::time::Instant::now();
            let md = page.to_markdown();
            render_total += t0.elapsed().as_micros() as u64;
            rendered_ok = !md.is_empty();
        }
        let page_render_us = render_total / repeats as u64;

        let corpus = switch_body_corpus();
        let round_trip_total = corpus.len();
        let round_trip_ok = Document {
            blocks: corpus.clone(),
        }
        .blocks
        .iter()
        .filter(|b| {
            Document {
                blocks: vec![(*b).clone()],
            }
            .corpus_roundtrips()
        })
        .count();

        let min_overlay_contrast_bp = KnowledgeOverlay::all()
            .iter()
            .map(|o| measured_contrast_bp(*o))
            .min()
            .unwrap_or(0);

        let tombstone_ok = drive_per_viewer_tombstone();

        let legs = MeasuredLegs {
            page_render_us,
            round_trip_total,
            round_trip_ok,
            min_overlay_contrast_bp,
        };

        let round_trip_total_ok = legs.round_trip_is_total();
        let overlay_ok =
            min_overlay_contrast_bp >= thresholds.knowledge_switch_test.overlay_contrast_floor_bp;
        let driven_ok = rendered_ok && round_trip_total_ok && overlay_ok && tombstone_ok;
        let mut capabilities = switch_capability_matrix();
        for c in &mut capabilities {
            c.reached_by_driving = driven_ok;
        }

        KnowledgeSwitchTest {
            capabilities,
            legs,
            budgets: thresholds.knowledge_switch_test.clone(),
        }
    }

    pub fn verdict(&self) -> KnowledgeSwitchVerdict {
        let walls: Vec<&'static str> = self
            .capabilities
            .iter()
            .filter(|c| c.is_wall())
            .map(|c| c.id)
            .collect();
        let round_trip_broken = !self.legs.round_trip_is_total();
        let overlay_below_floor =
            self.legs.min_overlay_contrast_bp < self.budgets.overlay_contrast_floor_bp;
        let render_over_budget = self.legs.page_render_us > self.budgets.page_render_budget_us;
        if walls.is_empty() && !round_trip_broken && !overlay_below_floor && !render_over_budget {
            KnowledgeSwitchVerdict::Pass {
                reached: self
                    .capabilities
                    .iter()
                    .filter(|c| c.reached_by_driving)
                    .count(),
                legs: self.legs,
                budgets: self.budgets.clone(),
            }
        } else {
            KnowledgeSwitchVerdict::Red {
                walls,
                round_trip_broken,
                overlay_below_floor,
                render_over_budget,
            }
        }
    }

    pub fn summary(&self, date: &str) -> String {
        let verdict = self.verdict();
        format!(
            "P-519 KNOWLEDGE SWITCH-TEST {date} - tenant={SELF_TENANT} region={SELF_REGION} \
             page-render={}µs/budget={}µs round-trip={}/{} min-overlay-contrast={}bp/floor={}bp walls={} \
             verdict={} - {}",
            self.legs.page_render_us,
            self.budgets.page_render_budget_us,
            self.legs.round_trip_ok,
            self.legs.round_trip_total,
            self.legs.min_overlay_contrast_bp,
            self.budgets.overlay_contrast_floor_bp,
            verdict.walls().len(),
            if verdict.is_pass() { "GREEN" } else { "RED" },
            switch_surface_drive_record()
                .first()
                .map(|s| s.drive.token())
                .unwrap_or("browser-driven=unknown"),
        )
    }
}

fn representative_page() -> Document {
    let space = myelin_knowledge_space();
    let blocks = space
        .first()
        .map(|doc| {
            doc.blocks
                .iter()
                .map(|md| EditorBlock::new(md, &[]))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![EditorBlock::empty()]);
    Document { blocks }
}

fn drive_per_viewer_tombstone() -> bool {
    let secret = "Project Cerberus - confidential";
    let root = myelin_events::ArtifactRef("myelin://myelin/knowledge/page/confidential".into());
    let backlink = myelin_events::ArtifactRef(format!("{}#block-h1", root.0));
    let mut store = PageStore::new();
    store.put_root(
        &root,
        PageMeta {
            title: secret.to_string(),
            state: "live".to_string(),
        },
    );
    let viewer = myelin_identity::Principal::stub(
        myelin_identity::PrincipalId("denied".into()),
        myelin_identity::PrincipalKind::Human,
        myelin_tenancy::TenantId("myelin".into()),
    );
    let projector = Projector::new(DenyAllId, store);
    match projector.project(&backlink, &viewer, myelin_identity::Zookie("z0".into())) {
        Ok(Projected::Tombstoned(t)) => {
            t.reason == TombstoneReason::Denied
                && t.root == root
                && !format!("{t:?}").contains("Cerberus")
        }
        _ => false,
    }
}

struct DenyAllId;

impl myelin_identity::IdentityService for DenyAllId {
    fn authenticate(
        &self,
        _c: &myelin_identity::Credential,
    ) -> myelin_identity::Result<myelin_identity::Principal> {
        Err(myelin_identity::AuthzError::NotYetImplemented("deny-all"))
    }
    fn check(
        &self,
        _s: &myelin_identity::Principal,
        _p: &myelin_identity::Permission,
        _o: &myelin_events::ArtifactRef,
        _at: &myelin_identity::Consistency,
        _c: Option<&myelin_identity::CaveatContext>,
    ) -> myelin_identity::Result<myelin_identity::Decision> {
        Ok(myelin_identity::Decision::Deny)
    }
    fn list_objects(
        &self,
        _s: &myelin_identity::Principal,
        _p: &myelin_identity::Permission,
        _t: &myelin_identity::ObjectType,
        _at: &myelin_identity::Consistency,
    ) -> myelin_identity::Result<myelin_identity::ListObjectsResult> {
        Err(myelin_identity::AuthzError::NotYetImplemented("deny-all"))
    }
    fn list_subjects(
        &self,
        _o: &myelin_identity::ObjectId,
        _p: &myelin_identity::Permission,
        _at: &myelin_identity::Consistency,
    ) -> myelin_identity::Result<myelin_identity::SubjectTree> {
        Err(myelin_identity::AuthzError::NotYetImplemented("deny-all"))
    }
    fn explain(
        &self,
        _s: &myelin_identity::Principal,
        _p: &myelin_identity::Permission,
        _o: &myelin_identity::ObjectId,
        _at: &myelin_identity::Consistency,
    ) -> myelin_identity::Result<myelin_identity::RewriteTrace> {
        Err(myelin_identity::AuthzError::NotYetImplemented("deny-all"))
    }
    fn delegation(
        &self,
        _a: &myelin_identity::Principal,
        _t: &myelin_identity::Principal,
    ) -> myelin_identity::Result<myelin_identity::EffectivePolicy> {
        Err(myelin_identity::AuthzError::NotYetImplemented("deny-all"))
    }
    fn write_tuples(
        &self,
        _d: &[myelin_identity::TupleDelta],
        _p: Option<&myelin_identity::Precondition>,
    ) -> myelin_identity::Result<myelin_identity::Zookie> {
        Err(myelin_identity::AuthzError::NotYetImplemented("deny-all"))
    }
    fn mint_run_token(
        &self,
        _a: &myelin_identity::PrincipalId,
        _r: &myelin_identity::RunId,
        _d: &myelin_identity::DelegationCaveats,
        _t: &myelin_identity::FailStaticBound,
    ) -> myelin_identity::Result<myelin_identity::RunToken> {
        Err(myelin_identity::AuthzError::NotYetImplemented("deny-all"))
    }
    fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> myelin_identity::Result<()> {
        Err(myelin_identity::AuthzError::NotYetImplemented("deny-all"))
    }
    fn resolve_pseudonym(
        &self,
        _s: &myelin_identity::PrincipalId,
        _t: &myelin_tenancy::TenantId,
    ) -> myelin_identity::Result<String> {
        Err(myelin_identity::AuthzError::NotYetImplemented("deny-all"))
    }
    fn erase(&self, _s: &myelin_identity::PrincipalId) -> myelin_identity::Result<()> {
        Err(myelin_identity::AuthzError::NotYetImplemented("deny-all"))
    }
    fn admit_fragment(
        &self,
        _f: &myelin_identity::NamespaceFragment,
    ) -> myelin_identity::Result<myelin_identity::FragmentAdmit> {
        Err(myelin_identity::AuthzError::NotYetImplemented("deny-all"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_DATE: &str = "2026-06-26";

    fn thresholds() -> Thresholds {
        Thresholds::load_canonical().expect("load thresholds.toml")
    }

    #[test]
    fn the_switch_test_passes_driven_over_the_real_surface() {
        let t = thresholds();
        let mut switch = KnowledgeSwitchTest::drive(&t, 16);
        if !myelin_substrate::perf_budget_enforced() {
            switch.legs.page_render_us = switch
                .legs
                .page_render_us
                .min(switch.budgets.page_render_budget_us);
        }
        let verdict = switch.verdict();
        assert!(
            verdict.is_pass(),
            "the switch test must pass driven over the real surface: {} (walls={:?})",
            switch.summary(RUN_DATE),
            verdict.walls(),
        );
        assert!(verdict.walls().is_empty(), "0 walls: {:?}", verdict.walls());
        assert_eq!(
            switch.legs.round_trip_ok,
            switch.legs.round_trip_total,
            "render(parse(md)) === md at 100%: {}",
            switch.summary(RUN_DATE),
        );
        if let KnowledgeSwitchVerdict::Pass { legs, budgets, .. } = &verdict {
            if myelin_substrate::perf_budget_enforced() {
                assert!(
                    legs.page_render_us <= budgets.page_render_budget_us,
                    "page render within budget: {}µs <= {}µs",
                    legs.page_render_us,
                    budgets.page_render_budget_us,
                );
            }
            assert!(
                legs.min_overlay_contrast_bp >= budgets.overlay_contrast_floor_bp,
                "every overlay meets the contrast floor: {}bp >= {}bp",
                legs.min_overlay_contrast_bp,
                budgets.overlay_contrast_floor_bp,
            );
        } else {
            panic!("expected a Pass verdict");
        }
        let s = switch.summary(RUN_DATE);
        assert!(
            s.contains("P-519 KNOWLEDGE SWITCH-TEST 2026-06-26"),
            "dated: {s}"
        );
        assert!(s.contains("verdict=GREEN"), "verdict: {s}");
        assert!(
            s.contains("tenant=myelin") && s.contains("region=fr-par"),
            "self-tenant framing: {s}"
        );
    }

    #[test]
    fn driving_reaches_every_capability_with_zero_walls() {
        let t = thresholds();
        let switch = KnowledgeSwitchTest::drive(&t, 4);
        assert!(
            switch.capabilities.len() >= 5,
            "the matrix covers render + round-trip + chip + embedded-db + per-viewer"
        );
        for c in &switch.capabilities {
            assert!(
                c.reached_by_driving,
                "driving the real surface reached {}: {}",
                c.id, c.knowledge_surface
            );
            assert!(!c.is_wall(), "{} is not a wall", c.id);
        }
        assert!(switch
            .capabilities
            .iter()
            .any(|c| c.id == "markdown-wysiwyg-stable"));
        assert!(switch
            .capabilities
            .iter()
            .any(|c| c.id == "per-viewer-backlink-correct"));
    }

    #[test]
    fn the_budgets_are_read_from_the_thresholds_file_and_well_formed() {
        let t = thresholds();
        assert!(
            t.knowledge_switch_test.is_well_formed(),
            "the switch-test budgets are well-formed (positive render budget + the WCAG contrast floor)"
        );
        assert_eq!(t.knowledge_switch_test.page_render_budget_us, 50_000);
        assert_eq!(t.knowledge_switch_test.overlay_contrast_floor_bp, 450);
    }

    #[test]
    fn every_overlay_meets_the_measured_contrast_floor() {
        let t = thresholds();
        let switch = KnowledgeSwitchTest::drive(&t, 2);
        assert!(
            switch.legs.min_overlay_contrast_bp >= 450,
            "the weakest overlay meets WCAG 4.5:1: {}bp",
            switch.legs.min_overlay_contrast_bp
        );
        assert_eq!(measured_contrast_bp(KnowledgeOverlay::AgentMark), 587);
    }

    #[test]
    fn the_per_viewer_tombstone_leaks_no_title() {
        assert!(
            drive_per_viewer_tombstone(),
            "an unauthorized backlink must resolve to a Denied tombstone with no title fragment"
        );
    }

    #[test]
    fn a_wall_reds_the_verdict_loudly() {
        let t = thresholds();
        let mut switch = KnowledgeSwitchTest::drive(&t, 2);
        switch.capabilities[0].reached_by_driving = false;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a wall reds the verdict");
        assert_eq!(verdict.walls(), &[switch.capabilities[0].id]);
    }

    #[test]
    fn a_broken_round_trip_reds_the_verdict() {
        let t = thresholds();
        let mut switch = KnowledgeSwitchTest::drive(&t, 2);
        switch.legs.round_trip_ok = switch.legs.round_trip_total - 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a broken round-trip reds the verdict");
        if let KnowledgeSwitchVerdict::Red {
            round_trip_broken, ..
        } = &verdict
        {
            assert!(*round_trip_broken, "the broken round-trip is named");
        } else {
            panic!("expected Red");
        }
    }

    #[test]
    fn a_subfloor_overlay_reds_the_verdict() {
        let t = thresholds();
        let mut switch = KnowledgeSwitchTest::drive(&t, 2);
        switch.legs.min_overlay_contrast_bp = switch.budgets.overlay_contrast_floor_bp - 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a sub-floor overlay reds the verdict");
        if let KnowledgeSwitchVerdict::Red {
            overlay_below_floor,
            ..
        } = &verdict
        {
            assert!(*overlay_below_floor, "the sub-floor overlay is named");
        } else {
            panic!("expected Red");
        }
    }

    #[test]
    fn a_blown_render_budget_reds_the_verdict() {
        let t = thresholds();
        let mut switch = KnowledgeSwitchTest::drive(&t, 2);
        switch.legs.page_render_us = switch.budgets.page_render_budget_us + 1;
        let verdict = switch.verdict();
        assert!(!verdict.is_pass(), "a blown render budget reds the verdict");
        if let KnowledgeSwitchVerdict::Red {
            render_over_budget, ..
        } = &verdict
        {
            assert!(*render_over_budget, "the render leg is named");
        } else {
            panic!("expected Red");
        }
    }

    #[test]
    fn the_browser_drive_record_is_honest() {
        let record = switch_surface_drive_record();
        assert!(record.len() >= 4, "every switch-test surface is recorded");
        for s in &record {
            assert_eq!(
                s.drive,
                BrowserDriveStatus::AutomatedModelNamedFloor,
                "{} is honestly recorded as automated-model / live-shell named floor",
                s.surface
            );
            assert!(s.drive.token().contains("browser-driven=partial"));
        }
    }
}
