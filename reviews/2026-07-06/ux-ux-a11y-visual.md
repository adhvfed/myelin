# UX: accessibility & visual craft

_The web app is broadly faithful to the token system (semantic-tokens-only, no hex, no physical left/right, good status-not-by-colour in the diff and PR pills) and the overlay/ARIA plumbing is solid. But three findings cut against the design manual's own hard rules: the autofocused command-palette input kills its focus ring with an inline outline:none (the one control the manual singles out), the nav rail active state uses a saturated accent fill instead of the specified surface-hover treatment (against the Instrument direction and the user's R1 preference), and an accent-coloured link fails AA contrast in the light theme. Loading states are plain text rather than the mandated skeleton+aria-busy pattern across every route._

**Kept findings:** 7  (🟠 3 high  ·  🟡 3 medium  ·  🔵 1 low)

---

### 1. 🟠 Command-palette search input suppresses its focus ring with inline outline:none

- **Severity:** high  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** accessibility
- **Location:** `frontend/apps/web/src/components/CommandPalette.tsx:108`

**What:** The autofocused palette <input> sets `outline: "none"` inline (actually line 108, not 111 — line 111 is `font: "inherit"`). The design-system focus ring is delivered via a zero-specificity `:where(a,button,input,...):focus-visible` rule (build-tokens.mjs FOUNDATION_RULES, lines 203-209). An inline declaration beats any selector, so the outline is unconditionally removed; the wrapper div (lines 76-86) has a border but no `:focus-within` outline fallback. The palette input therefore has no token-driven focus ring.

**Impact:** When ⌘K opens the palette, Dialog initialFocus (line 73) lands focus on the input with the shared focus ring suppressed. Text inputs match :focus-visible whenever focused, so the ring would otherwise apply. The manual names this exact case as a cross-cutting must-ship: §6 ('One focus-ring covers every interactive element, including the autofocused palette input', line 170) and §8.4 must-ship #5 ('the focus token covers the autofocused palette control', line 540). WCAG 2.4.7 concern on the flagship keyboard surface.

**Fix:** Remove the inline `outline: "none"`. If a custom treatment is wanted, put a `:focus-within` outline using `--focus-ring` on the wrapper container, or let the shared `:focus-visible` rule apply. Never zero the outline inline on an interactive element.

