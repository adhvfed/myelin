# External surface (myelin-edge, myelin-cli, myelin-mcp, myelin-client)

_The external surface (edge gateway, CLI, MCP, resilient client) is well-structured and security-conscious: the gateway is total over malformed input, the 401/403 envelope is genuinely oracle-free and PII-free (error.rs), tenant scope is always taken from the verified token (never path/body), pagination is capped, and the resilient client's breaker/retry-storm guards are carefully reasoned. The most serious issue is in MCP agent governance: the HITL approval gate trusts a caller-supplied boolean, so an autonomous agent can self-approve a gated tool. Secondary issues are structural (the re-authorization seam is action-string-only and cannot express object-scoped authz) plus two named-but-latent secret-handling floors (predictable session ids, a token-file chmod TOCTOU) and a CLI plaintext-only transport constraint._

**Kept findings:** 5  (🟡 2 medium  ·  🔵 3 low)

---

### 1. 🟡 MCP HITL approval is a caller-controlled boolean — an autonomous agent can self-approve a gated tool

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** agent-governance
- **Location:** `crates/myelin-mcp/src/server.rs:149`

**What:** The HITL gate in the governance chokepoint is driven entirely by `params.approval.granted`, a naked boolean lifted straight out of the incoming `tools/call` JSON-RPC message (server.rs:149-153) and passed to `GovernedRouter::call`, where `if tool.requires_approval() && !approval_granted { return Gated }` (governance.rs:224). Nothing binds `approval_granted` to a real human decision: no verification of a previously-issued gate id, no lookup in an approval/gate store, no check that a human principal (vs the calling agent) granted it, and the `gate_id` handed back (`hitl:{jti}:{tool}`) is deterministic, not a challenge. Since the MCP client for an autonomous agent IS the agent runtime, the gated principal supplies its own approval flag.

**Impact:** An autonomous agent calling a `requires_approval` tool (e.g. `git.merge`) can set `{"approval":{"granted":true}}` in its own request to clear the ROUTER-level HITL gate, so the router proceeds to `EffectApi::apply`. With the reference `SkeletonEffectApi` (governance.rs:294-300) the effect then 'applies' unconditionally. Mitigating: the boolean is NOT forwarded into `EffectApi::apply` (run_ctx_for/proposed_effect_for at governance.rs:259-268 carry only jti/principal/tool/args), so the production `myelin_agent_service::PlanThenApply` — whose eight-step pipeline includes its own HITL step per the module docs — re-gates independently of the caller flag. The bypass therefore defeats the router's advertised-as-REAL HITL leg (and the skeleton path), but does not by itself defeat the injected production HITL enforcement.

**Fix:** Do not treat the client-supplied approval flag as the enforcement point. On a gated tool, mint a durable gate record keyed to `(jti, tool, args-hash)` and require the re-drive to present that gate id AND prove a distinct human principal approved it (server-side lookup), so the acting agent cannot manufacture its own approval. Keep the real HITL enforcement in `EffectApi`/PlanThenApply and make the router gate fail-closed rather than pass a caller boolean through.

> _Verifier note:_ Verified server.rs:149-155 reads approval.granted from params with unwrap_or(false) and passes it to router.call. Verified governance.rs:224-227 gate is `tool.requires_approval() && !approval_granted`. Verified approval_granted is used ONLY at that gate — it is not threaded into run_ctx_for (line 259-261) or proposed_effect_for (266-268), so EffectApi::apply never sees it; hence production PlanThenApply re-gates independently. SkeletonEffectApi::apply (294-300) applies unconditionally. Module docstring (governance.rs:12-13, 21-27) lists the HITL gate as REAL but names PlanThenApply as the injected production body. Severity lowered from high to medium: real weakness at the router seam, but the caller boolean cannot bypass the injected production HITL step.

### 2. 🟡 Edge re-authorization is action-string only — it cannot enforce object/repo-scoped authz within a tenant

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** authz
- **Location:** `crates/myelin-edge/src/gateway.rs:235`

**What:** The gateway re-authorizes every call as `self.authorizer.authorize(&principal, &route.action)` (gateway.rs:235), and the `Authorizer` trait is `authorize(&self, principal: &Principal, action: &str) -> bool` (topology.rs:266-269). The `action` is a static per-route constant (`"git.pr.merge"` git_durable.rs:1131, `"git.blob.commit"` :1119, `"git.repo.branch_protection.set"` :1137) carrying no repo slug, PR number, tenant, or resource id. The verified `TenantScope`, the path params, and the request are never handed to the authorizer, so a Zanzibar-style `check(object, relation, user)` cannot be expressed at this seam.

**Impact:** Tenant isolation is enforced separately (scope-from-token + IDOR reject/audit), so this is not a cross-tenant leak. But WITHIN a tenant, once the real Identity-M1 authorizer lands it can only decide 'may this principal merge (in general)', not 'may this principal merge THIS repo's PR'. Any principal granted `git.pr.merge`/`git.blob.commit`/`git.repo.branch_protection.set` for one repo would be authorized for every repo in the tenant, including setting branch-protection policy. The re-authorization step reads as a strong control but is coarse by construction. Note the git_durable.rs:438 comment already claims 'the production authorizer resolves Id.check(repo_admin)' — an object-scoped check the current seam cannot deliver.

**Fix:** Widen the `Authorizer` seam to receive the resource context (tenant scope + resolved object identity, e.g. `authorize(principal, action, &ResourceRef)`), and have handlers/routes carry the object into the check, before the real authorizer body is wired — otherwise the M1 authorizer inherits an object-blind interface that its own docstrings already assume it does not have.

