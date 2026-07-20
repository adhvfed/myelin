// THE DEV EDGE — a clearly-marked stand-in for the real `myelin-edge` binary (see dev-contract.mjs).
//
// It implements the SUBSET of the MR-014/015 edge HTTP contract the app shell exercises:
//   GET  /v1/git/repos      → the Git RepoHome ViewModel list in the `{items,page}` envelope (Bearer-auth)
//   GET  /v1/whoami         → the verified principal + scope (Bearer-auth)
//   POST /v1/auth/refresh   → the single-refresh round-trip (returns a fresh access token, or 401)
//   GET  /healthz           → liveness for the Playwright webServer
// Auth is REAL in shape (Bearer required; a uniform oracle-free 401 `{error:{message}}` envelope on
// failure); only the accepted token is the well-known DEV token (NOT production crypto). This is the
// exact contract the SolidStart gateway client speaks — point the gateway at the real `edge` binary
// instead (one env var) once the identity track can issue a human a capability token.

import { createServer } from "node:http";
import {
  DEV_ACCESS_TOKEN,
  DEV_REFRESH_TOKEN,
  whoamiJson,
  reposEnvelope,
  repoHomeJson,
  blobJson,
  commitsEnvelope,
  commitDiffJson,
  prJson,
  prChecksJson,
  prThreadsJson,
  prDiffJson,
  fileLinesJson,
  prCommitsEnvelope,
  devPost,
  repoPrsEnvelope,
  myPrsEnvelope,
  refsJson,
  treeJson,
  rawBytes,
  DEV_ISSUE_TARGET,
  freshIssueFixtures,
  issuesEnvelope,
  issueJson,
  unauthorizedEnvelope,
  notFoundEnvelope,
} from "./dev-contract.mjs";

const PORT = Number(process.env.DEV_EDGE_PORT ?? 8787);

// ── Mutable dev-double state (test-controllable, NEVER shipped) ──
// The dev edge is a clearly-marked test double; a test-control seam on it is legitimate scaffolding
// (it lets the first-run E2E model a fresh empty tenant + a dev-seam-off deployment against the SAME
// single harness). Toggled via `POST /__test/config`; reset by the test in a finally.
const state = {
  // A fresh tenant has no repos yet — the first-run onboarding empty state.
  emptyRepos: process.env.DEV_EDGE_EMPTY_REPOS === "1",
  // Whether the login page's dev seam may render (the `dev_login_enabled` server flag). Default on so
  // the harness's dev-login seam is reachable; a test flips it off to assert the seam disappears.
  devLoginEnabled: true,
  // R4.0 — whether the edge advertises the OPERATOR-TOKEN login (`token_login_enabled`). Default OFF so
  // the existing first-run spec's login posture is unchanged; the token-login spec flips it on. The
  // whoami route below already verifies a pasted token (Bearer === DEV_ACCESS_TOKEN), so with this on
  // the paste→verify→session flow runs end-to-end against this double.
  tokenLoginEnabled: false,
  // Test-only token-expiry switch: reject both access and refresh credentials so the browser harness
  // can verify the full expired-session redirect under streaming SSR and a strict CSP.
  forceUnauthorized: false,
  // Issues test controls: empty/projection-unavailable states and how many status reads precede
  // activation. Two proves the UI renders a genuine pending interval before active.
  emptyIssues: false,
  onlyClosedIssues: false,
  issuesUnavailable: false,
  issueActivationPolls: 2,
  issueActivationUnavailable: false,
  issueCreateUnavailable: false,
  issueCloseUnavailable: false,
  issueListFirstPageHolds: 0,
  issueListFirstPageDelaysMs: [],
  issueListCursorDelaysMs: [],
  issueListFirstPageDelayedRequests: 0,
  issueListFirstPageDelayedResponses: 0,
  issueListCursorRequests: 0,
  issueListCursorResponses: 0,
  issueListCursorRequestsByState: { open: 0, closed: 0, all: 0 },
};

let issueRows = freshIssueFixtures();
const issueReceipts = new Map();
const issueListDelayTimers = new Map();
const heldIssueListResponses = new Set();
let issueListDelayGeneration = 0;
let issueSequence = 200;
const ISSUE_BASE_TIME_FOR_CREATE = Date.parse("2026-07-20T00:00:00.000Z");

