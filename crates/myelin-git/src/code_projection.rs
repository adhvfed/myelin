//! # `code_projection` — the code-projection EMITTER for Search (GIT-P25 / P-287, M3-G5)
//!
//! Git **owns what to index**; Search owns the index (00-overview §1.1; contract 6.3/6.5; Search
//! §4.4). The SPEC half — git's `declare_indexable` projection schema — shipped in GIT-P5 / P-231
//! ([`crate::search_projection`]). **THIS module is the EMITTER half**: the receive-pack post-commit
//! hook (architecture
//! `planning/04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md` §9 —
//! TE-27, the code projection) that, on a `git.ref.updated` to an **indexed ref**, walks the blobs
//! changed between the ref's `last_indexed_oid` (the [`code_projection_cursor`](CodeProjectionCursor))
//! and the new tip and **emits one projection doc per changed blob through the outbox**.
//!
//! ## The §9 algorithm (verbatim shape)
//!
//! ```text
//! for each blob changed between last_indexed_oid and new tip (code_projection_cursor):
//!   emit a projection doc per blob:
//!     { artifact_ref: myelin://<tenant>/git/blob/<repo>:<ref>:<path>,
//!       path, language (detected),
//!       symbols:  [ identifiers split on camelCase/snake_case, def-like names ],
//!       literals: [ string/number literals ],
//!       text: <blob text>,                 # Search builds trigrams (Cox 2012) — we supply the text
//!       commit_message: <tip commit message>, blob_oid }
//!   via OutboxTx::emit (so replay re-emits the same path — contract 2.6 / SEARCH-1)
//! update code_projection_cursor
//! ```
//!
//! Git emits the projection; it does **NOT** build the trigram/symbol/path/literal index — Search
//! does (no cross-DB; the §9 "Search builds trigrams"). The doc rides the **outbox** so
//! `replay(scope, since)` re-emits exactly the same path-keyed doc for a cold Search reindex (the
//! one rebuild path, EI-04 §5.3) — the live-emit and cold-replay shapes are byte-identical
//! ([`BlobProjection::into_search_projection`] is the ONE shape both produce).
//!
//! ## Incremental-on-push (the GATE: emit-count == changed-blob-count)
//!
//! A push of `N` changed blobs emits **exactly `N`** projection updates — 0 missed, 0 stale. The
//! cursor ([`CodeProjectionCursor`]) is the `last_indexed_oid` per `(repo, ref)`; the diff
//! ([`diff_trees`]) is `new_tip ∖ last_indexed` at path granularity:
//! - a blob ADDED or MODIFIED at a path → one [`BlobChange::Upserted`] → one projection emit;
//! - a blob DELETED at a path → one [`BlobChange::Deleted`] → one **tombstone** emit (so Search
//!   removes the stale doc; `Gone` is never silently dropped — EI-01 §3, no stale index doc).
//!
//! Unchanged blobs (same oid at the same path) emit nothing — that is what makes it incremental
//! (a 10k-file monorepo whose push touches 3 files emits 3 docs, not 10k). The cursor advances to
//! the new tip **only after** the emits commit in the SAME outbox transaction (emit-iff-committed,
//! BUS-2) — a crash before commit re-diffs from the un-advanced cursor on retry (0 missed, the
//! deterministic snapshot id no-ops any duplicate, replay §).
//!
//! ## Restriction-safe (GDPR `restrict`, `03 §6`)
//!
//! The emitter SKIPS a restricted subject's blob content (the `restrict` suppression): a restricted
//! path is projected as a **tombstone** (path + oid, no body), never leaking the restricted text.
//! This mirrors the §9 "Restriction-safe: the emitter skips a restricted subject's content".
//!
//! ## ACL scoping is the repo (NOT the blob)
//!
//! The doc's `acl_object_type` is the parent **`repo`** (the spec, GIT-P5) — so Search's
//! `list_objects(viewer, read, repo)` push-down pre-filters per viewer (no leak / no N+1; the
//! `search-requires-acl-filter` lint, GIT-D11). The leak-free SetExpr list push-down + the
//! code-search pre-filter is the **GIT-P26 / P-288** follow-on (it conjoins THIS projection).
//!
//! ## Floors named (VISION §3)
//! - **GF-3** — trigram/lexical code search v1 (symbol/path/literal/trigram-grade) is what this
//!   projection feeds. The **AST-aware "find usages"** via CI-produced SCIP/LSIF indices (contract
//!   6.5, R-3) is the **GIT-P33 / M5** follow-on — git will CONSUME the CI-produced index and project
//!   "find usages"; named, demand-triggered. Not in v1.
//! - **GIT-P26 / P-288** — the leak-free fast repo/PR lists + the code-search pre-filter (the
//!   `list_objects` SetExpr push-down, GIT-D11) that conjoins this projection per viewer.
//! - The production tree-walk + blob read ride the [`crate::gix_backend`] / [`crate::core`] seam; here
//!   the changed-blob SET is the diff over the modeled tree snapshots ([`Tree`]) the receive-pack path
//!   produces (the diff math + the projection shape are the deliverable — the byte plumbing is GIT-P13).
//!
//! ## Coherence (EI-01 §7)
//! The emitter constructs the ONE Search-owned [`myelin_search::SearchProjection`] (NOT a second
//! projection type) and emits the NAMED [`crate::events::GIT_BLOB_SNAPSHOT`] token through the ONE
//! [`myelin_events::OutboxStore`] co-commit (NOT a parallel bus). It REUSES the GIT-P5 spec's facet
//! keys ([`crate::search_projection`]'s `FACET_*`) so the emitted doc's structured facets match the
//! declared schema exactly (a facet drift fails the CDC).

use std::collections::BTreeMap;

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventType, IdMinter,
    OutboxError, OutboxStore, OutboxTx, Visibility,
};
use myelin_query::FieldValue;
use myelin_search::SearchProjection;

use crate::events::GIT_BLOB_SNAPSHOT;
use crate::search_projection::{FACET_BLOB_OID, FACET_LANGUAGE, FACET_PATH};

// ───────────────────────────── the indexed-ref policy ────────────────────────────────────────────

/// Whether a ref is an **indexed ref** — the code projection only fires for the repo's indexed refs
/// (architecture §9: "the default branch + configured refs"), NOT every ref. A push to a throwaway
/// feature branch does not index code (the index tracks the canonical view). v1: the default branch
/// (`refs/heads/main`) is indexed; the configurable additional-ref set is a per-repo policy the live
/// store carries (modeled here as the default-branch rule). Returns `true` iff `ref_name` is indexed.
pub fn is_indexed_ref(ref_name: &str, default_branch: &str) -> bool {
    ref_name == format!("refs/heads/{default_branch}") || ref_name == default_branch
}

// ───────────────────────────── the tree model (the diff source) ──────────────────────────────────

/// A git object id (rendered hex) of a blob — the content-addressed identity the projection pins
/// (`blob_oid`). Equal-oid blobs at equal paths are unchanged (the incremental skip).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobOid(pub String);

impl BlobOid {
    /// Wrap a hex blob oid.
    pub fn new(hex: impl Into<String>) -> BlobOid {
        BlobOid(hex.into())
    }
}

