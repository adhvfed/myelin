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

// Two repos in the verified tenant — a POPULATED one and an EMPTY one — so the screen exercises both
// the data row AND an unglamorous (empty/onboarding) state. Shapes match RepoHome::to_json exactly.
export const SEED_REPOS = [
  {
    state: "populated",
    slug: "acme/myelin",
    readme_excerpt: "# acme/myelin\n\nThe make-it-real spine.",
    clone_url: "ssh://git@myelin/acme/myelin.git",
    entries: [
      { path: "README.md", is_dir: false },
      { path: "crates", is_dir: true },
      { path: "Cargo.toml", is_dir: false },
    ],
  },
  {
    state: "empty",
    slug: "acme/sandbox",
    clone_url: "ssh://git@myelin/acme/sandbox.git",
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

// One file in acme/myelin@main (WebEditForm::to_json).
const SEED_BLOBS = {
  "myelin/main/README.md": {
    path: "README.md",
    contents: "# acme/myelin\n\nThe make-it-real spine.\n",
    base_oid: "blake3:readmecontentaddress0001",
    viewer_may_edit: true,
  },
};

/** GET /v1/git/repos/{repo}/blob/{ref}/{path} → the WebEditForm ViewModel (null = 404). */
export function blobJson(repo, ref, path) {
  return SEED_BLOBS[`${repo}/${ref}/${path}`] ?? null;
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

/** GET /v1/git/repos/{repo}/commits/{ref} → the `{items,page}` commit-log envelope (null = 404). */
export function commitsEnvelope(repo, limit = 50) {
  const items = SEED_COMMITS[repo];
  if (!items) return null;
  return { items, page: { next_cursor: null, limit } };
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

// Two PRs: #1 blocked (a required check not green + an untrusted-fork run), #2 ready.
const SEED_PRS = {
  1: {
    pr: {
      number: 1,
      pr_state: "open",
      base_ref: "refs/heads/main",
      head_ref: "refs/heads/feature",
      head_oid: C2,
      author: "u_dev_operator@acme.noreply",
      reviews: 0,
      durable: true,
    },
    checks: {
      required_contexts: ["ci/build", "ci/test"],
      required_approvals: 1,
      green_contexts: ["ci/build"],
      endorsed_contexts: [],
      fork_unendorsed_contexts: ["ci/test"],
      gate_admitted: false,
      durable: true,
    },
  },
  2: {
    pr: {
      number: 2,
      pr_state: "open",
      base_ref: "refs/heads/main",
      head_ref: "refs/heads/hotfix",
      head_oid: C1,
      author: "u_dev_operator@acme.noreply",
      reviews: 1,
      durable: true,
    },
    checks: {
      required_contexts: ["ci/build"],
      required_approvals: 0,
      green_contexts: ["ci/build"],
      endorsed_contexts: [],
      fork_unendorsed_contexts: [],
      gate_admitted: true,
      durable: true,
    },
  },
};

/** GET /v1/git/repos/{repo}/prs/{n} → the durable PR record (null = 404). */
export function prJson(repo, n) {
  return repo === "myelin" && SEED_PRS[n] ? SEED_PRS[n].pr : null;
}

/** GET /v1/git/repos/{repo}/prs/{n}/checks → the checks + merge-gate projection (null = 404). */
export function prChecksJson(repo, n) {
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
