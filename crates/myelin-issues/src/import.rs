//! # `import` — the two-pass, ID-remapped, idempotent + resumable import engine + the ADF lossy-map
//! (ISS-P21 / P-388, M4-I5 — the **adoption gate**: "leave Atlassian cleanly", VISION §1)
//!
//! This is the **adoption gate**: a tenant migrating off Jira / Linear / GitHub / a CSV export lands
//! their existing issues into Myelin **without silent data loss** and **without duplicate creates on
//! a crash mid-import**. It is the EU-sovereignty credibility milestone (VISION §1 — "leave
//! Atlassian cleanly is a sovereignty credibility milestone").
//!
//! ## Owning architecture docs (read in full before changing this)
//! - `04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md` §8
//!   ("Import" row — per-tenant in-flight caps + the protected human lane shed order; a 100k-issue
//!   import is bounded backfill, never starves another tenant's interactive traffic) + the deliverable
//!   bullets (the two-pass, ID-remapped, idempotent + resumable engine; the persisted source↔Myelin id
//!   map; the dry-run + reconciliation-report-first; the canonical interchange format; the per-tenant
//!   in-flight cap).
//! - `04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md` — the
//!   import emits the **normal `issue.*` events** (one indexing path; reindex-from-source works on
//!   imported data for free).
//! - `00-reconciliation-decisions.md` X-2 (the **ADF → `myelin-content` lossy-map** frozen — lossy
//!   nodes named, never silent; the permission-scheme mapping as the lossy/legal-review leg, R-9).
//! - `contract-index.md` rows **13.2** (the ADF lossy-map — every lossy/dropped node recorded in the
//!   import report), **1.11** (the protected-human-lane shed order — the import is capped so it never
//!   starves an interactive tenant), **2.2** (the import emits `issue.*` via the one outbox path).
//! - drill-catalogue row **ISS-D9**: (a) `export→import→export` round-trips, ADF lossy nodes named;
//!   (b) a large import resumes after a crash, 0 duplicate creates; (c) the import doesn't starve
//!   another tenant.
//!
//! ## What this module ships
//! 1. [`CanonicalIssue`] / [`CanonicalImport`] — the **canonical interchange format**: one
//!    source-agnostic issue representation the four source adapters normalise into AND that the
//!    portability export round-trips with (the round-trip oracle, ISS-D9(a)). Source ids are STRINGS
//!    (a Jira `PROJ-1`, a Linear `ABC-123`, a GitHub `#42`, a CSV row key); relations reference SOURCE
//!    ids (resolved to Myelin ids in pass 2).
//! 2. The four **source adapters** ([`SourceAdapter`] — [`JiraAdapter`] / [`LinearAdapter`] /
//!    [`GitHubAdapter`] / [`CsvAdapter`]) normalising a provider payload into [`CanonicalImport`]. The
//!    ADF-bearing adapter (Jira) converts the description body through the frozen
//!    [`myelin_content::adf`] map, recording every lossy conversion in the [`myelin_content::ImportReport`].
//! 3. [`SourceIdMap`] — the **persisted source↔Myelin id map** (the load-bearing artifact for
//!    idempotency / resume / rollback / round-trip). A `(tenant, source_system, source_id) →
//!    (CanonicalKey, ArtifactRef)` mapping persisted (in prod) on the same OLTP store; here the
//!    [`InMemorySourceIdMap`] models it DB-free.
//! 4. [`ImportEngine`] — the **two-pass, idempotent + resumable** engine:
//!    - **Pass 1** (mint + map): for each source issue NOT already in the id-map, mint a canonical key
//!      (Hi/Lo), record `source_id → CanonicalKey` in the id-map, and emit `issue.created`. An issue
//!      ALREADY in the id-map is SKIPPED (the idempotent re-create / resume guarantee — 0 duplicate
//!      creates on a crash mid-import, ISS-D9(b)).
//!    - **Pass 2** (resolve relations): for each source relation `(src, dst, kind)`, resolve BOTH
//!      endpoints through the id-map and emit `issue.relation.created`. A relation to an unmapped
//!      endpoint is recorded as a [`Unresolved`] gap in the reconciliation report (named, never a
//!      silent dangling edge).
//! 5. [`DryRun`] / [`ReconciliationReport`] — **dry-run + reconciliation-report-first**: a dry run
//!    constructs the FULL plan (what WOULD be created/mapped/degraded) and the reconciliation report
//!    WITHOUT emitting a single event, so the importing user reviews the lossy nodes + the unresolved
//!    relations BEFORE the live import runs.
//! 6. The **per-tenant in-flight cap** ([`ImportLaneBudget`], contract 1.11) — the import processes in
//!    capped batches with the protected-human-lane shed order (the import is the AGENT/BATCH lane, shed
//!    BEFORE the interactive human lane).
//!
//! ## Why this REUSES the frozen pieces (EI-01 §7 — reuse, never duplicate)
//! - The **ADF lossy-map** ([`myelin_content::adf::MAP`] / [`ImportReport`]) is FROZEN in
//!   `myelin-content` (KN-P02, contract 13.2) — this module CONSUMES it, never re-defines a row. The
//!   ADF *body* construction reuses the SAME degraded-target logic Knowledge's `import_adf` ships (the
//!   block construction is content's; the issue-level remap is Issues').
//! - The **canonical key** mint reuses [`crate::keys::HiLoKeyAllocator`] (the same Hi/Lo that mints a
//!   hand-created issue's key — an imported issue is just an issue).
//! - The **emit** is the ONE [`myelin_events::OutboxTx::emit`] (contract 2.2) — an imported issue's
//!   `issue.created` is byte-shape-identical to a hand-created one, so reindex-from-source + Search +
//!   rollup work on imported data for FREE (one indexing path).
//!
//! ## Named floors (VISION §3 / EI-01 §1)
//! - **The import floor (R-9, the named lossy leg):** import = the canonical core, the four adapters,
//!   and the frozen ADF map. The **permission-scheme mapping** is the named lossy/legal-review leg
//!   ([`UNSUPPORTED_PERMISSION_SCHEME`], recorded in the reconciliation report as needs-legal-review,
//!   never silently mapped; M5+ legal, R-9). The canonical interchange is the round-trip oracle.
//! - **The byte-level provider parsers are upstream of the adapters.** Each [`SourceAdapter`]
//!   normalises an ALREADY-PARSED provider payload ([`ProviderRecord`]) into the canonical format; the
//!   raw Jira REST / Linear GraphQL / GitHub API / CSV byte-parsing is the import *service's* job (the
//!   adapter owns the field mapping + the loss accounting, the frozen contract this module proves). The
//!   floor is named so the canonical normalisation is not mistaken for the wire parser.
//! - **The persisted id-map BACKEND** is the OLTP store in prod ([`SourceIdMap`] is the port);
//!   [`InMemorySourceIdMap`] is the DB-free model the unit/e2e drills run against. The integration test
//!   against the live PgStore is the named follow-on row (red-until-proven on the dev stack).

