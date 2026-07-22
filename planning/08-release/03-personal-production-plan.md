# Personal Production Plan — the P-track (Tier P between Dogfood and Design Partners)

_2026-07-22, against HEAD `6c605c20`. Founder decision recorded: the primary launch is **personal
production in 6–12 months** (2027-01 → 2027-07). This document amends the post-R4 horizon of
[01-technical-release-plan.md](01-technical-release-plan.md); R0–R4 are unchanged and in flight.
The commercial track ([02-commercial-plan.md](02-commercial-plan.md)) stays intact but its clock is
re-keyed: Tier B/GA promotion becomes **demand- and decision-triggered, not schedule-driven** (§6).
Per VISION §3 "quality over plan-adherence" and the deviation rule in VISION's preamble, this is the
written-down deviation._

## 1. The reframe, and the velocity math that forces it

Demonstrated throughput is ~300 commits/week sustained (weeks 2026-28/29; 1,302 total since 2026-06),
with the R-track's own estimate putting Tier D (dogfood) around **Sep 2026**. The founder's launch
window for personal use opens **Jan 2027**. That leaves roughly **4–9 months of surplus capacity
beyond the point the current plan runs out of mandated work** — on the order of 8,000–15,000 further
commits.

The failure mode this plan exists to prevent: **velocity without a consumer.** A system built years
ahead of any user accumulates speculative machinery that reality would have deleted, and its
verification (drills, synthetic load) drifts from the failure modes that matter. The R-track's own
deepest lesson (reviews/2026-07-06: every critical at the seam where product meets user) generalizes:
*the constraint on this project is not code production — it is contact with reality and maintenance
of trust.* So the surplus is not spent widening toward GA features. It is spent making the dogfood
instance indistinguishable from production, expanding what "dogfood" covers until it is the founder's
entire engineering life, and hardening the ratchets that keep a machine-built codebase trustworthy
over a multi-year horizon.

One sentence: **after R4, the platform is done proving itself with drills; the next proof is living
on it.**

## 2. Tier P — Personal production (new tier, between D and B)

- **Tier D — Dogfood** (unchanged): Myelin hosts Myelin's git + CI. Gate: R0–R3 complete.
- **Tier P — Personal production** (NEW): Myelin is the founder's daily driver for **all**
  engineering work — every active repository (Myelin and non-Myelin, per-repo opt-in), their CI,
  the founder's issue tracking and notes, and the autonomous builder itself operating *through* the
  platform. Run on a real EU production cell with paging, drills, and rehearsed upgrades. This is
  the launch the founder means by "launch for personal use."
- **Tier B — Design-partner beta** (unchanged in content, re-keyed in trigger): first external data.
  Gate: Tier P held + R6 subset `B` + **founder's explicit go decision** (§6).
- **Tier GA** (unchanged).

**Tier P promotion gate (quantified, per doctrine — the gates are the truth):**

| # | Condition | Proof artifact |
|---|---|---|
| TP-1 | 8 consecutive weeks of the founder's daily engineering work entirely on Myelin, including **≥1 active non-Myelin project**, with zero GitHub fallback events (mirror pushes excluded) | dated usage ledger, auto-generated from platform events |
| TP-2 | Scheduled backup/restore drill green **monthly on real production data** to a clean target; RPO/RTO from thresholds.toml met on real volumes | drill attestations |
| TP-3 | Upgrade drill green: N-1→N schema migration rehearsed on a production snapshot, and ≥3 real production deploys executed through the rehearsed path (§4 P0.3) | drill attestations + deploy log |
| TP-4 | Paging live: a red permanent gate, failed drill, or golden-signal breach pages the founder within 5 min; ≥1 real page handled per the runbook | alert config + incident notes |
| TP-5 | The builder agent operates through Myelin itself (§4 P2): its pushes traverse the Myelin wire, its work is tracked in Myelin issues, its steering surface is a Myelin channel | platform audit events |
| TP-6 | All permanent gates green **including the Firecracker half of AG-D4** (R0.1 attested on the production path) and the promoted supply-chain floor (§4 P0.2) | scorecards, machine-generated |

## 3. Standing longevity ratchets (start now, not a phase)

These bind from today and run for the life of the project. They answer "how does a machine-built
codebase stay trustworthy at 300 commits/week for years." Each is a ratchet in the EI-01 §5 sense:
committed, fail-closed, with red/green fixtures where applicable.

