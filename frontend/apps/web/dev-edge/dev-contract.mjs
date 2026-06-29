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

// The edge's `{error:{message, code}}` envelope (error.rs) — uniform, oracle-free 401.
export function unauthorizedEnvelope() {
  return { error: { message: "authentication required", code: "unauthorized" } };
}