/// One blob in a tree: its content-addressed oid + its raw bytes (the body the projection analyses).
/// In production the bytes are read lazily from the object DB through [`crate::gix_backend`] only for
/// CHANGED paths (never the whole tree); here the modeled [`Tree`] carries them so the diff + project
/// is exercised end-to-end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    /// the content-addressed identity.
    pub oid: BlobOid,
    /// the raw blob bytes (the analysed body).
    pub bytes: Vec<u8>,
}

impl Blob {
    /// A blob from its oid + bytes.
    pub fn new(oid: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Blob {
        Blob {
            oid: BlobOid::new(oid),
            bytes: bytes.into(),
        }
    }
}

/// A repository tree at a commit: `path → Blob`. The diff between two trees is the changed-blob set
/// the emitter projects. A `BTreeMap` so the diff order is deterministic (ascending path) — the
/// emit order is byte-reproducible (cold == live).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    entries: BTreeMap<String, Blob>,
}

impl Tree {
    /// An empty tree (the `last_indexed_oid` of a never-indexed ref — the zero tree; every blob is
    /// then an Upsert, so the FIRST index of a ref projects the whole tree).
    pub fn empty() -> Tree {
        Tree::default()
    }

    /// Put a blob at a path.
    pub fn with(mut self, path: impl Into<String>, blob: Blob) -> Tree {
        self.entries.insert(path.into(), blob);
        self
    }

    /// The number of entries (files) in the tree.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ───────────────────────────── the changed-blob set (the diff) ───────────────────────────────────

/// One changed blob between `last_indexed` and `new_tip` at a path — the unit the emitter projects
/// (exactly one emit per change; the §9 incremental invariant).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobChange {
    /// A blob was added or modified at `path` (oid differs / path is new) → emit a projection doc.
    Upserted {
        /// the path within the repo (the `path` facet + part of the artifact ref).
        path: String,
        /// the new blob (oid + body).
        blob: Blob,
    },
    /// A blob was deleted at `path` (present in `last_indexed`, absent in `new_tip`) → emit a
    /// **tombstone** so Search removes the stale doc (`Gone` is never silently dropped).
    Deleted {
        /// the path the blob was removed from.
        path: String,
        /// the oid the deleted blob had (pins which doc to remove).
        oid: BlobOid,
    },
}

impl BlobChange {
    /// The path this change touches (the projection-doc key half).
    pub fn path(&self) -> &str {
        match self {
            BlobChange::Upserted { path, .. } | BlobChange::Deleted { path, .. } => path,
        }
    }
}

/// **The tree diff (`new_tip ∖ last_indexed` at path granularity).** Returns the changed-blob set —
/// one [`BlobChange`] per changed path, deterministic ascending-path order. An UNCHANGED blob (same
/// oid at the same path) is OMITTED (the incremental skip — what makes a 3-file push of a 10k-file
/// repo emit 3 docs). The set's length is the **changed-blob-count** the GATE asserts the emit count
/// equals.
pub fn diff_trees_bounded(
    last_indexed: &Tree,
    new_tip: &Tree,
    maximum_changes: usize,
    maximum_blob_bytes: usize,
    maximum_total_blob_bytes: usize,
    maximum_path_bytes: usize,
) -> Result<Vec<BlobChange>, String> {
    let mut changes = Vec::new();
    let mut total_blob_bytes = 0usize;
    // Upserts: a path in new_tip that is new OR whose oid changed.
    for (path, new_blob) in &new_tip.entries {
        match last_indexed.entries.get(path) {
            // Unchanged (same oid at the same path) — the incremental skip (no emit).
            Some(old) if old.oid == new_blob.oid => {}
            // Added or modified — one Upsert.
            _ => {
                ensure_projection_change_capacity(
                    &changes,
                    path,
                    maximum_changes,
                    maximum_path_bytes,
                )?;
                if new_blob.bytes.len() > maximum_blob_bytes {
                    return Err("code projection blob limit exceeded".into());
                }
                total_blob_bytes = total_blob_bytes
                    .checked_add(new_blob.bytes.len())
                    .ok_or_else(|| "code projection blob byte count overflowed".to_string())?;
                if total_blob_bytes > maximum_total_blob_bytes {
                    return Err("code projection aggregate blob limit exceeded".into());
                }
                changes.push(BlobChange::Upserted {
                    path: path.clone(),
                    blob: new_blob.clone(),
                });
            }
        }
    }
    // Deletes: a path in last_indexed that is absent in new_tip.
    for (path, old_blob) in &last_indexed.entries {
        if !new_tip.entries.contains_key(path) {
            ensure_projection_change_capacity(
                &changes,
                path,
                maximum_changes,
                maximum_path_bytes,
            )?;
            changes.push(BlobChange::Deleted {
                path: path.clone(),
                oid: old_blob.oid.clone(),
            });
        }
    }
    Ok(changes)
}

fn ensure_projection_change_capacity(
    changes: &[BlobChange],
    path: &str,
    maximum_changes: usize,
    maximum_path_bytes: usize,
) -> Result<(), String> {
    if changes.len() >= maximum_changes {
        return Err("code projection changed-blob limit exceeded".into());
    }
    if path.len() > maximum_path_bytes {
        return Err("code projection path limit exceeded".into());
    }
    Ok(())
}

// ───────────────────────────── language detection (the `language` facet) ─────────────────────────

/// Detect the source language from a path's extension (the `language` facet — the per-language
/// analyzer/facet tag, §9 "language (detected)"). A minimal extension map (the real detector is
/// linguist-grade, SRCH-P12); an unknown extension is `und` (undetermined — the indexer's
/// pass-through analyzer, never a guess). The tag is lowercase-stable so the facet de-dups.
pub fn detect_language(path: &str) -> String {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("rs") => "rust",
        Some("py") => "python",
        Some("js" | "mjs" | "cjs") => "javascript",
        Some("ts") => "typescript",
        Some("go") => "go",
        Some("java") => "java",
        Some("c" | "h") => "c",
        Some("cc" | "cpp" | "cxx" | "hpp") => "cpp",
        Some("rb") => "ruby",
        Some("md" | "markdown") => "markdown",
        Some("toml") => "toml",
        Some("json") => "json",
        Some("yaml" | "yml") => "yaml",
        Some("sh" | "bash") => "shell",
        Some("sql") => "sql",
        _ => "und",
    }
    .to_string()
}

// ───────────────────────────── symbol + literal extraction (the GF-3 body) ───────────────────────

