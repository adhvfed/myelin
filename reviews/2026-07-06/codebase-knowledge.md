# Knowledge platform (myelin-knowledge)

_The myelin-knowledge crate is largely healthy and unusually well-tested. The core security-sensitive paths are solid: the Layer-2 per-op permission gate (authority.rs) is fail-closed with a correct monotone zookie new-enemy guard; the document/block-tree model (block_tree.rs) enforces stable ids, cycle guards, and single-mint invariants; and the read-side ACL push-down (list_filter.rs) uses strict bound parameters with fail-closed defaults, closing count-leaks by construction. The one real defect is a stored-XSS hole in the HTML exporter, which renders user-authored link URLs into `href` with no scheme allowlist. A latent SQL-injection inconsistency exists in the block-tree query-plan SQL helpers (block_id interpolated rather than bound), currently not executed._

**Kept findings:** 2  (🟡 1 medium  ·  🔵 1 low)

---

### 1. 🟡 HTML export renders user-authored link URLs into href with no scheme allowlist (stored XSS)

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** content-sanitization
- **Location:** `crates/myelin-knowledge/src/export.rs:689`

**What:** `inline_to_html` (export.rs:689) writes `<a href="{}">` interpolating a user-authored link URL through only `html_escape` (export.rs:740, which escapes only & < > " '). No URL-scheme allowlist exists anywhere in myelin-knowledge or myelin-content. The URL originates verbatim from user markdown: `parse_link` (myelin-content/src/inline.rs:294-340) copies every char between `(`...`)` into the url string unchecked (line 336 `url.push(chars[j])`), and `inline_to_html` reparses and emits it. `html_escape` neutralizes attribute-breakout (quotes) but does NOT neutralize a dangerous scheme, since `:` and `javascript`/`data`/`vbscript` prefixes are not touched. So `[x](javascript:...)` and `[x](data:text/html,...)` survive into the emitted href.

**Impact:** A knowledge author writes `[click me](javascript:...)` or `[x](data:text/html,<script>...)`; `ExportDoc::to_html` (export.rs:271) emits `<a href="javascript:...">click me</a>` inside a self-contained `<!DOCTYPE html>` document. When another user opens the exported HTML and clicks the link, attacker-controlled JS executes. Note the reviewer's 'viewer's origin/document.cookie of the app' framing is overstated: to_html is documented as a downloadable self-contained document, so it opens in a file/null origin — the app's session cookies are not directly reachable there and execution requires a user click (not automatic). The real, still-legitimate impact is script execution in whatever context opens the export, and a direct miss of the crate's named content-sanitization obligation.

**Fix:** Before emitting href, parse the URL and allowlist safe schemes only (http, https, mailto, relative/anchor internal refs); drop or neutralize javascript:, data:, vbscript:, file: (replace with about:blank#blocked or strip the href). Apply the same allowlist to the markdown export path if it emits raw links. Add a regression test with a `javascript:` link asserting the scheme is stripped.

> _Verifier note:_ Read export.rs:271-282 (to_html emits full HTML doc), 674-719 (inline_to_html writes href via html_escape only), 739-753 (html_escape escapes only &<>"'). Read myelin-content/src/inline.rs:294-340 (parse_link copies url verbatim, no validation). grep for javascript/vbscript/scheme/allowlist/about:blank across both crates' src returned only unrelated 'one scheme, two stores' doc comments — confirms no scheme validation. Defect confirmed; downgraded high→medium because the export is a standalone null/file-origin download requiring a click, so the app-cookie-theft impact is overstated.

### 2. 🔵 block_tree query-plan SQL interpolates block_id directly into the statement (latent SQL injection)

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** security
- **Location:** `crates/myelin-knowledge/src/block_tree.rs:439`

**What:** `children_index_range_sql` (block_tree.rs:439) and `recursive_subtree_cte_sql` (block_tree.rs:455) bind tenant/page_id as $1/$2 but interpolate `parent.as_str()`/`root.as_str()` directly into the statement via format! inside single quotes (`parent_id = '{}'` at line 445; `block_id = '{}'` at line 460). BlockId is `pub struct BlockId(pub String)` (line 59) with a public field and NO validation in construction or as_str (lines 61-65), so an id containing a single quote breaks the literal. This diverges from the bound-param discipline used elsewhere. The functions are currently only re-exported (lib.rs:146) and exercised in string-shape unit tests (lines 740, 763) — no live-DB executor consumes them, so this is latent, not an active exploit.

**Impact:** If these strings are executed against a live adjacency-list table once the Postgres read path lands, a block_id carrying a single quote would break out of the literal and inject SQL under the tenant's DB session (or at minimum corrupt the query plan). Today no runtime path executes them, so current impact is confined to a divergence from the codebase's injection-safe convention plus a footgun for whoever wires the live driver.

**Fix:** Bind the parent/root block_id as a parameter ($3) exactly as tenant/page_id are bound, so the emitted artifact is injection-safe by construction when a live driver consumes it.

> _Verifier note:_ Read block_tree.rs:426-469 (both functions use format! with parent/root.as_str() inside quoted literal while $1/$2 bind tenant/page_id), and 55-65 (BlockId is a pub-field String tuple with no validation). grep across the whole repo for the three symbols: only re-export at lib.rs:146 and two in-crate string-assertion tests (740, 763) — no live executor. Latent SQLi confirmed; low severity is correct.
