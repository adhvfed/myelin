//! # The Export/Import service — Art. 20 lossless JSON + multi-format + ADF import (KN-P24 / P-314, M3)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/06-reconciliation-compliance.md`
//! §1 (13.2 "The Import service applies the frozen conversion table … every lossy conversion recorded
//! in a per-import Knowledge doc — named, not silent") + §8 (the Art. 20 portability mechanism the
//! GDPR holder reuses) and `04-views-cli-and-api.md` (the export affordances: `myelin kb export
//! --format json|md|html|pdf|csv`, `myelin kb import --from adf`).
//!
//! **Contract-index:** row **10.1** (`export(subject)` — the Art. 20 portable bundle: the lossless
//! JSON mechanism the GDPR `PersonalDataHolder::export` in **KN-P25** reuses — OWNED here as the
//! Export service), row **13.2** (the ADF → `myelin-content` lossy-map — CONSUMED: the import builds
//! against the frozen [`myelin_content::MAP`] + records each lossy node in the
//! [`myelin_content::ImportReport`]), row **13.1** (the content model the export round-trips —
//! `render(parse(md)) === md` holds across export/import).
//!
//! ## What this module ships (KN-P24's owned work)
//! 1. [`ExportDoc`] — a **self-contained, content-bearing document model**: the page metadata + the
//!    full block-taxonomy content tree ([`myelin_content::Block`] payloads with stable [`BlockId`]s
//!    and nested children). The block TREE (`myelin_knowledge::block_tree`) carries only structure
//!    (`parent_id`/`order_key`/`block_type`); the EXPORT model additionally carries the actual
//!    [`Block`] payload so the bundle is lossless and self-describing. This is the **Art. 20 lossless
//!    JSON** surface — it round-trips byte-faithfully through serde ([`ExportDoc::to_json_bundle`] /
//!    [`ExportDoc::from_json_bundle`]).
//! 2. The **lossless JSON export/import round-trip gate** ([`ExportDoc::json_roundtrips`]): a doc
//!    exported to JSON and re-imported is structurally identical AND `serialize(parse(md)) == md`
//!    holds for every inline payload (the KN-D2 frozen correctness bar, end-to-end across the
//!    export/import boundary).
//! 3. The **multi-format exporters**: [`ExportDoc::to_markdown`] (the markdown-subset string, §8.3),
//!    [`ExportDoc::to_html`] (a semantic HTML render), [`ExportDoc::to_pdf`] (a minimal valid PDF
//!    document — a self-contained byte stream, no external typesetter), and [`export_rows_to_csv`]
//!    (the flexible-database `db_row` CSV export over the frozen [`FieldSchema`]).
//! 4. The **ADF lossy-map import** ([`import_adf`]): consumes the frozen [`myelin_content::MAP`],
//!    constructs the named target [`Block`]s, and records every lossy conversion in the
//!    [`ImportReport`] (the X-2 "named, never silent" obligation).
//!
//! ## FLOOR named (EI-01 §1) — none for the lossless JSON
//! The lossless JSON export is the FULL Art. 20 mechanism (no floor). The ADF import's lossy nodes
//! are **lossy by source-format limit** (Jira macros are not executed; a multi-column layout has no
//! Myelin equivalent) — these are named in the [`ImportReport`], NOT a Myelin floor. The PDF
//! exporter ships a self-contained minimal-but-valid PDF (text-flow, no embedded fonts / no media
//! rasterisation): a richer typeset PDF (embedded fonts, image rasterisation, pagination control)
//! is a presentation-layer follow-on — the lossless surfaces (JSON + Markdown + HTML) carry the full
//! fidelity, the PDF is a print rendition. Named here in writing.
//!
//! ## DEVIATION FROM THE PROMPT'S CRATE LOCATION (EI-01 §1 — code wins, write it down)
//! The prompt names the deliverable "In crate myelin-knowledge". The frozen ADF map + `ImportReport`
//! already live in `myelin-content` (KN-P02 / P-235); this module CONSUMES them in place (it does NOT
//! re-define the map or the report — EI-01 §7 one primitive). The new Export/Import SERVICE lives
//! here in `myelin-knowledge` exactly as the prompt specifies.

use std::fmt::Write as _;

use myelin_content::adf::mapping_for;
use myelin_content::ImportReport;
use myelin_content::{
    parse_inline, serialize_inline, AdfNode, AdfTarget, Block, CalloutTone, Cell, Column,
    EmbedDisplay, HeadingLevel, Inline, InlineNode, ListItem, Loss, TaskItem,
};
use myelin_query::{FieldId, FieldValue};
use serde::{Deserialize, Serialize};

use crate::block_tree::{BlockId, PageId};
use crate::database::{DbRow, FieldSchema};

/// **The export format** (the `--format` affordance, `04-views-cli-and-api.md`). `Json` is the
/// lossless Art. 20 surface; the rest are render targets (Markdown/HTML lossless-for-content,
/// PDF a print rendition).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// The lossless JSON bundle (Art. 20 portability — the GDPR holder reuses this).
    Json,
    /// The markdown-subset string render (§8.3 — the string survives copy/paste/export).
    Markdown,
    /// A semantic HTML render.
    Html,
    /// A minimal valid PDF document (a self-contained byte stream).
    Pdf,
    /// CSV — the flexible-database `db_row` tabular export (over a [`FieldSchema`]).
    Csv,
}

impl ExportFormat {
    /// The canonical file extension for this format (the export affordance names the file).
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Json => "json",
            ExportFormat::Markdown => "md",
            ExportFormat::Html => "html",
            ExportFormat::Pdf => "pdf",
            ExportFormat::Csv => "csv",
        }
    }
}