/// **Split an identifier on camelCase / snake_case / kebab boundaries (the §9 symbol split).** Given
/// a source token (`parseHTTPResponse`, `parse_http_response`, `parse-http`) returns the lowercased
/// sub-words PLUS the original token, so a code search for `http` or `response` or the whole symbol
/// all match (the GF-3 "find this identifier across the repo"). Acronym runs (`HTTP`) split as one
/// word; a digit run is its own boundary. Deterministic + de-duplicated.
pub fn split_symbol(token: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // The original token (lowercased) is always a search term (an exact-symbol query matches it).
    let lower = token.to_ascii_lowercase();
    if !lower.is_empty() {
        out.push(lower);
    }
    // Split on snake_case / kebab-case separators first.
    for part in token.split(['_', '-']) {
        if part.is_empty() {
            continue;
        }
        // Then split each part on camelCase / acronym / digit boundaries.
        for word in split_camel(part) {
            let w = word.to_ascii_lowercase();
            if !w.is_empty() {
                out.push(w);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Split one separator-free token on camelCase + acronym + digit boundaries. `parseHTTPResponse` →
/// `["parse", "HTTP", "Response"]`; `v2Index` → `["v", "2", "Index"]`. A boundary is: lower→upper, an
/// acronym→Word (upper-run followed by an upper+lower), or a letter↔digit transition.
fn split_camel(token: &str) -> Vec<String> {
    let chars: Vec<char> = token.chars().collect();
    let mut words = Vec::new();
    let mut start = 0usize;
    for i in 1..chars.len() {
        let prev = chars[i - 1];
        let cur = chars[i];
        let boundary =
            // lower/digit → upper (camel hump): parseR | esponse
            (!prev.is_uppercase() && cur.is_uppercase())
            // acronym run end: HTTPResponse → HTTP | Response (prev upper, cur upper, next lower)
            || (prev.is_uppercase()
                && cur.is_uppercase()
                && chars.get(i + 1).is_some_and(|n| n.is_lowercase()))
            // letter ↔ digit transition (v2 → v | 2)
            || (prev.is_alphabetic() != cur.is_alphabetic());
        if boundary {
            words.push(chars[start..i].iter().collect());
            start = i;
        }
    }
    if start < chars.len() {
        words.push(chars[start..].iter().collect());
    }
    words
}

/// **Extract the symbols of a blob's text (the §9 `symbols` facet).** Tokenizes the text into
/// identifier-like runs (`[A-Za-z_][A-Za-z0-9_]*`) and camel/snake-splits each (via [`split_symbol`]),
/// returning the de-duplicated sorted union — the GF-3 "find this identifier" search terms. Pure +
/// deterministic (cold == live).
pub fn extract_symbols(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in identifier_tokens(text) {
        out.extend(split_symbol(&tok));
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// **Extract the string + number literals of a blob's text (the §9 `literals` facet).** A literal is
/// a `"…"` / `'…'` quoted run (the inner text) or a numeric run (`42`, `3.14`, `0xFF`). De-duplicated
/// + sorted; the "find this literal across the repo" GF-3 search terms. Pure + deterministic.
pub fn extract_literals(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' || c == '\'' {
            // A quoted string literal — capture the inner run up to the matching quote.
            let quote = c;
            let mut j = i + 1;
            let mut buf = String::new();
            while j < chars.len() && chars[j] != quote {
                // Skip a backslash-escape (so `\"` does not terminate the run).
                if chars[j] == '\\' && j + 1 < chars.len() {
                    buf.push(chars[j + 1]);
                    j += 2;
                    continue;
                }
                buf.push(chars[j]);
                j += 1;
            }
            if !buf.is_empty() {
                out.push(buf);
            }
            i = j + 1;
        } else if c.is_ascii_digit() {
            // A numeric literal (incl. `0x..` hex and `.` decimals).
            let mut j = i;
            let mut buf = String::new();
            while j < chars.len()
                && (chars[j].is_ascii_alphanumeric() || chars[j] == '.' || chars[j] == '_')
            {
                buf.push(chars[j]);
                j += 1;
            }
            // Trim a trailing dot (`end.` → `end`) so `1.` does not carry the separator.
            let lit = buf.trim_end_matches('.').to_string();
            if !lit.is_empty() {
                out.push(lit);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Tokenize text into identifier-like runs (`[A-Za-z_][A-Za-z0-9_]*`). A helper for [`extract_symbols`].
fn identifier_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' {
            cur.push(c);
        } else if !cur.is_empty() {
            // An identifier must start with a letter/underscore (a bare number is a literal, not a symbol).
            if cur
                .chars()
                .next()
                .is_some_and(|f| f.is_alphabetic() || f == '_')
            {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
    }
    if !cur.is_empty()
        && cur
            .chars()
            .next()
            .is_some_and(|f| f.is_alphabetic() || f == '_')
    {
        out.push(cur);
    }
    out
}

// ───────────────────────────── the per-blob projection doc (the §9 shape) ────────────────────────

/// **One projection doc per changed blob (the §9 shape).** The full code-projection record the
/// emitter builds for an UPSERTED blob: the artifact ref + path + detected language + camel/snake-split
/// symbols + literals + the blob text + the tip commit message + the blob oid. This is git's OWNED
/// shape; it lowers to the Search-owned [`SearchProjection`] via [`into_search_projection`] (the ONE
/// shape both the live emit and the cold replay produce).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobProjection {
    /// `myelin://<tenant>/git/blob/<repo>:<ref>:<path>` — the stable per-blob artifact ref.
    pub artifact_ref: ArtifactRef,
    /// the path within the repo.
    pub path: String,
    /// the detected source language (the `language` facet).
    pub language: String,
    /// the camel/snake-split identifiers (the GF-3 symbol search terms).
    pub symbols: Vec<String>,
    /// the string + number literals.
    pub literals: Vec<String>,
    /// the raw blob text (Search builds the trigrams over this).
    pub text: String,
    /// the tip commit message (so a commit-message search hits the touched blobs).
    pub commit_message: String,
    /// the content-addressed blob oid (the `blob_oid` facet — the index doc identity / de-dup pin).
    pub blob_oid: BlobOid,
}

impl BlobProjection {
    /// **Lower to the Search-owned [`SearchProjection`] (the ONE indexable shape).** The structured
    /// facets are the GIT-P5 spec's declared three (`path` / `language` / `blob_oid`, all `Text`); the
    /// full-text body (`text`) is the concatenation Search analyses for trigrams + symbols + literals +
    /// the commit message (the §9 "we supply the text"). The `lang` tag pins the analyzer.
    ///
    /// The body is `<symbols> <literals> <commit_message> <text>` — a single analysable run so a query
    /// for an identifier, a literal, a commit-message word, or any token in the file all hit. (Search
    /// builds the inverted/trigram index over this; git does not pre-tokenize the index — §9.)
    pub fn into_search_projection(self) -> SearchProjection {
        let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
        fields.insert(FACET_PATH.to_string(), FieldValue::Text(self.path.clone()));
        fields.insert(
            FACET_LANGUAGE.to_string(),
            FieldValue::Text(self.language.clone()),
        );
        fields.insert(
            FACET_BLOB_OID.to_string(),
            FieldValue::Text(self.blob_oid.0.clone()),
        );
        // The analysable body: symbols + literals + commit message + the raw text (one run; Search
        // tokenizes — git supplies, it does not index).
        let body = format!(
            "{} {} {} {}",
            self.symbols.join(" "),
            self.literals.join(" "),
            self.commit_message,
            self.text,
        );
        SearchProjection {
            text: body,
            fields,
            lang: Some(self.language),
        }
    }
}

// ───────────────────────────── the code-projection cursor (the incremental fence) ────────────────

/// **The `code_projection_cursor` — the `last_indexed_oid` per `(repo, ref)`** (§9). The emitter
/// diffs `new_tip ∖ last_indexed` and, on a committed emit, advances the cursor to the new tip. A
/// never-indexed ref has NO cursor → the first index diffs against the empty tree (the whole tree is
/// projected once). Modeled in-memory here (the live store persists it in the `git_ref` row alongside
/// `target_oid`); the SHAPE — a per-ref tip the next diff reads — does not change.
#[derive(Debug, Default)]
pub struct CodeProjectionCursor {
    /// `(repo, ref) → last_indexed tip oid`. The tip the NEXT push diffs against.
    last_indexed: std::sync::Mutex<BTreeMap<(String, String), String>>,
}

impl CodeProjectionCursor {
    /// A fresh cursor (every ref un-indexed).
    pub fn new() -> CodeProjectionCursor {
        CodeProjectionCursor::default()
    }

    /// The `last_indexed_oid` for `(repo, ref)` — `None` if the ref was never indexed.
    pub fn last_indexed(&self, repo: &str, ref_name: &str) -> Option<String> {
        self.last_indexed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(repo.to_string(), ref_name.to_string()))
            .cloned()
    }

    /// Advance the cursor to `new_tip` for `(repo, ref)` (done ONLY after the emits commit —
    /// emit-iff-committed).
    fn advance(&self, repo: &str, ref_name: &str, new_tip: &str) {
        self.last_indexed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                (repo.to_string(), ref_name.to_string()),
                new_tip.to_string(),
            );
    }
}

// ───────────────────────────── the restriction port (GDPR `restrict`, §6) ────────────────────────

/// **The `restrict`-suppression port (`03 §6`).** The emitter asks this whether a `(repo, path)`'s
/// subject is restricted; a restricted path is projected as a **tombstone** (path + oid, no body), so
/// the restricted text never enters the index (§9 "the emitter skips a restricted subject's
/// content"). The live binding reads the GDPR `restrict` flag; here it is a seam a test drives.
/// `Send + Sync` so the emitter holds it behind a reference across serving threads.
pub trait RestrictionPolicy: Send + Sync {
    /// Whether the blob at `(repo, path)` is restricted (its body must be suppressed from the index).
    fn is_restricted(&self, repo: &str, path: &str) -> bool;
}

/// The default policy: nothing is restricted (the common case — a repo with no active `restrict`
/// DSR). The live GDPR-backed policy swaps in behind [`RestrictionPolicy`].
#[derive(Clone, Copy, Debug, Default)]
pub struct NoRestrictions;

impl RestrictionPolicy for NoRestrictions {
    fn is_restricted(&self, _repo: &str, _path: &str) -> bool {
        false
    }
}

// ───────────────────────────── the emitter (the receive-pack post-commit hook) ───────────────────

/// The outcome of an [`CodeProjectionEmitter::emit_for_push`]: the projection docs emitted (the
/// committed event ids) + the changed-blob count, so the GATE asserts `emitted.len() ==
/// changed_blob_count` (per-blob, incremental — 0 missed / 0 stale).
#[derive(Clone, Debug)]
pub struct ProjectionEmit {
    /// the committed `git.blob.snapshot` event ids (one per changed blob — upsert OR delete tombstone).
    pub emitted: Vec<myelin_events::EventId>,
    /// the number of changed blobs the diff produced (the GATE's expected emit count).
    pub changed_blob_count: usize,
}

const PROJECTION_MAX_CHANGED_BLOBS: usize = 1_000;
const PROJECTION_MAX_BLOB_BYTES: usize = 1024 * 1024;
const PROJECTION_MAX_TOTAL_BLOB_BYTES: usize = 64 * 1024 * 1024;
const PROJECTION_MAX_PATH_BYTES: usize = 4 * 1024;
const PROJECTION_MAX_COMMIT_MESSAGE_BYTES: usize = 8 * 1024;

/// **The code-projection emitter (GIT-P25 / P-287, §9).** Hooks the receive-pack post-commit path:
/// on a `git.ref.updated` to an indexed ref it diffs `new_tip ∖ last_indexed`, builds the per-blob
/// projection doc, and emits one `git.blob.snapshot` per changed blob through the outbox — then
/// advances the cursor. Holds the shared outbox + minter (the frozen substrate co-commit, reused),
/// the repo locator, and the cursor.
pub struct CodeProjectionEmitter<'a, R: RestrictionPolicy> {
    repo: String,
    default_branch: String,
    ctx_base: EmitContextBase,
    outbox: &'a OutboxStore,
    minter: std::sync::Arc<dyn IdMinter>,
    cursor: &'a CodeProjectionCursor,
    restriction: &'a R,
}

impl<'a, R: RestrictionPolicy> CodeProjectionEmitter<'a, R> {
    /// Build an emitter for a repo (the default branch is the indexed ref; the cursor + restriction
    /// port are shared seams the live store owns).
    pub fn new(
        repo: impl Into<String>,
        default_branch: impl Into<String>,
        ctx_base: EmitContextBase,
        outbox: &'a OutboxStore,
        minter: std::sync::Arc<dyn IdMinter>,
        cursor: &'a CodeProjectionCursor,
        restriction: &'a R,
    ) -> Self {
        Self {
            repo: repo.into(),
            default_branch: default_branch.into(),
            ctx_base,
            outbox,
            minter,
            cursor,
            restriction,
        }
    }

    /// The per-blob artifact ref `myelin://<tenant>/git/blob/<repo>:<ref>:<path>` (§9 / contract 5.1).
    fn blob_ref(&self, ref_name: &str, path: &str) -> ArtifactRef {
        ArtifactRef(format!(
            "myelin://{}/git/blob/{}:{}:{}",
            self.ctx_base.tenant.0, self.repo, ref_name, path
        ))
    }

    /// The per-ref aggregate for the projection emits — the SAME per-ref key the `git.ref.updated`
    /// event uses (`<repo>:<ref>`), so a blob projection is ordered behind the ref move that produced
    /// it (per-aggregate ordering, contract 2.3). One push's blob docs share the ref aggregate.
    fn aggregate(&self, ref_name: &str) -> AggregateKey {
        AggregateKey(format!("{}:{}", self.repo, ref_name))
    }

    /// Build the full [`BlobProjection`] for an upserted blob (the §9 per-blob doc). A restricted path
    /// suppresses the body (symbols/literals/text empty) — the tombstone-on-restrict (§6).
    fn project_upsert(
        &self,
        ref_name: &str,
        path: &str,
        blob: &Blob,
        commit_message: &str,
    ) -> BlobProjection {
        let restricted = self.restriction.is_restricted(&self.repo, path);
        let text = if restricted {
            String::new()
        } else {
            String::from_utf8_lossy(&blob.bytes).into_owned()
        };
        BlobProjection {
            artifact_ref: self.blob_ref(ref_name, path),
            path: path.to_string(),
            language: detect_language(path),
            symbols: if restricted {
                Vec::new()
            } else {
                extract_symbols(&text)
            },
            literals: if restricted {
                Vec::new()
            } else {
                extract_literals(&text)
            },
            text,
            // A restricted blob still carries its path/oid (so the doc identity is stable for removal),
            // but never the commit message body.
            commit_message: if restricted {
                String::new()
            } else {
                commit_message.to_string()
            },
            blob_oid: blob.oid.clone(),
        }
    }

    /// **Emit the code projection for a push to an indexed ref (the §9 algorithm, the GATE).** Diffs
    /// `new_tip ∖ last_indexed` (the cursor), emits ONE `git.blob.snapshot` per changed blob through
    /// the outbox in one transaction, commits, then advances the cursor — emit-iff-committed.
    ///
    /// Returns `Ok(None)` if `ref_name` is NOT an indexed ref (no projection — a feature-branch push
    /// does not index). Otherwise `Ok(Some(ProjectionEmit))` with the committed ids + the changed-blob
    /// count (the GATE asserts `emitted.len() == changed_blob_count`).
    ///
    /// `commit_message` is the tip commit's message (carried onto every upserted blob's doc, §9).
    pub fn emit_for_push(
        &self,
        ref_name: &str,
        new_tip_oid: &str,
        last_indexed_tree: &Tree,
        new_tip_tree: &Tree,
        commit_message: &str,
    ) -> Result<Option<ProjectionEmit>, OutboxError> {
        // Only indexed refs index code (§9: default branch + configured refs).
        if !is_indexed_ref(ref_name, &self.default_branch) {
            return Ok(None);
        }
        if commit_message.len() > PROJECTION_MAX_COMMIT_MESSAGE_BYTES {
            return Err(OutboxError(
                "code projection commit message limit exceeded".into(),
            ));
        }

        // The diff — the changed-blob set (new ∖ last, path granularity). UNCHANGED blobs are omitted
        // (the incremental skip). Its length is the changed-blob-count the GATE pins the emit to.
        let changes = diff_trees_bounded(
            last_indexed_tree,
            new_tip_tree,
            PROJECTION_MAX_CHANGED_BLOBS,
            PROJECTION_MAX_BLOB_BYTES,
            PROJECTION_MAX_TOTAL_BLOB_BYTES,
            PROJECTION_MAX_PATH_BYTES,
        )
        .map_err(OutboxError)?;
        let changed_blob_count = changes.len();

        // Stage one git.blob.snapshot per change in ONE outbox transaction (co-commit — the emits are
        // durable iff the transaction commits; the cursor advance is gated on the same commit).
        let mut tx = self
            .outbox
            .begin(std::sync::Arc::clone(&self.minter), self.ctx_base.clone());
        // The cursor advance is the state change the transaction carries (in the live store this is the
        // `UPDATE git_ref SET code_projection_cursor = new_tip` row alongside the outbox inserts).
        tx.stage_state_change(format!(
            "code_projection_cursor {}:{} -> {new_tip_oid}",
            self.repo, ref_name
        ));

        let mut emitted = Vec::new();
        for change in &changes {
            let payload = match change {
                BlobChange::Upserted { path, blob } => {
                    let proj = self.project_upsert(ref_name, path, blob, commit_message);
                    // references-not-payloads: the doc carries the blob ref + path + facets + the
                    // (restriction-suppressed) text. The body text is repo content under the processor
                    // posture, NOT inline subject PII — it is the indexable code (§9).
                    serde_json::json!({
                        "op": "upsert",
                        "artifact_ref": proj.artifact_ref.0,
                        "path": proj.path,
                        "language": proj.language,
                        "symbols": proj.symbols,
                        "literals": proj.literals,
                        "text": proj.text,
                        "commit_message": proj.commit_message,
                        "blob_oid": proj.blob_oid.0,
                        "acl_object_type": crate::search_projection::GIT_BLOB_ACL_OBJECT_TYPE,
                    })
                }
                BlobChange::Deleted { path, oid } => {
                    // A delete tombstone: Search removes the stale doc (Gone is never silently dropped).
                    serde_json::json!({
                        "op": "delete",
                        "artifact_ref": self.blob_ref(ref_name, path).0,
                        "path": path,
                        "blob_oid": oid.0,
                        "acl_object_type": crate::search_projection::GIT_BLOB_ACL_OBJECT_TYPE,
                    })
                }
            };
            let draft = EventDraft {
                type_: EventType(GIT_BLOB_SNAPSHOT.into()),
                subject: self.blob_ref(ref_name, change.path()),
                aggregate: self.aggregate(ref_name),
                payload,
                // Repo content — the tenant org is the controller (processor posture, Art. 28 / §4.3).
                data_role: DataRole::Processor,
                visibility: Visibility::Internal,
                // The code projection carries no inline subject PII (the pusher pseudonym is not in the
                // blob doc; commit-identity PII lives behind the pseudonym indirection, never here).
                contains_personal_data: false,
                pii_key_ref: None,
            };
            // A root projection emit (its causal root is the push; a real wiring threads the
            // git.ref.updated envelope as the cause). One id per changed blob — the GATE's count.
            let id = tx.emit(draft, None)?;
            emitted.push(id);
        }

        // Commit: the projection docs become durable atomically. ONLY now advance the cursor (so a
        // crash before commit re-diffs from the un-advanced cursor — 0 missed; the deterministic id
        // no-ops a duplicate on replay).
        tx.commit()?;
        self.cursor.advance(&self.repo, ref_name, new_tip_oid);

        Ok(Some(ProjectionEmit {
            emitted,
            changed_blob_count,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{Actor, MonotonicMinter, Region, TenantId, Timestamp};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-22T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-22T00:00:01Z".into()),
            caused_by: None,
        }
    }

    fn emitter<'a, R: RestrictionPolicy>(
        outbox: &'a OutboxStore,
        cursor: &'a CodeProjectionCursor,
        restriction: &'a R,
    ) -> CodeProjectionEmitter<'a, R> {
        CodeProjectionEmitter::new(
            "core",
            "main",
            ctx_base(),
            outbox,
            std::sync::Arc::new(MonotonicMinter::new()),
            cursor,
            restriction,
        )
    }

    // ── the symbol/literal/language unit tests ──

    #[test]
    fn split_symbol_handles_camel_snake_kebab_and_acronyms() {
        assert_eq!(
            split_symbol("parse_http_response"),
            vec!["http", "parse", "parse_http_response", "response"]
        );
        // camelCase + acronym run: parseHTTPResponse → parse / http / response (+ the whole token)
        let s = split_symbol("parseHTTPResponse");
        assert!(s.contains(&"parse".to_string()));
        assert!(s.contains(&"http".to_string()));
        assert!(s.contains(&"response".to_string()));
        assert!(
            s.contains(&"parsehttpresponse".to_string()),
            "the whole token is searchable too"
        );
        // kebab + digit boundary
        let k = split_symbol("api-v2");
        assert!(k.contains(&"api".to_string()));
        assert!(k.contains(&"v".to_string()));
        assert!(k.contains(&"2".to_string()));
    }

    #[test]
    fn extract_symbols_splits_identifiers_only_not_numbers() {
        let text = "fn parseHttp() { let maxRetries = 42; }";
        let syms = extract_symbols(text);
        assert!(syms.contains(&"parse".to_string()));
        assert!(syms.contains(&"http".to_string()));
        assert!(syms.contains(&"max".to_string()));
        assert!(syms.contains(&"retries".to_string()));
        assert!(syms.contains(&"fn".to_string()));
        // 42 is a literal, NOT a symbol.
        assert!(!syms.contains(&"42".to_string()));
    }

    #[test]
    fn extract_literals_finds_strings_and_numbers() {
        let text = r#"let url = "https://example.test"; let n = 42; let pi = 3.14;"#;
        let lits = extract_literals(text);
        assert!(
            lits.contains(&"https://example.test".to_string()),
            "{lits:?}"
        );
        assert!(lits.contains(&"42".to_string()));
        assert!(lits.contains(&"3.14".to_string()));
    }

    /// Pin the literal-extraction EDGE cases (the boundary math in the scanner): a backslash escape
    /// inside a string, single-quoted strings, hex + underscore-grouped numerics, and a trailing-dot
    /// trim. These kill the off-by-one / operator mutants on the inner scan loops.
    #[test]
    fn extract_literals_handles_escapes_hex_and_trailing_dot() {
        // A backslash-escaped quote does NOT terminate the string; the inner char is captured.
        let escaped = extract_literals(r#""a\"b""#);
        assert!(
            escaped.contains(&"a\"b".to_string()),
            "escape handling: {escaped:?}"
        );
        // Single-quoted string.
        let sq = extract_literals("x = 'hello'");
        assert!(sq.contains(&"hello".to_string()), "{sq:?}");
        // Hex + underscore-grouped numeric literals.
        let nums = extract_literals("a = 0xFF; b = 1_000;");
        assert!(nums.contains(&"0xFF".to_string()), "hex literal: {nums:?}");
        assert!(
            nums.contains(&"1_000".to_string()),
            "underscore-grouped: {nums:?}"
        );
        // A trailing dot is trimmed (`5.` → `5`, the `.` was a statement separator).
        let td = extract_literals("n = 5. end");
        assert!(
            td.contains(&"5".to_string()),
            "trailing dot trimmed: {td:?}"
        );
        assert!(
            !td.iter().any(|l| l == "5."),
            "the trailing-dot form is not emitted: {td:?}"
        );
        // An empty string literal `""` emits nothing.
        assert!(
            extract_literals(r#"x = """#).is_empty(),
            "an empty string literal emits no literal"
        );
    }

    /// Identifier tokenization: a bare number is NOT a symbol; an identifier may contain digits +
    /// underscores after its first letter. Kills the boundary mutants in `identifier_tokens`.
    #[test]
    fn identifier_tokens_require_a_leading_letter_or_underscore() {
        // `_private` and `var2` ARE identifiers; `42` is NOT.
        let syms = extract_symbols("let _private = var2 + 42;");
        assert!(
            syms.iter().any(|s| s == "_private" || s == "private"),
            "{syms:?}"
        );
        assert!(syms.contains(&"var".to_string()), "{syms:?}");
        // 42 is a number → not a symbol.
        assert!(
            !syms.contains(&"42".to_string()),
            "a bare number is a literal, not a symbol: {syms:?}"
        );
        // A trailing identifier with no following separator is still captured.
        let trailing = extract_symbols("call doThing");
        assert!(
            trailing.contains(&"do".to_string()) && trailing.contains(&"thing".to_string()),
            "{trailing:?}"
        );
    }

    /// The underscore is part of an identifier (NOT a separator at the tokenizer level — the
    /// camel/snake split happens later in `split_symbol`). A mid-token `_` keeps the whole token
    /// searchable; the WHOLE token survives in the symbol set. Kills the `c == '_'` boundary mutant.
    #[test]
    fn underscore_is_part_of_the_identifier_token() {
        // `parse_config` is ONE identifier token → its whole-token form is a symbol (plus the split).
        let syms = extract_symbols("fn parse_config");
        assert!(
            syms.contains(&"parse_config".to_string()),
            "the whole snake token is searchable: {syms:?}"
        );
        assert!(syms.contains(&"parse".to_string()) && syms.contains(&"config".to_string()));
        // A snake identifier at END-OF-TEXT (no trailing separator) still flushes WITH its underscore.
        let tail = extract_symbols("see also_this");
        assert!(
            tail.contains(&"also_this".to_string()),
            "the trailing snake token flushes whole: {tail:?}"
        );
    }

    #[test]
    fn detect_language_maps_extensions() {
        assert_eq!(detect_language("src/main.rs"), "rust");
        assert_eq!(detect_language("a/b/x.py"), "python");
        assert_eq!(detect_language("README.md"), "markdown");
        assert_eq!(detect_language("noext"), "und");
    }

    /// Pin EVERY language arm distinctly (a dropped/merged arm changes one of these tags). This kills
    /// the "delete match arm" mutants on the less-common extensions.
    #[test]
    fn detect_language_pins_every_arm() {
        for (path, want) in [
            ("x.rs", "rust"),
            ("x.py", "python"),
            ("x.js", "javascript"),
            ("x.mjs", "javascript"),
            ("x.ts", "typescript"),
            ("x.go", "go"),
            ("x.java", "java"),
            ("x.c", "c"),
            ("x.h", "c"),
            ("x.cpp", "cpp"),
            ("x.hpp", "cpp"),
            ("x.rb", "ruby"),
            ("x.md", "markdown"),
            ("x.markdown", "markdown"),
            ("x.toml", "toml"),
            ("x.json", "json"),
            ("x.yaml", "yaml"),
            ("x.yml", "yaml"),
            ("x.sh", "shell"),
            ("x.bash", "shell"),
            ("x.sql", "sql"),
            ("x.unknownext", "und"),
            ("noextension", "und"),
        ] {
            assert_eq!(detect_language(path), want, "language for `{path}`");
        }
        // The detection is case-insensitive on the extension.
        assert_eq!(detect_language("X.RS"), "rust");
    }

    /// `split_camel`'s last word (the run after the final boundary) must be included — kills the
    /// off-by-one mutants on the trailing-run bound.
    #[test]
    fn split_camel_includes_the_trailing_run() {
        // parseHTTPResponse → parse / HTTP / Response — the final "Response" must be present.
        let s = split_symbol("parseHTTPResponse");
        assert!(
            s.contains(&"response".to_string()),
            "the trailing camel run is included: {s:?}"
        );
        // A single-word token: the whole token + the one word.
        assert_eq!(split_symbol("hello"), vec!["hello"]);
        // A token ending in a digit run: foo2 → foo / 2 (both, incl. the trailing digit run).
        let d = split_symbol("foo2");
        assert!(
            d.contains(&"foo".to_string()) && d.contains(&"2".to_string()),
            "{d:?}"
        );
    }

    // ── the diff unit tests (the incremental invariant) ──

    fn diff_for_test(old: &Tree, new: &Tree) -> Vec<BlobChange> {
        diff_trees_bounded(old, new, 100, 1024, 4096, 256).expect("small test diff")
    }

    #[test]
    fn diff_emits_only_changed_blobs_not_the_whole_tree() {
        let old = Tree::empty()
            .with("a.rs", Blob::new("oid-a1", b"fn a() {}".to_vec()))
            .with("b.rs", Blob::new("oid-b1", b"fn b() {}".to_vec()))
            .with("c.rs", Blob::new("oid-c1", b"fn c() {}".to_vec()));
        // new tip: a unchanged, b modified, c deleted, d added.
        let new = Tree::empty()
            .with("a.rs", Blob::new("oid-a1", b"fn a() {}".to_vec())) // unchanged
            .with("b.rs", Blob::new("oid-b2", b"fn b2() {}".to_vec())) // modified
            .with("d.rs", Blob::new("oid-d1", b"fn d() {}".to_vec())); // added
        let changes = diff_for_test(&old, &new);
        // 3 changes: b modified, d added, c deleted. NOT a (unchanged → no emit).
        assert_eq!(changes.len(), 3, "{changes:?}");
        let paths: Vec<&str> = changes.iter().map(|c| c.path()).collect();
        assert!(paths.contains(&"b.rs"));
        assert!(paths.contains(&"d.rs"));
        assert!(paths.contains(&"c.rs"));
        assert!(
            !paths.contains(&"a.rs"),
            "an unchanged blob emits nothing (incremental)"
        );
        // c is a delete; b and d are upserts.
        assert!(changes
            .iter()
            .any(|c| matches!(c, BlobChange::Deleted { path, .. } if path == "c.rs")));
    }

    #[test]
    fn first_index_of_a_ref_projects_the_whole_tree() {
        let new = Tree::empty()
            .with("a.rs", Blob::new("oid-a", b"fn a() {}".to_vec()))
            .with("b.rs", Blob::new("oid-b", b"fn b() {}".to_vec()));
        let changes = diff_for_test(&Tree::empty(), &new);
        assert_eq!(
            changes.len(),
            2,
            "the first index of a ref projects every blob once"
        );
    }

    #[test]
    fn tree_diff_enforces_every_projection_materialization_limit() {
        let old = Tree::empty();
        let new = Tree::empty()
            .with("a.rs", Blob::new("a", vec![1; 4]))
            .with("b.rs", Blob::new("b", vec![2; 4]));
        assert_eq!(
            diff_trees_bounded(&old, &new, 2, 4, 8, 4)
                .expect("exact limits accepted")
                .len(),
            2
        );
        assert!(diff_trees_bounded(&old, &new, 1, 4, 8, 4).is_err());
        assert!(diff_trees_bounded(&old, &new, 2, 3, 8, 4).is_err());
        assert!(diff_trees_bounded(&old, &new, 2, 4, 7, 4).is_err());
        assert!(diff_trees_bounded(&old, &new, 2, 4, 8, 3).is_err());
    }

    #[test]
    fn emitter_rejects_oversized_projection_input_before_staging() {
        let outbox = OutboxStore::new();
        let cursor = CodeProjectionCursor::new();
        let r = NoRestrictions;
        let e = emitter(&outbox, &cursor, &r);
        let oversized_blob = Tree::empty().with(
            "large.rs",
            Blob::new("large", vec![b'x'; PROJECTION_MAX_BLOB_BYTES + 1]),
        );

        assert!(
            e.emit_for_push(
                "refs/heads/main",
                "blob-tip",
                &Tree::empty(),
                &oversized_blob,
                "small",
            )
            .is_err()
        );
        assert!(
            e.emit_for_push(
                "refs/heads/main",
                "message-tip",
                &Tree::empty(),
                &Tree::empty(),
                &"x".repeat(PROJECTION_MAX_COMMIT_MESSAGE_BYTES + 1),
            )
            .is_err()
        );
        assert_eq!(outbox.committed_count(), 0);
        assert!(cursor.last_indexed("core", "refs/heads/main").is_none());
    }

    // ── the GATE: emit-count == changed-blob-count, incremental ──

    #[test]
    fn emit_count_equals_changed_blob_count_incremental() {
        let outbox = OutboxStore::new();
        let cursor = CodeProjectionCursor::new();
        let r = NoRestrictions;
        let e = emitter(&outbox, &cursor, &r);

        // Push 1: first index of main — 2 files → 2 emits.
        let t1 = Tree::empty()
            .with(
                "src/lib.rs",
                Blob::new("o1", b"pub fn helloWorld() {}".to_vec()),
            )
            .with("README.md", Blob::new("o2", b"# project".to_vec()));
        let p1 = e
            .emit_for_push(
                "refs/heads/main",
                "tip1",
                &Tree::empty(),
                &t1,
                "initial commit",
            )
            .unwrap()
            .expect("indexed ref emits");
        assert_eq!(p1.changed_blob_count, 2);
        assert_eq!(
            p1.emitted.len(),
            2,
            "emit-count == changed-blob-count (the GATE)"
        );
        assert_eq!(outbox.committed_count(), 2);
        assert_eq!(
            cursor.last_indexed("core", "refs/heads/main").as_deref(),
            Some("tip1")
        );

        // Push 2: modify ONE file, add ONE — 2 changed of 2 files in the tree. README unchanged.
        let t2 = t1
            .clone()
            .with(
                "src/lib.rs",
                Blob::new("o1b", b"pub fn helloWorld() { ok() }".to_vec()),
            ) // modified
            .with("src/new.rs", Blob::new("o3", b"fn n() {}".to_vec())); // added
        let p2 = e
            .emit_for_push("refs/heads/main", "tip2", &t1, &t2, "second commit")
            .unwrap()
            .unwrap();
        assert_eq!(
            p2.changed_blob_count, 2,
            "2 changed (1 modified + 1 added); README unchanged"
        );
        assert_eq!(
            p2.emitted.len(),
            2,
            "incremental: exactly 2 emits, NOT the whole 3-file tree"
        );
        // Total committed: 2 + 2.
        assert_eq!(outbox.committed_count(), 4);
        assert_eq!(
            cursor.last_indexed("core", "refs/heads/main").as_deref(),
            Some("tip2")
        );
    }

    #[test]
    fn a_push_with_no_changed_blobs_emits_nothing() {
        let outbox = OutboxStore::new();
        let cursor = CodeProjectionCursor::new();
        let r = NoRestrictions;
        let e = emitter(&outbox, &cursor, &r);
        let t = Tree::empty().with("a.rs", Blob::new("o", b"fn a(){}".to_vec()));
        // The same tree on both sides (e.g. a merge that changed no blobs on this ref).
        let p = e
            .emit_for_push("refs/heads/main", "tip", &t, &t, "noop")
            .unwrap()
            .unwrap();
        assert_eq!(p.changed_blob_count, 0);
        assert_eq!(p.emitted.len(), 0, "0 changed blobs → 0 emits");
        assert_eq!(outbox.committed_count(), 0);
    }

    #[test]
    fn a_non_indexed_ref_emits_no_projection() {
        let outbox = OutboxStore::new();
        let cursor = CodeProjectionCursor::new();
        let r = NoRestrictions;
        let e = emitter(&outbox, &cursor, &r);
        let t = Tree::empty().with("a.rs", Blob::new("o", b"fn a(){}".to_vec()));
        let out = e
            .emit_for_push("refs/heads/feature", "tip", &Tree::empty(), &t, "wip")
            .unwrap();
        assert!(out.is_none(), "a feature-branch push does not index code");
        assert_eq!(outbox.committed_count(), 0);
        assert!(cursor.last_indexed("core", "refs/heads/feature").is_none());
    }

    #[test]
    fn the_emitted_doc_carries_the_full_6_3_shape() {
        let outbox = OutboxStore::new();
        let cursor = CodeProjectionCursor::new();
        let r = NoRestrictions;
        let e = emitter(&outbox, &cursor, &r);
        let t = Tree::empty().with(
            "src/main.rs",
            Blob::new(
                "blob-oid-1",
                b"fn parseHttp() { let url = \"http://x\"; }".to_vec(),
            ),
        );
        let p = e
            .emit_for_push("refs/heads/main", "tip", &Tree::empty(), &t, "add parser")
            .unwrap()
            .unwrap();
        let row = outbox.row(&p.emitted[0]).unwrap();
        assert_eq!(row.envelope.type_.0, GIT_BLOB_SNAPSHOT);
        let pl = &row.envelope.payload;
        assert_eq!(pl["op"], serde_json::json!("upsert"));
        assert_eq!(
            pl["artifact_ref"],
            serde_json::json!("myelin://acme/git/blob/core:refs/heads/main:src/main.rs")
        );
        assert_eq!(pl["path"], serde_json::json!("src/main.rs"));
        assert_eq!(pl["language"], serde_json::json!("rust"));
        assert_eq!(pl["blob_oid"], serde_json::json!("blob-oid-1"));
        assert_eq!(pl["commit_message"], serde_json::json!("add parser"));
        assert_eq!(
            pl["acl_object_type"],
            serde_json::json!("repo"),
            "ACL keys on the parent repo"
        );
        // symbols carry the camel-split identifiers; literals carry the string.
        let syms = pl["symbols"].as_array().unwrap();
        assert!(syms.iter().any(|s| s == "parse"));
        assert!(syms.iter().any(|s| s == "http"));
        let lits = pl["literals"].as_array().unwrap();
        assert!(lits.iter().any(|l| l == "http://x"));
        // the per-ref aggregate is shared with git.ref.updated (<repo>:<ref>).
        assert_eq!(row.aggregate, AggregateKey("core:refs/heads/main".into()));
    }

    #[test]
    fn into_search_projection_uses_the_spec_facets() {
        let bp = BlobProjection {
            artifact_ref: ArtifactRef("myelin://acme/git/blob/core:refs/heads/main:a.rs".into()),
            path: "a.rs".into(),
            language: "rust".into(),
            symbols: vec!["parse".into(), "http".into()],
            literals: vec!["lit".into()],
            text: "fn parse() {}".into(),
            commit_message: "msg".into(),
            blob_oid: BlobOid::new("oid-1"),
        };
        let sp = bp.into_search_projection();
        // The structured facets are exactly the GIT-P5 spec's three (path / language / blob_oid).
        assert_eq!(
            sp.fields.get(FACET_PATH),
            Some(&FieldValue::Text("a.rs".into()))
        );
        assert_eq!(
            sp.fields.get(FACET_LANGUAGE),
            Some(&FieldValue::Text("rust".into()))
        );
        assert_eq!(
            sp.fields.get(FACET_BLOB_OID),
            Some(&FieldValue::Text("oid-1".into()))
        );
        assert_eq!(sp.fields.len(), 3, "exactly the three declared facets");
        assert_eq!(sp.lang.as_deref(), Some("rust"));
        // The full-text body carries the symbols + literals + commit message + text (one analysable run).
        assert!(sp.text.contains("parse"));
        assert!(sp.text.contains("lit"));
        assert!(sp.text.contains("msg"));
        assert!(sp.text.contains("fn parse() {}"));
    }

    // ── restriction-safe (the §6 suppression) ──

    struct RestrictPath(&'static str);
    impl RestrictionPolicy for RestrictPath {
        fn is_restricted(&self, _repo: &str, path: &str) -> bool {
            path == self.0
        }
    }

    #[test]
    fn a_restricted_blob_is_projected_without_its_body() {
        let outbox = OutboxStore::new();
        let cursor = CodeProjectionCursor::new();
        let r = RestrictPath("secret.rs");
        let e = emitter(&outbox, &cursor, &r);
        let t = Tree::empty()
            .with(
                "secret.rs",
                Blob::new("os", b"const KEY = \"top-secret-value\";".to_vec()),
            )
            .with("ok.rs", Blob::new("ok", b"fn ok() {}".to_vec()));
        let p = e
            .emit_for_push("refs/heads/main", "tip", &Tree::empty(), &t, "msg")
            .unwrap()
            .unwrap();
        // Both blobs still emit a doc (the path/oid identity is indexed), but the restricted one
        // carries NO body — the secret text never enters the index.
        assert_eq!(p.emitted.len(), 2);
        let mut secret_doc = None;
        for id in &p.emitted {
            let row = outbox.row(id).unwrap();
            if row.envelope.payload["path"] == serde_json::json!("secret.rs") {
                secret_doc = Some(row.envelope.payload.clone());
            }
        }
        let sd = secret_doc.expect("the restricted doc was emitted");
        assert_eq!(
            sd["text"],
            serde_json::json!(""),
            "the restricted body is suppressed"
        );
        assert_eq!(
            sd["symbols"],
            serde_json::json!([]),
            "no symbols leak from a restricted blob"
        );
        assert_eq!(
            sd["literals"],
            serde_json::json!([]),
            "the secret literal never enters the index"
        );
        // But the path/language/oid (the non-content facets) are still present (the doc identity).
        assert_eq!(sd["path"], serde_json::json!("secret.rs"));
        assert_eq!(sd["blob_oid"], serde_json::json!("os"));
    }

    // ── the deleted-blob tombstone ──

    #[test]
    fn a_deleted_blob_emits_a_delete_tombstone() {
        let outbox = OutboxStore::new();
        let cursor = CodeProjectionCursor::new();
        let r = NoRestrictions;
        let e = emitter(&outbox, &cursor, &r);
        let t1 = Tree::empty().with("gone.rs", Blob::new("g1", b"fn gone() {}".to_vec()));
        e.emit_for_push("refs/heads/main", "t1", &Tree::empty(), &t1, "add")
            .unwrap();
        // Push 2: delete the file.
        let p = e
            .emit_for_push("refs/heads/main", "t2", &t1, &Tree::empty(), "rm")
            .unwrap()
            .unwrap();
        assert_eq!(p.emitted.len(), 1);
        let row = outbox.row(&p.emitted[0]).unwrap();
        assert_eq!(
            row.envelope.payload["op"],
            serde_json::json!("delete"),
            "Gone is a tombstone, not a silent drop"
        );
        assert_eq!(row.envelope.payload["path"], serde_json::json!("gone.rs"));
        assert_eq!(row.envelope.payload["blob_oid"], serde_json::json!("g1"));
    }
}
