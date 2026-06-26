# Make It Real — Master Plan (full package, sequenced, dogfood-driven)

Date: 2026-06-26. Status: PLAN. Owner: the orchestrator + founder.

The goal is to build **the full Myelin package, properly** — Git + Actions, team chat, issue tracker,
docs/knowledge base, the CLI/MCP that lets a local agent drive it, and the UI (web + app) to use it — and to
**sequence** the work so Myelin becomes a genuinely useful daily driver early, then dogfood it daily for ~6 months
and let organic adoption follow (a few users → word of mouth → the ball rolls). Nothing is cut from the
destination; sequencing decides only the *order* in which each part is made real. Hosted agents are deferred for
now (cost), with the local-Claude-via-CLI/MCP path standing in. Daily dogfooding by a builder who won't stop until
it's good is the forcing-function that makes this also the most honest path to a production-real core.

This is the master sequence. It reconciles three existing bodies of work:
- the M7 prompt band `planning/07-prompts/by-system/production-readiness.md` (P-522..P-546) — the floor-filling impls + verifications;
- the M7 review pack `planning/system-reviews/2026-06-26/00..04` — the vetting overlay (gates, blackbox drills, chunking);
- the confidence campaign `…/05-confidence-campaign-plan.md` — the discovery method (census, oracle tests, design review, break-the-coupling).

---

## The three rules

1. **Harden in the order the work reaches it.** The full package is the destination; you make each surface
   production-real when the dogfood is about to lean on it, not before. This is sequencing, not amputation — it
   keeps you from polishing a surface no one uses yet (the substrate-instead-of-product trap) without cutting
   anything from the eventual whole.
2. **Order by blast radius within that.** Where priorities allow, make low-stakes surfaces real first
   (collaboration), hardest/most-dangerous last (untrusted execution / CI).
3. **"Real" = a concrete failure that would actually hurt** — lose my code, leak across the boundary, forge a
   credential, run a supply-chain hole — *prevented and independently proven*, not green-gated. Verification stays
   independent of the builder (different agent / real backend / external oracle), because the project's hardest
   lesson is that the agent that wrote the code can't certify it.

---

## What is deferred and sequenced (nothing is cut from the destination)

The full package is the goal. These are *later*, not *never* — sequenced behind the work that makes Myelin useful
to dogfood first:
- **Hosted agents / the real `LlmAgentRuntime`** — deferred for cost. Near-term, your local Claude drives Myelin
  through the CLI/MCP layer (below), so agent *interaction* ships now; agent *hosting* comes when it's worth the
  spend. The mock→real-runtime shape review (Tier 0) still happens so the seam is right when you switch it on.
- **Multi-cell / world-scale / the 30× surge family / fleet** — deferred until real scale (more than one box,
  many users) demands it. The machinery exists; it just isn't on the critical path to a usable daily driver.
- **The graduation tier (HSM-KMS, supply-chain governance, independent reviews + pentest, the full P-546 release
  gate, sovereignty certs)** — sequenced to when users arrive, which is explicitly on the roadmap. The moment
  Myelin holds someone else's data, Tier 4 is required, not optional. It's staged, not skipped.

The **UI (web + app)** and the **CLI/MCP** are *not* deferred — they're first-class, because "useful to dogfood"
and "useful to the first curious users" both require them.

---

## The two product-surface workstreams (first-class, threaded through every subsystem)

Not a tier — they advance alongside each subsystem as it's made real.

**The CLI + MCP server (agent-operability).** A `myelin` CLI for scriptable/human use, and — the important half —
an **MCP server** so a local agent (Claude Code on your machine) drives Myelin's git, CI, issues, docs, and chat
as native tools. This is the near-term answer to "agents, but not hosted yet": Claude acts as a principal through
the CLI/MCP under the same auth + audit as a human, so agent *governance* (who/what did what, with what authority)
is real from day one even though agent *hosting* is deferred. It's also the "substrate an agent can operate
natively, cold" differentiator. Built per-subsystem alongside that subsystem's surface.

**The UI (web + app).** A web UI usable from your laptop and an app, built on the frozen design system
(`design-planning/08-design-system/`, React + React-Aria + Style Dictionary). Each subsystem ships its UI as part
of "made real" — you won't use a daily driver without a real interface, and it's what the first curious users
judge on sight. Real, substantial work; advances per-subsystem in the priority order below.

