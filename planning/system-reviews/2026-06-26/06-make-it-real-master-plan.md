# Make It Real — Master Hardening Plan (dogfood-first)

Date: 2026-06-26. Status: PLAN. Owner: the orchestrator + founder.

The goal is no longer "make Myelin a product the world can buy." It is **"make Myelin real enough to run our own
work on it, in the order the work touches it, and no further than the work demands."** Monetization paths
(verification layer, sovereign substrate, vibe-coder PLG) stay open downstream, but they are *not* what this plan
hardens for. This plan hardens for *trustworthy dogfooding*, which is also the cheapest, most honest path to a
genuinely production-real core.

This is the master sequence. It reconciles three existing bodies of work and reorders them around the dogfood:
- the M7 prompt band `planning/07-prompts/by-system/production-readiness.md` (P-522..P-546) — the floor-filling impls + verifications;
- the M7 review pack `planning/system-reviews/2026-06-26/00..04` — the vetting overlay (gates, blackbox drills, chunking);
- the confidence campaign `…/05-confidence-campaign-plan.md` — the discovery method (census, oracle tests, design review, break-the-coupling).

---

## The three rules that make this tractable

1. **Scope = the dogfood's request path, not Myelin's surface.** You only harden the code a real request actually
   traverses. This is roughly a 10x descope of the original "verify all of Myelin" framing — and it is the
   single most important sentence in this document.
2. **Order = blast radius.** Move low-stakes surfaces first (collaboration), hardest/most-dangerous last
   (untrusted execution / CI). Never harden a surface you don't yet use — that is the substrate-instead-of-product
   trap, and it is the most likely way this kills the real work.
3. **"Real" is defined per tier by a concrete failure that would actually hurt** — lose my code, leak across the
   boundary, forge a credential, run a supply-chain hole — *prevented and independently proven*, not green-gated.
   Verification stays independent of the builder (different agent / real backend / external oracle), because the
   whole project's hardest lesson is that the agent that wrote the code can't be trusted to certify it.

---

## What this plan deliberately does NOT do (the descopes that keep it solo-tractable)

Naming the non-goals matters as much as the goals. For *internal dogfood*, the following are **out of scope** and
must not be hardened "while we're in there":
- **Multi-cell / world-scale / the 30× surge family / fleet** — you run one box. The cross-cell machinery can stay
  stubbed or be ripped out of the dogfood path.
- **Sovereignty certifications (SecNumCloud/BSI-C5), external pentest, independent audits** — deferred to the
  graduation tier (Tier 4), only if Myelin ever serves data you are liable for to others.
- **The full P-546 fail-closed *release* gate** — that gates an external production release, not internal use.
  A scoped internal version of its evidence discipline is in Tier 0; the full gate is Tier 4.
- **Five-subsystem switch-test polish** — not needed to run your own work.

Re-enter these only if/when Myelin graduates from "our infrastructure" to "touches someone else's data."

---

## Tier 0 — Foundation truth (before trusting ANY of it for real work)

The cheap, broad nets and the universal floor. Nothing above this tier is trustworthy until this is done.
- **Shortcut census, scoped to the Tier-1 surfaces** (campaign Stage 0 / Stage A): find the *silent* shortcuts in
  git/issues/knowledge/chat + their shared substrate, model-tiered (Opus on security-critical, Haiku on
  mechanical). Output: a ranked `shortcut-inventory.md` for the dogfood path.
- **Known-gap triage** (M7 audit + the `Floor named:` sweep): fix the cheap known gaps now so the census doesn't
  rehash them.
- **Evidence-integrity skeleton, built first** so nothing below can lie to you: production-graph absence scanners
  with **red fixtures**, a red-by-default gate binary, attested (not hand-editable) scorecards. This is the
  internal, scoped version of P-540/P-541.
- **Durable persistence (P-522/P-523).** The universal floor: nothing is "real" while principal/tuple/revocation
  or any load-bearing state lives in a `HashMap`. Bind the live OLTP/cache pool; prove crash/restart + restart
  over the same backend keeps state. This underlies every surface, so it comes first.
- **Kick off the shape/design review (campaign Stage D)** for the surfaces you'll harden — including the
  **mock→real agent-runtime** line item — so you don't harden an early-era shape you should be redrawing.

**Done-bar:** you have a ranked inventory of what's actually stubbed on the dogfood path; load-bearing state
survives a `kill -9` + restart; and your gates are attested, red-by-default, and proven to bite on a red fixture.

---

## Tier 1 — The collaboration substrate (move your work here first; low blast radius)

The first real dogfood. No untrusted execution, so the only thing that can really hurt you is **losing data**.
- Git hosting, issues, knowledge/docs, chat running on **durable stores** (Tier-0 persistence applied per
  subsystem).
- **Real backup + a real *destructive* restore of YOUR data** (P-529/P-530, scoped to "don't lose my code/docs"):
  a modeled WAL offset is not a backup. Prove it by restoring to a clean target and reading your repos/issues back.
- **Auth real enough for your use:** single-org, so the bar is lower — but if it's reachable beyond localhost,
  the token-crypto floor (Tier 2) applies before exposure.
- **External-oracle tests where cheap** (campaign Stage B): real `git` clone/push + `git fsck` against your repos;
  `render(parse(md)) === md` for docs. These prove "the feature is real" without trusting your own tests.