/// **One node in the export content tree** — a stable [`BlockId`], its full [`Block`] payload, and
/// its (ordered) children. The block TREE in [`crate::block_tree`] is an adjacency list (rows keyed
/// by id with a `parent_id`/`order_key`); the EXPORT tree is the materialised, ordered nesting with
/// the actual content payload inlined, so the bundle is self-contained and lossless.
///
/// The `block` is the frozen [`myelin_content::Block`] — the load-bearing fidelity. `children` is
/// the ordered child list (the live `order_key` ordering already applied at export time), so the
/// bundle carries the *result* of the ordering, not the raw `order_key`s (which are an internal
/// fractional index, not portable data).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportBlock {
    /// The stable opaque block id (the `b<id>`/`h<id>` `#sub` target, 5.7 — carried so a re-import
    /// preserves ref identity).
    pub id: BlockId,
    /// The frozen block-taxonomy payload (the actual content — the lossless fidelity).
    pub block: Block,
    /// The ordered children (already in `order_key` order at export time).
    pub children: Vec<ExportBlock>,
}

impl ExportBlock {
    /// A leaf export block (no children).
    pub fn leaf(id: impl Into<String>, block: Block) -> ExportBlock {
        ExportBlock {
            id: BlockId(id.into()),
            block,
            children: Vec::new(),
        }
    }

    /// An export block with children.
    pub fn with_children(
        id: impl Into<String>,
        block: Block,
        children: Vec<ExportBlock>,
    ) -> ExportBlock {
        ExportBlock {
            id: BlockId(id.into()),
            block,
            children,
        }
    }

    /// Every block in this subtree, depth-first (the export-order walk).
    fn walk<'a>(&'a self, out: &mut Vec<&'a ExportBlock>) {
        out.push(self);
        for c in &self.children {
            c.walk(out);
        }
    }
}

/// **The self-contained export document** — the page metadata + the content tree. THIS is the
/// **Art. 20 lossless JSON** unit: [`ExportDoc::to_json_bundle`] serialises it; [`ExportDoc::
/// from_json_bundle`] re-imports it byte-faithfully. The GDPR `PersonalDataHolder::export` (KN-P25)
/// assembles one of these per subject and serialises the bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportDoc {
    /// The stable opaque page id (the `knowledge/page/<page_id>` `#sub` root).
    pub page_id: PageId,
    /// The page title (a plain string — the page's display name).
    pub title: String,
    /// The page's parent page (the `page_parent` folder nesting), or `None` for a root page.
    pub parent_page: Option<PageId>,
    /// The ordered top-level blocks (each carrying its content subtree).
    pub blocks: Vec<ExportBlock>,
    /// A schema-version tag so a future bundle format is self-describing (Art. 20 portability is a
    /// long-lived format; the version lets an importer detect a shape it predates).
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
}

/// The current export-bundle schema version (bumped only on a whole-workspace contract PR).
pub const EXPORT_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    EXPORT_SCHEMA_VERSION
}

impl ExportDoc {
    /// Construct an export document from page metadata + the materialised content tree.
    pub fn new(
        page_id: impl Into<String>,
        title: impl Into<String>,
        parent_page: Option<PageId>,
        blocks: Vec<ExportBlock>,
    ) -> ExportDoc {
        ExportDoc {
            page_id: PageId(page_id.into()),
            title: title.into(),
            parent_page,
            blocks,
            schema_version: EXPORT_SCHEMA_VERSION,
        }
    }

    /// Every block in the document, depth-first (the export-order walk over all subtrees).
    pub fn all_blocks(&self) -> Vec<&ExportBlock> {
        let mut out = Vec::new();
        for b in &self.blocks {
            b.walk(&mut out);
        }
        out
    }

    // ── the Art. 20 lossless JSON surface (10.1) ───────────────────────────────────────────────

    /// **Serialise to the lossless JSON bundle (Art. 20).** This is the portable artifact the GDPR
    /// holder (KN-P25) emits per subject. Pretty-printed so an exported bundle is human-readable
    /// (the data subject receives it).
    pub fn to_json_bundle(&self) -> String {
        serde_json::to_string_pretty(self).expect("ExportDoc serialises (a closed serde shape)")
    }

    /// **Re-import a lossless JSON bundle.** The inverse of [`ExportDoc::to_json_bundle`] — a
    /// byte-faithful round-trip (the JSON is the canonical lossless form). A malformed bundle is a
    /// LOUD, typed error (EI-01 §5; never a silent partial parse).
    pub fn from_json_bundle(json: &str) -> Result<ExportDoc, ExportError> {
        serde_json::from_str(json).map_err(|e| ExportError::MalformedBundle(e.to_string()))
    }

    /// **THE EXPORT/IMPORT ROUND-TRIP GATE (10.1 + 13.1).** Export this doc to JSON, re-import it,
    /// and assert: (a) the re-imported doc is STRUCTURALLY identical (the lossless property), and
    /// (b) `serialize_inline(parse_inline(md)) == md` holds for every inline payload across the
    /// boundary (the KN-D2 frozen correctness bar). Returns `true` iff both hold — the green
    /// artifact the prompt's round-trip gate names.
    pub fn json_roundtrips(&self) -> bool {
        let json = self.to_json_bundle();
        let back = match ExportDoc::from_json_bundle(&json) {
            Ok(d) => d,
            Err(_) => return false,
        };
        // (a) structural identity — the lossless property.
        if &back != self {
            return false;
        }
        // (b) the content render path round-trips end-to-end (every inline is a fixed point).
        self.all_inlines_roundtrip() && back.all_inlines_roundtrip()
    }

    /// Whether `serialize_inline(parse_inline(serialize_inline(inline))) == serialize_inline(inline)`
    /// for every inline payload in the document (the content-model round-trip, 13.1). Re-serialising
    /// the stored inline, re-parsing it, and re-serialising must be a fixed point — the KN-D2 bar
    /// applied to every block's content.
    pub fn all_inlines_roundtrip(&self) -> bool {
        self.all_blocks()
            .iter()
            .all(|b| inline_roundtrips(&b.block))
    }

