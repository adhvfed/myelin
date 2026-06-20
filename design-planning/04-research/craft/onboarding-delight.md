# R-20 — First-Run / Onboarding Delight Patterns (3 archetypes)

> Phase 4 research corpus item **R-20** (WS-J, Seq #18). Methods: **#20 cognitive walkthrough**
> (learnability, no users) as the spine, **#2 comparative teardown** (Linear / Notion / Slack
> onboarding) as the "what's proven" lens, **#19 heuristics** as the critique lens.
> **File date: 2026-06-20.** No real users exist; P1–P15 are HYPOTHESES (`personas.md` §0).
>
> Builds ON, does not re-derive: design-language **§5.10** (empty = onboarding-forward),
> **§7.6** (onboarding & empty-platform flows; startup vs. enterprise-admin first-run), **P4**
> (progressive disclosure), **P7** (agents visible), **§6** (agent contract). Extends prior
> corpus outputs **R-01** ([`teardown-dossier.md`](../north-star/teardown-dossier.md) — the
> onboarding teardown bar) and **R-04** ([`cross-surface-flows.md`](../jtbd-flows/cross-surface-flows.md)
> — the flows onboarding *leads into*). Feeds **rubric D2** (first-run delight) and every Phase-6
> finalist's empty/first-run state; feeds **sketch-funnel** (the zero-data shell + first-value path
> across Axes 1/2/4/5/6).
>
> Tags (VISION §3 honesty rule): **PROVEN** = cited standard / vendor behaviour / existing
> architecture mechanism we *surface*; **HOUSE STYLE** = our design synthesis. `[VERIFY]` =
> time-sensitive (vendor onboarding mechanics drift). This item is **user-dep: none** — the
> cognitive walkthrough IS the deliverable, not a substitute for a deferred study — but the
> learnability claims it produces are HYPOTHESES until Phase-4 usability testing confirms them
> (§8 records that plan).

---

## 0. How to read this file

The deliverable is **three archetype first-runs**, each structured identically:

1. **The archetype + its first-value bet** — what "first value" *is* for this org, and the
   churn knife (`personas.md` §6: "a weak onboarding/UX story loses [the startup] instantly").
2. **The empty-platform → first-value path** — the guided-start sequence stitching the §5.10
   empty states (first repo → first issue → first doc → first channel → first agent run) into one
   coherent arc, *not* five disconnected blank screens.
