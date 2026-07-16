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
  repoPrsEnvelope,
  myPrsEnvelope,
  unauthorizedEnvelope,
  notFoundEnvelope,
} from "./dev-contract.mjs";

const PORT = Number(process.env.DEV_EDGE_PORT ?? 8787);

function send(res, status, json, headers = {}) {
  const body = json === null ? "" : JSON.stringify(json);
  res.writeHead(status, { "content-type": "application/json", ...headers });
  res.end(body);
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

  // The refresh round-trip: a valid refresh token mints a fresh access token (here, the same dev
  // token — the dev seam's token is long-lived); anything else is a uniform 401.
  if (method === "POST" && path === "/v1/auth/refresh") {
    if (bearer(req) === DEV_REFRESH_TOKEN) {
      return send(res, 200, { access_token: DEV_ACCESS_TOKEN });
    }
    return send(res, 401, unauthorizedEnvelope());
  }

  // Every data route requires a valid Bearer (the auth floor). A missing/forged token → uniform 401.
  const authed = bearer(req) === DEV_ACCESS_TOKEN;

  if (method === "GET" && path === "/v1/whoami") {
    if (!authed) return send(res, 401, unauthorizedEnvelope());
    return send(res, 200, whoamiJson());
  }

  if (method === "GET" && path === "/v1/git/repos") {
    if (!authed) return send(res, 401, unauthorizedEnvelope());
    const limit = Number(url.searchParams.get("limit") ?? 50);
    return send(res, 200, reposEnvelope(limit));
  }

  // GT-004 browse + PR routes (every one Bearer-gated; a missing seed is the uniform 404 envelope).
  if (method === "GET") {
    if (!authed) return send(res, 401, unauthorizedEnvelope());
    const limit = Number(url.searchParams.get("limit") ?? 50);
    const seg = (s) => decodeURIComponent(s);
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
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/blob\/([^/]+)\/([^/]+)$/))) {
      const v = blobJson(seg(m[1]), seg(m[2]), seg(m[3]));
      return v ? send(res, 200, v) : send(res, 404, notFoundEnvelope("file"));
    }
    if ((m = path.match(/^\/v1\/git\/repos\/([^/]+)\/commits\/([^/]+)$/))) {
      const v = commitsEnvelope(seg(m[1]), limit);
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
