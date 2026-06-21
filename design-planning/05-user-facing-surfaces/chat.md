# Surface group: Chat (§7.5)

> Phase 5 surface map · group **H** · maps [`design-language §7.5`](../../planning/02-holistic-architecture/design-language.md)
> against the [§2 template](./README.md#2-the-per-surface-map-template). Pointer map; PROVEN / HOUSE
> STYLE tagged; date 2026-06-20. Cross-cutting obligations ([README §3](./README.md#3)) inherited.
> **The threading model is resolved in [README §4.3](./README.md#43-critic-fix-3--the-chat-threading-model-resolve-explicitly):
> adopt Zulip-style topics-within-channels** (agent-volume is the deciding factor). **H-9 (the HITL card)
> is the funnel's recommended agent moment**; **H-5 (live unfurl) is a wedge candidate.**

---

## H-9 — HITL approval-card surface *(agent flagship; funnel target)*
1. **Jobs:** E11/M9 plane, agent flagship. Flow **F-AGT-1** §7. 2. **IA + shell:** `Chat → <channel>` (the card in-stream) **and** mirrored in `[G] Inbox` (S-4) so a gate is never missed. Chat is the primary home (system-overview §8.2).
3. **Components:** **the HITL card (R-10 §5 / R-14 §2)**, chip/unfurl (per-effect targets as live chips), overlays.
4. **Density:** 0.3 — earns via J3 (structured proposed-effects layout) but is **one card** appearing in many places, never per-subsystem agent UIs (the bolted-on-bot-console fracture, R-07 §2).
5. **Agent (R-14 §2/§3/§5 — the D6 flagship):** **plan-then-apply** card shows proposed effects per artifact + delegated authority + per-effect gate-marker + scope + budget *before* anything happens; **Approve / Edit / Reject** (Edit re-runs the full pipeline within scope, re-checks delegation/tenant/budget). The 10 agent states (pending→working→gate-awaiting→{approved/edited/rejected} + agent-error/budget/loop-guard/denied/stale). Provenance "Why?" + audit link (R-15 §1).
6. **Sovereignty:** agent scope/delegation visible ("FixAgent, on behalf of @dev, may: open PR #88"); cross-cell effect → Denied with the missing grant named, never leaks target (R-04 §7.2).
7. **State set (R-21 §2f — HITL surface **owns** agent-pending col 6):** gate-awaiting; gate-rejected ("Rejected by <human> · <reason>", PR discarded); gate-edited (diff between proposed and amended); stale-approval ("base changed — re-propose?"); **approval-card storm** → inbox collapses to "7 approvals awaiting you" (R-15 §5.2).
8. **A11y (R-17 §5.5 HITL hard component):** keyboard (Approve/Edit/Reject + per-effect controls individually focusable); SR (proposed effects, target, authority+GATE, scope, budget in accessible name; **agent treatment as TEXT not colour/icon**, WCAG 1.4.1; arrival announced politely; state transitions as text). **G2:** humanised effect descriptions; locale-aware budget/time.
9. **Device:** **approve/reject from inbox on mobile is allowed** (the gate must be reachable, S-4); **Edit defers to desktop** (MOB-6) — amending a proposed effect is authoring; popover under the composer flips (MOB-3).
10. **Wedge/motion:** **W6 (one `correlation_id` across all five surfaces)** — the chain (CI→issue→chat→PR) reads as one story, approval *in chat where the team is*; reserved `motion.agentEnter` on card arrival, `motion.agentResolve` on Approve/Reject. **No sparkle** (R-12 anti-list, §8b.3). 11. **DoD + switch:** every proposed effect shown before it happens; never a silent agent edit; never colour-alone for agent state; the approval lives in chat, not a separate ops console (F-AGT-1 🔪). This is the D6 score surface.

## H-2 — Message timeline view *(Axis-3 0.7; densest non-engineer stream)*
1. **Jobs:** E9 (incident as one timeline), M6. Flow **F-PM-1**. 2. **IA + shell:** `Chat → <channel>`; content; **with Zulip-style topics** (§4.3) — a topic = a `correlation_id` chain.
3. **Components:** chip/unfurl (R-09, inline + live), composer (H-3), reactions (R-10 §5.5, the low-friction-ack idea stolen from Slack without emoji-as-UI, R-01 §3.4).
4. **Density:** **0.7** — earns via J1 (reverse-chrono live stream, presence, typing, threading) + J3 (unfurl-in-place). 5. **Agent:** **agent volume routed OUT of the main timeline** (R-13 §B.4, R-15 §5.1) — into topics/collapsible summaries/inbox; calm-by-default (P8).
6. **Sovereignty:** message hidden if channel-scoped (no leak); cross-space refs gated.
7. **State set (R-21 §2f — chat timeline **owns** stream-drop/resume with live log, col 8):** message skeletons; erased message → "Message deleted" tombstone (thread intact); **🔪 notification storm** → inbox dedups by `origin_event`, collapses "23 updates on INCIDENT-9", agent volume out of stream (R-04 §4.2); stale/reconnecting (resume on `chat.*` replay).
8. **A11y/i18n:** virtualized infinite scroll must not break SR; new-message live region politeness (R-17 §6.1, no spam); **G2** — message prose mirrors in RTL, mixed-direction (Arabic prose + LTR `@handle`/ref) bidi-isolated (R-18 §4.4). 9. **Device (MOB-1):** message hover-actions (react, convert-to-issue, thread) touch-reachable; **MOB-3** composer-anchored popovers flip; timeline is a primary mobile surface.
10. **Wedge/motion:** unfurl-in-place; coalesced updates during a storm (R-12 R2, no strobe). 11. **DoD + switch:** an incident runs as one linked timeline (issue + channel + CI + runbook), agent chatter never drowns humans (the Teams firehose / Slack-flat-channel-agent-pollution regression dissolved, §4.3 rationale).

## H-5 — Unfurl cards *(the wedge; Axis-3 0.05)*
1. **Jobs:** the wedge, all flows. Flow F-PM-1 (runbook unfurls in thread). 2. **IA:** in H-2/H-3. 3. **Components:** **the §5.3 / R-09 chip→card** — chat is the densest consumer (R-01 §3.1).
4. **Density:** 0.05 (minimal — per-artifact-*type* rendering ≠ distinctness; one component, R-07 §2). 5. **Agent:** agent-proposal unfurl = Approve/Edit/Reject (= H-9). 6. **Sovereignty (PROVEN hard rule):** **live, not snapshot** (beats Slack's cached-snapshot rot); **permission-aware per viewer** (cross-space no-access → graceful card, never the title); tombstone on erasure (R-09 §5.3 four guarantees, R-02 R-LEAK).
7. **State set (R-21 §2a — chip/unfurl **owns** no-access/tombstone/moved/cross-cell, cols 4/5/11/12):** all 9 resolver states (R-09 §1.1). 8. **A11y:** card body skeletons (chrome instant); inline-action buttons keyboard-reachable; **G2** humanised. 9. **Device (MOB-3):** **calm chip-by-default** — one card auto-expands, the rest stay compact chips (R-22 anti-wedge: never auto-expand every ref into a Slack-noise wall); card flips above the composer.
10. **Wedge/motion:** **W2 (paste-a-link, unfurls live with inline action)** — re-run a CI job, transition an issue, approve a PR *from the card*; `motion.enter` on expand, `motion.liveUpdate` on in-place refresh. 11. **DoD + switch:** the unfurl is live + permission-aware + actionable (Slack's snapshot-that-rots, 404s-private, preview-not-action regressions all dissolved).

## H-1 / H-3 / H-4 / H-6 / H-7 — channel list · composer · thread pane · activity inbox · search
- **H-1 Channel / conversation list** (sidebar): `Chat` sidebar; sections (channels/DMs/topics/mentions), unread/mention markers. Density 0.3. **With topics (§4.3)**, the list shows channel→topic structure. MOB-2: → drawer.
- **H-3 Composer** (R-10 §3): rich text, slash-commands (= palette verb vocabulary, R-08), `@mention` autocomplete (humans+agents+artifacts), paste-URL-to-unfurl, code blocks, file upload. Density 0.5. **MOB-3/MOB-4:** bottom-pinned; pickers flip above; `min-height:0` so it never drops below the fold. SR: same editor model as K-1.
- **H-4 Thread pane** (R-15 §5.1): side-by-side/overlay; **where most agent/incident detail lives** (calm-by-default, §6.5). Density 0.6. With topics, this is the topic view. MOB-2: → full-screen on narrow.
- **H-6 Mentions / "Activity" inbox**: feeds the unified `[G] Inbox` (S-4). Density 0.2. One read-state truth (R-10 §4).
- **H-7 Search view**: messages + artifact-scoped, permission-filtered (R-08). Density 0.3.

## H-8 — Incident / "canvas" view `[UNCERTAIN/DEFER]` *(resolved in [README §4.3](./README.md#43))*
1. **Resolution: adopt the thin incident pinned-summary; defer the heavy freeform canvas.** Job-backed by **E9** (incident as one linked timeline) + **F-AGT-1** (agent posts summary).
2. **IA:** `Chat → <channel>` (pinned atop an incident channel/topic). 3. **Components:** pinned unfurl/summary block (R-09) + Knowledge embed (R-10 §3) — *cheaply expressible over the existing thread*, not a new authoring surface.
4. **Density:** 0.6. 5. **Agent:** "Summary agent drafting…" agent-pending (R-04 §4.2). 6. **Sovereignty:** inherits channel visibility.
7. **State set (R-21 §2f):** agent-pending summary; live-update as the incident evolves. 8. **A11y/i18n:** pinned region landmark; humanised. 9. **Device:** read-friendly pinned summary on mobile.
10. **Motion:** `motion.liveUpdate` as summary updates. 11. **DoD + switch:** an incident has a pinned structured summary (issue + timeline + runbook links) without a separate war-room tool. **Deferred:** the full Notion-class collaborative canvas/whiteboard — gated on the P4 designer-canvas scope decision (design-language §9), **not** invented by Phase 6.

---

**Group invariants reminder:** the unfurl in chat (H-5) is the *same* R-09 chip as in the diff, the
issue, and the backlinks panel — chat is its densest consumer, not a different component. The HITL card
(H-9) is the *same* card whether it surfaces in chat, the inbox, or inline on a PR (R-07 §3 invariant 8).
The threading recommendation (Zulip topics) is a **carried, unvalidated bet** ([README §4.3](./README.md#43)).
