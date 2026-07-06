# Frontend web app (correctness + security)

_I read the full SolidStart web unit at frontend/apps/web. The security-load-bearing core is genuinely sound. src/server/session.ts issues an httpOnly, SameSite=Lax, path-scoped cookie carrying ONLY an opaque CSPRNG id (randomBytes(24).base64url via freshId()); the Bearer/refresh tokens live server-side in the globalThis-backed Map, so real tokens never serialize to client JS. src/server/gateway-core.ts::runGateway correctly implements the no-token→Unauthorized / single-refresh-then-one-retry / envelope-typed GatewayError lifecycle and is well covered by gateway-core.test.ts. src/server/gateway.ts wires vinxi/http + node fetch to a FIXED host from MYELIN_EDGE_URL with every user segment passed through encodeURIComponent (seg() in lib/api.ts and encodeURIComponent in the repo tree links), so there is no SSRF and no path traversal to the edge. XSS surface is clean: every git-derived value (blob contents split by line in blob/[ref]/[path].tsx, README excerpt and diff line content, PR author/refs) is rendered as Solid JSX text (auto-escaped) or inside <pre>/<code> — there is no innerHTML/dangerouslySetInnerHTML and the README is shown raw, not markdown-rendered. auth.ts keeps getViewer/requireViewer/logout behind "use server", and (app).tsx guards the whole shell via requireViewer with a server-side /login redirect. Reactivity is correct: createAsync sources read params/search reactively, CommandPalette's createMemo(matches) and clampActive are sound, and AppShell's global ⌘K listener is registered in onMount with a matching onCleanup. The defects I did find are lower-severity: the dev-login action has no environment guard, the refresh path drops a rotated refresh token, and a couple of CommandPalette/AppShell UX/a11y nits._

**Kept findings:** 4  (🟡 1 medium  ·  🔵 1 low  ·  ⚪ 2 nit)

---

### 1. 🟡 Dev-login action mints a full authenticated session with no environment guard

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** auth
- **Location:** `src/lib/auth.ts:58`

**What:** loginDev is a `"use server"` action bound to the /login form (login.tsx:40) that calls issueSession() with the well-known DEV_ACCESS_TOKEN/DEV_REFRESH_TOKEN/DEV_PRINCIPAL unconditionally. There is no NODE_ENV / feature-flag guard anywhere (grep confirms the only NODE_ENV check in the unit is the cookie `secure` flag in session.ts:80). Any unauthenticated POST to this server action grants a complete, valid app session for DEV_PRINCIPAL.

**Impact:** If this bundle is ever deployed with the real OIDC login still deferred (or the seam is left registered after OIDC lands), anyone on the network can obtain an authenticated session by POSTing to the dev-login action — a full authentication bypass. Today it is the intended stand-in, but nothing at runtime prevents it from minting sessions in production.

**Fix:** Gate loginDev behind an explicit runtime check (e.g. `if (process.env.NODE_ENV === "production" && !process.env.MYELIN_ALLOW_DEV_LOGIN) throw redirect("/login")`, or refuse outright) so the seam cannot issue sessions once deployed, mirroring the edge's refuse-not-mock stance for POST /v1/auth/login.

> _Verifier note:_ Verified: loginDev (auth.ts:58-70) is a `"use server"` action bound to the /login form (login.tsx:40, method=post) and calls issueSession() unconditionally with DEV_ACCESS_TOKEN/DEV_REFRESH_TOKEN/DEV_PRINCIPAL. grep of src/ confirms the ONLY process.env checks are session.ts:80 (cookie `secure`) and gateway.ts:23 (edge URL) — no NODE_ENV/feature-flag guard on loginDev. Any unauthenticated POST mints a session. Nuance on the stated impact: the app's auth gate requireViewer (auth.ts:40-50) passes on session-record EXISTENCE alone, so the authenticated shell/identity/viewer facts render regardless of edge — that part is a genuine full bypass of the app guard. However, actual edge data calls carry DEV_ACCESS_TOKEN, which only the dev edge accepts (dev-edge/server.mjs:60); a real `myelin-edge` would 401 those and gateway-core would bounce to /login. So 'complete app session / anyone on the network gets data' is true only when deployed against the dev edge, and the shell-level guard bypass holds unconditionally. Real latent risk; medium is fair given the misconfiguration precondition and the clear dev-seam labeling.

### 2. 🔵 Refresh flow persists a rotated access token but drops a rotated refresh token

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** correctness
- **Location:** `src/server/gateway.ts:63`

