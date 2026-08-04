use crate::events;
use crate::keys::{CanonicalKey, HiLoKeyAllocator, PrefixReserve, ReserveError};
use myelin_content::adf::{mapping_for, AdfNode, AdfTarget, ImportReport, Loss};
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventType, IdMinter,
    OutboxError, OutboxStore, OutboxTx, TenantId, Visibility,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceSystem {
    Jira,
    Linear,
    GitHub,
    Csv,
}

impl SourceSystem {
    pub fn token(self) -> &'static str {
        match self {
            SourceSystem::Jira => "jira",
            SourceSystem::Linear => "linear",
            SourceSystem::GitHub => "github",
            SourceSystem::Csv => "csv",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalRelation {
    pub src_source_id: String,
    pub dst_source_id: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalIssue {
    pub source_id: String,
    pub project_key: String,
    pub title: String,
    pub body_md: String,
    pub reporter_pseudonym: String,
    pub state: String,
    pub contains_pii: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalImport {
    pub source_system: SourceSystem,
    pub issues: Vec<CanonicalIssue>,
    pub relations: Vec<CanonicalRelation>,
    pub report: ImportReport,
}

impl CanonicalImport {
    pub fn new(source_system: SourceSystem) -> CanonicalImport {
        CanonicalImport {
            source_system,
            issues: Vec::new(),
            relations: Vec::new(),
            report: ImportReport::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRecord {
    pub source_id: String,
    pub project_key: String,
    pub title: String,
    pub body_md: String,
    pub body_adf: Vec<AdfBodyNode>,
    pub reporter_pseudonym: String,
    pub state: String,
    pub contains_pii: bool,
    pub relations: Vec<CanonicalRelation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdfBodyNode {
    pub kind: AdfNode,
    pub text: String,
    pub resolved: bool,
}

pub trait SourceAdapter {
    fn source_system(&self) -> SourceSystem;

    fn normalise(&self, records: &[ProviderRecord]) -> CanonicalImport;
}

pub const UNSUPPORTED_PERMISSION_SCHEME: &str =
    "permission-scheme mapping is the R-9 legal-review leg (M5+) - recorded, not auto-mapped";

fn convert_adf_body(nodes: &[AdfBodyNode], report: &mut ImportReport) -> String {
    let mut lines: Vec<String> = Vec::new();
    for node in nodes {
        let mapping = mapping_for(node.kind);
        let effective_target = match &mapping.loss {
            Loss::None => mapping.target,
            Loss::Lossy { what } => {
                report.record(node.kind, mapping.target, what.to_string());
                mapping.target
            }
            Loss::Conditional {
                what, degraded_to, ..
            } => {
                if node.resolved {
                    mapping.target
                } else {
                    report.record(node.kind, *degraded_to, what.to_string());
                    *degraded_to
                }
            }
        };
        lines.push(render_degraded(effective_target, &node.text));
    }
    lines.join("\n")
}

fn render_degraded(target: AdfTarget, text: &str) -> String {
    match target {
        AdfTarget::Paragraph
        | AdfTarget::Heading
        | AdfTarget::Blockquote
        | AdfTarget::PlainText
        | AdfTarget::UnicodeGlyph
        | AdfTarget::FlattenedBlocks
        | AdfTarget::Mention
        | AdfTarget::ArtifactRef => text.to_string(),
        AdfTarget::CodeBlock => format!("```\n{text}\n```"),
        AdfTarget::Divider => "---".to_string(),
        AdfTarget::BulletList | AdfTarget::TaskList | AdfTarget::TaskItem => format!("- {text}"),
        AdfTarget::OrderedList => format!("1. {text}"),
        AdfTarget::Table => format!("| {text} |"),
        AdfTarget::Image | AdfTarget::ImageWithAttachments => format!("![{text}]()"),
        AdfTarget::Callout => format!("> {text}\n> [unsupported macro: {text}]"),
        AdfTarget::Toggle => format!("<details>{text}</details>"),
        AdfTarget::Link => format!("[{text}]({text})"),
        AdfTarget::InlineCode => format!("`{text}`"),
    }
}

fn normalise_markdown_native(
    source_system: SourceSystem,
    records: &[ProviderRecord],
) -> CanonicalImport {
    let mut import = CanonicalImport::new(source_system);
    for r in records {
        import.issues.push(CanonicalIssue {
            source_id: r.source_id.clone(),
            project_key: r.project_key.clone(),
            title: r.title.clone(),
            body_md: r.body_md.clone(),
            reporter_pseudonym: r.reporter_pseudonym.clone(),
            state: r.state.clone(),
            contains_pii: r.contains_pii,
        });
        import.relations.extend(r.relations.iter().cloned());
    }
    import
}

#[derive(Clone, Copy, Debug, Default)]
pub struct JiraAdapter;

impl SourceAdapter for JiraAdapter {
    fn source_system(&self) -> SourceSystem {
        SourceSystem::Jira
    }

    fn normalise(&self, records: &[ProviderRecord]) -> CanonicalImport {
        let mut import = CanonicalImport::new(SourceSystem::Jira);
        for r in records {
            let body_md = if r.body_adf.is_empty() {
                r.body_md.clone()
            } else {
                convert_adf_body(&r.body_adf, &mut import.report)
            };
            import.issues.push(CanonicalIssue {
                source_id: r.source_id.clone(),
                project_key: r.project_key.clone(),
                title: r.title.clone(),
                body_md,
                reporter_pseudonym: r.reporter_pseudonym.clone(),
                state: r.state.clone(),
                contains_pii: r.contains_pii,
            });
            import.relations.extend(r.relations.iter().cloned());
        }
        import
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LinearAdapter;

impl SourceAdapter for LinearAdapter {
    fn source_system(&self) -> SourceSystem {
        SourceSystem::Linear
    }
    fn normalise(&self, records: &[ProviderRecord]) -> CanonicalImport {
        normalise_markdown_native(SourceSystem::Linear, records)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GitHubAdapter;

impl SourceAdapter for GitHubAdapter {
    fn source_system(&self) -> SourceSystem {
        SourceSystem::GitHub
    }
    fn normalise(&self, records: &[ProviderRecord]) -> CanonicalImport {
        normalise_markdown_native(SourceSystem::GitHub, records)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CsvAdapter;

impl SourceAdapter for CsvAdapter {
    fn source_system(&self) -> SourceSystem {
        SourceSystem::Csv
    }
    fn normalise(&self, records: &[ProviderRecord]) -> CanonicalImport {
        normalise_markdown_native(SourceSystem::Csv, records)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdMapEntry {
    pub canonical_key: CanonicalKey,
    pub artifact_ref: ArtifactRef,
}

pub trait SourceIdMap {
    fn get(
        &self,
        tenant: &TenantId,
        source_system: SourceSystem,
        source_id: &str,
    ) -> Option<IdMapEntry>;

    fn put(
        &self,
        tenant: &TenantId,
        source_system: SourceSystem,
        source_id: &str,
        entry: IdMapEntry,
    );

    fn remove(&self, tenant: &TenantId, source_system: SourceSystem, source_id: &str);

    fn count(&self, tenant: &TenantId, source_system: SourceSystem) -> usize;
}

#[derive(Debug, Default)]
pub struct InMemorySourceIdMap {
    inner: std::sync::Mutex<BTreeMap<(String, &'static str, String), IdMapEntry>>,
}

impl InMemorySourceIdMap {
    pub fn new() -> InMemorySourceIdMap {
        InMemorySourceIdMap::default()
    }

    fn key(
        tenant: &TenantId,
        source_system: SourceSystem,
        source_id: &str,
    ) -> (String, &'static str, String) {
        (
            tenant.0.clone(),
            source_system.token(),
            source_id.to_string(),
        )
    }
}

impl SourceIdMap for InMemorySourceIdMap {
    fn get(
        &self,
        tenant: &TenantId,
        source_system: SourceSystem,
        source_id: &str,
    ) -> Option<IdMapEntry> {
        self.inner
            .lock()
            .expect("id-map mutex")
            .get(&Self::key(tenant, source_system, source_id))
            .cloned()
    }

    fn put(
        &self,
        tenant: &TenantId,
        source_system: SourceSystem,
        source_id: &str,
        entry: IdMapEntry,
    ) {
        self.inner
            .lock()
            .expect("id-map mutex")
            .insert(Self::key(tenant, source_system, source_id), entry);
    }

    fn remove(&self, tenant: &TenantId, source_system: SourceSystem, source_id: &str) {
        self.inner.lock().expect("id-map mutex").remove(&Self::key(
            tenant,
            source_system,
            source_id,
        ));
    }

    fn count(&self, tenant: &TenantId, source_system: SourceSystem) -> usize {
        self.inner
            .lock()
            .expect("id-map mutex")
            .keys()
            .filter(|(t, s, _)| t == &tenant.0 && *s == source_system.token())
            .count()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unresolved {
    pub relation: CanonicalRelation,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconciliationReport {
    pub created: usize,
    pub skipped_already_mapped: usize,
    pub relations_created: usize,
    pub unresolved: Vec<Unresolved>,
    pub loss: ImportReport,
    pub legal_review: Vec<String>,
}

impl ReconciliationReport {
    pub fn new() -> ReconciliationReport {
        ReconciliationReport::default()
    }

    pub fn is_clean(&self) -> bool {
        self.loss.is_lossless() && self.unresolved.is_empty() && self.legal_review.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DryRun {
    pub report: ReconciliationReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportLaneBudget {
    pub max_in_flight: usize,
}

impl ImportLaneBudget {
    pub const DEFAULT_MAX_IN_FLIGHT: usize = 64;

    pub fn default_budget() -> ImportLaneBudget {
        ImportLaneBudget {
            max_in_flight: Self::DEFAULT_MAX_IN_FLIGHT,
        }
    }

    pub fn batches(&self, total: usize) -> Vec<usize> {
        let cap = self.max_in_flight.max(1);
        let mut out = Vec::new();
        let mut remaining = total;
        while remaining > 0 {
            let take = remaining.min(cap);
            out.push(take);
            remaining -= take;
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    KeyReserve(String),
    Emit(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::KeyReserve(e) => write!(f, "import key reserve failed: {e}"),
            ImportError::Emit(e) => write!(f, "import emit failed: {e}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<ReserveError> for ImportError {
    fn from(e: ReserveError) -> Self {
        ImportError::KeyReserve(e.to_string())
    }
}

pub struct ImportEngine<'a, R: PrefixReserve, M: SourceIdMap> {
    allocator: &'a HiLoKeyAllocator<R>,
    id_map: &'a M,
    budget: ImportLaneBudget,
}

impl<'a, R: PrefixReserve, M: SourceIdMap> ImportEngine<'a, R, M> {
    pub fn new(
        allocator: &'a HiLoKeyAllocator<R>,
        id_map: &'a M,
        budget: ImportLaneBudget,
    ) -> ImportEngine<'a, R, M> {
        ImportEngine {
            allocator,
            id_map,
            budget,
        }
    }

    pub fn dry_run(
        &self,
        tenant: &TenantId,
        import: &CanonicalImport,
        has_permission_scheme: bool,
    ) -> DryRun {
        let mut report = ReconciliationReport::new();
        report.loss = import.report.clone();

        let mut would_be_mapped: HashMap<&str, ()> = HashMap::new();
        for issue in &import.issues {
            if self
                .id_map
                .get(tenant, import.source_system, &issue.source_id)
                .is_some()
            {
                report.skipped_already_mapped += 1;
            } else {
                report.created += 1;
            }
            would_be_mapped.insert(issue.source_id.as_str(), ());
        }

        for rel in &import.relations {
            let src_ok = would_be_mapped.contains_key(rel.src_source_id.as_str())
                || self
                    .id_map
                    .get(tenant, import.source_system, &rel.src_source_id)
                    .is_some();
            let dst_ok = would_be_mapped.contains_key(rel.dst_source_id.as_str())
                || self
                    .id_map
                    .get(tenant, import.source_system, &rel.dst_source_id)
                    .is_some();
            if src_ok && dst_ok {
                report.relations_created += 1;
            } else {
                report.unresolved.push(Unresolved {
                    relation: rel.clone(),
                    reason: unresolved_reason(src_ok, dst_ok),
                });
            }
        }

        if has_permission_scheme {
            report
                .legal_review
                .push(UNSUPPORTED_PERMISSION_SCHEME.to_string());
        }

        DryRun { report }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        tenant: &TenantId,
        import: &CanonicalImport,
        has_permission_scheme: bool,
        store: &OutboxStore,
        minter: Arc<dyn IdMinter>,
        ctx_base: EmitContextBase,
        crash_after_batch: Option<usize>,
    ) -> Result<ReconciliationReport, ImportError> {
        let mut report = ReconciliationReport::new();
        report.loss = import.report.clone();
        if has_permission_scheme {
            report
                .legal_review
                .push(UNSUPPORTED_PERMISSION_SCHEME.to_string());
        }

        let batches = self.budget.batches(import.issues.len());
        let mut offset = 0usize;
        for (batch_idx, batch_len) in batches.into_iter().enumerate() {
            let mut tx = store.begin(Arc::clone(&minter), ctx_base.clone());
            tx.stage_state_change(format!("import pass1 batch {batch_idx}"));
            for issue in &import.issues[offset..offset + batch_len] {
                if self
                    .id_map
                    .get(tenant, import.source_system, &issue.source_id)
                    .is_some()
                {
                    report.skipped_already_mapped += 1;
                    continue;
                }
                let key = self.allocator.allocate(tenant, &issue.project_key)?;
                let artifact_ref = key.issue_artifact_ref(tenant);
                let draft = issue_created_draft(&key, &artifact_ref, issue);
                tx.emit(draft, None).map_err(emit_err)?;
                self.id_map.put(
                    tenant,
                    import.source_system,
                    &issue.source_id,
                    IdMapEntry {
                        canonical_key: key.clone(),
                        artifact_ref: artifact_ref.clone(),
                    },
                );
                report.created += 1;
            }
            tx.commit().map_err(emit_err)?;
            offset += batch_len;
            if crash_after_batch == Some(batch_idx) {
                return Ok(report);
            }
        }

        let rel_batches = self.budget.batches(import.relations.len());
        let mut roffset = 0usize;
        for (batch_idx, batch_len) in rel_batches.into_iter().enumerate() {
            let mut tx = store.begin(Arc::clone(&minter), ctx_base.clone());
            tx.stage_state_change(format!("import pass2 batch {batch_idx}"));
            for rel in &import.relations[roffset..roffset + batch_len] {
                let src = self
                    .id_map
                    .get(tenant, import.source_system, &rel.src_source_id);
                let dst = self
                    .id_map
                    .get(tenant, import.source_system, &rel.dst_source_id);
                match (src, dst) {
                    (Some(s), Some(d)) => {
                        let draft = relation_created_draft(&s, &d, &rel.kind);
                        tx.emit(draft, None).map_err(emit_err)?;
                        report.relations_created += 1;
                    }
                    (s, d) => {
                        report.unresolved.push(Unresolved {
                            relation: rel.clone(),
                            reason: unresolved_reason(s.is_some(), d.is_some()),
                        });
                    }
                }
            }
            tx.commit().map_err(emit_err)?;
            roffset += batch_len;
        }

        Ok(report)
    }

    pub fn rollback(&self, tenant: &TenantId, import: &CanonicalImport) -> usize {
        let mut removed = 0;
        for issue in &import.issues {
            if self
                .id_map
                .get(tenant, import.source_system, &issue.source_id)
                .is_some()
            {
                self.id_map
                    .remove(tenant, import.source_system, &issue.source_id);
                removed += 1;
            }
        }
        removed
    }
}

fn unresolved_reason(src_ok: bool, dst_ok: bool) -> String {
    match (src_ok, dst_ok) {
        (false, false) => "both endpoints are outside the import set (unmapped)".to_string(),
        (false, true) => "the source endpoint is outside the import set (unmapped)".to_string(),
        (true, false) => {
            "the destination endpoint is outside the import set (unmapped)".to_string()
        }
        (true, true) => "resolved".to_string(),
    }
}

fn emit_err(e: OutboxError) -> ImportError {
    ImportError::Emit(e.0)
}

fn issue_created_draft(
    key: &CanonicalKey,
    artifact_ref: &ArtifactRef,
    issue: &CanonicalIssue,
) -> EventDraft {
    EventDraft {
        type_: EventType(events::ISSUE_CREATED.into()),
        subject: artifact_ref.clone(),
        aggregate: AggregateKey(format!("issue:{}:{}", issue.project_key, key.render())),
        payload: serde_json::json!({
            "issue": artifact_ref.0,
            "canonical_key": key.render(),
            "state": issue.state,
            "imported": true,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: issue.contains_pii,
        pii_key_ref: None,
    }
}

fn relation_created_draft(src: &IdMapEntry, dst: &IdMapEntry, kind: &str) -> EventDraft {
    EventDraft {
        type_: EventType(events::RELATION_CREATED.into()),
        subject: src.artifact_ref.clone(),
        aggregate: AggregateKey(format!("issue:{}", src.canonical_key.render())),
        payload: serde_json::json!({
            "src": src.artifact_ref.0,
            "dst": dst.artifact_ref.0,
            "kind": kind,
            "imported": true,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

pub fn adapter_for(source_system: SourceSystem) -> Box<dyn SourceAdapter> {
    match source_system {
        SourceSystem::Jira => Box::new(JiraAdapter),
        SourceSystem::Linear => Box::new(LinearAdapter),
        SourceSystem::GitHub => Box::new(GitHubAdapter),
        SourceSystem::Csv => Box::new(CsvAdapter),
    }
}

#[cfg(test)]
mod tests;
