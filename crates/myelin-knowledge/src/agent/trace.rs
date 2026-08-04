use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_content::Block;
use myelin_events::ArtifactRef;
use myelin_gdpr::{Receipt, SubjectRef, TenantId};
use myelin_search::engine::IndexBackend;
use myelin_storage::{ErasureLedgerSink, KmsEngine, PseudonymShred};

use crate::compaction::content_address;
use crate::gdpr::erase_floor::{
    BusEraseSeam, KnowledgeBacklinkTombstone, KnowledgeEmbeddingPurge, KnowledgeErase,
};

pub const TRACE_HOLDER_ID: &str = "agent_fabric_trace";

pub const AUDIT_LOG_STORE_ID: &str = "audit_log";

pub fn trace_is_distinct_from_audit() -> bool {
    let distinct_ids = TRACE_HOLDER_ID != AUDIT_LOG_STORE_ID;
    let distinct_erasability = TRACE_ERASABLE && !AUDIT_LOG_ERASABLE;
    distinct_ids && distinct_erasability
}

pub const TRACE_ERASABLE: bool = true;
pub const AUDIT_LOG_ERASABLE: bool = false;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentTrace {
    pub run_id: String,
    pub actor_principal_id: String,
    pub blocks: Vec<Block>,
}

impl AgentTrace {
    pub fn new(
        run_id: impl Into<String>,
        actor_principal_id: impl Into<String>,
        blocks: Vec<Block>,
    ) -> AgentTrace {
        AgentTrace {
            run_id: run_id.into(),
            actor_principal_id: actor_principal_id.into(),
            blocks,
        }
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let value = serde_json::json!({
            "schema": "myelin.agent_trace.v1",
            "run_id": self.run_id,
            "actor": self.actor_principal_id,
            "blocks": self.blocks,
        });
        serde_json::to_vec(&value).expect("the agent trace serialises (a closed serde shape)")
    }

    pub fn trace_ref(&self, tenant: &TenantId) -> ArtifactRef {
        let hash = content_address(&self.canonical_bytes());
        ArtifactRef(format!(
            "myelin://{}/knowledge/agent_trace/{}",
            tenant.as_str(),
            hash.to_multihash_string()
        ))
    }

    pub fn content_hash(&self) -> String {
        content_address(&self.canonical_bytes()).to_multihash_string()
    }
}

#[derive(Default)]
pub struct AgentTraceHolder {
    traces: Mutex<BTreeMap<String, StoredTrace>>,
}

#[derive(Clone, Debug)]
struct StoredTrace {
    tenant: TenantId,
    trace: AgentTrace,
}

impl AgentTraceHolder {
    pub fn new() -> AgentTraceHolder {
        AgentTraceHolder::default()
    }

    pub fn holder_id(&self) -> &'static str {
        TRACE_HOLDER_ID
    }

    pub fn write(&self, tenant: &TenantId, trace: AgentTrace) -> ArtifactRef {
        let key = trace.content_hash();
        let trace_ref = trace.trace_ref(tenant);
        let mut store = self.traces.lock().expect("agent trace holder poisoned");
        store.entry(key).or_insert_with(|| StoredTrace {
            tenant: tenant.clone(),
            trace,
        });
        trace_ref
    }

    pub fn len(&self) -> usize {
        self.traces
            .lock()
            .expect("agent trace holder poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains_ref(&self, trace_ref: &ArtifactRef) -> bool {
        let Some((_, hash)) = trace_ref.0.rsplit_once("/agent_trace/") else {
            return false;
        };
        self.traces
            .lock()
            .expect("agent trace holder poisoned")
            .contains_key(hash)
    }

    pub fn subject_trace_hashes(&self, subject: &SubjectRef, tenant: &TenantId) -> Vec<String> {
        let sid = &subject.principal.principal_id.0;
        self.traces
            .lock()
            .expect("agent trace holder poisoned")
            .iter()
            .filter(|(_, st)| {
                st.tenant.as_str() == tenant.as_str() && &st.trace.actor_principal_id == sid
            })
            .map(|(k, _)| k.clone())
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn erase_subject_traces<B: IndexBackend>(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        region: myelin_tenancy::Region,
        engine: &KmsEngine,
        pseudonym: &dyn PseudonymShred,
        bus: &dyn BusEraseSeam,
        ledger: &dyn ErasureLedgerSink,
        index: &Mutex<B>,
        page_store: &Mutex<crate::refs_glue::PageStore>,
        now: myelin_storage::EpochMillis,
    ) -> Result<TraceEraseReceipt, myelin_storage::EraseError> {
        let hashes = self.subject_trace_hashes(subject, tenant);
        let trace_doc_ids: Vec<String> = hashes
            .iter()
            .map(|h| format!("kn:agent_trace:{h}"))
            .collect();
        let trace_refs: Vec<ArtifactRef> = hashes
            .iter()
            .map(|h| {
                ArtifactRef(format!(
                    "myelin://{}/knowledge/agent_trace/{h}",
                    tenant.as_str()
                ))
            })
            .collect();
        let trace_count = hashes.len();

        let eraser = KnowledgeErase::new(engine, region);
        let embeddings = KnowledgeEmbeddingPurge::new(index, trace_doc_ids);
        let backlinks = KnowledgeBacklinkTombstone::new(page_store, trace_refs);
        let kn_receipt = eraser.erase_subject(
            subject,
            tenant,
            pseudonym,
            &embeddings,
            &backlinks,
            bus,
            ledger,
            now,
        )?;

        let attribution_pseudonym = subject.principal.principal_id.0.clone();

        let receipt = Receipt::content_addressed(
            "erase",
            TRACE_HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            "kn agent-trace erase (KN-D12): per-subject DEK crypto-shred of the trace content \
             (unrecoverable in op-log/snapshots/backups, 11.4) + index purge + backlink tombstone; \
             attribution falls back to the opaque pseudonym (4.8); DISTINCT from the audit log (§6.5)",
            None,
            now,
        );

        Ok(TraceEraseReceipt {
            receipt,
            traces_shredded: trace_count,
            recoverable_in_backup: kn_receipt.recoverable_in_backup,
            embeddings_purged: kn_receipt.embeddings_purged,
            backlinks_tombstoned: kn_receipt.backlinks_tombstoned,
            attribution_pseudonym,
            re_run: kn_receipt.re_run,
            at_ms: now,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceEraseReceipt {
    pub receipt: Receipt,
    pub traces_shredded: usize,
    pub recoverable_in_backup: usize,
    pub embeddings_purged: usize,
    pub backlinks_tombstoned: usize,
    pub attribution_pseudonym: String,
    pub re_run: bool,
    pub at_ms: u64,
}

impl TraceEraseReceipt {
    pub fn is_green(&self) -> bool {
        self.recoverable_in_backup == 0 && !self.attribution_pseudonym.is_empty()
    }
}

pub fn write_agent_trace(
    holder: &AgentTraceHolder,
    tenant: &TenantId,
    run_id: impl Into<String>,
    content: Vec<Block>,
    actor_principal_id: impl Into<String>,
) -> ArtifactRef {
    let trace = AgentTrace::new(run_id, actor_principal_id, content);
    holder.write(tenant, trace)
}

#[cfg(test)]
mod tests;