---

## Sequencing: a shared spine, then subsystems in priority order

The horizontal tiers (T0–T4) are the *hardening model* every surface passes through. The build sequences them:

**Spine first (serves everything):** Tier 0 (census + evidence skeleton + durable persistence + shape review) +
the Tier-2 auth/tenant-isolation floor (the moment a UI and your machine talk to it, it's an exposed trust
boundary) + the UI shell + the CLI/MCP substrate. Nothing subsystem-specific is trustworthy until the spine is
real.

**Then each subsystem, in your priority order, through its relevant tiers + its UI + its CLI/MCP surface:**
1. **Git** — hosting on durable stores, real *destructive* backup/restore of your repos, git UI, git CLI/MCP. Low
   blast radius, highest priority, first. It's the home everything else hangs off.
2. **Actions / CI** — *the long pole.* CI is Tier 3: it needs the sandbox **production** exec floor (P-544/545)
   made real before it can run your builds safely, because CI runs your own build + dependency code and a weak
   sandbox is a supply-chain hole in your products. So Git becomes usable fast while CI hardens on its **own
   track** in parallel. Don't let the hardest subsystem block the useful one; don't move off GitHub Actions until
   the sandbox is genuinely hardened.
3. **Team chat** — durable + real-time, UI + app, CLI/MCP.
4. **Issue tracker** — tracker + UI + CLI/MCP.
5. **Docs / knowledge base** — editor + UI + CLI/MCP.

The one collision: *Git* is the easiest/safest to make real and *Actions* is the hardest/most-dangerous, yet both
are priority-one. The resolution is to ship Git as a daily driver quickly and treat CI/sandbox as a deliberate,
separately-verified parallel track that lands when it's actually safe.

---

## Tier 0 — Foundation truth (the spine; before trusting ANY surface)

- **Shortcut census, scoped to the Git-first surfaces** (campaign Stage 0): find the *silent* stubs, model-tiered
  (Opus on security-critical, Haiku on mechanical). Output: a ranked `shortcut-inventory.md`.
- **Known-gap triage** (M7 audit + `Floor named:` sweep): fix the cheap known gaps now.
- **Evidence-integrity skeleton, first** so nothing below can lie to you: production-graph absence scanners with
  **red fixtures**, a red-by-default gate binary, attested (not hand-editable) scorecards.
- **Durable persistence (P-522/P-523):** nothing is real while load-bearing state lives in a `HashMap`. Bind the
  live OLTP/cache pool; prove crash/restart keeps state. Underlies every surface, so first.
- **Shape/design review kickoff (campaign Stage D)**, including the **mock→real agent-runtime** seam, so you don't
  harden an early-era shape you should be redrawing.

**Done-bar:** ranked inventory of what's actually stubbed; load-bearing state survives `kill -9` + restart; gates
are attested, red-by-default, and proven to bite on a red fixture.

---

## Tier 1 — Make a subsystem a real daily driver (start with Git)

Per subsystem (Git first), no untrusted execution, so the thing that can hurt you is **losing data**.
- The subsystem on **durable stores**; its **UI** (web + app) and its **CLI/MCP** surface.
- **Real backup + a real *destructive* restore of YOUR data** (P-529/P-530, scoped): a modeled WAL offset is not a
  backup. Prove it by restoring to a clean target and reading your repos/issues/docs back.
- **External-oracle tests where cheap** (campaign Stage B): real `git` clone/push + `git fsck`; `render(parse(md))
  === md` for docs — proves the feature is real without trusting your own tests.

**Done-bar:** you use the subsystem for real every day through its UI and CLI; a `kill -9` mid-write + restore
loses nothing; the external oracle (real `git`, etc.) is satisfied.

---

## Tier 2 — Internet-exposure hardening (part of the spine; once your team can reach it)

- **Real auth/token crypto (P-526/P-527/P-528):** remove `StructuralTokenVerifier`/`StructuralTokenSigner`/the
  attestation verifiers from the production graph; real signature verification; forged/expired/replayed corpus.
