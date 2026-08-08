// THE DEV-SEAM CONTRACT (shared by the dev edge + the dev-login seam) — clearly marked, NOT production.
//
// Why this exists: the real `myelin-edge` binary authenticates with signed capabilities and a seeded
// S1 principal directory, while hermetic UI tests intentionally run without an external IdP. The dev
// edge therefore accepts ONE well-known dev token and the explicitly guarded dev-login seam mints a
// session carrying it. The gateway client + session machinery remain contract-faithful; only this
// local authentication fixture is a stand-in. NEVER ship it.
//
// The data the dev edge serves mirrors both REAL Git contracts: summary-only catalogue rows and the
// enriched object-addressed RepoHome. Keeping those fixtures distinct prevents either shape from
// accidentally satisfying the other's decoder.

import { createHash } from "node:crypto";

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
    // Mirror the production whoami contract: the web session must be bounded by the presented
    // capability's expiry. This dev-only token is treated as a fresh one-hour capability per call.
    expires_at: Math.floor(Date.now() / 1_000) + 60 * 60,
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
  "A.txt": { file: "ASCII uppercase\n" },
  "a.txt": { file: "ASCII lowercase\n" },
  "e\u0301.txt": { file: "decomposed accent\n" },
  "é.txt": { file: "composed accent\n" },
  "😀.txt": { file: "emoji\n" },
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

// Full RepoHome fixtures belong only to the object-addressed home endpoint. Catalogue fixtures below
// are intentionally separate so an enriched home cannot mask a summary-contract regression.
export const SEED_REPO_HOMES = [
  {
    state: "populated",
    slug: "acme/myelin",
    default_branch: "main",
    readme: MYELIN_TREE["README.md"].file,
    readme_excerpt: "# acme/myelin\n\nThe make-it-real spine.",
    clone_url: "/acme/eu-west/myelin.git",
    latest_commit: LATEST,
    counts: { branches: 2, tags: 1 },
    entries: [
      { name: "crates", path: "crates", is_dir: true, latest_commit: LATEST },
      { name: "Cargo.toml", path: "Cargo.toml", is_dir: false, size: 34, latest_commit: LATEST },
      { name: "README.md", path: "README.md", is_dir: false, size: 120, latest_commit: LATEST },
    ],
    snapshot_oid: LATEST.oid,
    entries_page: {
      ref: "refs/heads/main",
      next_cursor: null,
      limit: 100,
      snapshot_oid: LATEST.oid,
    },
  },
  {
    state: "empty",
    slug: "acme/sandbox",
    default_branch: "main",
    clone_url: "/acme/eu-west/sandbox.git",
    counts: { branches: 0, tags: 0 },
  },
];

export const SEED_REPO_SUMMARIES = [
  {
    state: "populated",
    slug: "acme/myelin",
    clone_url: "/acme/eu-west/myelin.git",
  },
  {
    state: "empty",
    slug: "acme/sandbox",
  },
];

// Legacy no-view catalogue fixture retained while old clients migrate to the summary projection.
export function reposEnvelope(limit = 50) {
  return { items: SEED_REPO_HOMES, page: { next_cursor: null, limit } };
}

const REPO_LIST_QUERY_MAX_BYTES = 16 * 1024;
const REPO_LIST_CURSOR_MAX_BYTES = 512;

function repoListCursor(slug) {
  return `rl1_${Buffer.from(slug, "utf8").toString("base64url")}`;
}

function decodeRepoListCursor(cursor) {
  if (typeof cursor !== "string" || textEncoder.encode(cursor).byteLength > REPO_LIST_CURSOR_MAX_BYTES ||
      !/^rl1_[A-Za-z0-9_-]+$/.test(cursor)) return null;
  try {
    const encoded = cursor.slice("rl1_".length);
    const bytes = Buffer.from(encoded, "base64url");
    if (bytes.toString("base64url") !== encoded) return null;
    const slug = bytes.toString("utf8");
    if (!slug || Buffer.from(slug, "utf8").compare(bytes) !== 0 ||
        !slug.split("/").every((part) => part && part !== "." && part !== ".." &&
          /^[A-Za-z0-9._-]+$/.test(part))) return null;
    return slug;
  } catch {
    return null;
  }
}