    // ── the multi-format exporters ─────────────────────────────────────────────────────────────

    /// **Export to the markdown-subset string** (§8.3 — the string survives copy/paste/export). The
    /// document's title is an `# H1`; each block renders to its canonical markdown, blocks joined by
    /// a blank line (the standard block separator).
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        if !self.title.is_empty() {
            let _ = writeln!(out, "# {}\n", self.title);
        }
        let mut first = true;
        for b in &self.blocks {
            if !first {
                out.push('\n');
            }
            first = false;
            block_to_markdown(b, 0, &mut out);
        }
        out
    }

    /// **Export to semantic HTML.** Each block renders to its HTML element; inline marks render to
    /// `<strong>`/`<em>`/`<code>`/`<del>`/`<a>`. The output is a complete `<!DOCTYPE html>` document
    /// (the page title in the `<title>` + an `<h1>`), so the export is self-contained.
    pub fn to_html(&self) -> String {
        let mut body = String::new();
        for b in &self.blocks {
            block_to_html(b, &mut body);
        }
        format!(
            "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n</head>\n<body>\n<h1>{}</h1>\n{}</body>\n</html>\n",
            html_escape(&self.title),
            html_escape(&self.title),
            body
        )
    }

    /// **Export to a minimal valid PDF document** (a self-contained byte stream; no external
    /// typesetter, no embedded fonts). The text is laid out as a single content stream over the
    /// standard Helvetica font with simple line flow — a print rendition. The richer typeset PDF is
    /// a named follow-on (the module doc); the lossless fidelity lives in JSON/Markdown/HTML.
    pub fn to_pdf(&self) -> Vec<u8> {
        // The plain-text projection (markdown, then strip the markdown delimiters to readable text).
        let text = self.to_plain_text();
        render_minimal_pdf(&self.title, &text)
    }

    /// The plain-text projection of the document (the title + every block's text content, one line
    /// per block) — the body the PDF lays out and a useful preview surface.
    pub fn to_plain_text(&self) -> String {
        let mut lines = Vec::new();
        if !self.title.is_empty() {
            lines.push(self.title.clone());
        }
        for b in &self.blocks {
            block_to_plain_text(b, &mut lines);
        }
        lines.join("\n")
    }
}

/// The error surface of the Export/Import service (LOUD + typed — EI-01 §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportError {
    /// A JSON bundle did not parse to a well-formed [`ExportDoc`].
    MalformedBundle(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::MalformedBundle(e) => write!(f, "malformed export bundle: {e}"),
        }
    }
}

impl std::error::Error for ExportError {}

// ───────────────────────────── the content round-trip (13.1) ──────────────────────────────────

/// Whether every inline payload of a block re-serialises to a fixed point
/// (`serialize(parse(serialize(inline))) == serialize(inline)`) — recursively over nested blocks'
/// content. A `code_block.text` is RAW (never markdown-parsed, §2.1) so it is excluded from the
/// inline check (its round-trip is verbatim byte-equality, which serde already guarantees).
fn inline_roundtrips(block: &Block) -> bool {
    let ok = |inline: &Inline| -> bool {
        let s = serialize_inline(inline);
        serialize_inline(&parse_inline(&s, inline.nodes.as_slice())) == s
    };
    match block {
        Block::Paragraph { inline } => ok(inline),
        Block::Heading { inline, .. } => ok(inline),
        Block::Toggle { summary, blocks } => ok(summary) && blocks.iter().all(inline_roundtrips),
        Block::BulletList { items } | Block::OrderedList { items, .. } => items
            .iter()
            .all(|it| it.blocks.iter().all(inline_roundtrips)),
        Block::TaskList { items } => items.iter().all(|it| ok(&it.inline)),
        Block::Blockquote { blocks } | Block::Callout { blocks, .. } => {
            blocks.iter().all(inline_roundtrips)
        }
        Block::Table { columns, rows } => {
            columns.iter().all(|c| ok(&c.header))
                && rows.iter().all(|r| {
                    r.iter()
                        .all(|cell| cell.blocks.iter().all(inline_roundtrips))
                })
        }
        Block::Image { caption, .. } => caption.as_ref().map(ok).unwrap_or(true),
        // No inline payload (code is raw; the structured/leaf nodes carry refs, not inline runs).
        Block::CodeBlock { .. }
        | Block::Divider
        | Block::Embed { .. }
        | Block::DbView { .. }
        | Block::SyncBlock { .. } => true,
    }
}

// ───────────────────────────── the markdown exporter ──────────────────────────────────────────