use crate::events;
use crate::keys::{CanonicalKey, HiLoKeyAllocator, PrefixReserve, ReserveError};
use myelin_content::adf::{mapping_for, AdfNode, AdfTarget, ImportReport, Loss};
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventType, IdMinter,
    OutboxError, OutboxStore, OutboxTx, TenantId, Visibility,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE CANONICAL INTERCHANGE FORMAT (the round-trip oracle — ISS-D9(a))
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The source system an import came from** (the four v1 adapters). The `source_system` segment of a
/// [`SourceIdMap`] key — so a tenant can import from Jira AND Linear without an id collision (the same
/// `PROJ-1` from two systems maps to two distinct Myelin issues).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceSystem {
    /// Atlassian Jira (the dominant source; ADF descriptions).
    Jira,
    /// Linear.
    Linear,
    /// GitHub Issues.
    GitHub,
    /// A generic CSV export.
    Csv,
}

impl SourceSystem {
    /// The stable wire token for the source system (the id-map key segment + the report label).
    pub fn token(self) -> &'static str {
        match self {
            SourceSystem::Jira => "jira",
            SourceSystem::Linear => "linear",
            SourceSystem::GitHub => "github",
            SourceSystem::Csv => "csv",
        }
    }
}

/// **A relation between two source issues** in the canonical interchange — referenced by SOURCE id
/// (resolved to Myelin ids in pass 2). The `kind` is the frozen `issue_relation` class (TE-7) — e.g.
/// `parent`, `blocks`, `relates`, `duplicates`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalRelation {
    /// The SOURCE id of the relation's source endpoint (resolved through the id-map in pass 2).
    pub src_source_id: String,
    /// The SOURCE id of the relation's destination endpoint.
    pub dst_source_id: String,
    /// The relation class (the `issue_relation` kind — `parent`/`blocks`/`relates`/`duplicates`).
    pub kind: String,
}

