//! **`humanise` — the ONE platform templating surface (contract 7.3, owned)** + the
//! `humanise_template` store + the NOTIF-D4 (0 title/PII leak) gate.
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/notifications.md` §3.3 (the humanise render
//! pipeline step-by-step: look up template → for EACH [`ArtifactRef`] arg resolve `resolve(ref,
//! viewer, Display)` → tombstone on deny → ICU-format; the ONE platform templating surface, no
//! second template engine; the four load-bearing properties: **permission-safe, erasure-safe,
//! always-current, agent-inherited**; markdown through the one `myelin-content` WASM render path;
//! email = sanitised-HTML, CLI = plain-text) + §2.5 (`humanise_template`, ICU MessageFormat,
//! platform-defaulted + tenant/locale-overridable).
//!
//! **Contract rows implemented:**
//! - **7.3** `humanise(item | (template_key, args), viewer, locale) -> HumanisedString{text,
//!   links[], icon}` (OWNED — the sole templating surface every other subsystem registers against,
//!   so the signature must NOT diverge locally; CI `HumanisedRef`, KN/Issues/Chat templates lower
//!   onto THIS).
//! - **5.2** `resolve(ref, viewer, mode=Display) -> Projection | Tombstone` (CONSUMED) — through the
//!   [`RefResolvePort`] seam: per-viewer, permission-checked; a denied/erased ref binds the slot to
//!   a tombstone display, NEVER the title (the NOTIF-D4 / F1 leak floor). This mirrors the Refs
//!   resolve chokepoint (REF-P10, `crates/myelin-refs-service/src/resolve.rs`) — Notif holds the
//!   port (a thin trait over 5.2), not the full `ResolveService` (the same seam discipline as the
//!   [`crate::list_inbox::ReadAuthorizePort`] over 4.2).
//! - **5.6** `project` (CONSUMED, transitively) — the owner's per-viewer projection the resolve
//!   chokepoint returns; humanise reads only `{title, icon}` off the [`RefProjection`] and a
//!   click-route to the ref.
//! - **13.1** `myelin-content` markdown-subset + WASM render (CONSUMED) — markdown in a humanised
//!   string renders through the ONE render path ([`render_markdown`]); `render(parse(md)) === md`
//!   (the content-crate round-trip generalised to humanise output). Email gets a sanitised-HTML
//!   projection ([`render_html`]); CLI gets plain-text ([`render_plain`]) — ONE content model, many
//!   channel projections, never per-channel string maps.
//!
//! ## The NOTIF-D4 leak invariant (the load-bearing property — 0 title/PII leak)
//! The render is **permission-safe by construction**: a confidential subject humanises to a
//! TOMBSTONE, the title NEVER leaks. This is structural — humanise binds each `{N}` slot to the
//! per-viewer [`RefResolution`] BEFORE formatting, and a [`RefResolution::Tombstone`] carries no
//! title field for a denied viewer's content to leak into (it carries only the opaque root URN + a
//! structured reason → the tombstone DISPLAY, e.g. `a restricted issue` / `[erased user]`). The
//! threshold is **0** — never inverted, never softened. The router (NOTIF-P3) additionally
//! suppresses an item whose subject the recipient cannot see; humanise is the SECOND, per-slot
//! line of defence (defence in depth — even a routed-by-mistake item cannot leak a title).
//!
//! ## Erasure-safe for free (EI-04 §1 — references-not-payloads)
//! An erased actor humanises to `[erased user]` with NO stored PII to scrub: the inbox stores
//! [`ArtifactRef`]s, never rendered strings (NOTIF-1), so erasing a person makes their ref resolve
//! to a `Tombstone{Erased}` → the erased display, with no mutation of the inbox row (§3.9, C7).
//!
//! ## Always-current (the chained-mutation property — EI-01 §4)
//! Because the slot is bound at READ time through the per-viewer resolve, a permission CHANGE
//! between two renders flips the slot from a title to a tombstone with NO re-write: render for a
//! viewer WITH access (title shown) → revoke (a new zookie) → re-render → the title is now a
//! tombstone (the chained test below proves it).
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **The canonical `myelin-content` WASM render path is KN-P01 (P-234).** The ledger inverts the
//!   dependency (P-234 is ordered AFTER this P-187), and `myelin-content` at this point ships only
//!   the three substrate inline-node TYPES (no render body). So [`render_markdown`] ships the
//!   deterministic markdown-SUBSET render here (round-trip `render(parse(md)) === md` PROVEN), and
//!   when KN-P01 lands the canonical WASM target, humanise calls THROUGH it (the render seam is one
//!   function — the swap is a body change, not a signature change). DOCUMENTED DEVIATION (see the
//!   report): the prompt names the WASM target as a dependency, but it is ordered later; the
//!   round-trip property is met by the subset renderer in the interim.
//! - **Cross-cell humanisation is single-home-cell here (OQ-I).** The always-cell-local resolution
//!   rule is built into the [`RefResolvePort`] call shape (a ref resolves in its home cell; only the
//!   already-filtered projection/tombstone crosses), but the multi-cell aggregation is **NOTIF-P24**
//!   (N-M5.1). Named.
//! - **The production resolve transport is the Refs `ResolveService` over the resilient client.**
//!   [`RefResolvePort`] is the call SEAM; the synthetic resolver in tests stands in for the real
//!   Refs chokepoint (REF-P10) reached over the substrate `ResilientClient` (whose wire body is the
//!   named `myelin-client` floor). The CHOKEPOINT logic (per-viewer gate → tombstone-never-leak) is
//!   real in Refs; here humanise CONSUMES its `Projection | Tombstone` result.
//!
//! ## Mutation-score floor (mandatory-core — EI-01 §3 / VISION §4 prove-it)
//! humanise is mandatory-core (every channel renderer leans on it) and leak-of-title-critical.
//! Floor: **≥ 80% of viable mutants caught** (`cargo mutants -p myelin-notif -f
//! crates/myelin-notif/src/humanise.rs`). Measured 2026-06-20 — see the dated artifact in the
//! commit body. Every leak-bearing rule (the tombstone-binds-on-deny arm, the erased→`[erased
//! user]` arm, the `{N}` slot substitution, the plural/select selection, the markdown round-trip,
//! each channel projection) has a test a mutation flips.

use std::collections::BTreeMap;
use std::sync::Arc;

use myelin_identity::{Consistency, Principal};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::router::RoutedInboxItem;
use crate::HumanisedString;

/// The frozen render MODE Notif resolves refs in (contract 5.2 `mode`) — always `Display` from
/// humanise (the SAME Refs chokepoint as `Live`; the mode only shapes the owner's render hint,
/// never whether the per-viewer permission gate runs). A named constant so a drill asserts the
/// NAME, never a literal.
pub const HUMANISE_RESOLVE_MODE: &str = "Display";

/// The platform-default tenant sentinel for the `humanise_template` store (§2.5) — the NULL-tenant
/// platform default row. A tenant override shadows it; absent an override, the default renders. The
/// ONE table whose tenant is nullable (the platform-defaulted + tenant/locale-overridable store).
pub const PLATFORM_DEFAULT_TENANT: &str = "00000000-0000-0000-0000-000000000000";

/// The default locale a viewer with no explicit locale renders in (§2.5 `locale text DEFAULT 'en'`).
pub const DEFAULT_LOCALE: &str = "en";

// ===========================================================================================
//  The 5.2 resolve seam (CONSUMED) — the per-viewer projection / tombstone humanise binds slots to
// ===========================================================================================

/// **The per-viewer projection an allowed ref resolves to (contract 5.6 shape, the slice humanise
/// reads).** Mirrors the Refs `Projection` (REF-P10) — humanise reads `{title, icon}` and a
/// click-route to the ref. Only ever produced on the ALLOWED branch (the resolve chokepoint gates
/// it); a denied/erased ref yields a [`RefResolution::Tombstone`] instead, which has NO title field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefProjection {
    /// The artifact this projection renders (the click-route target a humanised link points at).
    pub ref_: ArtifactRef,
    /// The human-facing title (MAY contain a name — `PersonalDataHolder` payload, §3.6). Present
    /// ONLY on a projection; a tombstone structurally cannot carry it (the leak invariant).
    pub title: String,
    /// The render icon hint (owner-supplied; drives the [`HumanisedString::icon`] for the subject).
    pub icon: String,
}