3. **The cognitive-walkthrough table** (method #20) — for each first-step, the three CW questions:
   **(Q1) Will the user try to do the right thing?** (is the next action obvious / motivated?)
   **(Q2) Will they notice the control?** (is the affordance visible where they look?)
   **(Q3) Will they understand the feedback?** (does the result confirm progress?). A step that
   fails any question is a learnability defect; we name the fix.
4. **Delight moments + progressive-disclosure rule** — where the wedge (P6) and first agent
   proposal (§6) produce a felt win, and how the *next* archetype's depth stays one layer down.

**Two cross-cutting design laws** (HOUSE STYLE, derived from the teardown evidence in §1) govern
all three:

- **Law A — Onboarding-forward empty, never a tour.** Every empty state is a *workspace that
  teaches by doing*: one line of what-goes-here, one primary action, one optional secondary path
  (import/seed). **No modal tour, no "3 of 7" checklist wall, no coachmark spray.** This is the
  explicit anti-pattern the prompt forbids (the patronizing tour) and the one the proven North
  Star (Linear) *also* refuses — see §1.1.
- **Law B — One arc, disclosed by role.** The startup sees the *shortest* arc (repo → green check →
  first agent proposal). The scale-up and enterprise see the *same* arc with extra rungs (process
  setup; SSO/residency/agent-policy) **disclosed one layer down, never in the startup's face** (P4;
  `personas.md` §6 "from sensible defaults, invisible … to fully controlled and audited … without
  forcing the startup through enterprise complexity").

---

## 1. Onboarding teardown bar (method #2; what's PROVEN, extends R-01)

R-01 §1.1/§2.3 covered the palette/slash-menu *as discovery surfaces*; here we extend it to the
**first-run arc specifically**, with fresh 2025–2026 grounding.

### 1.1 Linear — "anti-onboarding": teach by doing, not by touring  `[VERIFY]`
- **PROVEN behaviour.** ~60-second / 7-step signup with **no role/permission/workflow config**;
  the workspace is **pre-populated with model seed data** (clearly-named projects, hours-scoped
  issues, 2-week cycles) that functions as *behavioural training by example*; **Cmd+K is taught
  before the workspace is populated** as the model for how the product works; **task-based** ("create
  an issue, use the command menu, set a priority") instead of a feature tour; each empty state =
  **one subtle animation + one line of what-goes-here + one button**; explicitly **no product
  tours, no tooltips spray, no "3 of 7" progress checklist, no gamification** (Candu teardown 2025–26;
  Supademo screen-by-screen). The thesis: *"the best onboarding is the one users don't notice."*
- **Steal → Myelin principle.** Steal *teach-by-doing + empty-state-as-launchpad* → serves **P4**
  (powerful on demand, simple by default) and **D2**. Steal *Cmd+K taught first* → serves **P3**
  and Myelin's palette-as-nerve-centre (R-08).
- **The trap (extends R-01 §1.1).** Seed-data-as-training is **load-bearing for engineers but
  dangerous for the regulated buyer** — fake activity in a compliance-sensitive tenant erodes
  trust, and the 2025 SaaS-onboarding consensus warns *against* dummy data that fakes
  stats/activity (ProductLed; Userpilot). **Myelin rule:** seed/sample content must be (a)
  unmistakably labelled as a removable example, (b) one-click-clearable, (c) **off by default in a
  regulated/enterprise tenant** (Law B). Never fabricate *audit-relevant* activity.

### 1.2 Notion — jobs-based questionnaire + template gallery (time-to-value via structure)  `[VERIFY]`
- **PROVEN behaviour.** A **jobs-based question** at start ("what are you trying to do — plan a
  project / build a wiki / manage a team") routes to **curated templates**; a template is
  **duplicated into the workspace in one click**, so the user lands in a *structured*, not blank,
  page (Userpilot/Notion onboarding; super.so/notionapps template coverage 2025–26).
- **Steal → Myelin principle.** Steal *jobs-question → template/seed* → serves **P4** and the
  **§2 dual-audience** start (a PM-led org and an engineer-led org want different first surfaces).
  Steal *one-click duplicate* → serves **P2** (first value is fast, not configured).
- **The trap.** Template-gallery sprawl is its own config maze (R-01 §2.3 slash-bloat trap); and
  Notion's template lives *inside Notion only*. **Myelin rule:** a **short, ranked set of starter
  templates per archetype** (≤4), and a starter template can **seed across surfaces** (a starter
  repo + linked issue + runbook doc) — which Notion structurally cannot (R-01 §2.2 silo trap).

### 1.3 Slack / B2B-enterprise — connect-existing + guided self-service admin  `[VERIFY]`
- **PROVEN behaviour.** PLG products (Slack/Notion/Canva) design onboarding to **shorten
  time-to-value and turn individual actions into team adoption** (Venue PLG playbook). Enterprise
  readiness in 2025–26 = a **self-serve admin portal** that turns SSO/SCIM setup from a multi-week
  services engagement into a guided, self-paced flow; **set up an admin/service user first**, then
  **guided SCIM/SSO configuration** with validation (WorkOS enterprise-readiness 2026; SSOJet SSO
  checklist; Descope/PropelAuth SCIM).
- **Steal → Myelin principle.** Steal *connect-what-you-have* (import an existing git repo / SSO
  directory) → serves the **switch-test (D10)**: first value is often *your real data showing up*,
  not a toy. Steal *guided self-service admin* → serves **P15** and **D9** (sovereignty config is
  legible, not a support ticket).
- **The trap.** Enterprise setup wizards become a **15-screen gate before any value** (the Jira
  config-maze, R-02 territory). **Myelin rule (Law B):** the admin's *value* (a working, compliant
  tenant) and the admin's *team's* value (engineers shipping) are **decoupled** — the admin can
  invite the team into a working default tenant *before* finishing SSO/residency hardening, with a
  visible "hardening checklist" that is **optional-to-defer, not blocking** (§4.3).

**Synthesis (HOUSE STYLE).** The proven pattern across all three: *land in something, not nothing;
teach by doing the real job; disclose depth on demand; never tour.* Myelin's net-new obligation the
North Stars don't carry: **the first-value path spans five surfaces and includes a first agent
proposal** — so onboarding must thread the §5.10 empty states into one arc (§2–§4) and make the
agent's debut legible, not magic (P7).

---

## 2. Archetype 1 — The low-friction startup (P1): near-zero-friction or lost

**First-value bet (HOUSE STYLE).** For the solo/startup (`personas.md` §6: size 1–15, decisive
buyer = the founder/engineer, biggest churn risk = *friction vs. incumbents*), **first value = a
real repo pushed, CI green, and an agent proposing something useful — inside one session, with
zero config.** The knife: "a weak onboarding/UX story loses this segment instantly"
(`personas.md` §6). So the arc must be **the engineer flagship flow F-ENG-1 (R-04 §2) reached as
fast as possible** — onboarding is not a separate experience, it is *the wedge flow with seed
scaffolding removed rung by rung*.

### 2.1 Empty-platform → first-value path (the guided-start arc)

The zero-data shell (README §9 "the *zero-data* shell … the startup persona P1 lands here first")
is **not five blank panels**; it is **one arc with a single highlighted next rung**, the rest
calm-present (P8) but not nagging. Each empty state is Law-A shaped.

| Rung | §5.10 empty state | The onboarding-forward content (one line + one primary action) | First-value signal |
|---|---|---|---|
| 0 | **Zero-data shell** | "Let's get your code in." Primary: **Connect/clone a repo** (import existing — Slack-pattern §1.3) · secondary: **Start from a starter template** (Notion-pattern §1.2, ≤4 starters) | The shell is populated, not configured |
| 1 | **First repo** (§7.1) | repo lands with files; "Open a PR to see review + CI" pulse on the primary action | code is *here*, in-region (P9 cue, ambient) |
| 2 | **First CI run** (§7.2) | "CI runs on your first push — here's the result." Red/green check **with glyph+label** (G1) | the **red→green wedge** is live (F-ENG-1) |
| 3 | **First issue** (§7.3) | offered *in context* from a failing check: "Track this? Convert to an issue" (no empty backlog to stare at) | issue is born *linked* to the run (P6) |
| 4 | **First doc** (§7.4) | offered when the fix lands: "Capture why — start a decision note" (one block, linked) | knowledge is born *linked*, not orphaned |
| 5 | **First channel** (§7.5) | "Invite a teammate / talk to your team" — appears when a 2nd person is invited, not before | collaboration arc opens only when relevant |
| 6 | **First agent run** (§6/§7.6) | "An agent can pre-triage your CI failures — turn it on" → first **plan-then-apply card** | the **first agent proposal** (the delight peak) |

**Design note (HOUSE STYLE):** rungs 3–5 are **offered in-flow at the moment they become useful**
(issue offered *by* a failing check; doc offered *by* a merged fix; channel offered *by* an invite),
not as a checklist the user must clear. This is the difference between *onboarding-forward empty
states* and a *tour*: the product surfaces the next surface **when the user's own action makes it
relevant**, so it never feels patronizing (the forbidden anti-pattern).

### 2.2 Cognitive walkthrough (method #20) — the first steps

| First-step | Q1 try the right thing? | Q2 notice the control? | Q3 understand feedback? | Defect → fix |
|---|---|---|---|---|
| **Connect a repo** (rung 0) | ✅ Founder's mental model = "get my code in"; matches the one primary CTA | ✅ Single highlighted action in an otherwise calm shell (Linear empty-state pattern §1.1) | ✅ Files appear; repo nav populates | **Risk:** import auth (token/SSH) is a cliff → **fix:** offer both *paste a remote URL* and *start-from-template* so a stuck import never dead-ends value |
| **Open first PR / see CI** (rung 1–2) | ✅ Pulse on "Open a PR" + the seeded "CI runs on push" line motivates it | ✅ Checks panel is where R-01 §4.1 trains the eye; glyph+label not colour-alone (G1) | ✅ Red check → one-click step→line (F-ENG-1) is the feedback | **Risk:** opaque red log (the incumbent failure) → **fix:** step→line prefetch (R-13) so feedback is *actionable*, not a log dump |
| **Convert failing check → issue** (rung 3) | ✅ Offered at the exact moment of need, not from a blank backlog | ✅ Inline action on the check card (hover/`.` menu, R-04 §4.2 pattern) | ✅ Issue chip appears *linked* to the run (live backlink, P6) | **Risk:** user doesn't know what an "issue" buys them → **fix:** the offer states the payoff ("track + link to this run"), not the noun |
| **Turn on first agent** (rung 6) | ⚠️ Founder may not trust an agent yet | ✅ One toggle in context, labelled, never a sparkle (P7/§8b.3) | ✅ First action is a **plan-then-apply card** showing proposed effects + authority *before* acting (§6.2) | **Risk:** agent feels like magic/over-reach → **fix:** debut on a **low-stakes, reversible** effect (triage/label), gated, with the Edit path visible (R-14) — earn trust before scope grows |

**Walkthrough verdict (HYPOTHESIS, CW-method PROVEN):** the arc passes Q1–Q3 at every rung *if*
the import cliff and the agent-trust cliff are mitigated as noted. Both are the load-bearing
learnability risks to test first (§8).

### 2.3 Delight moments (named, without a tutorial slog)
- **D-S1 — First red→green** (rung 2): the CI check flips to green with the functional motion
  (R-12 live-update-transition); the wedge appears *as a result of the user's own push*, not a demo.
  **HOUSE STYLE.** Maps **sketch-funnel Axis 1** (dense engineer surface at the calm-startup default).
- **D-S2 — Born-linked artifact** (rung 3–4): the issue/doc the user creates is *already* a live
  chip on the PR (P6 wedge, R-22 territory) — "I made one thing and it connected itself." This is
  the "old stack can't do this" felt moment, surfaced at first-run. **HOUSE STYLE.**
- **D-S3 — First agent proposal** (rung 6): the plan-then-apply card's *legibility* is the delight
  (you see exactly what it will do, on which artifact, under whose authority — §6.2), not a "magic"
  reveal. **HOUSE STYLE; the contract is PROVEN (§6).** Maps **Axis 5** (agent presence) — startup
  default = foregrounded-but-gated.

**Progressive-disclosure rule for this archetype (P4):** the startup **never sees** SSO, residency
console, agent-policy, RBAC depth, or RoPA at first-run. They exist behind the settings surface
(§7.6) at "sensible defaults, invisible" (`personas.md` §6). This is the rung the *next* archetypes
pull up one layer.

---

## 3. Archetype 2 — The scale-up: introducing PMs/process without bureaucracy

**First-value bet (HOUSE STYLE).** The scale-up (`personas.md` §6: ~15–300, "growing pains," the
**PM-vs-engineering tool split (P6) bites hard**, first DPO/security hire appears, "scale process
without splitting tools" — the **core wedge candidate**). First value = **a PM gets a roadmap that
is the same data engineers burn down on a board** (the D1/D5 same-data proof, R-04 F-PM-2), reached
*without* the engineers feeling new process imposed. The risk here is **bureaucracy, not friction**:
onboarding must add a *lens*, not a tool.

### 3.1 Empty-platform → first-value path (the arc, extended one rung)

The scale-up usually arrives **already running** (engineers onboarded via §2, or migrating). The
new first-run surface is **the PM/process lens onto existing data**, plus light governance the first
DPO/EM wants. Law B: this is §2's arc **+ process rungs**, the engineer rungs unchanged.

| Rung | Empty/first-run state | Onboarding-forward content | First-value signal |
|---|---|---|---|
| P0 | **PM lands on empty roadmap** (§7.3) | "Your roadmap reflects real delivery — no parallel doc to maintain." Primary: **group existing issues into now/next/later** | the report *is* the delivery data (R-04 F-PM-2; no Productboard parallel) |
| P1 | **First saved view** (§5.6) | "Save this as your team's roadmap view" — same records, PM lens (config, not a fork, R-16) | dual-audience proof, felt (D5) |
| P2 | **First process rung** (cycles/SLAs/labels) | offered *as the team grows*, not at signup; "Group is getting big — try cycles?" | process is *earned*, not imposed (P5) |
| P3 | **First governance toggle** (the new DPO/EM) | "You've added a DPO — here's the data-rights console" (§7.6), light-touch | governance appears *with the role*, calm |

### 3.2 Cognitive walkthrough (method #20)

| First-step | Q1 right thing? | Q2 notice control? | Q3 understand feedback? | Defect → fix |
|---|---|---|---|---|
| **PM groups issues into a roadmap** (P0) | ✅ PM's job = "show what's shipping"; the empty roadmap states exactly that | ✅ Group/sort controls are the §5.6 views chrome (R-10), discoverable by pointer (P3 second-half) | ✅ The roadmap *is live* — an engineer's transition moves it (R-04 F-PM-2) | **Risk:** PM fears editing "engineers' data" → **fix:** the lens is *read-shaped by default* with non-destructive grouping; copy reassures it's a view, not a fork |
| **Save the PM view** (P1) | ✅ Natural "make this mine" follow-through | ✅ "Save view" in the views toolbar | ✅ View appears in their default-landing (R-06 per-role landing: PM→roadmap) | **Risk:** vocabulary mismatch ("issue" vs "work item") → **fix:** persona-adaptive vocabulary (R-06/R-16), config-held, fracturing-risk flagged |
| **Adopt a process rung** (P2) | ⚠️ Engineers resist imposed process | ✅ Offered as a *suggestion at a growth trigger*, dismissible | ✅ Cycles/SLAs apply as config on the same component, reversible | **Risk:** feels like bureaucracy → **fix:** offer at a real signal (team size, WIP), never at signup; always reversible (anti-Jira-maze, R-02) |
| **First DPO sees data-rights console** (P3) | ✅ DPO's job is exactly this | ✅ Surfaced *when the DPO role is assigned*, in §7.6 | ✅ Console shows the DSR fan-out blueprint (R-04 F-GOV-1) | **Risk:** premature governance noise for non-DPO roles → **fix:** Law B — governance surfaces are **role-gated**, invisible to engineers |

**Walkthrough verdict (HYPOTHESIS):** the scale-up arc's central learnability bet is **P0–P1: does
a PM believe a roadmap-that-is-a-view rather than a doc?** This is the dual-audience hinge (R-16's
deferred both-audience validation) and the first thing to test with paired PM+engineer sessions (§8).

### 3.3 Delight moments + disclosure rule
- **D-U1 — "The report maintains itself"** (P0–P1): the PM watches the roadmap update when an
  engineer transitions an issue, with no copy-paste (R-04 §5 seam dissolved). **HOUSE STYLE.** Maps
  **Axis 3** (unification) and **Axis 1** (calm PM surface vs. dense engineer board, same component).
- **D-U2 — Process that arrives when earned** (P2): the *absence* of an imposed setup is itself the
  delight for the scale-up (contrast: Jira's project-setup gate, R-02). **HOUSE STYLE.**

**Progressive-disclosure rule (P4):** the scale-up sees process + light governance **one layer
above the startup**, but still **not** SSO/SCIM/residency-binding/agent-policy depth — those belong
to the enterprise admin (§4), disclosed only when an admin role appears. The arc grows by *role
arrival*, never by a wall.

---

## 4. Archetype 3 — The regulated-enterprise admin (P15): standing up SSO / residency / agent-policy

**First-value bet (HOUSE STYLE).** The enterprise admin (`personas.md` P15, §6: regulated
enterprise; decisive buyers = CTO + CISO + DPO + procurement; **nothing adopts without passing the
gatekeepers**; biggest churn risk = *failed audit/compliance gap*). First value is **two-headed**:
(a) for the **admin**, a *provably compliant, hardened tenant* (SSO, residency, RBAC, agent policy,
audit) stood up *self-service* without a multi-week services engagement (Slack/WorkOS pattern §1.3);
(b) for the admin's **team**, the §2/§3 value — engineers shipping, PMs reporting — which must
**not be blocked** by (a). Law B is the whole game here: **decouple the admin's hardening arc from
the team's value arc.**

### 4.1 Empty-platform → first-value path (two parallel, decoupled arcs)

| Rung (admin arc) | First-run state | Onboarding-forward content | First-value signal |
|---|---|---|---|
| A0 | **Admin lands on the tenant setup surface** (§7.6) | "Stand up your org — at your own pace. Your team can start now; hardening can finish after." | the **decoupling promise** stated first (anti-15-screen-gate) |
| A1 | **Residency binding** (§7.6 residency console) | "Choose where this tenant's data lives" — region picker with the residency cue (P9, Axis 6 always-on) | sovereignty is *legible and first*, not buried (D9) |
| A2 | **SSO / SCIM** (guided, validated) | guided connector (OIDC/SAML/SCIM) with a **test-connection** step (SSOJet/WorkOS pattern); create a **service admin** first (§1.3) | identity integrated without a ticket |
| A3 | **RBAC + agent policy** (§7.6 agent governance console) | "Set what agents may touch + org kill-switch" — scopes/budgets/delegation visible (R-15) | agent autonomy is *governed before use* (P12/P15) |
| A4 | **Audit + data-rights console** (§7.6) | "Your audit log and DSR tooling are live" — the DSR fan-out (R-04 F-GOV-1) ready for the DPO | the compliance gatekeepers can be passed (D9) |

**The hardening checklist (HOUSE STYLE, the anti-pattern killer).** A0 exposes A1–A4 as a **visible,
persistent, non-blocking checklist** — *not* a modal wizard the admin must complete before anyone
works. Each item shows status (done / recommended / deferred) and *consequence* ("until SSO is
configured, members sign in with email — fine for pilot, required for production"). This is the
Linear-anti-tour principle (§1.1) applied to *enterprise* setup: **teach the depth by surfacing it,
let the admin sequence it, never gate the team behind it.** Defer is a first-class choice, dated and
auditable.

### 4.2 Cognitive walkthrough (method #20)

| First-step | Q1 right thing? | Q2 notice control? | Q3 understand feedback? | Defect → fix |
|---|---|---|---|---|
| **Land + understand "team can start now"** (A0) | ✅ Admin's fear = "this will take weeks"; the decoupling line answers it | ✅ One setup surface (§7.6), the checklist visible but non-blocking | ✅ "Invite team" is enabled immediately, not greyed pending setup | **Risk:** admin assumes everything is blocking (incumbent habit) → **fix:** explicit "deferrable" badges + a "pilot vs production" distinction |
| **Bind residency** (A1) | ✅ Sovereignty is the buying reason (P9) | ✅ Region picker is the first hardening rung; residency cue then persists near data (Axis 6) | ✅ A confirming residency badge appears on the tenant + on artifacts (R-19) | **Risk:** irreversible-choice anxiety → **fix:** state migration consequences honestly (HAX "convey consequences"); confirm before commit |
| **Configure SSO/SCIM** (A2) | ✅ Core admin job | ✅ Guided connector with provider presets | ✅ **Test-connection** gives a pass/fail with a real diagnostic, not a raw error (§8b.5 humanised) | **Risk:** SSO misconfig is the classic dead-end → **fix:** validation step + a working email-auth fallback so a failed SSO never locks the admin out |
| **Set agent policy** (A3) | ✅ CISO/admin must govern agents before trusting them | ✅ Agent governance console (§7.6/§6.4), scopes/budgets/kill-switch legible | ✅ Policy shows *what each agent may touch*, on-behalf-of, with audit link (R-15) | **Risk:** agent governance feels like a black box → **fix:** surface the existing agent-fabric scope/budget mechanics (R-14/R-15), don't invent a new model |

**Walkthrough verdict (HYPOTHESIS):** the enterprise arc passes Q1–Q3 *if* the decoupling is
believed (A0) and no rung is a lock-out cliff (A1 irreversibility, A2 SSO lock-out). These are the
two highest-stakes learnability defects and map directly to the regulated-buyer review (§8; shared
with R-19's deferred P13/P14 review).

### 4.3 Delight moments + disclosure rule
- **D-E1 — "Pass the gatekeepers self-service"** (A1–A4): the admin completes residency + SSO +
  agent policy + audit **without a sales/services engagement** — the time-to-value the incumbents
  can't match for a *compliant* stack (D10 switch test, D9). **HOUSE STYLE.** Maps **Axis 6**
  (sovereignty always-on cues) and **Axis 5** (governed agent presence).
- **D-E2 — "The team is already working"** (A0): the felt relief that hardening doesn't block
  shipping — the inversion of the enterprise-setup-gate expectation. **HOUSE STYLE.**
- **D-E3 — Sovereignty cue debuts at setup** (A1): the residency badge the admin sets at A1 is the
  *same* cue every data-subject later sees near their data (R-19) — the admin's first action seeds
  the platform's visible-sovereignty story. **HOUSE STYLE; cue mechanism PROVEN (R-19/§7.6).**

**Progressive-disclosure rule (P4):** the enterprise admin is the **deepest layer**; everything the
startup never saw lives here, but even *here* it is a self-paced checklist, not a wall. Crucially,
the enterprise admin's *team members* (engineers, PMs) **still get the §2/§3 arcs** — the admin's
depth does not leak into the IC's first-run. This is Law B's final form: **depth is per-role, the
arc is one.**

---

## 5. Cross-archetype synthesis — one arc, three depths

| | Startup (P1) | Scale-up | Enterprise admin (P15) |
|---|---|---|---|
| **First value** | red→green + first agent proposal, zero config | PM roadmap = engineer's data (dual-audience) | compliant tenant, self-service, team unblocked |
| **Dominant risk** | friction → instant churn | bureaucracy / imposed process | setup-gate; lock-out cliffs; failed-audit |
| **Arc depth (Law B)** | shortest (repo→agent), no governance shown | + process + light governance, role-gated | + SSO/residency/agent-policy, non-blocking checklist |
| **Seed/sample data** | labelled starter template, clearable | existing data + PM lens | **off by default** (no faked compliance activity) |
| **Disclosure trigger** | the user's own action makes the next surface relevant | a **role arrival** (PM, DPO) opens its surface | the admin **sequences** the checklist; team value never waits |
| **Delight peak** | born-linked artifact + legible agent debut | "the report maintains itself" | "pass the gatekeepers self-service" |
| **Anti-tour stance** | empty-state-as-launchpad (Linear §1.1) | suggestions at growth signals, dismissible | hardening checklist, deferrable, never modal-gates |

**The single shared spine (HOUSE STYLE):** *empty states are an arc, not a void; each next surface
is offered when the user's own action (or role) makes it relevant; depth is disclosed per-role; and
nothing is ever a forced tour.* The three archetypes are the **same arc at three depths**, which is
exactly the "sensible defaults invisible → fully controlled and audited, without forcing the startup
through enterprise complexity" mandate (`personas.md` §6) made into a first-run design.

---

## 6. Actionability toward the control artifacts

| Control artifact | What R-20 equips | Where |
|---|---|---|
| **rubric D2** (first-run delight / approachability) | Three archetype first-runs, each CW-checked; the empty-state-teaches anchor; "warmth without toylike" via copy-that-states-payoff not nouns; the **anti-tour** stance is the D2-4 ("the empty state teaches") evidence | §2–§5 |
| **rubric D2 ↔ D5** (dual-audience) | The scale-up arc P0–P1 is the *first-run* face of the same-data dual-audience proof (R-16) | §3 |
| **rubric D9** (sovereignty-as-UX) | The enterprise A1/A4 rungs make residency/audit/DSR legible *at setup*; the residency cue seeded at A1 is the always-on cue (R-19) | §4 |
| **rubric D10** (switch test) | "Connect existing repo/SSO" first-value = real data, not a toy; the decoupled enterprise arc is "switch without a multi-week gate" | §2.1, §4 |
| **sketch-funnel — the zero-data shell** | Every finalist's empty/first-run state has a concrete arc to depict, not a blank panel (README §9 zero-data shell, owned jointly with R-21 for the *state craft*; R-20 owns the *arc*) | §2.1, §3.1, §4.1 |
| **sketch-funnel Axes** | Axis 1 (startup-calm vs engineer-dense default), Axis 4 (tone of first-run copy), Axis 5 (agent-debut presence), Axis 6 (sovereignty cue at setup) each get a first-run datum | §2.3, §3.3, §4.3 |
| **Phase-6 requirement (HOUSE STYLE):** | each finalist should sketch **at least one archetype's first-value rung** (the startup's red→green-to-first-agent is the cheapest, highest-signal) as its empty/first-run comparable screen | §2 |

**Boundary with R-21 (avoid duplication):** R-20 owns the **onboarding *arc* and learnability**
(the sequence, the CW, the disclosure rule). **R-21 owns the *state craft*** — the visual/interaction
spec of each empty/loading/error/permission/erased/agent-pending state per component. R-20 says
*"the first-repo empty state offers connect-or-template and leads to the CI rung"*; R-21 says *"here
is exactly how that empty state is rendered, with its skeleton and its tombstone sibling."* Both
reference §5.10; neither re-derives it.

---

## 7. Completeness-critic (README §9) — which gloss-risks R-20 owns vs. routes

R-20 owns the **onboarding-forward empty states + zero-data shell** gloss-risks (README §9
"Unglamorous UI states"):

- **Empty states that are onboarding-forward (first repo / issue / doc / channel / agent run)** —
  **OWNED & covered** as the *arc*: §2.1 stitches all five into one sequence; §3.1/§4.1 extend it
  per archetype. The per-state *render craft* is routed to **R-21** (named, §6 boundary).
- **The zero-data shell (startup P1 lands here first)** — **OWNED & covered**: §2.1 rung 0.
- **Agent-pending / first agent run** — **covered as the arc's delight peak** (§2.1 rung 6, §2.3
  D-S3); the agent *state set* and legibility spec are routed to **R-14/R-15** (named).
- **Permission-denied / erased-tombstone first-run interactions** — **consciously deferred to R-21**
  (state craft) and **R-09** (chip/unfurl no-access); a first-run rarely *starts* in these states,
  so the arc names them but does not own their render.
- **Loading-shows-structure (skeletons)** — **routed to R-13/R-21**; the arc assumes the §8b.6
  skeleton discipline rather than re-specifying it.
- **Conflict / stale / degraded-static / storm** — **out of R-20's scope by design** (these are
  *running-system* states, not first-run); routed to R-21. Naming-and-routing keeps the corpus
  cumulative (standing instruction), no first-run experience is hidden by the omission.

**§9 "onboarding for startup vs enterprise admin" explicitly** (design-language §7.6) — **OWNED &
covered** by §2 (startup) and §4 (enterprise admin), with §3 (scale-up) added as the prompt's third
archetype.

---

## 8. `[DEFERRED-UNTIL-USERS]` — what only users can confirm

R-20 is **user-dep: none** — the cognitive walkthrough is the *deliverable method*, not a stand-in
for a deferred study. But every learnability **verdict** above is a **HYPOTHESIS** (CW with no
users predicts *plausible* trouble spots; it does not measure real ones). The validation is recorded
as an executable plan, **not faked as done**:

- **What to test (first-run usability + CW-with-users):** can the recruited persona reach first
  value **in one session** without hitting (a) the import/auth cliff (startup §2.2), (b) the
  "is-a-roadmap-really-a-view" disbelief (scale-up §3.2 P0–P1), (c) a setup lock-out (enterprise
  §4.2 A1 irreversibility / A2 SSO lock-out)? Measure **time-to-first-green-check**, **time-to-first
  saved PM view**, **time-to-team-unblocked** (admin invites team before finishing hardening).
- **With whom:** startup arc → P1/P2 founders/ICs; scale-up arc → **paired P6 PM + P1 engineer**
  (the dual-audience hinge, shared with R-16's deferred both-audience study); enterprise arc →
  **P15 admin + P12 security + P13 DPO + P14 procurement** (run jointly with **R-19's** deferred
  regulated-buyer review — same recruits, one session).
- **What would falsify the design hypothesis:** (1) startups abandon before first green check
  (friction not actually near-zero → the §2 bet fails); (2) a PM treats the roadmap-view as
  untrustworthy and rebuilds a parallel doc (the dual-audience first-run failed → D5 regressed);
  (3) an admin believes the hardening checklist *is* blocking and stalls the team (Law B not
  communicated → the enterprise bet fails); (4) any rung produces a dead-end (import, SSO) that
  loses the user (anti-tour ≠ anti-help — absence of help at a cliff is its own failure).
- **Caveat (carried to R-14/R-15):** the **first agent proposal** (D-S3) is drawn against the
  **mock** agent runtime; whether a *real* first agent proposal earns trust depends on real-LLM
  output quality. The **contract** (plan-then-apply, low-stakes reversible debut, gated, Edit path)
  is designed to be trustworthy *regardless of runtime* — that is what to validate, not the mock's
  specific suggestion.

---

## 9. Self-check against R-20 acceptance criteria

| Criterion (prompt R-20 / ws-j) | Status | Evidence |
|---|---|---|
| **Three archetype first-runs specified** | ✅ Met | §2 startup (P1), §3 scale-up, §4 regulated-enterprise admin (P15) |
| **Guided-start ties the empty states together** (repo→issue→doc→channel→agent) | ✅ Met | §2.1 arc table (rungs 0–6 = all five §5.10 empty states as one sequence); extended §3.1/§4.1 |
| **Progressive disclosure keeps enterprise depth out of the startup's face** | ✅ Met | Law B (§0); §2.3 / §3.3 / §4.3 disclosure rules; §5 one-arc-three-depths table |
| **Each first-step cognitive-walkthrough-checked** (Q1 try / Q2 notice / Q3 feedback) | ✅ Met | §2.2, §3.2, §4.2 CW tables, each with defect→fix |
| **Delight moments named without a tutorial slog** | ✅ Met | D-S1/2/3, D-U1/2, D-E1/2/3; Law A anti-tour stance throughout |
| **Avoids the patronizing-tour anti-pattern** | ✅ Met | Law A (§0); §1.1 Linear anti-onboarding evidence; offers-at-relevance not checklists; enterprise = non-blocking checklist not modal wizard |
| **Method #20 CW + #2 teardown + #19 heuristics; cited web research** | ✅ Met | CW spine (§2.2/§3.2/§4.2); §1 teardown (Linear/Notion/Slack-enterprise, cited §10); heuristic critique in defect→fix columns |
| **Builds ON R-01 + R-04, doesn't duplicate** | ✅ Met | §1 extends R-01's onboarding bar; the arc *leads into* R-04 flows (F-ENG-1 startup, F-PM-2 scale-up, F-GOV-1 enterprise), named not re-drawn |
| **PROVEN/HOUSE-STYLE tags + date** | ✅ Met | Dated 2026-06-20; tags throughout; vendor mechanics `[VERIFY]`-flagged |
| **§9 gloss-risks addressed (own/route/defer)** | ✅ Met | §7 owns onboarding-forward empty + zero-data shell; routes craft to R-21, agent states to R-14/15, no-access/tombstone to R-09 |
| **Actionable toward rubric D2 + sketch-funnel** | ✅ Met | §6 mapping (D2/D5/D9/D10, axes 1/4/5/6, the zero-data-shell comparable screen) |
| **Deferred validation recorded as a plan, not faked** | ✅ Met | §8 `[DEFERRED-UNTIL-USERS]`: what / with-whom / falsification + mock-vs-real agent caveat |

**Honest partials / top uncertainties.**
1. **All CW verdicts are HYPOTHESES** — CW-without-users predicts trouble spots; the import cliff,
   the roadmap-is-a-view disbelief, and the enterprise lock-out cliffs are the three to test first
   (§8), and any could move the design.
2. **Seed/sample-data policy is a real tension** — Linear's seed-as-training (§1.1) helps engineers
   but the 2025 consensus warns against faked activity, and it's actively wrong for a regulated
   tenant; the "labelled, clearable, off-by-default-in-enterprise" rule (§1.1) is HOUSE STYLE and
   under-evidenced for the regulated archetype specifically.
3. **The agent debut at first-run is a bet** — debuting an agent in the first session may delight
   (force-multiplier for tiny teams, `personas.md` §6) *or* spook a cautious buyer; the low-stakes
   reversible gated debut (§2.2/§8) hedges it, but the *timing* (first session vs. later) is a
   HYPOTHESIS to test, especially for the enterprise.
4. **Enterprise decoupling depends on communication** — Law B only works if the admin *believes*
   the team can start before hardening finishes (§4.2 A0); if that message fails, the design
   collapses into the very setup-gate it avoids.

---

## 10. Sources (web-verified, 2025–2026)

- Linear onboarding / anti-onboarding teardown: https://www.candu.ai/blog/linear-onboarding-teardown · https://www.candu.ai/blog/the-anti-onboarding-strategy-how-linear-converts-philosophy-into-product-adoption · https://supademo.com/user-flow-examples/linear
- Empty-state UI patterns (onboarding-forward, never a void): https://mobbin.com/glossary/empty-state · https://www.useronboard.com/onboarding-ux-patterns/empty-states/ · https://www.setproduct.com/blog/empty-state-ui-design · https://carbondesignsystem.com/patterns/empty-states-pattern/
- SaaS onboarding best practices 2025 (avoid tours; don't fake data; progressive disclosure): https://productled.com/blog/5-best-practices-for-better-saas-user-onboarding · https://userpilot.medium.com/onboarding-ux-patterns-and-best-practices-in-saas-c46bcc7d562f · https://www.guidejar.com/blog/7-saas-onboarding-best-practices-for-2025-that-actually-work
- Notion jobs-based questionnaire + template duplicate-into-workspace: https://userpilot.medium.com/onboarding-ux-patterns-and-best-practices-in-saas-c46bcc7d562f · https://super.so/templates/notion-onboarding-templates · https://www.notionapps.com/blog/best-notion-templates-employee-onboarding-2025
- PLG time-to-value (Slack/Notion/Canva): https://venue.cloud/news/insights/from-signup-to-sticky-slack-notion-canva-s-plg-onboarding-playbook
- Enterprise admin self-service SSO/SCIM onboarding: https://workos.com/blog/enterprise-readiness-checklist-2026 · https://ssojet.com/ciam-101/sso-implementation-checklist-enterprise-security-requirements-for-b2b-saas · https://www.descope.com/blog/post/scim-providers-b2b-saas · https://www.propelauth.com/post/scim-provisioning-what-it-is-and-when-you-need-it

---

*End of R-20 deliverable. Date: 2026-06-20. CW (#20) + teardown (#2) + heuristics (#19) methods
PROVEN (cited); all specific first-run choreography and the two design laws HOUSE STYLE; no first-run
user-validated (§8). Extends R-01 + R-04; feeds rubric D2 and every Phase-6 finalist's first-run
state.*
