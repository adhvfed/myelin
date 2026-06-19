# Sketch 04 — Permissions: page-tree inheritance-with-overrides + granularity

> Phase 4, Knowledge, **exploration**. Canonical: ADR-03 (ReBAC core / RBAC face / ABAC edges),
> `identity-and-access.md` §5 Knowledge clause, the Phase-3 handoff ("OWN the
> page-tree-inheritance-with-overrides ReBAC namespace"). I declare the `knowledge.*` ReBAC
> namespace fragment; Id owns the engine and never invents object ids.

---

## 0. What is already fixed

`identity-and-access.md` §5 already gives the Knowledge namespace skeleton and the canonical pattern:

> **Knowledge** — `space`, `page`, `block`, `database_row`: page-tree inheritance with **overrides**
> is the canonical Zanzibar pattern — `page.read = parent_page->read + direct_reader - direct_block`.
> A sub-page can *narrow* (override) inherited access via a `direct_block` exclusion relation.

So the **mechanism is decided** (union + tuple-to-userset rewrite for inheritance + an exclusion
userset for override/narrowing — the four Zanzibar operators, no bespoke check path). My job is to
(a) declare the *complete* namespace fragment, (b) decide the **granularity** (page-only vs +row vs
+field — deep-dive Q6, the scope decision flagged in `knowledge-platform.md` §9.5), and (c) wire it
to the page-tree storage + collab authority.

---

## 1. The namespace fragment (declared; Id compiles)

```
definition space {                         // = the workspace/teamspace grouping layer
  relation parent_project: project          // maps to the platform org→team→project hierarchy
  relation member:  user | team#member
  relation manager: user | team#member
  permission view  = member + manager + parent_project->read
  permission manage = manager + parent_project->admin
}

definition page {                          // a page is a doc AND a folder (Notion-unifies them)
  relation parent_space: space
  relation parent_page:  page               // sub-pages = the folder-like nesting
  relation direct_reader:  user | team#member
  relation direct_editor:  user | team#member
  relation direct_manager: user | team#member
  relation direct_block:   user | team#member   // OVERRIDE: narrows inherited access (exclusion)
  // inheritance via tuple-to-userset rewrite; override via exclusion userset
  permission read   = (parent_page->read + parent_space->view + direct_reader
                       + direct_editor + direct_manager) - direct_block
  permission edit   = (parent_page->edit + direct_editor + direct_manager) - direct_block
  permission manage = parent_page->manage + direct_manager
  permission publish = direct_manager + parent_space->manage   // publish-to-web is a tight permission
}

definition database {                      // an in-doc database (lives in a page)
  relation parent_page: page
  permission read  = parent_page->read
  permission edit  = parent_page->edit
  permission manage = parent_page->manage
}

definition database_row {                  // OPTIONAL row-level visibility (scope decision §2)
  relation parent_database: database
  relation row_owner: user | team#member        // for "see only your team's rows"
  permission view = parent_database->read        // floor: inherits the db (page-level granularity)
  // promotion: view = (parent_database->read - row_restricted) + row_owner   (row-level)
}
```

Notes:
- **A page is both a doc and a folder** (Notion-unifies; deep-dive §2.3) — so `parent_page` carries
  both the content-nesting and the permission-inheritance. The folders-vs-pure-pages question
  (deep-dive Q7) is resolved **pure-pages** for the model (everything is a page; "folder" is a
  presentation affordance over a childless-but-nesting page), keeping the namespace one type, not two.
- **Override = the `direct_block` exclusion** (a sub-page that should be *more* restricted than its
  parent sets `direct_block` to remove inherited grantees; a sub-page that should be *more* open adds
  `direct_reader`). This is the "inheritance with overrides" requirement reduced to one rewrite + one
  exclusion (the §5 pattern), so a narrowed sub-page **disappears from a non-grantee's `list_objects`
  by construction**, not by a post-filter.
- **`publish` is a distinct tight permission** (publish-to-web is a high-risk personal-data export,
  sketch 06) — never inherited from `read`/`edit`.

## 2. Granularity (the scope decision — deep-dive Q6)

Three levels, each a cost step. The corporate buyer (Confluence/Drive-migration) often wants row- and
field-level; the cost is real (`knowledge-platform.md` §2.7, §9.5).

| Level | Mechanism | Verdict |
|---|---|---|
| **Page / database** (default) | the namespace above; page-tree inheritance + overrides | **v1 floor (built).** Covers the dominant cases; the page is the permission boundary. |
| **Row-level** | `database_row.view = (parent_database->read - row_restricted) + row_owner`; "see only your team's rows" via an `row_owner` tuple or an ABAC caveat over a row field | **v1-eligible for DBs that opt in** (a per-database "row-level access" toggle); kept off the hot `list_objects` path where possible (ABAC caveats evaluate at `check` with context, identity §9). |
| **Field-level** (hide the salary column) | ABAC caveat on a `field` sub-object (`field.view` predicate), evaluated at read with context | **named follow-on (R5)** — promotion-triggered by a tenant that needs it. The mechanism (ABAC-at-the-edge, identity §9) is declared; the per-field UX + the read-path cost is the deferred part. |

**Leaning**: ship **page/database-level as the floor**, with **row-level as an opt-in per-database
capability in v1** (the common "your team's rows" need), and **field-level as the named ABAC-caveat
follow-on**. Block-level permission is *possible* (a block is an addressable object) but **deferred**
— block-level ACLs explode the permission surface and complicate erasure/synced-blocks; v1 makes the
*page* the boundary, with the block addressable for refs/erasure but not independently ACL'd.

## 3. How permissions wire to storage + collab (the enforcement chain)

- **The page-tree is NOT a column tree I enforce** — it compiles to ReBAC tuples (identity §3): the
  page-tree storage (sketch 02, per-block rows + page hierarchy) holds the *authoring* structure;
  Id's tuple store holds the *authorization* edges. A `page.move` / re-parent updates the
  `parent_page` tuple (via `write_tuples`, returning a zookie) in lockstep with the structure write.
- **The collab authority enforces permission on every incoming op** (deep-dive §6; CRDTs can't
  enforce ACLs): the doc session actor (sketch 01) calls `Id.check(actor, edit, page)` before
  appending an op to the op-log. A revoked editor's op is rejected at the authority — the merge layer
  never sees an unauthorized op. This is the "server is the authority for what the merge layer can't
  enforce" point, made the gate on the op-log append.
- **Reads are pre-filtered, never post-filtered**: every view, backlink, search result, embedded-view
  content uses Id's `list_objects` (ADR-03; the §5.6 / §7.4 invariant). A confidential sub-page's rows
  are *absent* from a non-grantee's view — the leak-free-by-construction property (the zero-escape leak
  drill, identity D4).
