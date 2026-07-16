// THE DEV-SEAM CONTRACT (shared by the dev edge + the dev-login seam) — clearly marked, NOT production.
//
// Why this exists: the real `myelin-edge` binary authenticates with REAL PASETO capability tokens and
// a seeded S1 principal directory; a HUMAN cannot yet be issued such a token (the human/OIDC login is
// MR-012-deferred — `POST /v1/auth/login` REFUSES, refuse-not-mock). So to run the shell + Playwright
// authenticated END-TO-END now, the dev edge accepts ONE well-known dev token and the dev-login seam
// mints a session carrying it. The gateway client + the session machinery are REAL and contract-
// faithful; only the *token issuance* is the dev stand-in the real login replaces. NEVER ship this.
//
// The data the dev edge serves is the REAL Git ViewModel JSON shape (RepoHome::to_json,
// crates/myelin-git/src/web.rs) re-rooted under the MR-015 `/v1/git/...` edge contract — so the screen
// renders the genuine edge ViewModel, not an invented shape.

export const DEV_ACCESS_TOKEN = "dev.access.myelin-shell-e2e";
export const DEV_REFRESH_TOKEN = "dev.refresh.myelin-shell-e2e";
export const DEV_SCHEME = "pat";

export const DEV_PRINCIPAL = {
  principalId: "u_dev_operator",
  displayName: "Dev Operator",
  tenant: "acme",
  region: "eu-west",
};

// The whoami view-model the edge's WhoamiHandler returns (gateway.rs) — principal + the SET scope.
export function whoamiJson() {
  return {
    principal_id: DEV_PRINCIPAL.principalId,
    tenant: DEV_PRINCIPAL.tenant,
    region: DEV_PRINCIPAL.region,
    kind: "human",
  };
}

// A brief commit projection (the latest-commit bar + per-entry activity — R3.4).
const LATEST = {
  short_oid: "b2c3d4e5f607",
  oid: "b2c3d4e5f60718293a4b5c6d7e8f900112233445",
  summary: "docs: expand the README",
  author: "u_dev_operator@acme.noreply",
  committed_at: 1719446400,
};

// The acme/myelin working tree (R3.4) — a NESTED structure so the browse surface exercises tree-at-path
// + nested blobs. `{ file }` = a blob (text unless `binary`), `{ dir }` = a subtree.
const MYELIN_TREE = {
  "README.md": {
    file:
      "# acme/myelin\n\nThe make-it-real spine.\n\n## Usage\n\n- Browse the tree\n- Open a file\n\nSee `crates/` for the code.\n",
  },
  "Cargo.toml": { file: '[workspace]\nmembers = ["crates/*"]\n' },
  crates: {
    dir: {
      "myelin-edge": {
        dir: {
          "lib.rs": { file: "pub fn edge() {\n    // the product edge\n}\n" },
          "README.md": { file: "# myelin-edge\n\nThe product edge crate.\n" },
        },
      },
    },
  },
};

// Two repos in the verified tenant — a POPULATED one (enriched: default_branch, full README, latest
// commit, counts, name+activity entries) and an EMPTY one — so the screen exercises both the data row
// AND an unglamorous (empty/onboarding) state. Shapes match the R3.4 edge RepoHome VM.
export const SEED_REPOS = [
  {
    state: "populated",
    slug: "acme/myelin",
    default_branch: "main",
    readme: MYELIN_TREE["README.md"].file,
    readme_excerpt: "# acme/myelin\n\nThe make-it-real spine.",
    clone_url: "ssh://git@myelin/acme/myelin.git",
    latest_commit: LATEST,
    counts: { branches: 2, tags: 1 },
    entries: [
      { name: "crates", path: "crates", is_dir: true, latest_commit: LATEST },
      { name: "Cargo.toml", path: "Cargo.toml", is_dir: false, size: 34, latest_commit: LATEST },
      { name: "README.md", path: "README.md", is_dir: false, size: 120, latest_commit: LATEST },
    ],
  },
  {
    state: "empty",
    slug: "acme/sandbox",
    default_branch: "main",
    clone_url: "ssh://git@myelin/acme/sandbox.git",
    counts: { branches: 0, tags: 0 },
  },
];