/// **Why a ref tombstoned (the frozen §4.6 ladder reasons; mirrors the Refs `TombstoneReason`).** A
/// structured enum — NEVER free-text that could leak the artifact's content. Each maps to a fixed,
/// PII-free tombstone DISPLAY (e.g. `a restricted issue` / `[erased user]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    /// The viewer is not permitted to `view` the subject (`check -> Deny`). The leak-free chokepoint
    /// — a confidential artifact degrades to a placeholder, the title NEVER present (NOTIF-D4).
    Denied,
    /// The parent artifact no longer exists.
    RootGone,
    /// The root resolves but the `#sub` anchor is gone (the embed shows the parent).
    SubGone,
    /// The artifact (or a level of it) was erased (pseudonym-shred / crypto-shred made it
    /// unrenderable) — the `[erased user]` display.
    Erased,
}

/// **A tombstone — the non-leaking placeholder (contract 5.2 / §4.6; mirrors the Refs `Tombstone`).**
/// It carries NO projection content — only the opaque root [`ArtifactRef`] + the structured
/// [`TombstoneReason`]. This is the STRUCTURAL guarantee of the leak invariant: there is no
/// `title`/`icon` field for a denied viewer's content to leak into (NOTIF-D4 — 0 leak).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    /// The root the tombstone carries (§4.6 — "a tombstone always carries the root"). An OPAQUE URN,
    /// never the title/content — safe to render as the kind-shaped display.
    pub root: ArtifactRef,
    /// Why the ref tombstoned (the §4.6 ladder reason). NEVER content — a structured enum.
    pub reason: TombstoneReason,
}