fn block_to_markdown(b: &ExportBlock, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    match &b.block {
        Block::Paragraph { inline } => {
            let _ = writeln!(out, "{indent}{}", serialize_inline(inline));
        }
        Block::Heading { level, inline } => {
            let hashes = "#".repeat(level.get() as usize);
            let _ = writeln!(out, "{indent}{hashes} {}", serialize_inline(inline));
        }
        Block::BulletList { items } => {
            for it in items {
                list_item_md(it, depth, "- ", out);
            }
        }
        Block::OrderedList { items, start } => {
            for (i, it) in items.iter().enumerate() {
                let marker = format!("{}. ", *start as usize + i);
                list_item_md(it, depth, &marker, out);
            }
        }
        Block::TaskList { items } => {
            for it in items {
                let mark = if it.checked { "x" } else { " " };
                let _ = writeln!(out, "{indent}- [{mark}] {}", serialize_inline(&it.inline));
            }
        }
        Block::Blockquote { blocks } => {
            for sub in blocks {
                let mut buf = String::new();
                block_to_markdown(&ExportBlock::leaf("", sub.clone()), 0, &mut buf);
                for line in buf.lines() {
                    let _ = writeln!(out, "{indent}> {line}");
                }
            }
        }
        Block::CodeBlock { lang, text } => {
            let lang = lang.as_deref().unwrap_or("");
            let _ = writeln!(out, "{indent}```{lang}");
            for line in text.lines() {
                let _ = writeln!(out, "{indent}{line}");
            }
            let _ = writeln!(out, "{indent}```");
        }
        Block::Callout { tone, blocks } => {
            let _ = writeln!(out, "{indent}> [!{}]", callout_tone_tag(*tone));
            for sub in blocks {
                let mut buf = String::new();
                block_to_markdown(&ExportBlock::leaf("", sub.clone()), 0, &mut buf);
                for line in buf.lines() {
                    let _ = writeln!(out, "{indent}> {line}");
                }
            }
        }
        Block::Table { columns, rows } => {
            let headers: Vec<String> = columns
                .iter()
                .map(|c| serialize_inline(&c.header))
                .collect();
            let _ = writeln!(out, "{indent}| {} |", headers.join(" | "));
            let _ = writeln!(
                out,
                "{indent}| {} |",
                vec!["---"; headers.len().max(1)].join(" | ")
            );
            for row in rows {
                let cells: Vec<String> = row.iter().map(cell_to_inline_md).collect();
                let _ = writeln!(out, "{indent}| {} |", cells.join(" | "));
            }
        }
        Block::Divider => {
            let _ = writeln!(out, "{indent}---");
        }
        Block::Image { alt, .. } => {
            let _ = writeln!(out, "{indent}![{alt}]()");
        }
        Block::Embed { reference, .. } => {
            let _ = writeln!(out, "{indent}[embed: {}]", reference.0);
        }
        Block::DbView { db, .. } => {
            let _ = writeln!(out, "{indent}[db_view: {}]", db.0);
        }
        Block::Toggle { summary, blocks } => {
            let _ = writeln!(
                out,
                "{indent}<details><summary>{}</summary>",
                serialize_inline(summary)
            );
            for sub in blocks {
                block_to_markdown(&ExportBlock::leaf("", sub.clone()), depth + 1, out);
            }
            let _ = writeln!(out, "{indent}</details>");
        }
        Block::SyncBlock { source } => {
            let _ = writeln!(out, "{indent}[sync_block: {}]", source.0);
        }
    }
    // The export tree's structural children (nested blocks the adjacency list carried separately).
    for child in &b.children {
        block_to_markdown(child, depth + 1, out);
    }
}

fn list_item_md(it: &ListItem, depth: usize, marker: &str, out: &mut String) {
    let indent = "  ".repeat(depth);
    // The first block of a list item shares the marker line; the rest nest under it.
    let mut blocks = it.blocks.iter();
    if let Some(first) = blocks.next() {
        let mut buf = String::new();
        block_to_markdown(&ExportBlock::leaf("", first.clone()), 0, &mut buf);
        let line = buf.lines().next().unwrap_or("");
        let _ = writeln!(out, "{indent}{marker}{line}");
        // any extra lines/blocks nest one level deeper
        for extra in buf.lines().skip(1) {
            let _ = writeln!(out, "{indent}  {extra}");
        }
    } else {
        let _ = writeln!(out, "{indent}{marker}");
    }
    for sub in blocks {
        block_to_markdown(&ExportBlock::leaf("", sub.clone()), depth + 1, out);
    }
}