// The MR-014 uniform list envelope `{ items, page: { next_cursor, limit } }` (catalogue.rs).
export function reposEnvelope(limit = 50) {
  return { items: SEED_REPOS, page: { next_cursor: null, limit } };
}

// The bare repo name (the edge route param) for a tenant-qualified slug (`acme/myelin` → `myelin`).
function bareName(slug) {
  const parts = slug.split("/");
  return parts.length > 1 ? parts.slice(1).join("/") : slug;
}

// ── GT-004 browse surface: the single repo home, blob, commit log + diff, PR overview ──
//
// These mirror the REAL durable-edge JSON shapes exactly (crates/myelin-git/src/web.rs `to_json` +
// crates/myelin-edge/src/git_durable.rs) so the Solid screens render the genuine contract. The data is
// the SAME seed the repos list serves (acme/myelin populated, acme/sandbox empty), extended with a
// real commit pair, a file, and two PRs (one blocked-with-a-fork-trust-badge, one ready).

/** GET /v1/git/repos/{repo} → the RepoHome ViewModel (null = 404). */
export function repoHomeJson(repo) {
  return SEED_REPOS.find((r) => bareName(r.slug) === repo) ?? null;
}

// ── R3.4: the ref switcher + nested tree + enriched blob (all keyed off MYELIN_TREE) ──

/** Walk MYELIN_TREE to a node at `path` (""=root). Returns `{ node, kind }` or null (absent). */
function walkTree(path) {
  const parts = (path ?? "").split("/").filter(Boolean);
  let node = MYELIN_TREE;
  for (let i = 0; i < parts.length; i++) {
    const entry = node[parts[i]];
    if (!entry) return null;
    if (entry.dir) {
      node = entry.dir;
      if (i === parts.length - 1) return { node: entry.dir, kind: "dir" };
    } else {
      // A file: only valid if it is the LAST segment.
      return i === parts.length - 1 ? { node: entry, kind: "file" } : null;
    }
  }
  return { node, kind: "dir" };
}

function entriesOf(dirNode, base) {
  return Object.keys(dirNode)
    .map((name) => {
      const e = dirNode[name];
      const full = base ? `${base}/${name}` : name;
      const is_dir = Boolean(e.dir);
      const row = { name, path: full, is_dir, latest_commit: LATEST };
      if (!is_dir) row.size = (e.file ?? "").length;
      return row;
    })
    .sort((a, b) => (a.is_dir === b.is_dir ? a.name.localeCompare(b.name) : a.is_dir ? -1 : 1));
}

/** GET /v1/git/repos/{repo}/refs → the RefsVM (null = 404). */
export function refsJson(repo) {
  if (repo !== "myelin") return null;
  return {
    branches: [
      { name: "main", oid: LATEST.oid, is_default: true },
      { name: "feature", oid: "a1b2c3d4e5f60718293a4b5c6d7e8f9001122334" },
    ],
    tags: [{ name: "v0.1", oid: LATEST.oid }],
    default_branch: "main",
  };
}

/** GET /v1/git/repos/{repo}/tree/{ref}/{...path} → the TreeVM (null = 404). */
export function treeJson(repo, _ref, path) {
  if (repo !== "myelin") return null;
  const hit = walkTree(path);
  if (!hit) return { __status: 404 };
  if (hit.kind === "file") return { redirect_to_blob: true, ref: _ref, path };
  const base = (path ?? "").replace(/^\/+|\/+$/g, "");
  const readme = hit.node["README.md"]?.file ?? null;
  return { ref: _ref, path: base, entries: entriesOf(hit.node, base), readme };
}

/** GET /v1/git/repos/{repo}/blob/{ref}/{...path} → the enriched BlobVM (null = 404). */
export function blobJson(repo, ref, path) {
  if (repo !== "myelin") return null;
  const hit = walkTree(path);
  if (!hit) return { __status: 404 };
  if (hit.kind === "dir") return { redirect_to_tree: true, ref, path };
  const content = hit.node.file ?? "";
  const is_binary = Boolean(hit.node.binary);
  return {
    path,
    contents: is_binary ? "" : content,
    base_oid: "blake3:readmecontentaddress0001",
    viewer_may_edit: true,
    is_binary,
    size_bytes: content.length,
    is_truncated: false,
    raw_url: `/v1/git/repos/${repo}/raw/${ref}/${path}`,
    download_url: `/v1/git/repos/${repo}/download/${ref}/${path}`,
  };
}