/// **The resolution outcome of `resolve(ref, viewer, Display)` (contract 5.2 — `Projection |
/// Tombstone`).** The leak invariant lives in the SHAPE: the tombstone arm cannot carry a title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefResolution {
    /// The ref rendered to a per-viewer projection (the ALLOWED + present branch).
    Projection(RefProjection),
    /// The ref degraded to a non-leaking tombstone (denied / gone / erased).
    Tombstone(Tombstone),
}

/// **The 5.2 resolve port humanise consumes (the per-viewer chokepoint seam).** A thin trait over
/// `resolve(ref, viewer, mode=Display) -> Projection | Tombstone` — Notif holds the PORT, not the
/// full Refs `ResolveService` (the same seam discipline as [`crate::list_inbox::ReadAuthorizePort`]
/// over 4.2). The production wire is the Refs resolve chokepoint (REF-P10) reached over the
/// substrate resilient client (the named floor); a denied/erased ref MUST return a
/// [`RefResolution::Tombstone`], never a leak.
///
/// `Send + Sync` so the humaniser can hold it behind an [`Arc`] across serving threads.
pub trait RefResolvePort: Send + Sync {
    /// Resolve `ref_` for `viewer` in `Display` mode at consistency `at`. Per-viewer,
    /// permission-checked: a denied/erased ref returns a [`RefResolution::Tombstone`] (NEVER a
    /// title); an allowed ref returns a [`RefResolution::Projection`].
    fn resolve_display(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        at: &Consistency,
    ) -> RefResolution;
}

// ===========================================================================================
//  The humanise_template store (§2.5) — the ONE templating surface's ICU MessageFormat templates
// ===========================================================================================

