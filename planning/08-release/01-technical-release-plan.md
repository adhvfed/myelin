# Technical Release Plan — the R-track (R0–R6)

_2026-07-06, against HEAD `d1cacb8`. Inputs: `reviews/2026-07-06/` (00-executive-summary + DELTA),
ledgers 09/11/12/13, `06-make-it-real-master-plan.md`, VISION.md. Every item cites its source finding
so nothing is invented and nothing from the review is silently dropped._

## Ordering logic

1. **Live holes before latent holes.** The CT + MR-009b commits put previously-dormant findings on live
   paths (DELTA: FC egress, git-wire N1/N2, reconciler). These come first — they are on the daily-driver
   path the founder is about to depend on.
2. **Finish what's in flight before starting new surface.** MR-009b W3b–W7 completes the durable
   substrate; every latent HIGH is scheduled *inside* the wave that would make it live, so no backend
   flips on with a known hole (DELTA priority #5).
3. **Product surface before external users.** The Git/PR UX criticals block even dogfood usefulness.
4. **Graduation gate before other people's data.** Staged, not skipped (master plan Tier 4).

Phases R0→R1→R2 are strictly ordered. R3 (product surface) can run **in parallel** with R1/R2 — it
touches frontend/edge, they touch storage/identity — and should, to keep momentum on both fronts.

---

## R0 — Stop-the-bleed: live security holes (≈1–2 weeks solo)

The daily driver must not be the founder's own breach. All four are on paths that execute today.

| # | Item | Source | Exit proof |
|---|---|---|---|
| R0.1 | **Fail-closed the Firecracker NIC**: no egress-capable NIC until a real per-tap egress firewall (nftables/eBPF on `tap-myelin`, allowlist actually applied) is emitted and attested. gVisor already safe. | ci #1, DELTA now-live HIGH | AG-D4 SSRF probe through production `launch()` with egress requested → metadata/cross-tenant IPs unreachable; attestation records the enforced ruleset. |
| R0.2 | **Wire push respects the merge-gate + per-repo branch-protection ruleset**: replace `PushPolicy::default()` hardcoding with the same policy evaluation the PR merge path uses; protected branches come from repo config, not a literal list. | DELTA N1 (HIGH) | Adversarial test: principal with `git.wire.receive_pack` pushes to `main` with CI-red → REJECTED over the wire; force-push/delete on any configured protected branch rejected. |
| R0.3 | **Per-repo object authorization on the wire routes** (read and write): the wire must consult the same per-repo grant seam as the product API before serving pack data. This *seeds* the platform-wide object-authz seam completed in R2 — build the seam here, template-correct. | DELTA N2 (HIGH), xc-tenancy | Adversarial test: principal in-tenant without a grant on repo X → clone/fetch/push of X denied, 0-leak. |
| R0.4 | **Git crash reconciler: durable monotonic generation instead of reflog-length `update_seq`.** | git #1 (HIGH) | Delete+recreate a branch, crash, restart → reconciler converges; burst-crash ordering test no longer leaves ref DELETED. |
| R0.5 | Bound the wire HTTP request body at the front door (stream with cap, 413 over limit) — quick, same files as R0.2/0.3. | DELTA N3 | Oversized push rejected without host RAM growth. |
| R0.6 | **Dev-login environment guard**: refuse to mint sessions unless an explicit `MYELIN_DEV_LOGIN=1`-style flag AND non-production build; log loudly. Cheap now, catastrophic if forgotten at exposure time. | fe-web (auth bypass) | Prod-config boot + dev-login attempt → refused + audit event. |

Also in R0, zero-cost hygiene batch (one prompt): shallow-push connectivity check via `rev-list`-style
ancestry walk or quarantine fsck (DELTA N4), `digest_pinned` length check (ci low), config `Debug`
secret redaction + CLI token chmod-before-write (support/edge lows).

## R1 — Finish MR-009b: durable substrate to baseline 0 (≈2–4 weeks solo)

Execute ledger 13 as written — W3b (outbox retirement), W5 (KMS, high blast radius), W6a–d (build-then-
wire: pseudonym/cost/erasure/bus ledgers, control-plane placement backing), W7 (blob bytes + CT-004b CI
slice + scanner blind-spots). **Amendments from the review, folded into the waves so latent HIGHs close
before their backend flips live:**

| Wave | Fold in | Source |
|---|---|---|
| W6b (storage ErasureLedger) | **Erasure ledger records COMPLETION time, not submission time**, before it becomes the durable system of record; add the restore-inside-window resurrection test. | gdpr #1 (HIGH, "gravest failure") |
| W6b/W6 | git DSR receipts stop hardcoding `holders_hit = ALL`; DSR completion reconciles receipts against the data-map (mapped-but-unregistered holder → NotGreen). | xc-gdpr, themes |
| W6 (any PG wave) | Kill the latent SQL-interpolation shapes while touching the crates: storage rls predicate_sql, knowledge block_tree, migration TRUNCATE classifier — binds/quoting even where model-only. | storage/knowledge lows, theme |
| W5 (KMS) | Keep ledger-13 risk #2 discipline: independent verifier classifies all ~90 construction sites; boot fails LOUD on missing durable KMS config. | ledger 13 |
| W7 | Region-scope correctness sweep: identity PG path honors `scope.region()` (now live, DELTA); refs/gdpr hardcoded fr-par partitions parameterized. | identity #b, gdpr residency |

Exit gate: scanner `no-in-memory-durable-store` baseline **0**; `cargo test --workspace` stays DB-free;
`--features integration` green on live stack; kill-9 drills pass on every newly-flipped store.

## R2 — Authorization completion: remove AllowAll honestly (≈2–3 weeks solo)

The review's deepest architectural finding: action-scoped-only authz, propagated by template. This phase
makes object-level authz real and then removes the AllowAll authorizer — in that order (DELTA priority #2).

| # | Item | Source |
|---|---|---|
| R2.1 | **Object-level (relationship) authorization at the edge**: extend the R0.3 seam platform-wide — edge re-authorization takes (action, object) and consults the tuple store; `git_edge` template corrected first so every subsystem copies the fixed shape. **Includes wiring R0.2/R0.3 LIVE** (R0 verifier finding): production `main.rs` must inject a real grant-backed `RepoAuthorizer` (not the `AllowAllRepos` default) and `register_git_wire` in the production gateway — until then the R0.2 branch-protection gate and R0.3 per-repo authz are correct but latent. | xc-tenancy HIGH, R0 verifier |
| R2.2 | Identity `check()` authorizes on fully-qualified object, not bare trailing id; query `EventMatcher` and SSE scope get the same object-qualification treatment. | identity #a, themes |
| R2.3 | **Fail-static authz cache: full-key comparison** (no 64-bit-hash aliasing). | substrate HIGH |
| R2.4 | **MCP HITL**: approval is a server-side verdict looked up by the gate, never a caller-supplied boolean; batch partial-approval applies effects by approval-id, not tool name. | mcp/agent HIGHs |
| R2.5 | **Real human login wired** (OIDC path from the MR spine) at the edge; SSO RefuseUnsupported stays for unimplemented providers; dev-login now dead in prod builds (R0.6 guard becomes structural). | firstrun, fe-web |
| R2.6 | **Remove AllowAll from `main.rs`**; boot refuses to start with a permissive authorizer outside test-support. Scanner-style lint added so it cannot return. | exec summary rec (2) |
| R2.7 | Search vector-path ACL: `AclFilter::admits` matches `doc_id OR acl_object` like the lexical clause; deny-set leak test both directions. | search HIGH |

Exit gate: adversarial verifier campaign — a dedicated red-team subagent per subsystem tries intra-tenant
object reach-around through edge, wire, MCP, SSE, and search; all denied; AllowAll gone.

## R3 — Flagship product surface: Git/PR UX + first-run (≈3–5 weeks solo; parallel with R1/R2)

The three review criticals and the daily-driver blockers. Design sketches precede code (VISION §3).

| # | Item | Source |
|---|---|---|
| R3.1 | **PR list + navigation front door**: PRs listed per-repo and cross-repo ("my PRs / needs my review"); linked from repo page and nav. | ux-git critical 1 |
| R3.2 | **PR diff / files-changed view**: side-by-side + unified, per-file, with comment anchoring. This is the single highest-value screen in the product. | ux-git critical 2 |
| R3.3 | **PR context pane**: description, linked issue/run/doc (the refs graph is built — surface it), discussion, commits. This is the differentiator screen — the cross-artifact story made visible. | ux-git critical 3 |
| R3.4 | Repo browsing completeness: subdirectory navigation, README render, branch/tag switcher; kill the four nav-destination 404s (real pages or remove the destinations). | ux-git highs |
| R3.5 | First-run flow: login → create/join tenant → create repo → push (copy-paste instructions with the wire URL) → first CI run. One continuous path, empty states designed. | ux-firstrun |
| R3.6 | A11y AA batch: command-palette focus ring, nav-rail accent, commit-link contrast, plus the design-system findings (fe-ds). EN 301 549 is a *sales asset* for the EU public-sector ICP — treat as product, not polish. | ux-a11y, fe-ds |
| R3.7 | GT-004b (PR review/merge UI follow-up) and flow-engine budget leak (reservation released on retry-exhaustion; reserve/settle inside the FLOW-D5 co-commit) — the latter before CI metering is real money. | ledger follow-ups, flow HIGH |

Exit gate: **the founder reviews and merges a real Myelin PR entirely inside Myelin**, from notification
to diff to merge, and axe/Playwright a11y suite green on the PR surfaces.

## R4 — Dogfood cutover (Tier D) (≈1–2 weeks, then continuous)

The reward and the real test. Master-plan Tier 1/3 completion:

- R4.1 Myelin repo mirrored into Myelin; founder's daily push/pull/PR flow moves over (GitHub kept as
  read-only mirror for a full quarter — bus-factor honesty).
- R4.2 **CT-007: cut CI over from GitHub Actions** (CT-003 attestation already green for gVisor; FC gated
  on R0.1). CT-004/CT-005 (CI backend harden + API/UI/CLI/MCP) execute from ledger 12 if not done by now.
- R4.3 Backup/restore drill on the real dogfood data to a clean target (master plan Tier 0 promise);
  scheduled repeating drill, not a one-off.
- R4.4 Run a **finding-burndown loop**: every rough edge the founder hits becomes an issue *in Myelin's
  own tracker* (issues subsystem gets exercised for free).

Exit gate: 4 consecutive weeks where the founder never needed GitHub for daily work.

> **Amendment 2026-07-22:** everything below this line is re-scoped by
> [03-personal-production-plan.md](03-personal-production-plan.md) (the P-track). R5.1–R5.3/R5.5 execute
> inside P0 sized for one tenant; R6.2/R6.6 are promoted into Tier P; the remaining R5/R6 items are
> deferred behind the Tier B go decision (doc 03 §6). R0–R4 above are unchanged and remain in flight.

## R5 — Production operations (Tier B enabler) (≈3–4 weeks solo)

What "hosted" means before anyone else's data arrives:

- R5.1 **EU production deploy**: single-region managed-infra start (Scaleway fr-par or Hetzner —
  matches the existing fr-par assumptions), IaC'd, with the docker-stack dependencies (PG/Valkey/NATS/S3)
  as managed services. One production cell; the multi-cell architecture waits for demand.
- R5.2 Observability: metrics/alerts on the golden signals + the platform's own attestation gates as
  alerts (a red absence-scanner or failed drill pages the founder). Status page (hosted external).
- R5.3 Upgrade + migration path proven: blue-green or documented-downtime deploys; schema migrations
  rehearsed on a production snapshot.
- R5.4 Self-host packaging floor: `docker compose up` single-node evaluation mode, versioned releases.
  (Sales asset for the sovereignty ICP; also the design-partner escape hatch — reduces their risk of
  depending on a solo operator.)
- R5.5 Ops runbook + incident comms template; TLS/edge exposure hardening (master plan Tier 2: DPoP
  binding + resource limits follow-ups from the ledger).

Exit gate: a stranger-shaped tenant (founder's second identity) onboards on production without founder
intervention; one full disaster-recovery drill from backups on production.

## R6 — Graduation gate (Tier B subset `B`, rest for GA)

Staged-not-skipped (direction memory; master plan Tier 4). Marked **B** = required before design-partner
data; unmarked = required before paid GA.

- R6.1 **[B]** Design-partner agreement (data-processing terms, no-SLA honesty, exit/export promise —
  GDPR portability is already an engine feature, make it a contract feature).
- R6.2 **[B]** Supply-chain floor: `cargo audit`/`cargo deny` in CI, lockfile-pinned, SBOM emitted,
  signed releases.
- R6.3 External penetration test (scoped: edge, wire, sandbox, MCP) + fix window. Budgeted (see
  commercial plan); scheduled after R2 so the money isn't spent finding AllowAll.
- R6.4 KMS posture for v1: documented software-sealed KMS with root-secret handling procedure; HSM
  deferred with a written trigger (first customer requiring it / first compliance audit demanding it).
- R6.5 GDPR paper pack: records of processing, subprocessor list (the managed-infra providers), DPA
  template, privacy policy, retention schedule — generated *from* the data-map the engine already
  maintains (turn compute-but-don't-enforce on its head: enforce-and-then-attest becomes a document).
- R6.6 **P-546 fail-closed release gate** wired into Myelin's own CI: a release cannot be cut with a red
  attestation, open critical finding, or scanner regression.
- R6.7 EN 301 549 self-assessment doc (audit later, when a public-sector deal warrants the cost).

Exit gate for GA: R6 complete + commercial plan's viability gates (doc 02) met at Tier B.

---

## What is explicitly deferred (named floors, per VISION §3)

- **Chat, knowledge, issues beyond dogfood level** — issues gets exercised in R4 and hardened
  opportunistically; chat + knowledge remain post-GA tracks (direction: Git → CI → chat → issues → docs;
  GA scope is deliberately Git+CI+Issues — see commercial plan §GA-scope).
- **Hosted agents** — deferred for cost (standing decision); local-Claude-via-CLI/MCP is the agent story
  at GA, and it is *sufficient* for the agent-native positioning because MCP governance (R2.4) is real.
- **Multi-cell / world-scale activation** — architecture stays, second cell waits for a paying reason.
- **HSM KMS, sovereignty certifications, independent SOC2-style audit** — triggers written in R6.4/R6.7.
- **Tauri mobile** — post-GA.

## Sequencing summary

```
R0 (live security)          ──►  R1 (MR-009b W3b–W7)  ──►  R2 (authz completion)
        │                                                        │
        └──►  R3 (Git/PR UX, parallel)  ─────────────────────────┤
                                                                 ▼
                            R4 (dogfood cutover, Tier D)
                                                                 ▼
                            R5 (production ops)  ──►  R6[B] ──► Tier B (design partners)
                                                                 ▼
                                            R6 (rest) + viability gates ──► GA
```

Honest solo-pace estimate (using demonstrated MR/GT/CT throughput): Tier D around **Sep 2026**, Tier B
around **Nov–Dec 2026**, GA around **Q1–Q2 2027**. Dates are planning instruments, not promises; the
gates are the truth.