/** GET raw/download bytes (R3.4) — returns `{ body, contentType, attachment }` (null = 404). */
export function rawBytes(repo, _ref, path, attachment) {
  if (repo !== "myelin") return null;
  const hit = walkTree(path);
  if (!hit || hit.kind !== "file") return null;
  const filename = path.split("/").pop() || "download";
  return {
    body: hit.node.file ?? "",
    contentType: hit.node.binary ? "application/octet-stream" : "text/plain; charset=utf-8",
    disposition: `${attachment ? "attachment" : "inline"}; filename="${filename}"`,
  };
}

// A real two-commit history for acme/myelin (CommitRow::to_json).
const C1 = "a1b2c3d4e5f60718293a4b5c6d7e8f9001122334";
const C2 = "b2c3d4e5f60718293a4b5c6d7e8f900112233445";
const SEED_COMMITS = {
  myelin: [
    {
      oid: C2,
      short_oid: C2.slice(0, 12),
      summary: "docs: expand the README",
      author: "u_dev_operator@acme.noreply",
      committed_at: 1719446400,
      parents: [C1],
    },
    {
      oid: C1,
      short_oid: C1.slice(0, 12),
      summary: "feat: land the make-it-real spine",
      author: "u_dev_operator@acme.noreply",
      committed_at: 1719360000,
      parents: [],
    },
  ],
};

/** GET /v1/git/repos/{repo}/commits/{ref} → the `{items,page}` commit-log envelope (null = 404). R3.4:
 *  bidirectional cursor + honest range/page position (no fabricated total). */
export function commitsEnvelope(repo, limit = 50, cursor) {
  const all = SEED_COMMITS[repo];
  if (!all) return null;
  const offset = Number.parseInt(cursor ?? "0", 10) || 0;
  const items = all.slice(offset, offset + limit);
  const next = offset + limit < all.length ? String(offset + limit) : null;
  const prev = offset > 0 ? String(Math.max(0, offset - limit)) : null;
  return {
    items,
    page: {
      next_cursor: next,
      prev_cursor: prev,
      limit,
      offset,
      range: { from: items.length ? offset + 1 : 0, to: offset + items.length },
    },
  };
}

// The per-commit diff (CommitDiff::to_json).
const SEED_DIFFS = {
  [C2]: {
    oid: C2,
    short_oid: C2.slice(0, 12),
    summary: "docs: expand the README",
    message: "docs: expand the README\n\nAdd the spine tagline.",
    author: "u_dev_operator@acme.noreply",
    committed_at: 1719446400,
    parents: [C1],
    files: [
      {
        path: "README.md",
        old_path: null,
        status: "M",
        lines: [
          { origin: " ", content: "# acme/myelin" },
          { origin: " ", content: "" },
          { origin: "+", content: "The make-it-real spine." },
        ],
      },
    ],
  },
  [C1]: {
    oid: C1,
    short_oid: C1.slice(0, 12),
    summary: "feat: land the make-it-real spine",
    message: "feat: land the make-it-real spine",
    author: "u_dev_operator@acme.noreply",
    committed_at: 1719360000,
    parents: [],
    files: [
      {
        path: "README.md",
        old_path: null,
        status: "A",
        lines: [{ origin: "+", content: "# acme/myelin" }],
      },
    ],
  },
};

/** GET /v1/git/repos/{repo}/commit/{oid} → the CommitDiff ViewModel (null = 404). */
export function commitDiffJson(repo, oid) {
  return SEED_DIFFS[oid] ?? null;
}