/// **A `humanise_template` row (§2.5) — an ICU-MessageFormat-subset template body.** Keyed
/// `(tenant|default, template_key, locale)`: a NULL-tenant ([`PLATFORM_DEFAULT_TENANT`]) row is the
/// platform default; a tenant row overrides (brand/locale). `body` is ICU-subset MessageFormat (the
/// markdown-subset string each `{N}` slot is substituted into; see [`render_message`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HumaniseTemplate {
    /// The owning tenant ([`PLATFORM_DEFAULT_TENANT`] for the platform default row).
    pub tenant: String,
    /// The template selector (e.g. `git.pr.merged`) — a stable key, NOT PII (§2.5).
    pub template_key: String,
    /// The locale the body is authored in (§2.5 `locale DEFAULT 'en'`).
    pub locale: String,
    /// The ICU-MessageFormat-subset body — markdown-subset text with `{N}` slots + `{N, plural,
    /// …}` / `{N, select, …}` (see [`render_message`]).
    pub body: String,
    /// The icon key the template renders with (the [`HumanisedString::icon`] default; a subject
    /// projection's icon overrides it when slot 0 is the subject).
    pub icon: String,
}

/// **The `humanise_template` store (§2.5) — the ONE platform templating store.** Platform-defaulted
/// plus tenant/locale-overridable. An in-memory model of the `notif_humanise_template` table
/// (NOTIF-P2): a `(tenant|default, template_key, locale)` lookup with the §2.5 fallback ladder
/// (tenant+locale, then tenant+'en', then default+locale, then default+'en'). The durable
/// persistence is the table; this models exactly its keyed read (the named live-DB floor — the
/// schema + RLS apply is proven in the integration test).
#[derive(Clone, Default)]
pub struct TemplateStore {
    /// Keyed `(tenant, template_key, locale)` → the template body. A `BTreeMap` so iteration /
    /// lookup is DETERMINISTIC (never a HashMap order leak — the render is reproducible).
    rows: BTreeMap<(String, String, String), HumaniseTemplate>,
}

impl TemplateStore {
    /// An empty store (no templates registered — every lookup misses → the fallback display).
    pub fn new() -> TemplateStore {
        TemplateStore { rows: BTreeMap::new() }
    }

    /// Register (or overwrite) a template row (§2.5 — a platform default or a tenant override).
    pub fn put(&mut self, t: HumaniseTemplate) {
        self.rows.insert(
            (t.tenant.clone(), t.template_key.clone(), t.locale.clone()),
            t,
        );
    }

    /// The §2.5 fallback ladder: tenant+locale → tenant+'en' → default+locale → default+'en'. The
    /// first hit wins; a tenant override always shadows the platform default at the same key. Returns
    /// `None` only if neither the tenant nor the platform default registered the key at all.
    pub fn lookup(
        &self,
        tenant: &str,
        template_key: &str,
        locale: &str,
    ) -> Option<&HumaniseTemplate> {
        let candidates = [
            (tenant.to_string(), template_key.to_string(), locale.to_string()),
            (tenant.to_string(), template_key.to_string(), DEFAULT_LOCALE.to_string()),
            (PLATFORM_DEFAULT_TENANT.to_string(), template_key.to_string(), locale.to_string()),
            (PLATFORM_DEFAULT_TENANT.to_string(), template_key.to_string(), DEFAULT_LOCALE.to_string()),
        ];
        candidates.iter().find_map(|k| self.rows.get(k))
    }

