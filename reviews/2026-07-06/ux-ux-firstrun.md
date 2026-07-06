# UX: first-run, login & the one-shell

_I reviewed the real first-run/shell surface: routes/login.tsx (the dev-session seam `loginDev` + the disabled SSO button), routes/(app).tsx (the `requireViewer` auth guard wrapping `AppShell`), routes/(app)/index.tsx (a hard `<Navigate href="/git/repos">`), components/AppShell.tsx (the `NAV` array, the ⌘K trigger + global keydown, the `commands()` registry, the `ResidencyCue`, and the hardcoded inbox), components/CommandPalette.tsx (the combobox/listbox `matches()` roving surface), components/NotAvailable.tsx, and routes/(app)/git/repos/index.tsx (`ReposScreen`/`RepoRow` with the `repos-empty` state). I also confirmed via app.tsx and a repo-wide grep that there is NO catch-all/`notFound`/404 route. The chrome itself is genuinely well built — real skip link, `aria-current` nav, WCAG-1.4.1-conscious residency + inbox cues, a proper focus-trapped Dialog-based palette with `aria-activedescendant`, and a dignified `NotAvailable` component — but the first-run JOURNEY is broken in three concrete ways: (1) four of the five primary nav destinations are unbuilt and fall through to a raw framework 404 rather than the dignified state the codebase already owns; (2) the de-facto landing (repos list) empty state is a dead-end sentence with no next action and no guided start, failing the R-20/D2 "empty state teaches" bar; (3) the shell shows hardcoded fake activity ("2 unread", fake inbox items) that misleads a first-timer on an empty tenant. The intended "one shell" IA is also only partially realized: everyone is force-landed on the engineer Code surface and there is no context-pane slot the ws-c IA specifies._

**Kept findings:** 6  (🟠 1 high  ·  🟡 3 medium  ·  🔵 2 low)

---

### 1. 🟠 Four of five primary nav destinations are dead ends — a raw framework 404, not the dignified state the codebase already owns

- **Severity:** high  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** dead-end
- **Location:** `frontend/apps/web/src/components/AppShell.tsx:23`

**What:** NAV (AppShell.tsx:23-29) presents five equal icons — Code, Issues, Chat, CI, Knowledge — but only /git/repos is routed. Clicking Issues/Chat/CI/Knowledge (or the matching palette entries) falls through to SolidStart's default 404; there is no catch-all/notFound route and NotAvailable is used only for a missing git route param.

**Impact:** A first-timer exploring the rail hits an uninformative framework 404 on 4 of the 5 primary destinations, bypassing the calm-not-crash NotAvailable surface the repo already owns.

**Fix:** Add a catch-all route rendering NotAvailable inside the shell, and/or render each unbuilt subsystem index with NotAvailable; have the rail honestly signal not-yet-available destinations.

> _Verifier note:_ Confirmed against source. NAV array lines 23-29 has exactly those 5 hrefs. `find routes` shows only git/repos routes + login + (app)/index — no /issues, /chat, /ci, /knowledge, and no [...404]/notFound file. app.tsx just mounts <FileRoutes/> with no notFound. NotAvailable.tsx exists and is imported ONLY in git/repos/[repo]/index.tsx as a fallback for missing params.repo. The NAV code comment even claims the rail is 'honest about what's behind each' — but the destinations dead-end to a raw 404, so the honesty is not realized. Severity high is defensible: guaranteed, ugly, hit by every explorer. One tempering fact worth noting — the same comment plus the MR-019 marker ('Only Code is wired to a real screen in MR-019') shows this is a known early-build state, so it reads as incomplete-by-plan rather than a regression.

