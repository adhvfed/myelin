use std::collections::BTreeMap;
use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_gdpr::SubjectRef;
use myelin_query::FieldType;
use myelin_storage::{DekHandle, KmsError, NONCE_LEN};
use myelin_tenancy::{Region, TenantId};

use crate::dek::SearchDekPin;
use crate::engine::SubjectMatcher;
use crate::erase::SearchEraseHolder;
use crate::indexer::{IncrementalIndexer, MockEmbeddingAdapter, SearchProjection};

#[derive(Clone, Debug)]
pub struct SealedBackupSegment {
    pub doc_id: String,
    nonce: [u8; NONCE_LEN],
    ciphertext: Vec<u8>,
}

impl SealedBackupSegment {
    pub fn seal(dek: &DekHandle, doc_id: &str, plaintext: &[u8]) -> SealedBackupSegment {
        let (nonce, ciphertext) = dek.seal(plaintext);
        SealedBackupSegment {
            doc_id: doc_id.to_string(),
            nonce,
            ciphertext,
        }
    }

    pub fn try_recover(&self, dek: &DekHandle) -> Option<Vec<u8>> {
        dek.open(&self.nonce, &self.ciphertext)
    }

    pub fn to_blob_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + self.nonce.len() + self.ciphertext.len());
        out.push(self.nonce.len() as u8);
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&self.ciphertext);
        out
    }

    pub fn from_blob_bytes(doc_id: &str, bytes: &[u8]) -> Option<SealedBackupSegment> {
        let (&nonce_len, rest) = bytes.split_first()?;
        let nonce_len = nonce_len as usize;
        if nonce_len != NONCE_LEN || rest.len() < nonce_len {
            return None;
        }
        let (nonce_slice, ciphertext) = rest.split_at(nonce_len);
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(nonce_slice);
        Some(SealedBackupSegment {
            doc_id: doc_id.to_string(),
            nonce,
            ciphertext: ciphertext.to_vec(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupScaleEraseArtifact {
    pub tenant: TenantId,
    pub region: Region,
    pub live_docs_purged: usize,
    pub live_docs_remaining: usize,
    pub zero_orphan_embedding: bool,
    pub backup_segments_recoverable_before_shred: usize,
    pub backup_segments_recoverable_after_shred: usize,
    pub ran_at: String,
}

impl BackupScaleEraseArtifact {
    pub fn is_green(&self) -> bool {
        self.live_docs_remaining == 0
            && self.zero_orphan_embedding
            && self.backup_segments_recoverable_after_shred == 0
            && self.backup_segments_recoverable_before_shred > 0
    }

    pub fn summary(&self) -> String {
        format!(
            "search backup-scale erasure PASS (SRCH-D4 at backup scale): purged {} live doc(s); \
             live_docs_remaining={} (MUST be 0: purged not hidden); 0-orphan-embedding={}; backups: \
             {} segment(s) recoverable BEFORE the crypto-shred -> {} recoverable AFTER (MUST be 0: \
             the per-tenant index DEK / per-subject backstop is destroyed, §7.5 - 0 recoverable incl. \
             vectors incl. backups).",
            self.live_docs_purged,
            self.live_docs_remaining,
            self.zero_orphan_embedding,
            self.backup_segments_recoverable_before_shred,
            self.backup_segments_recoverable_after_shred,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackupScaleEraseFailure {
    LiveEraseFailed(String),
    KmsUnavailable(String),
    LiveDocsRemain(usize),
    OrphanEmbedding,
    BackupRecoverableAfterShred(usize),
    NoBackupBeforeShred,
}

impl core::fmt::Display for BackupScaleEraseFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BackupScaleEraseFailure::LiveEraseFailed(e) => write!(
                f,
                "SEARCH BACKUP-SCALE ERASE FAIL - the live erase failed: {e}"
            ),
            BackupScaleEraseFailure::KmsUnavailable(error) => write!(
                f,
                "SEARCH BACKUP-SCALE ERASE FAIL - KMS unavailable during shred: {error}"
            ),
            BackupScaleEraseFailure::LiveDocsRemain(n) => write!(
                f,
                "SEARCH BACKUP-SCALE ERASE FAIL - {n} live doc(s) STILL reference the subject after \
                 the erase (purged-not-hidden violated)"
            ),
            BackupScaleEraseFailure::OrphanEmbedding => write!(
                f,
                "SEARCH BACKUP-SCALE ERASE FAIL - an orphan embedding survived the compaction \
                 (embeddings are personal data, §3.3)"
            ),
            BackupScaleEraseFailure::BackupRecoverableAfterShred(n) => write!(
                f,
                "SEARCH BACKUP-SCALE ERASE FAIL - {n} BACKUP SEGMENT(S) STILL RECOVERABLE after the \
                 crypto-shred: the per-tenant index DEK / per-subject backstop destroy did NOT reach \
                 the backups (§7.5 violated - a restore could resurrect the subject). THE GRAVEST \
                 backup-scale failure"
            ),
            BackupScaleEraseFailure::NoBackupBeforeShred => write!(
                f,
                "SEARCH BACKUP-SCALE ERASE FAIL - the backup proof is vacuous: no backup segment was \
                 recoverable BEFORE the shred, so `0 recoverable after` proves nothing"
            ),
        }
    }
}

impl std::error::Error for BackupScaleEraseFailure {}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a backup-scale erasure verdict must be checked - a dropped RED is a SWALLOWED \
              recoverable-backup / un-erased-subject failure (SRCH-D4 at backup scale, EI-01 §5)"]
pub enum BackupScaleEraseVerdict {
    Green(BackupScaleEraseArtifact),
    Red(BackupScaleEraseFailure),
}

impl BackupScaleEraseVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, BackupScaleEraseVerdict::Green(_))
    }
    pub fn artifact(&self) -> Option<&BackupScaleEraseArtifact> {
        match self {
            BackupScaleEraseVerdict::Green(a) => Some(a),
            BackupScaleEraseVerdict::Red(_) => None,
        }
    }
    pub fn failure(&self) -> Option<&BackupScaleEraseFailure> {
        match self {
            BackupScaleEraseVerdict::Red(f) => Some(f),
            BackupScaleEraseVerdict::Green(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BackupScaleEraseGate;

impl BackupScaleEraseGate {
    pub fn new() -> BackupScaleEraseGate {
        BackupScaleEraseGate
    }

    pub fn run(&self, inputs: &mut BackupScaleEraseInputs<'_>) -> BackupScaleEraseVerdict {
        let tenant = inputs.tenant.clone();
        let region = inputs.erase_holder.region().clone();

        let live_dek = match inputs.dek.resolve(&inputs.index_key_ref, &region) {
            Ok(h) => h,
            Err(e) => {
                return BackupScaleEraseVerdict::Red(BackupScaleEraseFailure::LiveEraseFailed(
                    format!("could not resolve the live index DEK to seal/probe backups: {e}"),
                ));
            }
        };
        let backup_segments_recoverable_before_shred = inputs
            .backup_segments
            .iter()
            .filter(|seg| seg.try_recover(&live_dek).is_some())
            .count();
        if backup_segments_recoverable_before_shred == 0 {
            return BackupScaleEraseVerdict::Red(BackupScaleEraseFailure::NoBackupBeforeShred);
        }

        let outcome = match inputs.erase_holder.erase_subject(&inputs.subject, &tenant) {
            Ok(o) => o,
            Err(e) => {
                return BackupScaleEraseVerdict::Red(BackupScaleEraseFailure::LiveEraseFailed(
                    format!("{e:?}"),
                ));
            }
        };
        let live_docs_remaining = inputs
            .erase_holder
            .locate_doc_count(&inputs.subject, &tenant);
        if live_docs_remaining != 0 {
            return BackupScaleEraseVerdict::Red(BackupScaleEraseFailure::LiveDocsRemain(
                live_docs_remaining,
            ));
        }
        if !outcome.zero_orphan_embedding {
            return BackupScaleEraseVerdict::Red(BackupScaleEraseFailure::OrphanEmbedding);
        }

        if let Some(subject_id) = &inputs.subject_backstop_id {
            if let Err(error) = inputs.dek.destroy_subject_backstop(&tenant, subject_id) {
                return BackupScaleEraseVerdict::Red(BackupScaleEraseFailure::KmsUnavailable(
                    error.to_string(),
                ));
            }
        }
        if let Err(error) = inputs.dek.destroy_tenant_index_dek(&tenant, &region) {
            return BackupScaleEraseVerdict::Red(BackupScaleEraseFailure::KmsUnavailable(
                error.to_string(),
            ));
        }

        let backup_segments_recoverable_after_shred =
            match inputs.dek.resolve(&inputs.index_key_ref, &region) {
                Ok(dead_handle) => inputs
                    .backup_segments
                    .iter()
                    .filter(|seg| seg.try_recover(&dead_handle).is_some())
                    .count(),
                Err(KmsError::KekUnavailable(_)) | Err(KmsError::DekUnavailable(_)) => 0,
                Err(_) => 0,
            };
        if backup_segments_recoverable_after_shred != 0 {
            return BackupScaleEraseVerdict::Red(
                BackupScaleEraseFailure::BackupRecoverableAfterShred(
                    backup_segments_recoverable_after_shred,
                ),
            );
        }

        BackupScaleEraseVerdict::Green(BackupScaleEraseArtifact {
            tenant,
            region,
            live_docs_purged: outcome.docs_purged,
            live_docs_remaining,
            zero_orphan_embedding: outcome.zero_orphan_embedding,
            backup_segments_recoverable_before_shred,
            backup_segments_recoverable_after_shred,
            ran_at: inputs.now.clone(),
        })
    }

    pub fn run_or_fail_ci(
        &self,
        inputs: &mut BackupScaleEraseInputs<'_>,
    ) -> Result<BackupScaleEraseArtifact, BackupScaleEraseFailure> {
        match self.run(inputs) {
            BackupScaleEraseVerdict::Green(a) => Ok(a),
            BackupScaleEraseVerdict::Red(f) => Err(f),
        }
    }
}

pub struct BackupScaleEraseInputs<'a> {
    pub erase_holder: &'a SearchEraseHolder,
    pub dek: &'a SearchDekPin,
    pub index_key_ref: myelin_storage::PiiKeyRef,
    pub subject: SubjectRef,
    pub tenant: TenantId,
    pub backup_segments: &'a [SealedBackupSegment],
    pub subject_backstop_id: Option<String>,
    pub now: String,
}

pub fn backup_scale_page_spec() -> crate::indexer::IndexSpec {
    let mut fields = BTreeMap::new();
    fields.insert("actor".to_string(), FieldType::Principal);
    fields.insert("assignee".to_string(), FieldType::Principal);
    crate::indexer::IndexSpec::new("knowledge", "page", fields).semantic()
}

fn created_event(tenant: &TenantId, region: &Region, doc: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("ev:{doc}")),
        type_: EventType("knowledge.page.created".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: region.clone(),
        actor: Actor(myelin_identity::Principal::stub(
            myelin_identity::PrincipalId("sys".into()),
            myelin_identity::PrincipalKind::Human,
            tenant.clone(),
        )),
        subject: ArtifactRef(doc.into()),
        aggregate: AggregateKey(format!("agg:{doc}")),
        causation_id: None,
        correlation_id: CorrelationId(doc.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: true,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

pub struct MapFetcher {
    map: std::sync::Mutex<std::collections::HashMap<String, SearchProjection>>,
}

impl MapFetcher {
    pub fn new(pairs: impl IntoIterator<Item = (String, SearchProjection)>) -> MapFetcher {
        MapFetcher {
            map: std::sync::Mutex::new(pairs.into_iter().collect()),
        }
    }
}

impl crate::indexer::ProjectFetcher for MapFetcher {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, crate::indexer::ProjectFetchError> {
        match self.map.lock().unwrap().get(&ref_.0) {
            Some(p) => Ok(p.clone()),
            None => Err(crate::indexer::ProjectFetchError::Gone),
        }
    }
}

fn proj(text: &str, fields: BTreeMap<String, myelin_query::FieldValue>) -> SearchProjection {
    SearchProjection {
        text: text.into(),
        fields,
        lang: None,
    }
}

pub fn build_live_corpus(
    tenant: &TenantId,
    region: &Region,
    subject_id: &str,
    subject_docs: &[&str],
    other_docs: &[&str],
) -> (Arc<IncrementalIndexer>, Vec<String>) {
    let mut actor = BTreeMap::new();
    actor.insert(
        "actor".to_string(),
        myelin_query::FieldValue::Principal(subject_id.into()),
    );
    let mut pairs: Vec<(String, SearchProjection)> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for d in subject_docs {
        let ref_ = format!("myelin://{}/knowledge/page/{d}", tenant.0);
        pairs.push((
            ref_.clone(),
            proj(
                &format!("{subject_id}'s note {d} on raft leadership and quorum"),
                actor.clone(),
            ),
        ));
        ids.push(ref_);
    }
    for d in other_docs {
        let ref_ = format!("myelin://{}/knowledge/page/{d}", tenant.0);
        pairs.push((
            ref_.clone(),
            proj(&format!("unrelated note {d} on paxos consensus"), {
                let mut f = BTreeMap::new();
                f.insert(
                    "actor".to_string(),
                    myelin_query::FieldValue::Principal(format!("u-{d}")),
                );
                f
            }),
        ));
        ids.push(ref_);
    }
    let ix = Arc::new(IncrementalIndexer::new(
        vec![backup_scale_page_spec()],
        Arc::new(MapFetcher::new(pairs)),
        Arc::new(MockEmbeddingAdapter::new(8)),
    ));
    for id in &ids {
        ix.index(&created_event(tenant, region, id)).expect("index");
    }
    (ix, ids)
}

pub fn subject_matcher(subject_id: &str, tenant: &TenantId) -> SubjectMatcher {
    let pseudonym =
        myelin_identity::PseudonymHandle::new(subject_id, &tenant.0).map(|h| h.render());
    SubjectMatcher::new(subject_id, pseudonym)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_gdpr::{EraseScope, PersonalDataHolder};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_storage::KmsEngine;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    #[test]
    fn srch_d4_backup_scale_zero_recoverable_incl_backups() {
        let (ix, ids) = build_live_corpus(
            &tenant(),
            &region(),
            "u-target",
            &["t1", "t2"],
            &["o1", "o2", "o3"],
        );
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        let key_ref = pin
            .reserve(&tenant(), &region())
            .expect("reserve index DEK");
        let dek = pin.resolve(&key_ref, &region()).expect("resolve live DEK");
        let backups: Vec<SealedBackupSegment> = ids
            .iter()
            .take(2)
            .map(|id| SealedBackupSegment::seal(&dek, id, b"u-target's design note plaintext"))
            .collect();

        let holder = SearchEraseHolder::new(ix.clone(), pin.clone(), region());

        let mut inputs = BackupScaleEraseInputs {
            erase_holder: &holder,
            dek: &pin,
            index_key_ref: key_ref,
            subject: subject("u-target"),
            tenant: tenant(),
            backup_segments: &backups,
            subject_backstop_id: None,
            now: "2026-06-24T00:00:00Z".into(),
        };

        let verdict = BackupScaleEraseGate::new().run(&mut inputs);
        assert!(verdict.is_green(), "verdict: {:?}", verdict.failure());
        let a = verdict.artifact().expect("green artifact");
        assert_eq!(a.live_docs_purged, 2, "the two subject docs were purged");
        assert_eq!(a.live_docs_remaining, 0, "0 live docs remain (not hidden)");
        assert!(a.zero_orphan_embedding, "0 orphan embedding after compact");
        assert_eq!(
            a.backup_segments_recoverable_before_shred, 2,
            "the backups DID hold the plaintext before the shred (the proof is real)"
        );
        assert_eq!(
            a.backup_segments_recoverable_after_shred, 0,
            "0 backup segments recoverable after the crypto-shred (incl. backups)"
        );
        assert!(a.summary().contains("SRCH-D4 at backup scale"));
    }

    #[test]
    fn srch_d4_backup_segment_is_recoverable_before_and_dead_after_shred() {
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        let key_ref = pin.reserve(&tenant(), &region()).expect("reserve");
        let dek = pin.resolve(&key_ref, &region()).expect("resolve");
        let seg = SealedBackupSegment::seal(&dek, "doc1", b"secret index segment");
        assert_eq!(
            seg.try_recover(&dek).as_deref(),
            Some(&b"secret index segment"[..]),
            "the backup is recoverable while the DEK lives"
        );
        assert!(pin.destroy_tenant_index_dek(&tenant(), &region()).unwrap());
        assert!(
            pin.resolve(&key_ref, &region()).is_err(),
            "the shredded DEK does not resolve - the backup ciphertext is dead (§7.5)"
        );
    }

    #[test]
    fn srch_d4_vacuous_backup_proof_fails_loud() {
        let (ix, _ids) = build_live_corpus(&tenant(), &region(), "u-target", &["t1"], &[]);
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        let key_ref = pin.reserve(&tenant(), &region()).expect("reserve");
        let other = TenantId("other".into());
        let other_ref = pin.reserve(&other, &region()).expect("reserve other");
        let other_dek = pin.resolve(&other_ref, &region()).expect("resolve other");
        let foreign = SealedBackupSegment::seal(&other_dek, "doc1", b"foreign");

        let holder = SearchEraseHolder::new(ix.clone(), pin.clone(), region());
        let mut inputs = BackupScaleEraseInputs {
            erase_holder: &holder,
            dek: &pin,
            index_key_ref: key_ref,
            subject: subject("u-target"),
            tenant: tenant(),
            backup_segments: std::slice::from_ref(&foreign),
            subject_backstop_id: None,
            now: "2026-06-24T00:00:00Z".into(),
        };
        let verdict = BackupScaleEraseGate::new().run(&mut inputs);
        assert_eq!(
            verdict.failure(),
            Some(&BackupScaleEraseFailure::NoBackupBeforeShred),
            "no recoverable backup before the shred → the proof is vacuous → fail loud"
        );
    }

    #[test]
    fn srch_d4_backup_scale_destroys_per_subject_backstop_too() {
        let (ix, ids) = build_live_corpus(&tenant(), &region(), "u-target", &["t1"], &["o1"]);
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        let key_ref = pin.reserve(&tenant(), &region()).expect("reserve");
        pin.reserve_subject_source_backstop(&tenant(), &region(), "u-target")
            .expect("reserve backstop");

        let dek = pin.resolve(&key_ref, &region()).expect("resolve");
        let backups = vec![SealedBackupSegment::seal(&dek, &ids[0], b"plaintext")];
        let holder = SearchEraseHolder::new(ix.clone(), pin.clone(), region());
        let mut inputs = BackupScaleEraseInputs {
            erase_holder: &holder,
            dek: &pin,
            index_key_ref: key_ref,
            subject: subject("u-target"),
            tenant: tenant(),
            backup_segments: &backups,
            subject_backstop_id: Some("u-target".into()),
            now: "2026-06-24T00:00:00Z".into(),
        };
        let verdict = BackupScaleEraseGate::new().run(&mut inputs);
        assert!(verdict.is_green(), "verdict: {:?}", verdict.failure());
        assert!(
            !pin.destroy_subject_backstop(&tenant(), "u-target").unwrap(),
            "the per-subject backstop was destroyed by the gate (a re-destroy is a no-op)"
        );
    }

    #[test]
    fn srch_d4_backup_run_or_fail_ci_err_on_red() {
        let (ix, _ids) = build_live_corpus(&tenant(), &region(), "u-target", &["t1"], &[]);
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        let key_ref = pin.reserve(&tenant(), &region()).expect("reserve");
        let holder = SearchEraseHolder::new(ix.clone(), pin.clone(), region());
        let mut inputs = BackupScaleEraseInputs {
            erase_holder: &holder,
            dek: &pin,
            index_key_ref: key_ref,
            subject: subject("u-target"),
            tenant: tenant(),
            backup_segments: &[],
            subject_backstop_id: None,
            now: "2026-06-24T00:00:00Z".into(),
        };
        let r = BackupScaleEraseGate::new().run_or_fail_ci(&mut inputs);
        assert!(r.is_err(), "a red run → Err (CI fails loud)");
    }

    #[test]
    fn srch_p15_erase_mutation_floor_still_holds() {
        let (ix, _ids) =
            build_live_corpus(&tenant(), &region(), "u-target", &["t1", "t2"], &["o1"]);
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        pin.reserve(&tenant(), &region()).expect("reserve");
        let holder = SearchEraseHolder::new(ix.clone(), pin, region());
        let before = holder.locate_doc_count(&subject("u-target"), &tenant());
        assert_eq!(before, 2, "the subject references two live docs");
        holder
            .erase(EraseScope::Subject {
                subject: subject("u-target"),
                tenant: tenant(),
            })
            .expect("erase");
        let after = holder.locate_doc_count(&subject("u-target"), &tenant());
        assert_eq!(
            after, 0,
            "0 recoverable live after the erase (SRCH-P15 floor)"
        );
    }
}
