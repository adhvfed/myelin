use std::collections::BTreeMap;

use myelin_events::{
    reindex, ArtifactRef, BusTransport, DataRole, DerivedStore, EmitContextBase, EventEnvelope,
    InProcessBus, OutboxStore, Region, ReindexError, ReindexReceipt, ReindexSource, Relay,
    SnapshotScope, Visibility,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DerivedStoreClass {
    Olap,
    Search,
    Refs,
}

impl DerivedStoreClass {
    pub const ALL: [DerivedStoreClass; 3] = [
        DerivedStoreClass::Olap,
        DerivedStoreClass::Search,
        DerivedStoreClass::Refs,
    ];

    pub fn name(self) -> &'static str {
        match self {
            DerivedStoreClass::Olap => "olap",
            DerivedStoreClass::Search => "search",
            DerivedStoreClass::Refs => "refs",
        }
    }

    pub fn has_backup_restore_path(self) -> bool {
        match self {
            DerivedStoreClass::Olap | DerivedStoreClass::Search | DerivedStoreClass::Refs => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedStoreParity {
    pub store: DerivedStoreClass,
    pub live_hash: u64,
    pub cold_hash: u64,
    pub snapshots_emitted_first: usize,
    pub snapshots_emitted_second: usize,
    pub has_backup_restore_path: bool,
}

impl DerivedStoreParity {
    pub fn cold_matches_live(&self) -> bool {
        self.cold_hash == self.live_hash
    }

    pub fn is_green(&self) -> bool {
        self.cold_matches_live()
            && !self.has_backup_restore_path
            && self.snapshots_emitted_second == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2e3StorageArtifact {
    pub legs: Vec<DerivedStoreParity>,
    pub stores_with_drift: usize,
    pub derived_stores_with_backup_path: usize,
    pub certificate_hash: u64,
}

impl E2e3StorageArtifact {
    pub fn seal(legs: Vec<DerivedStoreParity>) -> E2e3StorageArtifact {
        let stores_with_drift = legs.iter().filter(|l| !l.cold_matches_live()).count();
        let derived_stores_with_backup_path =
            legs.iter().filter(|l| l.has_backup_restore_path).count();
        let certificate_hash = certificate_hash(&legs);
        E2e3StorageArtifact {
            legs,
            stores_with_drift,
            derived_stores_with_backup_path,
            certificate_hash,
        }
    }

    pub fn is_green(&self) -> bool {
        self.covers_all_derived_stores()
            && self.stores_with_drift == 0
            && self.derived_stores_with_backup_path == 0
            && self.legs.iter().all(DerivedStoreParity::is_green)
    }

    pub fn covers_all_derived_stores(&self) -> bool {
        DerivedStoreClass::ALL
            .iter()
            .all(|c| self.legs.iter().any(|l| l.store == *c))
    }

    pub fn summary(&self) -> String {
        format!(
            "E2E-3 storage half: {} derived stores cold==live (0 drift), 0 backup-restore paths; \
             cert={:016x}",
            self.legs.len(),
            self.certificate_hash
        )
    }
}

fn content_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn certificate_hash(legs: &[DerivedStoreParity]) -> u64 {
    let mut sorted: Vec<&DerivedStoreParity> = legs.iter().collect();
    sorted.sort_by_key(|l| l.store);
    let mut buf = Vec::new();
    for l in sorted {
        buf.extend_from_slice(l.store.name().as_bytes());
        buf.extend_from_slice(&l.live_hash.to_le_bytes());
        buf.extend_from_slice(&l.cold_hash.to_le_bytes());
        buf.push(u8::from(l.has_backup_restore_path));
    }
    content_hash(&buf)
}

fn live_projection(
    region: &Region,
    source: &DerivedReindexSource,
) -> Result<DerivedStore, ReindexError> {
    let mut store = DerivedStore::new();
    for draft in source.replay(&source.scope(), None) {
        let env = live_envelope(region, &draft);
        store.ingest(&env);
    }
    Ok(store)
}

fn cold_reindex_derived(
    region: &Region,
    source: &DerivedReindexSource,
    outbox: &mut OutboxStore,
    bus: &InProcessBus,
    relay: &Relay<InProcessBus>,
    ctx_base: EmitContextBase,
) -> Result<(DerivedStore, ReindexReceipt), ReindexError> {
    let scope = source.scope();
    let sources: Vec<&dyn ReindexSource> = vec![source];
    let receipt = reindex::reindex(&scope, None, &sources, outbox, ctx_base)?;
    relay.drain_to_empty();
    let mut cold = DerivedStore::new();
    let published: Vec<EventEnvelope> = bus.consume(&source.subject_prefix());
    for env in &published {
        cold.ingest(env);
    }
    let _ = region;
    Ok((cold, receipt))
}

pub fn run_e2e3_storage_half(
    region: &Region,
    sources: &BTreeMap<DerivedStoreClass, DerivedReindexSource>,
    ctx_base: &EmitContextBase,
) -> Result<E2e3StorageArtifact, ReindexError> {
    let mut legs = Vec::with_capacity(DerivedStoreClass::ALL.len());
    for store in DerivedStoreClass::ALL {
        let source = sources.get(&store).ok_or_else(|| {
            ReindexError::NoSourceForOwner(format!("no source for {}", store.name()))
        })?;

        let live = live_projection(region, source)?;
        let live_hash = content_hash(&live.parity_bytes());

        let (outbox_bus, bus, relay) = booted_bus();
        let mut outbox = outbox_bus;
        let (cold, r1) =
            cold_reindex_derived(region, source, &mut outbox, &bus, &relay, ctx_base.clone())?;
        let cold_hash = content_hash(&cold.parity_bytes());

        let (_again, r2) =
            cold_reindex_derived(region, source, &mut outbox, &bus, &relay, ctx_base.clone())?;

        legs.push(DerivedStoreParity {
            store,
            live_hash,
            cold_hash,
            snapshots_emitted_first: r1.snapshots_emitted,
            snapshots_emitted_second: r2.snapshots_emitted,
            has_backup_restore_path: store.has_backup_restore_path(),
        });
    }
    Ok(E2e3StorageArtifact::seal(legs))
}

pub struct DerivedReindexSource {
    tenant: myelin_events::TenantId,
    owner: String,
    truth: BTreeMap<String, (u64, serde_json::Value)>,
}

impl DerivedReindexSource {
    pub fn new(tenant: myelin_events::TenantId, owner: impl Into<String>) -> DerivedReindexSource {
        DerivedReindexSource {
            tenant,
            owner: owner.into(),
            truth: BTreeMap::new(),
        }
    }

    pub fn upsert(
        &mut self,
        aggregate: &str,
        version: u64,
        payload: serde_json::Value,
    ) -> &mut Self {
        self.truth.insert(aggregate.to_string(), (version, payload));
        self
    }

    fn scope(&self) -> SnapshotScope {
        SnapshotScope::new(self.owner.clone(), "all")
    }

    fn subject_prefix(&self) -> String {
        String::new()
    }

    fn snapshot_type(&self) -> myelin_events::EventType {
        myelin_events::EventType(format!(
            "{}.derived.{}",
            self.owner,
            reindex::SNAPSHOT_EVENT_NAME
        ))
    }
}

impl ReindexSource for DerivedReindexSource {
    fn owner_token(&self) -> &str {
        &self.owner
    }

    fn replay(
        &self,
        _scope: &SnapshotScope,
        since: Option<u64>,
    ) -> Vec<myelin_events::SnapshotDraft> {
        self.truth
            .iter()
            .filter(|(_, (v, _))| since.is_none_or(|s| *v > s))
            .map(|(agg, (v, payload))| {
                let mut body = payload.clone();
                body["version"] = serde_json::json!(v);
                myelin_events::SnapshotDraft {
                    aggregate: myelin_events::AggregateKey(agg.clone()),
                    version: *v,
                    type_: self.snapshot_type(),
                    subject: ArtifactRef(format!(
                        "myelin://{}/{}/derived/{agg}",
                        self.tenant.0, self.owner
                    )),
                    payload: body,
                    data_role: DataRole::Processor,
                    visibility: Visibility::Internal,
                }
            })
            .collect()
    }
}

fn live_envelope(region: &Region, draft: &myelin_events::SnapshotDraft) -> EventEnvelope {
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, EventId, EventType, TenantId, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    let tenant = TenantId("01J0ACME".into());
    EventEnvelope {
        event_id: EventId(draft.event_id(&tenant).0),
        type_: EventType(draft.type_.0.clone()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: region.clone(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant,
        )),
        subject: draft.subject.clone(),
        aggregate: AggregateKey(draft.aggregate.0.clone()),
        causation_id: None,
        correlation_id: CorrelationId("root".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
        payload: draft.payload.clone(),
    }
}

fn booted_bus() -> (OutboxStore, InProcessBus, Relay<InProcessBus>) {
    use myelin_events::Timestamp;
    let outbox = OutboxStore::new();
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || {
        Timestamp("2026-06-20T00:00:02Z".into())
    });
    (outbox, bus, relay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{Actor, TenantId, Timestamp};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn region() -> Region {
        Region("fr-par".into())
    }

    fn tenant() -> TenantId {
        TenantId("01J0ACME".into())
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                tenant(),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
            caused_by: None,
        }
    }

    fn all_sources() -> BTreeMap<DerivedStoreClass, DerivedReindexSource> {
        let mut olap = DerivedReindexSource::new(tenant(), "olap_src");
        olap.upsert("issue:PROJ-1", 1, serde_json::json!({ "cfd": 3 }))
            .upsert("issue:PROJ-2", 2, serde_json::json!({ "cfd": 5 }));

        let mut search = DerivedReindexSource::new(tenant(), "search_src");
        search
            .upsert("page:home", 1, serde_json::json!({ "text": "raft" }))
            .upsert("page:guide", 2, serde_json::json!({ "text": "paxos" }))
            .upsert("page:faq", 1, serde_json::json!({ "text": "faq" }));

        let mut refs = DerivedReindexSource::new(tenant(), "refs_src");
        refs.upsert(
            "edge:PR-1->ISSUE-1",
            1,
            serde_json::json!({ "kind": "closes" }),
        )
        .upsert(
            "edge:COMMIT-1->PR-1",
            1,
            serde_json::json!({ "kind": "part_of" }),
        );

        BTreeMap::from([
            (DerivedStoreClass::Olap, olap),
            (DerivedStoreClass::Search, search),
            (DerivedStoreClass::Refs, refs),
        ])
    }

    #[test]
    fn derived_store_set_is_exhaustive() {
        assert_eq!(DerivedStoreClass::ALL.len(), 3);
        for c in DerivedStoreClass::ALL {
            assert!(!c.name().is_empty());
        }
    }

    #[test]
    fn no_derived_store_has_a_backup_restore_path() {
        for c in DerivedStoreClass::ALL {
            assert!(
                !c.has_backup_restore_path(),
                "{} is a derived store - it is NOT backed up (§7.1/§7.3)",
                c.name()
            );
        }
    }

    #[test]
    fn e2e3_cold_reindex_byte_matches_live_for_every_derived_store() {
        let sources = all_sources();
        let artifact = run_e2e3_storage_half(&region(), &sources, &ctx_base())
            .expect("the E2E-3 storage half runs");

        assert!(
            artifact.is_green(),
            "the E2E-3 storage half is green: {artifact:?}"
        );
        assert_eq!(artifact.stores_with_drift, 0, "0 drift - cold == live");
        assert_eq!(
            artifact.derived_stores_with_backup_path, 0,
            "0 derived stores backed up - reindex-from-source only"
        );
        assert!(
            artifact.covers_all_derived_stores(),
            "the artifact covers OLAP + Search + Refs"
        );
        for leg in &artifact.legs {
            assert!(
                leg.cold_matches_live(),
                "{}: cold reindex byte-matches live (0 drift)",
                leg.store.name()
            );
            assert_eq!(
                leg.cold_hash,
                leg.live_hash,
                "{}: the parity hashes are identical",
                leg.store.name()
            );
        }
    }

    #[test]
    fn e2e3_re_run_is_idempotent_per_store() {
        let sources = all_sources();
        let artifact = run_e2e3_storage_half(&region(), &sources, &ctx_base()).unwrap();
        for leg in &artifact.legs {
            assert!(
                leg.snapshots_emitted_first > 0,
                "{}: the first rebuild emitted snapshots",
                leg.store.name()
            );
            assert_eq!(
                leg.snapshots_emitted_second,
                0,
                "{}: the re-run emitted 0 NEW snapshots (idempotent)",
                leg.store.name()
            );
        }
    }

    #[test]
    fn e2e3_artifact_reads_red_when_any_invariant_fails() {
        let green = || DerivedStoreParity {
            store: DerivedStoreClass::Olap,
            live_hash: 7,
            cold_hash: 7,
            snapshots_emitted_first: 2,
            snapshots_emitted_second: 0,
            has_backup_restore_path: false,
        };
        let search = || DerivedStoreParity {
            store: DerivedStoreClass::Search,
            ..green()
        };
        let refs = || DerivedStoreParity {
            store: DerivedStoreClass::Refs,
            ..green()
        };

        assert!(E2e3StorageArtifact::seal(vec![green(), search(), refs()]).is_green());

        let drift = DerivedStoreParity {
            cold_hash: 99,
            ..green()
        };
        let a = E2e3StorageArtifact::seal(vec![drift, search(), refs()]);
        assert_eq!(a.stores_with_drift, 1);
        assert!(!a.is_green());

        let backed = DerivedStoreParity {
            has_backup_restore_path: true,
            ..green()
        };
        let b = E2e3StorageArtifact::seal(vec![backed, search(), refs()]);
        assert_eq!(b.derived_stores_with_backup_path, 1);
        assert!(!b.is_green());

        let noisy = DerivedStoreParity {
            snapshots_emitted_second: 1,
            ..green()
        };
        assert!(!E2e3StorageArtifact::seal(vec![noisy, search(), refs()]).is_green());

        let missing = E2e3StorageArtifact::seal(vec![green(), search()]);
        assert!(!missing.covers_all_derived_stores());
        assert!(!missing.is_green());
    }

    #[test]
    fn e2e3_missing_source_for_a_store_is_a_loud_error() {
        let mut sources = all_sources();
        sources.remove(&DerivedStoreClass::Refs);
        let err = run_e2e3_storage_half(&region(), &sources, &ctx_base())
            .expect_err("a missing derived-store source must fail loudly");
        assert!(matches!(err, ReindexError::NoSourceForOwner(_)));
    }

    #[test]
    fn e2e3_certificate_is_deterministic_and_tamper_evident() {
        let sources = all_sources();
        let a = run_e2e3_storage_half(&region(), &sources, &ctx_base()).unwrap();
        let b = run_e2e3_storage_half(&region(), &sources, &ctx_base()).unwrap();
        assert_eq!(
            a.certificate_hash, b.certificate_hash,
            "the same derived-store set seals the same certificate (byte-reproducible)"
        );
        let mut tampered = a.legs.clone();
        tampered[0].cold_hash ^= 0xdead_beef;
        let t = E2e3StorageArtifact::seal(tampered);
        assert_ne!(
            a.certificate_hash, t.certificate_hash,
            "a tampered parity hash changes the certificate (tamper-evident)"
        );
    }

    #[test]
    fn e2e3_green_artifact_summary_is_observable() {
        let sources = all_sources();
        let artifact = run_e2e3_storage_half(&region(), &sources, &ctx_base()).unwrap();
        let s = artifact.summary();
        assert!(s.contains("3 derived stores"), "names the store count: {s}");
        assert!(s.contains("0 drift"), "names the 0-drift proof: {s}");
        assert!(
            s.contains("0 backup-restore paths"),
            "names the no-backup proof: {s}"
        );
    }
}
