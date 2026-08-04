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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Json,
    Markdown,
    Html,
    Pdf,
    Csv,
}

impl ExportFormat {
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportBlock {
    pub id: BlockId,
    pub block: Block,
    pub children: Vec<ExportBlock>,
}

impl ExportBlock {
    pub fn leaf(id: impl Into<String>, block: Block) -> ExportBlock {
        ExportBlock {
            id: BlockId(id.into()),
            block,
            children: Vec::new(),
        }
    }

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

    fn walk<'a>(&'a self, out: &mut Vec<&'a ExportBlock>) {
        out.push(self);
        for c in &self.children {
            c.walk(out);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportDoc {
    pub page_id: PageId,
    pub title: String,
    pub parent_page: Option<PageId>,
    pub blocks: Vec<ExportBlock>,
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
}

pub const EXPORT_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    EXPORT_SCHEMA_VERSION
}

impl ExportDoc {
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

    pub fn all_blocks(&self) -> Vec<&ExportBlock> {
        let mut out = Vec::new();
        for b in &self.blocks {
            b.walk(&mut out);
        }
        out
    }

    pub fn to_json_bundle(&self) -> String {
        serde_json::to_string_pretty(self).expect("ExportDoc serialises (a closed serde shape)")
    }

    pub fn from_json_bundle(json: &str) -> Result<ExportDoc, ExportError> {
        serde_json::from_str(json).map_err(|e| ExportError::MalformedBundle(e.to_string()))
    }

    pub fn json_roundtrips(&self) -> bool {
        let json = self.to_json_bundle();
        let back = match ExportDoc::from_json_bundle(&json) {
            Ok(d) => d,
            Err(_) => return false,
        };
        if &back != self {
            return false;
        }
        self.all_inlines_roundtrip() && back.all_inlines_roundtrip()
    }

    pub fn all_inlines_roundtrip(&self) -> bool {
        self.all_blocks()
            .iter()
            .all(|b| inline_roundtrips(&b.block))
    }

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

    pub fn to_pdf(&self) -> Vec<u8> {
        let text = self.to_plain_text();
        render_minimal_pdf(&self.title, &text)
    }

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportError {
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
        Block::CodeBlock { .. }
        | Block::Divider
        | Block::Embed { .. }
        | Block::DbView { .. }
        | Block::SyncBlock { .. } => true,
    }
}

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
    for child in &b.children {
        block_to_markdown(child, depth + 1, out);
    }
}

fn list_item_md(it: &ListItem, depth: usize, marker: &str, out: &mut String) {
    let indent = "  ".repeat(depth);
    let mut blocks = it.blocks.iter();
    if let Some(first) = blocks.next() {
        let mut buf = String::new();
        block_to_markdown(&ExportBlock::leaf("", first.clone()), 0, &mut buf);
        let line = buf.lines().next().unwrap_or("");
        let _ = writeln!(out, "{indent}{marker}{line}");
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

fn render_minimal_pdf(title: &str, body: &str) -> Vec<u8> {
    let mut content = String::new();
    content.push_str("BT\n/F1 12 Tf\n14 TL\n72 760 Td\n");
    let mut lines: Vec<String> = Vec::new();
    if !title.is_empty() {
        lines.push(title.to_string());
        lines.push(String::new());
    }
    lines.extend(body.lines().map(|l| l.to_string()));
    let mut first = true;
    for line in lines {
        if !first {
            content.push_str("T*\n");
        }
        first = false;
        let _ = writeln!(content, "({}) Tj", pdf_escape(&line));
    }
    content.push_str("ET\n");

    let mut pdf = String::new();
    let mut offsets: Vec<usize> = Vec::new();
    pdf.push_str("%PDF-1.4\n");

    let push_obj = |pdf: &mut String, offsets: &mut Vec<usize>, body: &str| {
        offsets.push(pdf.len());
        pdf.push_str(body);
    };

    push_obj(
        &mut pdf,
        &mut offsets,
        "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
    );
    push_obj(
        &mut pdf,
        &mut offsets,
        "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    push_obj(
        &mut pdf,
        &mut offsets,
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n",
    );
    push_obj(
        &mut pdf,
        &mut offsets,
        &format!(
            "4 0 obj\n<< /Length {} >>\nstream\n{}endstream\nendobj\n",
            content.len(),
            content
        ),
    );
    push_obj(
        &mut pdf,
        &mut offsets,
        "5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
    );

    let xref_offset = pdf.len();
    let n = offsets.len() + 1;
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

fn pdf_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => out.push('?'),
            c => out.push(c),
        }
    }
    out
}

pub fn export_rows_to_csv(schema: &FieldSchema, rows: &[DbRow]) -> String {
    let fields: Vec<&FieldId> = schema.fields().iter().map(|d| &d.field_id).collect();
    let mut out = String::new();
    out.push_str(
        &fields
            .iter()
            .map(|f| csv_escape(f.as_str()))
            .collect::<Vec<_>>()
            .join(","),
    );
    out.push('\n');
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

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedAdfNode {
    pub kind: AdfNode,
    pub text: String,
    pub resolved: bool,
}

impl ParsedAdfNode {
    pub fn resolved(kind: AdfNode, text: impl Into<String>) -> ParsedAdfNode {
        ParsedAdfNode {
            kind,
            text: text.into(),
            resolved: true,
        }
    }

    pub fn unresolved(kind: AdfNode, text: impl Into<String>) -> ParsedAdfNode {
        ParsedAdfNode {
            kind,
            text: text.into(),
            resolved: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AdfImportResult {
    pub doc: ExportDoc,
    pub report: ImportReport,
}

pub fn import_adf(page_id: &str, title: &str, nodes: &[ParsedAdfNode]) -> AdfImportResult {
    let mut report = ImportReport::new();
    let mut blocks = Vec::new();

    for (i, node) in nodes.iter().enumerate() {
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
        let block = construct_block(effective_target, node);
        blocks.push(ExportBlock::leaf(format!("imported-{i}"), block));
    }

    AdfImportResult {
        doc: ExportDoc::new(page_id, title, None, blocks),
        report,
    }
}

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

fn escape_plain(text: &str) -> String {
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

fn import_mention_principal(id: &str) -> myelin_identity::Principal {
    myelin_identity::Principal::stub(
        myelin_identity::PrincipalId(id.to_string()),
        myelin_identity::PrincipalKind::Human,
        myelin_tenancy::TenantId("imported".to_string()),
    )
}

#[cfg(test)]
mod tests;
