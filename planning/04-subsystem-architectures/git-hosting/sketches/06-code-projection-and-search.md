# Sketch 06 — Code projection & code-search v1 scope (TE-27)

> Exploration note. We OWN the indexable code projection (Phase-3 handoff; Search contract 6.5). Search
> does not parse repos — we emit a per-blob/ref/symbol projection; Search indexes it. Decides what v1
> indexes, how it updates incrementally on push, and where the v1/v2 line sits. Date: 2026-06-19.

## The obligation (from Phase-3)

- **Search contract 6.5 / 6.3:** "Git emits an indexable `git.*` projection per blob/ref/symbol (path,
  symbols, literals, commit message) for code-search v1. Search does not parse repos."
- **Search §4.4** fixes v1 grade: **symbol/path/literal/trigram-grade**; AST/cross-reference is the
  named follow-on. v1 indexes file paths, identifiers/symbols (a lightweight per-language tokenizer
  splitting camelCase/snake_case, keeping operators), string literals, and commit messages, with
  **trigram/n-gram indexing for substring/regex-lite** (Russ Cox, *Regular Expression Matching with a
  Trigram Index*, 2012).
- **`declare_indexable(IndexSpec)`** (6.3) — we declare how a git blob/ref projects to an index doc.
- **ACL:** Search **always conjoins `list_objects(viewer, read, repo)`** before scoring (the
  `search-requires-acl-filter` lint) — so our projection just needs the right `acl_object_type` (`repo`)
  and Search handles the permission filter. We don't build the ACL; we declare the object type.

## What we emit (the code projection)

Two projection granularities, both off the **push path** (incremental on `git.ref.updated`), both
references-not-payloads (the *index doc* carries text; the *event* carries a pointer + the doc):

1. **Per-blob projection** (on the default branch + indexed branches): for each changed blob in the
   push, emit an indexable doc:
   ```
   IndexDoc {
     acl_object_type: "repo", acl_object_id: <repo_id>,         // Search ACL-filters via list_objects(repo)
     subsystem: "git", type: "blob",
     ref: myelin://<tenant>/git/blob/<repo>/<path>#<branch>,    // ArtifactRef, sub-artifact granular
     path: "src/auth/sso.rs",                                   // path tokens (split on / and case)
     ft_fields: { content_tokens, code_symbols: [...], string_literals: [...] },  // §4.4 grade
     struct_fields: { lang, branch, blob_sha, size },
     trigrams: <for substring/regex-lite>                       // Search builds the trigram index
   }
   ```
2. **Per-commit-message + per-PR/review/comment projection:** commit messages, PR titles/bodies, review
   and inline-comment text → FT docs (so "find that commit/PR/comment" works). These are control-plane
   rows, projected the same way.

**Incremental update on push (the algorithm):**
```
on git.ref.updated(repo, ref, old_sha, new_sha):              # consume our own event, async off the bus
  if ref is an indexed branch (default + configured):
     diff = tree_diff(old_sha, new_sha)                        # gix in-process (sketch 02)
     for each {added|modified} blob: emit upsert IndexDoc (tokenize path/symbols/literals)
     for each {deleted} blob:        emit delete IndexDoc
     emit commit-message docs for the new commits
```
- **Tokenizer:** a **lightweight per-language tokenizer** (tree-sitter-lite or a curated set of
  language lexers) that splits identifiers on camelCase/snake_case and preserves operators — *not* a
  full parser/AST (that's the v2 follow-on). Undetected languages fall back to Unicode word
  segmentation (UAX #29), matching Search §4.4's language-agnostic fallback.
- **`replay(scope, since)`** (the reindex-from-source obligation): on a Search rebuild we re-emit the
  full per-blob projection for a repo/ref by walking the tree at the indexed tip — **sub-artifact-
  granular** `git.blob.snapshot` events (Bus reindex needs sub-artifact granularity, contract 2.6). One
  code path: the snapshot walk and the incremental push-diff produce the same `IndexDoc` shape.

## Scope candidates for v1

- **A. Per-repo/per-tenant lexical only** (default branch): paths + symbols + literals + trigram
  substring + commit/PR/comment text. Cross-repo search is "run the query across the repos you can see"
  (Search fans out, ACL-filtered). *This is Search §4.4's v1 grade exactly.*
- **B. Cross-repo semantic/symbol nav ("go to definition / find references")**: needs per-language AST
  indices (SCIP/LSIF), ideally **produced by CI** and consumed here. *Much larger; multi-year at world
  scale (Phase-1 §4.5 — "Sourcegraph is a whole company").*
- **C. Code embeddings / semantic code retrieval**: vector search over code. *Follow-on; HYOK content
  can't be embedded (storage §6.1 — `can_derive_plaintext_index()=false`).*

## Leaning (committed in findings)

**v1 = Candidate A** — per-blob path/symbol/literal/trigram + commit/PR/comment FT, **indexed on the
default branch (+ configured branches), incremental on push**, emitted as `IndexDoc`s with
`acl_object_type=repo` so Search's `list_objects` pre-filter handles permissions. We **own the
tokenizer + the diff-driven incremental update + `replay` for reindex-from-source**; Search owns the
trigram/FT/index plumbing. **Indexed branches default to the default branch** (indexing every branch of
every repo is the volume trap — measure first).

**Named follow-ons:** (1) **SCIP/LSIF symbol indices produced by CI** → "go to definition / find
references" (the Git↔CI code-intelligence seam); (2) **code embeddings** for semantic retrieval. Both
promotion-triggered by demand, **not built in v1** (Search §4.4 floor).

## Prior art / sources

- Russ Cox, *Regular Expression Matching with a Trigram Index* (2012) — Google Code Search / the v1
  substring approach.
- GitHub **Blackbird** (trigram code search rebuild); **Sourcegraph** / SCIP/LSIF (code intelligence).
- tree-sitter (incremental parsing) — the v2 AST path.
- Phase-3 Search §4.4 (code-search v1 grade), §5.3 (`declare_indexable`/`IndexSpec`), §6.5 (Git input);
  contract-index 6.5; Bus contract 2.6 (sub-artifact-granular snapshot).
- UAX #29 (Unicode text segmentation) — the language-agnostic fallback.