/** Strict summary-list query grammar: exact view, canonical decimal limit, canonical rl1 cursor. */
export function parseRepoSummaryQuery(rawQuery) {
  if (typeof rawQuery !== "string" ||
      textEncoder.encode(rawQuery).byteLength > REPO_LIST_QUERY_MAX_BYTES) return null;
  const parsed = {};
  for (const pair of rawQuery.split("&")) {
    const equals = pair.indexOf("=");
    if (equals <= 0) return null;
    const name = decodeQueryComponent(pair.slice(0, equals));
    const value = decodeQueryComponent(pair.slice(equals + 1));
    if (name === null || value === null || !["view", "limit", "cursor"].includes(name) ||
        Object.hasOwn(parsed, name)) return null;
    parsed[name] = value;
  }
  if (parsed.view !== "summary") return null;
  const limit = parsed.limit === undefined ? 50 : Number(parsed.limit);
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100 ||
      (parsed.limit !== undefined && String(limit) !== parsed.limit)) return null;
  if (parsed.cursor !== undefined && decodeRepoListCursor(parsed.cursor) === null) return null;
  return { limit, ...(parsed.cursor === undefined ? {} : { cursor: parsed.cursor }) };
}

export function repoSummaryEnvelope(options = {}) {
  const limit = options.limit ?? 50;
  const after = options.cursor === undefined ? null : decodeRepoListCursor(options.cursor);
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100 ||
      (options.cursor !== undefined && after === null)) return null;
  const sorted = [...SEED_REPO_SUMMARIES].sort((left, right) =>
    left.slug < right.slug ? -1 : left.slug > right.slug ? 1 : 0);
  const remaining = after === null ? sorted : sorted.filter((row) => row.slug > after);
  const items = remaining.slice(0, limit);
  const next = remaining.length > items.length && items.length > 0
    ? repoListCursor(items.at(-1).slug)
    : null;
  return { items, page: { next_cursor: next, limit } };
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
  return SEED_REPO_HOMES.find((r) => bareName(r.slug) === repo) ?? null;
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
    .sort((a, b) => (a.is_dir === b.is_dir ? compareUtf8(a.name, b.name) : a.is_dir ? -1 : 1));
}

/** Git names are byte strings. The product Edge orders valid UTF-8 names by their UTF-8 bytes. */
export function compareUtf8(left, right) {
  return Buffer.from(left, "utf8").compare(Buffer.from(right, "utf8"));
}

export const SEED_REFS = [
  { kind: "branch", full_name: "refs/heads/main", name: "main", oid: LATEST.oid, is_default: true },
  { kind: "branch", full_name: "refs/heads/feature", name: "feature", oid: "a1b2c3d4e5f60718293a4b5c6d7e8f9001122334", is_default: false },
  { kind: "branch", full_name: "refs/heads/A", name: "A", oid: LATEST.oid, is_default: false },
  { kind: "branch", full_name: "refs/heads/a", name: "a", oid: LATEST.oid, is_default: false },
  { kind: "branch", full_name: "refs/heads/e\u0301", name: "e\u0301", oid: LATEST.oid, is_default: false },
  { kind: "branch", full_name: "refs/heads/é", name: "é", oid: LATEST.oid, is_default: false },
  { kind: "branch", full_name: "refs/heads/😀", name: "😀", oid: LATEST.oid, is_default: false },
  { kind: "tag", full_name: "refs/tags/v0.1", name: "v0.1", oid: LATEST.oid, is_default: false },
];

function compareRefs(left, right) {
  if (left.kind !== right.kind) return left.kind === "branch" ? -1 : 1;
  return compareUtf8(left.name, right.name) || compareUtf8(left.full_name, right.full_name);
}

function refsSnapshot(refs) {
  return createHash("sha256")
    .update(JSON.stringify([...refs].sort(compareRefs)
      .map(({ kind, full_name, oid, is_default }) => [kind, full_name, oid, is_default])))
    .digest("hex");
}

function refsScope(repo, query) {
  return createHash("sha256")
    .update(`myelin.git.refs.scope.v1\0${repo}\0${query}`)
    .digest("hex");
}

function refsCursor(repo, query, snapshot, row) {
  const frame = JSON.stringify([1, snapshot, refsScope(repo, query), row.kind, row.full_name]);
  return `gr1_${Buffer.from(frame, "utf8").toString("base64url")}`;
}