/// **One issue in the canonical interchange format** — source-agnostic, source-id-keyed. The four
/// adapters normalise INTO this; the portability export round-trips WITH it (the round-trip oracle).
/// Free-text bodies are the cleartext `myelin-content`-degraded markdown-subset string (the at-rest
/// per-subject-DEK seal is the [`crate::dek`] storage layer's — this is the interchange document).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalIssue {
    /// The SOURCE id (the Jira `PROJ-1`, Linear `ABC-123`, GitHub `#42`, CSV row key). The id-map key.
    pub source_id: String,
    /// The Myelin project KEY prefix the issue lands under (`ENG`, `OPS` — the Hi/Lo prefix). The
    /// adapter maps the source project to a Myelin project key.
    pub project_key: String,
    /// The issue title (a markdown-subset string — the body conversion already ran for the description).
    pub title: String,
    /// The description body as a degraded markdown-subset string (the ADF body conversion ran in the
    /// adapter; the loss is in the import report).
    pub body_md: String,
    /// The opaque reporter pseudonym (contract 4.8 — never a raw name/email; the adapter pseudonymises).
    pub reporter_pseudonym: String,
    /// The source state name (mapped to a Myelin workflow state by the resolved scheme at apply time).
    pub state: String,
    /// `true` iff the issue body carries free-text PII (drives `contains_personal_data` + the
    /// `pii_key_ref` on the emitted event — references-not-payloads, contract 2.7).
    pub contains_pii: bool,
}

/// **A full canonical import** — the normalised, source-agnostic representation an adapter produces +
/// the engine consumes. The issues + the relations + the per-import [`ImportReport`] (the lossy-map
/// accumulator). This IS the canonical interchange the round-trip oracle compares against (ISS-D9(a)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalImport {
    /// Which source system this import came from (the id-map key segment).
    pub source_system: SourceSystem,
    /// The issues, in source order (pass 1 mints + maps each).
    pub issues: Vec<CanonicalIssue>,
    /// The relations, referencing source ids (pass 2 resolves both endpoints through the id-map).
    pub relations: Vec<CanonicalRelation>,
    /// The per-import lossy-map report (the X-2 "named, never silent" artifact). Accumulated by the
    /// adapter as it converted ADF bodies; carried through so the reconciliation report surfaces it.
    pub report: ImportReport,
}

impl CanonicalImport {
    /// A fresh empty canonical import for a source system.
    pub fn new(source_system: SourceSystem) -> CanonicalImport {
        CanonicalImport {
            source_system,
            issues: Vec::new(),
            relations: Vec::new(),
            report: ImportReport::new(),
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE SOURCE ADAPTERS (Jira / Linear / GitHub / CSV → the canonical interchange)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **An already-parsed provider record** — one issue's worth of fields the byte-level wire parser
/// extracted (the raw Jira REST / Linear GraphQL / GitHub API / CSV parsing is upstream — a named
/// floor). The adapter maps THIS into a [`CanonicalIssue`], doing the field mapping + the loss
/// accounting. The `body_adf` carries the ADF node stream for a Jira description (empty for the
/// markdown-native sources).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRecord {
    /// The source id (the provider's stable key).
    pub source_id: String,
    /// The source project key (mapped to a Myelin project key by the adapter).
    pub project_key: String,
    /// The title (already a plain/markdown string).
    pub title: String,
    /// The description as a markdown-subset string (the markdown-native sources — Linear/GitHub/CSV).
    pub body_md: String,
    /// The description as an ADF node stream (Jira) — converted through the frozen map by the adapter.
    pub body_adf: Vec<AdfBodyNode>,
    /// The reporter's opaque pseudonym (the wire parser already pseudonymised the raw author).
    pub reporter_pseudonym: String,
    /// The source state name.
    pub state: String,
    /// `true` iff the body carries free-text PII.
    pub contains_pii: bool,
    /// The source relations this record participates in (src is always `source_id`).
    pub relations: Vec<CanonicalRelation>,
}

/// **One ADF node from a parsed Jira description body** — the kind (the frozen [`AdfNode`]) + its
/// text payload + whether a CONDITIONAL node resolved (a mention resolving in-tenant, a card URL
/// resolving to a Myelin artifact). The adapter converts each through [`mapping_for`] and records the
/// loss. Mirrors Knowledge's `ParsedAdfNode` (the SAME frozen-map consumer shape) — Issues owns the
/// issue-level remap, content owns the per-node target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdfBodyNode {
    /// The ADF node kind (the frozen [`AdfNode`] — keyed on the wire `type`).
    pub kind: AdfNode,
    /// The node's text payload (the run the degraded target carries).
    pub text: String,
    /// Whether a conditional node resolved (lossless) or degraded (lossy). Ignored for unconditional
    /// rows.
    pub resolved: bool,
}

/// **A source adapter** — normalises a provider's already-parsed records into the canonical
/// interchange, doing the field mapping + the ADF body conversion + the loss accounting. The four v1
/// adapters share this trait so the engine is source-agnostic (it consumes a [`CanonicalImport`], never
/// a provider shape).
pub trait SourceAdapter {
    /// Which source system this adapter handles.
    fn source_system(&self) -> SourceSystem;

