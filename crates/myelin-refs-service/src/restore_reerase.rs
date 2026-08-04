use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::{EventEnvelope, EventHandler};
use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId as GdprTenantId};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::cache::R2ProjectionCache;
use crate::dek::RefsDekPin;
use crate::edge_builder::{EdgeProjection, RefsEdgeBuilder};
use crate::holder::RefsCacheHolder;
use crate::resolve::{Projection, ProjectionCacheRead};

pub const WORLD_SCALE_BACKUP_FLEET_FLOOR: &str =
    "REF-D5 at full 30x world-scale backup cardinality over the PgStore-backed edge partition + the \
     KMS/Valkey backup on real fleet hardware (the ONE legitimate remaining floor); the \
     0-recoverable-PII property + the restore→re-erase cross-seam are proven here over a deterministic \
     backup-scale corpus with REAL crypto-shred";

pub const REERASE_RECOVERABLE_PII_SIGNAL: &str = "refs.reerase_recoverable_pii";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsErasedSubject {
    pub subject_id: String,
    pub tenant: TenantId,
    pub region: Region,
    pub key_refs: Vec<String>,
    pub edge_ids: Vec<String>,
    pub erased_at: String,
}

#[derive(Clone, Default)]
pub struct RefsErasureLedger {
    entries: Arc<Mutex<LedgerMap>>,
}

type LedgerMap = BTreeMap<LedgerKey, RefsErasedSubject>;

type LedgerKey = (String, String, String);

impl RefsErasureLedger {
    pub fn new() -> RefsErasureLedger {
        RefsErasureLedger::default()
    }

    fn key(tenant: &TenantId, region: &Region, subject_id: &str) -> LedgerKey {
        (tenant.0.clone(), region.0.clone(), subject_id.to_string())
    }

    pub fn record(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject_id: &str,
        key_refs: &[String],
        edge_ids: &[String],
        erased_at: &str,
    ) {
        let mut g = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = g
            .entry(Self::key(tenant, region, subject_id))
            .or_insert_with(|| RefsErasedSubject {
                subject_id: subject_id.to_string(),
                tenant: tenant.clone(),
                region: region.clone(),
                key_refs: Vec::new(),
                edge_ids: Vec::new(),
                erased_at: erased_at.to_string(),
            });
        for k in key_refs {
            if !entry.key_refs.contains(k) {
                entry.key_refs.push(k.clone());
            }
        }
        for e in edge_ids {
            if !entry.edge_ids.contains(e) {
                entry.edge_ids.push(e.clone());
            }
        }
        entry.key_refs.sort();
        entry.edge_ids.sort();
    }

    pub fn is_erased(&self, tenant: &TenantId, region: &Region, subject_id: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&Self::key(tenant, region, subject_id))
    }

    pub fn entries(&self) -> Vec<RefsErasedSubject> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug)]
pub struct BackupScaleErasureCorpus {
    pub tenant: TenantId,
    pub region: Region,
    pub subjects: Vec<String>,
    pub edges: Vec<CorpusEdge>,
}

#[derive(Clone, Debug)]
pub struct CorpusEdge {
    pub subject_id: String,
    pub source: ArtifactRef,
    pub target: ArtifactRef,
    pub edge_id: String,
    pub cached_title: String,
}

pub fn build_backup_scale_corpus(
    tenant: &TenantId,
    region: &Region,
    subjects: usize,
    edges_per_subject: usize,
) -> BackupScaleErasureCorpus {
    assert!(subjects > 0, "the backup-scale corpus needs ≥1 subject");
    assert!(
        edges_per_subject > 0,
        "each subject must author ≥1 edge (a name-bearing cached title)"
    );
    let mut subject_ids = Vec::with_capacity(subjects);
    let mut edges = Vec::with_capacity(subjects * edges_per_subject);
    for s in 0..subjects {
        let subject_id = format!("p-opaque-{s}");
        subject_ids.push(subject_id.clone());
        for e in 0..edges_per_subject {
            let source = ArtifactRef(format!("myelin://{}/chat/message/m-{s}-{e}", tenant.0));
            let target = ArtifactRef(format!("myelin://{}/knowledge/page/p-{s}-{e}", tenant.0));
            edges.push(CorpusEdge {
                subject_id: subject_id.clone(),
                edge_id: crate::edge_builder::edge_id(tenant, &source.0, &target.0, "mentions"),
                source,
                target,
                cached_title: format!("Subject {s} Name (#{e})"),
            });
        }
    }
    BackupScaleErasureCorpus {
        tenant: tenant.clone(),
        region: region.clone(),
        subjects: subject_ids,
        edges,
    }
}

