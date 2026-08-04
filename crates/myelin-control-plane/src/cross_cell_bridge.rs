use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_tenancy::{ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewerId(String);

impl ViewerId {
    #[inline]
    pub fn from_token(token: impl Into<String>) -> Self {
        ViewerId(token.into())
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BridgeMode {
    Live,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeProjection {
    pub subject: OpaqueSubjectId,
    pub title: String,
    pub state: String,
    pub icon: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeTombstone {
    pub subject: OpaqueSubjectId,
    pub reason: BridgeTombstoneReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BridgeTombstoneReason {
    Denied,
    Gone,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeResolution {
    Projection(BridgeProjection),
    Tombstone(BridgeTombstone),
}

impl BridgeResolution {
    pub fn is_projection(&self) -> bool {
        matches!(self, BridgeResolution::Projection(_))
    }

    pub fn is_tombstone(&self) -> bool {
        matches!(self, BridgeResolution::Tombstone(_))
    }

    pub fn tombstone_reason(&self) -> Option<BridgeTombstoneReason> {
        match self {
            BridgeResolution::Tombstone(t) => Some(t.reason),
            BridgeResolution::Projection(_) => None,
        }
    }
}

pub trait CellLocalResolver: Send + Sync {
    fn resolve_in_cell(
        &self,
        pointer: &CrossCellPointer,
        viewer: &ViewerId,
        mode: BridgeMode,
    ) -> BridgeResolution;
}

pub trait ResolverProjection: Send + Sync {
    fn resolver_for(&self, cell: &CellId) -> Option<Arc<dyn CellLocalResolver>>;
}

#[derive(Clone)]
pub struct CellResolverRegistry {
    backend: CellResolverBackend,
}

#[derive(Clone)]
enum CellResolverBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(std::collections::HashMap<CellId, Arc<dyn CellLocalResolver>>),
    Projected(Arc<dyn ResolverProjection>),
}

impl CellResolverRegistry {
    pub fn projected(projection: Arc<dyn ResolverProjection>) -> CellResolverRegistry {
        CellResolverRegistry {
            backend: CellResolverBackend::Projected(projection),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    #[allow(clippy::new_without_default)]
    pub fn new() -> CellResolverRegistry {
        CellResolverRegistry {
            backend: CellResolverBackend::Memory(std::collections::HashMap::new()),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn register(&mut self, cell: CellId, resolver: Arc<dyn CellLocalResolver>) {
        match &mut self.backend {
            CellResolverBackend::Memory(map) => {
                map.insert(cell, resolver);
            }
            CellResolverBackend::Projected(_) => {
                panic!("register() is the test-support double; a projected registry is durable-authoritative")
            }
        }
    }

    fn resolver_for(&self, cell: &CellId) -> Option<Arc<dyn CellLocalResolver>> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CellResolverBackend::Memory(map) => map.get(cell).cloned(),
            CellResolverBackend::Projected(projection) => projection.resolver_for(cell),
        }
    }
}

impl core::fmt::Debug for CellResolverRegistry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let backend = match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CellResolverBackend::Memory(_) => "memory(test-support)",
            CellResolverBackend::Projected(_) => "projected(durable-cell-table)",
        };
        f.debug_struct("CellResolverRegistry")
            .field("backend", &backend)
            .finish()
    }
}

#[derive(Clone)]
pub struct CrossCellBridge {
    cell_id: CellId,
    resolvers: CellResolverRegistry,
    cross_cell_resolves: Arc<AtomicU64>,
    cross_cell_raw_rows: Arc<AtomicU64>,
}

impl CrossCellBridge {
    pub fn new(cell_id: CellId, resolvers: CellResolverRegistry) -> CrossCellBridge {
        CrossCellBridge {
            cell_id,
            resolvers,
            cross_cell_resolves: Arc::new(AtomicU64::new(0)),
            cross_cell_raw_rows: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn cell_id(&self) -> &CellId {
        &self.cell_id
    }

    pub fn resolve(
        &self,
        pointer: &CrossCellPointer,
        viewer: &ViewerId,
        mode: BridgeMode,
    ) -> BridgeResolution {
        self.cross_cell_resolves.fetch_add(1, Ordering::SeqCst);

        let home = pointer.home_cell();
        match self.resolvers.resolver_for(home) {
            Some(resolver) => resolver.resolve_in_cell(pointer, viewer, mode),
            None => BridgeResolution::Tombstone(BridgeTombstone {
                subject: pointer.subject().clone(),
                reason: BridgeTombstoneReason::Gone,
            }),
        }
    }

    pub fn rollup(
        &self,
        pointers: &[CrossCellPointer],
        viewer: &ViewerId,
        mode: BridgeMode,
    ) -> Vec<BridgeProjection> {
        pointers
            .iter()
            .filter_map(|p| match self.resolve(p, viewer, mode) {
                BridgeResolution::Projection(proj) => Some(proj),
                BridgeResolution::Tombstone(_) => None,
            })
            .collect()
    }

    pub fn cross_cell_resolves(&self) -> u64 {
        self.cross_cell_resolves.load(Ordering::SeqCst)
    }

    pub fn cross_cell_raw_rows(&self) -> u64 {
        self.cross_cell_raw_rows.load(Ordering::SeqCst)
    }
}

impl core::fmt::Debug for CrossCellBridge {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CrossCellBridge")
            .field("cell_id", &self.cell_id.as_str())
            .field("cross_cell_resolves", &self.cross_cell_resolves())
            .field("cross_cell_raw_rows", &self.cross_cell_raw_rows())
            .finish()
    }
}

pub fn bridge_carried_fields(
    pointer: &CrossCellPointer,
) -> (&OpaqueSubjectId, &ArtifactType, &CorrelationId, &CellId) {
    (
        pointer.subject(),
        pointer.artifact_type(),
        pointer.correlation_id(),
        pointer.home_cell(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_tenancy::ArtifactRef;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct HomeCellResolver {
        permitted: HashMap<(String, String), bool>,
        rendered: HashMap<String, (String, String, String)>,
        gone: Vec<String>,
        resolved_here: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl HomeCellResolver {
        fn new() -> HomeCellResolver {
            HomeCellResolver {
                permitted: HashMap::new(),
                rendered: HashMap::new(),
                gone: Vec::new(),
                resolved_here: Arc::new(Mutex::new(Vec::new())),
            }
        }
        fn permit(&mut self, subject: &str, viewer: &str) {
            self.permitted.insert((subject.into(), viewer.into()), true);
        }
        fn render(&mut self, subject: &str, title: &str, state: &str, icon: &str) {
            self.rendered
                .insert(subject.into(), (title.into(), state.into(), icon.into()));
        }
        fn mark_gone(&mut self, subject: &str) {
            self.gone.push(subject.into());
        }
    }

    impl CellLocalResolver for HomeCellResolver {
        fn resolve_in_cell(
            &self,
            pointer: &CrossCellPointer,
            viewer: &ViewerId,
            _mode: BridgeMode,
        ) -> BridgeResolution {
            let subject_str = pointer.subject().artifact_ref().0.clone();
            self.resolved_here
                .lock()
                .unwrap()
                .push((subject_str.clone(), viewer.as_str().into()));

            let allowed = *self
                .permitted
                .get(&(subject_str.clone(), viewer.as_str().into()))
                .unwrap_or(&false);
            if !allowed {
                return BridgeResolution::Tombstone(BridgeTombstone {
                    subject: pointer.subject().clone(),
                    reason: BridgeTombstoneReason::Denied,
                });
            }
            if self.gone.contains(&subject_str) {
                return BridgeResolution::Tombstone(BridgeTombstone {
                    subject: pointer.subject().clone(),
                    reason: BridgeTombstoneReason::Gone,
                });
            }
            let (title, state, icon) = self
                .rendered
                .get(&subject_str)
                .cloned()
                .unwrap_or_else(|| ("untitled".into(), "open".into(), "doc".into()));
            BridgeResolution::Projection(BridgeProjection {
                subject: pointer.subject().clone(),
                title,
                state,
                icon,
            })
        }
    }

    fn pointer(subject: &str, kind: ArtifactType, home: &str) -> CrossCellPointer {
        CrossCellPointer::new(
            OpaqueSubjectId::from_ref(ArtifactRef(subject.into())),
            kind,
            CorrelationId("01J0CORR".into()),
            CellId::from_token(home),
        )
    }

    #[test]
    fn bridge_carries_exactly_the_four_frozen_fields() {
        let p = pointer(
            "myelin://01J0BETA/issues/issue/7",
            ArtifactType::Issue,
            "cell-b",
        );
        let (subject, kind, corr, home) = bridge_carried_fields(&p);
        assert_eq!(subject.artifact_ref().0, "myelin://01J0BETA/issues/issue/7");
        assert_eq!(kind, &ArtifactType::Issue);
        assert_eq!(corr, &CorrelationId("01J0CORR".into()));
        assert_eq!(home.as_str(), "cell-b");
    }

    #[test]
    fn cross_cell_resolve_permission_checks_in_home_cell_and_returns_projection() {
        let mut b = HomeCellResolver::new();
        b.permit("myelin://01J0BETA/issues/issue/7", "viewer-1");
        b.render(
            "myelin://01J0BETA/issues/issue/7",
            "Ship M5",
            "open",
            "issue",
        );
        let b_seen = b.resolved_here.clone();

        let mut reg = CellResolverRegistry::new();
        reg.register(CellId::from_token("cell-b"), Arc::new(b));
        let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

        let p = pointer(
            "myelin://01J0BETA/issues/issue/7",
            ArtifactType::Issue,
            "cell-b",
        );
        let res = bridge.resolve(&p, &ViewerId::from_token("viewer-1"), BridgeMode::Live);

        assert!(
            res.is_projection(),
            "an authorised viewer gets the projection"
        );
        assert!(!res.is_tombstone(), "a projection is NOT a tombstone");
        assert_eq!(
            res.tombstone_reason(),
            None,
            "a projection has no tombstone reason"
        );
        let BridgeResolution::Projection(proj) = res else {
            unreachable!()
        };
        assert_eq!(proj.title, "Ship M5");
        assert_eq!(proj.state, "open");
        assert_eq!(
            b_seen.lock().unwrap().as_slice(),
            &[(
                "myelin://01J0BETA/issues/issue/7".to_string(),
                "viewer-1".to_string()
            )]
        );
        assert_eq!(bridge.cross_cell_raw_rows(), 0);
        assert_eq!(bridge.cross_cell_resolves(), 1);
    }

    #[test]
    fn unauthorised_cross_cell_viewer_gets_a_tombstone() {
        let mut b = HomeCellResolver::new();
        b.render(
            "myelin://01J0BETA/issues/issue/7",
            "Secret",
            "open",
            "issue",
        );

        let mut reg = CellResolverRegistry::new();
        reg.register(CellId::from_token("cell-b"), Arc::new(b));
        let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

        let p = pointer(
            "myelin://01J0BETA/issues/issue/7",
            ArtifactType::Issue,
            "cell-b",
        );
        let res = bridge.resolve(&p, &ViewerId::from_token("viewer-2"), BridgeMode::Live);

        assert!(
            res.is_tombstone(),
            "an unauthorised viewer gets a tombstone"
        );
        assert!(!res.is_projection(), "a tombstone is NOT a projection");
        assert_eq!(res.tombstone_reason(), Some(BridgeTombstoneReason::Denied));
        let BridgeResolution::Tombstone(t) = res else {
            unreachable!()
        };
        assert_eq!(
            t.subject.artifact_ref().0,
            "myelin://01J0BETA/issues/issue/7"
        );
        assert_eq!(bridge.cross_cell_raw_rows(), 0);
    }

    #[test]
    fn gone_artifact_resolves_to_a_gone_tombstone() {
        let mut b = HomeCellResolver::new();
        b.permit("myelin://01J0BETA/kn/page/9", "viewer-1");
        b.mark_gone("myelin://01J0BETA/kn/page/9");

        let mut reg = CellResolverRegistry::new();
        reg.register(CellId::from_token("cell-b"), Arc::new(b));
        let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

        let p = pointer("myelin://01J0BETA/kn/page/9", ArtifactType::Page, "cell-b");
        let res = bridge.resolve(&p, &ViewerId::from_token("viewer-1"), BridgeMode::Live);
        assert_eq!(res.tombstone_reason(), Some(BridgeTombstoneReason::Gone));
    }

    #[test]
    fn a_home_pointer_resolves_locally() {
        let mut b = HomeCellResolver::new();
        b.permit("myelin://01J0BETA/chat/channel/3", "viewer-1");
        b.render(
            "myelin://01J0BETA/chat/channel/3",
            "#general",
            "active",
            "channel",
        );

        let mut reg = CellResolverRegistry::new();
        reg.register(CellId::from_token("cell-b"), Arc::new(b));
        let bridge = CrossCellBridge::new(CellId::from_token("cell-b"), reg);

        let p = pointer(
            "myelin://01J0BETA/chat/channel/3",
            ArtifactType::Channel,
            "cell-b",
        );
        let res = bridge.resolve(&p, &ViewerId::from_token("viewer-1"), BridgeMode::Live);
        assert!(res.is_projection());
        let BridgeResolution::Projection(proj) = res else {
            unreachable!()
        };
        assert_eq!(proj.title, "#general");
    }

    #[test]
    fn unknown_home_cell_degrades_to_a_tombstone() {
        let bridge =
            CrossCellBridge::new(CellId::from_token("cell-a"), CellResolverRegistry::new());
        let p = pointer(
            "myelin://01J0GHOST/issues/issue/1",
            ArtifactType::Issue,
            "cell-unknown",
        );
        let res = bridge.resolve(&p, &ViewerId::from_token("viewer-1"), BridgeMode::Live);
        assert_eq!(res.tombstone_reason(), Some(BridgeTombstoneReason::Gone));
        assert_eq!(bridge.cross_cell_raw_rows(), 0);
    }

    #[test]
    fn iss_rollup_aggregates_projections_and_excludes_tombstones() {
        let mut b = HomeCellResolver::new();
        b.permit("myelin://01J0BETA/issues/issue/7", "viewer-1");
        b.render(
            "myelin://01J0BETA/issues/issue/7",
            "Visible",
            "open",
            "issue",
        );
        b.render(
            "myelin://01J0BETA/issues/issue/8",
            "Hidden",
            "open",
            "issue",
        );

        let mut c = HomeCellResolver::new();
        c.permit("myelin://01J0GAMMA/issues/issue/1", "viewer-1");
        c.render(
            "myelin://01J0GAMMA/issues/issue/1",
            "Other cell",
            "open",
            "issue",
        );

        let mut reg = CellResolverRegistry::new();
        reg.register(CellId::from_token("cell-b"), Arc::new(b));
        reg.register(CellId::from_token("cell-c"), Arc::new(c));
        let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

        let portfolio = vec![
            pointer(
                "myelin://01J0BETA/issues/issue/7",
                ArtifactType::Issue,
                "cell-b",
            ),
            pointer(
                "myelin://01J0BETA/issues/issue/8",
                ArtifactType::Issue,
                "cell-b",
            ),
            pointer(
                "myelin://01J0GAMMA/issues/issue/1",
                ArtifactType::Issue,
                "cell-c",
            ),
        ];
        let rolled = bridge.rollup(
            &portfolio,
            &ViewerId::from_token("viewer-1"),
            BridgeMode::Live,
        );
        let titles: Vec<&str> = rolled.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, vec!["Visible", "Other cell"]);
        assert_eq!(bridge.cross_cell_resolves(), 3);
        assert_eq!(bridge.cross_cell_raw_rows(), 0);
    }

    #[test]
    fn bridge_debug_is_pii_free() {
        let mut b = HomeCellResolver::new();
        b.permit("myelin://01J0BETA/issues/issue/7", "viewer-secret");
        b.render("myelin://01J0BETA/issues/issue/7", "Title", "open", "issue");
        let mut reg = CellResolverRegistry::new();
        reg.register(CellId::from_token("cell-b"), Arc::new(b));
        let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);
        let _ = bridge.resolve(
            &pointer(
                "myelin://01J0BETA/issues/issue/7",
                ArtifactType::Issue,
                "cell-b",
            ),
            &ViewerId::from_token("viewer-secret"),
            BridgeMode::Live,
        );
        let dbg = format!("{bridge:?}");
        assert!(dbg.contains("cell-a"), "Debug shows the cell id: {dbg}");
        assert!(
            dbg.contains("cross_cell_resolves"),
            "Debug shows the counter: {dbg}"
        );
        assert!(
            !dbg.contains("viewer-secret"),
            "Debug leaks no viewer id: {dbg}"
        );
        assert!(
            !dbg.contains("Title"),
            "Debug leaks no rendered content: {dbg}"
        );
    }

    #[test]
    fn viewer_id_is_opaque_not_personal() {
        let v = ViewerId::from_token("01J0PRINCIPAL");
        assert_eq!(v.as_str(), "01J0PRINCIPAL");
    }

    struct ProjectedFromCells {
        resolvers: HashMap<CellId, Arc<dyn CellLocalResolver>>,
    }
    impl ResolverProjection for ProjectedFromCells {
        fn resolver_for(&self, cell: &CellId) -> Option<Arc<dyn CellLocalResolver>> {
            self.resolvers.get(cell).cloned()
        }
    }

    #[test]
    fn cp_d8_zero_holds_on_the_projected_production_arm() {
        let mut b = HomeCellResolver::new();
        b.permit("myelin://01J0BETA/issues/issue/7", "viewer-1");
        b.render("myelin://01J0BETA/issues/issue/7", "Ship M5", "open", "issue");

        let mut resolvers: HashMap<CellId, Arc<dyn CellLocalResolver>> = HashMap::new();
        resolvers.insert(CellId::from_token("cell-b"), Arc::new(b));
        let reg = CellResolverRegistry::projected(Arc::new(ProjectedFromCells { resolvers }));
        let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

        let p = pointer("myelin://01J0BETA/issues/issue/7", ArtifactType::Issue, "cell-b");
        let ok = bridge.resolve(&p, &ViewerId::from_token("viewer-1"), BridgeMode::Live);
        assert!(ok.is_projection(), "authorised viewer gets the projection on the projected arm");
        assert_eq!(bridge.cross_cell_raw_rows(), 0);

        let denied = bridge.resolve(&p, &ViewerId::from_token("viewer-2"), BridgeMode::Live);
        assert_eq!(denied.tombstone_reason(), Some(BridgeTombstoneReason::Denied));

        let ghost = pointer("myelin://01J0GHOST/issues/issue/1", ArtifactType::Issue, "cell-unknown");
        let gone = bridge.resolve(&ghost, &ViewerId::from_token("viewer-1"), BridgeMode::Live);
        assert_eq!(gone.tombstone_reason(), Some(BridgeTombstoneReason::Gone));
        assert_eq!(bridge.cross_cell_raw_rows(), 0);
    }
}
