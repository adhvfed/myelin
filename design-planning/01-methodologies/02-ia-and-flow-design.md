# 02 — Information Architecture & Flow Design

> Covers the macro-process frame plus the methods that turn the §7 view catalogue into navigable IA and
> concrete flows. Tags: **PROVEN** / **HOUSE STYLE**. The no-user constraint is flagged per method.

---

## 5. Double Diamond as the macro-frame — **ADAPT**

**What it is.** The Design Council's Double Diamond (2005; "Framework for Innovation" update 2019): two
diamonds — **Discover → Define** (problem space) then **Develop → Deliver** (solution space) — alternating
divergent and convergent thinking. (Source: Design Council; Wikipedia.)

**Why it fits Myelin (specifically).** It's the cleanest mental map of *our actual phase pipeline*: Phase 1
research + this methodology selection = Discover; Phase 2 roadmap + Phase 5 surface mapping = Define; Phase 6
fifteen sketches = Develop (deliberate divergence); Phase 7 judging + Phase 8 framework = Deliver
(convergence). It's orientation, not a deliverable.

**How WE would use it.** Purely as a shared vocabulary for *where we are* and *whether we're diverging or
converging* — e.g. Phase 6's fifteen sketches are *intentional divergence*; resist converging too early
(Phase 7's job).

**Effort/cost.** Negligible. **PROVEN.**

**Uncertainties & risks.** It can become ceremony. We ADAPT (not ADOPT) it: a frame, never a gate; the real
gates are the methods below.

**Verdict: ADAPT.** Use as orientation only; don't manufacture diamond-shaped artifacts.

---

## 6. Information architecture design (expert-led) — **ADOPT**

**What it is.** Structuring content and functionality so people can find and understand it: the navigation
model, labelling/taxonomy, grouping, and the relationships between objects. (Source: NN/g IA study guide.)
Expert-led IA produces a *proposed* structure without users; card sorting/tree testing (#7) *validates* it.

**Why it fits Myelin (specifically).** Myelin is *one product, not five tools* (P1), and the navigation shell
(design-language §5.1) is the physical embodiment of that promise — primary nav (Code · CI · Issues ·
Knowledge · Chat · Inbox · Search), contextual sidebar, content, context pane. IA is the method that decides
how five subsystems' object models (repo→PR→diff; space→page→block; channel→thread→message) collapse into one
coherent, deep-linkable structure (ADR-13 `ArtifactRef` down to sub-artifact). It's also where the
dual-audience default-landing question lives (PM lands on roadmap, engineer on cycle board — design-language
§2). The §7 catalogue is essentially an IA inventory waiting to be structured.

**How WE would use it.**
- *Phase 5:* produce the **platform IA** — the unified object/navigation model across subsystems, the
  labelling scheme (the "issue" vs "work item" persona-adaptive vocabulary, design-language §2), and the
  URL/`ArtifactRef` structure. This is the backbone of the surface map.
- *Phase 6:* each sketch inherits the IA (sidebar, breadcrumb, deep-link structure).

**Effort/cost.** Medium. **PROVEN.**

**Uncertainties & risks.** Expert IA encodes *our* mental model, which may not match users' (the precise gap
card sorting closes). Cross-subsystem labelling is genuinely hard (the persona-adaptive vocabulary is an open
question, design-language §9). Mitigation: design the IA to be tree-tested in Phase 4; keep labels in tokens/
config so they're cheap to change.

**Verdict: ADOPT.** Unavoidable and high-leverage; just sequence its validation (#7) into Phase 4.

---

## 7. Card sorting / tree testing — **ADAPT (defer execution)**

**What it is.** **Card sorting** (participants group labelled cards) *generates* an IA by surfacing users'
mental models; **tree testing** *evaluates* a proposed IA by asking participants to find items in a text-only
hierarchy. Both are participant-driven. (Source: NN/g.)

**Why it fits Myelin (specifically).** These are the methods that would *validate* the #6 IA against the
hardest Myelin risk: that the cross-subsystem navigation and the persona-adaptive vocabulary make sense to
*both* engineers (P1–P5) and PM/corporate (P6–P11). Tree testing the §7 catalogue would directly answer "can
a PM find the roadmap and an engineer find the diff in the same shell?" — the dual-audience mandate as a
findability test.

**How WE would use it.**
- *Phase 2:* plan the studies (define realistic task scenarios from the jobs catalogue).
- *Phase 4 (deferred — needs participants):* run a hybrid/closed card sort to refine taxonomy, then tree-test
  the Phase-5 IA before Phase-6 sketches harden it. Run separately for the engineer and PM segments to expose
  the dual-audience split.

**Effort/cost.** Medium; **blocked on users.** **PROVEN.**

**Uncertainties & risks.** Pure no-user blocker. There's a sequencing circularity: tree-test tasks come from
jobs that are themselves unvalidated (README §5.8). Mitigation: Phase 2 sequences IA validation *after* the
first JTBD interviews so tasks are grounded.

**Verdict: ADAPT (defer).** Cannot run now; plan now, run in Phase 4. Do not let Phase 6 treat the IA as
validated before this happens.

---

## 8. Service blueprinting (cross-subsystem flows) — **ADOPT**

**What it is.** A service blueprint maps a user journey against the *layers beneath it*: frontstage actions,
backstage processes, and supporting systems, including the hand-offs between actors. (Source: service-design
practice; NN/g.) It's an expert synthesis method — it doesn't strictly require users to produce value,
though user research enriches it.

**Why it fits Myelin (specifically).** Myelin's defining flows are *cross-subsystem and cross-actor, including
agents* — agent-native-design.md §8's worked examples are literally service blueprints waiting to be drawn:
"CI fails → triage agent opens issue → links commit → posts chat → proposes fix PR → HITL approval → human
approves → review agent comments." Blueprinting is the right tool because these flows cross subsystem
boundaries *and* the human/agent boundary, with backstage event-bus/trigger/HITL machinery (ADR-08/09) that
must surface correctly frontstage. It's also how we design the **sovereignty/GDPR flows legibly** (P9): a DSR
"erase this subject across all holders" blueprint (design-language §7.6) shows where the DSR orchestrator,
audit log, and tombstoning surface to the DPO (P13).

**How WE would use it.**
- *Phase 5:* blueprint the **flagship cross-subsystem journeys** — the PR context-pane flow (the wedge
  flagship, system-overview §8.1), the agent CI-triage flow (the HITL flagship, §8.2), the DSR/erasure flow
  (§8.3), incident response (P3/A5). Each blueprint shows frontstage screens (mapped to §7 views), the
  backstage events/triggers, and every actor including agents.
- *Phase 6:* these blueprints are the spec the multi-screen sketches implement.

**Effort/cost.** Medium-high (cross-subsystem detail). **PROVEN.**

**Uncertainties & risks.** Blueprints can balloon; agent flows have non-deterministic branches (the agent
*decides*) that don't blueprint cleanly. Mitigation: blueprint the *plan-then-apply* contract (propose →
gate → apply) which is deterministic even when the agent's content isn't; keep blueprints to flagship flows,
not every path.

**Verdict: ADOPT.** The natural tool for Myelin's integration thesis and agent/HITL flows; uniquely good at
making the backstage event/agent machinery legible frontstage.

---

## 9. User-flow / job-flow & jobs-story mapping — **ADOPT**

**What it is.** Step-by-step flow diagrams of a user accomplishing a task (screens + decisions + states),
and "jobs stories" connecting JTBD (#1) to concrete flows. Lighter-weight than blueprints; one actor, one
goal. (Source: standard interaction-design practice.)

**Why it fits Myelin (specifically).** Every sketch in Phase 6 needs the flow *behind* it — not just the happy
path but the §5.10 empty/loading/error/permission-denied/erased states that VISION §3 and design-language
§5.10 *require* in sketches. Flow mapping is where we enumerate those states per screen (e.g. the unfurl
card's live/no-access/tombstoned states, design-language §5.3). It's also how keyboard-first flows (P3) get
specified — the `j/k`/command-palette path through a board or diff is a flow, not a static screen.

**How WE would use it.**
- *Phase 5:* for each primary §7 view, a flow that enumerates entry points, the keyboard *and* pointer paths
  (P3 dual-modality), and all §5.10 states. Output is the per-surface "states checklist."
- *Phase 6:* sketches must depict the states the flow enumerates (not just the happy path).

**Effort/cost.** Low-medium per flow; high in aggregate (many views). **PROVEN.**

**Uncertainties & risks.** Volume — the §7 catalogue is large. Mitigation: prioritise flagship surfaces;
reuse the shared-component flows (one comment-thread flow serves PR/issue/doc/chat per design-language §5.5).

**Verdict: ADOPT.** The connective tissue between IA, blueprints, and the actual sketches; the §5.10 state
checklist is non-negotiable per VISION.

---

## SKIPs in this theme (do not relitigate)
- **First-click testing / live findability metrics — SKIP now (no users).** Subsumed by tree testing in
  Phase 4.
- **Journey mapping as a *separate* deliverable — SKIP.** Folded into service blueprinting (#8), which is the
  same artifact with the backstage layers we actually need for the agent/event machinery.