- **L1 — Gates re-arm themselves.** Every attestation/scorecard re-runs on a schedule (CI cron) and
  on any commit touching its subject area; scorecard files are **generated from gate output, never
  hand-edited** (enforced: a lint rejects manual edits outside the generator). A gate whose evidence
  is older than its re-arm window is *stale* and pages, exactly like red. Rationale: the
  make-it-real scorecard sat RED-and-stale for three weeks while the work it gated landed — at this
  velocity, any truth that depends on an agent remembering will rot. The ledgers-in-commit-messages
  become *derived* views; the gate output is the record.
- **L2 — Erosion budget.** New lints: production module size ceiling (soft 3,000 / hard 5,000 lines
  excluding `#[cfg(test)]`; the 9,813-line `git_durable.rs` is the trigger incident and the first
  burndown item), plus dependency-direction enforcement between crate layers. Existing files over
  the ceiling are enumerated in a burndown allowlist that only shrinks (same discipline as
  `claimed_not_proven`).
- **L3 — Compounding-payoff metric, measured.** The doctrine's "features getting harder means the
  substrate is wrong" becomes a number: lines-changed-per-plan-row and files-touched-per-plan-row,
  tracked per completed row, trend reviewed at every phase boundary. Rising trend = stop-and-repair
  signal, per master-sequencing §1 closing.
- **L4 — Traceability is mechanical.** Commit subjects cite their plan row (R-/P-tag) — enforced by
  a CI check on commit-message shape, not convention (the tagging discipline held for ~600 commits
  and then silently lapsed; convention does not survive velocity).
- **L5 — Contract honesty across language boundaries.** Shared golden request/response vectors
  consumed by both Rust edge tests and the TypeScript dev-edge/contract tests, registered in
  `contract-coverage.toml` (already steered 2026-07-22; recorded here as permanent doctrine: any
  mock or second implementation of a contract must be gated against the same vectors as the real
  one, or it is a fiction).

## 4. The P-track (after R4; absorbs and re-scopes R5)

Phases are sequential except where noted. Standing process from the README (one builder per prompt,
adversarial verifier on security-load-bearing prompts, evidence over assertion) applies verbatim; a
new ledger `planning/system-reviews/2026-06-26/15-personal-production-ledger.md` is created when P0
execution starts.

### P0 — Production-for-one (absorbs R5.1–R5.3, R5.5; ≈3–4 weeks)

- P0.1 EU production cell as R5.1 wrote it (Scaleway fr-par / Hetzner, IaC'd, managed PG/Valkey/
  NATS/S3). Sized for one human tenant plus agents; multi-cell stays architectural.
- P0.2 **Promoted from R6 to Tier P** (a public-internet instance holding all the founder's source
  deserves them regardless of external users): R6.2 supply-chain floor (`cargo audit`/`deny`,
  SBOM, signed releases) and R6.6 fail-closed release gate wired into Myelin's own CI. The external
  pentest (R6.3) stays at Tier B, but the R2 adversarial red-team campaign is **re-run against the
  production deployment** as a P0 exit condition.
- P0.3 R5.3 upgrade path, hardened into a **permanent drill** (TP-3): blue-green or documented-
  downtime deploy, N-1 schema-migration rehearsal on a prod snapshot, rollback rehearsed. This drill
  class is new to the catalogue — repeated *operation* fails differently than repeated construction.
- P0.4 R5.2 observability + paging (TP-4), including L1's stale-gate paging. R5.5 edge hardening.
- P0.5 R5.4 self-host packaging floor moves here only if trivially cheap; otherwise stays Tier B
  (its buyer is the design partner, not the founder).

### P1 — Full personal cutover (≈2–3 weeks, then continuous)

- P1.1 Every active founder repository migrates, **per-repo opt-in, each with a GitHub read-only
  mirror held for a full quarter** (R4.1 discipline extended). Named floor, stated plainly: hosting
  the founder's commercial work on a solo-operated platform is a risk decision; the mirror is the
  honesty. A repo may stay GitHub-primary with written reason.
- P1.2 Founder's issue tracking and project notes become Myelin-primary (issues via the R4.4 loop
  already; knowledge floor per P3 order). The planning corpus (`planning/`, `reviews/`) migrates
  into the knowledge subsystem **when P3 admits it, not before**.
- P1.3 GitHub demoted to mirror-and-nothing-else; TP-1 clock starts.

### P2 — The builder becomes the first real agent (≈3–5 weeks)

The strategy-pattern seams (VISION §3 "mock agents during development") get their first real
implementation, and it is the agent that built the platform. This is the agent-native thesis proven
on itself, and it exercises R2.4 MCP governance, notifications, and the event fabric with a real
consumer:

- P2.1 The builder's pushes traverse the Myelin git wire under a real agent principal; its merges go
  through Myelin PRs gated by Myelin CI (it already lives under branch protection semantics from R0.2).