fn cell_to_inline_md(cell: &Cell) -> String {
    // A table cell is a block sequence; for the markdown table we render the inline content of each
    // block joined by a space (markdown tables are single-line cells).
    cell.blocks
        .iter()
        .map(|b| match b {
            Block::Paragraph { inline } => serialize_inline(inline),
            Block::Heading { inline, .. } => serialize_inline(inline),
            other => {
                let mut buf = String::new();
                block_to_markdown(&ExportBlock::leaf("", other.clone()), 0, &mut buf);
                buf.trim().replace('\n', " ")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn callout_tone_tag(tone: CalloutTone) -> &'static str {
    match tone {
        CalloutTone::Info => "INFO",
        CalloutTone::Warn => "WARNING",
        CalloutTone::Success => "SUCCESS",
        CalloutTone::Danger => "DANGER",
        CalloutTone::Note => "NOTE",
    }
}

// ───────────────────────────── the HTML exporter ──────────────────────────────────────────────

fn block_to_html(b: &ExportBlock, out: &mut String) {
    match &b.block {
        Block::Paragraph { inline } => {
            let _ = writeln!(out, "<p>{}</p>", inline_to_html(inline));
        }
        Block::Heading { level, inline } => {
            let l = level.get();
            let _ = writeln!(out, "<h{l}>{}</h{l}>", inline_to_html(inline));
        }
        Block::BulletList { items } => {
            out.push_str("<ul>\n");
            for it in items {
                let _ = writeln!(out, "<li>{}</li>", item_blocks_html(&it.blocks));
            }
            out.push_str("</ul>\n");
        }
        Block::OrderedList { items, start } => {
            let _ = writeln!(out, "<ol start=\"{start}\">");
            for it in items {
                let _ = writeln!(out, "<li>{}</li>", item_blocks_html(&it.blocks));
            }
            out.push_str("</ol>\n");
        }
        Block::TaskList { items } => {
            out.push_str("<ul class=\"task-list\">\n");
            for it in items {
                let checked = if it.checked { " checked" } else { "" };
                let _ = writeln!(
                    out,
                    "<li><input type=\"checkbox\" disabled{checked}> {}</li>",
                    inline_to_html(&it.inline)
                );
            }
            out.push_str("</ul>\n");
        }
        Block::Blockquote { blocks } => {
            out.push_str("<blockquote>\n");
            for sub in blocks {
                block_to_html(&ExportBlock::leaf("", sub.clone()), out);
            }
            out.push_str("</blockquote>\n");
        }
        Block::CodeBlock { lang, text } => {
            let cls = lang
                .as_deref()
                .map(|l| format!(" class=\"language-{}\"", html_escape(l)))
                .unwrap_or_default();
            let _ = writeln!(out, "<pre><code{cls}>{}</code></pre>", html_escape(text));
        }
        Block::Callout { tone, blocks } => {
            let _ = writeln!(
                out,
                "<div class=\"callout callout-{}\">",
                callout_tone_class(*tone)
            );
            for sub in blocks {
                block_to_html(&ExportBlock::leaf("", sub.clone()), out);
            }
            out.push_str("</div>\n");
        }
        Block::Table { columns, rows } => {
            out.push_str("<table>\n<thead>\n<tr>");
            for c in columns {
                let _ = write!(out, "<th>{}</th>", inline_to_html(&c.header));
            }
            out.push_str("</tr>\n</thead>\n<tbody>\n");
            for row in rows {
                out.push_str("<tr>");
                for cell in row {
                    let _ = write!(out, "<td>{}</td>", item_blocks_html(&cell.blocks));
                }
                out.push_str("</tr>\n");
            }
            out.push_str("</tbody>\n</table>\n");
        }
        Block::Divider => out.push_str("<hr>\n"),
        Block::Image { alt, .. } => {
            let _ = writeln!(out, "<figure><img alt=\"{}\"></figure>", html_escape(alt));
        }
        Block::Embed { reference, display } => {
            let _ = writeln!(
                out,
                "<div class=\"embed embed-{}\" data-ref=\"{}\"></div>",
                embed_display_class(*display),
                html_escape(&reference.0)
            );
        }
        Block::DbView { db, .. } => {
            let _ = writeln!(
                out,
                "<div class=\"db-view\" data-db=\"{}\"></div>",
                html_escape(&db.0)
            );
        }
        Block::Toggle { summary, blocks } => {
            let _ = writeln!(
                out,
                "<details><summary>{}</summary>",
                inline_to_html(summary)
            );
            for sub in blocks {
                block_to_html(&ExportBlock::leaf("", sub.clone()), out);
            }
            out.push_str("</details>\n");
        }
        Block::SyncBlock { source } => {
            let _ = writeln!(
                out,
                "<div class=\"sync-block\" data-source=\"{}\"></div>",
                html_escape(&source.0)
            );
        }
    }
    for child in &b.children {
        block_to_html(child, out);
    }
}

fn item_blocks_html(blocks: &[Block]) -> String {
    let mut s = String::new();
    for b in blocks {
        block_to_html(&ExportBlock::leaf("", b.clone()), &mut s);
    }
    // Unwrap a single paragraph so `<li><p>x</p></li>` reads `<li>x</li>` for the common case.
    let trimmed = s.trim();
    if let Some(inner) = trimmed
        .strip_prefix("<p>")
        .and_then(|t| t.strip_suffix("</p>"))
    {
        if !inner.contains("<p>") {
            return inner.to_string();
        }
    }
    trimmed.to_string()
}

fn callout_tone_class(tone: CalloutTone) -> &'static str {
    match tone {
        CalloutTone::Info => "info",
        CalloutTone::Warn => "warn",
        CalloutTone::Success => "success",
        CalloutTone::Danger => "danger",
        CalloutTone::Note => "note",
    }
}

fn embed_display_class(d: EmbedDisplay) -> &'static str {
    match d {
        EmbedDisplay::Inline => "inline",
        EmbedDisplay::Card => "card",
        EmbedDisplay::Preview => "preview",
    }
}

/// Render an [`Inline`] to HTML — re-parse the canonical markdown-subset string (the ONE render
/// path, 13.1) into spans, then map marks/links to HTML elements. There is no second parser: this
/// reuses [`parse_inline`] so the HTML and the markdown agree by construction.
fn inline_to_html(inline: &Inline) -> String {
    use myelin_content::Span;
    let md = serialize_inline(inline);
    let reparsed = parse_inline(&md, inline.nodes.as_slice());
    let mut node_idx = 0usize;
    let mut out = String::new();
    for span in &reparsed.spans {
        match span {
            Span::Text { text, marks, link } => {
                let mut open = String::new();
                let mut close = String::new();
                if let Some(url) = link {
                    let _ = write!(open, "<a href=\"{}\">", html_escape(url));
                    close.insert_str(0, "</a>");
                }
                for m in marks {
                    let (o, c) = mark_tags(*m);
                    open.push_str(o);
                    close.insert_str(0, c);
                }
                out.push_str(&open);
                out.push_str(&html_escape(text));
                out.push_str(&close);
            }
            Span::Node { .. } => {
                // A structured node renders to a placeholder span carrying its kind (the unfurl is a
                // viewer-time projection — the export carries the typed ref, not a resolved render).
                let label = inline
                    .nodes
                    .get(node_idx)
                    .map(node_label)
                    .unwrap_or_else(|| "ref".to_string());
                node_idx += 1;
                let _ = write!(
                    out,
                    "<span class=\"inline-node\">{}</span>",
                    html_escape(&label)
                );
            }
        }
    }
    out
}

fn mark_tags(m: myelin_content::Mark) -> (&'static str, &'static str) {
    use myelin_content::Mark;
    match m {
        Mark::Bold => ("<strong>", "</strong>"),
        Mark::Italic => ("<em>", "</em>"),
        Mark::Code => ("<code>", "</code>"),
        Mark::Strike => ("<del>", "</del>"),
    }
}

fn node_label(n: &InlineNode) -> String {
    match n {
        InlineNode::Mention(p) => format!("@{}", p.principal_id.0),
        InlineNode::ArtifactRefNode(r) => format!("ref:{}", r.0),
        InlineNode::Embed(r) => format!("embed:{}", r.0),
    }
}

/// Escape the five HTML-significant characters (the minimal, correct entity set).
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

// ───────────────────────────── the plain-text + PDF exporter ───────────────────────────────────

fn block_to_plain_text(b: &ExportBlock, lines: &mut Vec<String>) {
    match &b.block {
        Block::Paragraph { inline } => lines.push(inline_to_plain(inline)),
        Block::Heading { inline, .. } => lines.push(inline_to_plain(inline)),
        Block::BulletList { items } => {
            for it in items {
                lines.push(format!("- {}", item_blocks_plain(&it.blocks)));
            }
        }
        Block::OrderedList { items, start } => {
            for (i, it) in items.iter().enumerate() {
                lines.push(format!(
                    "{}. {}",
                    *start as usize + i,
                    item_blocks_plain(&it.blocks)
                ));
            }
        }
        Block::TaskList { items } => {
            for it in items {
                let m = if it.checked { "[x]" } else { "[ ]" };
                lines.push(format!("{m} {}", inline_to_plain(&it.inline)));
            }
        }
        Block::Blockquote { blocks } | Block::Callout { blocks, .. } => {
            for sub in blocks {
                block_to_plain_text(&ExportBlock::leaf("", sub.clone()), lines);
            }
        }
        Block::CodeBlock { text, .. } => {
            for line in text.lines() {
                lines.push(line.to_string());
            }
        }
        Block::Table { columns, rows } => {
            lines.push(
                columns
                    .iter()
                    .map(|c| inline_to_plain(&c.header))
                    .collect::<Vec<_>>()
                    .join(" | "),
            );
            for row in rows {
                lines.push(
                    row.iter()
                        .map(|c| item_blocks_plain(&c.blocks))
                        .collect::<Vec<_>>()
                        .join(" | "),
                );
            }
        }
        Block::Divider => lines.push("----".to_string()),
        Block::Image { alt, .. } => lines.push(format!("[image: {alt}]")),
        Block::Embed { reference, .. } => lines.push(format!("[embed: {}]", reference.0)),
        Block::DbView { db, .. } => lines.push(format!("[db_view: {}]", db.0)),
        Block::Toggle { summary, blocks } => {
            lines.push(inline_to_plain(summary));
            for sub in blocks {
                block_to_plain_text(&ExportBlock::leaf("", sub.clone()), lines);
            }
        }
        Block::SyncBlock { source } => lines.push(format!("[sync_block: {}]", source.0)),
    }
    for child in &b.children {
        block_to_plain_text(child, lines);
    }
}

fn item_blocks_plain(blocks: &[Block]) -> String {
    let mut lines = Vec::new();
    for b in blocks {
        block_to_plain_text(&ExportBlock::leaf("", b.clone()), &mut lines);
    }
    lines.join(" ")
}

/// The plain (no-delimiter) text of an inline — strips the markdown marks, keeps the text + a
/// placeholder for each structured node.
fn inline_to_plain(inline: &Inline) -> String {
    use myelin_content::Span;
    let mut out = String::new();
    let mut node_idx = 0usize;
    for span in &inline.spans {
        match span {
            Span::Text { text, .. } => out.push_str(text),
            Span::Node { .. } => {
                if let Some(n) = inline.nodes.get(node_idx) {
                    out.push_str(&node_label(n));
                }
                node_idx += 1;
            }
        }
    }
    out
}

/// **Render a minimal, valid, self-contained PDF** (a `%PDF-1.4` document: catalog → pages → page →
/// content stream over the standard Helvetica font). The text flows as one column of lines; long
/// documents flow off the single page (pagination is the named follow-on). No external typesetter,
/// no embedded fonts, no I/O — a pure byte builder, so the export is deterministic and dependency-
/// free.
fn render_minimal_pdf(title: &str, body: &str) -> Vec<u8> {
    // Build the content stream: a text object placing each line down the page (PDF y grows upward).
    let mut content = String::new();
    content.push_str("BT\n/F1 12 Tf\n14 TL\n72 760 Td\n");
    // The title in a slightly larger run, then a blank line, then the body lines.
    let mut lines: Vec<String> = Vec::new();
    if !title.is_empty() {
        lines.push(title.to_string());
        lines.push(String::new());
    }
    lines.extend(body.lines().map(|l| l.to_string()));
    let mut first = true;
    for line in lines {
        if !first {
            // T* moves to the next line (using the leading set by TL).
            content.push_str("T*\n");
        }
        first = false;
        let _ = writeln!(content, "({}) Tj", pdf_escape(&line));
    }
    content.push_str("ET\n");

    // Assemble the PDF objects, tracking byte offsets for the xref table.
    let mut pdf = String::new();
    let mut offsets: Vec<usize> = Vec::new();
    pdf.push_str("%PDF-1.4\n");

    let push_obj = |pdf: &mut String, offsets: &mut Vec<usize>, body: &str| {
        offsets.push(pdf.len());
        pdf.push_str(body);
    };

    // 1: Catalog
    push_obj(
        &mut pdf,
        &mut offsets,
        "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
    );
    // 2: Pages
    push_obj(
        &mut pdf,
        &mut offsets,
        "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    // 3: Page (US Letter)
    push_obj(
        &mut pdf,
        &mut offsets,
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n",
    );
    // 4: Contents stream
    push_obj(
        &mut pdf,
        &mut offsets,
        &format!(
            "4 0 obj\n<< /Length {} >>\nstream\n{}endstream\nendobj\n",
            content.len(),
            content
        ),
    );
    // 5: Font (standard Helvetica — no embedding needed)
    push_obj(
        &mut pdf,
        &mut offsets,
        "5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
    );

    // xref table
    let xref_offset = pdf.len();
    let n = offsets.len() + 1; // +1 for the free object 0
    let _ = writeln!(pdf, "xref\n0 {n}");
    pdf.push_str("0000000000 65535 f \n");
    for off in &offsets {
        let _ = writeln!(pdf, "{off:010} 00000 n ");
    }
    let _ = write!(
        pdf,
        "trailer\n<< /Size {n} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
    );

    pdf.into_bytes()
}

/// Escape the three characters significant inside a PDF literal string `( )`.
fn pdf_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\\' => out.push_str("\\\\"),
            // non-ASCII collapses to '?' — the standard Helvetica WinAnsi base set is ASCII-safe;
            // the lossless surfaces (JSON/MD/HTML) carry the full unicode fidelity.
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => out.push('?'),
            c => out.push(c),
        }
    }
    out
}