> _Verifier note:_ Verified CommandPalette.tsx:88-113 — input has inline `outline: "none"` at line 108 (finding's line 111 is off by 3). Verified build-tokens.mjs:202-209 the ONE focus rule uses `:where(...)` (zero specificity) with `input` in the selector list. Verified manual §6 line 170 and §8.4 line 540 name the autofocused palette input explicitly. Severity kept high: it is a named ship-gate. Nuance: a text caret provides some focus indication, but the product's own manual mandates the ring here.  Line corrected 111→108.

### 2. 🟠 Nav rail active item uses a saturated accent fill instead of the specified surface-hover treatment

- **Severity:** high  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** visual
- **Location:** `frontend/apps/web/src/components/AppShell.tsx:226`

**What:** The active primary-nav rail item renders `color: var(--on-accent)` + `background: var(--accent)` on a `border-radius: var(--radius-1)` tile (lines 225-227). The shell spec is explicit: 'Active rail item = --surface-hover fill + brighter text, no colored side-bar marker (R1, §7)' (DESIGN-MANUAL.md line 266) and §7 Do (lines 433-435): 'Express selected/active as --surface-hover fill + brighter text ... No colored side-bar / inset accent edge marker'. A purpose-built `rail-active-accent` component token exists (→ accent) intended only for an optional glyph tint, not a full fill.

**Impact:** Every authenticated screen shows a rounded, saturated-blue active nav tile — over-spending the rationed accent and reading as exactly the rounded colored active indicator the user recorded as a dislike (R1 is marked binding). A full accent fill is a stronger violation than the side-bar marker R1 bans.

**Fix:** Set active = `background: var(--surface-hover)` + `color: var(--text-primary)` (brighter than the resting `--text-muted`), optionally tinting only the glyph via the `rail-active-accent` token. Keep the resting state muted; do not fill the tile with `--accent`.

> _Verifier note:_ Verified AppShell.tsx:219-228 — active uses `--on-accent`/`--accent` on a radius-1 tile. Verified manual line 266 and lines 433-435 mandate --surface-hover fill + no colored marker, tagged R1 binding. Verified tokens.json has a `rail-active-accent` component token → accent (glyph tint). Also matches the recorded user preference in memory (design-no-rounded-colored-side-borders). Not an a11y/legal issue (on-accent/accent contrast is fine) but a violation of a binding refinement — high justified.

### 3. 🟠 Accent-coloured commit link fails AA contrast in the light theme

- **Severity:** high  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** accessibility
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/[repo]/commits/[ref].tsx:57`

**What:** The commit short-oid link uses `color: var(--accent)` (small monospace) on a `--surface-raised` card row (row background at line 55). In the light theme accent = #2f6bff on surface-raised = #f7f8fa. I computed the WCAG contrast at 4.23:1 — below the 4.5:1 AA floor for normal text. DESIGN-MANUAL.md §3.1 (lines 165-170) says accent 'may carry brand ... even near the AA floor' and lands 'exactly at 4.50:1' in light — i.e. it must not be used as body text where readability matters.

**Impact:** Every commit-hash link in the log is below AA in light (WCAG 1.4.3) — an EN 301 549 eligibility failure for the EU public-sector market. Small mono text makes the shortfall more perceptible.

**Fix:** Use `--text-primary` for the link text (reserve `--accent` for non-text affordances per the §3.1 carve-out), or `--info` + underline which passes higher. Re-verify against the measured-contrast gate (§8.1) in all three themes.

> _Verifier note:_ Verified [ref].tsx:55 (row background `--surface-raised`) and :57 (`color: var(--accent)`, `font-family: var(--font-mono)`, small text). Verified tokens.json $themes.light: surface-raised→neutral-light.1=#f7f8fa, accent→blue.identity-light=#2f6bff. Computed WCAG contrast = 4.23:1 (L_fg≈0.183, L_bg≈0.938) — matches the finding's 4.23:1 exactly, below 4.5:1. Manual §3.1 confirms the carve-out intent. CONFIRMED.

### 4. 🟡 Loading states are plain text, not the mandated skeleton + aria-busy live region

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** consistency
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/index.tsx:29`

**What:** Route Suspense fallbacks are a bare `<p style="color:var(--text-muted)">Loading …</p>` (repos/index.tsx:29; commits/[ref].tsx:44). None is a structure-matching skeleton, none sets `aria-busy`, none announces via a live region. DESIGN-MANUAL.md is emphatic: 'Loading = a structure-matching skeleton — there is no spinner token in the system' (§5.3, line 341), 'Show structure while loading ... never a blank spinner' (§7 Do, line 442), and 'Every skeleton sets aria-busy + announces via one debounced polite live region' (§6, lines 413-414); it is cross-cutting must-ship #4 (line 539).

**Impact:** Systemic across the Git surface: slow loads collapse to one muted line (layout shift when content arrives) and screen-reader users get no polite loading→loaded announcement. Violates the product's own state-craft and a11y ship gates.

**Fix:** Introduce a shared Skeleton primitive that matches each surface's row/card structure, sets `aria-busy="true"`, and drives one debounced polite live region; replace the text fallbacks with it.

> _Verifier note:_ Verified repos/index.tsx:29 and commits/[ref].tsx:44 are both plain `<p>Loading…</p>` fallbacks. Verified manual line 341 (skeleton, no spinner token), lines 413-414 (aria-busy + one polite live region on every skeleton), line 442 (show structure), line 539 must-ship #4. Did not open every additional file the finding lists but the two I checked confirm the pattern; the claim is well-supported. Medium is fair — a named ship gate but non-legal-blocking in early build.

### 5. 🟡 Interactive elements set colour/background via inline styles, defeating hover/focus states

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** consistency
- **Location:** `frontend/apps/web/src/components/AppShell.tsx:226`

**What:** The nav rail links (lines 219-228), the ⌘K trigger button (lines 121-134), and the inbox button (lines 151-161) apply `color`/`background` via the inline `style` prop. DESIGN-MANUAL.md §7 Don'ts (lines 449-450): 'No colour via inline style on an interactive element (inline beats :hover/:focus specificity) — interactive colour comes from tokens/utility classes only. (PROVEN.)' Because inline colour wins specificity, these elements cannot receive a CSS `:hover` colour change, and none define one.

**Impact:** The nav rail and chrome buttons have no hover feedback — nothing changes on mouse hover of an inactive rail item, weakening the discoverable affordance the manual requires (P3). It also blocks a token-driven `--surface-hover` hover from ever applying. (Note: the focus ring is delivered via `outline`, which inline `color` does not defeat, so :focus-visible still works on these — the specific harm is hover colour only.)

**Fix:** Move interactive colour into design-system CSS classes / data-attributes (e.g. a `.nav-rail-item` class with `:hover`/`[aria-current]`/`:focus-visible` rules); keep only layout in inline styles.

> _Verifier note:_ Verified AppShell.tsx: nav links set inline `color`/`background` (226-227), ⌘K button inline `background`/`color` (130-131), inbox button inline `background`/`color` (158-159); none carry a CSS class or hover rule. Verified manual lines 449-450 quote (PROVEN). Corrected the finding's framing slightly: the focus ring uses `outline` (separate cascade) so it is NOT defeated by inline color — the real defect is missing hover colour, as the impact now states. Line anchored to 226 (nav link, the primary case). Medium confirmed.

### 6. 🟡 Primary CTA fill rides --accent instead of the derived button-primary-bg token

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** visual
- **Location:** `frontend/apps/web/src/routes/login.tsx:52`

**What:** The primary 'Continue as Dev Operator' button uses `background: var(--accent)` + `color: var(--on-accent)` (lines 52-53). The design-system ships a `button-primary-bg`/`button-primary-text` component token pair (button-primary-bg → focus-ring), and DESIGN-MANUAL.md §3.1 (lines 166-170) states the primary-action fill must ride the derived, higher-contrast token: 'button-primary-bg → focus-ring (6.55:1), not → accent'.

**Impact:** The primary affordance renders on the floor-contrast `accent` (in light, on-accent #fff on accent #2f6bff ≈ 4.5:1 — right at the AA floor) rather than the deliberately derived focus-ring fill (focus-light #1452d6, higher contrast). It also diverges from how a shared Button should render, undermining P1 coherence — the exact case §3.1's focus≠identity carve-out exists to prevent.

**Fix:** Use `var(--c-btn-primary-bg)` / `var(--c-btn-primary-text)` for the primary button (and factor a shared Button into the design-system so surfaces don't hand-roll accent fills).

> _Verifier note:_ Verified login.tsx:52-53 uses `--accent`/`--on-accent`. Verified tokens.json: component.button-primary-bg → semantic.focus-ring, button-primary-text → on-accent; light focus-ring = blue.focus-light #1452d6 vs accent identity-light #2f6bff. Verified manual lines 166-170 mandate button-primary-bg→focus-ring not →accent. Kept medium: white-on-accent text is ~4.5:1 (technically at the AA floor, so not a hard contrast failure) but the derived-token mandate is violated — a visual/consistency deviation, not a legal blocker.

### 7. 🔵 Shell and login use 100vh, clipping on mobile browser chrome

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** visual
- **Location:** `frontend/apps/web/src/components/AppShell.tsx:96`

**What:** The shell grid is pinned to `height: "100vh"` (line 96) and the login screen to `min-height: "100vh"` (login.tsx:14). On mobile browsers `100vh` counts the retractable address bar, so the bottom of the fixed shell (header/rail/main scroller) can be cut off or produce a double-scroll until the bar hides.

**Impact:** On phones/tablets the app chrome can overflow the visible viewport — a responsive-layout defect for a platform meant to work across form factors. Real-world impact is limited given the desktop-oriented 'command-deck' framing.

**Fix:** Use `100dvh` (with a `100vh` fallback) for the shell height and login min-height.

> _Verifier note:_ Verified AppShell.tsx:96 `height: "100vh"` and login.tsx:14 `min-height: "100vh"`. Confirmed the code uses vh not dvh. Grepped the manual — it does NOT mandate dvh (no 'dvh' occurrence), so this is a general best-practice recommendation rather than a spec violation; severity low is correct and arguably even generous for a desktop-first product.
