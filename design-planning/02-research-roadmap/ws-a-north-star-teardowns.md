# WS-A — North-Star Teardowns & Trap Audit

> Workstream A of the research roadmap (see [`README.md`](./README.md)). The single highest-leverage
> no-user method (Phase-1 method #2 teardown): competitors' users already stress-tested their designs.
> Produces the **comparative judging baseline** the rubric and Phase 7 lean on. Tags: PROVEN.

---

## R-01 — North-Star teardown dossier (Linear · Notion · Slack · GitHub)

**Questions answered.** What exactly makes Linear feel instant and keyboard-native? How does Notion's
block editor + database/views model actually work, screen by screen? How do Slack's unfurls,
slash-commands, and threading behave? What is GitHub's PR/diff/review bar Myelin must meet? For each:
what to *steal*, how Myelin *adapts* it to P1–P9, and the *trap* hiding inside the pattern.

**Phase-1 methodology.** #2 comparative/competitive teardown (ADOPT). Paired with #19 heuristics as the
lens for "why it works."

**Inputs.** `competitive-landscape.md` §1–§5 (the named North Stars/steal lists); design-language §5
(the shared components each teardown maps onto), §7 (the view catalogue); `VISION.md` §1.

**Deliverable.** `design-planning/04-research/north-star/teardown-dossier.md`. Must contain, per North
Star, a screen-by-screen teardown mapped to the §7 catalogue and the §5 shared components: **Linear** →
command palette, issue board/triage/cycles, speed/optimistic mechanics; **Notion** → block editor, the
database/views primitive (the §5.6 reuse boundary), slash menu, mentions; **Slack** → unfurl card,
slash-commands, threading (+ Zulip topic model as a contrast for agent volume); **GitHub** → PR
overview, diff/files-changed, batched review, Checks API surfacing. Each entry: *pattern → why it works
(evidenced) → how Myelin adapts it to which principle → the trap to avoid*. Date the dossier (agent/AI
features move fast — `[VERIFY]` the time-sensitive ones).

**Sequencing & dependencies.** Seq #1. No dependencies; a foundational parallel-start item. Unblocks
R-02, R-08, R-09, R-10, R-20, and the rubric's comparative baseline.

**User-dependency.** none.

**Effort.** L (hands-on product walkthrough + write-up across four products).

**Acceptance criteria.** Every §5 shared component has at least one North-Star teardown entry behind it;
every "steal" is paired with the Myelin principle it must serve (not "they do it"); time-sensitive
agent/AI features are dated and `[VERIFY]`-flagged; the dossier is usable as a Phase-7 "meets/beats the
North Star or regresses" baseline.

---

## R-02 — Trap / anti-pattern audit (Jira · Atlassian · Teams)

**Questions answered.** What specifically makes Jira slow/over-configurable, Atlassian feel
stitched-together, and Teams bloated — at the *interaction and IA* layer, not just brand? Which of these
traps does Myelin's architecture make easy to fall into anyway (e.g. progressive-disclosure done wrong
re-creates Jira's config maze)? What is the explicit "do-not-do" register?

**Phase-1 methodology.** #2 teardown (the avoid half); #19 heuristics (which Nielsen/P1–P9 heuristic each
trap violates).

**Inputs.** `competitive-landscape.md` §3/§6 (the traps), §6.1 (the "stitched-together" failure);
design-language §2 (the dual-audience compromise trap), P4 (progressive disclosure), P8 (calm).

**Deliverable.** `design-planning/04-research/north-star/trap-audit.md`. A register of named
anti-patterns: each row = *the trap → where it shows in the incumbent → the principle it violates → the
Myelin design rule that prevents it → the surface most at risk of re-creating it*. Must cover at least:
config-maze (Jira), stitched-together identity/permission/UI seams (Atlassian), notification overload
(all), the dual-audience "serves neither" compromise (§2), enterprise-density-without-calm.

**Sequencing & dependencies.** Seq #2. Depends on R-01 (shares the teardown method/format). Feeds the
rubric's D7/D10 anchors and the completeness-critic.

**User-dependency.** none.

**Effort.** M.

**Acceptance criteria.** Each trap maps to a specific violated principle and a specific Myelin
surface-at-risk; the register is phrased as falsifiable design rules Phase 5/6 can be checked against;
no trap is left as a generic complaint ("Jira is bad").