> _Verifier note:_ Confirmed the trait at topology.rs:266-269 is object-blind (action: &str only), with an AllowAll and a stub impl at :277/:286. Confirmed gateway.rs:235 passes only (&principal, &route.action); the resolved `scope` and `params` exist in handle_inner but are not passed to authorize. Confirmed action strings are static route constants at git_durable.rs:1119/1131/1137. Confirmed git_durable.rs:436-439 docstring asserts a repo-admin object check the seam cannot express. Forward-looking design finding (no real authorizer wired yet — AllowAll in tests); severity medium retained.

### 3. 🔵 Session ids are generated from a counter + nanosecond timestamp, not a CSPRNG (predictable, hijackable)

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** security
- **Location:** `crates/myelin-edge/src/session.rs:97`

**What:** `SessionStore::fresh_id` returns `format!("sess-{nanos:x}-{n:x}")` (session.rs:97-104), where `n` is a process-monotonic `AtomicU64` counter and `nanos` is the current UNIX time in nanoseconds. Both components are low-entropy and guessable. The session id is the sole bearer of authentication in the httpOnly-cookie web path (`SessionStore::get` maps it to the server-side capability token, used by the gateway at gateway.rs:282-290). This is explicitly named as a floor in the module docs (session.rs:18-20), and login currently refuses (503, gateway.rs:334-339) so no session is issued in production yet — but `issue()` is public (session.rs:57) and the docs state the human-verifier landing is 'a config change, not new plumbing', i.e. this id generator would ship as-is.

**Impact:** When the human login path is wired, an attacker who can approximate issue time and counter range could brute-force a valid session id and hijack an authenticated session — full account takeover, defeating the httpOnly cookie protection.

**Fix:** Generate the session id from a CSPRNG (>=128 bits) before the login path is enabled; keep the opaque-id-in-httpOnly-cookie shape unchanged.

> _Verifier note:_ Confirmed fresh_id at session.rs:97-104 uses fetch_add counter + SystemTime nanos, hex-formatted. Confirmed issue() is public (57) and get() (67) is the sole auth mapping used by gateway.rs authenticate (282-290). Confirmed login refuses 503 (gateway.rs:334-339) so not yet reachable in prod, and the module docstring names CSPRNG as a hardening floor (session.rs:18-20). Severity low retained — real defect gated behind an unwired login path.

### 4. 🔵 CLI token file is written with default (umask) permissions before being chmod'd to 0600 — TOCTOU read window

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** secret-handling
- **Location:** `crates/myelin-cli/src/config.rs:128`

**What:** `store_token` does `std::fs::write(&path, token.as_bytes())` (config.rs:128) and only THEN calls `set_owner_only` to chmod 0600 (config.rs:130,136-140). `fs::write` creates a new file with mode `0666 & !umask` (commonly 0644), so between the write and the chmod the token bytes are on disk group/world-readable.

**Impact:** On a shared/multi-user host, a local attacker racing the window (or reading the file before the chmod) can exfiltrate the capability token — a full bearer credential. Small window and local-only.

**Fix:** Create the file atomically owner-only via `OpenOptions::new().write(true).create(true).truncate(true).mode(0o600)` (unix) and write into that handle, rather than write-then-chmod. Consider write-to-temp-then-rename for atomic replacement.

> _Verifier note:_ Confirmed config.rs:128 fs::write precedes set_owner_only at :130; set_owner_only (136-140) chmods 0600 only on unix and is a no-op on non-unix (144-147). The token is a bearer capability token (config.rs module docs). Also note an existing token file with laxer perms would only get tightened after the world-readable write, and on non-unix perms are never tightened. Severity low retained (local, small window).

### 5. 🔵 CLI presents the bearer capability token but speaks plaintext HTTP only (https is rejected outright)

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** security
- **Location:** `crates/myelin-cli/src/client.rs:28`

**What:** `client::target` rejects any `https://` edge URL with a `CliError::Transport` and only accepts `http://` (client.rs:27-36), while `execute` sends the capability token as `Authorization: Bearer <token>` over the plaintext hyper http1 connection (client.rs:60-79). The default edge is `http://127.0.0.1:8080` (config.rs:31) so this is fine for local dev, but the CLI cannot be pointed at a TLS endpoint at all — if used against a non-localhost edge the token traverses the network in cleartext, and there is no supported client-side way to enable TLS.

**Impact:** For a remote edge, the bearer token (and all request/response data) is exposed to any on-path observer — at odds with the EU-sovereign/GDPR-safe posture for anything beyond loopback. The client also does not refuse a non-loopback `http://` host, so a misconfigured `$MYELIN_EDGE` silently sends the credential in cleartext.

**Fix:** Support `https://` (native TLS, or an explicit documented 'insecure localhost only' flag) so the token never crosses a network in cleartext; at minimum refuse a non-loopback `http://` host rather than refusing `https://`.

> _Verifier note:_ Confirmed client.rs:29-33 returns Transport error for https:// and :34-36 for any non-http scheme; the plaintext TcpStream+http1 handshake is at :60-66 and the Bearer header at :79. Confirmed DEFAULT_EDGE is http://127.0.0.1:8080 (config.rs:31) and there is no TLS path anywhere in the crate. No loopback restriction on the http host (target only checks host is non-empty, :42-44). Severity low retained — safe at the loopback default, but genuinely unable to protect the token off-host.