// ───────────────────────────── the CSV exporter (flexible DB) ──────────────────────────────────

/// **Export flexible-database rows to CSV** (the `db_row` tabular export). The header row is the
/// [`FieldSchema`]'s field ids in declared order (a stable, total order — never the unordered map
/// iteration); each data row renders its [`FieldValue`]s for those fields (an absent field is an
/// empty cell). RFC-4180 quoting: a cell containing a comma / quote / newline is double-quoted with
/// `"` doubled. The export is permission-AGNOSTIC at this layer — the caller passes the already
/// permission-filtered rows ([`crate::execute_view_query`] result), so the CSV never re-implements
/// the ACL (EI-01 §7, the list-pushdown is the ONE filter).
pub fn export_rows_to_csv(schema: &FieldSchema, rows: &[DbRow]) -> String {
    let fields: Vec<&FieldId> = schema.fields().iter().map(|d| &d.field_id).collect();
    let mut out = String::new();
    // header
    out.push_str(
        &fields
            .iter()
            .map(|f| csv_escape(f.as_str()))
            .collect::<Vec<_>>()
            .join(","),
    );
    out.push('\n');
    // data rows
    for row in rows {
        let cells: Vec<String> = fields
            .iter()
            .map(|f| {
                row.props
                    .get(*f)
                    .map(field_value_to_csv)
                    .unwrap_or_default()
            })
            .map(|c| csv_escape(&c))
            .collect();
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    out
}

/// Render a [`FieldValue`] to its CSV cell text (the human-readable scalar, not the JSONB form).
fn field_value_to_csv(v: &FieldValue) -> String {
    match v {
        FieldValue::Text(s) | FieldValue::Date(s) | FieldValue::Select(s) => s.clone(),
        FieldValue::Relation(r) => r.clone(),
        FieldValue::Principal(p) => p.clone(),
        FieldValue::Int(n) => n.to_string(),
        FieldValue::Bool(b) => b.to_string(),
        FieldValue::OrderKey(k) => k.as_str().to_string(),
    }
}

/// RFC-4180 CSV quoting: a field with a comma, quote, or newline is wrapped in `"` with inner `"`
/// doubled; otherwise it passes through verbatim.
fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// ───────────────────────────── the ADF lossy-map import (13.2) ─────────────────────────────────

/// **A parsed ADF node** the importer consumes — the source node KIND + the already-resolved
/// payload the byte-level ADF JSON parser produced. THIS module owns the *conversion* (node → the
/// named [`Block`] target + the recorded loss); the byte-level ADF JSON parse (the Issues import's
/// job) is upstream — here a node carries the resolved bits the conversion needs (the text, whether
/// a mention/card resolved in-tenant, the macro name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedAdfNode {
    /// The ADF node kind (the frozen [`AdfNode`] the map keys on).
    pub kind: AdfNode,
    /// The node's text payload (the heading/paragraph text, the code body, the macro name, …).
    pub text: String,
    /// Whether a CONDITIONAL node resolved losslessly (a mention's principal resolved in-tenant; a
    /// card's URL resolved to a Myelin artifact; an emoji is a standard unicode glyph). Ignored for
    /// unconditional nodes.
    pub resolved: bool,
}