### 2. 🟡 The de-facto first-run landing has no next action and no guided start — the empty state states a fact instead of teaching

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** onboarding
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/index.tsx:35`

**What:** (app)/index.tsx redirects every authenticated user to /git/repos, so ReposScreen is the first surface. Its empty branch renders a single muted sentence 'No repositories in this tenant yet.' (data-testid=repos-empty) with no button, clone/import affordance, or explanation, and there is no guided cross-subsystem first-run sequence.

**Impact:** A first-time user on a fresh tenant lands on an uninformative sentence with no obvious next action; the empty state states a fact rather than teaching.

**Fix:** Turn repos-empty into an onboarding-forward state: a teaching line plus concrete CTAs (clone URL / push instructions / import), ideally tied into a first-run checklist across subsystems.

> _Verifier note:_ Confirmed against source. (app)/index.tsx is `<Navigate href="/git/repos" />`. repos/index.tsx empty fallback (the <Show> around line 33-38) is exactly the single muted <p> with no affordance. Notably the PER-REPO empty state (repos/[repo]/index.tsx, home.state==="empty") DOES teach — it shows the clone URL + `git clone / git push` block — which sharpens the contrast: the tenant-level list empty state is the one that doesn't. Downgraded high->medium: the consequence is an uninformative screen, not a functional block ('immediately blocked' in the finding overstates it), and repo provisioning on this platform may be out-of-band (push-to-create), so a literal 'Create repo' button may not fit the model — but the teaching/next-action gap is real.

### 3. 🟡 Shell shows hardcoded fake activity that misleads a first-timer on an empty tenant

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** honesty
- **Location:** `frontend/apps/web/src/components/AppShell.tsx:146`

**What:** The inbox affordance hardcodes an unread badge of '2' (aria-label 'Inbox, 2 unread' and a visible '2'), and the inbox Dialog lists two fabricated, non-linking items — 'A pull request needs your review' and 'CI passed on acme/myelin'. On an empty tenant this signals activity that doesn't exist and points at a repo the user doesn't have.

**Impact:** Erodes trust at first contact: the badge promises 2 items on an empty tenant and the fake rows link nowhere — a dead end inside the chrome.

**Fix:** Drive the badge/inbox from real (even if empty) notification state and give the inbox a proper empty state; zero/hide the badge when nothing is unread.

> _Verifier note:_ Confirmed against source. aria-label 'Inbox, 2 unread' and the hardcoded visible '2' are present in the inbox button; the Dialog <ul> hardcodes exactly the two named <li> spans (no <a>/href, so they link nowhere). One mitigating detail: the Dialog's own `description` prop already says 'Notifications arrive here. Live delivery (SSE) is a later wiring' — so the placeholder nature is partially disclosed. But the always-on '2' badge and seeded rows still actively misrepresent state. Medium is appropriate.

### 4. 🟡 The 'one shell' IA is only partially realized: force-landed on the engineer surface, and no context-pane slot

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** ia
- **Location:** `frontend/apps/web/src/routes/(app)/index.tsx:5`

**What:** (app)/index.tsx unconditionally does <Navigate href="/git/repos">, dropping every persona (PM, exec, DPO) onto the engineer Code list. AppShell provides only header + icon rail + secondaryNav slot + main; there is no context-pane region, which R-06's four-part structure (primary-nav + contextual-sidebar + content + context-pane) names as first-class.

**Impact:** A non-engineer's first-run is an engineer's screen, and the shell structurally can't host the PR/issue context-pane the one-shell IA and wedge work depend on.

**Fix:** Make the app-root landing role-/intent-aware per R-06's default-landing map, and add the context-pane region to the shell grid.

> _Verifier note:_ Code facts confirmed: (app)/index.tsx is an unconditional <Navigate>, role-blind. AppShell's grid has header, a Primary <nav> icon rail, a `secondaryNav` <aside> slot, and <main> — no fourth context-pane region. The cited spec is corroborated: design-planning/02-research-roadmap/ws-c-ia-one-shell.md and 07-judging/lens-first-run.md both exist, and 03-research-prompts.md R-06 explicitly names the 'per-role default-landing map (PM→roadmap, engineer→cycle board)' and a 'context-pane structure as a concrete tree'. Important caveat lowering severity from being higher: R-06 is a research-roadmap/design-planning prompt (a future spec), not an implemented MR-019 acceptance requirement — the shell is explicitly early ('Only Code is wired'), so this is measuring the current build against a not-yet-scheduled target. The UX observations are valid; medium is fair.

### 5. 🔵 Command-palette navigation does a full page reload and re-points at the dead-end destinations

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** efficiency
- **Location:** `frontend/apps/web/src/components/AppShell.tsx:73`

**What:** commands() nav entries run window.location.assign(n.href) instead of the client-side router (<A>) used by the rail, so every palette navigation triggers a full document reload — re-running the auth guard and flashing the shell — and for four of five entries it reloads straight into the 404 in finding 1.

**Impact:** Sluggish, jarring palette navigation and the same dead-end destinations reached via a slower path; inconsistent with the client-side <A> rail.

**Fix:** Use useNavigate() for palette navigation to stay client-side, and drop/disable palette entries for unbuilt subsystems or route them through NotAvailable.

> _Verifier note:_ Confirmed against source. commands() maps NAV to `run: () => { window.location.assign(n.href); }` (the code is at lines 69-76; the assign call is line 73). The rail uses <A href> (client router). So the full-reload claim and the re-pointing at the four unrouted destinations both hold. Low is appropriate.

### 6. 🔵 Login offers only the dev seam; a real deployment first-timer is fully blocked with no path

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** onboarding
- **Location:** `frontend/apps/web/src/routes/login.tsx:62`

**What:** login.tsx offers the 'Continue as Dev Operator' seam (labelled 'not production auth') and an SSO button disabled with the deferral reason only in a hover `title`. In a non-dev deployment where SSO isn't configured, a real user has no actionable path, and the sole explanation is a mouse-only tooltip invisible to touch/keyboard users. The dev seam is not gated to dev builds.

**Impact:** In a non-dev deployment the login page is a soft dead end for real users, and the SSO-unavailable reason is mouse-only.

**Fix:** Surface the SSO-deferred reason as visible text (not just title), and gate/hide the dev seam outside dev builds.

> _Verifier note:_ Confirmed against source. login.tsx renders the dev-login <form action={loginDev}> with visible text 'Development session seam — not production auth', and a `disabled` SSO <button> whose only explanation is the `title` attribute (the MR-012 deferral string, at the button around line 62-65). The dev seam is rendered unconditionally — no import.meta.env/dev-build guard. The a11y point (title is inaccessible to keyboard/touch) is accurate. Low is appropriate; this is the finding most explicitly acknowledged as intentional in-code.