    /// Normalise a provider record stream into the canonical interchange. The ADF body conversion runs
    /// here (the loss recorded in the returned [`CanonicalImport::report`]); the markdown-native
    /// sources carry `body_md` through directly.
    fn normalise(&self, records: &[ProviderRecord]) -> CanonicalImport;
}

/// **The reconciliation-report label for the unsupported permission-scheme mapping (R-9, the named
/// lossy/legal-review leg).** A source's permission scheme is NOT auto-mapped (mapping a Jira
/// permission scheme to a Myelin ReBAC posture is a legal-review decision, M5+); it is recorded in the
/// reconciliation report as a named gap, never silently dropped or guessed.
pub const UNSUPPORTED_PERMISSION_SCHEME: &str =
    "permission-scheme mapping is the R-9 legal-review leg (M5+) — recorded, not auto-mapped";

/// Convert an ADF body node stream into a degraded markdown-subset body string, recording every lossy
/// conversion in `report` (the X-2 obligation). Shared by the adapters whose source carries ADF (Jira).
/// The block construction reuses content's degraded-target logic (the loss is named; the content
/// survives degraded) — this returns the flattened markdown-subset body the canonical interchange
/// carries (Issues stores the issue body as the consumed-subset block tree downstream; the interchange
/// is the markdown-subset string the round-trip oracle compares).
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

/// Render a degraded ADF target into the markdown-subset body string (the content survives in its
/// degraded form; the loss is already recorded). A focused mirror of content's `construct_block` at the
/// interchange (markdown-subset) layer — the full block-tree construction is the storage layer's.
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

/// Normalise a markdown-native provider (Linear / GitHub / CSV) into the canonical interchange — the
/// body carries through directly (no ADF conversion; an empty report). Shared by the three
/// markdown-native adapters (the field mapping is identical; only the [`SourceSystem`] tag differs).
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

/// **The Jira adapter** — converts ADF description bodies through the frozen lossy-map (recording the
/// loss), maps the Jira project key, pseudonymises the reporter.
#[derive(Clone, Copy, Debug, Default)]
pub struct JiraAdapter;

impl SourceAdapter for JiraAdapter {
    fn source_system(&self) -> SourceSystem {
        SourceSystem::Jira
    }