// ── R3.2 · G-7 — the PR three-dot diff + expand-context fixtures ──────────────────────────────────
const B_OLD = "0000000000000000000000000000000000000001";
const B_NEW = "0000000000000000000000000000000000000002";
const SEED_PR_DIFFS = {
  1: {
    number: 1,
    base_ref: "refs/heads/main",
    base_oid: C1,
    short_base_oid: C1.slice(0, 7),
    head_oid: C2,
    short_head_oid: C2.slice(0, 7),
    three_dot: true,
    files: [
      {
        path: "src/list_filter.rs",
        old_path: null,
        status: "M",
        kind: "text",
        additions: 2,
        deletions: 1,
        size_bytes: null,
        hunks: [
          {
            header: "@@ -1,3 +1,4 @@ impl ListFilter {",
            old_start: 1,
            old_lines: 3,
            new_start: 1,
            new_lines: 4,
            lines: [
              { origin: " ", content: "impl ListFilter {", old_no: 1, new_no: 1 },
              { origin: "-", content: "    let cap = 50;", old_no: 2, new_no: null },
              { origin: "+", content: "    let cap = self.limit.min(100);", old_no: null, new_no: 2 },
              { origin: " ", content: "    let cursor = 0;", old_no: 3, new_no: 3 },
              { origin: "+", content: "    debug_assert!(cap > 0);", old_no: null, new_no: 4 },
            ],
          },
        ],
        deleted_body_available: false,
        truncated: false,
      },
      {
        path: "assets/logo.png",
        old_path: null,
        status: "A",
        kind: "binary",
        additions: 0,
        deletions: 0,
        size_bytes: 20480,
        hunks: [],
        deleted_body_available: false,
        truncated: false,
      },
    ],
    restricted_files: 0,
    total_files: 2,
    total_additions: 2,
    total_deletions: 1,
    page: { next_cursor: null, limit: 50 },
  },
};
// A pre-seeded anchored thread + a rebase-orphan (outdated) thread on PR #1's diff — anchored threads
// (anchor != null) never appear in the overview's discussion, so this is safe across surfaces.
const SEED_ANCHORED = {
  1: [
    {
      id: "seed-t1",
      anchor: { path: "src/list_filter.rs", line: 2, anchor_state: "live" },
      resolved: false,
      comments: [{ id: "seed-c1", author: { kind: "human", display: "u_dev_operator@acme.noreply", on_behalf_of: null, trigger: null }, body_md: "Clamp looks right — nice.", created_at: 1719450500, edited_at: null, state: "visible", review_id: null, pending: false }],
    },
    {
      id: "seed-t2",
      anchor: { path: "src/list_filter.rs", line: 87, anchor_state: "outdated" },
      resolved: false,
      comments: [{ id: "seed-c2", author: { kind: "human", display: "u_dev_operator@acme.noreply", on_behalf_of: null, trigger: null }, body_md: "This was flagged before the rebase.", created_at: 1719450400, edited_at: null, state: "visible", review_id: null, pending: false }],
    },
  ],
};

/** GET /v1/git/repos/{repo}/prs/{n}/diff?cursor= → the PrDiffVM (null = 404). PR #4 pages by cursor. */
export function prDiffJson(repo, n, cursor) {
  if (repo !== "myelin" || !SEED_PRS[n]) return null;
  if (n === 4) return pagedDiff(cursor);
  return SEED_PR_DIFFS[n] ?? {
    number: n,
    base_ref: SEED_PRS[n].pr.base_ref,
    base_oid: C1,
    short_base_oid: C1.slice(0, 7),
    head_oid: SEED_PRS[n].pr.head_oid,
    short_head_oid: SEED_PRS[n].pr.head_oid.slice(0, 7),
    three_dot: true,
    files: [],
    restricted_files: 0,
    total_files: 0,
    total_additions: 0,
    total_deletions: 0,
    page: { next_cursor: null, limit: 50 },
  };
}

/** GET /v1/git/repos/{repo}/file-lines/{oid} → expand-context lines (context, origin " "). */
export function fileLinesJson(repo, oid, start, end) {
  if (repo !== "myelin") return null;
  const lines = [];
  for (let i = start; i <= (end || start + 10); i++) {
    lines.push({ origin: " ", content: `    // context line ${i}`, old_no: null, new_no: i });
  }
  return { lines };
}

