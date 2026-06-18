# Doctrine Integration Analysis — UX & Design

> Phase: `02b-doctrine-integration`. Source doctrine:
> [`external-insights/05-ux-and-design.md`](../../../external-insights/05-ux-and-design.md)
> (treated as a DEFAULT-to-follow, README status equal to VISION). Primary binding target:
> [`02-holistic-architecture/design-language.md`](../../02-holistic-architecture/design-language.md)
> (hereafter **DL**). Secondary: the Views/Screens sections of the five
> [`02-holistic-architecture/subsystems/*.md`](../../02-holistic-architecture/subsystems/).
> Downstream: Phase-4 per-subsystem design sketches; Phase-5 testing; Phase-8 frontend execution.

## How to read this

The doctrine doc CONFIRMS our design-language's *posture* almost wholesale: density-calm, command
palette over the graph, system-assembles-context, agents-not-magic, design-before-code,
empty/loading/error as first-class, borders-over-shadow, accessibility as token constraint. Our DL
already holds these as **principles**. The genuine value the doctrine adds is **concrete, day-one,
testable mandates** that our DL states only as direction — the *specific bugs* and the *correctness
gates* that turn a principle into a thing you can fail a CI check on. That is where the deltas are.

The strategic move for this phase: **augment DL with a new "§11 — Concrete frontend mandates
(doctrine-bound)"** section that pins these specifics as platform law, and route the
build-discipline items (round-trip gate, switch test, measured-contrast gate) to Phase-5 testing and
Phase-8 execution so they bind to a gate, not just a paragraph. No ADR needs reopening; no CONFLICTS.

---

## Classification table

Legend for WHERE-IT-BINDS: **DL-§11** = new doctrine-bound mandates section appended to
design-language.md (Phase-2 back-patch, design-language augmentation); **DL-edit** = sharpen an
existing DL section in place; **P4** = Phase-4 subsystem design sketches; **P5** = Phase-5 testing
strategy; **P8** = Phase-8 execution discipline.