**Done-bar:** you run your team's code + issues + docs + chat on Myelin; a `kill -9` mid-write followed by restore
loses nothing; real `git` is happy with your repositories.

---

## Tier 2 — Internet-exposure hardening (once it's reachable beyond localhost)

The moment Myelin is reachable from the internet (so your team can use it), the trust boundary becomes real.
- **Real auth/token crypto (P-526/P-527/P-528):** remove `StructuralTokenVerifier`/`StructuralTokenSigner`/the
  attestation verifiers from the production graph; real signature verification; forged/expired/replayed negative
  corpus. The absence scanner from Tier 0 proves they're gone.
- **Tenant/session isolation (P-531):** `SET LOCAL` transaction-scoped RLS + reset-on-release (fixes the
  `set_config(..., false)` pooled-bleed), bare-connection guard, identifier allowlist. Even single-org, a pooled
  connection that leaks session state is a real hole once there's real data.
- **Secret handling (P-532/P-533):** redacted Debug/Error/serialization + a sentinel leak corpus through all sinks.
- **Runtime reality (P-539):** real SIGTERM drain, readiness, OTel export — so an internet-exposed service behaves.
- **Destructive restore proven (P-530)** if not already done in Tier 1.

**Done-bar:** Myelin can be exposed to the internet for your team with no credential-forgery, pooled-bleed, or
secret-leak hole, each proven by a scanner-with-red-fixture and a blackbox drill — not by a passing unit test.

---

## Tier 3 — CI / untrusted execution (the prize; last because it's hardest and most dangerous)

This is the GitHub-Actions-bill killer, and it is gated on the single most security-critical floor. CI runs your
own build + dependency code, so a weak sandbox is a supply-chain hole in *your* products.
- **Sandbox production exec (P-544):** a real `JobSpec.command` flows through the hardened production `launch()` on
  **both** Firecracker and gVisor (today they boot `init=/bin/true` / probe `runsc --version` — `spec.command`
  never runs in prod). Capture exit/stdout/stderr; timeout kills the whole guest; settle once after completion.
- **Production-path escape verification (P-545):** re-run the AG-D4 adversarial corpus **through the production
  `launch()`** on both backends, 0 escapes, with a guard that fails red if the corpus is routed to the harness
  shortcut.
- **Then, and only then,** move CI off GitHub Actions and retire the Actions bill.

**Done-bar:** real builds execute through the production-hardened sandbox; the escape corpus passes *through the
production path* with 0 escapes; you'd trust it with your own supply chain. The Actions-bill saving is the reward
that arrives *after* this work, not the thing that funds it.

---

## Tier 4 — Graduation (only if Myelin ever touches data you're liable for to others)

The bridge back to the monetization paths (sovereign substrate, customer-facing product). Not needed for internal
dogfood; **required before any external customer data.**
- HSM-class KMS + key lifecycle (P-524/P-525); secret zeroize depth.
- Supply-chain governance (P-534..P-538): SHA-pinned actions, digest-pinned images, pinned toolchain, cargo-deny,
  SBOM/provenance, SECURITY.md/CODEOWNERS.
- Independent crypto + sandbox review + third-party pentest (P-542/P-543).
- The full fail-closed **release gate (P-546)**, green only on dated, attested, independent evidence.
- Sovereignty certifications, if the sovereign/regulated path is taken.

**Done-bar:** P-546 is green on fresh, attested, independent artifacts; no structural/mock impl in the production
graph; the property-falsification map (campaign §6) has every load-bearing claim mapped to an independent gate or
a recorded human blocker.

---

## Execution discipline (threaded through every tier)

- **Break builder/verifier coupling:** the agent (or person) that verifies a floor is never the one that filled
  it; verification runs against a real backend or an external oracle.
- **Prompt-sizing:** each unit of work is chunked so its execution lands ~400k–700k tokens, never above 700k
  (`04-m7-agent-handoff-chunks.md`). The CI/sandbox tier especially must be split into safe, separately-verified
  packets.
- **Evidence first, always:** every scanner ships with a red fixture; every scorecard is attested; a green that
  can't prove it bites is not evidence.
- **The dogfood pulls the hardening.** Harden a surface when the real work reaches it, in blast-radius order.
  Hardening ahead of use is the substrate trap.

---

## The two variables that tune this plan (need the founder's answer)

1. **What you dogfood, and its request path.** This sets the scope at each tier.
   - Git + issues + docs + chat, internal/localhost → **Tiers 0–1** get you running.
   - Exposed to your team over the internet → add **Tier 2**.
   - Move CI off GitHub Actions → add **Tier 3** (the hardest).
   - Ever customer-facing / others' data → add **Tier 4**.
2. **Execution model + runway.** Solo via the batch runner (same gated-ledger method), or with help; bootstrapped
   vs. grant-funded (Sovereign Tech Fund / NLnet / NGI for the open parts). This sets pace and how aggressively to
   descope.

## Suggested first move

**Tier 0, scoped to the Tier-1 surfaces.** Census + known-gap triage + the evidence-integrity skeleton + durable
persistence. It is the cheapest work that makes the *first* real dogfood — your repos, issues, docs, and chat —
actually trustworthy, and it is the prerequisite for everything above it. Everything else waits behind a real
foundation and a real inventory of what's stubbed.