// Two PRs: #1 blocked (a required check not green + an untrusted-fork run), #2 ready.
const SEED_PRS = {
  1: {
    pr: {
      number: 1,
      pr_state: "open",
      title: "R3.3 PR overview + context pane",
      body_md: "This wires the **context pane** and the checks panel.\n\nThe gate is authoritative.",
      base_ref: "refs/heads/main",
      head_ref: "refs/heads/feature",
      head_oid: C2,
      author: "u_dev_operator@acme.noreply",
      author_is_agent: false,
      reviews: 0,
      created_at: 1719360000,
      updated_at: 1719446400,
      commits_count: 2,
      commits_count_capped: false,
      durable: true,
    },
    checks: {
      required_contexts: ["ci/build", "ci/test"],
      required_approvals: 1,
      green_contexts: ["ci/build"],
      endorsed_contexts: [],
      fork_unendorsed_contexts: ["ci/test"],
      gate_admitted: false,
      changes_requested: false,
      current_approvals: 0,
      durable: true,
    },
  },
  2: {
    pr: {
      number: 2,
      pr_state: "open",
      title: "Hotfix: saturating cursor arithmetic",
      body_md: null,
      base_ref: "refs/heads/main",
      head_ref: "refs/heads/hotfix",
      head_oid: C1,
      author: "u_dev_operator@acme.noreply",
      author_is_agent: false,
      reviews: 1,
      created_at: 1719100000,
      updated_at: 1719200000,
      commits_count: 1,
      commits_count_capped: false,
      durable: true,
    },
    checks: {
      required_contexts: ["ci/build"],
      required_approvals: 0,
      green_contexts: ["ci/build"],
      endorsed_contexts: [],
      fork_unendorsed_contexts: [],
      gate_admitted: true,
      changes_requested: false,
      current_approvals: 0,
      durable: true,
    },
  },
  // PR #3 — the checks-degrade fixture: the record exists, its checks route 404s (see prChecksJson).
  3: {
    pr: {
      number: 3,
      pr_state: "open",
      title: "Checks-degrade fixture",
      body_md: null,
      base_ref: "refs/heads/main",
      head_ref: "refs/heads/degrade",
      head_oid: C2,
      author: "u_dev_operator@acme.noreply",
      author_is_agent: false,
      reviews: 0,
      created_at: 1719000000,
      updated_at: 1719000000,
      commits_count: 1,
      commits_count_capped: false,
      durable: true,
    },
    // `checks` is intentionally never served for #3 (prChecksJson 404s) — the local-degrade path.
    checks: { required_contexts: [], required_approvals: 0, green_contexts: [], endorsed_contexts: [], fork_unendorsed_contexts: [], gate_admitted: false, durable: true },
  },
  // PR #4 — the >50-file paging fixture: its diff is served in two pages via the MR-014 file cursor
  // (see prDiffJson). Proves "Load remaining files" actually pages (finding #15).
  4: {
    pr: {
      number: 4,
      pr_state: "open",
      title: "Big refactor across many files",
      body_md: null,
      base_ref: "refs/heads/main",
      head_ref: "refs/heads/big",
      head_oid: C2,
      author: "u_dev_operator@acme.noreply",
      author_is_agent: false,
      reviews: 0,
      created_at: 1718900000,
      updated_at: 1718900000,
      commits_count: 1,
      commits_count_capped: false,
      durable: true,
    },
    checks: { required_contexts: [], required_approvals: 0, green_contexts: [], endorsed_contexts: [], fork_unendorsed_contexts: [], gate_admitted: false, durable: true },
  },
};

// PR #4's paged diff: 60 changed files served 50-per-page via the file cursor. Page 1 (no cursor)
// carries files 0–49 + `next_cursor: "c50"`; page 2 (?cursor=c50) carries files 50–59 + no cursor.
const PAGED_TOTAL_FILES = 60;
const PAGED_PAGE = 50;
function pagedFile(i) {
  const idx = String(i).padStart(3, "0");
  return {
    path: `src/paged/file_${idx}.txt`,
    old_path: null,
    status: "A",
    kind: "text",
    additions: 1,
    deletions: 0,
    size_bytes: null,
    hunks: [
      {
        header: `@@ -0,0 +1 @@ file_${idx}`,
        old_start: 0,
        old_lines: 0,
        new_start: 1,
        new_lines: 1,
        lines: [{ origin: "+", content: `line for file ${idx}`, old_no: null, new_no: 1 }],
      },
    ],
    deleted_body_available: false,
    truncated: false,
  };
}
function pagedDiff(cursor) {
  const start = cursor === "c50" ? PAGED_PAGE : 0;
  const end = Math.min(start + PAGED_PAGE, PAGED_TOTAL_FILES);
  const files = [];
  for (let i = start; i < end; i++) files.push(pagedFile(i));
  return {
    number: 4,
    base_ref: "refs/heads/main",
    base_oid: C1,
    short_base_oid: C1.slice(0, 7),
    head_oid: C2,
    short_head_oid: C2.slice(0, 7),
    three_dot: true,
    files,
    restricted_files: 0,
    total_files: PAGED_TOTAL_FILES,
    total_additions: PAGED_TOTAL_FILES,
    total_deletions: 0,
    page: { next_cursor: end < PAGED_TOTAL_FILES ? "c50" : null, limit: PAGED_PAGE },
  };
}