| # | Doctrine item (doc §) | Class | Where it binds | Integration action |
|---|---|---|---|---|
| 1 | **Overlay primitives built day-one** (Dialog/Confirm/Popover/Dropdown/Tooltip/Toast) (§1) | SHARPENS | DL-§11 + P4 + P6 | DL §5 names the command palette / unfurl hovercard / HITL card / toasts as floating surfaces but never mandates a *shared overlay primitive set built before features*. Add to DL-§11 a named primitive catalogue as a **Phase-6 sequencing prerequisite** (build before any feature consumes overlays). |
| 2 | **Portal-always to document root** (§1) | NEW | DL-§11 | Net-new concrete rule. Our DL has no statement about portaling or transformed/overflow-clipped ancestors. Mandate: every overlay primitive portals to root; the "create dialog renders inside the 240px sidebar" bug class is forbidden by construction. |
| 3 | **One documented z-index scale** (chrome < popover < modal < toast) (§1) | NEW | DL-§11 + DL §3.5 | DL §3.5 lists elevation *surfaces* but no z-index ordering token. Add a single z-index scale token to the elevation system; per-component magic numbers banned. |
| 4 | **Centralised focus-trap + return-focus, scroll-lock w/ scrollbar compensation, Escape/backdrop dismiss, ARIA in the primitive** (§1) | SHARPENS | DL-§11 | DL §4 says "screen-reader correctness is a component contract" and "no keyboard traps" as a *goal*; doctrine says *where* it lives — inside the overlay primitive so consumers inherit it free. Sharpen DL §4 to bind this behaviour to the primitive, not to each consumer. |
| 5 | **Keep each overlay primitive single-purpose; "nine menus are three shapes"** (§1) | NEW | DL-§11 + P4 | Net-new anti-complecting rule. Bind as a design-review heuristic: split overlays by *shape* (viewport-pinned popover / inline-flow dropdown / externally-positioned grid), don't force one component. This is also the §7 design-doc payoff (see #21). |
| 6 | **One editor render path: read and edit run the SAME inline parser** (§2) | NEW | DL-§11 + P4(Knowledge) + P5 | DL §5.9 commits "one editor over the shared content model" and ADR-05 commits "share the AST, not the engine," but **neither mandates a single render path nor a read==edit parser**. This is the deepest net-new mandate. Bind in DL-§11; the Knowledge P4 sketch owns the implementation; P5 owns the gate (#7). |
| 7 | **`render(parse(md)) === md` round-trip gate over a corpus** (§2) | RESOLVES-OPEN → **TE-15** (and TE-4) | DL-§11 + **P5** | Hands TE-15 (CRDT-vs-OT, editor) a concrete **DEFAULT-TO-BEAT**: whatever concurrency engine is chosen, round-trip correctness over a markdown corpus is a hard CI gate. Routes primarily to **Phase-5 testing strategy** as a named gate, referenced by the Knowledge P4 prototype. |
| 8 | **Controlled `contenteditable`, NOT `<textarea>`; caret = char offset into serialised markdown; bridge to/from DOM** (§2) | RESOLVES-OPEN → **TE-15** | DL-§11 + P4(Knowledge) | A concrete editor-architecture default. DL §5.9 is silent on the rendering mechanism. Bind as the default approach (textarea "fundamentally cannot show formatting as you type"); browser-variance (Enter/IME/paste) flagged as top risk for the Knowledge P4 sketch. |
| 9 | **Store inline content as a markdown-subset STRING (not inline-range JSON)** (§2) | SHARPENS / borderline-CONFLICTS | DL-§11 + P4(Knowledge) + P3-adjacent | ADR-05 / `myelin-content` commits a shared **AST** content model. Doctrine says inline content should serialise to a **markdown-subset string** (no server sanitisation, survives copy/paste/export/diff, zero-migration through an editor rewrite). These are compatible — the AST can hold inline content as a markdown-subset string at leaf level — but the seam must be made explicit so we don't ship an inline-range-JSON representation. Flag for the Knowledge P4 sketch + a note to the shared-content-model owner; recommended resolution: **AST for block structure, markdown-subset string for inline runs.** |
| 10 | **Enter-splits-block / caret-after-split deserve their own design; slice rewrite primitive-first (serializer, offset model, DOM-surgery shipped + unit-tested standalone)** (§2) | NEW | DL-§11 + P4(Knowledge) + P6 | Net-new execution sequencing for the editor. "Enter just inserts a newline" named as the #1 "not a real editor" tell. Bind as a Phase-6 roadmap note for Knowledge: editor primitives are independently shipped/tested before the integrated editor. |
| 11 | **Measure contrast, never trust a stated ratio** (§3) | SHARPENS | DL §4 + **P5** | DL §4 already says "contrast as a token constraint … validated in the token system itself." Doctrine sharpens *how*: measured, not claimed, with a worked failure (a brand accent at ~2.8:1 fails AA). Add a **measured-contrast gate** to P5 testing over the token table. |
| 12 | **The focus token is NOT the identity token** (§3) | NEW | DL §3.2 + DL §4 | Net-new, sharp. DL §3.2 has a `focus-ring` semantic token and an `agent` token but never says the focus/primary-button token may need to *differ from the brand accent* because the accent can fail AA. Add this as an explicit token-derivation rule. |
| 13 | **Status never by colour alone (glyph/label/position); no saturated status fills — "the screen is not a traffic light"** (§3) | CONFIRMS | DL §3.2 / §4 | DL §4 already requires "functional-status treatments never rely on colour alone (always icon + text label)." Keep; the "no saturated fills" taste note can be folded into DL §3.2 as house-style. |
| 14 | **Hierarchy from weight & colour before size; a hairline/region groups more than whitespace, but reach for space first; spacing on a fixed ramp (5/7/13px = amateur tell)** (§3) | SHARPENS | DL §3.4 / §3.5 | DL §3.4 already mandates "one spacing scale, no magic numbers" and §3.5 "borders-and-surfaces first." Doctrine sharpens the *typographic-hierarchy* rule (weight/colour before size; large/heavy type as the amateur tell) which DL does not state — add to DL §3.3. |
| 15 | **Borders carry separation; ~one shadow token for genuinely floating surfaces** (§3) | CONFIRMS | DL §3.5 | DL §3.5 already says "borders-and-surfaces first, shadow sparingly … reserved for genuinely floating layers." Validation. |
| 16 | **Never set colour via inline style on an interactive element (inline beats hover:/focus: specificity)** (§3) | NEW | DL-§11 + P8 | Net-new, concrete bug-class. Not in DL. Bind as an execution lint rule (P8) and a DL-§11 mandate: interactive colour comes from tokens/utility classes only. |
| 17 | **Live styleguide rendered from the product's REAL tokens, runnable with the stack down** (§3) | SHARPENS | DL §8 + P6 | DL §8 commits "one design-system package implementing the tokens." Doctrine adds the *live, stack-down-runnable styleguide* so the reference can't drift from the app. Add as a Phase-6 deliverable on the design-system package. |
| 18 | **Agents look like agents — no sparkle/shimmer/magic-wand AI iconography; no emoji as UI (can't inherit currentColor / be re-themed)** (§3) | SHARPENS | DL §6.1 / §3.7 | DL §6/P7 commits "agents never magic, always labelled." Doctrine sharpens to a concrete *iconography ban* (no sparkle/wand) and *no-emoji-as-UI* rule (re-theming/currentColor reason). Add to DL §3.7 (iconography) + §6.1. |
| 19 | **Tag each rule as PROVEN (cite standard) vs HOUSE STYLE (taste)** (§3) | NEW | DL-edit (whole §3/§4) + DL-§11 | Net-new honesty discipline for the design system itself. Adopt the proven-vs-house-style tag convention across DL §3–§4 and DL-§11, so "accessibility requires this" vs "we prefer this" stays honest (mirrors the VISION/README honesty rule). |
| 20 | **One shell everywhere (rail + contextual secondary nav + header)** (§4) | CONFIRMS | DL §5.1 / P1 | DL §5.1 (navigation shell) + P1 ("one product, not five") already commit exactly this. Validation. |
| 21 | **Command palette over the universal reference graph; the graph that powers automation powers navigation** (§4) | CONFIRMS | DL §5.2 + ADR-13 | DL §5.2 commits the palette over `ArtifactRef`/the reference graph, sharing the query AST with views and triggers. Direct match. |
| 22 | **The system assembles context; the user never does (show the link, pre-fetch — failing check → step → line; notification → why)** (§4) | CONFIRMS / SHARPENS | DL §5.3 / §7.1 (PR context pane) | DL §5.3 (unfurl + context pane) and the PR context pane (git §4.2) commit "the system assembles context." Doctrine sharpens with the **pre-fetch** mandate (don't just link — pre-fetch the next hop) and the "notification → *why it fired*" provenance. Add the pre-fetch line to DL §5.3 and the "why" to DL §5.8 (inbox provenance). |
| 23 | **Optimistic updates, honest rollback ("optimism for latency, honesty on failure")** (§4) | CONFIRMS | DL P2 / §5.10 | DL P2 + §5.10 commit optimistic UI. Doctrine names the rollback honesty pairing explicitly — fold the phrase into DL §5.10 (loading) as the rule. |
| 24 | **Reversibility over confirmation (undo window + restorable history > "are you sure?")** (§4) | NEW | DL-§11 + P4 | Net-new interaction principle. DL has destructive/erasure states but no general "prefer undo over confirm" stance. Add as a DL-§11 interaction mandate (with the obvious carve-out: irreversible/consequential + GDPR/agent-HITL actions still confirm per §6.3). |
| 25 | **Real-time as default on any live-capable surface, over a reconnect-safe transport (liveness must be trustworthy)** (§4) | CONFIRMS | DL P2 + ADR-04 | DL P2 ("live updates pushed via the event bus") + the firehose transport (ADR-04) commit this. The "reconnect-safe / trustworthy liveness" emphasis sharpens — note it on DL §5.1. |
| 26 | **Empty/loading/error first-class: empty explains+offers create; loading shows STRUCTURE (skeletons, never spinner-on-blank); error blames the system in one quiet line + path; degraded fails STATIC** (§4) | CONFIRMS / SHARPENS | DL §5.10 | DL §5.10 already mandates these five+ states. Doctrine sharpens the *specifics*: skeletons-match-final-layout (not spinners), error blames the *system* not the user in one quiet line, degraded surface fails *static* ("temporarily unavailable" for that surface only). Tighten DL §5.10 wording. |
| 27 | **Hard latency budgets: keyboard < ~100ms; suppress flash-of-spinner < ~1s; "pages render, they don't animate in"** (§4) | RESOLVES-OPEN / SHARPENS → DL P2 | DL P2 + **P5** | DL P2 commits "sub-100ms perceived response" but as a soft aim. Doctrine makes it a **hard budget** + two concrete companions (no spinner-flash < 1s; pages don't animate in). Bind as numeric budgets in DL P2 and as a **P5 performance-gate** default. |
| 28 | **Pin shell to viewport (100vh/overflow:hidden); each region its own scroller; flex child that scrolls needs `min-height:0` + overscroll-contain** (§5) | NEW | DL-§11 + P4 | Net-new concrete layout-containment bug. DL §5.1 says "the shell owns the layout grid" but nothing this specific. The `min-height:0` detail is load-bearing — without it overflow leaks up the tree and pushes the composer below the fold. Bind in DL-§11 as a shell mandate. |
| 29 | **`width:100%` is not a takeover — collapse the other column at the breakpoint** (§5) | NEW | DL-§11 + P4(Chat/Issues) | Net-new mobile bug class. A full-width mobile panel laid out *beside* a still-present main column is clipped off-screen. Bind in DL-§11; the responsive-shell P4 sketches inherit it. |
| 30 | **Hover is not touch-reachable — surface row actions by default or behind an explicit mobile affordance** (§5) | NEW | DL-§11 + P4 | Net-new. DL §4 covers keyboard but not hover-only-actions-invisible-on-touch. Directly affects the issue-list row actions, chat message hover actions, knowledge backlinks. Bind in DL-§11. |
| 31 | **Flip popovers when they'd go off-screen (flip-above + max-height); test against the REAL anchor** (§5) | NEW | DL-§11 + P5 | Net-new concrete bug (picker under a bottom-pinned composer renders off-screen). This is the overlay primitive's positioning contract (#1/#5). Bind in DL-§11; "test against the real anchor" → P5/P8. |
| 32 | **Mobile drawer pattern (rail/secondary-nav → toggled overlays w/ backdrop+Escape+route-change auto-close); name the fixed-width assumptions before responsive** (§5) | SHARPENS | DL §3.4 + P4 | DL §3.4 mentions responsive breakpoints + the shell owning the frame, but no drawer pattern. Add the drawer pattern + the "name fixed-width assumptions first" caution to DL §5.1 and flag for P4 responsive sketches. |
| 33 | **Humanise machine strings at the BACKEND, paired with a routable reference (not a frontend string map)** (§6) | RESOLVES-OPEN / NEW → binds to ADR-13 + Notifications | DL §6.x + **P3(Notifications/Refs)** + P4 | Net-new architectural placement. DL §5.8 wants "clear why-am-I-getting-this provenance" but never says humanisation lives *at the source*. The doctrine's insight is structural: `"merge_request merged"`, raw ids, unrendered markdown are the #1 "unfinished" tell, and the fix is backend-side humanisation + a routable `ArtifactRef` so every consumer **and every agent-authored message** gets it free. **Binds to Phase-3** (Notifications copy/templating + Reference Graph display-name resolution) and to a DL note. This is the highest-leverage non-editor delta. |
| 34 | **Design-first: IA + flows + wireframes incl. empty/loading/error, reviewed before UI** (§7) | CONFIRMS | VISION §3/§5.2 + DL §7 / §8.3 | VISION §3 (no frontend code without a design sketch) + DL §7 catalogue + §8.3 rule already commit this at canonical status. Validation. |
| 35 | **Reading the design doc reveals "nine menus are three shapes" — design-first pays off concretely** (§7) | SHARPENS | DL §7 + P4 | Sharpens *why* design-first matters with the overlay example (#5). Add to DL §7 as the rationale binding the overlay-primitive catalogue to the design-sketch review. |
| 36 | **The switch test, reached by driving the REAL UI in a browser — done = a team could move without hitting a wall the old tool didn't have; a "does this feel finished?" pass finds a dozen+ issues a checklist misses** (§7) | NEW | **P5** + **P8** | Net-new done-bar. Our planning has no "switch test." Bind as the **frontend definition-of-done** in Phase-5 testing strategy and Phase-8 execution discipline (the design analogue of process-doctrine §4 "actually try it"). |

---

## Genuine conflicts / seams to watch

- **Item #9 (markdown-subset string vs AST content model)** is the only near-conflict, and it's a
  *representation seam*, not a true disagreement. ADR-05 / `myelin-content` is an AST; the doctrine
  wants inline content stored as a markdown-subset string for sanitisation-freedom, paste/export/diff
  survival, and migration-freedom. **Recommended resolution:** AST for block structure, markdown-subset
  string for inline runs — make this explicit in DL-§11 and hand it to the Knowledge P4 sketch and the
  shared-content-model owner. If we instead shipped inline-range JSON, we'd re-pay the lesson the
  doctrine paid. No ADR reopening required; this is a refinement note.

- **No other conflicts.** Everything else CONFIRMS, SHARPENS, or is cleanly NEW.

---

## Prioritized deltas (the 5–8 that matter most)

1. **One editor render path + `render(parse(md))===md` round-trip gate (items #6, #7, #8).** The
   deepest net-new mandate. Binds to **DL-§11**, the **Knowledge P4 sketch**, and a **Phase-5 CI
   gate**. Hands **TE-15** a concrete DEFAULT-TO-BEAT (controlled contenteditable, caret-as-markdown-
   offset, round-trip corpus). This is where a vague "share the AST" becomes a testable correctness
   property.

2. **Overlay primitives day-one: portal-always + one z-index scale + single-purpose-by-shape (items
   #1, #2, #3, #5).** The most expensive retrofit per the doctrine. Binds to **DL-§11** as a
   primitive catalogue and to **Phase-6 sequencing** (build before any feature consumes overlays).
   Net-new specifics (portal-always, z-scale) our DL lacks.

3. **Humanise machine strings at the backend, paired with a routable reference (item #33).** The #1
   "feels unfinished" tell, and structurally it must live in **Phase-3** (Notifications templating +
   Reference Graph name resolution) so agents inherit it too — not a frontend string map. Highest-
   leverage non-editor delta.

4. **The switch test as frontend done-bar (item #36).** Net-new. Binds to **Phase-5** (definition of
   done) and **Phase-8** (execution discipline): a surface is finished only when a team could switch
   to it in a real browser without hitting a wall the old tool didn't have.

5. **Layout-containment + mobile bug mandates: `min-height:0` scroller rule, `width:100%`-isn't-a-
   takeover, hover-isn't-touch, flip-popovers-off-screen (items #28–#31).** Four concrete net-new bug
   classes. Bind to **DL-§11** + the **responsive P4 sketches** (Chat/Issues row actions, composer
   pickers most exposed).

6. **Measured-not-claimed tokens: focus-token ≠ identity-token, measured-contrast gate (items #11,
   #12).** Sharpens DL §4 from "validate contrast" to "measure it, and accept the brand accent may
   fail AA so the focus/primary token differs." Binds to **DL §3.2/§4** + a **Phase-5 measured-
   contrast gate**.

7. **Hard latency budgets (item #27):** keyboard < ~100ms, no spinner-flash < ~1s, pages render not
   animate-in. Promotes DL P2's soft aim to numeric budgets with a **Phase-5 performance gate**.

8. **Proven-vs-house-style tagging across the design system (item #19).** Net-new honesty discipline;
   keeps "accessibility requires this" separate from "we prefer this" throughout DL §3–§4 and DL-§11.

---

## Where each delta binds (digest)

- **Phase-2 back-patch → new DL §11 "Concrete frontend mandates (doctrine-bound)"**: overlay
  primitives + portal-always + z-scale + single-purpose-by-shape (#1–#5); editor render path /
  contenteditable / markdown-string (#6, #8, #9, #10); no-inline-colour-on-interactive (#16);
  reversibility-over-confirmation (#24); the four layout/mobile bug mandates (#28–#31);
  proven-vs-house-style tagging (#19).
- **Phase-2 back-patch → in-place DL sharpening**: focus≠identity + measured contrast (#11, #12 →
  §3.2/§4); typographic hierarchy from weight/colour (#14 → §3.3); agent-iconography ban + no-emoji
  (#18 → §3.7/§6.1); pre-fetch context + notification "why" (#22 → §5.3/§5.8); skeleton/quiet-error/
  fail-static specifics (#26 → §5.10); hard latency budgets (#27 → P2); live styleguide (#17 → §8);
  drawer pattern (#32 → §5.1).
- **Phase-3 (shared systems)**: backend humanisation of machine strings (#33 → Notifications
  templating + Reference Graph display-name resolution; ADR-13).
- **Phase-4 (subsystems)**: Knowledge owns the editor sketch + the inline-representation seam (#6–#10);
  Chat/Issues own the hover-action + width-takeover responsive cases (#29, #30); all subsystems inherit
  the overlay catalogue and DL-§11.
- **Phase-5 (testing)**: round-trip editor gate (#7); measured-contrast gate (#11); latency/
  performance gate (#27); the switch test as frontend definition-of-done (#36); "test popovers/overlays
  against the real anchor" (#31).
- **Phase-6 (roadmaps)**: overlay primitives + editor primitives sequenced *before* the features that
  consume them (#1, #10); the live styleguide as a design-system deliverable (#17).
- **Phase-8 (execution)**: no-inline-colour lint (#16); drive the real UI for the switch test (#36);
  test the real anchor (#31).