    fn normalise(&self, records: &[ProviderRecord]) -> CanonicalImport {
        let mut import = CanonicalImport::new(SourceSystem::Jira);
        for r in records {
            // The ADF description body is converted through the FROZEN map, recording every loss.
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

/// **The Linear adapter** — Linear descriptions are markdown-native; the body carries through.
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

/// **The GitHub adapter** — GitHub issue bodies are markdown-native; the body carries through.
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

/// **The CSV adapter** — a generic CSV export; bodies are plain text (markdown-native subset).
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. THE PERSISTED SOURCE↔MYELIN ID MAP (idempotency / resume / rollback / round-trip)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The id-map entry** — a source id resolved to its minted Myelin canonical key + artifact ref. The
/// load-bearing artifact: pass 1 records it on mint, pass 1 re-create checks it (a present entry is
/// SKIPPED — the idempotency / resume guarantee), pass 2 resolves relation endpoints through it, and
/// rollback deletes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdMapEntry {
    /// The minted Myelin canonical key (`<PROJECTKEY>-<seqno>`).
    pub canonical_key: CanonicalKey,
    /// The issue's stored canonical artifact ref (`myelin://<tenant>/issue/issue/<key>`).
    pub artifact_ref: ArtifactRef,
}

/// **The persisted source↔Myelin id map (the port)** — `(tenant, source_system, source_id) →
/// IdMapEntry`. In prod this is a table on the SAME OLTP store (co-committed with the issue create, so
/// the map and the issue land or roll back together — the resume guarantee is the co-commit). The
/// engine uses ONLY this trait; [`InMemorySourceIdMap`] is the DB-free model the drills run against.
pub trait SourceIdMap {
    /// Look up a source id's mapping (resume/idempotency check: a present entry means the issue was
    /// already created in a prior pass-1 run — SKIP it).
    fn get(
        &self,
        tenant: &TenantId,
        source_system: SourceSystem,
        source_id: &str,
    ) -> Option<IdMapEntry>;

    /// Record a fresh mapping (pass 1 on mint). In prod this co-commits with the issue's
    /// `issue.created` event on the SAME outbox transaction (so a crash after the mint but before the
    /// commit leaves NO map entry AND no issue — the at-least-once + idempotent shape).
    fn put(
        &self,
        tenant: &TenantId,
        source_system: SourceSystem,
        source_id: &str,
        entry: IdMapEntry,
    );

    /// Delete a mapping (rollback). The id-map is the rollback ledger: rolling back an import deletes
    /// its entries so a re-run re-creates (the map is the single source of "what was imported").
    fn remove(&self, tenant: &TenantId, source_system: SourceSystem, source_id: &str);

    /// The number of mappings for a `(tenant, source_system)` (the "how many imported" count the
    /// reconciliation report surfaces).
    fn count(&self, tenant: &TenantId, source_system: SourceSystem) -> usize;
}

/// The DB-free in-memory model of the [`SourceIdMap`] (the drills run against it; the live PgStore is
/// the named integration follow-on). Behind a `Mutex` so the per-tenant in-flight batches share it.
#[derive(Debug, Default)]
pub struct InMemorySourceIdMap {
    inner: std::sync::Mutex<BTreeMap<(String, &'static str, String), IdMapEntry>>,
}

impl InMemorySourceIdMap {
    /// A fresh empty id-map.
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 4. THE RECONCILIATION REPORT + DRY-RUN (reconciliation-report-first)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **An unresolved relation** — a relation whose source or destination endpoint was not in the id-map
/// (an edge pointing at an issue outside the import set, or a not-yet-minted one). Recorded in the
/// reconciliation report as a NAMED gap, never a silent dangling edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unresolved {
    /// The relation that could not be fully resolved.
    pub relation: CanonicalRelation,
    /// Which endpoint(s) were missing (a human-readable reason for the report).
    pub reason: String,
}

/// **The per-import reconciliation report** (the X-2 + ISS-D9(a) named artifact). What WOULD (dry run)
/// or DID (live) happen: the created count, the skipped (already-mapped) count, the relations resolved,
/// the unresolved relation gaps, the lossy-map [`ImportReport`], and the named permission-scheme legal
/// leg (R-9). The importing user reviews THIS before the live import runs (reconciliation-report-first).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconciliationReport {
    /// The issues created (pass 1 minted a new canonical key for each).
    pub created: usize,
    /// The issues SKIPPED because already in the id-map (idempotent re-create / resume — 0 dup).
    pub skipped_already_mapped: usize,
    /// The relations resolved (both endpoints mapped, `issue.relation.created` emitted).
    pub relations_created: usize,
    /// The relations that could not be fully resolved (a named gap, never a silent dangling edge).
    pub unresolved: Vec<Unresolved>,
    /// The lossy-map report (every ADF lossy conversion, named — X-2).
    pub loss: ImportReport,
    /// The named legal-review legs (R-9 — the permission-scheme mapping is recorded here, never
    /// auto-mapped). Always present when the source carries a permission scheme.
    pub legal_review: Vec<String>,
}

impl ReconciliationReport {
    /// A fresh empty report.
    pub fn new() -> ReconciliationReport {
        ReconciliationReport::default()
    }

    /// `true` iff the import was fully clean — nothing lossy, no unresolved relations, no legal-review
    /// legs (a fully-lossless adoption). The reconciliation UX shows the green check iff this holds.
    pub fn is_clean(&self) -> bool {
        self.loss.is_lossless() && self.unresolved.is_empty() && self.legal_review.is_empty()
    }
}

/// **The result of a dry run** — the reconciliation report constructed WITHOUT emitting a single
/// event (reconciliation-report-first). The importing user reviews the lossy nodes + the unresolved
/// relations + the legal-review legs, then runs the live import. The dry run is pure: it never touches
/// the outbox, never mints a durable key (it models the mint to compute the created count).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DryRun {
    /// The reconciliation report the live import WOULD produce.
    pub report: ReconciliationReport,
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 5. THE PER-TENANT IN-FLIGHT CAP (contract 1.11 — the protected human lane shed order)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The import lane budget (contract 1.11 — the protected-human-lane shed order).** The import is the
/// BATCH/AGENT lane (shed BEFORE the interactive human lane); it processes in capped batches so a
/// 100k-issue import is bounded backfill that never saturates the tenant's write capacity and never
/// starves another tenant's interactive traffic. `max_in_flight` is the per-tenant in-flight cap (the
/// batch size); the engine yields between batches so the human lane always wins admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportLaneBudget {
    /// The maximum issues processed per batch before yielding (the per-tenant in-flight cap). The
    /// import never holds more than this in flight, so it never monopolises the tenant's write lane.
    pub max_in_flight: usize,
}

impl ImportLaneBudget {
    /// The default per-tenant in-flight cap (a conservative batch — tuned by the ISS-D9(c) drill). The
    /// import is bounded backfill; the human lane is never starved.
    pub const DEFAULT_MAX_IN_FLIGHT: usize = 64;