// ── R3.3 — a per-process mutable thread/review store (the dev-edge stateful surface the e2e drives).
//    Keyed by `${repo}:${n}`; a fresh process starts empty (an honest "No discussion yet"). ──
const THREADS = new Map();
let threadSeq = 0;
function subjectKey(repo, n) {
  return `${repo}:${n}`;
}
function subjectDoc(repo, n) {
  const k = subjectKey(repo, n);
  if (!THREADS.has(k)) THREADS.set(k, { threads: [], reviews: [] });
  return THREADS.get(k);
}
/** GET …/threads → the viewer-scoped envelope (the dev-edge treats the dev operator as the viewer, so
 *  pending comments authored by them are visible to them; a real edge filters per principal). */
export function prThreadsJson(repo, n, viewer = "u_dev_operator@acme.noreply") {
  if (repo !== "myelin" || !SEED_PRS[n]) return null;
  const doc = subjectDoc(repo, n);
  const visible = doc.threads
    .map((t) => ({
      ...t,
      comments: t.comments.filter((c) => !c.pending || c.author.display === viewer),
    }))
    .filter((t) => t.comments.length > 0);
  const reviews = doc.reviews.filter((r) => r.submitted_at != null || r.reviewer.display === viewer);
  // The diff surface's pre-seeded anchored threads (live + rebase-orphan) merge in read-only.
  const withSeed = [...(SEED_ANCHORED[n] ?? []), ...visible];
  return {
    discussion: withSeed.filter((t) => t.anchor == null),
    anchored: withSeed.filter((t) => t.anchor != null),
    threads: withSeed,
    reviews,
    durable: true,
  };
}
/** GET …/prs/{n}/commits → the MR-014 commits envelope. */
export function prCommitsEnvelope(repo, n, limit = 50) {
  if (repo !== "myelin" || !SEED_PRS[n]) return null;
  const items = [
    { oid: C2, short_oid: C2.slice(0, 7), summary: "Wire the context pane region", author: "u_dev_operator@acme.noreply", committed_at: 1719360000, parents: [C1] },
  ];
  return { items, page: { next_cursor: null, limit, offset: 0 } };
}
/** POST handlers (stateful). Return the same `{ applied: … }` envelope the edge does. */
export function devPost(repo, n, tail, body, viewer = "u_dev_operator@acme.noreply") {
  if (repo !== "myelin" || !SEED_PRS[n]) return { status: 404 };
  const doc = subjectDoc(repo, n);
  const who = { kind: "human", display: viewer, on_behalf_of: null, trigger: null };
  const id = (p) => `${p}-${++threadSeq}`;
  // POST …/threads
  if (tail === "threads") {
    // A line-anchored comment carries `{ anchor: { path, line, side? } }`; a discussion comment has none.
    const anchor = body.anchor ? { path: body.anchor.path, line: body.anchor.line ?? null, anchor_state: "live" } : null;
    const thread = { id: id("t"), anchor, resolved: false, comments: [
      { id: id("c"), author: who, body_md: body.body_md, created_at: 1719450000, edited_at: null, state: "visible", review_id: null, pending: false },
    ] };
    doc.threads.push(thread);
    return { status: 201, json: { applied: { action: "git.pr.thread.create", thread }, durable: true } };
  }
  // POST …/threads/{tid}/comments
  let m;
  if ((m = tail.match(/^threads\/([^/]+)\/comments$/))) {
    const t = doc.threads.find((x) => x.id === m[1]);
    if (!t) return { status: 404 };
    const comment = { id: id("c"), author: who, body_md: body.body_md, created_at: 1719450001, edited_at: null, state: "visible", review_id: null, pending: false };
    t.comments.push(comment);
    return { status: 201, json: { applied: { action: "git.pr.comment.create", comment }, durable: true } };
  }
  // POST …/reviews/start
  if (tail === "reviews/start") {
    const review = { id: id("r"), reviewer: who, verdict: "in_progress", advisory: false, submitted_at: null, summary_md: null };
    doc.reviews.push(review);
    return { status: 201, json: { applied: { action: "git.pr.review.start", review }, durable: true } };
  }
  // POST …/reviews/{rid}/comments
  if ((m = tail.match(/^reviews\/([^/]+)\/comments$/))) {
    const thread = { id: id("t"), anchor: null, resolved: false, comments: [
      { id: id("c"), author: who, body_md: body.body_md, created_at: 1719450002, edited_at: null, state: "visible", review_id: m[1], pending: true },
    ] };
    doc.threads.push(thread);
    return { status: 201, json: { applied: { action: "git.pr.review.comment", comment: thread.comments[0] }, durable: true } };
  }
  // POST …/reviews/{rid}/submit
  if ((m = tail.match(/^reviews\/([^/]+)\/submit$/))) {
    const r = doc.reviews.find((x) => x.id === m[1]);
    if (!r) return { status: 404 };
    const first = r.submitted_at == null;
    if (first) {
      r.verdict = body.verdict ?? "commented";
      r.submitted_at = 1719450003;
      r.summary_md = body.summary_md ?? null;
      for (const t of doc.threads) for (const c of t.comments) if (c.review_id === r.id) c.pending = false;
    }
    return { status: 200, json: { applied: { action: "git.pr.review.submit", result: { emitted: first, review: r } }, durable: true } };
  }
  // POST …/reviews/{rid}/discard
  if ((m = tail.match(/^reviews\/([^/]+)\/discard$/))) {
    doc.threads = doc.threads.filter((t) => (t.comments = t.comments.filter((c) => c.review_id !== m[1])).length > 0);
    doc.reviews = doc.reviews.filter((r) => r.id !== m[1]);
    return { status: 200, json: { applied: { action: "git.pr.review.discard", result: { discarded: m[1] } }, durable: true } };
  }
  // POST …/merge — PR 2 is mergeable (gate admitted); PR 1 is blocked (409 + fresh checks, N6).
  if (tail === "merge") {
    if (SEED_PRS[n].checks.gate_admitted) {
      return { status: 200, json: { applied: { action: "git.pr.merge", merged: true, base_ref: SEED_PRS[n].pr.base_ref, new_oid: SEED_PRS[n].pr.head_oid }, durable: true } };
    }
    return { status: 409, json: { error: { code: "merge_blocked", message: "merge blocked by branch protection" }, checks: SEED_PRS[n].checks, durable: true } };
  }
  return { status: 404 };
}