    /// Seed the platform-default reason templates (the Notif-owned default set, NOTIF-P8 reasons).
    /// Each is a NULL-tenant ([`PLATFORM_DEFAULT_TENANT`]) `en` row; a tenant brands/localises by
    /// [`TemplateStore::put`]ting an override. ICU-subset bodies — `{0}` binds the SUBJECT ref.
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

/// The Notif-owned platform-default templates (the §3.1 reason vocabulary's render bodies). `{0}`
/// is the SUBJECT slot (resolved per-viewer → title or tombstone display); higher slots are extra
/// args. ICU-subset bodies (markdown-subset text + `{N}` + plural/select). Frozen as the platform
/// default; a tenant overrides by registering its own `(tenant, key, locale)` row.
pub const PLATFORM_DEFAULT_TEMPLATES: &[(&str, &str, &str)] = &[
    ("approval_requested", "Approval requested on {0}", "approval"),
    ("escalated", "Escalated: {0}", "escalation"),
    ("sla", "SLA timer fired on {0}", "sla"),
    ("review_requested", "Review requested on {0}", "review"),
    ("assigned", "You were assigned {0}", "assigned"),
    ("mentioned", "You were mentioned in {0}", "mention"),
    ("replied", "New reply on {0}", "reply"),
    ("agent_proposal", "An agent proposed an effect on {0}", "agent"),
    ("watched", "{0} changed", "watch"),
    ("state_changed", "{0} changed state", "state"),
    ("fyi", "FYI: {0}", "fyi"),
    ("blocked", "{0} became blocked", "blocked"),
    ("unblocked", "{0} was unblocked", "unblocked"),
    ("thread_watched", "New activity in {0}", "thread"),
    ("shared", "{0} was shared with you", "shared"),
    ("comments", "New comments on {0}", "comments"),
];

// ===========================================================================================
//  The tombstone display (PII-free) — what a denied/erased slot renders as (NEVER the title)
// ===========================================================================================

/// **The PII-free tombstone DISPLAY for a [`TombstoneReason`] (§4.6 / NOTIF-D4).** A denied subject
/// renders as a kind-shaped placeholder (`a restricted <kind>`), an erased actor as `[erased user]`
/// — NEVER the title (which is structurally absent from a [`Tombstone`]). The kind is derived from
/// the OPAQUE root URN (the subsystem/artifact-type token), never from any content.
pub fn tombstone_display(t: &Tombstone) -> String {
    match t.reason {
        // An erased actor → the canonical erased display (EI-04 §1). Independent of kind.
        TombstoneReason::Erased => "[erased user]".to_string(),
        // Denied / gone → a kind-shaped restricted placeholder (the root carries the kind, never the
        // title). "a restricted issue" / "a restricted page" / "a restricted item".
        TombstoneReason::Denied | TombstoneReason::RootGone | TombstoneReason::SubGone => {
            let kind = artifact_kind(&t.root);
            format!("a restricted {kind}")
        }
    }
}

/// The artifact KIND token from an opaque `myelin://<tenant>/<subsystem>/<type>/<id>` root URN — the
/// PII-free noun the tombstone display uses (`issue`/`page`/`pr`/`message`/…). NEVER the title or
/// any content; derived purely from the URN structure. Unknown shapes fall back to `item`.
fn artifact_kind(root: &ArtifactRef) -> String {
    root.0
        .strip_prefix("myelin://")
        .and_then(|rest| {
            let mut parts = rest.split('/');
            let _tenant = parts.next();
            let _subsystem = parts.next();
            parts.next() // the artifact-TYPE token (issue/page/pr/message/…)
        })
        .filter(|k| !k.is_empty())
        .unwrap_or("item")
        .to_string()
}

// ===========================================================================================
//  The ICU-MessageFormat-subset formatter — {N} slots + {N, plural, …} + {N, select, …}
// ===========================================================================================

/// **Render an ICU-MessageFormat-SUBSET `body` with positional `args` (§2.5).** The subset:
/// - `{N}` — substitute the Nth arg verbatim (the per-viewer-bound slot text).
/// - `{N, plural, one {…} other {…}}` — select by the Nth arg parsed as an integer (`one` iff `==
///   1`, else `other`); `#` inside a branch is the count.
/// - `{N, select, key {…} other {…}}` — select by the Nth arg's string value (the matching key, else
///   `other`).
///
/// DETERMINISTIC, allocation-bounded, and dependency-free (the canonical ICU engine is the KN-P01
/// WASM floor; this subset covers the platform's plural/select/positional needs and round-trips
/// through the markdown path). An out-of-range slot renders as the empty string (never a panic — a
/// template referencing a missing arg degrades, it does not crash the inbox render).
pub fn render_message(body: &str, args: &[String]) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' {
            // Find the matching close brace (balanced — a plural/select branch nests braces).
            let (inner, next) = match read_braced(&chars, i) {
                Some(v) => v,
                // An unbalanced `{` is literal text (defensive — never panic on a malformed body).
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

/// Read a brace-balanced span starting at `chars[start] == '{'`; return its INNER text (without the
/// outer braces) and the index just past the matching `}`. `None` on an unbalanced run.
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

/// Render ONE placeholder's inner text (`0` | `0, plural, …` | `0, select, …`). Splits on the first
/// two top-level commas to find the arg index + the kind.
fn render_placeholder(inner: &str, args: &[String]) -> String {
    let parts: Vec<&str> = split_top_level_commas(inner, 2);
    let idx: usize = match parts.first().map(|s| s.trim().parse::<usize>()) {
        Some(Ok(n)) => n,
        // A non-numeric placeholder (`{foo}`) is not a slot — render it literally (defensive).
        _ => return format!("{{{inner}}}"),
    };
    let arg = args.get(idx).cloned().unwrap_or_default();
    match parts.get(1).map(|s| s.trim()) {
        Some("plural") => render_plural(parts.get(2).copied().unwrap_or(""), &arg, args),
        Some("select") => render_select(parts.get(2).copied().unwrap_or(""), &arg, args),
        // A bare `{N}` slot — substitute the arg verbatim.
        None => arg,
        // An unknown kind degrades to the bare slot (never a panic).
        Some(_) => arg,
    }
}

/// Split `s` on top-level (depth-0) commas into at most `limit + 1` pieces (the rest stays joined in
/// the last piece — a plural/select body has internal commas that must NOT split).
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

/// Render a `plural` body: `one {…} other {…}`. Selects `one` iff the arg parses to `1`, else
/// `other`. `#` in the chosen branch is replaced with the count.
fn render_plural(body: &str, arg: &str, args: &[String]) -> String {
    let branches = parse_branches(body);
    let n: i64 = arg.trim().parse().unwrap_or(-1);
    let key = if n == 1 { "one" } else { "other" };
    let chosen = branches
        .get(key)
        .or_else(|| branches.get("other"))
        .cloned()
        .unwrap_or_default();
    // `#` is the count; then recurse so nested `{M}` slots in the branch still bind.
    let with_count = chosen.replace('#', arg.trim());
    render_message(&with_count, args)
}

/// Render a `select` body: `key {…} other {…}`. Selects the branch matching the arg's value, else
/// `other`.
fn render_select(body: &str, arg: &str, args: &[String]) -> String {
    let branches = parse_branches(body);
    let chosen = branches
        .get(arg.trim())
        .or_else(|| branches.get("other"))
        .cloned()
        .unwrap_or_default();
    render_message(&chosen, args)
}

/// Parse `key {body} key {body} …` into a map. Deterministic (a `BTreeMap`).
fn parse_branches(s: &str) -> BTreeMap<String, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = BTreeMap::new();
    let mut i = 0;
    while i < chars.len() {
        // Skip whitespace, then read a key up to the next `{`.
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
        let key: String = chars[key_start..i].iter().collect::<String>().trim().to_string();
        if let Some((body, next)) = read_braced(&chars, i) {
            out.insert(key, body);
            i = next;
        } else {
            break;
        }
    }
    out
}

// ===========================================================================================
//  The ONE myelin-content render path (13.1) — markdown-subset, round-trip render(parse(md)) === md
// ===========================================================================================

/// The parsed markdown-subset document — the ONE content model (13.1). Many channel projections
/// ([`render_plain`] / [`render_html`]) lower from THIS — never per-channel string maps. A subset:
/// inline `**bold**`, `*italic*`, `` `code` ``, and `[label](url)` links over plain runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentDoc {
    /// The inline spans, in document order.
    pub spans: Vec<Span>,
}