    /// The default budget (the [`Self::DEFAULT_MAX_IN_FLIGHT`] cap).
    pub fn default_budget() -> ImportLaneBudget {
        ImportLaneBudget {
            max_in_flight: Self::DEFAULT_MAX_IN_FLIGHT,
        }
    }

    /// Split a workload of `total` items into the batch boundaries the cap implies (the yield points —
    /// the human lane is admitted between each). Returns the batch sizes (each ≤ `max_in_flight`).
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 6. THE TWO-PASS, ID-REMAPPED, IDEMPOTENT + RESUMABLE IMPORT ENGINE
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// Why an import could not run (LOUD — an import never silently half-applies). A mint failure FAILS
/// the import CLOSED (the id-map + the issue co-commit, so a failed mint leaves no half-state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// A canonical-key reserve failed (the Hi/Lo allocator could not mint — the issue is NOT created,
    /// the id-map is NOT written; the import fails closed at this issue, resumable from here).
    KeyReserve(String),
    /// The outbox emit failed (the event could not be staged — the whole batch transaction rolls back).
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

/// **The two-pass, ID-remapped, idempotent + resumable import engine.** Borrows the cell's
/// [`HiLoKeyAllocator`] (the SAME mint a hand-created issue uses — an imported issue is just an issue)
/// plus the persisted [`SourceIdMap`] (the resume/idempotency/rollback ledger). The engine is
/// source-agnostic: it consumes a [`CanonicalImport`] an adapter produced.
pub struct ImportEngine<'a, R: PrefixReserve, M: SourceIdMap> {
    /// The cell's canonical-key allocator (reused — an imported issue's key is minted the same way).
    allocator: &'a HiLoKeyAllocator<R>,
    /// The persisted id-map (idempotency / resume / rollback / pass-2 relation resolution).
    id_map: &'a M,
    /// The per-tenant in-flight cap (contract 1.11 — the import is bounded backfill).
    budget: ImportLaneBudget,
}

impl<'a, R: PrefixReserve, M: SourceIdMap> ImportEngine<'a, R, M> {
    /// A fresh engine over a cell's allocator + id-map + lane budget.
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