**What:** In edgeRequest's refresh(), the code parses only `access_token` from the /v1/auth/refresh response and calls updateSessionToken(fresh). The refresh credential (rec.refreshToken) is read from the session but the response's rotated refresh token, if the edge issues one, is never captured or persisted back to the session record.

**Impact:** If the edge rotates refresh tokens on use (a common single-use-refresh-token posture), the stored refreshToken becomes stale after the first refresh. The next 401 will attempt a refresh with a spent token, fail, and bounce the user to /login prematurely — cutting sessions short well before the 8h cookie maxAge.

**Fix:** Parse a `refresh_token` field from the refresh response alongside `access_token` and persist both onto the session record (extend updateSessionToken, or add updateSessionTokens) so refresh-token rotation is honored.

> _Verifier note:_ Verified the code fact: gateway.ts:64 types the refresh response as `{ access_token?: string }` and line 66 calls updateSessionToken(fresh), which (session.ts:60-65) only writes `token`, never `refreshToken`. A rotated refresh_token in the response is neither parsed nor persisted, so rec.refreshToken stays the original. Impact is conditional and currently zero: the dev edge (dev-edge/server.mjs:52-54) returns only `{ access_token: DEV_ACCESS_TOKEN }` and does NOT rotate refresh tokens, and DEV_REFRESH_TOKEN is a fixed constant reused every time. So this only bites a future real edge with single-use refresh-token rotation. Accurate as a latent/defensive correctness gap; low is correct.

### 3. ⚪ Command palette leaves the active row index stale across close/reopen

- **Severity:** nit  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** reactivity
- **Location:** `src/components/CommandPalette.tsx:41`

**What:** runActive() and both onClose handlers reset queryText but never reset the `active` signal to 0. On reopen the search box is cleared (full command list shown) while `active` retains its previous value, so the highlighted row is whatever was last selected rather than the first command.

**Impact:** Minor UX inconsistency: reopening ⌘K highlights an arbitrary row, and if the previously-active index exceeds a later (filtered) match list, Enter is briefly a no-op until the user arrows. No correctness or security impact.

**Fix:** Reset setActive(0) alongside setQueryText("") in runActive and in the Dialog onClose handler (or reset on open).

> _Verifier note:_ Verified: runActive (line 41-48) resets setQueryText("") but not `active`; the Dialog onClose handler (lines 67-70) likewise resets only queryText. `setActive(0)` is called only in the input onInput (line 94) — which fires on typing, not on open. Nothing resets active on open (initialFocus just focuses the input). So on reopen, matches() shows the full list (empty query) while active() retains the prior index, highlighting a stale row. Correct nit; no correctness/security impact since ArrowUp/Down clampActive re-bounds and Enter guards on m[active()] being defined.

### 4. ⚪ Dead keyboard handler and full-page reload in shell navigation

- **Severity:** nit  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** a11y
- **Location:** `src/components/CommandPalette.tsx:143`

**What:** The <li role="option"> onKeyDown (Enter/Space) can never fire because the li has no tabindex and is not focusable — the real keyboard surface is the combobox input via aria-activedescendant, so this handler is dead code despite the comment claiming it makes the option key-operable. Separately, AppShell.tsx:73 navigates via window.location.assign(n.href) from the palette, forcing a full page reload while the nav rail uses client-side <A>.

**Impact:** No functional a11y gap (the input drives selection) but the dead handler is misleading, and the palette's full-reload navigation is inconsistent with the SPA nav elsewhere, losing client-side transition benefits.

**Fix:** Remove the unreachable li onKeyDown handler (or make the li focusable if pointer users need it), and use the router's navigate() instead of window.location.assign for command-palette navigation.

> _Verifier note:_ Verified both claims. (1) The <li role="option"> (line 132) has no tabindex and is never programmatically focused, so it cannot receive keyboard focus; its onKeyDown (lines 143-149) is unreachable via keyboard — the real key surface is the combobox input (onKeyDown line 96 + aria-activedescendant line 101). The comment at 140-142 claiming it 'keeps the option key-operable' is misleading; onClick (136) still serves pointer users, so removing onKeyDown loses nothing. (2) AppShell.tsx:74 nav commands use window.location.assign(n.href) (full reload) while the nav rail (AppShell.tsx:215) uses client-side <A> — inconsistent, forgoes SPA transition. Both accurate; nit, no functional a11y or security impact.