/// One inline span of the markdown-subset content model (13.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Span {
    /// Plain text.
    Text(String),
    /// `**bold**`.
    Bold(String),
    /// `*italic*`.
    Italic(String),
    /// `` `code` ``.
    Code(String),
    /// `[label](url)`.
    Link { label: String, url: String },
}

/// **Parse a markdown-subset string into the ONE content model (13.1 `parse`).** The inverse of
/// [`render_markdown`]: `render_markdown(parse_markdown(md)) === md` (the round-trip the content
/// crate freezes; here generalised to humanise output — see the round-trip test). Deterministic,
/// dependency-free (the canonical WASM path is the KN-P01 floor).
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
        // `**bold**`
        if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            if let Some((inner, next)) = read_delim(&chars, i + 2, "**") {
                flush(&mut text, &mut spans);
                spans.push(Span::Bold(inner));
                i = next;
                continue;
            }
        }
        // `*italic*`
        if chars[i] == '*' {
            if let Some((inner, next)) = read_delim(&chars, i + 1, "*") {
                flush(&mut text, &mut spans);
                spans.push(Span::Italic(inner));
                i = next;
                continue;
            }
        }
        // `` `code` ``
        if chars[i] == '`' {
            if let Some((inner, next)) = read_delim(&chars, i + 1, "`") {
                flush(&mut text, &mut spans);
                spans.push(Span::Code(inner));
                i = next;
                continue;
            }
        }
        // `[label](url)`
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