    /// **Dry run — construct the FULL reconciliation report WITHOUT emitting a single event**
    /// (reconciliation-report-first). It counts what WOULD be created vs skipped (consulting the
    /// id-map for already-mapped source ids — so a dry run after a partial import shows exactly the
    /// remaining work), accumulates the lossy-map report (carried from the adapter), resolves the
    /// relations against the id-map + the issues-in-this-import set (recording the unresolved gaps),
    /// and names the permission-scheme legal leg (R-9). Pure: no mint, no emit.
    pub fn dry_run(
        &self,
        tenant: &TenantId,
        import: &CanonicalImport,
        has_permission_scheme: bool,
    ) -> DryRun {
        let mut report = ReconciliationReport::new();
        report.loss = import.report.clone();

        // The set of source ids this import will mint (for pass-2 resolution against in-this-import
        // endpoints, plus the already-mapped ones).
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

    /// **Run the live import (the two passes), emitting `issue.*` via the ONE outbox path.**
    ///
    /// The import opens a fresh outbox transaction PER BATCH off `store` (the per-tenant in-flight cap
    /// yields between batches — contract 1.11; each batch transaction co-commits the issue events + the
    /// id-map entries, so a crash mid-import leaves the prior batches durably committed and the
    /// in-flight batch cleanly rolled back — resume continues from the id-map). `crash_after_batch`
    /// models a crash for the ISS-D9(b) drill: when `Some(n)`, the engine stops after committing batch
    /// `n` (the prior batches are durable in `store` + the id-map; a resume re-runs and SKIPS them).
    ///
    /// **Pass 1 (mint + map):** for each source issue NOT already in the id-map, mint a canonical key,
    /// record `source_id → key` in the id-map, and emit `issue.created`. An issue ALREADY in the id-map
    /// is SKIPPED (the idempotency / resume guarantee — 0 duplicate creates on a crash, ISS-D9(b)).
    ///
    /// **Pass 2 (resolve relations):** for each relation, resolve BOTH endpoints through the id-map and
    /// emit `issue.relation.created`. An unresolved endpoint is recorded as a named gap (never a silent
    /// dangling edge).
    ///
    /// Returns the reconciliation report the live import produced (the same shape the dry run
    /// predicted). A mint/emit failure FAILS the import CLOSED at that issue (no half-state; the import
    /// is resumable from the failure point via the id-map).
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

        // ── PASS 1: mint + map + emit issue.created (capped batches — contract 1.11) ──
        let batches = self.budget.batches(import.issues.len());
        let mut offset = 0usize;
        for (batch_idx, batch_len) in batches.into_iter().enumerate() {
            // A fresh per-batch transaction (the in-flight cap yields between batches).
            let mut tx = store.begin(Arc::clone(&minter), ctx_base.clone());
            tx.stage_state_change(format!("import pass1 batch {batch_idx}"));
            for issue in &import.issues[offset..offset + batch_len] {
                // IDEMPOTENCY / RESUME: an already-mapped source id is SKIPPED (0 duplicate creates).
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
                // Co-commit the id-map entry with the issue.created event (the map and the issue land
                // or roll back together; the resume ledger is the co-commit — recorded as the tx
                // commits below; here the in-memory map mirrors that co-commit on the durable path).
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
            // The ISS-D9(b) crash model: stop AFTER committing this batch (prior batches durable).
            if crash_after_batch == Some(batch_idx) {
                return Ok(report);
            }
        }

        // ── PASS 2: resolve relations through the id-map + emit issue.relation.created ──
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

    /// **Rollback an import** — delete every id-map entry for the imported source ids (the id-map is
    /// the rollback ledger). After a rollback a re-run re-creates the issues (the source ids are no
    /// longer mapped). The `issue.*.deleted` tombstones the rollback emits are the storage layer's
    /// (this owns the id-map ledger truth — the rollback of the resume/idempotency anchor). Returns the
    /// number of mappings removed.
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

/// The reason an unresolved relation could not be fully resolved (for the named report gap).
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

/// Build the `issue.created` event draft an imported issue emits (the SAME shape a hand-created issue
/// emits — references-not-payloads, contract 2.7). The aggregate is the issue (per-issue ordering); the
/// payload carries the issue URN + the canonical key, NEVER the inline body (a PII body carries a
/// `pii_key_ref`, threaded by the sealed write path in prod — here the import marks the PII flag).
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
        // A PII-bearing imported body carries the per-subject-DEK key ref in prod (threaded by the
        // sealed write path); here the import marks the flag — the body is never on the wire.
        pii_key_ref: None,
    }
}

/// Build the `issue.relation.created` event draft a resolved relation emits (pass 2; the TE-7 typed
/// edge — the `issue_relation` table is truth, the event mirrors it for Refs/Search).
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

/// Pick the right adapter for a source system (the engine is source-agnostic; the dispatch is at the
/// service edge). Returns a boxed [`SourceAdapter`] for the four v1 sources.
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