impl BackupScaleErasureCorpus {
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn edges_of<'a>(
        &'a self,
        subject_id: &'a str,
    ) -> impl Iterator<Item = &'a CorpusEdge> + 'a {
        self.edges
            .iter()
            .filter(move |e| e.subject_id == subject_id)
    }

    fn edge_event(&self, edge: &CorpusEdge) -> EventEnvelope {
        use myelin_events::{
            Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp, Visibility,
        };
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        EventEnvelope {
            event_id: EventId(format!("live-{}", edge.edge_id)),
            type_: EventType("refs.edge.created".into()),
            schema_ver: 1,
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            actor: Actor(Principal::stub(
                PrincipalId(edge.subject_id.clone()),
                PrincipalKind::Human,
                self.tenant.clone(),
            )),
            subject: edge.source.clone(),
            aggregate: AggregateKey(format!("edge:{}->{}", edge.source.0, edge.target.0)),
            causation_id: None,
            correlation_id: CorrelationId(format!("live-{}", edge.edge_id)),
            caused_by: None,
            depth: 1,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
            payload: serde_json::json!({
                "source": edge.source.0,
                "target": edge.target.0,
                "rel": "mentions",
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupScaleReEraseReport {
    pub tenant: TenantId,
    pub region: Region,
    pub re_erased_subjects: usize,
    pub cached_titles_resurrected_by_restore: usize,
    pub deks_resurrected_by_restore: usize,
    pub edges_re_tombstoned: usize,
    pub recoverable_pii: usize,
    pub live_deks_post_reerase: usize,
    pub live_edges_post_reerase: usize,
    pub ran_at: String,
}

impl BackupScaleReEraseReport {
    pub fn is_ref_d5_backup_scale_green(&self) -> bool {
        self.recoverable_pii == 0
            && self.live_deks_post_reerase == 0
            && self.live_edges_post_reerase == 0
    }

    pub fn summary(&self) -> String {
        format!(
            "REF-D5 backup-scale: re_erased={} (restore resurrected {} titles / {} DEKs) \
             re_tombstoned={} → recoverable_pii={} live_deks={} live_edges={} green={}",
            self.re_erased_subjects,
            self.cached_titles_resurrected_by_restore,
            self.deks_resurrected_by_restore,
            self.edges_re_tombstoned,
            self.recoverable_pii,
            self.live_deks_post_reerase,
            self.live_edges_post_reerase,
            self.is_ref_d5_backup_scale_green(),
        )
    }
}

fn warm_subject_titles(
    corpus: &BackupScaleErasureCorpus,
    cache: &R2ProjectionCache,
    dek: &RefsDekPin,
    subject_id: &str,
) -> Vec<String> {
    let key_ref = dek
        .reserve_subject_backstop(&corpus.tenant, &corpus.region, subject_id)
        .expect("reserve per-subject DEK backstop");
    for edge in corpus.edges_of(subject_id) {
        let proj = Projection {
            ref_: edge.source.clone(),
            title: edge.cached_title.clone(),
            state: "open".into(),
            icon: "doc".into(),
            render_hint: "card".into(),
            sub_anchor: None,
            flag: None,
        };
        cache
            .fill(&corpus.tenant, &corpus.region, &edge.source, &proj)
            .expect("warm the subject's cached title");
    }
    vec![key_ref.to_uri()]
}

#[allow(clippy::too_many_arguments)]
fn erase_and_record_at_scale(
    corpus: &BackupScaleErasureCorpus,
    cache: &Arc<R2ProjectionCache>,
    dek: &RefsDekPin,
    projection: &EdgeProjection,
    ledger: &RefsErasureLedger,
    subject_id: &str,
    key_refs: &[String],
    now: &str,
) {
    let holder = RefsCacheHolder::with_cache(Arc::clone(cache), projection.clone());
    holder
        .erase(EraseScope::Subject {
            subject: subject_ref(subject_id, &corpus.tenant),
            tenant: gtenant(&corpus.tenant),
        })
        .expect("§4.6 cache-PII purge");

    dek.destroy_subject_backstop(&corpus.tenant, subject_id);

    let mut edge_ids = Vec::new();
    for edge in corpus.edges_of(subject_id) {
        projection.tombstone(
            &corpus.tenant,
            &corpus.region,
            &edge.edge_id,
            &format!("erased:{subject_id}"),
        );
        edge_ids.push(edge.edge_id.clone());
    }

    ledger.record(
        &corpus.tenant,
        &corpus.region,
        subject_id,
        key_refs,
        &edge_ids,
        now,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn re_erase_at_backup_scale(
    corpus: &BackupScaleErasureCorpus,
    builder: &RefsEdgeBuilder,
    cache: &Arc<R2ProjectionCache>,
    dek: &RefsDekPin,
    ledger: &RefsErasureLedger,
    subjects_to_erase: &[String],
    now: &str,
) -> BackupScaleReEraseReport {
    let projection = builder.projection().clone();

    for edge in &corpus.edges {
        builder.handle(&corpus.edge_event(edge), &mut myelin_events::HandlerTx::none());
    }
    let mut subject_keys: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for subject_id in &corpus.subjects {
        let keys = warm_subject_titles(corpus, cache, dek, subject_id);
        subject_keys.insert(subject_id.clone(), keys);
    }

    for subject_id in subjects_to_erase {
        let keys = subject_keys.get(subject_id).cloned().unwrap_or_default();
        erase_and_record_at_scale(
            corpus,
            cache,
            dek,
            &projection,
            ledger,
            subject_id,
            &keys,
            now,
        );
    }

    let mut cached_titles_resurrected_by_restore = 0usize;
    let mut deks_resurrected_by_restore = 0usize;
    for subject_id in subjects_to_erase {
        dek.reserve_subject_backstop(&corpus.tenant, &corpus.region, subject_id)
            .expect("restore re-seals the per-subject DEK");
        deks_resurrected_by_restore += 1;
        for edge in corpus.edges_of(subject_id) {
            let proj = Projection {
                ref_: edge.source.clone(),
                title: edge.cached_title.clone(),
                state: "open".into(),
                icon: "doc".into(),
                render_hint: "card".into(),
                sub_anchor: None,
                flag: None,
            };
            cache
                .fill(&corpus.tenant, &corpus.region, &edge.source, &proj)
                .expect("restore re-warms the cached title");
            if cache
                .read(&corpus.tenant, &corpus.region, &edge.source)
                .is_some()
            {
                cached_titles_resurrected_by_restore += 1;
            }
        }
    }
    for edge in &corpus.edges {
        if subjects_to_erase.contains(&edge.subject_id) {
            builder.handle(&corpus.edge_event(edge), &mut myelin_events::HandlerTx::none());
        }
    }

    let mut edges_re_tombstoned = 0usize;
    for entry in ledger.entries() {
        if entry.tenant != corpus.tenant || entry.region != corpus.region {
            continue;
        }
        let holder = RefsCacheHolder::with_cache(Arc::clone(cache), projection.clone());
        holder
            .erase(EraseScope::Subject {
                subject: subject_ref(&entry.subject_id, &corpus.tenant),
                tenant: gtenant(&corpus.tenant),
            })
            .expect("re-erase cache purge");
        for _ in &entry.key_refs {
            dek.destroy_subject_backstop(&corpus.tenant, &entry.subject_id);
        }
        for edge_id in &entry.edge_ids {
            projection.tombstone(
                &corpus.tenant,
                &corpus.region,
                edge_id,
                &format!("re-erased:{}", entry.subject_id),
            );
            edges_re_tombstoned += 1;
        }
    }

    let mut recoverable_pii = 0usize;
    let mut live_edges_post_reerase = 0usize;
    for subject_id in subjects_to_erase {
        for edge in corpus.edges_of(subject_id) {
            if cache
                .read(&corpus.tenant, &corpus.region, &edge.source)
                .is_some()
            {
                recoverable_pii += 1;
            }
            if projection
                .get(&corpus.tenant, &corpus.region, &edge.edge_id)
                .map(|r| !r.tombstoned)
                .unwrap_or(false)
            {
                live_edges_post_reerase += 1;
            }
        }
    }
    let mut live_deks_post_reerase = 0usize;
    for subject_id in subjects_to_erase {
        if dek.subject_backstop_is_live(&corpus.tenant, &corpus.region, subject_id) {
            live_deks_post_reerase += 1;
        }
    }

    BackupScaleReEraseReport {
        tenant: corpus.tenant.clone(),
        region: corpus.region.clone(),
        re_erased_subjects: ledger
            .entries()
            .iter()
            .filter(|e| e.tenant == corpus.tenant && e.region == corpus.region)
            .count(),
        cached_titles_resurrected_by_restore,
        deks_resurrected_by_restore,
        edges_re_tombstoned,
        recoverable_pii,
        live_deks_post_reerase,
        live_edges_post_reerase,
        ran_at: now.to_string(),
    }
}

fn subject_ref(subject_id: &str, tenant: &TenantId) -> SubjectRef {
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    SubjectRef::new(Principal::stub(
        PrincipalId(subject_id.into()),
        PrincipalKind::Human,
        tenant.clone(),
    ))
}

fn gtenant(tenant: &TenantId) -> GdprTenantId {
    tenant.clone()
}

#[cfg(test)]
mod tests;