/// Read an inline run delimited by `delim` starting at `start`; return the inner text + the index
/// just past the closing delimiter. `None` if the closer is absent (then the `*`/`` ` `` is literal).
fn read_delim(chars: &[char], start: usize, delim: &str) -> Option<(String, usize)> {
    let d: Vec<char> = delim.chars().collect();
    let mut i = start;
    let mut inner = String::new();
    while i < chars.len() {
        if chars[i..].starts_with(&d[..]) {
            // A non-empty run only (an empty `**` `**` is not bold; keep it literal).
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

/// Read a `[label](url)` link starting at `chars[start] == '['`. `None` if malformed (then `[` is
/// literal text).
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
    i += 1; // past ']'
    if i >= chars.len() || chars[i] != '(' {
        return None;
    }
    i += 1; // past '('
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

/// **Render the content model back to the markdown-subset string (13.1 `render`).** The exact
/// inverse of [`parse_markdown`]: `render_markdown(parse_markdown(md)) === md` (the content-crate
/// round-trip the prompt's render-determinism check requires). This is the ONE render path —
/// markdown is NEVER leaked raw; every channel projection lowers from the [`ContentDoc`].
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

/// **The CLI / in-app channel projection — plain text (the structure stripped).** Lowers the ONE
/// content model to plain text (no markup); the title slots already bound per-viewer (so a tombstone
/// stays a tombstone here too).
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

/// **The email channel projection — sanitised HTML.** Lowers the ONE content model to HTML with the
/// text content HTML-ESCAPED (never raw markdown, never an injection vector). The per-viewer slot
/// binding already happened, so a denied subject is a tombstone here too (the leak invariant holds
/// across every channel projection).
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

/// HTML-escape the five significant characters (sanitised-HTML — never an injection vector).
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

// ===========================================================================================
//  humanise — the ONE templating surface (contract 7.3, owned)
// ===========================================================================================

/// The channel a humanised string is projected for (§3.3 — ONE content model, many channel
/// projections). The slot binding (per-viewer resolve → title|tombstone) is channel-INDEPENDENT;
/// only the final content lowering differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// CLI / in-app — plain text ([`render_plain`]).
    Cli,
    /// Email — sanitised HTML ([`render_html`]).
    Email,
    /// The raw markdown-subset (the round-trip / firehose surface; [`render_markdown`]).
    Markdown,
}

/// **`humanise((template_key, args), viewer, locale) -> HumanisedString` — the ONE templating
/// surface (contract 7.3).** The §3.3 render pipeline:
/// 1. **Look up** `humanise_template[(tenant|default), template_key, viewer-locale]` (the §2.5
///    fallback ladder). An absent key → a stable fallback display (the key + the resolved args).
/// 2. **For EACH [`ArtifactRef`] arg, resolve `resolve(ref, viewer, Display)`** through the
///    [`RefResolvePort`] — PER-VIEWER, permission-checked. A [`RefResolution::Tombstone`] binds the
///    slot to the tombstone DISPLAY (`a restricted issue` / `[erased user]`); a
///    [`RefResolution::Projection`] binds it to the title (+ a click-route link to the ref + the
///    icon for slot 0, the subject).
/// 3. **ICU-format** the template body with the per-viewer-bound slots ([`render_message`]) → the
///    markdown-subset string → lower through the ONE content render path for `channel` → the final
///    [`HumanisedString`] {text, links, icon}.
///
/// **Permission-safe BY CONSTRUCTION (NOTIF-D4 — 0 title/PII leak):** a confidential subject
/// resolves to a tombstone → the slot is the tombstone display → the title is NEVER in `text` (the
/// tombstone carries no title field). The threshold is 0.
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
    // (2) Resolve EACH ref arg per-viewer BEFORE formatting — the slot text is the title (allowed)
    // or the tombstone display (denied/erased). Resolved FIRST so the leak invariant is structural:
    // a denied slot never carries a title into the formatter.
    let mut slot_texts: Vec<String> = Vec::with_capacity(args.len());
    let mut links: Vec<String> = Vec::new();
    let mut subject_icon: Option<String> = None;
    for (i, ref_) in args.iter().enumerate() {
        match resolver.resolve_display(tenant, region, ref_, viewer, at) {
            RefResolution::Projection(p) => {
                slot_texts.push(p.title);
                // A click-route link to the resolved ref (the allowed branch only — a tombstone
                // never yields a link to leak through).
                links.push(p.ref_.0.clone());
                // Slot 0 is the SUBJECT — its icon drives the item icon (overriding the template's).
                if i == 0 {
                    subject_icon = Some(p.icon);
                }
            }
            RefResolution::Tombstone(t) => {
                // The leak-free bind: the slot is the PII-free tombstone display, NEVER a title.
                slot_texts.push(tombstone_display(&t));
                // NO link for a tombstone (a denied/erased ref is not routable — never leak a route).
            }
        }
    }

    // (1) Look up the template (the §2.5 fallback ladder); absent → a stable fallback body.
    let (body, template_icon) = match templates.lookup(&tenant.0, template_key, locale) {
        Some(t) => (t.body.clone(), t.icon.clone()),
        // An unregistered key degrades to a stable, non-leaking fallback: the key + slot 0 (already
        // per-viewer-bound, so still a tombstone for a denied subject). Never raw, never a panic.
        None => (fallback_body(template_key, slot_texts.len()), template_key.to_string()),
    };

    // (3) ICU-format with the per-viewer-bound slots, then lower through the ONE content path.
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
        // The subject's projection icon wins (slot 0); else the template's icon.
        icon: subject_icon.unwrap_or(template_icon),
    }
}