function decodeRefsCursor(cursor) {
  if (typeof cursor !== "string" || !/^gr1_[A-Za-z0-9_-]+$/.test(cursor)) return null;
  try {
    const encoded = cursor.slice("gr1_".length);
    const bytes = Buffer.from(encoded, "base64url");
    if (bytes.toString("base64url") !== encoded) return null;
    const frame = JSON.parse(bytes.toString("utf8"));
    if (!Array.isArray(frame) || frame.length !== 5 || frame[0] !== 1 ||
        frame.slice(1).some((value) => typeof value !== "string")) {
      return null;
    }
    return { snapshot: frame[1], scope: frame[2], kind: frame[3], fullName: frame[4] };
  } catch {
    return null;
  }
}

/**
 * GET /v1/git/repos/{repo}/refs → the paginated RefsVM (null = 404).
 * @param {string} repo
 * @param {{ limit?: number, cursor?: string, q?: string, current?: string }} [options]
 * @param {Array<object>} [namespace] Injectable only so the contract test can move the ref namespace.
 */
export function refsJson(repo, options = {}, namespace = SEED_REFS) {
  if (repo !== "myelin") return null;
  const { limit = 100, cursor, q = "", current } = options;
  if (!Number.isInteger(limit) || limit < 1 || limit > 100) {
    return { __status: 400 };
  }
  const needle = q.trim().toLowerCase();
  const refs = [...namespace].sort(compareRefs);
  const snapshot = refsSnapshot(refs);
  const matches = refs.filter((row) => !needle || row.name.toLowerCase().includes(needle));
  let offset = 0;
  if (cursor !== undefined) {
    const decoded = decodeRefsCursor(cursor);
    if (!decoded || decoded.scope !== refsScope(repo, needle)) {
      return { __status: 400 };
    }
    if (decoded.snapshot !== snapshot) return { __status: 409 };
    offset = matches.findIndex((row) =>
      row.kind === decoded.kind && row.full_name === decoded.fullName) + 1;
    if (offset === 0) return { __status: 400 };
  }
  const selected = matches.slice(offset, offset + limit);
  const next = offset + selected.length < matches.length && selected.length > 0
    ? refsCursor(repo, needle, snapshot, selected.at(-1))
    : null;
  const currentPin = refs.find((row) => row.full_name === current);
  const defaultPin = refs.find((row) => row.kind === "branch" && row.is_default);
  const pinned = [currentPin, defaultPin]
    .filter(Boolean)
    .filter((row, index, rows) => rows.findIndex((candidate) => candidate.full_name === row.full_name) === index);
  return {
    branches: selected
      .filter((row) => row.kind === "branch")
      .map(({ name, oid, is_default }) => ({ name, oid, is_default })),
    tags: selected.filter((row) => row.kind === "tag").map(({ name, oid }) => ({ name, oid })),
    default_branch: "main",
    pinned,
    page: { next_cursor: next, limit },
  };
}

const TREE_QUERY_MAX_BYTES = 16 * 1024;
const TREE_Q_MAX_BYTES = 256;
const TREE_CURSOR_MAX_BYTES = 8 * 1024;
const textEncoder = new TextEncoder();

function decodeQueryComponent(raw) {
  for (let index = 0; index < raw.length; index += 1) {
    if (raw[index] !== "%") continue;
    if (!/^[0-9a-fA-F]{2}$/.test(raw.slice(index + 1, index + 3))) return null;
    index += 2;
  }
  try {
    const decoded = decodeURIComponent(raw.replace(/\+/g, " "));
    return /\p{Cc}/u.test(decoded) ? null : decoded;
  } catch {
    return null;
  }
}

/** Strictly parse the raw query string with the same exact allowlist and bounds as myelin-edge. */
export function parseTreeQuery(rawQuery) {
  if (typeof rawQuery !== "string" || textEncoder.encode(rawQuery).byteLength > TREE_QUERY_MAX_BYTES) {
    return null;
  }
  if (!rawQuery) return { limit: 100 };
  const parsed = {};
  for (const pair of rawQuery.split("&")) {
    const equals = pair.indexOf("=");
    if (equals <= 0) return null;
    const name = decodeQueryComponent(pair.slice(0, equals));
    const value = decodeQueryComponent(pair.slice(equals + 1));
    if (name === null || value === null || !["limit", "cursor", "q"].includes(name) ||
        Object.hasOwn(parsed, name)) return null;
    if (name === "limit") {
      const limit = Number(value);
      if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100 || String(limit) !== value) {
        return null;
      }
      parsed.limit = limit;
    } else if (name === "cursor") {
      if (!value || textEncoder.encode(value).byteLength > TREE_CURSOR_MAX_BYTES) return null;
      parsed.cursor = value;
    } else {
      if (textEncoder.encode(value).byteLength > TREE_Q_MAX_BYTES) return null;
      parsed.q = value;
    }
  }
  return { limit: parsed.limit ?? 100, ...("cursor" in parsed ? { cursor: parsed.cursor } : {}),
    ...("q" in parsed ? { q: parsed.q } : {}) };
}