impl ParsedAdfNode {
    /// A node that resolves losslessly (the lossless branch of a conditional row, or a node whose
    /// row is unconditionally lossless).
    pub fn resolved(kind: AdfNode, text: impl Into<String>) -> ParsedAdfNode {
        ParsedAdfNode {
            kind,
            text: text.into(),
            resolved: true,
        }
    }

    /// A node that does NOT resolve (the degraded branch of a conditional row).
    pub fn unresolved(kind: AdfNode, text: impl Into<String>) -> ParsedAdfNode {
        ParsedAdfNode {
            kind,
            text: text.into(),
            resolved: false,
        }
    }
}

/// **The result of an ADF import** — the constructed [`ExportDoc`] + the [`ImportReport`] naming
/// every lossy conversion (the X-2 "recorded in the import report" obligation). The report is the
/// named-floor artifact the importing user sees; a fully-lossless import yields an empty report.
#[derive(Clone, Debug)]
pub struct AdfImportResult {
    /// The imported document (the `myelin-content` content tree).
    pub doc: ExportDoc,
    /// The per-import report — every lossy conversion, named, never silent.
    pub report: ImportReport,
}

/// **Import a stream of parsed ADF nodes into a Knowledge document** (contract 13.2). For each node
/// the importer reads the frozen [`mapping_for`] row, constructs the named target [`Block`], and —
/// when the conversion actually degraded — records the loss in the [`ImportReport`]. The conversion
/// table is the FROZEN [`myelin_content::MAP`]; this importer is bounded to exactly it (the X-2
/// anti-drift anchor).
///
/// Block-level nodes become top-level [`ExportBlock`]s; this models the flat block stream a single
/// page import produces (the nested-list / nested-table reconstruction is the byte-level parser's
/// job upstream — this owns the per-node target + loss).
pub fn import_adf(page_id: &str, title: &str, nodes: &[ParsedAdfNode]) -> AdfImportResult {
    let mut report = ImportReport::new();
    let mut blocks = Vec::new();

    for (i, node) in nodes.iter().enumerate() {
        let mapping = mapping_for(node.kind);
        // Decide the effective target + record the loss (the X-2 conditional/unconditional branches).
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
        let block = construct_block(effective_target, node);
        blocks.push(ExportBlock::leaf(format!("imported-{i}"), block));
    }

    AdfImportResult {
        doc: ExportDoc::new(page_id, title, None, blocks),
        report,
    }
}

