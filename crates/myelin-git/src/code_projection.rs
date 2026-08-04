use std::collections::{BTreeMap, BTreeSet};

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventType, IdMinter,
    OutboxError, OutboxStore, OutboxTx, SubjectComponent, Visibility,
};
use myelin_query::FieldValue;
use myelin_search::SearchProjection;

use crate::events::GIT_BLOB_SNAPSHOT;
use crate::receive_pack::{GitRefEventKey, RefName};
use crate::search_projection::{FACET_BLOB_OID, FACET_LANGUAGE, FACET_PATH};

pub fn is_indexed_ref(ref_name: &str, default_branch: &str) -> bool {
    ref_name == format!("refs/heads/{default_branch}") || ref_name == default_branch
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobOid(pub String);

impl BlobOid {
    pub fn new(hex: impl Into<String>) -> BlobOid {
        BlobOid(hex.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    pub oid: BlobOid,
    pub bytes: Vec<u8>,
}

impl Blob {
    pub fn new(oid: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Blob {
        Blob {
            oid: BlobOid::new(oid),
            bytes: bytes.into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    entries: BTreeMap<String, Blob>,
}

impl Tree {
    pub fn empty() -> Tree {
        Tree::default()
    }

    pub fn with(mut self, path: impl Into<String>, blob: Blob) -> Tree {
        self.entries.insert(path.into(), blob);
        self
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobChange {
    Upserted {
        path: String,
        blob: Blob,
    },
    Deleted {
        path: String,
        oid: BlobOid,
    },
}

impl BlobChange {
    pub fn path(&self) -> &str {
        match self {
            BlobChange::Upserted { path, .. } | BlobChange::Deleted { path, .. } => path,
        }
    }
}

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
    for (path, new_blob) in &new_tip.entries {
        match last_indexed.entries.get(path) {
            Some(old) if old.oid == new_blob.oid => {}
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
    for (path, old_blob) in &last_indexed.entries {
        if !new_tip.entries.contains_key(path) {
            ensure_projection_change_capacity(&changes, path, maximum_changes, maximum_path_bytes)?;
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

pub fn split_symbol(token: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lower = token.to_ascii_lowercase();
    if !lower.is_empty() {
        out.push(lower);
    }
    for part in token.split(['_', '-']) {
        if part.is_empty() {
            continue;
        }
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

fn split_camel(token: &str) -> Vec<String> {
    let chars: Vec<char> = token.chars().collect();
    let mut words = Vec::new();
    let mut start = 0usize;
    for i in 1..chars.len() {
        let prev = chars[i - 1];
        let cur = chars[i];
        let boundary =
            (!prev.is_uppercase() && cur.is_uppercase())
            || (prev.is_uppercase()
                && cur.is_uppercase()
                && chars.get(i + 1).is_some_and(|n| n.is_lowercase()))
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

pub fn extract_symbols_bounded(
    text: &str,
    maximum_input_bytes: usize,
    maximum_terms: usize,
    maximum_term_bytes: usize,
    maximum_total_term_bytes: usize,
) -> Result<Vec<String>, String> {
    ensure_projection_text_limit(text, maximum_input_bytes)?;
    let mut out = BTreeSet::new();
    let mut total_term_bytes = 0usize;
    for tok in identifier_tokens(text) {
        for term in split_symbol(&tok) {
            insert_projection_term(
                &mut out,
                &mut total_term_bytes,
                term,
                maximum_terms,
                maximum_term_bytes,
                maximum_total_term_bytes,
            )?;
        }
    }
    Ok(out.into_iter().collect())
}

pub fn extract_literals_bounded(
    text: &str,
    maximum_input_bytes: usize,
    maximum_terms: usize,
    maximum_term_bytes: usize,
    maximum_total_term_bytes: usize,
) -> Result<Vec<String>, String> {
    ensure_projection_text_limit(text, maximum_input_bytes)?;
    let mut out = BTreeSet::new();
    let mut total_term_bytes = 0usize;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' || c == '\'' {
            let quote = c;
            let mut j = i + 1;
            let mut buf = String::new();
            while j < chars.len() && chars[j] != quote {
                if chars[j] == '\\' && j + 1 < chars.len() {
                    buf.push(chars[j + 1]);
                    j += 2;
                } else {
                    buf.push(chars[j]);
                    j += 1;
                }
                ensure_projection_term_length(&buf, maximum_term_bytes)?;
            }
            if !buf.is_empty() {
                insert_projection_term(
                    &mut out,
                    &mut total_term_bytes,
                    buf,
                    maximum_terms,
                    maximum_term_bytes,
                    maximum_total_term_bytes,
                )?;
            }
            i = j + 1;
        } else if c.is_ascii_digit() {
            let mut j = i;
            let mut buf = String::new();
            while j < chars.len()
                && (chars[j].is_ascii_alphanumeric() || chars[j] == '.' || chars[j] == '_')
            {
                buf.push(chars[j]);
                j += 1;
                ensure_projection_term_length(&buf, maximum_term_bytes)?;
            }
            let lit = buf.trim_end_matches('.').to_string();
            if !lit.is_empty() {
                insert_projection_term(
                    &mut out,
                    &mut total_term_bytes,
                    lit,
                    maximum_terms,
                    maximum_term_bytes,
                    maximum_total_term_bytes,
                )?;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    Ok(out.into_iter().collect())
}

fn ensure_projection_text_limit(text: &str, maximum_input_bytes: usize) -> Result<(), String> {
    if text.len() > maximum_input_bytes {
        return Err("code projection token input limit exceeded".into());
    }
    Ok(())
}

fn ensure_projection_term_length(term: &str, maximum_term_bytes: usize) -> Result<(), String> {
    if term.len() > maximum_term_bytes {
        return Err("code projection term length limit exceeded".into());
    }
    Ok(())
}

fn insert_projection_term(
    out: &mut BTreeSet<String>,
    total_term_bytes: &mut usize,
    term: String,
    maximum_terms: usize,
    maximum_term_bytes: usize,
    maximum_total_term_bytes: usize,
) -> Result<(), String> {
    ensure_projection_term_length(&term, maximum_term_bytes)?;
    if out.contains(&term) {
        return Ok(());
    }
    if out.len() >= maximum_terms {
        return Err("code projection term count limit exceeded".into());
    }
    *total_term_bytes = total_term_bytes
        .checked_add(term.len())
        .ok_or_else(|| "code projection term byte count overflowed".to_string())?;
    if *total_term_bytes > maximum_total_term_bytes {
        return Err("code projection aggregate term limit exceeded".into());
    }
    out.insert(term);
    Ok(())
}

fn identifier_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' {
            cur.push(c);
        } else if !cur.is_empty() {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobProjection {
    pub artifact_ref: ArtifactRef,
    pub path: String,
    pub language: String,
    pub symbols: Vec<String>,
    pub literals: Vec<String>,
    pub text: String,
    pub commit_message: String,
    pub blob_oid: BlobOid,
}

impl BlobProjection {
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

#[derive(Debug, Default)]
pub struct CodeProjectionCursor {
    last_indexed: std::sync::Mutex<BTreeMap<(String, String), String>>,
}

impl CodeProjectionCursor {
    pub fn new() -> CodeProjectionCursor {
        CodeProjectionCursor::default()
    }

    pub fn last_indexed(&self, repo: &str, ref_name: &str) -> Option<String> {
        self.last_indexed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(repo.to_string(), ref_name.to_string()))
            .cloned()
    }

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

pub trait RestrictionPolicy: Send + Sync {
    fn is_restricted(&self, repo: &str, path: &str) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoRestrictions;

impl RestrictionPolicy for NoRestrictions {
    fn is_restricted(&self, _repo: &str, _path: &str) -> bool {
        false
    }
}

#[derive(Clone, Debug)]
pub struct ProjectionEmit {
    pub emitted: Vec<myelin_events::EventId>,
    pub changed_blob_count: usize,
}

const PROJECTION_MAX_CHANGED_BLOBS: usize = 1_000;
const PROJECTION_MAX_BLOB_BYTES: usize = 1024 * 1024;
const PROJECTION_MAX_TOTAL_BLOB_BYTES: usize = 64 * 1024 * 1024;
const PROJECTION_MAX_PATH_BYTES: usize = 4 * 1024;
const PROJECTION_MAX_COMMIT_MESSAGE_BYTES: usize = 8 * 1024;
const PROJECTION_MAX_TERMS_PER_BLOB: usize = 20_000;
const PROJECTION_MAX_TERM_BYTES: usize = 4 * 1024;
const PROJECTION_MAX_TOTAL_TERM_BYTES_PER_FACET: usize = 1024 * 1024;

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

    fn blob_ref(&self, ref_name: &str, path: &str) -> Result<ArtifactRef, OutboxError> {
        let repo = SubjectComponent::encode(&self.repo)
            .map_err(|_| OutboxError("invalid blob repository component".into()))?;
        let ref_name = SubjectComponent::encode(ref_name)
            .map_err(|_| OutboxError("invalid blob ref component".into()))?;
        let path = SubjectComponent::encode(path)
            .map_err(|_| OutboxError("invalid blob path component".into()))?;
        myelin_refs::parse(&format!(
            "myelin://{}/git/blob/{}:{}:{}",
            self.ctx_base.tenant.0,
            repo.as_str(),
            ref_name.as_str(),
            path.as_str()
        ))
        .map_err(|_| OutboxError("invalid canonical blob reference".into()))
    }

    fn aggregate(&self, ref_name: &str) -> Result<AggregateKey, OutboxError> {
        GitRefEventKey::new(&self.repo, &RefName::new(ref_name))
            .map(|key| key.aggregate())
            .map_err(|_| OutboxError("invalid code projection ref key".into()))
    }

    fn project_upsert(
        &self,
        artifact_ref: ArtifactRef,
        path: &str,
        blob: &Blob,
        commit_message: &str,
    ) -> Result<BlobProjection, String> {
        let restricted = self.restriction.is_restricted(&self.repo, path);
        let text = if restricted {
            String::new()
        } else {
            String::from_utf8_lossy(&blob.bytes).into_owned()
        };
        Ok(BlobProjection {
            artifact_ref,
            path: path.to_string(),
            language: detect_language(path),
            symbols: if restricted {
                Vec::new()
            } else {
                extract_symbols_bounded(
                    &text,
                    PROJECTION_MAX_BLOB_BYTES,
                    PROJECTION_MAX_TERMS_PER_BLOB,
                    PROJECTION_MAX_TERM_BYTES,
                    PROJECTION_MAX_TOTAL_TERM_BYTES_PER_FACET,
                )?
            },
            literals: if restricted {
                Vec::new()
            } else {
                extract_literals_bounded(
                    &text,
                    PROJECTION_MAX_BLOB_BYTES,
                    PROJECTION_MAX_TERMS_PER_BLOB,
                    PROJECTION_MAX_TERM_BYTES,
                    PROJECTION_MAX_TOTAL_TERM_BYTES_PER_FACET,
                )?
            },
            text,
            commit_message: if restricted {
                String::new()
            } else {
                commit_message.to_string()
            },
            blob_oid: blob.oid.clone(),
        })
    }

    pub fn emit_for_push(
        &self,
        ref_name: &str,
        new_tip_oid: &str,
        last_indexed_tree: &Tree,
        new_tip_tree: &Tree,
        commit_message: &str,
    ) -> Result<Option<ProjectionEmit>, OutboxError> {
        if !is_indexed_ref(ref_name, &self.default_branch) {
            return Ok(None);
        }
        if commit_message.len() > PROJECTION_MAX_COMMIT_MESSAGE_BYTES {
            return Err(OutboxError(
                "code projection commit message limit exceeded".into(),
            ));
        }

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
        let aggregate = self.aggregate(ref_name)?;

        let mut tx = self
            .outbox
            .begin(std::sync::Arc::clone(&self.minter), self.ctx_base.clone());
        tx.stage_state_change(format!(
            "code_projection_cursor {}:{} -> {new_tip_oid}",
            self.repo, ref_name
        ));

        let mut emitted = Vec::new();
        for change in &changes {
            let subject = self.blob_ref(ref_name, change.path())?;
            let payload = match change {
                BlobChange::Upserted { path, blob } => {
                    let proj = self
                        .project_upsert(subject.clone(), path, blob, commit_message)
                        .map_err(OutboxError)?;
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
                    serde_json::json!({
                        "op": "delete",
                        "artifact_ref": subject.0,
                        "path": path,
                        "blob_oid": oid.0,
                        "acl_object_type": crate::search_projection::GIT_BLOB_ACL_OBJECT_TYPE,
                    })
                }
            };
            let draft = EventDraft {
                type_: EventType(GIT_BLOB_SNAPSHOT.into()),
                subject,
                aggregate: aggregate.clone(),
                payload,
                data_role: DataRole::Processor,
                visibility: Visibility::Internal,
                contains_personal_data: false,
                pii_key_ref: None,
            };
            let id = tx.emit(draft, None)?;
            emitted.push(id);
        }

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

    fn symbols(text: &str) -> Vec<String> {
        extract_symbols_bounded(text, 1024, 100, 256, 4096).expect("small symbol input")
    }

    fn literals(text: &str) -> Vec<String> {
        extract_literals_bounded(text, 1024, 100, 256, 4096).expect("small literal input")
    }

    #[test]
    fn split_symbol_handles_camel_snake_kebab_and_acronyms() {
        assert_eq!(
            split_symbol("parse_http_response"),
            vec!["http", "parse", "parse_http_response", "response"]
        );
        let s = split_symbol("parseHTTPResponse");
        assert!(s.contains(&"parse".to_string()));
        assert!(s.contains(&"http".to_string()));
        assert!(s.contains(&"response".to_string()));
        assert!(
            s.contains(&"parsehttpresponse".to_string()),
            "the whole token is searchable too"
        );
        let k = split_symbol("api-v2");
        assert!(k.contains(&"api".to_string()));
        assert!(k.contains(&"v".to_string()));
        assert!(k.contains(&"2".to_string()));
    }

    #[test]
    fn extract_symbols_splits_identifiers_only_not_numbers() {
        let text = "fn parseHttp() { let maxRetries = 42; }";
        let syms = symbols(text);
        assert!(syms.contains(&"parse".to_string()));
        assert!(syms.contains(&"http".to_string()));
        assert!(syms.contains(&"max".to_string()));
        assert!(syms.contains(&"retries".to_string()));
        assert!(syms.contains(&"fn".to_string()));
        assert!(!syms.contains(&"42".to_string()));
    }

    #[test]
    fn extract_literals_finds_strings_and_numbers() {
        let text = r#"let url = "https://example.test"; let n = 42; let pi = 3.14;"#;
        let lits = literals(text);
        assert!(
            lits.contains(&"https://example.test".to_string()),
            "{lits:?}"
        );
        assert!(lits.contains(&"42".to_string()));
        assert!(lits.contains(&"3.14".to_string()));
    }

    #[test]
    fn extract_literals_handles_escapes_hex_and_trailing_dot() {
        let escaped = literals(r#""a\"b""#);
        assert!(
            escaped.contains(&"a\"b".to_string()),
            "escape handling: {escaped:?}"
        );
        let sq = literals("x = 'hello'");
        assert!(sq.contains(&"hello".to_string()), "{sq:?}");
        let nums = literals("a = 0xFF; b = 1_000;");
        assert!(nums.contains(&"0xFF".to_string()), "hex literal: {nums:?}");
        assert!(
            nums.contains(&"1_000".to_string()),
            "underscore-grouped: {nums:?}"
        );
        let td = literals("n = 5. end");
        assert!(
            td.contains(&"5".to_string()),
            "trailing dot trimmed: {td:?}"
        );
        assert!(
            !td.iter().any(|l| l == "5."),
            "the trailing-dot form is not emitted: {td:?}"
        );
        assert!(
            literals(r#"x = """#).is_empty(),
            "an empty string literal emits no literal"
        );
    }

    #[test]
    fn token_extractors_enforce_input_count_term_and_aggregate_byte_limits() {
        assert_eq!(
            extract_symbols_bounded("a b", 3, 2, 1, 2).expect("exact symbol limits accepted"),
            vec!["a", "b"]
        );
        assert!(extract_symbols_bounded("a b", 2, 2, 1, 2).is_err());
        assert!(extract_symbols_bounded("a b", 3, 1, 1, 2).is_err());
        assert!(extract_symbols_bounded("ab", 2, 2, 1, 2).is_err());
        assert!(extract_symbols_bounded("a b", 3, 2, 1, 1).is_err());
        assert!(extract_literals_bounded("\"abcd\"", 6, 1, 3, 4).is_err());
    }

    #[test]
    fn identifier_tokens_require_a_leading_letter_or_underscore() {
        let syms = symbols("let _private = var2 + 42;");
        assert!(
            syms.iter().any(|s| s == "_private" || s == "private"),
            "{syms:?}"
        );
        assert!(syms.contains(&"var".to_string()), "{syms:?}");
        assert!(
            !syms.contains(&"42".to_string()),
            "a bare number is a literal, not a symbol: {syms:?}"
        );
        let trailing = symbols("call doThing");
        assert!(
            trailing.contains(&"do".to_string()) && trailing.contains(&"thing".to_string()),
            "{trailing:?}"
        );
    }

    #[test]
    fn underscore_is_part_of_the_identifier_token() {
        let syms = symbols("fn parse_config");
        assert!(
            syms.contains(&"parse_config".to_string()),
            "the whole snake token is searchable: {syms:?}"
        );
        assert!(syms.contains(&"parse".to_string()) && syms.contains(&"config".to_string()));
        let tail = symbols("see also_this");
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
        assert_eq!(detect_language("X.RS"), "rust");
    }

    #[test]
    fn split_camel_includes_the_trailing_run() {
        let s = split_symbol("parseHTTPResponse");
        assert!(
            s.contains(&"response".to_string()),
            "the trailing camel run is included: {s:?}"
        );
        assert_eq!(split_symbol("hello"), vec!["hello"]);
        let d = split_symbol("foo2");
        assert!(
            d.contains(&"foo".to_string()) && d.contains(&"2".to_string()),
            "{d:?}"
        );
    }

    fn diff_for_test(old: &Tree, new: &Tree) -> Vec<BlobChange> {
        diff_trees_bounded(old, new, 100, 1024, 4096, 256).expect("small test diff")
    }

    #[test]
    fn diff_emits_only_changed_blobs_not_the_whole_tree() {
        let old = Tree::empty()
            .with("a.rs", Blob::new("oid-a1", b"fn a() {}".to_vec()))
            .with("b.rs", Blob::new("oid-b1", b"fn b() {}".to_vec()))
            .with("c.rs", Blob::new("oid-c1", b"fn c() {}".to_vec()));
        let new = Tree::empty()
            .with("a.rs", Blob::new("oid-a1", b"fn a() {}".to_vec()))
            .with("b.rs", Blob::new("oid-b2", b"fn b2() {}".to_vec()))
            .with("d.rs", Blob::new("oid-d1", b"fn d() {}".to_vec()));
        let changes = diff_for_test(&old, &new);
        assert_eq!(changes.len(), 3, "{changes:?}");
        let paths: Vec<&str> = changes.iter().map(|c| c.path()).collect();
        assert!(paths.contains(&"b.rs"));
        assert!(paths.contains(&"d.rs"));
        assert!(paths.contains(&"c.rs"));
        assert!(
            !paths.contains(&"a.rs"),
            "an unchanged blob emits nothing (incremental)"
        );
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
        let oversized_term = Tree::empty().with(
            "term.rs",
            Blob::new("term", vec![b'x'; PROJECTION_MAX_TERM_BYTES + 1]),
        );

        assert!(e
            .emit_for_push(
                "refs/heads/main",
                "term-tip",
                &Tree::empty(),
                &oversized_term,
                "small",
            )
            .is_err());
        assert!(e
            .emit_for_push(
                "refs/heads/main",
                "blob-tip",
                &Tree::empty(),
                &oversized_blob,
                "small",
            )
            .is_err());
        assert!(e
            .emit_for_push(
                "refs/heads/main",
                "message-tip",
                &Tree::empty(),
                &Tree::empty(),
                &"x".repeat(PROJECTION_MAX_COMMIT_MESSAGE_BYTES + 1),
            )
            .is_err());
        assert_eq!(outbox.committed_count(), 0);
        assert!(cursor.last_indexed("core", "refs/heads/main").is_none());
    }

    #[test]
    fn emit_count_equals_changed_blob_count_incremental() {
        let outbox = OutboxStore::new();
        let cursor = CodeProjectionCursor::new();
        let r = NoRestrictions;
        let e = emitter(&outbox, &cursor, &r);

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

        let t2 = t1
            .clone()
            .with(
                "src/lib.rs",
                Blob::new("o1b", b"pub fn helloWorld() { ok() }".to_vec()),
            )
            .with("src/new.rs", Blob::new("o3", b"fn n() {}".to_vec()));
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
            serde_json::json!("myelin://acme/git/blob/core:refs%2Fheads%2Fmain:src%2Fmain%2Ers")
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
        let syms = pl["symbols"].as_array().unwrap();
        assert!(syms.iter().any(|s| s == "parse"));
        assert!(syms.iter().any(|s| s == "http"));
        let lits = pl["literals"].as_array().unwrap();
        assert!(lits.iter().any(|l| l == "http://x"));
        assert_eq!(
            row.aggregate,
            AggregateKey("ref:core:refs%2Fheads%2Fmain".into())
        );
    }

    #[test]
    fn into_search_projection_uses_the_spec_facets() {
        let bp = BlobProjection {
            artifact_ref: ArtifactRef(
                "myelin://acme/git/blob/core:refs%2Fheads%2Fmain:a%2Ers".into(),
            ),
            path: "a.rs".into(),
            language: "rust".into(),
            symbols: vec!["parse".into(), "http".into()],
            literals: vec!["lit".into()],
            text: "fn parse() {}".into(),
            commit_message: "msg".into(),
            blob_oid: BlobOid::new("oid-1"),
        };
        let sp = bp.into_search_projection();
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
        assert!(sp.text.contains("parse"));
        assert!(sp.text.contains("lit"));
        assert!(sp.text.contains("msg"));
        assert!(sp.text.contains("fn parse() {}"));
    }

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
        assert_eq!(sd["path"], serde_json::json!("secret.rs"));
        assert_eq!(sd["blob_oid"], serde_json::json!("os"));
    }

    #[test]
    fn a_deleted_blob_emits_a_delete_tombstone() {
        let outbox = OutboxStore::new();
        let cursor = CodeProjectionCursor::new();
        let r = NoRestrictions;
        let e = emitter(&outbox, &cursor, &r);
        let t1 = Tree::empty().with("gone.rs", Blob::new("g1", b"fn gone() {}".to_vec()));
        e.emit_for_push("refs/heads/main", "t1", &Tree::empty(), &t1, "add")
            .unwrap();
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