/** GET /v1/git/repos/{repo}/prs/{n} → the durable PR record (null = 404). */
export function prJson(repo, n) {
  return repo === "myelin" && SEED_PRS[n] ? SEED_PRS[n].pr : null;
}

/** GET /v1/git/repos/{repo}/prs/{n}/checks → the checks + merge-gate projection (null = 404). PR #3
 *  deliberately 404s its checks (the record exists) so the e2e can drive the LOCAL checks-degrade
 *  state (the PR stays live around a "Checks unavailable" region — ux-git finding 5). */
export function prChecksJson(repo, n) {
  if (repo === "myelin" && n === 3) return null;
  return repo === "myelin" && SEED_PRS[n] ? SEED_PRS[n].checks : null;
}

// ── R3.1 PR-list rows (PrListRowVM) — a spread that exercises the states the screens render:
// open/draft/merged, agent + human authors, pass/running/none checks, a title + a legacy #number
// fallback (title: null), and a "review requested" row for the cross-repo bucket. ──
const SEED_PR_ROWS = [
  {
    number: 48, title: "R2.4 MCP HITL server-side verdicts", pr_state: "open",
    base_ref: "refs/heads/main", head_ref: "refs/heads/feat/mcp-hitl-verdicts",
    author: "u_dev_operator@acme.noreply", author_is_agent: false,
    reviews: 2, review_state: "none", you_are_requested: false,
    checks_summary: { verdict: "running", passing: 4, failing: 0, total: 5 },
    updated_at: 1719446400, repo: "myelin/myelin",
  },
  {
    number: 46, title: "AuthzScanner: eliminate 2 residual reach-arounds", pr_state: "open",
    base_ref: "refs/heads/main", head_ref: "refs/heads/agent/scanner-reach-arounds",
    author: "AuthzScanner@acme.noreply", author_is_agent: true,
    reviews: 0, review_state: "requested", you_are_requested: true,
    checks_summary: { verdict: "pass", passing: 5, failing: 0, total: 5 },
    updated_at: 1719360000, repo: "myelin/myelin",
  },
  {
    number: 44, title: "R2.5 Grobkörnige Berechtigungsprüfung entfernen", pr_state: "draft",
    base_ref: "refs/heads/main", head_ref: "refs/heads/refactor/coarse-authz-removal",
    author: "j_voegel@acme.noreply", author_is_agent: false,
    reviews: 0, review_state: "none", you_are_requested: false,
    checks_summary: { verdict: "none", passing: 0, failing: 0, total: 0 },
    updated_at: 1719273600, repo: "myelin/myelin",
  },
  {
    number: 39, title: null, pr_state: "merged",
    base_ref: "refs/heads/main", head_ref: "refs/heads/chore/scanner-true-zero",
    author: "u_dev_operator@acme.noreply", author_is_agent: false,
    reviews: 3, review_state: "approved", you_are_requested: false,
    checks_summary: { verdict: "pass", passing: 1, failing: 0, total: 1 },
    updated_at: 1719100000, repo: "myelin/myelin",
  },
];