- **Tenant/session isolation (P-531):** `SET LOCAL` transaction-scoped RLS + reset-on-release (fixes the
  `set_config(..., false)` pooled-bleed), bare-connection guard, identifier allowlist.
- **Secret handling (P-532/P-533):** redacted Debug/Error/serialization + a sentinel leak corpus through all sinks.
- **Runtime reality (P-539):** real SIGTERM drain, readiness, OTel export.

**Done-bar:** exposable to the internet for your team with no credential-forgery, pooled-bleed, or secret-leak
hole — each proven by a scanner-with-red-fixture and a blackbox drill, not a passing unit test.

---

## Tier 3 — Actions / CI (the long pole; the GitHub-Actions-bill killer)

Gated on the most security-critical floor; CI runs your own build + dependency code.
- **Sandbox production exec (P-544):** a real `JobSpec.command` flows through the hardened production `launch()` on
  **both** Firecracker and gVisor (today they boot `init=/bin/true` / probe `runsc --version`; `spec.command`
  never runs in prod). Capture exit/stdout/stderr; timeout kills the whole guest; settle once after completion.
- **Production-path escape verification (P-545):** re-run the AG-D4 corpus **through the production `launch()`** on
  both backends, 0 escapes, with a guard that fails red if routed to the harness shortcut.
- **Then** move CI off GitHub Actions and retire the bill — the reward *after* the work, not what funds it.

**Done-bar:** real builds execute through the production-hardened sandbox; the escape corpus passes through the
prod path with 0 escapes; you'd trust it with your own supply chain.

---

## Tier 4 — Graduation (staged for when users arrive — and they're on the roadmap)

Required before Myelin holds anyone else's data. The bridge to the monetization paths.
- HSM-class KMS + key lifecycle (P-524/P-525); secret zeroize depth.
- Supply-chain governance (P-534..P-538): SHA-pinned actions, digest-pinned images, pinned toolchain, cargo-deny,
  SBOM/provenance, SECURITY.md/CODEOWNERS.
- Independent crypto + sandbox review + third-party pentest (P-542/P-543).
- The full fail-closed **release gate (P-546)**, green only on dated, attested, independent evidence.
- Sovereignty certifications, if/when that path is taken.

**Done-bar:** P-546 green on fresh, attested, independent artifacts; no structural/mock impl in the production
graph; the property-falsification map (campaign §6) complete.

---

## Execution discipline (threaded through every tier and subsystem)

- **Break builder/verifier coupling:** whoever verifies a floor never filled it; verification runs against a real
  backend or an external oracle.
- **Prompt-sizing:** each unit of work chunked so its execution lands ~400k–700k tokens, never above 700k
  (`04-m7-agent-handoff-chunks.md`). The CI/sandbox and the UI work especially split into safe, separately-verified
  packets.
- **Evidence first:** every scanner ships with a red fixture; every scorecard is attested; a green that can't prove
  it bites is not evidence.
- **The dogfood pulls the hardening.** Make a surface real when the daily driver reaches it, in priority order;
  never harden ahead of use.

---

## The variables that tune this plan

**Settled (founder's call, 2026-06-26):** build the *full* package, properly; priority order
**Git → Actions → team chat → issue tracker → docs/knowledge**; hosted agents deferred (local-Claude-via-CLI/MCP
stands in); **UI (web + app) and CLI/MCP are first-class**; users are on the roadmap (so Tier 4 is staged, not
skipped). The destination is the whole thing.

**Still open — execution model + pace:** solo via the same gated batch-runner method that built Myelin, or with
help; bootstrapped vs. grant-funded (Sovereign Tech Fund / NLnet / NGI fit the open parts). This sets the cadence
and how many subsystem tracks run at once — not whether anything gets built.

## Suggested first move

**Build the spine, then Git.** Tier 0 (census + known-gap triage + evidence-integrity skeleton + durable
persistence + the shape review) plus the auth/tenant-isolation floor, the UI shell, and the CLI/MCP substrate —
then make **Git** a real daily driver on top of it (durable hosting + destructive restore + git UI + git CLI/MCP).
That's the shortest path to using Myelin for real every day, and everything else — Actions, chat, issues, docs —
hangs off that spine. CI/sandbox starts as a parallel hardening track the moment Git is moving.