- P2.2 The builder consumes the **durable notification inbox** (closing the dark-read-path floor
  recorded 2026-07-22) as its work queue: steering messages, CI results, review requests.
- P2.3 The founder⇄builder steering conversation moves from a tmux session to a **Myelin chat
  channel** (chat's first real floor: one channel, two principals, artifact references). The tmux
  session remains as break-glass.
- P2.4 Agent actions carry HITL where R2.4 demands it; budgets/metering observed (flow-engine work
  from R3.7). Exit proof: one full plan-row executed end-to-end — assignment read from an issue,
  branch pushed over the wire, PR opened, CI green, founder approval via chat, merge — with a
  complete platform-native audit trail.

### P3 — Subsystem deepening, strict admission order (fills the remaining months)

Depth-before-breadth, mechanically: a subsystem is admitted for deepening only when the previous one
is **boring** — 4 consecutive weeks of daily founder use with zero new findings ≥ MED severity.
Order: **Issues → Knowledge → Chat.** (Git+CI reach "boring" via TP-1 itself.) For each: design
sketches precede frontend per VISION §3, drawing on `design-planning/`; scope is *founder-grade
daily-driver depth*, explicitly not the corporate feature matrix (SLAs, custom fields at breadth,
reporting) — those stay at their commercial-plan tier. Each subsystem's deepening ends with its own
quantified gate written into the ledger before work starts, per the pre-registration discipline.

If the P3 queue empties before the launch window (possible at demonstrated velocity): the overflow
valve is **hardening depth, not surface breadth** — mutation-testing coverage (the `cargo-mutants`
bar VISION already signals), chaos-drill variety on the production cell, performance baselines under
the 10×/30× load profiles on real hardware. Not new subsystems, not multi-cell, not GA features.

## 5. Explicitly out of Tier P (deferred with written triggers)

- Multi-cell activation, billing/Stripe, DPA/legal pack, EN 301 549 assessment, external pentest,
  design-partner recruitment, waitlist/GTM motions — **trigger: the founder's Tier B go decision.**
- Hosted agents beyond the builder itself — standing cost decision unchanged.
- The commercial plan's §10 immediate actions (trademark, waitlist, NLnet draft) remain founder-
  discretionary; nothing in the P-track depends on them.

## 6. The re-keyed commercial clock

02-commercial-plan.md remains the commercial truth, but its GTM sequence keyed promotion to R-track
completion; that coupling is dissolved. **Tier B opens on a founder decision made *after* Tier P has
held for its 8 weeks**, informed by: appetite for operating for others, support-load reality observed
during Tier P, and the state of the sovereignty market then. The build-in-public narrative can start
any time (it costs one founder-hour/week); its absence blocks nothing in the P-track. The viability
gates (02 §8) apply unchanged from Tier B onward.

## 7. Amended sequencing

```
R0 ──► R1 ──► R2 ──► R4 (dogfood cutover)          [R3 parallel, unchanged]
                        │
                        ▼
        P0 (production-for-one: cell, paging, upgrade drill, supply-chain)
                        │
                        ▼
        P1 (full personal cutover — TP-1 clock starts)
                        │
                        ▼
        P2 (builder becomes the first real agent)
                        │
                        ▼
        P3 (deepen: issues → knowledge → chat; overflow → hardening depth)
                        │
                        ▼
              Tier P held (TP-1..TP-6)  ═══ the launch ═══
                        │
                        ▼  (founder go decision, §6)
        R5 residue + R6[B] ──► Tier B ──► R6 rest ──► GA   [02 unchanged]
```

Standing ratchets L1–L5 run underneath the whole track from today.

## 8. Risks, honestly

1. **Self-hosting the founder's livelihood on a solo platform.** Mitigated by mandatory quarter-long
   mirrors (P1.1), monthly real-data restore drills (TP-2), and the rehearsed upgrade path (TP-3).
   The residual risk is accepted in writing by the per-repo opt-in.
2. **Verification debt at velocity.** 300 commits/week against one part-time human reviewer means
   the gates *are* the review. L1 (self-re-arming), L4 (traceability), and the adversarial-verifier
   standing process are the load-bearing answer; if any of them regresses, that outranks all feature
   work (order-by-non-negotiability applies to process too).
3. **The builder-as-agent recursion** (P2) makes the platform a dependency of its own development.
   Break-glass paths are named (tmux, GitHub mirror); the builder must be able to operate degraded
   (direct git push) if the platform is down — fail-static applied to the development loop itself.
4. **Drift back toward GA-chasing.** The surplus-velocity failure mode in reverse: the P3 overflow
   valve exists precisely so "done with the queue" never silently becomes "start GA features."
   Tier B work before the §6 trigger is a plan violation, not initiative.