function treeCursor(offset, ref, path, query, snapshot = LATEST.oid) {
  const frame = JSON.stringify([offset, ref, path, query, snapshot]);
  return `gt1_${Buffer.from(frame).toString("base64url")}`;
}

function decodeTreeCursor(cursor) {
  if (typeof cursor !== "string" || textEncoder.encode(cursor).byteLength > TREE_CURSOR_MAX_BYTES ||
      !/^gt1_[A-Za-z0-9_-]+$/.test(cursor)) return null;
  try {
    const encoded = cursor.slice(4);
    const bytes = Buffer.from(encoded, "base64url");
    if (bytes.toString("base64url") !== encoded) return null;
    const frame = JSON.parse(bytes.toString("utf8"));
    if (!Array.isArray(frame) || frame.length !== 5 || !Number.isSafeInteger(frame[0]) ||
        frame[0] < 0 || frame.slice(1).some((value) => typeof value !== "string")) return null;
    return { offset: frame[0], ref: frame[1], path: frame[2], query: frame[3], snapshot: frame[4] };
  } catch {
    return null;
  }
}

/** GET /v1/git/repos/{repo}/tree/{ref}/{...path} → the modern paginated TreeVM. */
export function treeJson(repo, ref, path, options = {}) {
  if (repo !== "myelin") return null;
  const hit = walkTree(path);
  if (!hit) return { __status: 404 };
  if (hit.kind === "file") return { redirect_to_blob: true, ref, path };
  const base = (path ?? "").replace(/^\/+|\/+$/g, "");
  const limit = options.limit ?? 100;
  const normalizedQuery = (options.q ?? "").trim().toLowerCase();
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100 ||
      textEncoder.encode(options.q ?? "").byteLength > TREE_Q_MAX_BYTES ||
      /\p{Cc}/u.test(options.q ?? "")) return { __status: 400 };
  let offset = 0;
  if (options.cursor !== undefined) {
    const decoded = decodeTreeCursor(options.cursor);
    if (!decoded || decoded.ref !== ref || decoded.path !== base ||
        decoded.query !== normalizedQuery) return { __status: 400 };
    if (decoded.snapshot !== LATEST.oid) return { __status: 409 };
    offset = decoded.offset;
  }
  const matches = entriesOf(hit.node, base)
    .filter((entry) => !normalizedQuery || entry.name.toLowerCase().includes(normalizedQuery));
  if (offset > matches.length) return { __status: 400 };
  const entries = matches.slice(offset, offset + limit);
  const nextOffset = offset + entries.length;
  const next = nextOffset < matches.length
    ? treeCursor(nextOffset, ref, base, normalizedQuery)
    : null;
  return {
    ref,
    path: base,
    snapshot_oid: LATEST.oid,
    entries,
    ...(options.cursor === undefined && !normalizedQuery
      ? { readme: hit.node["README.md"]?.file ?? null }
      : {}),
    page: { next_cursor: next, limit },
  };
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

