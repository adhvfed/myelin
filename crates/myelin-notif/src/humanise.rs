use std::collections::BTreeMap;
use std::sync::Arc;

use myelin_identity::{Consistency, Principal};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::router::RoutedInboxItem;
use crate::HumanisedString;

pub const HUMANISE_RESOLVE_MODE: &str = "Display";

pub const PLATFORM_DEFAULT_TENANT: &str = "00000000-0000-0000-0000-000000000000";

pub const DEFAULT_LOCALE: &str = "en";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefProjection {
    pub ref_: ArtifactRef,
    pub title: String,
    pub icon: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    Denied,
    RootGone,
    SubGone,
    Erased,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    pub root: ArtifactRef,
    pub reason: TombstoneReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefResolution {
    Projection(RefProjection),
    Tombstone(Tombstone),
}

pub trait RefResolvePort: Send + Sync {
    fn resolve_display(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        at: &Consistency,
    ) -> RefResolution;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HumaniseTemplate {
    pub tenant: String,
    pub template_key: String,
    pub locale: String,
    pub body: String,
    pub icon: String,
}

#[derive(Clone, Default)]
pub struct TemplateStore {
    rows: BTreeMap<(String, String, String), HumaniseTemplate>,
}

impl TemplateStore {
    pub fn new() -> TemplateStore {
        TemplateStore {
            rows: BTreeMap::new(),
        }
    }

    pub fn put(&mut self, t: HumaniseTemplate) {
        self.rows.insert(
            (t.tenant.clone(), t.template_key.clone(), t.locale.clone()),
            t,
        );
    }

    pub fn lookup(
        &self,
        tenant: &str,
        template_key: &str,
        locale: &str,
    ) -> Option<&HumaniseTemplate> {
        let candidates = [
            (
                tenant.to_string(),
                template_key.to_string(),
                locale.to_string(),
            ),
            (
                tenant.to_string(),
                template_key.to_string(),
                DEFAULT_LOCALE.to_string(),
            ),
            (
                PLATFORM_DEFAULT_TENANT.to_string(),
                template_key.to_string(),
                locale.to_string(),
            ),
            (
                PLATFORM_DEFAULT_TENANT.to_string(),
                template_key.to_string(),
                DEFAULT_LOCALE.to_string(),
            ),
        ];
        candidates.iter().find_map(|k| self.rows.get(k))
    }

    pub fn with_platform_defaults() -> TemplateStore {
        let mut s = TemplateStore::new();
        for (key, body, icon) in PLATFORM_DEFAULT_TEMPLATES {
            s.put(HumaniseTemplate {
                tenant: PLATFORM_DEFAULT_TENANT.to_string(),
                template_key: (*key).to_string(),
                locale: DEFAULT_LOCALE.to_string(),
                body: (*body).to_string(),
                icon: (*icon).to_string(),
            });
        }
        s
    }
}

pub const PLATFORM_DEFAULT_TEMPLATES: &[(&str, &str, &str)] = &[
    (
        "approval_requested",
        "Approval requested on {0}",
        "approval",
    ),
    ("escalated", "Escalated: {0}", "escalation"),
    ("sla", "SLA timer fired on {0}", "sla"),
    ("review_requested", "Review requested on {0}", "review"),
    ("assigned", "You were assigned {0}", "assigned"),
    ("mentioned", "You were mentioned in {0}", "mention"),
    ("replied", "New reply on {0}", "reply"),
    (
        "agent_proposal",
        "An agent proposed an effect on {0}",
        "agent",
    ),
    ("watched", "{0} changed", "watch"),
    ("state_changed", "{0} changed state", "state"),
    ("fyi", "FYI: {0}", "fyi"),
    ("blocked", "{0} became blocked", "blocked"),
    ("unblocked", "{0} was unblocked", "unblocked"),
    ("thread_watched", "New activity in {0}", "thread"),
    ("shared", "{0} was shared with you", "shared"),
    ("comments", "New comments on {0}", "comments"),
];

pub fn tombstone_display(t: &Tombstone) -> String {
    match t.reason {
        TombstoneReason::Erased => "[erased user]".to_string(),
        TombstoneReason::Denied | TombstoneReason::RootGone | TombstoneReason::SubGone => {
            let kind = artifact_kind(&t.root);
            format!("a restricted {kind}")
        }
    }
}

fn artifact_kind(root: &ArtifactRef) -> String {
    root.0
        .strip_prefix("myelin://")
        .and_then(|rest| {
            let mut parts = rest.split('/');
            let _tenant = parts.next();
            let _subsystem = parts.next();
            parts.next()
        })
        .filter(|k| !k.is_empty())
        .unwrap_or("item")
        .to_string()
}

pub fn render_message(body: &str, args: &[String]) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' {
            let (inner, next) = match read_braced(&chars, i) {
                Some(v) => v,
                None => {
                    out.push('{');
                    i += 1;
                    continue;
                }
            };
            out.push_str(&render_placeholder(&inner, args));
            i = next;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn read_braced(chars: &[char], start: usize) -> Option<(String, usize)> {
    debug_assert_eq!(chars[start], '{');
    let mut depth = 0usize;
    let mut inner = String::new();
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '{' => {
                if depth > 0 {
                    inner.push('{');
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((inner, i + 1));
                }
                inner.push('}');
            }
            c => inner.push(c),
        }
        i += 1;
    }
    None
}