/// **`humanise(item, viewer, locale)` — the inbox-item overload (contract 7.3).** Derives the
/// `(template_key, args)` from a [`RoutedInboxItem`]: the template key is the item's reason token,
/// the FIRST arg is the subject (slot 0). Re-uses the SAME [`humanise`] pipeline (the ONE templating
/// surface — never a second render path for items).
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
    // Slot 0 is the subject (the ref the item is about). The reason templates bind `{0}` to it.
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

/// The platform template key for a [`Reason`](crate::Reason) (the §3.1 reason → template mapping;
/// the snake_case token that keys the platform-default templates).
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

/// A stable, non-leaking fallback body for an UNREGISTERED template key (defence — a subsystem that
/// forgot to register a template still renders safely). `{0}` is the per-viewer-bound subject (still
/// a tombstone for a denied viewer), so even the fallback cannot leak a title.
fn fallback_body(template_key: &str, arg_count: usize) -> String {
    if arg_count > 0 {
        format!("{template_key}: {{0}}")
    } else {
        template_key.to_string()
    }
}

/// A convenience constructor for an [`Arc`]ed [`TemplateStore`] with the platform defaults — the
/// `serve` boot path holds ONE shared template store the humaniser reads.
pub fn shared_platform_templates() -> Arc<TemplateStore> {
    Arc::new(TemplateStore::with_platform_defaults())
}

#[cfg(test)]
mod tests;