function cancelIssueListDelays() {
  issueListDelayGeneration += 1;
  for (const [timer, res] of issueListDelayTimers) {
    clearTimeout(timer);
    if (!res.writableEnded) {
      send(res, 409, { error: { message: "test fixture generation reset", code: "conflict" } });
    }
  }
  issueListDelayTimers.clear();
  for (const held of heldIssueListResponses) {
    if (!held.res.writableEnded) {
      send(held.res, 409, { error: { message: "test fixture generation reset", code: "conflict" } });
    }
  }
  heldIssueListResponses.clear();
}

function resetIssues() {
  cancelIssueListDelays();
  issueRows = freshIssueFixtures();
  issueReceipts.clear();
  issueSequence = 200;
  state.emptyIssues = false;
  state.onlyClosedIssues = false;
  state.issuesUnavailable = false;
  state.issueActivationPolls = 2;
  state.issueActivationUnavailable = false;
  state.issueCreateUnavailable = false;
  state.issueCloseUnavailable = false;
  state.issueListFirstPageHolds = 0;
  state.issueListFirstPageDelaysMs = [];
  state.issueListCursorDelaysMs = [];
  state.issueListFirstPageDelayedRequests = 0;
  state.issueListFirstPageDelayedResponses = 0;
  state.issueListCursorRequests = 0;
  state.issueListCursorResponses = 0;
  state.issueListCursorRequestsByState = { open: 0, closed: 0, all: 0 };
}

function send(res, status, json, headers = {}) {
  const body = json === null ? "" : JSON.stringify(json);
  res.writeHead(status, { "content-type": "application/json", ...headers });
  res.end(body);
}

function delayedIssueListResponse(res, envelope, delay, responseCounter) {
  const generation = issueListDelayGeneration;
  const timer = setTimeout(() => {
    issueListDelayTimers.delete(timer);
    if (generation !== issueListDelayGeneration) {
      if (!res.writableEnded) {
        send(res, 409, { error: { message: "test fixture generation changed", code: "conflict" } });
      }
      return;
    }
    state[responseCounter] += 1;
    send(res, 200, envelope);
  }, delay);
  issueListDelayTimers.set(timer, res);
  res.on("close", () => {
    if (!res.writableEnded && issueListDelayTimers.delete(timer)) clearTimeout(timer);
  });
  return timer;
}

function holdIssueListResponse(res, envelope, responseCounter) {
  const held = { res, envelope, responseCounter, generation: issueListDelayGeneration };
  heldIssueListResponses.add(held);
  res.on("close", () => {
    if (!res.writableEnded) heldIssueListResponses.delete(held);
  });
}

function releaseHeldIssueListResponses() {
  for (const held of heldIssueListResponses) {
    heldIssueListResponses.delete(held);
    if (held.res.writableEnded) continue;
    if (held.generation !== issueListDelayGeneration) {
      send(held.res, 409, { error: { message: "test fixture generation changed", code: "conflict" } });
      continue;
    }
    state[held.responseCounter] += 1;
    send(held.res, 200, held.envelope);
  }
}

function bearer(req) {
  const h = req.headers["authorization"] ?? "";
  const m = /^Bearer (.+)$/.exec(Array.isArray(h) ? h[0] : h);
  return m ? m[1] : null;
}