fn render_placeholder(inner: &str, args: &[String]) -> String {
    let parts: Vec<&str> = split_top_level_commas(inner, 2);
    let idx: usize = match parts.first().map(|s| s.trim().parse::<usize>()) {
        Some(Ok(n)) => n,
        _ => return format!("{{{inner}}}"),
    };
    let arg = args.get(idx).cloned().unwrap_or_default();
    match parts.get(1).map(|s| s.trim()) {
        Some("plural") => render_plural(parts.get(2).copied().unwrap_or(""), &arg, args),
        Some("select") => render_select(parts.get(2).copied().unwrap_or(""), &arg, args),
        None => arg,
        Some(_) => arg,
    }
}

fn split_top_level_commas(s: &str, limit: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut last = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 && out.len() < limit => {
                out.push(&s[last..i]);
                last = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[last..]);
    out
}

fn render_plural(body: &str, arg: &str, args: &[String]) -> String {
    let branches = parse_branches(body);
    let n: i64 = arg.trim().parse().unwrap_or(-1);
    let key = if n == 1 { "one" } else { "other" };
    let chosen = branches
        .get(key)
        .or_else(|| branches.get("other"))
        .cloned()
        .unwrap_or_default();
    let with_count = chosen.replace('#', arg.trim());
    render_message(&with_count, args)
}

fn render_select(body: &str, arg: &str, args: &[String]) -> String {
    let branches = parse_branches(body);
    let chosen = branches
        .get(arg.trim())
        .or_else(|| branches.get("other"))
        .cloned()
        .unwrap_or_default();
    render_message(&chosen, args)
}