/** GET /v1/git/repos/{repo}/blame/{ref}/{...path} → snapshot-pinned line attribution. */
export function blameJson(repo, ref, path) {
  if (repo !== "myelin") return null;
  const hit = walkTree(path);
  if (!hit || hit.kind !== "file" || hit.node.binary) return { __status: 404 };
  const contents = hit.node.file ?? "";
  const lineCount = contents === "" ? 0 : contents.split("\n").length - Number(contents.endsWith("\n"));
  const oldCommit = {
    oid: C1,
    summary: "feat: land the make-it-real spine",
    author: "u_dev_operator@acme.noreply",
    committed_at: 1719360000,
  };
  const latestCommit = {
    oid: C2,
    summary: "docs: expand the README",
    author: "u_dev_operator@acme.noreply",
    committed_at: 1719446400,
  };
  const hunks = path === "README.md"
    ? [
        { start_line: 1, line_count: 2, commit: oldCommit },
        { start_line: 3, line_count: 1, commit: latestCommit },
        { start_line: 4, line_count: lineCount - 3, commit: oldCommit },
      ]
    : lineCount === 0 ? [] : [{ start_line: 1, line_count: lineCount, commit: oldCommit }];
  return { path, ref, snapshot_oid: C2, contents, hunks };
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
const BLOB_LIST_FILTER = "c3d4e5f60718293a4b5c6d7e8f90011223344556";
const BLOB_BINARY = "d4e5f60718293a4b5c6d7e8f9001122334455667";
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
        new_blob_oid: BLOB_LIST_FILTER,
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
          {
            header: "@@ -20,2 +20,2 @@ impl CursorWindow {",
            old_start: 20,
            old_lines: 2,
            new_start: 20,
            new_lines: 2,
            lines: [
              { origin: " ", content: "impl CursorWindow {", old_no: 20, new_no: 20 },
              { origin: " ", content: "    // bounded continuation", old_no: 21, new_no: 21 },
            ],
          },
        ],
        deleted_body_available: false,
        truncated: false,
      },
      {
        path: "assets/logo.png",
        old_path: null,
        new_blob_oid: BLOB_BINARY,
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
      id: "t-1001",
      anchor: { path: "src/list_filter.rs", line: 2, side: "new", base_oid: C1, head_oid: C2, anchor_state: "live" },
      resolved: false,
      comments: [{ id: "c-1001", author: { kind: "human", display: "u_dev_operator@acme.noreply", on_behalf_of: null, trigger: null }, body_md: "Clamp looks right — nice.", created_at: 1719450500, edited_at: null, state: "visible", review_id: null, pending: false }],
    },
    {
      id: "t-1002",
      anchor: { path: "src/list_filter.rs", line: 87, side: "new", base_oid: C1, head_oid: C2, anchor_state: "outdated" },
      resolved: false,
      comments: [{ id: "c-1002", author: { kind: "human", display: "u_dev_operator@acme.noreply", on_behalf_of: null, trigger: null }, body_md: "This was flagged before the rebase.", created_at: 1719450400, edited_at: null, state: "visible", review_id: null, pending: false }],
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

/** Production's bounded PR-diff capacity envelope. The shared golden artifact pins these bytes. */
export function prDiffCapacityEnvelope() {
  return {
    error: {
      message: "pull request diff exceeds the interactive file limit",
      code: "payload_too_large",
    },
  };
}

/** GET /v1/git/repos/{repo}/file-lines/{oid} → expand-context lines (context, origin " "). */
export function fileLinesJson(repo, oid, start, end) {
  if (repo !== "myelin" || oid !== BLOB_LIST_FILTER) return null;
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
      commits_count: 23,
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
  // PR #5 — the production interactive diff ceiling. The route returns the shared golden 413
  // envelope rather than fabricating a partial diff or crashing the browser route.
  5: {
    pr: {
      number: 5,
      pr_state: "open",
      title: "Source snapshot beyond the interactive diff ceiling",
      body_md: null,
      base_ref: "refs/heads/main",
      head_ref: "refs/heads/source-snapshot",
      head_oid: C2,
      author: "u_dev_operator@acme.noreply",
      author_is_agent: false,
      reviews: 0,
      created_at: 1718800000,
      updated_at: 1718800000,
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
    new_blob_oid: BLOB_LIST_FILTER,
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
/** Reset only the mutable PR discussion/review fixtures. The dev edge exposes this through its
 *  test-control route so Playwright retries and repeated local suites start from the same state. */
export function resetPrFixtures() {
  THREADS.clear();
  threadSeq = 0;
}
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
const PR_COMMIT_CURSOR_PREFIX = "pc1_";
const PR_COMMIT_CURSOR_FRAME_BYTES = 78;
const PR_COMMIT_CURSOR_MAX_BYTES = 256;
const PR_COMMIT_CURSOR_MAX_POSITION = 100_000;
const PR_COMMIT_QUERY_MAX_BYTES = 16 * 1024;

function prCommitScope(repo, n) {
  return createHash("sha256").update(`myelin-dev-pr-commits\0${repo}\0${n}`).digest();
}

function oidBytes(oid) {
  return Buffer.from(oid, "hex");
}

function mintPrCommitCursor(repo, n, baseOid, headOid, position) {
  const frame = Buffer.alloc(PR_COMMIT_CURSOR_FRAME_BYTES);
  frame[0] = 1;
  prCommitScope(repo, n).copy(frame, 1);
  if (baseOid) {
    frame[33] = 1;
    oidBytes(baseOid).copy(frame, 34);
  }
  oidBytes(headOid).copy(frame, 54);
  frame.writeUInt32BE(position, 74);
  return `${PR_COMMIT_CURSOR_PREFIX}${frame.toString("base64url")}`;
}

function parsePrCommitCursor(value, repo, n) {
  if (typeof value !== "string" || value.length > PR_COMMIT_CURSOR_MAX_BYTES ||
      !value.startsWith(PR_COMMIT_CURSOR_PREFIX)) return null;
  const encoded = value.slice(PR_COMMIT_CURSOR_PREFIX.length);
  if (!encoded || !/^[A-Za-z0-9_-]+$/.test(encoded)) return null;
  let frame;
  try {
    frame = Buffer.from(encoded, "base64url");
  } catch {
    return null;
  }
  if (frame.length !== PR_COMMIT_CURSOR_FRAME_BYTES || frame[0] !== 1 ||
      frame.toString("base64url") !== encoded || !frame.subarray(1, 33).equals(prCommitScope(repo, n))) {
    return null;
  }
  const baseDiscriminator = frame[33];
  const baseBytes = frame.subarray(34, 54);
  let baseOid;
  if (baseDiscriminator === 0) {
    if (!baseBytes.equals(Buffer.alloc(20))) return null;
    baseOid = null;
  } else if (baseDiscriminator === 1) {
    baseOid = baseBytes.toString("hex");
  } else {
    return null;
  }
  const position = frame.readUInt32BE(74);
  if (position < 1 || position > PR_COMMIT_CURSOR_MAX_POSITION) return null;
  return {
    base_oid: baseOid,
    head_oid: frame.subarray(54, 74).toString("hex"),
    position,
  };
}

function prCommitRows(n) {
  if (n !== 1) {
    const head = SEED_PRS[n]?.pr.head_oid;
    return head ? [{
      oid: head,
      short_oid: head.slice(0, 12),
      summary: "Wire the context pane region",
      author: "u_dev_operator@acme.noreply",
      committed_at: 1719360000,
      parents: [C1],
    }] : [];
  }
  const olderOids = Array.from({ length: 22 }, (_, index) =>
    BigInt(0x1000 + index).toString(16).padStart(40, "0"));
  const oids = [C2, ...olderOids];
  return oids.map((oid, index) => ({
    oid,
    short_oid: oid.slice(0, 12),
    summary: index === 0 ? "Wire the context pane region" : `PR continuation commit ${23 - index}`,
    author: "u_dev_operator@acme.noreply",
    committed_at: 1719360000 - index,
    parents: [oids[index + 1] ?? C1],
  }));
}

/** Strict raw-query parser for the snapshot-paged PR commit route. */
export function parsePrCommitsQuery(repo, n, rawQuery) {
  if (typeof rawQuery !== "string" || rawQuery.length > PR_COMMIT_QUERY_MAX_BYTES) return null;
  let limit = 50;
  let cursor;
  let sawLimit = false;
  let sawCursor = false;
  if (rawQuery) {
    for (const pair of rawQuery.split("&")) {
      const separator = pair.indexOf("=");
      if (separator < 0) return null;
      const name = pair.slice(0, separator);
      const value = pair.slice(separator + 1);
      if (name === "limit") {
        if (sawLimit || !/^(?:[1-9]|[1-9][0-9]|100)$/.test(value)) return null;
        sawLimit = true;
        limit = Number(value);
      } else if (name === "cursor") {
        if (sawCursor) return null;
        sawCursor = true;
        cursor = value;
      } else {
        return null;
      }
    }
  }
  if (cursor !== undefined) {
    const snapshot = parsePrCommitCursor(cursor, repo, n);
    if (!snapshot) return null;
    return { limit, position: snapshot.position, snapshot };
  }
  return { limit, position: 0 };
}

/** GET …/prs/{n}/commits → the exact snapshot-paged MR-014 commits envelope. */
export function prCommitsEnvelope(repo, n, input) {
  if (repo !== "myelin" || !SEED_PRS[n]) return null;
  if (input.snapshot && (
    input.snapshot.base_oid !== C1 || input.snapshot.head_oid !== SEED_PRS[n].pr.head_oid
  )) return { expired: true };
  const rows = prCommitRows(n);
  const items = rows.slice(input.position, input.position + input.limit);
  const nextPosition = input.position + items.length;
  return {
    items,
    page: {
      next_cursor: nextPosition < rows.length
        ? mintPrCommitCursor(repo, n, C1, SEED_PRS[n].pr.head_oid, nextPosition)
        : null,
      limit: input.limit,
    },
  };
}

/** The production `EdgeError::Conflict` envelope for an unavailable pinned commit snapshot. */
export function prCommitCursorExpiredEnvelope() {
  return { error: { message: "pull request commit cursor expired", code: "conflict" } };
}

/** Mirror `PrOperationId::parse`: production trims one non-empty, bounded ASCII-graphic key. */
export function validPrOperationId(value) {
  if (typeof value !== "string") return false;
  const trimmed = value.trim();
  return trimmed.length > 0 && trimmed.length <= 128 && /^[\x21-\x7e]+$/.test(trimmed);
}

/** POST handlers (stateful). Return the same `{ applied: … }` envelope the edge does. */
export function devPost(repo, n, tail, body, viewer = "u_dev_operator@acme.noreply") {
  if (repo !== "myelin" || !SEED_PRS[n]) return { status: 404 };
  const doc = subjectDoc(repo, n);
  const who = { kind: "human", display: viewer, on_behalf_of: null, trigger: null };
  const id = (p) => `${p}-${++threadSeq}`;
  // POST …/threads
  if (tail === "threads") {
    // A line-anchored comment is bound to the displayed side and immutable revision pair.
    const anchor = body.anchor ? {
      path: body.anchor.path,
      line: body.anchor.line ?? null,
      side: body.anchor.side,
      base_oid: C1,
      head_oid: C2,
      anchor_state: "live",
    } : null;
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
    updated_at: 1719446400, repo: "myelin",
  },
  {
    number: 46, title: "AuthzScanner: eliminate 2 residual reach-arounds", pr_state: "open",
    base_ref: "refs/heads/main", head_ref: "refs/heads/agent/scanner-reach-arounds",
    author: "AuthzScanner@acme.noreply", author_is_agent: true,
    reviews: 0, review_state: "requested", you_are_requested: true,
    checks_summary: { verdict: "pass", passing: 5, failing: 0, total: 5 },
    updated_at: 1719360000, repo: "myelin",
  },
  {
    number: 44, title: "R2.5 Grobkörnige Berechtigungsprüfung entfernen", pr_state: "draft",
    base_ref: "refs/heads/main", head_ref: "refs/heads/refactor/coarse-authz-removal",
    author: "j_voegel@acme.noreply", author_is_agent: false,
    reviews: 0, review_state: "none", you_are_requested: false,
    checks_summary: { verdict: "none", passing: 0, failing: 0, total: 0 },
    updated_at: 1719273600, repo: "myelin",
  },
  {
    number: 39, title: null, pr_state: "merged",
    base_ref: "refs/heads/main", head_ref: "refs/heads/chore/scanner-true-zero",
    author: "u_dev_operator@acme.noreply", author_is_agent: false,
    reviews: 3, review_state: "approved", you_are_requested: false,
    checks_summary: { verdict: "pass", passing: 1, failing: 0, total: 1 },
    updated_at: 1719100000, repo: "myelin",
  },
];

// FIXTURES-MIRROR-CONTRACT (peer-review #20): the real edge emits `PrListRowVM.repo` as the BARE repo
// slug (`e.repo_slug` = a single-segment `scan_repo_slugs` name like `myelin`/`sandbox`), and the repo
// route param is that bare name. A tenant-qualified `owner/repo` here (the prior `myelin/myelin`) makes
// every cross-repo row's link 404 against the harness — a divergence invisible in e2e because the
// fixture is self-consistent. Fail LOUD at load if a fixture row re-introduces a non-bare slug.
for (const r of SEED_PR_ROWS) {
  if (typeof r.repo !== "string" || r.repo.includes("/")) {
    throw new Error(
      `dev-contract SEED_PR_ROWS: repo must be a BARE slug to mirror the edge (PrListRowVM.repo = e.repo_slug); got ${JSON.stringify(r.repo)} on PR #${r.number}`,
    );
  }
}

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

// Deterministic issue fixtures. Titles are display data; `key` is the searchable field. Extra rows
// exercise pagination.
export const DEV_ISSUE_TARGET = {
  project_id: "20aee030-c7fa-4757-8243-700faf528690",
  type_id: "7d457754-f6a1-4cd8-8738-21751570b627",
  prefix: "MYL",
};
const ISSUE_BASE_TIME = Date.parse("2026-07-19T12:00:00.000Z");

const headlineIssues = [
  {
    id: "00000000-0000-4000-8000-000000000102",
    key: "MYL-102",
    project_id: DEV_ISSUE_TARGET.project_id,
    state: "Todo",
    state_category: "unstarted",
    title: "Close the collaboration feedback loop",
    version: 1,
    created_at: "2026-07-19T11:00:00.000Z",
    updated_at: "2026-07-19T12:00:00.000Z",
  },
  {
    id: "00000000-0000-4000-8000-000000000101",
    key: "MYL-101",
    project_id: DEV_ISSUE_TARGET.project_id,
    state: "In progress",
    state_category: "started",
    title: "Verify encrypted issue titles",
    version: 3,
    created_at: "2026-07-18T09:00:00.000Z",
    updated_at: "2026-07-19T11:00:00.000Z",
  },
  {
    id: "00000000-0000-4000-8000-000000000100",
    key: "MYL-100",
    project_id: DEV_ISSUE_TARGET.project_id,
    state: "Done",
    state_category: "completed",
    title: "Consolidate issue navigation",
    version: 2,
    created_at: "2026-07-17T08:00:00.000Z",
    updated_at: "2026-07-18T10:00:00.000Z",
  },
];

const pagedOpenIssues = Array.from({ length: 49 }, (_, index) => {
  const number = 99 - index;
  const instant = new Date(ISSUE_BASE_TIME - (index + 2) * 60_000).toISOString();
  return {
    id: `00000000-0000-4000-8000-${String(number).padStart(12, "0")}`,
    key: `MYL-${number}`,
    project_id: DEV_ISSUE_TARGET.project_id,
    state: "Todo",
    state_category: "unstarted",
    title: `Tracked issue ${number}`,
    version: 1,
    created_at: instant,
    updated_at: instant,
  };
});

// Enough terminal rows to exercise the closed cursor path independently from open rows. These use
// a separate UUID namespace while retaining normal MYL keys and authoritative terminal categories.
const pagedClosedIssues = Array.from({ length: 50 }, (_, index) => {
  const number = 199 - index;
  const instant = new Date(Date.parse("2026-07-18T09:59:00.000Z") - index * 60_000).toISOString();
  return {
    id: `20000000-0000-4000-8000-${String(number).padStart(12, "0")}`,
    key: `MYL-${number}`,
    project_id: DEV_ISSUE_TARGET.project_id,
    state: "Done",
    state_category: "completed",
    title: `Completed issue ${number}`,
    version: 2,
    created_at: instant,
    updated_at: instant,
  };
});

export const SEED_ISSUES = [...headlineIssues, ...pagedOpenIssues, ...pagedClosedIssues];

export function freshIssueFixtures() {
  return SEED_ISSUES.map((issue) => ({ ...issue }));
}

export function issuesEnvelope(rows, state = "open", key, limit = 50, cursor) {
  const wanted = rows
    .filter((issue) =>
      state === "all"
        ? true
        : state === "closed"
          ? issue.state_category === "completed" || issue.state_category === "cancelled"
          : issue.state_category === "unstarted" || issue.state_category === "started",
    )
    .filter((issue) => !key || issue.key.startsWith(key.toUpperCase()))
    .sort((a, b) => compareUtf8(b.updated_at, a.updated_at) || compareUtf8(b.id, a.id));
  const offset = cursor?.startsWith("ic_dev_") ? Number(cursor.slice("ic_dev_".length)) || 0 : 0;
  const items = wanted.slice(offset, offset + limit);
  const next = offset + items.length < wanted.length ? `ic_dev_${offset + items.length}` : null;
  return { items, page: { next_cursor: next, limit } };
}

export function issueJson(rows, id) {
  return rows.find((issue) => issue.id === id) ?? null;
}

/** The edge's `{error:{message, code}}` envelope (error.rs) — a uniform 404. */
export function notFoundEnvelope(what) {
  return { error: { message: `${what} not found`, code: "not_found" } };
}

// The edge's `{error:{message, code}}` envelope (error.rs) — uniform, oracle-free 401.
export function unauthorizedEnvelope() {
  return { error: { message: "authentication required", code: "unauthorized" } };
}