const server = createServer((req, res) => {
  const url = new URL(req.url ?? "/", `http://${req.headers.host}`);
  const path = url.pathname;
  const method = req.method ?? "GET";

  if (path === "/healthz") return send(res, 200, { ok: true });

  // R3.5 — the UNAUTHENTICATED public auth surface the logged-out login page reads. Matched BEFORE
  // the Bearer gate (reachable with no session), exactly like the real edge's built-in route.
  if (method === "GET" && path === "/v1/auth/config") {
    return send(res, 200, {
      // The default real deployment has no IdP configured — the honest "SSO unavailable" render.
      sso_configured: false,
      providers: [],
      dev_login_enabled: state.devLoginEnabled,
      token_login_enabled: state.tokenLoginEnabled,
    });
  }

  // Test-control seam (dev double ONLY — never a real edge route). Flips the fixture's first-run
  // posture so a single harness can exercise empty-tenant + dev-seam-off.
  if (method === "POST" && path === "/__test/config") {
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", () => {
      try {
        const body = raw ? JSON.parse(raw) : {};
        if (typeof body.emptyRepos === "boolean") state.emptyRepos = body.emptyRepos;
        if (typeof body.devLoginEnabled === "boolean") state.devLoginEnabled = body.devLoginEnabled;
        if (typeof body.tokenLoginEnabled === "boolean") state.tokenLoginEnabled = body.tokenLoginEnabled;
        if (typeof body.forceUnauthorized === "boolean") state.forceUnauthorized = body.forceUnauthorized;
        if (body.resetIssues === true) resetIssues();
        if (typeof body.emptyIssues === "boolean") state.emptyIssues = body.emptyIssues;
        if (typeof body.onlyClosedIssues === "boolean") state.onlyClosedIssues = body.onlyClosedIssues;
        if (typeof body.issuesUnavailable === "boolean") state.issuesUnavailable = body.issuesUnavailable;
        if (Number.isInteger(body.issueActivationPolls)) state.issueActivationPolls = body.issueActivationPolls;
        if (typeof body.issueActivationUnavailable === "boolean") state.issueActivationUnavailable = body.issueActivationUnavailable;
        if (typeof body.issueCreateUnavailable === "boolean") state.issueCreateUnavailable = body.issueCreateUnavailable;
        if (typeof body.issueCloseUnavailable === "boolean") state.issueCloseUnavailable = body.issueCloseUnavailable;
        if (Number.isInteger(body.issueListFirstPageHolds) && body.issueListFirstPageHolds >= 0) {
          state.issueListFirstPageHolds = body.issueListFirstPageHolds;
        }
        if (
          Array.isArray(body.issueListFirstPageDelaysMs) &&
          body.issueListFirstPageDelaysMs.every((delay) => Number.isInteger(delay) && delay >= 0 && delay <= 5_000)
        ) state.issueListFirstPageDelaysMs = [...body.issueListFirstPageDelaysMs];
        if (
          Array.isArray(body.issueListCursorDelaysMs) &&
          body.issueListCursorDelaysMs.every((delay) => Number.isInteger(delay) && delay >= 0 && delay <= 5_000)
        ) state.issueListCursorDelaysMs = [...body.issueListCursorDelaysMs];
        if (body.releaseIssueListFirstPages === true) releaseHeldIssueListResponses();
      } catch {
        /* ignore malformed control body */
      }
      send(res, 200, { ok: true, state });
    });
    return;
  }

  // The refresh round-trip: a valid refresh token mints a fresh access token (here, the same dev
  // token — the dev seam's token is long-lived); anything else is a uniform 401.
  if (method === "POST" && path === "/v1/auth/refresh") {
    if (!state.forceUnauthorized && bearer(req) === DEV_REFRESH_TOKEN) {
      return send(res, 200, { access_token: DEV_ACCESS_TOKEN });
    }
    return send(res, 401, unauthorizedEnvelope());
  }

  // Every data route requires a valid Bearer (the auth floor). A missing/forged token → uniform 401.
  const authed = !state.forceUnauthorized && bearer(req) === DEV_ACCESS_TOKEN;

  // R4.4 Issues writes. The create body is contract-checked against the one server-injected dogfood
  // target; the dev edge never accepts browser-selected scope IDs either.
  let im;
  if (method === "POST" && path === "/v1/issues") {
    if (!authed) return send(res, 401, unauthorizedEnvelope());
    if (state.issueCreateUnavailable) {
      return send(res, 503, { error: { message: "issue creation is temporarily unavailable", code: "unavailable" } });
    }
    let raw = "";
    req.on("data", (chunk) => (raw += chunk));
    req.on("end", () => {
      let body;
      try {
        body = JSON.parse(raw);
      } catch {
        return send(res, 400, { error: { message: "invalid issue create body", code: "bad_request" } });
      }
      if (
        body.project_id !== DEV_ISSUE_TARGET.project_id ||
        body.type_id !== DEV_ISSUE_TARGET.type_id ||
        body.prefix !== DEV_ISSUE_TARGET.prefix ||
        typeof body.title !== "string" ||
        !body.title.trim()
      ) {
        return send(res, 400, { error: { message: "invalid issue create body", code: "bad_request" } });
      }
      const number = ++issueSequence;
      const id = `10000000-0000-4000-8000-${String(number).padStart(12, "0")}`;
      const key = `${DEV_ISSUE_TARGET.prefix}-${number}`;
      const now = new Date(ISSUE_BASE_TIME_FOR_CREATE + number * 1_000).toISOString();
      const row = {
        id,
        key,
        project_id: DEV_ISSUE_TARGET.project_id,
        state: "Todo",
        state_category: "unstarted",
        title: body.title.trim(),
        version: 1,
        created_at: now,
        updated_at: now,
      };
      const requestEventId = `01J${String(number).padStart(23, "0")}`;
      issueReceipts.set(requestEventId, { row, polls: 0, active: false });
      return send(res, 202, {
        issue: { id, key, project_id: row.project_id },
        authorization: { status: "pending", request_event_id: requestEventId },
      });
    });
    return;
  }

  if (method === "POST" && (im = path.match(/^\/v1\/issues\/([^/]+)\/close$/))) {
    if (!authed) return send(res, 401, unauthorizedEnvelope());
    if (state.issueCloseUnavailable) {
      return send(res, 503, { error: { message: "issue close is temporarily unavailable", code: "unavailable" } });
    }
    const row = issueJson(issueRows, decodeURIComponent(im[1]));
    if (!row) return send(res, 404, notFoundEnvelope("issue"));
    if (row.state_category !== "completed") {
      row.state = "Done";
      row.state_category = "completed";
      row.version += 1;
      row.updated_at = new Date(Date.parse(row.updated_at) + 1_000).toISOString();
    }
    return send(res, 200, row);
  }

  // R3.3 — PR write paths (threads / reviews / merge). Stateful in-memory; the e2e drives these.
  let pm;
  if (
    method === "POST" &&
    (pm = path.match(/^\/v1\/git\/repos\/([^/]+)\/prs\/(\d+)\/(.+)$/))
  ) {
    if (!authed) return send(res, 401, unauthorizedEnvelope());
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", () => {
      let body = {};
      try {
        body = raw ? JSON.parse(raw) : {};
      } catch {
        body = {};
      }
      const out = devPost(decodeURIComponent(pm[1]), Number(pm[2]), pm[3], body);
      if (out.status === 404) return send(res, 404, notFoundEnvelope("pull request"));
      return send(res, out.status, out.json ?? null);
    });
    return;
  }

  if (method === "GET" && path === "/v1/whoami") {
    if (!authed) return send(res, 401, unauthorizedEnvelope());
    return send(res, 200, whoamiJson());
  }

  if (method === "GET" && path === "/v1/git/repos") {
    if (!authed) return send(res, 401, unauthorizedEnvelope());
    const limit = Number(url.searchParams.get("limit") ?? 50);
    // A fresh tenant (test-controlled) serves the empty envelope → the onboarding empty state.
    if (state.emptyRepos) return send(res, 200, { items: [], page: { next_cursor: null, limit } });
    return send(res, 200, reposEnvelope(limit));
  }

  if (method === "GET" && path === "/v1/issues") {
    if (!authed) return send(res, 401, unauthorizedEnvelope());
    if (state.issuesUnavailable) {
      return send(res, 503, { error: { message: "issue authorization is temporarily unavailable", code: "unavailable" } });
    }
    const listState = url.searchParams.get("state") ?? "open";
    const key = url.searchParams.get("key") ?? undefined;
    const limit = Number(url.searchParams.get("limit") ?? 50);
    const cursor = url.searchParams.get("cursor") ?? undefined;
    const rows = state.emptyIssues
      ? []
      : state.onlyClosedIssues
        ? issueRows.filter((issue) => issue.state_category === "completed" || issue.state_category === "cancelled")
        : issueRows;
    const envelope = issuesEnvelope(rows, listState, key, limit, cursor);
    const delay = cursor
      ? state.issueListCursorDelaysMs.shift() ?? 0
      : state.issueListFirstPageDelaysMs.shift() ?? 0;
    if (cursor) {
      state.issueListCursorRequests += 1;
      if (Object.hasOwn(state.issueListCursorRequestsByState, listState)) {
        state.issueListCursorRequestsByState[listState] += 1;
      }
    } else if (delay > 0) {
      state.issueListFirstPageDelayedRequests += 1;
    }
    if (!cursor && state.issueListFirstPageHolds > 0) {
      state.issueListFirstPageHolds -= 1;
      state.issueListFirstPageDelayedRequests += 1;
      holdIssueListResponse(res, envelope, "issueListFirstPageDelayedResponses");
      return;
    }
    if (delay > 0) {
      return delayedIssueListResponse(
        res,
        envelope,
        delay,
        cursor ? "issueListCursorResponses" : "issueListFirstPageDelayedResponses",
      );
    }
    if (cursor) state.issueListCursorResponses += 1;
    return send(res, 200, envelope);
  }

  if (
    method === "GET" &&
    (im = path.match(/^\/v1\/issues\/authorization-requests\/([^/]+)$/))
  ) {
    if (!authed) return send(res, 401, unauthorizedEnvelope());
    if (state.issuesUnavailable || state.issueActivationUnavailable) {
      return send(res, 503, { error: { message: "issue authorization is temporarily unavailable", code: "unavailable" } });
    }
    const receipt = issueReceipts.get(decodeURIComponent(im[1]));
    if (!receipt) return send(res, 404, notFoundEnvelope("issue"));
    receipt.polls += 1;
    if (
      !receipt.active &&
      state.issueActivationPolls >= 0 &&
      receipt.polls >= state.issueActivationPolls
    ) {
      receipt.active = true;
      if (!issueJson(issueRows, receipt.row.id)) issueRows.push(receipt.row);
    }
    return receipt.active
      ? send(res, 200, { status: "active", issue: receipt.row })
      : send(res, 202, {
          status: "pending",
          issue: {
            id: receipt.row.id,
            key: receipt.row.key,
            project_id: receipt.row.project_id,
          },
          retry_after_ms: 1_000,
        });
  }

  if (method === "GET" && (im = path.match(/^\/v1\/issues\/([^/]+)$/))) {
    if (!authed) return send(res, 401, unauthorizedEnvelope());
    const row = issueJson(issueRows, decodeURIComponent(im[1]));
    return row ? send(res, 200, row) : send(res, 404, notFoundEnvelope("issue"));
  }

  // R3.5 — the unified tenant firehose. The real edge emits typed repo.created/repo.pushed frames
  // here; the dev double holds the stream OPEN (a keepalive comment) but emits none (the named floor),
  // so the browser EventSource connects cleanly (no reconnect storm) and the manual Refresh is the
  // fallback. Bearer-gated like every data route.
  if (method === "GET" && /^\/v1\/t\/[^/]+\/events$/.test(path)) {
    if (!authed) return send(res, 401, unauthorizedEnvelope());
    res.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache, no-transform",
      connection: "keep-alive",
    });
    res.write(": connected\n\n");
    const keepalive = setInterval(() => res.write(": keepalive\n\n"), 15000);
    req.on("close", () => clearInterval(keepalive));
    return;
  }

  // GT-004 + R3.4 browse + PR routes (every one Bearer-gated; a missing seed is the uniform 404).
  if (method === "GET") {
    if (!authed) return send(res, 401, unauthorizedEnvelope());
    const limit = Number(url.searchParams.get("limit") ?? 50);
    const cursor = url.searchParams.get("cursor") ?? undefined;
    const seg = (s) => decodeURIComponent(s);
    // Decode a nested `{...path}` (keep the `/` separators).
    const nested = (s) => s.split("/").map(decodeURIComponent).join("/");
    let m;
    // R3.1 — the cross-repo front door (no {repo}).
    if (path === "/v1/git/prs") {
      const bucket = url.searchParams.get("bucket") ?? "needs-review";
      return send(res, 200, myPrsEnvelope(bucket, limit));
    }
    // Order: more-specific (/prs/{n}/checks) before /prs/{n} before the /prs collection.
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/prs\/(\d+)\/checks$/))) {
      const v = prChecksJson(seg(m[1]), Number(m[2]));
      return v ? send(res, 200, v) : send(res, 404, notFoundEnvelope("pull request"));
    }
    // R3.2 · G-7 — the PR three-dot diff.
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/prs\/(\d+)\/diff$/))) {
      const v = prDiffJson(seg(m[1]), Number(m[2]), url.searchParams.get("cursor") ?? undefined);
      return v ? send(res, 200, v) : send(res, 404, notFoundEnvelope("pull request"));
    }
    // R3.2 · G-7 N2 — expand-context lines at a blob oid.
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/file-lines\/([^/]+)$/))) {
      const start = Number(url.searchParams.get("start") ?? 1);
      const end = Number(url.searchParams.get("end") ?? 0);
      const v = fileLinesJson(seg(m[1]), seg(m[2]), start, end);
      return v ? send(res, 200, v) : send(res, 404, notFoundEnvelope("file"));
    }
    // R3.3 — the PR discussion + review batches.
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/prs\/(\d+)\/threads$/))) {
      const v = prThreadsJson(seg(m[1]), Number(m[2]));
      return v ? send(res, 200, v) : send(res, 404, notFoundEnvelope("pull request"));
    }
    // R3.3 — the commits IN a PR.
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/prs\/(\d+)\/commits$/))) {
      const v = prCommitsEnvelope(seg(m[1]), Number(m[2]), limit);
      return v ? send(res, 200, v) : send(res, 404, notFoundEnvelope("pull request"));
    }
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/prs\/(\d+)$/))) {
      const v = prJson(seg(m[1]), Number(m[2]));
      return v ? send(res, 200, v) : send(res, 404, notFoundEnvelope("pull request"));
    }
    // R3.1 — the per-repo PR list collection.
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/prs$/))) {
      const state = url.searchParams.get("state") ?? "open";
      const v = repoPrsEnvelope(seg(m[1]), state, limit);
      return v ? send(res, 200, v) : send(res, 404, notFoundEnvelope("repository"));
    }
    // R3.4: the ref switcher.
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/refs$/))) {
      const v = refsJson(seg(m[1]));
      return v ? send(res, 200, v) : send(res, 404, notFoundEnvelope("repository"));
    }
    // R3.4: tree-at-path (root = /tree/{ref}; nested = /tree/{ref}/{...path}).
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/tree\/([^/]+)(?:\/(.+))?$/))) {
      const v = treeJson(seg(m[1]), seg(m[2]), m[3] ? nested(m[3]) : "");
      if (!v || v.__status === 404) return send(res, 404, notFoundEnvelope("path"));
      return send(res, 200, v);
    }
    // R3.4: raw/download byte-serving (Content-Disposition set here).
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/(raw|download)\/([^/]+)\/(.+)$/))) {
      const b = rawBytes(seg(m[1]), seg(m[3]), nested(m[4]), m[2] === "download");
      if (!b) return send(res, 404, notFoundEnvelope("file"));
      res.writeHead(200, {
        "content-type": b.contentType,
        "content-disposition": b.disposition,
        "x-content-type-options": "nosniff",
      });
      return res.end(b.body);
    }
    // R3.4: nested blob.
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/blob\/([^/]+)\/(.+)$/))) {
      const v = blobJson(seg(m[1]), seg(m[2]), nested(m[3]));
      if (!v || v.__status === 404) return send(res, 404, notFoundEnvelope("file"));
      return send(res, 200, v);
    }
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/commits\/([^/]+)$/))) {
      const v = commitsEnvelope(seg(m[1]), limit, cursor);
      return v ? send(res, 200, v) : send(res, 404, notFoundEnvelope("repository"));
    }
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/commit\/([^/]+)$/))) {
      const v = commitDiffJson(seg(m[1]), seg(m[2]));
      return v ? send(res, 200, v) : send(res, 404, notFoundEnvelope("commit"));
    }
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)$/))) {
      const v = repoHomeJson(seg(m[1]));
      return v ? send(res, 200, v) : send(res, 404, notFoundEnvelope("repository"));
    }
  }

  return send(res, 404, { error: { message: `no route for ${method} ${path}`, code: "not_found" } });
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`[dev-edge] listening on http://127.0.0.1:${PORT} (DEV SEAM — not production auth)`);
});