/// Construct the named-target [`Block`] for an [`AdfTarget`] from a parsed node's payload. The
/// degraded targets (PlainText / Link / InlineCode / …) materialise into a `paragraph` with the
/// degraded inline content (a plain-text run, a `[text](url)` link, a `code` run) — the loss is
/// already recorded; here the content survives in its degraded form (never silently dropped).
fn construct_block(target: AdfTarget, node: &ParsedAdfNode) -> Block {
    let para = |inline: Inline| Block::Paragraph { inline };
    match target {
        AdfTarget::Paragraph => para(parse_inline(&node.text, &[])),
        AdfTarget::Heading => Block::Heading {
            level: HeadingLevel::new(1).expect("level 1 is valid"),
            inline: parse_inline(&node.text, &[]),
        },
        AdfTarget::Blockquote => Block::Blockquote {
            blocks: vec![para(parse_inline(&node.text, &[]))],
        },
        AdfTarget::CodeBlock => Block::CodeBlock {
            lang: None,
            text: node.text.clone(),
        },
        AdfTarget::Divider => Block::Divider,
        AdfTarget::BulletList => Block::BulletList {
            items: vec![ListItem {
                blocks: vec![para(parse_inline(&node.text, &[]))],
            }],
        },
        AdfTarget::OrderedList => Block::OrderedList {
            items: vec![ListItem {
                blocks: vec![para(parse_inline(&node.text, &[]))],
            }],
            start: 1,
        },
        AdfTarget::Table => Block::Table {
            columns: vec![Column {
                header: parse_inline(&node.text, &[]),
            }],
            rows: vec![],
        },
        AdfTarget::Image => Block::Image {
            blob: myelin_tenancy::ArtifactRef(format!("myelin://import/blob/{}", node.text)),
            alt: node.text.clone(),
            caption: None,
        },
        AdfTarget::TaskList | AdfTarget::TaskItem => Block::TaskList {
            items: vec![TaskItem {
                checked: false,
                inline: parse_inline(&node.text, &[]),
            }],
        },
        AdfTarget::Callout => Block::Callout {
            // a macro (extension) degrades to a note callout carrying the body + the marker.
            tone: CalloutTone::Note,
            blocks: vec![
                para(parse_inline(&node.text, &[])),
                para(parse_inline(
                    &format!(r"\[unsupported macro: {}]", node.text),
                    &[],
                )),
            ],
        },
        AdfTarget::Toggle => Block::Toggle {
            summary: parse_inline(&node.text, &[]),
            blocks: vec![],
        },
        AdfTarget::Mention => para(parse_inline(
            "\u{FFFC}",
            &[InlineNode::Mention(import_mention_principal(&node.text))],
        )),
        AdfTarget::ArtifactRef => para(parse_inline(
            "\u{FFFC}",
            &[InlineNode::ArtifactRefNode(myelin_tenancy::ArtifactRef(
                node.text.clone(),
            ))],
        )),
        // ── the degraded targets (the loss is recorded; the content survives degraded) ──
        AdfTarget::PlainText => para(parse_inline(&escape_plain(&node.text), &[])),
        AdfTarget::Link => para(parse_inline(
            &format!("[{}]({})", node.text, node.text),
            &[],
        )),
        AdfTarget::InlineCode => para(parse_inline(&format!("`{}`", node.text), &[])),
        AdfTarget::UnicodeGlyph => para(parse_inline(&escape_plain(&node.text), &[])),
        AdfTarget::FlattenedBlocks => para(parse_inline(&escape_plain(&node.text), &[])),
        AdfTarget::ImageWithAttachments => Block::Image {
            blob: myelin_tenancy::ArtifactRef(format!("myelin://import/blob/{}", node.text)),
            alt: node.text.clone(),
            caption: None,
        },
    }
}

/// Escape a degraded plain-text run so it round-trips through the markdown-subset grammar (a `*` in
/// degraded text must not become a delimiter). Re-uses the grammar's escaping by round-tripping
/// through the serializer.
fn escape_plain(text: &str) -> String {
    // parse a CODE-free literal: build an Inline of one plain run, then serialize (which escapes the
    // active delimiters), so the text is a fixed point of the grammar.
    use myelin_content::{Mark, Span};
    let inline = Inline {
        spans: vec![Span::Text {
            text: text.to_string(),
            marks: Vec::<Mark>::new(),
            link: None,
        }],
        nodes: vec![],
    };
    serialize_inline(&inline)
}

/// Build a [`Principal`] for an imported mention (a resolved in-tenant principal). The text is the
/// principal id; the kind is Human and the tenant a placeholder the import caller rebinds (the
/// byte-level parser resolves the real principal — here we carry the id so the structured node
/// survives).
fn import_mention_principal(id: &str) -> myelin_identity::Principal {
    myelin_identity::Principal::stub(
        myelin_identity::PrincipalId(id.to_string()),
        myelin_identity::PrincipalKind::Human,
        myelin_tenancy::TenantId("imported".to_string()),
    )
}

#[cfg(test)]
mod tests;