- **Zookie consistency** (identity §8.4): a permission change (revoke a reader, narrow a sub-page)
  returns a zookie stamped on the page version / the emitted `knowledge.access.revoked` event; later
  reads carry the zookie so a freshly-revoked grant cannot be read stale ("new enemy") — and a
  security-sensitive read bypasses Id's fail-static cache.

## 4. What this sketch commits to the findings

- **The `knowledge.*` ReBAC namespace fragment** (above): `space`/`page`/`database`/`database_row`,
  page-tree inheritance via tuple-to-userset rewrite + override via the `direct_block` exclusion. Pure-
  pages model (a page is doc+folder; folders are presentation). `publish` is a distinct tight permission.
- **Granularity**: page/database-level floor; **row-level as an opt-in per-database capability in v1**;
  field-level as the named ABAC-caveat follow-on (R5). Block-level ACL deferred (block stays addressable
  for refs/erasure, not independently ACL'd).
- **Enforcement chain**: page-tree compiles to tuples (no bespoke ACL); the collab authority gates the
  op-log append on `Id.check`; reads pre-filtered via `list_objects`; permission changes carry zookies.

## Cited prior art

- Zanzibar namespace model + tuple-to-userset rewrite + exclusion userset: Pang et al., *Zanzibar*
  (USENIX ATC 2019); SpiceDB schema DSL / OpenFGA modeling language; the §5 Knowledge clause.
- ABAC-at-the-edge (field-level caveats): NIST SP 800-162; SpiceDB caveats / OpenFGA conditions
  (identity §9).
- Page-tree inheritance with overrides: the Notion/Google-Drive ACL model (deep-dive §2.7).
- Zookie / "new enemy": Zanzibar §2.4.4 (identity §8.4).