function countBy(rows) {
  const open = rows.filter((r) => r.pr_state === "open" || r.pr_state === "draft").length;
  const merged = rows.filter((r) => r.pr_state === "merged").length;
  const closed = rows.filter((r) => r.pr_state === "closed").length;
  const yours = rows.filter((r) => r.author === "u_dev_operator@acme.noreply").length;
  const needs_review = rows.filter((r) => r.you_are_requested).length;
  return { open, merged, closed, all: rows.length, yours, needs_review };
}

/** GET /v1/git/repos/{repo}/prs?state=&sort= → the PrListPage (null = 404, the no-access analogue). */
export function repoPrsEnvelope(repo, state = "open", limit = 50) {
  // `sandbox` (the seeded empty repo) → an empty list (the teaching empty state); `myelin` → the seed;
  // anything else → a 0-leak 404 (the no-access / absent-repo analogue).
  if (repo === "sandbox") {
    return {
      items: [],
      page: { next_cursor: null, prev_cursor: null, limit, offset: 0, total: 0 },
      counts: { open: 0, merged: 0, closed: 0, all: 0, yours: 0, needs_review: 0 },
    };
  }
  if (repo !== "myelin") return null;
  const counts = countBy(SEED_PR_ROWS);
  const wanted =
    state === "merged" ? ["merged"]
    : state === "closed" ? ["closed"]
    : state === "all" ? ["draft", "open", "merged", "closed"]
    : ["draft", "open"];
  const items = SEED_PR_ROWS.filter((r) => wanted.includes(r.pr_state));
  return {
    items,
    page: { next_cursor: null, prev_cursor: null, limit, offset: 0, total: items.length },
    counts,
  };
}

/** GET /v1/git/prs?bucket=needs-review|yours → the cross-repo PrListPage (never 404 — empty if none). */
export function myPrsEnvelope(bucket = "needs-review", limit = 50) {
  const items =
    bucket === "yours"
      ? SEED_PR_ROWS.filter((r) => r.author === "u_dev_operator@acme.noreply" && r.pr_state !== "closed")
      : SEED_PR_ROWS.filter((r) => r.you_are_requested && r.author !== "u_dev_operator@acme.noreply");
  return {
    items,
    page: { next_cursor: null, prev_cursor: null, limit, offset: 0, total: items.length },
    counts: { bucket: items.length },
  };
}

/** The edge's `{error:{message, code}}` envelope (error.rs) — a uniform 404. */
export function notFoundEnvelope(what) {
  return { error: { message: `${what} not found`, code: "not_found" } };
}

// The edge's `{error:{message, code}}` envelope (error.rs) — uniform, oracle-free 401.
export function unauthorizedEnvelope() {
  return { error: { message: "authentication required", code: "unauthorized" } };
}