fn parse_branches(s: &str) -> BTreeMap<String, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = BTreeMap::new();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        let key_start = i;
        while i < chars.len() && chars[i] != '{' {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let key: String = chars[key_start..i]
            .iter()
            .collect::<String>()
            .trim()
            .to_string();
        if let Some((body, next)) = read_braced(&chars, i) {
            out.insert(key, body);
            i = next;
        } else {
            break;
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentDoc {
    pub spans: Vec<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Span {
    Text(String),
    Bold(String),
    Italic(String),
    Code(String),
    Link { label: String, url: String },
}

pub fn parse_markdown(md: &str) -> ContentDoc {
    let chars: Vec<char> = md.chars().collect();
    let mut spans = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    let flush = |text: &mut String, spans: &mut Vec<Span>| {
        if !text.is_empty() {
            spans.push(Span::Text(std::mem::take(text)));
        }
    };
    while i < chars.len() {
        if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            if let Some((inner, next)) = read_delim(&chars, i + 2, "**") {
                flush(&mut text, &mut spans);
                spans.push(Span::Bold(inner));
                i = next;
                continue;
            }
        }
        if chars[i] == '*' {
            if let Some((inner, next)) = read_delim(&chars, i + 1, "*") {
                flush(&mut text, &mut spans);
                spans.push(Span::Italic(inner));
                i = next;
                continue;
            }
        }
        if chars[i] == '`' {
            if let Some((inner, next)) = read_delim(&chars, i + 1, "`") {
                flush(&mut text, &mut spans);
                spans.push(Span::Code(inner));
                i = next;
                continue;
            }
        }
        if chars[i] == '[' {
            if let Some((label, url, next)) = read_link(&chars, i) {
                flush(&mut text, &mut spans);
                spans.push(Span::Link { label, url });
                i = next;
                continue;
            }
        }
        text.push(chars[i]);
        i += 1;
    }
    flush(&mut text, &mut spans);
    ContentDoc { spans }
}

fn read_delim(chars: &[char], start: usize, delim: &str) -> Option<(String, usize)> {
    let d: Vec<char> = delim.chars().collect();
    let mut i = start;
    let mut inner = String::new();
    while i < chars.len() {
        if chars[i..].starts_with(&d[..]) {
            if inner.is_empty() {
                return None;
            }
            return Some((inner, i + d.len()));
        }
        inner.push(chars[i]);
        i += 1;
    }
    None
}

fn read_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    debug_assert_eq!(chars[start], '[');
    let mut i = start + 1;
    let mut label = String::new();
    while i < chars.len() && chars[i] != ']' {
        label.push(chars[i]);
        i += 1;
    }
    if i >= chars.len() || chars[i] != ']' {
        return None;
    }
    i += 1;
    if i >= chars.len() || chars[i] != '(' {
        return None;
    }
    i += 1;
    let mut url = String::new();
    while i < chars.len() && chars[i] != ')' {
        url.push(chars[i]);
        i += 1;
    }
    if i >= chars.len() || chars[i] != ')' {
        return None;
    }
    Some((label, url, i + 1))
}

pub fn render_markdown(doc: &ContentDoc) -> String {
    let mut out = String::new();
    for span in &doc.spans {
        match span {
            Span::Text(t) => out.push_str(t),
            Span::Bold(t) => {
                out.push_str("**");
                out.push_str(t);
                out.push_str("**");
            }
            Span::Italic(t) => {
                out.push('*');
                out.push_str(t);
                out.push('*');
            }
            Span::Code(t) => {
                out.push('`');
                out.push_str(t);
                out.push('`');
            }
            Span::Link { label, url } => {
                out.push('[');
                out.push_str(label);
                out.push_str("](");
                out.push_str(url);
                out.push(')');
            }
        }
    }
    out
}

pub fn render_plain(doc: &ContentDoc) -> String {
    let mut out = String::new();
    for span in &doc.spans {
        match span {
            Span::Text(t) | Span::Bold(t) | Span::Italic(t) | Span::Code(t) => out.push_str(t),
            Span::Link { label, .. } => out.push_str(label),
        }
    }
    out
}

pub fn render_html(doc: &ContentDoc) -> String {
    let mut out = String::new();
    for span in &doc.spans {
        match span {
            Span::Text(t) => out.push_str(&html_escape(t)),
            Span::Bold(t) => {
                out.push_str("<strong>");
                out.push_str(&html_escape(t));
                out.push_str("</strong>");
            }
            Span::Italic(t) => {
                out.push_str("<em>");
                out.push_str(&html_escape(t));
                out.push_str("</em>");
            }
            Span::Code(t) => {
                out.push_str("<code>");
                out.push_str(&html_escape(t));
                out.push_str("</code>");
            }
            Span::Link { label, url } => {
                out.push_str("<a href=\"");
                out.push_str(&html_escape(url));
                out.push_str("\">");
                out.push_str(&html_escape(label));
                out.push_str("</a>");
            }
        }
    }
    out
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
            c => out.push(c),
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Cli,
    Email,
    Markdown,
}

#[allow(clippy::too_many_arguments)]
pub fn humanise(
    resolver: &dyn RefResolvePort,
    tenant: &TenantId,
    region: &Region,
    templates: &TemplateStore,
    template_key: &str,
    args: &[ArtifactRef],
    viewer: &Principal,
    locale: &str,
    at: &Consistency,
    channel: Channel,
) -> HumanisedString {
    let mut slot_texts: Vec<String> = Vec::with_capacity(args.len());
    let mut links: Vec<String> = Vec::new();
    let mut subject_icon: Option<String> = None;
    for (i, ref_) in args.iter().enumerate() {
        match resolver.resolve_display(tenant, region, ref_, viewer, at) {
            RefResolution::Projection(p) => {
                slot_texts.push(p.title);
                links.push(p.ref_.0.clone());
                if i == 0 {
                    subject_icon = Some(p.icon);
                }
            }
            RefResolution::Tombstone(t) => {
                slot_texts.push(tombstone_display(&t));
            }
        }
    }

    let (body, template_icon) = match templates.lookup(&tenant.0, template_key, locale) {
        Some(t) => (t.body.clone(), t.icon.clone()),
        None => (
            fallback_body(template_key, slot_texts.len()),
            template_key.to_string(),
        ),
    };

    let formatted = render_message(&body, &slot_texts);
    let doc = parse_markdown(&formatted);
    let text = match channel {
        Channel::Cli => render_plain(&doc),
        Channel::Email => render_html(&doc),
        Channel::Markdown => render_markdown(&doc),
    };

    HumanisedString {
        text,
        links,
        icon: subject_icon.unwrap_or(template_icon),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn humanise_item(
    resolver: &dyn RefResolvePort,
    templates: &TemplateStore,
    item: &RoutedInboxItem,
    viewer: &Principal,
    locale: &str,
    at: &Consistency,
    channel: Channel,
) -> HumanisedString {
    let key = reason_template_key(item.reason);
    let args = vec![item.subject.clone()];
    humanise(
        resolver,
        &item.tenant,
        &item.region,
        templates,
        key,
        &args,
        viewer,
        locale,
        at,
        channel,
    )
}

pub fn reason_template_key(reason: crate::Reason) -> &'static str {
    use crate::Reason::*;
    match reason {
        ApprovalRequested => "approval_requested",
        Escalated => "escalated",
        Sla => "sla",
        ReviewRequested => "review_requested",
        Assigned => "assigned",
        Mentioned => "mentioned",
        Replied => "replied",
        AgentProposal => "agent_proposal",
        Watched => "watched",
        StateChanged => "state_changed",
        Fyi => "fyi",
        Blocked => "blocked",
        Unblocked => "unblocked",
        ThreadWatched => "thread_watched",
        Shared => "shared",
        Comments => "comments",
    }
}

fn fallback_body(template_key: &str, arg_count: usize) -> String {
    if arg_count > 0 {
        format!("{template_key}: {{0}}")
    } else {
        template_key.to_string()
    }
}

pub fn shared_platform_templates() -> Arc<TemplateStore> {
    Arc::new(TemplateStore::with_platform_defaults())
}

#[cfg(test)]
mod tests;
